//! The `tag-delta` builder: a copy-and-insert delta of one asset
//! (docs/engine/asset-store.md, "Mods and packages").
//!
//! The item's source is a BPS patch (`content::patch::Bps`) from the base
//! asset's **image** to the changed asset's. An image is the asset as the
//! build holds it, with nothing that depends on where it sits ([`image`]):
//! its arrays in tree order, each its bytes and the fields the builder
//! fills in, by what they name (tags by group and path, string IDs by
//! text, raw resources by the hash of their bytes or by the cache and
//! offset they sit at). A patch holds only the bytes the author changed:
//! the rest are ranges of the user's own asset.
//!
//! The item names its target by `tag` (the key; the group is the second
//! extension of its source, `deltas\pistol.jpt!.bps`) and by `was`, the
//! asset hash of the base ([`crate::tag_hash`]). `was` must be the hash the
//! build first found the asset at, or, over an asset an earlier package
//! overrides whole, that override's hash, with the package requiring it
//! ([`crate::Cx::check_base`]). The patch's own source size and CRC32 must then
//! match the image, so a patch made on another image is refused too. A
//! patch names no raw resource the map does not hold: new resources are an
//! authored item's.
//!
//! A delta is for a change no `set` or `block` line can say. The tree it
//! writes is the author's, so the image is read back with every length,
//! index and alignment checked ([`unimage`]); a changed asset owns its
//! arrays (it keeps no sharing key). A mismatch is a refusal
//! ([`crate::Cx::refuse`]).
use std::collections::HashMap;
use std::path::Path;

use content::Sha256;
use content::patch::Bps;
use map::build::{Block, BspField, Fixup, FixupKind, RawId, RawRef, TagSet};
use map::{FourCC, ResourceFile, TagId};
use tags::defs::parse_fourcc;

use crate::build::{Cx, Failure};
use crate::builders::Job;

const MAGIC: &[u8; 4] = b"H2TI";
const VERSION: u32 = 1;
/// The largest alignment an array of the image may ask for.
const MAX_ALIGN: u32 = 0x1_0000;
/// How far an array may sit under the asset's root: the deepest a tag's
/// arrays nest, with room.
const MAX_DEPTH: usize = 64;
/// How much larger than twice its base a patch may make an asset.
const MAX_GROWN: u64 = 64 << 20;

// ---------------------------------------------------------------- the image

/// A tag reference or datum, as the build names it: `TagId::NULL`, a tag by
/// its group and path, or a slot that has no name by its datum.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Ref {
    Null,
    Name { group: u32, path: String },
    Datum(u32),
}

/// A raw resource, as the build names it: by the hash of its bytes (a
/// resource of the set), by the cache and offset it sits at, or none.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Raw {
    ByHash([u8; 32]),
    At { file: u8, offset: u32 },
    Nothing,
}

