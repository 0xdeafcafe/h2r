//! Infer a [`Schema`] from tag instances across many maps.
//!
//! Each group's root is an array of one element at the tag's address. For an
//! array layout (a "node") with all its instances known:
//!
//! 1. **Element size.** Every tag's candidate pointers (any byte offset whose
//!    pair `{count 1..2^20, address inside the tag}` could be a block) give a
//!    set of boundaries; an array at `a` with `c` elements ends at or before
//!    the next boundary after `a` (or the tag's end). The element size is the
//!    smallest `gap / c` over all instances, so no instance overlaps what
//!    follows it.
//! 2. **Blocks.** An offset is a block when, in every element of every
//!    instance, its pair is `{0, 0}`, `{0, address}` or `{count, address}`
//!    with the address inside the tag or (for ordinary-metadata tags) inside
//!    another tag: the cache compiler shares identical blocks between tags.
//!    At least one count must be nonzero. The child node's instances are the
//!    nonzero pairs, located in whichever tag holds them.
//! 3. **Tag references, datums, string IDs.** An offset is one when every
//!    instance's value is null or resolves in its own map's tag index or
//!    string table, and at least one resolves.
//!
//! 4. **Raw-resource pointers.** An offset is one when every instance is
//!    `0xffffffff`, 0, or a file offset (with its selector) past the header
//!    and before the selected file's index, with at least two distinct
//!    values, all 4-aligned and not all multiples of 64 KiB.
//!
//! 5. **BSP descriptors.** Twelve bytes are one when every instance is all
//!    zero or one of its map's BSP regions (file offset, size, load
//!    address), and at least one is. Checked before raw pointers, which a
//!    region's file offset would otherwise pass for.
//!
//! 6. **Plain words.** Of what is left, a 4-byte offset is a plain word
//!    when some instance holds a value inside one of its map's address
//!    spaces that is not 4-aligned: no pointer is, so the field is data
//!    (packed indices) the relocation check need not treat as a pointer.
//!
//! 7. **Raw sizes.** After the layouts settle, each raw pointer is paired
//!    with the free word of its element that is, in every instance with a
//!    non-null pointer, nonzero and no larger than the distance to the next
//!    raw pointer of that map into the same file (or the end of its raw
//!    data); of those, the one with the least total slack.
//!
//! Offsets are claimed in that order, lowest first, and never overlap.
//! Ambiguous fields (always zero, or mixed evidence) are left out; the
//! relocation check ([`super::walk`]) then refuses any tag where such a word
//! holds an address.
use super::{Field, MAX_ALIGN, Schema, Struct};
use map::{FourCC, Map};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek};

/// Most elements a candidate block may have.
const MAX_COUNT: u32 = 1 << 20;

/// One tag's bytes.
#[derive(Clone, Debug)]
pub struct SampleTag {
    /// Group.
    pub group: FourCC,
    /// Virtual address of the first byte.
    pub address: u32,
    /// The tag's bytes.
    pub bytes: Vec<u8>,
    /// Sorted candidate pointer targets inside the tag (4-aligned, room for
    /// their count), then the tag's end.
    candidates: Vec<u32>,
    /// The boundaries element sizes are measured against: the candidates at
    /// first, then refined (see [`infer`]).
    bounds: Vec<u32>,
    /// In the ordinary metadata (not a BSP region).
    pub ordinary: bool,
}

