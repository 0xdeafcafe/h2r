//! Tags out of a retail map ([`MapSource`]).
//!
//! Each tag is walked with its group's layout from [`crate::schema`]:
//!
//! - a tag [`Relocator::check`] accepts is taken whole;
//! - a tag in a BSP region (`sbsp`, `ltmp`) is checked the same way against
//!   that region's bytes;
//! - any other tag (every `scnr`, a few others) is taken field by field: the
//!   root's words must hold no undescribed address, and each root block is
//!   checked on its own ([`Relocator::block_field`]). A block the scenario
//!   editor grew outside every tag is read from the file when its layout has
//!   no block or raw field. A block that cannot be taken is left empty and
//!   recorded in [`super::Tree::losses`];
//! - a tag with no bytes of its own (index size 0) becomes an alias of the
//!   tag holding its address.
//!
//! Raw resources are read from the map or the shared cache their pointer
//! names. A resource's length is its paired size field when the schema has
//! one (with two known pairs the inference misses: a bitmap's first level of
//! detail, a sound chunk), otherwise up to the next raw start of that file.
use super::store::PutReport;
use super::{
    Body, Error, LooseTag, Node, Origin, RawRes, Region, Result, TagKey, TagSource, Tree, Value,
    sha256,
};
use crate::schema::{
    Field, Memory, Refusal, Relocator, Schema, Struct, check_subtree_with, stray_with, u32_at,
    visit, walk,
};
use map::{FourCC, Map, ResourceFile, TagId};
use std::collections::HashMap;
use std::io::{Read, Seek};

/// A raw-size field the schema does not pair: `(group, block path, raw
/// offset, size offset, mask)`.
/// `(group, block path, raw offset, size offset, mask)`.
type SizeOverride = (&'static [u8; 4], &'static [u32], u32, u32, u32);

const SIZE_OVERRIDES: [SizeOverride; 2] = [
    // A bitmap image's level 0 (`0x1c`) and its byte length (`0x34`).
    (b"bitm", &[0x44], 0x1c, 0x34, u32::MAX),
    // A sound gestalt chunk: pointer, then size with a flag in the top bit.
    (b"ugh!", &[0x40], 0x0, 0x4, 0x7fff_ffff),
];

/// Groups whose paired raw sizes are noise (one instance per map): the
/// length is at least the distance to the next raw start.
const NOISY_SIZES: [&[u8; 4]; 2] = [b"matg", b"scnr"];

/// One cache's file and its raw starts.
pub struct RawFile<R> {
    map: Map<R>,
    /// Sorted local raw starts (and region bounds) of the file.
    starts: Vec<u32>,
    len: u64,
}

impl<R: Read + Seek> RawFile<R> {
    /// Index the raw starts of `map`.
    pub fn new(schema: &Schema, mut map: Map<R>) -> Result<Self> {
        let starts = raw_starts(schema, &mut map)?;
        let len = map.storage_length();
        Ok(RawFile { map, starts, len })
    }

    fn read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>> {
        Ok(self.map.read_file_range(offset.into(), len.into())?)
    }

    /// Length of the resource at `offset`: `size` when trusted, else up to
    /// the next start that is not a sibling (and at least `size`).
    fn extent(&self, offset: u32, size: Option<u32>, exact: bool, siblings: &[u32]) -> u32 {
        let cap = u32::try_from(self.len.saturating_sub(u64::from(offset))).unwrap_or(u32::MAX);
        if exact && let Some(s) = size.filter(|s| *s > 0) {
            return s.min(cap);
        }
        let i = self.starts.partition_point(|&x| x <= offset);
        let next = self.starts[i..]
            .iter()
            .copied()
            .find(|x| !siblings.contains(x))
            .unwrap_or(offset.saturating_add(cap));
        size.unwrap_or(0).max(next - offset).min(cap)
    }
}

/// The three shared caches, for pointers into them. Any may be missing; a
/// tag pointing into a missing one fails to extract.
pub struct SharedFiles<R> {
    /// `mainmenu.map`, `shared.map`, `single_player_shared.map`.
    pub files: [Option<RawFile<R>>; 3],
    /// Hashes of shared resources already read: (file, offset, length).
    hashes: HashMap<(usize, u32, u32), [u8; 32]>,
    /// Take a pointer into a cache that was not supplied as its place
    /// alone (no length unless stored, no hash), instead of refusing.
    pub unread_ok: bool,
}

impl<R: Read + Seek> Default for SharedFiles<R> {
    fn default() -> Self {
        SharedFiles {
            files: [None, None, None],
            hashes: HashMap::new(),
            unread_ok: false,
        }
    }
}