/// One array on the way in or out: each `(offset, kind, a, b, c)` says what
/// one field names (see [`walk`] and [`assemble`]).
struct Node {
    align: u32,
    phase: u32,
    sort: u32,
    fixups: Vec<(u32, u8, u32, u32, u32)>,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Writer {
    strings: Vec<String>,
    string_at: HashMap<String, u32>,
    refs: Vec<Ref>,
    ref_at: HashMap<Ref, u32>,
    raws: Vec<Raw>,
    raw_at: HashMap<Raw, u32>,
    nodes: Vec<Node>,
}

fn intern<T: Clone + Eq + std::hash::Hash>(
    list: &mut Vec<T>,
    at: &mut HashMap<T, u32>,
    v: T,
) -> u32 {
    if let Some(i) = at.get(&v) {
        return *i;
    }
    let i = list.len() as u32;
    at.insert(v.clone(), i);
    list.push(v);
    i
}

fn reference(set: &TagSet, id: TagId) -> Ref {
    let Some(tag) = set.tag(id) else {
        return Ref::Datum(id.0);
    };
    match set.slots.get(id.index()).and_then(|s| s.name.as_deref()) {
        Some(path) => Ref::Name {
            group: tag.group.as_u32(),
            path: path.to_owned(),
        },
        None => Ref::Datum(id.0),
    }
}

fn resource(set: &TagSet, r: RawRef) -> Result<Raw, String> {
    Ok(match r {
        RawRef::Null => Raw::Nothing,
        RawRef::External { file, offset } => Raw::At {
            file: file_code(file),
            offset,
        },
        RawRef::Local(id) => {
            let blob = set
                .raw
                .get(id.0)
                .ok_or_else(|| format!("a raw resource (#{}) the set does not hold", id.0))?;
            Raw::ByHash(Sha256::of(&blob.bytes).0)
        }
    })
}

fn file_code(f: ResourceFile) -> u8 {
    match f {
        ResourceFile::Local => 0,
        ResourceFile::MainMenu => 1,
        ResourceFile::Shared => 2,
        ResourceFile::SinglePlayerShared => 3,
    }
}

fn resource_file(code: u8) -> Result<ResourceFile, String> {
    Ok(match code {
        0 => ResourceFile::Local,
        1 => ResourceFile::MainMenu,
        2 => ResourceFile::Shared,
        3 => ResourceFile::SinglePlayerShared,
        _ => return Err(format!("{code} is not a cache a raw resource sits in")),
    })
}

fn bsp_code(f: BspField) -> u32 {
    match f {
        BspField::FileOffset => 0,
        BspField::Size => 1,
        BspField::Address => 2,
    }
}

fn bsp_field(code: u32) -> Result<BspField, String> {
    Ok(match code {
        0 => BspField::FileOffset,
        1 => BspField::Size,
        2 => BspField::Address,
        _ => return Err(format!("{code} is not a BSP descriptor field")),
    })
}

fn put(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Write `b` and the arrays under it to `w`, in tree order (a parent, then
/// each array a field names, in field order). Its place in `w` is returned.
fn walk(w: &mut Writer, set: &TagSet, block: &Block) -> Result<u32, String> {
    let me = w.nodes.len() as u32;
    w.nodes.push(Node {
        align: block.align,
        phase: block.phase,
        sort: block.sort_by_string_id.unwrap_or(0),
        fixups: Vec::new(),
        bytes: block.bytes.clone(),
    });
    let mut fixups = Vec::with_capacity(block.fixups.len());
    for f in &block.fixups {
        let tag = |w: &mut Writer, id: TagId| {
            let r = reference(set, id);
            if let Ref::Name { path, .. } = &r {
                intern(&mut w.strings, &mut w.string_at, path.clone());
            }
            intern(&mut w.refs, &mut w.ref_at, r)
        };
        let (kind, a, b, c) = match &f.kind {
            FixupKind::Block { count, child } => (1, *count, walk(w, set, child)?, 0),
            FixupKind::Data(child) => (2, 0, walk(w, set, child)?, 0),
            FixupKind::Pointer(child) => (3, 0, walk(w, set, child)?, 0),
            FixupKind::TagRef(id) => (4, tag(w, *id), 0, 0),
            FixupKind::TagId(id) => (5, tag(w, *id), 0, 0),
            FixupKind::StringId(s) => {
                (6, intern(&mut w.strings, &mut w.string_at, s.clone()), 0, 0)
            }
            FixupKind::Raw(r) => (
                7,
                intern(&mut w.raws, &mut w.raw_at, resource(set, *r)?),
                0,
                0,
            ),
            FixupKind::Bsp { section, field } => (8, *section as u32, bsp_code(*field), 0),
            FixupKind::TagOffset {
                count,
                tag: id,
                offset,
            } => (9, *count, tag(w, *id), *offset),
        };
        fixups.push((f.offset, kind, a, b, c));
    }
    w.nodes[me as usize].fixups = fixups;
    Ok(me)
}

/// The image of the tag whose block tree is `root`, in `set`: its bytes,
/// arrays and fields (module docs), the same for the same asset in the same
/// map. An error names a raw resource the set does not hold.
pub fn image(set: &TagSet, root: &Block) -> Result<Vec<u8>, String> {
    let mut w = Writer::default();
    walk(&mut w, set, root)?;
    let mut out = MAGIC.to_vec();
    put(&mut out, VERSION);
    put(&mut out, w.strings.len() as u32);
    for s in &w.strings {
        put(&mut out, s.len() as u32);
        out.extend_from_slice(s.as_bytes());
    }
    put(&mut out, w.refs.len() as u32);
    for r in &w.refs {
        match r {
            Ref::Null => out.push(0),
            Ref::Name { group, path } => {
                out.push(1);
                put(&mut out, *group);
                put(&mut out, w.string_at[path]);
            }
            Ref::Datum(d) => {
                out.push(2);
                put(&mut out, *d);
            }
        }
    }
    put(&mut out, w.raws.len() as u32);
    for r in &w.raws {
        match r {
            Raw::ByHash(h) => {
                out.push(0);
                out.extend_from_slice(h);
            }
            Raw::At { file, offset } => {
                out.push(1);
                out.push(*file);
                put(&mut out, *offset);
            }
            Raw::Nothing => out.push(2),
        }
    }
    put(&mut out, w.nodes.len() as u32);
    for n in &w.nodes {
        put(&mut out, n.align);
        put(&mut out, n.phase);
        put(&mut out, n.sort);
        put(&mut out, n.bytes.len() as u32);
        put(&mut out, n.fixups.len() as u32);
        for (offset, kind, a, b, c) in &n.fixups {
            put(&mut out, *offset);
            out.push(*kind);
            put(&mut out, *a);
            put(&mut out, *b);
            put(&mut out, *c);
        }
    }
    for n in &w.nodes {
        out.extend_from_slice(&n.bytes);
    }
    Ok(out)
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .ok_or_else(|| "a length of the image overflows".to_owned())?;
        let s = self
            .b
            .get(self.at..end)
            .ok_or_else(|| "the image is truncated".to_owned())?;
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    /// A count of entries that take at least `min` bytes each: never more
    /// than the bytes left could hold, so no length is trusted.
    fn count(&mut self, min: usize) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n > (self.b.len() - self.at) / min {
            return Err("more entries than the image holds".into());
        }
        Ok(n)
    }
}

/// The bytes a field of kind `kind` takes in its array ([`FixupKind`]'s).
fn width(kind: u8) -> u64 {
    match kind {
        1 | 2 | 4 | 9 => 8,
        _ => 4,
    }
}

/// The tag an [`image`] wrote, for `set`. Every length, index and alignment
/// is checked; a reference or a raw resource the map does not hold is an
/// error naming it. The arrays keep no sharing key.
pub fn unimage(set: &TagSet, bytes: &[u8]) -> Result<Block, String> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(4)? != MAGIC {
        return Err("not a tag image".into());
    }
    let version = r.u32()?;
    if version != VERSION {
        return Err(format!(
            "image version {version}, this build reads {VERSION}"
        ));
    }
    let n = r.count(4)?;
    let mut strings: Vec<String> = Vec::with_capacity(n);
    for _ in 0..n {
        let len = r.u32()? as usize;
        let s = std::str::from_utf8(r.take(len)?)
            .map_err(|_| "a string of the image is not UTF-8".to_owned())?;
        strings.push(s.to_owned());
    }
    let text = |i: u32| -> Result<String, String> {
        strings
            .get(i as usize)
            .cloned()
            .ok_or_else(|| "a string the image's table does not hold".to_owned())
    };
    let n = r.count(1)?;
    let mut refs = Vec::with_capacity(n);
    for _ in 0..n {
        refs.push(match r.u8()? {
            0 => Ref::Null,
            1 => Ref::Name {
                group: r.u32()?,
                path: text(r.u32()?)?,
            },
            2 => Ref::Datum(r.u32()?),
            _ => return Err("an unknown reference kind in the image".into()),
        });
    }
    let n = r.count(1)?;
    let mut raws: Vec<Raw> = Vec::with_capacity(n);
    for _ in 0..n {
        raws.push(match r.u8()? {
            0 => Raw::ByHash(r.take(32)?.try_into().expect("32 bytes")),
            1 => Raw::At {
                file: r.u8()?,
                offset: r.u32()?,
            },
            2 => Raw::Nothing,
            _ => return Err("an unknown raw resource kind in the image".into()),
        });
    }
    let n = r.count(20)?;
    let mut nodes: Vec<Node> = Vec::with_capacity(n);
    let mut lens: Vec<usize> = Vec::with_capacity(n);
    for _ in 0..n {
        let align = r.u32()?;
        let phase = r.u32()?;
        let sort = r.u32()?;
        lens.push(r.u32()? as usize);
        let fixups = r.count(17)?;
        let mut list = Vec::with_capacity(fixups);
        for _ in 0..fixups {
            let offset = r.u32()?;
            let kind = r.u8()?;
            list.push((offset, kind, r.u32()?, r.u32()?, r.u32()?));
        }
        nodes.push(Node {
            align,
            phase,
            sort,
            fixups: list,
            bytes: Vec::new(),
        });
    }
    // Every array's header comes before any array's bytes, as they were written.
    for (node, len) in nodes.iter_mut().zip(lens) {
        let Node {
            align,
            phase,
            fixups,
            bytes,
            ..
        } = node;
        *bytes = r.take(len)?.to_vec();
        if !align.is_power_of_two() || *align > MAX_ALIGN {
            return Err(format!(
                "an array asks for alignment {align}, not a power of two up to {MAX_ALIGN}"
            ));
        }
        if *phase >= *align {
            return Err(format!(
                "an array's address phase {phase} is not under its alignment {align}"
            ));
        }
        for (offset, kind, ..) in fixups.iter() {
            if !matches!(kind, 1..=9) {
                return Err(format!(
                    "a field of kind {kind}, which this build does not know"
                ));
            }
            if u64::from(*offset) + width(*kind) > bytes.len() as u64 {
                return Err(format!(
                    "a field at {offset:#x} leaves its array of {} bytes",
                    bytes.len()
                ));
            }
        }
    }
    if r.at != bytes.len() {
        return Err("bytes after the image's arrays".into());
    }
    if nodes.is_empty() {
        return Err("an image with no array".into());
    }
    let cx = Context {
        set,
        strings: &strings,
        refs: &refs,
        raws: &raws,
    };
    let mut next = 0;
    let root = assemble(&nodes, &mut next, &cx, 0)?;
    if next != nodes.len() {
        return Err(format!(
            "{} arrays of the image no field reaches",
            nodes.len() - next
        ));
    }
    Ok(root)
}

/// What the tables of an image name, resolved against the map.
struct Context<'a> {
    set: &'a TagSet,
    strings: &'a [String],
    refs: &'a [Ref],
    raws: &'a [Raw],
}