/// One map's tags plus what resolves datums and string IDs there.
#[derive(Clone, Debug)]
pub struct MapSample {
    /// Map name (for reports).
    pub name: String,
    /// Tags with data.
    pub tags: Vec<SampleTag>,
    /// Indices of ordinary-metadata tags in `tags`, by address.
    ordinary: Vec<u32>,
    /// Datum to group.
    pub datums: HashMap<u32, FourCC>,
    /// Declared groups.
    pub groups: HashSet<u32>,
    /// String lengths by index (`None` for a missing entry).
    pub string_lengths: Vec<Option<usize>>,
    /// Map file length.
    pub file_size: u32,
    /// End of local raw data: local raw pointers fall in `0x800..raw_end`
    /// (the tag index offset).
    pub raw_end: u32,
    /// Lengths of `mainmenu.map`, `shared.map` and `single_player_shared.map`
    /// by raw-pointer selector (index 0 unused), when they are in the
    /// corpus; 0 when not (then pointers into them are not accepted).
    pub external: [u32; 4],
    /// BSP regions: (file offset, size, load address).
    pub bsps: HashSet<(u32, u32, u32)>,
    /// Address spaces (metadata and BSP regions): `[start, end)`.
    pub spaces: Vec<(u32, u32)>,
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

impl SampleTag {
    /// A tag at `address` holding `bytes`.
    pub fn new(group: FourCC, address: u32, bytes: Vec<u8>, ordinary: bool) -> Self {
        let end = address.wrapping_add(bytes.len() as u32);
        let mut bounds: Vec<u32> = (0..bytes.len().saturating_sub(7))
            .step_by(4)
            .filter_map(|k| {
                let c = u32_at(&bytes, k);
                let a = u32_at(&bytes, k + 4);
                (c > 0
                    && c <= MAX_COUNT
                    && a > address
                    && a.is_multiple_of(4)
                    && c <= end.wrapping_sub(a))
                .then_some(a)
            })
            .collect();
        bounds.push(end);
        bounds.sort_unstable();
        bounds.dedup();
        SampleTag {
            group,
            address,
            bytes,
            candidates: bounds.clone(),
            bounds,
            ordinary,
        }
    }

    fn end(&self) -> u32 {
        self.address.wrapping_add(self.bytes.len() as u32)
    }

    /// The first boundary after an array of `c` elements at `a` that leaves
    /// at least a byte per element.
    fn next_bound(&self, a: u32, c: u32) -> u32 {
        let i = self.bounds.partition_point(|&b| b < a.saturating_add(c));
        self.bounds.get(i).copied().unwrap_or(self.end())
    }
}

impl MapSample {
    /// Read every tag with data from `map`.
    pub fn read<R: Read + Seek>(name: &str, map: &mut Map<R>) -> crate::Result<Self> {
        let mut tags = Vec::new();
        let list: Vec<_> = map.tags().to_vec();
        let meta = map.metadata_space();
        let mut bsps = HashSet::new();
        let mut spaces: Vec<(u32, u32)> = meta
            .iter()
            .map(|s| (s.virtual_base, s.virtual_base.saturating_add(s.size)))
            .collect();
        for t in &list {
            let Some(loc) = t.location else { continue };
            if Some(loc.space) != meta {
                let r = (
                    loc.space.virtual_base,
                    loc.space.virtual_base.saturating_add(loc.space.size),
                );
                if !spaces.contains(&r) {
                    spaces.push(r);
                }
                bsps.insert((
                    u32::try_from(loc.space.file_offset).unwrap_or(u32::MAX),
                    loc.space.size,
                    loc.space.virtual_base,
                ));
            }
            let bytes = map.read_tag_bytes(t.id)?;
            tags.push(SampleTag::new(
                t.group,
                loc.address,
                bytes,
                Some(loc.space) == meta,
            ));
        }
        let mut ordinary: Vec<u32> = (0..tags.len() as u32)
            .filter(|&i| tags[i as usize].ordinary && !tags[i as usize].bytes.is_empty())
            .collect();
        ordinary.sort_by_key(|&i| tags[i as usize].address);
        Ok(MapSample {
            name: name.to_owned(),
            tags,
            ordinary,
            datums: list.iter().map(|t| (t.id.0, t.group)).collect(),
            groups: map.groups().iter().map(|g| g.class.as_u32()).collect(),
            string_lengths: map
                .strings()
                .iter()
                .map(|s| s.as_ref().map(String::len))
                .collect(),
            file_size: map.header().file_size,
            raw_end: map.header().index_offset,
            external: [0; 4],
            bsps,
            spaces,
        })
    }

    /// The tag holding address `a`, seen from tag `from`: `from` itself, or
    /// (for ordinary tags) any ordinary tag; blocks may be shared.
    fn locate(&self, from: u32, a: u32) -> Option<u32> {
        let t = &self.tags[from as usize];
        if a >= t.address && a < t.end() {
            return Some(from);
        }
        if !t.ordinary {
            return None;
        }
        let i = self
            .ordinary
            .partition_point(|&i| self.tags[i as usize].address <= a)
            .checked_sub(1)?;
        let j = self.ordinary[i];
        let u = &self.tags[j as usize];
        (a < u.end()).then_some(j)
    }