impl<R: Read + Seek> SharedFiles<R> {
    /// Add a shared cache by its file.
    pub fn set(&mut self, schema: &Schema, file: ResourceFile, map: Map<R>) -> Result<()> {
        if let Some(i) = shared_slot(file) {
            self.files[i] = Some(RawFile::new(schema, map)?);
        }
        Ok(())
    }
}

fn shared_slot(f: ResourceFile) -> Option<usize> {
    match f {
        ResourceFile::Local => None,
        ResourceFile::MainMenu => Some(0),
        ResourceFile::Shared => Some(1),
        ResourceFile::SinglePlayerShared => Some(2),
    }
}

/// Every local raw pointer any tag of `map` holds (whether its tag
/// relocates or not), the BSP regions, the low-detail region and the tag
/// index: the next one bounds a resource with no stored length.
fn raw_starts<R: Read + Seek>(schema: &Schema, map: &mut Map<R>) -> Result<Vec<u32>> {
    let mem = Memory::of(map)?;
    let meta = map.metadata_space();
    let mut starts = Vec::new();
    for t in map.tags().to_vec() {
        let (Some(root), Some(l)) = (schema.groups.get(&t.group), t.location) else {
            continue;
        };
        if Some(l.space) != meta {
            starts.push(u32::try_from(l.space.file_offset).unwrap_or(u32::MAX));
            continue;
        }
        if l.size == 0 {
            continue;
        }
        let _ = visit(root, l.address, &mem, &mut |s, e, _| {
            for (&o, f) in &s.fields {
                if matches!(f, Field::Raw) {
                    let v = u32_at(e, o as usize);
                    if v != u32::MAX && v != 0 && v >> 30 == 0 {
                        starts.push(v);
                    }
                }
            }
        });
    }
    let h = map.header();
    starts.push(h.index_offset);
    if h.low_detail_size > 0 {
        starts.push(h.low_detail_offset);
        starts.push(h.low_detail_offset + h.low_detail_size);
    }
    starts.sort_unstable();
    starts.dedup();
    Ok(starts)
}

/// Why a tag did not come out of a map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// The tag.
    pub key: TagKey,
    /// Why, readably.
    pub why: String,
}

/// A retail map as a [`TagSource`]. Raw resources are read on demand, from
/// the map or the shared caches.
pub struct MapSource<'s, R> {
    schema: &'s Schema,
    map: RawFile<R>,
    shared: &'s mut SharedFiles<R>,
    rel: Relocator<'s>,
    tags: Vec<map::Tag>,
    by_key: HashMap<TagKey, usize>,
    by_datum: HashMap<u32, usize>,
    strings: Vec<Option<String>>,
    groups: Vec<map::TagGroup>,
    /// Where each raw resource read so far is: hash → (file, offset, len).
    raws: HashMap<[u8; 32], (ResourceFile, u32, u32)>,
    /// BSP-region memory per region base, built on first use.
    bsp_memory: HashMap<u64, Memory>,
    /// Keep the bytes of every raw resource read, for [`MapSource::take_raw`].
    pub keep_raw: bool,
    /// Which cache this map is to the others (`Local` for a level).
    own_file: ResourceFile,
    kept: Vec<([u8; 32], Vec<u8>)>,
}

impl<'s, R: Read + Seek> MapSource<'s, R> {
    /// Open `map`; pointers into the shared caches read from `shared`.
    pub fn new(
        schema: &'s Schema,
        mut map: Map<R>,
        shared: &'s mut SharedFiles<R>,
    ) -> Result<Self> {
        let rel = Relocator::new(schema, &mut map)?;
        let own_file = match map.header().cache_type {
            map::CacheType::MainMenu => ResourceFile::MainMenu,
            map::CacheType::Shared => ResourceFile::Shared,
            map::CacheType::SinglePlayerShared => ResourceFile::SinglePlayerShared,
            _ => ResourceFile::Local,
        };
        let tags = map.tags().to_vec();
        let strings = map.strings().to_vec();
        let groups = map.groups().to_vec();
        let mut by_key = HashMap::new();
        let mut by_datum = HashMap::new();
        for (i, t) in tags.iter().enumerate() {
            by_key.insert(TagKey::new(t.group, t.name.clone().unwrap_or_default()), i);
            by_datum.insert(t.id.0, i);
        }
        Ok(MapSource {
            schema,
            map: RawFile::new(schema, map)?,
            shared,
            rel,
            tags,
            by_key,
            by_datum,
            strings,
            groups,
            raws: HashMap::new(),
            bsp_memory: HashMap::new(),
            keep_raw: false,
            own_file,
            kept: Vec::new(),
        })
    }

    /// Words outside every field that are data on this map, not pointers
    /// ([`Memory::data_words`]): a caller that has checked them keeps the
    /// tags holding them from being taken with losses.
    pub fn data_words(&mut self, words: &[u32]) {
        self.rel.memory_mut().data_words(words);
    }