impl Context<'_> {
    fn string(&self, i: u32) -> Result<String, String> {
        self.strings
            .get(i as usize)
            .cloned()
            .ok_or_else(|| "a string the image's table does not hold".to_owned())
    }
    fn tag(&self, i: u32) -> Result<TagId, String> {
        match self.refs.get(i as usize) {
            None => Err("a reference the image's table does not hold".into()),
            Some(Ref::Null) => Ok(TagId::NULL),
            Some(Ref::Datum(d)) => Ok(TagId(*d)),
            Some(Ref::Name { group, path }) => {
                let group = FourCC::from_u32(*group);
                self.set.find(group, path).ok_or_else(|| {
                    format!(
                        "names {}.{}, which the map does not have",
                        path,
                        tags::defs::fourcc_text(group)
                    )
                })
            }
        }
    }
    fn raw(&self, i: u32) -> Result<RawRef, String> {
        match self.raws.get(i as usize) {
            None => Err("a raw resource the image's table does not hold".into()),
            Some(Raw::Nothing) => Ok(RawRef::Null),
            Some(Raw::At { file, offset }) => Ok(RawRef::External {
                file: resource_file(*file)?,
                offset: *offset,
            }),
            Some(Raw::ByHash(want)) => {
                // The bytes, not their place: a resource the set holds under
                // another id is the same resource.
                let found = self
                    .set
                    .raw
                    .iter()
                    .position(|blob| Sha256::of(&blob.bytes).0 == *want);
                match found {
                    Some(i) => Ok(RawRef::Local(RawId(i))),
                    None => Err(format!(
                        "names the raw resource {}, which the map does not hold",
                        Sha256(*want)
                    )),
                }
            }
        }
    }
}