    /// A raw-resource pointer: `(null, valid)`. Null is `0xffffffff` (or 0);
    /// valid is an offset past the header and inside the selected file's
    /// raw data.
    fn raw_ok(&self, v: u32) -> (bool, bool) {
        if v == u32::MAX || v == 0 {
            return (true, false);
        }
        let sel = (v >> 30) as usize;
        let off = v & 0x3fff_ffff;
        let end = if sel == 0 {
            self.raw_end
        } else {
            self.external[sel]
        };
        (false, off >= 0x800 && off < end)
    }

    fn datum_ok(&self, v: u32) -> bool {
        self.datums.contains_key(&v)
    }

    fn string_ok(&self, v: u32) -> bool {
        let index = (v & 0x00ff_ffff) as usize;
        let len = (v >> 24) as usize;
        len > 0 && self.string_lengths.get(index).copied().flatten() == Some(len)
    }
}

/// An array instance: (map, tag, offset in the tag, count).
type Inst = (u16, u32, u32, u32);

/// Infer the schema of every group present in `maps`. Groups whose root
/// size cannot be found are left out.
///
/// Candidate boundaries include false positives (bulk data that happens to
/// look like a `{count, address}` pair), which make element sizes too
/// small. So inference runs in rounds: after each, every tag is walked with
/// the schema so far, and the next round's boundaries are the arrays found
/// plus the candidates outside them.
pub fn infer(maps: &mut [MapSample]) -> Schema {
    let mut external = [0; 4];
    for m in maps.iter() {
        let sel = match m.name.as_str() {
            "mainmenu" => 1,
            "shared" => 2,
            "single_player_shared" => 3,
            _ => continue,
        };
        external[sel] = m.file_size;
    }
    for m in maps.iter_mut() {
        if m.external == [0; 4] {
            m.external = external;
        }
    }
    let mut schema = infer_round(maps);
    for _ in 0..ROUNDS {
        for sample in maps.iter_mut() {
            for t in 0..sample.tags.len() {
                let Some(root) = schema.groups.get(&sample.tags[t].group) else {
                    continue;
                };
                let mut arrays = Vec::new();
                arrays_of(sample, t as u32, root, 0, 1, &mut arrays, 0);
                arrays.sort_unstable();
                let tag = &mut sample.tags[t];
                let inside = |b: u32| {
                    let off = b - tag.address;
                    let i = arrays.partition_point(|r| r.0 < off);
                    i > 0 && off < arrays[i - 1].0 + arrays[i - 1].1
                };
                let mut bounds: Vec<u32> = tag
                    .candidates
                    .iter()
                    .copied()
                    .filter(|&b| !inside(b))
                    .chain(arrays.iter().filter(|r| r.0 > 0).map(|r| tag.address + r.0))
                    .collect();
                bounds.sort_unstable();
                bounds.dedup();
                tag.bounds = bounds;
            }
        }
        let next = infer_round(maps);
        if next == schema {
            break;
        }
        schema = next;
    }
    apply_engine_data(&mut schema);
    apply_engine_refs(&mut schema);
    pair_raw_sizes(&mut schema, maps);
    apply_engine_strides(&mut schema);
    schema
}

/// Words the engine reads as data that inference takes for raw-resource
/// pointers: `(group, block path, offset)`. Their retail values are small,
/// 4-aligned numbers that pass for file offsets (step 4), so they came out
/// raw, and a fresh or from-scratch build copied "the resource" they named
/// and wrote a new file offset over them.
///
/// - `nhdt` bitmap widgets (`+0x8`) and text widgets (`+0x10`), `+0x18`:
///   three threshold bytes the state gate compares with the weapon's state
///   (E `0x0013ac8f`..`0x0013acbd`: `+0x18` against the battery's age,
///   `+0x19` the rounds loaded, `+0x1a` the rounds in reserve, each a byte;
///   the text widget's gate is the same call, `0x0013a63d`). V: a converted
///   Blood Gulch whose Halo 1 "Reload" widget was written `00 0b 01 00`
///   there came out of the from-scratch build as `00 da 22 01`, a file
///   offset: rounds loaded below 218, so "Reload" showed on a full
///   magazine. The text widget's `+0x2c`, which inference paired with it as
///   the resource's size, is the justification (E `0x0022ac0b`).
pub const ENGINE_DATA: &[(FourCC, &[u32], u32)] = &[
    (FourCC(*b"nhdt"), &[0x8], 0x18),
    (FourCC(*b"nhdt"), &[0x10], 0x18),
];

/// Applies [`ENGINE_DATA`] to a schema: the word is no field (copied as
/// bytes), and no size is paired with it.
pub fn apply_engine_data(schema: &mut Schema) {
    fn at<'a>(s: &'a mut Struct, path: &[u32]) -> Option<&'a mut Struct> {
        match path.split_first() {
            None => Some(s),
            Some((o, rest)) => match s.fields.get_mut(o)? {
                Field::Block(child) => at(child, rest),
                _ => None,
            },
        }
    }
    // The entire 0x44-byte gear is inline numeric data. H2 evaluates two
    // six-float torque curves at 0xf1540, reads the ratio at 0xf152c and
    // tests four shift parameters at 0xf18aa. No word is a resource.
    if let Some(gear) = schema.groups.get_mut(&FourCC(*b"vehi"))
        .and_then(|root| at(root, &[0x268])) {
        gear.fields.clear();
    }
    for &(group, path, offset) in ENGINE_DATA {
        let Some(s) = schema
            .groups
            .get_mut(&group)
            .and_then(|root| at(root, path))
        else {
            continue;
        };
        if matches!(s.fields.get(&offset), Some(Field::Raw)) {
            s.fields.remove(&offset);
        }
        s.fields
            .retain(|_, f| !matches!(f, Field::RawSize(of) if *of == offset));
    }
}

