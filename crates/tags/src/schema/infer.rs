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
        for t in &list {
            let Some(loc) = t.location else { continue };
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
    schema
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
    Some(s)
}