    /// Raw resources read since the last call (with [`MapSource::keep_raw`]).
    pub fn take_raw(&mut self) -> Vec<([u8; 32], Vec<u8>)> {
        std::mem::take(&mut self.kept)
    }

    /// The map.
    pub fn map(&mut self) -> &mut Map<R> {
        &mut self.map.map
    }

    /// The map's string-ID table.
    pub fn strings(&self) -> &[Option<String>] {
        &self.strings
    }

    /// The key of the tag with this datum.
    pub fn key_of(&self, id: TagId) -> Option<TagKey> {
        let t = &self.tags[*self.by_datum.get(&id.0)?];
        Some(TagKey::new(t.group, t.name.clone().unwrap_or_default()))
    }

    /// Every tag in slot order.
    pub fn tags(&self) -> &[map::Tag] {
        &self.tags
    }

    /// Extract one tag, or say why not.
    pub fn extract(&mut self, id: TagId) -> Result<std::result::Result<LooseTag, Refused>> {
        let i = *self
            .by_datum
            .get(&id.0)
            .ok_or_else(|| Error::Invalid(format!("no tag {id}")))?;
        let t = self.tags[i].clone();
        let key = TagKey::new(t.group, t.name.clone().unwrap_or_default());
        let refuse = |why: String| {
            Ok(Err(Refused {
                key: key.clone(),
                why,
            }))
        };
        let Some(l) = t.location else {
            return refuse("no data".into());
        };
        let Some(root) = self.schema.groups.get(&t.group) else {
            return refuse("no layout for the group".into());
        };
        let meta = self.map.map.metadata_space();
        if Some(l.space) != meta {
            return self.extract_bsp(&t, key, root);
        }
        if l.size == 0 {
            let Some(owner) = self.rel.owner(l.address) else {
                return refuse("alias of no tag".into());
            };
            let start = self.rel.memory().region(l.address).expect("owned").0;
            if l.address != start {
                // Inside another tag: a depth-first copy of the owner has no
                // fixed offset for it, so the tag gets its own copy of the
                // bytes its layout reaches.
                return self.extract_inner(key, root, l.address);
            }
            let owner = self.key_of(owner).expect("owner in the index");
            return Ok(Ok(LooseTag {
                key,
                body: Body::Alias {
                    owner,
                    offset: l.address - start,
                },
            }));
        }
        let mut w = Walker::new(self, t.group);
        let whole = w.src.rel.check(id);
        let result = match whole {
            Ok(_) => w.node(root, l.address, 1, &mut Vec::new(), None),
            Err(_) => w.by_field(root, l.address),
        };
        match result {
            Ok(_) => {
                let tree = w.finish();
                Ok(Ok(LooseTag {
                    key,
                    body: Body::Tree(tree),
                }))
            }
            Err(WalkError::Refused(why)) => refuse(why),
            Err(WalkError::Fatal(e)) => Err(e),
        }
    }

    /// A tag whose bytes start inside another tag's, as a tree of its own.
    fn extract_inner(
        &mut self,
        key: TagKey,
        root: &'s Struct,
        address: u32,
    ) -> Result<std::result::Result<LooseTag, Refused>> {
        let (start, bytes) = self.rel.memory().region(address).expect("owned");
        let own = (start, start + bytes.len() as u32);
        if let Err(d) = check_subtree_with(
            root,
            address,
            1,
            own,
            self.rel.memory(),
            &mut Vec::new(),
            true,
        ) {
            return Ok(Err(Refused {
                key,
                why: format!(
                    "inside another tag: {:?} at {:x?} +{:#x}",
                    d.refusal, d.path, d.offset
                ),
            }));
        }
        let group = key.group;
        let mut w = Walker::new(self, group);
        match w.node(root, address, 1, &mut Vec::new(), None) {
            Ok(_) => Ok(Ok(LooseTag {
                key,
                body: Body::Tree(w.finish()),
            })),
            Err(WalkError::Refused(why)) => Ok(Err(Refused { key, why })),
            Err(WalkError::Fatal(e)) => Err(e),
        }
    }