/// Tag references no retail tag fills: `(group, block path, offset)`.
/// Inference takes a field for a reference only when some element holds a
/// live datum there, and these hold a group word and a null datum (or
/// nothing) on every retail map, so they came out as bytes. A converter
/// that fills one then wrote a datum the schema doesn't know: an extract
/// or a from-scratch build copied it as data, unrenamed (a converted Blood
/// Gulch's assault rifle read back with no flashlight effect).
///
/// - `weap`'s item layer: `+0xfc` the collision sound, `+0x11c` the
///   detonating and `+0x124` the detonation effect (`tags/src/defs/item.txt`);
/// - `weap +0x17c` the detonation effect (E `0x000cf19f` reads its datum
///   at `+0x180`), `+0x244` and `+0x24c` the light power-on and power-off
///   effects (S: `effe` group words on retail weapons);
/// - the magazines' (`weap +0x2c0`) `+0x44` chambering effect (S: between
///   two `jpt!` group words, as the reloading effect at `+0x34`).
///
/// Halo 1's weapons fill each (`h1objects::arsenal::WEAPON_EFFECTS`,
/// `MAGAZINE_CHAMBERING_EFFECT`).
pub const ENGINE_REFS: &[(FourCC, &[u32], u32)] = &[
    (FourCC(*b"bipd"), &[], 0x4c),
    (FourCC(*b"vehi"), &[], 0x4c),
    (FourCC(*b"weap"), &[], 0x4c),
    (FourCC(*b"eqip"), &[], 0x4c),
    (FourCC(*b"proj"), &[], 0x4c),
    (FourCC(*b"scen"), &[], 0x4c),
    (FourCC(*b"garb"), &[], 0xfc),
    (FourCC(*b"garb"), &[], 0x10c),
    (FourCC(*b"garb"), &[], 0x11c),
    (FourCC(*b"garb"), &[], 0x124),
    (FourCC(*b"garb"), &[], 0x4c),
    (FourCC(*b"vehi"), &[], 0xc4),
    (FourCC(*b"vehi"), &[], 0x164),
    (FourCC(*b"vehi"), &[], 0x284),
    (FourCC(*b"vehi"), &[], 0x294),
    (FourCC(*b"eqip"), &[], 0xfc),
    (FourCC(*b"eqip"), &[], 0x11c),
    (FourCC(*b"weap"), &[], 0xfc),
    (FourCC(*b"weap"), &[], 0x10c),
    (FourCC(*b"weap"), &[], 0x11c),
    (FourCC(*b"weap"), &[], 0x124),
    (FourCC(*b"weap"), &[], 0x17c),
    (FourCC(*b"weap"), &[], 0x244),
    (FourCC(*b"weap"), &[], 0x24c),
    (FourCC(*b"weap"), &[0x2c0], 0x44),
];