/// The array at `*next` of `nodes` and, under it, every array its fields
/// name, in tree order. `depth` counts the arrays above it. The tree must
/// use every array exactly once and nest no deeper than [`MAX_DEPTH`].
fn assemble(
    nodes: &[Node],
    next: &mut usize,
    cx: &Context<'_>,
    depth: usize,
) -> Result<Block, String> {
    if depth > MAX_DEPTH {
        return Err(format!("an asset nested more than {MAX_DEPTH} arrays deep"));
    }
    let me = *next;
    *next += 1;
    let n = &nodes[me];
    let mut block = Block {
        bytes: n.bytes.clone(),
        align: n.align,
        fixups: Vec::with_capacity(n.fixups.len()),
        share: None,
        phase: n.phase,
        sort_by_string_id: (n.sort != 0).then_some(n.sort),
    };
    let child = |next: &mut usize| -> Result<Block, String> {
        if *next >= nodes.len() {
            return Err("an array the image's data does not hold".into());
        }
        assemble(nodes, next, cx, depth + 1)
    };
    for (offset, kind, a, b, c) in &n.fixups {
        let kind = match *kind {
            1..=3 => {
                // The array a field names is the next of the image, as it was written.
                if usize::try_from(*b).ok() != Some(*next) {
                    return Err(format!(
                        "a field names array {b}, which is not the next array of the image"
                    ));
                }
                let array = child(next)?;
                match *kind {
                    1 => FixupKind::Block {
                        count: *a,
                        child: array,
                    },
                    2 => FixupKind::Data(array),
                    _ => FixupKind::Pointer(array),
                }
            }
            4 => FixupKind::TagRef(cx.tag(*a)?),
            5 => FixupKind::TagId(cx.tag(*a)?),
            6 => FixupKind::StringId(cx.string(*a)?),
            7 => FixupKind::Raw(cx.raw(*a)?),
            8 => {
                if *a as usize >= cx.set.bsps.len() {
                    return Err(format!("BSP section {a}, which the map does not have"));
                }
                FixupKind::Bsp {
                    section: *a as usize,
                    field: bsp_field(*b)?,
                }
            }
            9 => FixupKind::TagOffset {
                count: *a,
                tag: cx.tag(*b)?,
                offset: *c,
            },
            _ => return Err("an unknown field kind in the image".into()),
        };
        block.fixups.push(Fixup {
            offset: *offset,
            kind,
        });
    }
    Ok(block)
}