    fn extract_bsp(
        &mut self,
        t: &map::Tag,
        key: TagKey,
        root: &'s Struct,
    ) -> Result<std::result::Result<LooseTag, Refused>> {
        let l = t.location.expect("located");
        let base = l.space.file_offset;
        if !self.bsp_memory.contains_key(&base) {
            // Every tag of this region, each its own region of memory, so
            // the walk checks tiling per tag.
            let mut regions = Vec::new();
            for o in self.tags.clone() {
                if let Some(ol) = o.location.filter(|ol| ol.space == l.space && ol.size > 0) {
                    regions.push((ol.address, self.map.map.read_tag_bytes(o.id)?));
                }
            }
            let mut spaces = vec![(
                l.space.virtual_base,
                l.space.virtual_base.saturating_add(l.space.size),
            )];
            if let Some(m) = self.map.map.metadata_space() {
                spaces.push((m.virtual_base, m.virtual_base.saturating_add(m.size)));
            }
            self.bsp_memory
                .insert(base, Memory::from_regions(regions, spaces));
        }
        let mem = self.bsp_memory.remove(&base).expect("built");
        let whole = walk(root, l.address, &mem).is_ok();
        let mut w = Walker::new(self, t.group);
        w.memory = Some(&mem);
        w.region = Region::Bsp;
        let result = if whole {
            w.node(root, l.address, 1, &mut Vec::new(), None)
        } else {
            w.by_field(root, l.address)
        };
        let out = match result {
            Ok(_) => {
                let tree = w.finish();
                Ok(Ok(LooseTag {
                    key,
                    body: Body::Tree(tree),
                }))
            }
            Err(WalkError::Refused(why)) => Ok(Err(Refused { key, why })),
            Err(WalkError::Fatal(e)) => Err(e),
        };
        self.bsp_memory.insert(base, mem);
        out
    }

    /// Where a raw resource this source has read sits: (file, offset, length).
    pub fn raw_place(&self, hash: &[u8; 32]) -> Option<(ResourceFile, u32, u32)> {
        self.raws.get(hash).copied()
    }

    /// Bytes of one of the files this source reads.
    pub fn read_raw(&mut self, file: ResourceFile, offset: u32, len: u32) -> Result<Vec<u8>> {
        match shared_slot(file) {
            None => self.map.read(offset, len),
            Some(i) => self.shared.files[i]
                .as_mut()
                .ok_or_else(|| Error::Invalid(format!("no {file:?} cache to read")))?
                .read(offset, len),
        }
    }

    /// The low-detail bitmap region (header `+0x158`, `+0x15c`).
    pub fn low_detail(&mut self) -> Result<Vec<u8>> {
        let h = self.map.map.header().clone();
        if h.low_detail_size == 0 {
            return Ok(Vec::new());
        }
        self.map.read(h.low_detail_offset, h.low_detail_size)
    }
}

enum WalkError {
    Refused(String),
    Fatal(Error),
}

impl From<Error> for WalkError {
    fn from(e: Error) -> Self {
        WalkError::Fatal(e)
    }
}

/// Builds one tag's nodes.
struct Walker<'a, 's, R> {
    src: &'a mut MapSource<'s, R>,
    group: FourCC,
    /// Memory to read arrays from (a BSP region), else the map's metadata.
    memory: Option<&'a Memory>,
    region: Region,
    nodes: Vec<Node>,
    /// (address or file offset, byte length, layout, from the file) → node.
    seen: HashMap<(u32, u32, usize, bool), u32>,
    raws: Vec<RawRes>,
    raw_index: HashMap<RawRes, u32>,
    losses: Vec<String>,
}

impl<'a, 's, R: Read + Seek> Walker<'a, 's, R> {
    fn new(src: &'a mut MapSource<'s, R>, group: FourCC) -> Self {
        Walker {
            src,
            group,
            memory: None,
            region: Region::Metadata,
            nodes: Vec::new(),
            seen: HashMap::new(),
            raws: Vec::new(),
            raw_index: HashMap::new(),
            losses: Vec::new(),
        }
    }

    fn finish(self) -> Tree {
        Tree {
            region: self.region,
            nodes: self.nodes,
            raws: self.raws,
            losses: self.losses,
        }
    }

    fn slice(&self, at: u32, n: u32) -> Option<Vec<u8>> {
        let mem = self.memory.unwrap_or(self.src.rel.memory());
        mem.slice(at, n).map(<[u8]>::to_vec)
    }