/// Applies [`ENGINE_REFS`] to a schema: each is a tag reference.
pub fn apply_engine_refs(schema: &mut Schema) {
    fn at<'a>(s: &'a mut Struct, path: &[u32]) -> Option<&'a mut Struct> {
        match path.split_first() {
            None => Some(s),
            Some((o, rest)) => match s.fields.get_mut(o)? {
                Field::Block(child) => at(child, rest),
                _ => None,
            },
        }
    }
    // Halo 2 embeds the same object header in all three. The native biped
    // populates both arrays, equipment and debris on the disc leave them empty.
    for at in [0x64, 0x94] {
        if let Some(field) = schema.groups.get(&FourCC(*b"bipd")).and_then(|r| r.fields.get(&at)).cloned() {
            for group in [b"eqip", b"garb"] {
                if let Some(root) = schema.groups.get_mut(&FourCC(*group)) {
                    root.fields.insert(at, field.clone());
                }
            }
        }
    }
    for &(group, path, offset) in ENGINE_REFS {
        if let Some(s) = schema
            .groups
            .get_mut(&group)
            .and_then(|root| at(root, path))
        {
            s.fields.insert(offset, Field::TagRef);
        }
    }
}

/// Root block element sizes the engine settles where the data can't:
/// `(group, offset, size, tag reference offsets)`. Inference takes the
/// smallest gap a block's instances allow, and the scenery palette
/// (`scnr +0x58`) is followed so closely by other arrays on some maps that
/// it came out 4 bytes. Every object palette is read with one `0x28`-byte
/// stride and its datum at `+4` (`0x000a743b`..`0x000a7473`, through the
/// object type table `0x00468630`; checked in
/// `h1convert/tests/h2_scenario.rs`).
pub const ENGINE_STRIDES: &[(FourCC, u32, u32, &[u32])] = &[(FourCC(*b"scnr"), 0x58, 0x28, &[0])];

/// Applies [`ENGINE_STRIDES`] to an inferred schema.
pub fn apply_engine_strides(schema: &mut Schema) {
    for &(group, offset, size, refs) in ENGINE_STRIDES {
        let Some(root) = schema.groups.get_mut(&group) else {
            continue;
        };
        if let Some(Field::Block(child)) = root.fields.get_mut(&offset) {
            child.size = size;
            for &r in refs {
                child.fields.insert(r, Field::TagRef);
            }
        }
    }
}

/// Elements holding raw pointers, by group and block path: (map, tag,
/// offset of the element in the tag).
type RawElements = BTreeMap<(FourCC, Vec<u32>), HashSet<(usize, u32, u32)>>;

#[allow(clippy::too_many_arguments)]
fn raw_elements(
    m: &MapSample,
    mi: usize,
    t: u32,
    s: &Struct,
    at: u32,
    count: u32,
    key: &mut (FourCC, Vec<u32>),
    out: &mut RawElements,
    depth: usize,
) {
    let tag = &m.tags[t as usize];
    let n = u64::from(s.size) * u64::from(count);
    if depth > 16 || u64::from(at) + n > tag.bytes.len() as u64 {
        return;
    }
    let raw = s.fields.values().any(|f| matches!(f, Field::Raw));
    for i in 0..count {
        let e = at + i * s.size;
        if raw && !out.entry(key.clone()).or_default().insert((mi, t, e)) {
            continue;
        }
        for (&o, f) in &s.fields {
            if let Field::Block(child) = f {
                let c = u32_at(&tag.bytes, (e + o) as usize);
                let a = u32_at(&tag.bytes, (e + o) as usize + 4);
                let Some(u) = (c > 0).then(|| m.locate(t, a)).flatten() else {
                    continue;
                };
                key.1.push(o);
                let off = a - m.tags[u as usize].address;
                raw_elements(m, mi, u, child, off, c, key, out, depth + 1);
                key.1.pop();
            }
        }
    }
}