// ---------------------------------------------------------------- the item

/// The key a delta patches: the item's `tag`, and the group its source's
/// name names (`<path>.<group>.bps`, as a `tag` item's source does).
fn key(job: &Job<'_>) -> Result<(FourCC, String), Failure> {
    let source = job.item.source.as_deref().unwrap_or("");
    let name = Path::new(source)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(source);
    let group = name
        .strip_suffix(".bps")
        .and_then(|s| s.rsplit_once('.').map(|(_, g)| g))
        .ok_or_else(|| format!("{source:?} is not <path>.<group>.bps"))?;
    let group =
        parse_fourcc(group).ok_or_else(|| format!("{group:?} in {source:?} is not a tag group"))?;
    let path = job
        .item
        .tag
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or("a tag-delta item needs `tag`, the path of the asset it patches")?;
    Ok((group, path.replace('/', "\\")))
}

/// The item's patch: a BPS file whose target is no larger than a delta of
/// this asset can make it.
fn patch(job: &Job<'_>) -> Result<Bps, Failure> {
    let patch = Bps::parse(job.source).map_err(|e| format!("not a BPS patch: {e}"))?;
    if patch.target_size
        > patch
            .source_size
            .saturating_mul(2)
            .saturating_add(MAX_GROWN)
    {
        return Err(format!(
            "the patch makes the asset {} bytes, over twice its base and {MAX_GROWN} more",
            patch.target_size
        )
        .into());
    }
    Ok(patch)
}