    /// `count` elements of `s` at `at` (or these `file` bytes, read from
    /// outside every tag) as a node; returns its index.
    fn node(
        &mut self,
        s: &Struct,
        at: u32,
        count: u32,
        path: &mut Vec<u32>,
        file: Option<Vec<u8>>,
    ) -> std::result::Result<u32, WalkError> {
        let n = s
            .size
            .checked_mul(count)
            .ok_or_else(|| WalkError::Refused("array too large".into()))?;
        let key = (at, n, s as *const Struct as usize, file.is_some());
        if let Some(&i) = self.seen.get(&key) {
            return Ok(i);
        }
        if path.len() > 64 {
            return Err(WalkError::Refused("blocks nested too deep".into()));
        }
        let mut bytes = match file {
            Some(b) => b,
            None => self
                .slice(at, n)
                .ok_or_else(|| WalkError::Refused(format!("array {at:#x} out of bounds")))?,
        };
        let index = self.nodes.len() as u32;
        self.seen.insert(key, index);
        let data = s.size == 1 && s.fields.is_empty();
        self.nodes.push(Node {
            bytes: Vec::new(),
            count: if data { n } else { count },
            size: s.size,
            align: s.align.max(1),
            fields: Vec::new(),
        });
        let mut fields = Vec::new();
        for i in 0..count {
            let e = i * s.size;
            for (&o, f) in &s.fields {
                let at_f = e + o;
                let w = |b: &[u8], k: u32| u32_at(b, (at_f + k) as usize);
                match f {
                    Field::Block(child) => {
                        let c = w(&bytes, 0);
                        let a = w(&bytes, 4);
                        if c == 0 {
                            // Empty: the builder writes address zero.
                            bytes[(at_f + 4) as usize..(at_f + 8) as usize].fill(0);
                            continue;
                        }
                        if a == 0 {
                            // A count with a null address: bytes kept.
                            continue;
                        }
                        path.push(o);
                        let sub = self.node(child, a, c, path, None)?;
                        path.pop();
                        bytes[(at_f + 4) as usize..(at_f + 8) as usize].fill(0);
                        let is_data = child.size == 1 && child.fields.is_empty();
                        fields.push((
                            at_f,
                            if is_data {
                                Value::Data(sub)
                            } else {
                                Value::Block(sub)
                            },
                        ));
                    }
                    Field::TagRef | Field::TagId => {
                        let d_at = if matches!(f, Field::TagRef) { 4 } else { 0 };
                        let d = w(&bytes, d_at);
                        if d == u32::MAX {
                            continue;
                        }
                        let p = (at_f + d_at) as usize;
                        match self.src.key_of(TagId(d)) {
                            Some(k) => {
                                bytes[p..p + 4].fill(0);
                                fields.push((at_f + d_at, Value::Ref(k)));
                            }
                            // A datum naming no tag: null, as the engine
                            // would find it.
                            None => bytes[p..p + 4].fill(0xff),
                        }
                    }
                    Field::StringId => {
                        let v = w(&bytes, 0);
                        if v == 0 {
                            continue;
                        }
                        let text = self
                            .src
                            .strings
                            .get((v & 0x00ff_ffff) as usize)
                            .cloned()
                            .flatten()
                            .ok_or_else(|| {
                                WalkError::Refused(format!("unknown string ID {v:#x}"))
                            })?;
                        bytes[at_f as usize..(at_f + 4) as usize].fill(0);
                        fields.push((at_f, Value::String(text)));
                    }
                    Field::Raw => {
                        let v = w(&bytes, 0);
                        if v == 0 || v == u32::MAX {
                            continue;
                        }
                        let r =
                            self.raw(s, &bytes[e as usize..(e + s.size) as usize], o, v, path)?;
                        bytes[at_f as usize..(at_f + 4) as usize].fill(0);
                        fields.push((at_f, Value::Raw(r)));
                    }
                    Field::Bsp => {
                        if (0..3).all(|k| w(&bytes, k * 4) == 0) {
                            continue;
                        }
                        bytes[at_f as usize..(at_f + 12) as usize].fill(0);
                        fields.push((at_f, Value::Bsp));
                    }
                    Field::Word | Field::RawSize(_) => {}
                }
            }
        }
        fields.sort_by_key(|f| f.0);
        let node = &mut self.nodes[index as usize];
        node.bytes = bytes;
        node.fields = fields;
        Ok(index)
    }