/// Step 7: give every raw pointer its size word (see the module docs).
fn pair_raw_sizes(schema: &mut Schema, maps: &[MapSample]) {
    let mut elements = RawElements::new();
    for (mi, m) in maps.iter().enumerate() {
        for t in 0..m.tags.len() as u32 {
            let g = m.tags[t as usize].group;
            let Some(root) = schema.groups.get(&g) else {
                continue;
            };
            raw_elements(m, mi, t, root, 0, 1, &mut (g, Vec::new()), &mut elements, 0);
        }
    }
    let at_path = |schema: &Schema, (g, path): &(FourCC, Vec<u32>)| -> Option<Struct> {
        let mut s = schema.groups.get(g)?;
        for o in path {
            match s.fields.get(o)? {
                Field::Block(c) => s = c,
                _ => return None,
            }
        }
        Some(s.clone())
    };
    // Every raw pointer of each map, by file selector: the next one after
    // a resource bounds its size.
    let mut starts: Vec<[Vec<u32>; 4]> = vec![Default::default(); maps.len()];
    for (key, els) in &elements {
        let Some(s) = at_path(schema, key) else {
            continue;
        };
        for &(mi, t, e) in els {
            let b = &maps[mi].tags[t as usize].bytes;
            for (&o, _) in s.fields.iter().filter(|(_, f)| matches!(f, Field::Raw)) {
                let v = u32_at(b, (e + o) as usize);
                if let (false, true) = maps[mi].raw_ok(v) {
                    starts[mi][(v >> 30) as usize].push(v & 0x3fff_ffff);
                }
            }
        }
    }
    for per in &mut starts {
        for v in per.iter_mut() {
            v.sort_unstable();
            v.dedup();
        }
    }
    // The next start skips the element's own pointers: a bitmap's first
    // level of detail spans the levels after it.
    let gap = |mi: usize, v: u32, own: &[u32]| -> u32 {
        let m = &maps[mi];
        let sel = (v >> 30) as usize;
        let off = v & 0x3fff_ffff;
        let list = &starts[mi][sel];
        let end = if sel == 0 { m.raw_end } else { m.external[sel] };
        let next = list[list.partition_point(|&x| x <= off)..]
            .iter()
            .copied()
            .find(|&x| !own.contains(&(x | (v & 0xc000_0000))))
            .unwrap_or(end);
        next.min(end) - off
    };
    for (key, els) in &elements {
        let Some(s) = at_path(schema, key) else {
            continue;
        };
        let mut taken: HashSet<u32> = HashSet::new();
        let mut pairs = Vec::new();
        for (&r, _) in s.fields.iter().filter(|(_, f)| matches!(f, Field::Raw)) {
            let mut best: Option<(u64, u32)> = None;
            for k in (0..s.size.saturating_sub(3)).step_by(4) {
                let free = !taken.contains(&k)
                    && !s
                        .fields
                        .iter()
                        .any(|(&o, f)| o < k + 4 && k < o + f.width());
                if !free {
                    continue;
                }
                let mut slack = 0u64;
                let mut any = false;
                let ok = els.iter().all(|&(mi, t, e)| {
                    let b = &maps[mi].tags[t as usize].bytes;
                    let own: Vec<u32> = s
                        .fields
                        .iter()
                        .filter(|(_, f)| matches!(f, Field::Raw))
                        .map(|(&o, _)| u32_at(b, (e + o) as usize))
                        .collect();
                    let p = u32_at(b, (e + r) as usize);
                    let v = u32_at(b, (e + k) as usize);
                    match maps[mi].raw_ok(p) {
                        (true, _) => true,
                        (false, true) => {
                            let g = gap(mi, p, &own);
                            any = true;
                            slack += u64::from(g.saturating_sub(v));
                            v > 0 && v <= g
                        }
                        (false, false) => false,
                    }
                });
                if ok && any && best.is_none_or(|(b, _)| slack < b) {
                    best = Some((slack, k));
                }
            }
            if let Some((_, k)) = best {
                taken.insert(k);
                pairs.push((k, r));
            }
        }
        if pairs.is_empty() {
            continue;
        }
        let mut s = schema.groups.get_mut(&key.0).expect("group");
        for o in &key.1 {
            match s.fields.get_mut(o) {
                Some(Field::Block(c)) => s = c,
                _ => unreachable!("path checked"),
            }
        }
        for (k, r) in pairs {
            s.fields.insert(k, Field::RawSize(r));
        }
    }
}