/// What `h2 package check` sees of a delta without a map: its key, and a
/// source that is a BPS patch of a sane size.
pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    key(job)?;
    patch(job)?;
    Ok(())
}

/// Patch the asset at the item's key with its BPS. The key, `was` and the
/// patch's own source check are refusals (module docs); anything else stops
/// the build.
pub fn apply(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    let (group, path) = key(job)?;
    let target = Cx::key(group, &path);
    let id = cx
        .find(&target)
        .ok_or_else(|| cx.refuse(format!("the map has no tag {target}")))?;
    cx.touch(id)?;
    cx.check_base(&target, id, job.item.was.as_deref())?;
    let patch = patch(job)?;
    let block = cx.read_block(id)?;
    let before = image(&*cx.set, &block).map_err(|why| format!("the image of {target}: {why}"))?;
    let after = patch
        .apply(&before)
        .map_err(|e| cx.refuse(format!("the patch was not made on this asset: {e}")))?;
    let block = unimage(&*cx.set, &after).map_err(|why| format!("the patched {target}: {why}"))?;
    cx.write_block(id, &target, block)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use map::build::{FixupKind, TagPayload};

    use super::*;
    use crate::tests::{A, ADD_C_FILE, C, base, cc, pkg, target};

    fn built(set: &TagSet, name: &str) -> (map::TagId, Block) {
        let id = set.find(cc(b"jpt!"), name).expect("the tag is in the map");
        let TagPayload::Built(root) = &set.tag(id).expect("in the set").payload else {
            panic!("not built");
        };
        (id, root.clone())
    }

    #[test]
    fn an_image_reads_back_as_the_same_tag() {
        // An invented asset with an array, a string ID and a reference beside one of defaults.
        let mut set = base();
        let source = format!(
            "like: 'jpt!:{A}'\nblock:\n  - {{path: player_responses, op: add}}\nfields:\n  damage_lower_bound: 7.0\n  \"player_responses[0].duration\": 0.5\n  sound: 'snd!:invented\\snd\\boom'\n"
        );
        let p = pkg(
            "p",
            &[],
            &[("tag", ADD_C_FILE, Some(C), "all")],
            &[(ADD_C_FILE, source.as_bytes())],
        );
        crate::build(&mut set, &[p], &target(), None).unwrap();
        for name in [A, C] {
            let (id, root) = built(&set, name);
            if name == C {
                assert!(
                    root.fixups
                        .iter()
                        .any(|f| matches!(f.kind, FixupKind::Block { .. } | FixupKind::TagRef(_))),
                    "{name} has an array and a reference"
                );
            }
            let bytes = image(&set, &root).unwrap();
            let back = unimage(&set, &bytes).unwrap();
            assert_eq!(back, root, "{name}");
            // The same asset gives the same image again, and the same hash once it is back in the set.
            assert_eq!(image(&set, &back).unwrap(), bytes);
            let hash = crate::tag_hash(&set, id);
            let mut again = set.clone();
            again.tag_mut(id).unwrap().payload = TagPayload::Built(back);
            assert_eq!(crate::tag_hash(&again, id), hash);
        }
    }

    #[test]
    fn an_image_names_tags_by_key_strings_by_text_and_raw_by_hash() {
        let mut set = base();
        let a = set.find(cc(b"jpt!"), A).unwrap();
        let pixels: Vec<u8> = (0..64u32).map(|i| (i * 3) as u8).collect();
        let raw = set.add_raw(pixels.clone(), 16);
        let mut root = Block::zeroed(20);
        root.tag_ref(0, a)
            .tag_id(8, TagId::NULL)
            .string_id(12, "invented")
            .raw(16, RawRef::Local(raw));
        let bytes = image(&set, &root).unwrap();
        // What it names is in it as text, never as a datum.
        let text = String::from_utf8_lossy(&bytes).into_owned();
        assert!(text.contains("invented") && text.contains(A), "{text:?}");
        let hash = Sha256::of(&pixels).0;
        assert!(
            bytes.windows(hash.len()).any(|w| w == hash.as_slice()),
            "the raw hash is present"
        );
        assert!(
            !bytes.windows(pixels.len()).any(|w| w == pixels.as_slice()),
            "the raw bytes are absent"
        );
        assert_eq!(unimage(&set, &bytes).unwrap(), root);
        // The map it is read into must hold what it names.
        let mut elsewhere = base();
        elsewhere.slots.retain(|s| s.name.as_deref() != Some(A));
        let why = unimage(&elsewhere, &bytes).unwrap_err();
        assert!(why.contains(A) && why.contains("does not have"), "{why}");
        let why = unimage(&base(), &bytes).unwrap_err();
        assert!(why.contains("raw resource"), "{why}");
    }

    #[test]
    fn an_image_is_refused_by_what_it_gets_wrong() {
        let set = base();
        let mut root = Block::zeroed(8);
        root.block(0, 0, Block::zeroed(0));
        let good = image(&set, &root).unwrap();
        assert_eq!(unimage(&set, &good).unwrap(), root);
        // Magic and version (8), three empty tables (12), the array count
        // (4), the root's header (20) and its one field (17), the empty
        // array's header (20), the root's 8 bytes.
        assert_eq!(&good[..8], b"H2TI\x01\0\0\0");
        assert_eq!(good.len(), 8 + 12 + 4 + 20 + 17 + 20 + 8);
        let word = |n: u32| n.to_le_bytes();
        let refused = |bytes: &[u8], what: &str| {
            let why = unimage(&set, bytes).expect_err("refused");
            assert!(why.contains(what), "want {what:?}: {why}");
        };
        let mut other = good.clone();
        other[4..8].copy_from_slice(&word(2));
        refused(&other, "version 2");
        refused(&good[..good.len() - 1], "truncated");
        let mut extra = good.clone();
        extra.push(0);
        refused(&extra, "bytes after");
        let mut none = good[..20].to_vec();
        none.extend_from_slice(&word(0));
        refused(&none, "no array");
        // The root's alignment, then its one field's offset, kind and array.
        let mut other = good.clone();
        other[24..28].copy_from_slice(&word(3));
        refused(&other, "alignment 3");
        let mut other = good.clone();
        other[44..48].copy_from_slice(&word(100));
        refused(&other, "leaves its array");
        let mut other = good.clone();
        other[48] = 77;
        refused(&other, "kind 77");
        let mut other = good.clone();
        other[53..57].copy_from_slice(&word(5));
        refused(&other, "not the next array");
        // A third array that no field reaches.
        let mut other = good[..61].to_vec();
        other.extend_from_slice(&good[61..81]);
        other.extend_from_slice(&good[61..]);
        other[20..24].copy_from_slice(&word(3));
        refused(&other, "no field reaches");
    }
}