    /// The raw resource of field `o` (value `v`) in element `elem` of `s`.
    fn raw(
        &mut self,
        s: &Struct,
        elem: &[u8],
        o: u32,
        v: u32,
        path: &[u32],
    ) -> std::result::Result<u32, WalkError> {
        let p = map::RawPointer::decode(v).expect("not null");
        let mut size = s.fields.iter().find_map(|(&k, f)| {
            matches!(f, Field::RawSize(of) if *of == o).then(|| u32_at(elem, k as usize))
        });
        let mut exact = size.is_some() && !NOISY_SIZES.iter().any(|g| self.group == FourCC(**g));
        for (g, p_, ro, so, mask) in SIZE_OVERRIDES {
            if self.group == FourCC(*g) && path == p_ && o == ro {
                size = Some(u32_at(elem, so as usize) & mask);
                exact = true;
            }
        }
        let siblings: Vec<u32> = s
            .fields
            .iter()
            .filter(|(k, f)| **k != o && matches!(f, Field::Raw))
            .map(|(&k, _)| u32_at(elem, k as usize))
            .filter(|x| *x >> 30 == v >> 30)
            .map(|x| x & 0x3fff_ffff)
            .collect();
        let len = match shared_slot(p.file) {
            None => self.src.map.extent(p.offset, size, exact, &siblings),
            Some(i) => match self.src.shared.files[i].as_ref() {
                Some(f) => f.extent(p.offset, size, exact, &siblings),
                None if self.src.shared.unread_ok => {
                    // No cache to read: the pointer alone (length as
                    // stored, no hash). Enough to compare places.
                    let r = RawRes {
                        origin: Origin::External {
                            file: p.file,
                            offset: p.offset,
                        },
                        len: size.unwrap_or(0),
                        hash: [0; 32],
                    };
                    return Ok(self.push_raw(r));
                }
                None => {
                    return Err(WalkError::Refused(format!(
                        "points into {:?}, not supplied",
                        p.file
                    )));
                }
            },
        };
        let cached = shared_slot(p.file)
            .and_then(|i| self.src.shared.hashes.get(&(i, p.offset, len)).copied());
        let hash = match cached {
            Some(h) => h,
            None => {
                let bytes = self.src.read_raw(p.file, p.offset, len)?;
                let hash = sha256(&bytes);
                if let Some(i) = shared_slot(p.file) {
                    self.src.shared.hashes.insert((i, p.offset, len), hash);
                }
                if self.src.keep_raw {
                    self.src.kept.push((hash, bytes));
                }
                hash
            }
        };
        self.src.raws.insert(hash, (p.file, p.offset, len));
        // A shared cache's own resources are where every other map points
        // at them: the same tag reads the same from either.
        let file = match p.file {
            ResourceFile::Local => self.src.own_file,
            f => f,
        };
        let origin = match file {
            ResourceFile::Local => Origin::Local,
            file => Origin::External {
                file,
                offset: p.offset,
            },
        };
        Ok(self.push_raw(RawRes { origin, len, hash }))
    }

    fn push_raw(&mut self, r: RawRes) -> u32 {
        if let Some(&i) = self.raw_index.get(&r) {
            return i;
        }
        let i = self.raws.len() as u32;
        self.raws.push(r.clone());
        self.raw_index.insert(r, i);
        i
    }

    /// A tag the whole-tag check refuses, one root field at a time (see the
    /// module docs).
    fn by_field(&mut self, root: &Struct, address: u32) -> std::result::Result<u32, WalkError> {
        let mem = self.memory.unwrap_or(self.src.rel.memory());
        let (start, bytes) = mem
            .region(address)
            .filter(|r| r.0 == address)
            .ok_or_else(|| WalkError::Refused("no bytes".into()))?;
        let own_range = (start, start + bytes.len() as u32);
        let own = mem
            .slice(address, root.size)
            .ok_or_else(|| WalkError::Refused("smaller than its root".into()))?
            .to_vec();
        // Root words other than block fields must hold no address. Block
        // fields are checked below, so blank them for this check.
        let mut blanked = own.clone();
        for (&o, f) in &root.fields {
            if matches!(f, Field::Block(_)) {
                blanked[o as usize..o as usize + 8].fill(0);
            }
        }
        if let Some((k, v)) = stray_with(root, &blanked, own_range, mem, true) {
            return Err(WalkError::Refused(format!(
                "{:?} in the root at +{k:#x} ({v:#x})",
                Refusal::UnknownPointer
            )));
        }
        let in_metadata = self.memory.is_none();
        let mut root_bytes = own;
        let mut children = Vec::new();
        for (&o, f) in &root.fields {
            let Field::Block(child) = f else { continue };
            let c = u32_at(&root_bytes, o as usize);
            let a = u32_at(&root_bytes, o as usize + 4);
            if c == 0 {
                root_bytes[o as usize + 4..o as usize + 8].fill(0);
                continue;
            }
            if a == 0 {
                continue;
            }
            let mem = self.memory.unwrap_or(self.src.rel.memory());
            match check_subtree_with(child, a, c, own_range, mem, &mut vec![o], true) {
                Ok(()) => children.push((o, child.as_ref(), a, c, None)),
                Err(d) => {
                    let flat = child
                        .fields
                        .values()
                        .all(|f| !matches!(f, Field::Block(_) | Field::Raw));
                    let read = (flat && in_metadata && d.refusal == Refusal::OutOfBounds)
                        .then(|| {
                            let n = c.checked_mul(child.size)?;
                            let off = self.src.map.map.file_offset(a, n).ok()?;
                            self.src.map.map.read_file_range(off, n.into()).ok()
                        })
                        .flatten();
                    match read {
                        Some(b) => children.push((o, child.as_ref(), a, c, Some(b))),
                        None => {
                            self.losses.push(format!(
                                "block +{o:#x} ({c} elements) left empty: {:?} at {:x?} +{:#x} ({:#x})",
                                d.refusal, d.path, d.offset, d.value
                            ));
                            root_bytes[o as usize..o as usize + 8].fill(0);
                        }
                    }
                }
            }
        }
        // The root as a node of its own bytes; its block fields filled in
        // from the children checked above.
        let mut root_only = root.clone();
        root_only
            .fields
            .retain(|_, f| !matches!(f, Field::Block(_)));
        let index = self.node(&root_only, address, 1, &mut Vec::new(), Some(root_bytes))?;
        let mut extra = Vec::new();
        for (o, child, a, c, file) in children {
            let mut path = vec![o];
            let sub = self.node(child, a, c, &mut path, file)?;
            let is_data = child.size == 1 && child.fields.is_empty();
            extra.push((
                o,
                if is_data {
                    Value::Data(sub)
                } else {
                    Value::Block(sub)
                },
            ));
        }
        let node = &mut self.nodes[index as usize];
        for (o, _) in &extra {
            node.bytes[*o as usize + 4..*o as usize + 8].fill(0);
        }
        node.fields.extend(extra);
        node.fields.sort_by_key(|f| f.0);
        // Without the whole-tag check nothing says the arrays found cover
        // the tag: count the bytes they miss that are not padding.
        let mut spans: Vec<(u32, u32)> = self
            .seen
            .keys()
            .filter(|k| !k.3 && (own_range.0..own_range.1).contains(&k.0))
            .map(|k| (k.0 - own_range.0, k.1))
            .collect();
        spans.push((0, root.size));
        spans.sort_unstable();
        let mem = self.memory.unwrap_or(self.src.rel.memory());
        let tag = mem
            .slice(own_range.0, own_range.1 - own_range.0)
            .unwrap_or(&[]);
        let (mut pos, mut missed, mut gaps) = (0u32, 0u64, 0u32);
        let mut gap = |lo: u32, hi: u32| {
            let n = tag[lo as usize..hi as usize]
                .iter()
                .filter(|b| **b != 0 && **b != 0xcd)
                .count();
            if n > 0 && hi - lo >= 4 {
                missed += n as u64;
                gaps += 1;
            }
        };
        for (at, n) in spans {
            if at > pos {
                gap(pos, at);
            }
            pos = pos.max(at + n);
        }
        if (pos as usize) < tag.len() {
            gap(pos, tag.len() as u32);
        }
        if missed > 0 {
            self.losses.push(format!(
                "{missed} nonzero bytes in {gaps} ranges no array covers"
            ));
        }
        Ok(index)
    }
}