/// Refinement rounds after the first.
const ROUNDS: usize = 4;

/// Arrays (offset, length) inside tag `t` reachable with `s`.
fn arrays_of(
    m: &MapSample,
    t: u32,
    s: &Struct,
    at: u32,
    count: u32,
    out: &mut Vec<(u32, u32)>,
    depth: usize,
) {
    let tag = &m.tags[t as usize];
    let n = u64::from(s.size) * u64::from(count);
    if depth > 16 || u64::from(at) + n > tag.bytes.len() as u64 {
        return;
    }
    out.push((at, n as u32));
    for i in 0..count {
        let e = (at + i * s.size) as usize;
        for (&o, f) in &s.fields {
            if let Field::Block(child) = f {
                let c = u32_at(&tag.bytes, e + o as usize);
                let a = u32_at(&tag.bytes, e + o as usize + 4);
                if c > 0 && a >= tag.address && a < tag.end() {
                    arrays_of(m, t, child, a - tag.address, c, out, depth + 1);
                }
            }
        }
    }
}

fn infer_round(maps: &[MapSample]) -> Schema {
    let mut by_group: BTreeMap<FourCC, Vec<Inst>> = BTreeMap::new();
    for (m, sample) in maps.iter().enumerate() {
        for (t, tag) in sample.tags.iter().enumerate() {
            if tag.bytes.is_empty() {
                continue;
            }
            by_group
                .entry(tag.group)
                .or_default()
                .push((m as u16, t as u32, 0, 1));
        }
    }
    let mut schema = Schema::default();
    for (group, insts) in by_group {
        if let Some(root) = node(maps, &insts, 0) {
            schema.groups.insert(group, root);
        }
    }
    schema
}