impl<R: Read + Seek> TagSource for MapSource<'_, R> {
    fn keys(&self) -> Vec<TagKey> {
        self.tags
            .iter()
            .map(|t| TagKey::new(t.group, t.name.clone().unwrap_or_default()))
            .collect()
    }

    fn read(&mut self, key: &TagKey) -> Result<Option<LooseTag>> {
        let Some(&i) = self.by_key.get(key) else {
            return Ok(None);
        };
        let id = self.tags[i].id;
        match self.extract(id)? {
            Ok(t) => Ok(Some(t)),
            Err(r) => Err(Error::Invalid(format!("{}: {}", r.key, r.why))),
        }
    }

    fn raw(&mut self, raw: &RawRes) -> Result<Vec<u8>> {
        let &(file, offset, len) = self
            .raws
            .get(&raw.hash)
            .ok_or_else(|| Error::Invalid(format!("raw {} not read from this map", raw.hex())))?;
        self.read_raw(file, offset, len)
    }

    fn groups(&self) -> Vec<map::TagGroup> {
        self.groups.clone()
    }
}

/// Extract every tag of `src` (the map `stem`) into `repo`, with its
/// manifest, header, low-detail region, group table and string IDs. With
/// `verify`, each stored file is read back and must decode to the same tag.
pub fn store_map<R: Read + Seek>(
    repo: &mut super::TagRepo,
    src: &mut MapSource<'_, R>,
    stem: &str,
    verify: bool,
) -> Result<PutReport> {
    use super::store::{Manifest, Stored};
    let mut report = PutReport::default();
    src.keep_raw = true;
    let h = src.map.map.header().clone();
    let header = src.map.read(0, 0x800)?;
    let word14 = src.map.read(h.index_offset + 0x14, 4)?;
    let mut m = Manifest {
        map: stem.to_owned(),
        cache_type: u32::from_le_bytes(header[0x140..0x144].try_into().expect("4")),
        name: h.name.clone(),
        index_word_14: u32::from_le_bytes(word14[..4].try_into().expect("4")),
        ..Default::default()
    };
    let scenario = src.map.map.scenario().map(|t| t.id);
    let globals = src.map.map.globals().map(|t| t.id);
    m.scenario = scenario.and_then(|id| src.key_of(id));
    m.globals = globals.and_then(|id| src.key_of(id));
    let low = src.low_detail()?;
    if !low.is_empty() {
        m.low_detail = Some(repo.put_raw(&low)?);
    }
    repo.merge_groups(&src.groups)?;
    repo.merge_strings(&src.strings)?;
    let ids: Vec<TagId> = src.tags.iter().map(|t| t.id).collect();
    for id in ids {
        let tag = match src.extract(id)? {
            Ok(t) => t,
            Err(r) => {
                report.refused.push((r.key.clone(), r.why.clone()));
                m.refused.push((r.key, r.why));
                continue;
            }
        };
        for (hash, bytes) in src.take_raw() {
            let rr = RawRes {
                origin: Origin::Local,
                len: bytes.len() as u32,
                hash,
            };
            if !repo.has_raw(&rr) {
                repo.put_raw(&bytes)?;
                report.raw_new.0 += 1;
                report.raw_new.1 += bytes.len() as u64;
            }
        }
        if let Some(t) = tag.tree()
            && !t.losses.is_empty()
        {
            report.lossy.push(tag.key.clone());
        }
        let variant = match repo.put(&tag)? {
            Stored::New => {
                report.new += 1;
                None
            }
            Stored::Same => {
                report.same += 1;
                None
            }
            Stored::Variant(v) => {
                report.variants.insert(tag.key.clone(), v.clone());
                Some(v)
            }
        };
        if verify {
            let p = repo.path_of(&tag.key, variant.as_deref());
            let back = super::format::decode(&std::fs::read(&p)?)?;
            if back != tag {
                return Err(Error::Invalid(format!(
                    "{} did not read back the same",
                    tag.key
                )));
            }
        }
        m.tags.push((tag.key, variant));
    }
    repo.put_manifest(&m, &header)?;
    src.keep_raw = false;
    Ok(report)
}