fn node(maps: &[MapSample], insts: &[Inst], depth: usize) -> Option<Struct> {
    let tag = |&(m, t, _, _): &Inst| &maps[m as usize].tags[t as usize];
    // 1. Element size.
    let mut size = u32::MAX;
    let mut align_bits = MAX_ALIGN.trailing_zeros();
    for i in insts {
        let t = tag(i);
        let a = t.address + i.2;
        let gap = t.next_bound(a, i.3) - a;
        size = size.min(gap / i.3);
        align_bits = align_bits.min(a.trailing_zeros());
    }
    if size == 0 || size == u32::MAX {
        return None;
    }
    let mut s = Struct {
        size,
        align: 1 << align_bits,
        fields: BTreeMap::new(),
    };
    // Deep recursion means a cycle of false positives; stop.
    if depth > 16 || size < 4 {
        return Some(s);
    }
    let elements = || {
        insts
            .iter()
            .flat_map(move |i| (0..i.3).map(move |e| (i, (i.2 + e * size) as usize)))
    };
    let mut claimed = vec![false; size as usize];
    let free = |claimed: &[bool], o: usize, w: usize| !claimed[o..o + w].iter().any(|c| *c);
    // 2. Blocks.
    let mut o = 0usize;
    while o + 8 <= size as usize {
        let mut any = false;
        let ok = elements().all(|(i, e)| {
            let t = tag(i);
            let c = u32_at(&t.bytes, e + o);
            let a = u32_at(&t.bytes, e + o + 4);
            // {count, 0} occurs (a count kept for stripped data): no child.
            if a == 0 {
                return true;
            }
            let found = maps[i.0 as usize].locate(i.1, a).is_some();
            if c == 0 {
                return found;
            }
            any = true;
            c <= MAX_COUNT && found
        });
        if ok && any {
            let children: Vec<Inst> = elements()
                .filter_map(|(i, e)| {
                    let t = tag(i);
                    let c = u32_at(&t.bytes, e + o);
                    let a = u32_at(&t.bytes, e + o + 4);
                    let m = &maps[i.0 as usize];
                    let u = m.locate(i.1, a)?;
                    (c > 0).then(|| (i.0, u, a - m.tags[u as usize].address, c))
                })
                .collect();
            if let Some(child) = node(maps, &children, depth + 1) {
                s.fields.insert(o as u32, Field::Block(Box::new(child)));
                claimed[o..o + 8].fill(true);
                o += 8;
                continue;
            }
        }
        o += 4;
    }
    // 3. Tag references, string IDs, datums.
    let sample = |i: &Inst| &maps[i.0 as usize];
    for o in (0..(size as usize).saturating_sub(7)).step_by(4) {
        if !free(&claimed, o, 8) {
            continue;
        }
        let mut any = false;
        let ok = elements().all(|(i, e)| {
            let t = tag(i);
            let g = u32_at(&t.bytes, e + o);
            let d = u32_at(&t.bytes, e + o + 4);
            if d == u32::MAX {
                return g == u32::MAX || sample(i).groups.contains(&g);
            }
            any = true;
            sample(i).datum_ok(d) && (g == u32::MAX || sample(i).groups.contains(&g))
        });
        if ok && any {
            s.fields.insert(o as u32, Field::TagRef);
            claimed[o..o + 8].fill(true);
        }
    }
    for (kind, test) in [
        (
            Field::StringId,
            (&|m: &MapSample, v: u32| (v == 0, m.string_ok(v)))
                as &dyn Fn(&MapSample, u32) -> (bool, bool),
        ),
        (Field::TagId, &|m: &MapSample, v: u32| {
            (v == u32::MAX, m.datum_ok(v))
        }),
    ] {
        for o in (0..(size as usize).saturating_sub(3)).step_by(4) {
            if !free(&claimed, o, 4) {
                continue;
            }
            let mut any = false;
            let ok = elements().all(|(i, e)| {
                let v = u32_at(&tag(i).bytes, e + o);
                let (null, valid) = test(sample(i), v);
                any |= valid;
                null || valid
            });
            if ok && any {
                s.fields.insert(o as u32, kind.clone());
                claimed[o..o + 4].fill(true);
            }
        }
    }
    // 5 (before 4). BSP descriptors.
    for o in (0..(size as usize).saturating_sub(11)).step_by(4) {
        if !free(&claimed, o, 12) {
            continue;
        }
        let mut any = false;
        let ok = elements().all(|(i, e)| {
            let b = &tag(i).bytes;
            let t = (u32_at(b, e + o), u32_at(b, e + o + 4), u32_at(b, e + o + 8));
            let hit = sample(i).bsps.contains(&t);
            any |= hit;
            hit || t == (0, 0, 0)
        });
        if ok && any {
            s.fields.insert(o as u32, Field::Bsp);
            claimed[o..o + 12].fill(true);
        }
    }
    // 4. Raw-resource pointers: every instance null or a plausible offset,
    // and at least two distinct non-null values (one repeated constant is
    // too little evidence).
    for o in (0..(size as usize).saturating_sub(3)).step_by(4) {
        if !free(&claimed, o, 4) {
            continue;
        }
        let mut seen = HashSet::new();
        let mut tz = 32;
        let ok = elements().all(|(i, e)| {
            let v = u32_at(&tag(i).bytes, e + o);
            let (null, valid) = sample(i).raw_ok(v);
            if valid {
                tz = tz.min((v & 0x3fff_ffff).trailing_zeros());
                if seen.len() < 2 {
                    seen.insert(v);
                }
            }
            null || valid
        });
        // Offsets that are all multiples of 64 KiB are round numbers (floats
        // with short mantissas, flag words), not resources; real ones are
        // at least 4-aligned (bitmap pixels are 0x200-aligned).
        if ok && seen.len() >= 2 && (2..16).contains(&tz) {
            s.fields.insert(o as u32, Field::Raw);
            claimed[o..o + 4].fill(true);
        }
    }
    // 6. Plain words.
    for o in (0..(size as usize).saturating_sub(3)).step_by(4) {
        if !free(&claimed, o, 4) {
            continue;
        }
        let odd = elements().any(|(i, e)| {
            let v = u32_at(&tag(i).bytes, e + o);
            !v.is_multiple_of(4)
                && sample(i)
                    .spaces
                    .iter()
                    .any(|&(lo, hi)| (lo..hi).contains(&v))
        });
        if odd {
            s.fields.insert(o as u32, Field::Word);
            claimed[o..o + 4].fill(true);
        }
    }
    Some(s)
}