/// How a rebuilt map compares with its source.
#[derive(Clone, Debug, Default)]
pub struct Comparison {
    /// Tags whose canonical forms (and local raw bytes) match.
    pub same: usize,
    /// Tags that differ, with the first differing line of each side.
    pub differ: Vec<(TagKey, String)>,
    /// Tags of the rebuilt map the source lacks.
    pub missing: Vec<TagKey>,
    /// Tags either side could not extract.
    pub refused: Vec<(TagKey, String)>,
    /// Local raw resources compared byte for byte.
    pub raw_compared: usize,
    /// Source tags with losses (their canonical forms are compared without
    /// the loss lines: both sides lose the same).
    pub lossy: Vec<TagKey>,
}

/// Compare every tag of `built` with the tag of the same key in `source`:
/// canonical forms with raw resources by place, then each local resource's
/// bytes (the source's length, read at the rebuilt map's pointer).
pub fn compare<R1: Read + Seek, R2: Read + Seek>(
    built: &mut MapSource<'_, R1>,
    source: &mut MapSource<'_, R2>,
) -> Result<Comparison> {
    let mut out = Comparison::default();
    for key in built.keys() {
        let Some(&si) = source.by_key.get(&key) else {
            out.missing.push(key);
            continue;
        };
        let b = built.extract(built.tags[built.by_key[&key]].id)?;
        let s = source.extract(source.tags[si].id)?;
        let (b, s) = match (b, s) {
            (Ok(b), Ok(s)) => (b, s),
            (Err(r), _) | (_, Err(r)) => {
                out.refused.push((key, r.why));
                continue;
            }
        };
        if s.tree().is_some_and(|t| !t.losses.is_empty()) {
            out.lossy.push(key.clone());
        }
        let strip = |(c, l): (Vec<String>, Vec<RawRes>)| {
            (
                c.into_iter()
                    .filter(|x| !x.starts_with("loss "))
                    .collect::<Vec<_>>(),
                l,
            )
        };
        let (cb, lb) = strip(super::canonical_with(&b, false));
        let (cs, ls) = strip(super::canonical_with(&s, false));
        if cb != cs {
            let i = cb
                .iter()
                .zip(&cs)
                .position(|(x, y)| x != y)
                .unwrap_or(cb.len().min(cs.len()));
            let line = |c: &[String]| c.get(i).cloned().unwrap_or_else(|| "(end)".into());
            out.differ.push((
                key,
                format!("line {i}: built {} / source {}", line(&cb), line(&cs)),
            ));
            continue;
        }
        let mut ok = true;
        for (rb, rs) in lb.iter().zip(&ls) {
            let Some((file, offset, _)) = built.raw_place(&rb.hash) else {
                ok = false;
                break;
            };
            let bytes = built.read_raw(file, offset, rs.len)?;
            out.raw_compared += 1;
            if super::sha256(&bytes) != rs.hash {
                ok = false;
                break;
            }
        }
        if ok {
            out.same += 1;
        } else {
            out.differ.push((key, "local raw bytes differ".into()));
        }
    }
    Ok(out)
}
