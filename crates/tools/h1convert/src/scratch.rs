//! A map assembled **from scratch**: an empty tag set filled by tag path
//! through `tags::repo` ([`TagSource`], [`closure`], [`to_tag_set`]), with
//! no slot of a base map's index and no byte of its file.
//!
//! [`build`] takes any [`TagSource`] for the tags it reads by path, and a
//! [`TagSet`] for the tags it takes as they are ([`BUILT_HERE`]: the
//! scenario and structure BSPs a conversion wrote, which the repository
//! cannot take exactly). Those are moved across ([`Transfer`]): every datum
//! in them (references, and the words scripts and the simulation table
//! hold) renamed for the new set, their raw copied, their BSP regions made
//! anew.
//!
//! The sound gestalt is then cut to the sounds kept ([`crate::compact`]).
//!
//! The roots are the globals, the [`Keep`] groups and path prefixes the set
//! holds, and every tag the scenario and BSPs name. What nothing reaches is
//! never read: there is nothing to prune.
//!
//! [`LevelSource`] is the source a conversion uses: its own tags (built, or
//! base tags it edited) from the map it wrote, the rest from the retail
//! base map as it is on the disc. Another source (tags written from typed
//! definitions) plugs in the same way.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Seek};

use map::build::{Block, FixupKind, RawId, RawRef, TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use tags::repo::{
    Body, BuildOptions, LooseTag, MapSource, Node, RawRes, Region, TagKey, TagSource, Tree,
    Value, closure, to_tag_set,
};
use tags::schema::{Field, Struct};

use crate::fresh::Keep;
use crate::{Error, Result};

/// A map source over bytes in memory.
pub type Src<'s> = MapSource<'s, Cursor<&'s [u8]>>;

/// The groups taken as the set holds them rather than through the
/// repository: the scenario and its BSPs.
pub const BUILT_HERE: [&[u8; 4]; 3] = [b"scnr", b"sbsp", b"ltmp"];

/// What [`build`] did.
#[derive(Debug, Clone, Default)]
pub struct ScratchReport {
    /// Tags in the map.
    pub tags: usize,
    /// Of them, from the retail base, and from the conversion.
    pub from_base: usize,
    pub from_level: usize,
    /// Tags built here ([`BUILT_HERE`]).
    pub built_here: usize,
    /// Datum words rewritten in them.
    pub words: usize,
    /// The sound gestalt cut to the kept sounds ([`crate::compact`]), or
    /// why it stays whole.
    pub gestalt: Option<crate::compact::Compacted>,
    pub gestalt_whole: Option<String>,
}

fn invalid(s: impl Into<String>) -> Error {
    Error::Fresh(s.into())
}

/// Where `tree` names `target`: each field as its offsets from the root,
/// with the element index where an array has more than one.
fn ref_fields(tree: &Tree, target: &TagKey) -> Vec<String> {
    fn go(t: &Tree, n: u32, target: &TagKey, path: &str, depth: u32, out: &mut Vec<String>) {
        let Some(node) = t.nodes.get(n as usize).filter(|_| depth <= 64) else {
            return;
        };
        let size = node.size.max(1);
        for (o, v) in &node.fields {
            let at = if node.count > 1 {
                format!("{path}[{}]+{:#x}", o / size, o % size)
            } else {
                format!("{path}+{:#x}", o % size)
            };
            match v {
                Value::Ref(k) if k == target => out.push(at),
                Value::Block(c) | Value::Data(c) => go(t, *c, target, &at, depth + 1, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    go(tree, 0, target, "", 0, &mut out);
    out.dedup();
    out
}

/// The error for references to tags the map cannot hold: (the tag naming
/// one, the tag named, the fields naming it). A reference is never nulled:
/// the writer of the naming tag, or the conversion that should have built
/// the named one, is what to fix.
fn dangling(map: &str, refs: &[(TagKey, TagKey, Vec<String>)]) -> Error {
    let list: Vec<String> = refs
        .iter()
        .take(8)
        .map(|(from, to, fields)| {
            if from == to {
                format!("{to} (a root: the globals, the keep list, or named by the scenario or BSPs)")
            } else {
                format!("{from} {} -> {to}", fields.join(", "))
            }
        })
        .collect();
    invalid(format!(
        "{map}: {} references to tags no source holds: {}",
        refs.len(),
        list.join("; ")
    ))
}

/// The two sources as one: a key the conversion built or edited reads from
/// the intermediate map, any other from the retail base.
pub struct LevelSource<'s> {
    pub base: Src<'s>,
    pub level: Src<'s>,
    /// Keys read from `level`.
    pub from_level: HashSet<TagKey>,
}

impl TagSource for LevelSource<'_> {
    fn keys(&self) -> Vec<TagKey> {
        let mut k = self.level.keys();
        k.extend(self.base.keys());
        k.sort();
        k.dedup();
        k
    }

    fn read(&mut self, key: &TagKey) -> tags::repo::Result<Option<LooseTag>> {
        if self.from_level.contains(key) {
            self.level.read(key)
        } else {
            match self.base.read(key)? {
                Some(t) => Ok(Some(t)),
                None => self.level.read(key),
            }
        }
    }

    fn raw(&mut self, raw: &RawRes) -> tags::repo::Result<Vec<u8>> {
        self.level.raw(raw).or_else(|_| self.base.raw(raw))
    }

    fn groups(&self) -> Vec<map::TagGroup> {
        let mut g = self.level.groups();
        for b in self.base.groups() {
            if !g.iter().any(|x| x.class == b.class) {
                g.push(b);
            }
        }
        g
    }
}

/// The keys of `set` a conversion built or changed: built or aliased, or
/// pinned with bytes unlike the same tag's in `base` (the retail map the
/// set is pinned on). The scenario and BSPs aside ([`BUILT_HERE`]).
pub fn changed<R: Read + Seek>(base: &mut Map<R>, set: &TagSet) -> HashSet<TagKey> {
    let mut out = HashSet::new();
    for s in &set.slots {
        let Some(t) = &s.tag else { continue };
        if built_here(t.group) {
            continue;
        }
        let edited = match &t.payload {
            TagPayload::Built(_) | TagPayload::Alias { .. } => true,
            TagPayload::Pinned { bytes, .. } => base
                .tag(t.id)
                .ok()
                .filter(|b| b.name == s.name && b.group == t.group)
                .map(|b| b.id)
                .is_none_or(|id| base.read_tag_bytes(id).ok().as_deref() != Some(bytes)),
            TagPayload::Empty => false,
        };
        if edited {
            out.insert(TagKey::new(t.group, s.name.clone().unwrap_or_default()));
        }
    }
    out
}

/// The low-detail bitmap region of `base`'s file, if any.
pub fn low_detail<R: Read + Seek>(base: &mut Map<R>) -> Result<Option<Vec<u8>>> {
    let h = base.header().clone();
    Ok((h.low_detail_size > 0)
        .then(|| base.read_file_range(h.low_detail_offset.into(), h.low_detail_size.into()))
        .transpose()?)
}

fn key_of(set: &TagSet, id: TagId) -> Option<TagKey> {
    let s = set.slots.get(id.index())?;
    let t = s.tag.as_ref().filter(|t| t.id == id)?;
    Some(TagKey::new(t.group, s.name.clone().unwrap_or_default()))
}

fn built_here(g: FourCC) -> bool {
    BUILT_HERE.iter().any(|b| g == FourCC(**b))
}

/// The offsets in `b` (elements of `s`) of the schema's reference datums:
/// a [`Field::TagRef`]'s datum (`+4`) and a [`Field::TagId`].
fn ref_words(b: &Block, s: &Struct) -> Vec<usize> {
    let size = s.size.max(1) as usize;
    let mut out = Vec::new();
    for i in 0..b.bytes.len() / size {
        for (&o, f) in &s.fields {
            match f {
                Field::TagRef => out.push(i * size + o as usize + 4),
                Field::TagId => out.push(i * size + o as usize),
                _ => {}
            }
        }
    }
    out.retain(|&k| k + 4 <= b.bytes.len());
    out
}

/// The words of `b` that may name a datum of `set`: in the scenario's tree
/// (`untyped`) every 4-aligned word (the scripts' atoms, the simulation
/// table); elsewhere the schema's reference words (`s`) and every word
/// laid out as a reference, the group of the tag it names just before it.
/// A bare word match would take a BSP's vertex index pairs for datums.
fn datum_words(b: &Block, s: Option<&Struct>, set: &TagSet, untyped: bool) -> Vec<usize> {
    let all = (0..b.bytes.len() / 4 * 4).step_by(4);
    if untyped {
        return all.collect();
    }
    let word = |k: usize| u32::from_le_bytes(b.bytes[k..k + 4].try_into().expect("4"));
    let mut out: Vec<usize> = all
        .filter(|&k| k >= 4)
        .filter(|&k| {
            key_of(set, TagId(word(k))).is_some_and(|key| {
                let g = word(k - 4);
                g == u32::from_le_bytes(key.group.0) || g == u32::from_be_bytes(key.group.0)
            })
        })
        .collect();
    out.extend(s.map(|s| ref_words(b, s)).unwrap_or_default());
    out.sort_unstable();
    out.dedup();
    out
}

/// The layout of the child block at `offset` of a block of `s` elements.
fn child_struct(s: &Struct, offset: u32) -> Option<&Struct> {
    match s.fields.get(&(offset % s.size.max(1)))? {
        Field::Block(c) => Some(c),
        _ => None,
    }
}

/// Every datum `b` names: reference fixups and, in the scenario's tree
/// (`untyped`), every 4-aligned word equal to a datum of `set` (the
/// scripts' atoms, the simulation table); elsewhere only the words the
/// schema (`s`, the layout of `b`'s elements) marks as references.
fn datums(b: &Block, s: Option<&Struct>, set: &TagSet, untyped: bool, out: &mut Vec<TagId>) {
    for k in datum_words(b, s, set, untyped) {
        let d = TagId(u32::from_le_bytes(b.bytes[k..k + 4].try_into().expect("4")));
        if d.0 != u32::MAX && key_of(set, d).is_some() {
            out.push(d);
        }
    }
    for f in &b.fixups {
        match &f.kind {
            FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) => {
                let c = s.and_then(|s| child_struct(s, f.offset));
                datums(child, c, set, untyped, out)
            }
            FixupKind::TagRef(t) | FixupKind::TagId(t) => out.push(*t),
            FixupKind::TagOffset { tag, .. } => out.push(*tag),
            _ => {}
        }
    }
}

/// A one-node placeholder for a tag `Transfer` fills.
fn stub(key: TagKey) -> LooseTag {
    LooseTag {
        key,
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![Node {
                bytes: vec![0; 4],
                count: 1,
                size: 4,
                align: 4,
                fields: Vec::new(),
            }],
            raws: Vec::new(),
            losses: Vec::new(),
        }),
    }
}

/// Groups whose tags keep every block inside their own bytes, sharing only
/// within themselves (`ugh!` not at all: its reader takes no overlap). Our
/// readers take these tags' bytes alone (`tags::DecodedTag` for bitmaps,
/// `h1sounds::gestalt::SoundGestalt::decode`), and retail never shares
/// across them (V, 2026-11-18, 05a, Coagulation, 03a and 07a: no `bitm` or
/// `ugh!` array outside its tag). Every other group shares equal blocks
/// across tags, as retail does in 25 to 28 groups per map (05a: 377,044
/// bytes of `effe` arrays and 245,966 of `shad` lie in other tags).
pub const SELF_CONTAINED: [&[u8; 4]; 2] = [b"bitm", b"ugh!"];

/// Applies [`SELF_CONTAINED`] to `set`'s built tags.
pub fn scope_shares(set: &mut TagSet) {
    for slot in &mut set.slots {
        if let Some(t) = slot.tag.as_mut()
            && SELF_CONTAINED.iter().any(|g| t.group == FourCC(**g))
            && let TagPayload::Built(b) = &mut t.payload
        {
            let ugh = t.group == FourCC(*b"ugh!");
            own_shares(b, t.id.0, ugh);
        }
    }
}

/// Scope `b`'s share keys to the tag `owner` (or drop them, `none`).
fn own_shares(b: &mut Block, owner: u32, none: bool) {
    if none {
        b.share = None;
    }
    if let Some(k) = &mut b.share {
        *k = k.rotate_left(17) ^ u64::from(owner).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    for f in &mut b.fixups {
        if let FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) =
            &mut f.kind
        {
            own_shares(child, owner, none);
        }
    }
}

/// The map from scratch: every tag `src` gives that the roots reach, plus
/// `set`'s built scenario and BSPs moved across. `from_level` only sorts
/// the report's counts; `low_detail` is the header's low-detail region.
/// `extra` are more roots of `set` (tags nothing names that the game swaps
/// in, such as `boot::precision`'s collections).
pub fn build(
    src: &mut dyn TagSource,
    set: &TagSet,
    keep: &Keep,
    extra: &[TagId],
    from_level: &HashSet<TagKey>,
    low_detail: Option<Vec<u8>>,
) -> Result<(TagSet, ScratchReport)> {
    let repo = |e: tags::repo::Error| invalid(format!("from scratch: {e}"));
    let mut here = Vec::new();
    for s in &set.slots {
        let Some(t) = &s.tag else { continue };
        if built_here(t.group) && matches!(t.payload, TagPayload::Built(_)) {
            here.push((
                t.id,
                TagKey::new(t.group, s.name.clone().unwrap_or_default()),
            ));
        }
    }

    // Roots: the globals, the kept groups and prefixes, and what the
    // scenario and BSPs name.
    let globals = key_of(set, set.globals).ok_or_else(|| invalid("no globals"))?;
    let scenario = key_of(set, set.scenario).ok_or_else(|| invalid("no scenario"))?;
    let mut roots = vec![globals.clone()];
    for s in &set.slots {
        let Some(t) = &s.tag else { continue };
        let name = s.name.as_deref().unwrap_or("");
        if keep.groups.iter().any(|g| t.group == FourCC(**g)) && !built_here(t.group)
            || keep.prefixes.iter().any(|p| name.starts_with(p))
        {
            roots.push(TagKey::new(t.group, name));
        }
    }
    roots.extend(extra.iter().filter_map(|&d| key_of(set, d)));
    let mut named = Vec::new();
    for (id, key) in &here {
        if let Some(TagPayload::Built(b)) = set.tag(*id).map(|t| &t.payload) {
            let layout = tags::schema::Schema::builtin().groups.get(&key.group);
            datums(b, layout, set, key.group == FourCC(*b"scnr"), &mut named);
        }
    }
    for d in named {
        if let Some(k) = key_of(set, d).filter(|k| !built_here(k.group)) {
            roots.push(k);
        }
    }
    roots.sort();
    roots.dedup();
    let mut c = closure(src, &roots).map_err(repo)?;
    let mut report = ScratchReport::default();
    if !c.missing.is_empty() {
        let refs: Vec<_> = c
            .missing
            .iter()
            .map(|(to, from)| {
                let fields = c
                    .get(from)
                    .and_then(|t| t.tree())
                    .map(|t| ref_fields(t, to))
                    .unwrap_or_default();
                (from.clone(), to.clone(), fields)
            })
            .collect();
        return Err(dangling(&set.name, &refs));
    }
    c.tags.retain(|t| !built_here(t.key.group));
    // A tag the repository could not take exactly would be built short.
    let lossy: Vec<String> = c
        .tags
        .iter()
        .filter(|t| t.tree().is_some_and(|tr| !tr.losses.is_empty()))
        .map(|t| format!("{} ({})", t.key, t.tree().expect("tree").losses.join("; ")))
        .collect();
    if !lossy.is_empty() {
        return Err(invalid(format!(
            "{} tags not taken exactly: {}",
            lossy.len(),
            lossy[..lossy.len().min(6)].join(", ")
        )));
    }
    for t in &c.tags {
        if from_level.contains(&t.key) {
            report.from_level += 1;
        } else {
            report.from_base += 1;
        }
    }
    let mut tags = c.tags;
    tags.extend(here.iter().map(|(_, k)| stub(k.clone())));
    let opts = BuildOptions {
        cache_type: set.cache_type,
        name: set.name.clone(),
        scenario: scenario.clone(),
        globals,
        strings: set.strings.clone(),
        header_template: set
            .pinned
            .as_ref()
            .map(|p| p.header.clone())
            .or_else(|| set.header_template.clone()),
        low_detail,
        index_word_14: set.index_word_14,
    };
    let (mut out, built) = to_tag_set(src, &tags, &opts).map_err(repo)?;
    if !built.dangling.is_empty() {
        let refs: Vec<_> = built
            .dangling
            .iter()
            .map(|(from, to)| {
                let fields = tags
                    .iter()
                    .find(|t| &t.key == from)
                    .and_then(|t| t.tree())
                    .map(|t| ref_fields(t, to))
                    .unwrap_or_default();
                (from.clone(), to.clone(), fields)
            })
            .collect();
        return Err(dangling(&set.name, &refs));
    }
    scope_shares(&mut out);
    out.scenario_path = set.scenario_path.clone();

    // The scenario and BSPs, as built, rewritten for `out`.
    let new_id: HashMap<TagKey, TagId> = out
        .slots
        .iter()
        .filter_map(|s| {
            let t = s.tag.as_ref()?;
            Some((
                TagKey::new(t.group, s.name.clone().unwrap_or_default()),
                t.id,
            ))
        })
        .collect();
    let map_id = |d: TagId| key_of(set, d).and_then(|k| new_id.get(&k).copied());
    let mut cx = Transfer {
        set,
        map_id: &map_id,
        raws: HashMap::new(),
        sections: HashMap::new(),
        words: 0,
        untyped: false,
        error: None,
        tag: String::new(),
        path: Vec::new(),
    };
    let mut payloads = Vec::new();
    for (id, key) in &here {
        let Some(TagPayload::Built(b)) = set.tag(*id).map(|t| &t.payload) else {
            continue;
        };
        let mut b = b.clone();
        cx.untyped = key.group == FourCC(*b"scnr");
        cx.tag = key.to_string();
        cx.path.clear();
        cx.block(
            &mut b,
            tags::schema::Schema::builtin().groups.get(&key.group),
            &mut out,
        );
        payloads.push((new_id[key], b));
    }
    if let Some(e) = cx.error {
        return Err(e);
    }
    report.words = cx.words;
    for (id, b) in payloads {
        out.tag_mut(id).expect("added").payload = TagPayload::Built(b);
    }
    match crate::compact::gestalt(&mut out) {
        Ok(c) => report.gestalt = Some(c),
        Err(e) => report.gestalt_whole = Some(e.to_string()),
    }
    report.built_here = here.len();
    report.tags = out.slots.iter().filter(|s| s.tag.is_some()).count();
    Ok((out, report))
}

struct Transfer<'a> {
    set: &'a TagSet,
    map_id: &'a dyn Fn(TagId) -> Option<TagId>,
    raws: HashMap<usize, RawId>,
    sections: HashMap<usize, usize>,
    words: usize,
    /// Whether every word outside the fixups that equals a datum is
    /// renamed: in the scenario (script nodes, the simulation table). In a
    /// BSP or lightmap only the schema's reference fields are: their
    /// geometry holds index pairs that look like datums (a collision
    /// edge's two `u16` vertices).
    untyped: bool,
    error: Option<Error>,
    /// The tag being moved, and the block offsets down to the one being
    /// rewritten, for the error naming a datum the map does not hold.
    tag: String,
    path: Vec<u32>,
}

impl Transfer<'_> {
    /// `d` in the new set. A datum the new set does not hold is an error
    /// naming the field (`at`, in the block being rewritten), never null.
    fn id(&mut self, d: TagId, at: u32) -> TagId {
        if d == TagId::NULL {
            return d;
        }
        if let Some(n) = (self.map_id)(d) {
            return n;
        }
        let path: String = self
            .path
            .iter()
            .chain([&at])
            .map(|o| format!("+{o:#x}"))
            .collect();
        let named = key_of(self.set, d).map_or_else(|| format!("{d}"), |k| k.to_string());
        self.error.get_or_insert(invalid(format!(
            "{}: {} {path} names {named}, which the map does not hold",
            self.set.name, self.tag
        )));
        TagId::NULL
    }

    /// `b` (a tree of `self.set`) rewritten for `out`.
    fn block(&mut self, b: &mut Block, s: Option<&Struct>, out: &mut TagSet) {
        let covered: HashSet<u32> = b
            .fixups
            .iter()
            .flat_map(|f| {
                let n = match f.kind {
                    FixupKind::Block { .. }
                    | FixupKind::Data(_)
                    | FixupKind::TagRef(_)
                    | FixupKind::TagOffset { .. } => 8,
                    _ => 4,
                };
                f.offset..f.offset + n
            })
            .collect();
        let words = datum_words(b, s, self.set, self.untyped);
        for k in words {
            if covered.contains(&(k as u32)) {
                continue;
            }
            let d = TagId(u32::from_le_bytes(b.bytes[k..k + 4].try_into().expect("4")));
            if d.0 != u32::MAX && key_of(self.set, d).is_some() {
                let n = self.id(d, k as u32);
                b.bytes[k..k + 4].copy_from_slice(&n.0.to_le_bytes());
                self.words += 1;
            }
        }
        for f in &mut b.fixups {
            match &mut f.kind {
                FixupKind::Block { child, .. }
                | FixupKind::Data(child)
                | FixupKind::Pointer(child) => {
                    let c = s.and_then(|s| child_struct(s, f.offset));
                    self.path.push(f.offset);
                    self.block(child, c, out);
                    self.path.pop();
                }
                FixupKind::TagRef(t) | FixupKind::TagId(t) => *t = self.id(*t, f.offset),
                FixupKind::TagOffset { .. } => {
                    self.error.get_or_insert(Error::Fresh(
                        "a scenario or BSP tree points into another tag's bytes".into(),
                    ));
                }
                FixupKind::Raw(RawRef::Local(r)) => {
                    let new = match self.raws.get(&r.0) {
                        Some(&n) => n,
                        None => {
                            let blob = &self.set.raw[r.0];
                            let n = out.add_raw(blob.bytes.clone(), blob.align);
                            self.raws.insert(r.0, n);
                            n
                        }
                    };
                    *r = new;
                }
                FixupKind::Bsp { section, .. } => {
                    let new = match self.sections.get(section) {
                        Some(&n) => n,
                        None => {
                            let s = &self.set.bsps[*section];
                            let bsp = self.id(s.bsp, f.offset);
                            let lightmap = s.lightmap.map(|l| self.id(l, f.offset));
                            let n = out.add_bsp(bsp, lightmap);
                            self.sections.insert(*section, n);
                            n
                        }
                    };
                    *section = new;
                }
                _ => {}
            }
        }
    }
}

/// The whole file of `m`, read in pieces (single reads are capped).
pub fn file_bytes<R: Read + Seek>(m: &mut Map<R>) -> Result<Vec<u8>> {
    let len = m.storage_length();
    let mut out = Vec::with_capacity(len as usize);
    let mut at = 0u64;
    while at < len {
        let n = (len - at).min(32 << 20);
        out.extend(m.read_file_range(at, n)?);
        at += n;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use map::CacheType;

    fn tag(set: &mut TagSet, group: &[u8; 4], name: &str) -> TagId {
        let g = FourCC(*group);
        set.add_group(g, None, None);
        let child = Block::new(vec![7; 16]).shared(42);
        let mut root = Block::zeroed(8);
        root.block(0, 1, child);
        set.add_tag(g, name, TagPayload::Built(root)).unwrap()
    }

    fn key(set: &TagSet, id: TagId) -> Option<u64> {
        let TagPayload::Built(b) = &set.tag(id).unwrap().payload else {
            unreachable!()
        };
        match &b.fixups[0].kind {
            FixupKind::Block { child, .. } => child.share,
            _ => unreachable!(),
        }
    }

    #[test]
    fn bitmaps_and_the_gestalt_keep_their_blocks_to_themselves() {
        let mut set = TagSet::new(CacheType::Multiplayer, "m", "s");
        let (e1, e2) = (tag(&mut set, b"effe", "a"), tag(&mut set, b"effe", "b"));
        let (b1, b2) = (tag(&mut set, b"bitm", "a"), tag(&mut set, b"bitm", "b"));
        let u = tag(&mut set, b"ugh!", "g");
        scope_shares(&mut set);
        // Effects share across tags, as retail's do.
        assert_eq!(key(&set, e1), Some(42));
        assert_eq!(key(&set, e2), Some(42));
        // Bitmaps share only within themselves, never with each other or
        // with another group.
        let (k1, k2) = (key(&set, b1), key(&set, b2));
        assert!(k1.is_some() && k2.is_some());
        assert_ne!(k1, k2);
        assert_ne!(k1, Some(42));
        // The gestalt shares nothing.
        assert_eq!(key(&set, u), None);
    }

    /// An in-memory source.
    struct Mem(Vec<LooseTag>);

    impl TagSource for Mem {
        fn keys(&self) -> Vec<TagKey> {
            self.0.iter().map(|t| t.key.clone()).collect()
        }
        fn read(&mut self, key: &TagKey) -> tags::repo::Result<Option<LooseTag>> {
            Ok(self.0.iter().find(|t| &t.key == key).cloned())
        }
        fn raw(&mut self, _: &RawRes) -> tags::repo::Result<Vec<u8>> {
            Err(tags::repo::Error::Invalid("no raw".into()))
        }
        fn groups(&self) -> Vec<map::TagGroup> {
            Vec::new()
        }
    }

    /// A globals tag naming a bitmap no source holds: the build fails,
    /// naming the map, the tag, the field and the missing tag, where it
    /// used to null the reference and go on.
    #[test]
    fn a_reference_no_source_holds_fails_the_build_naming_the_field() {
        let mut set = TagSet::new(CacheType::Multiplayer, "m", "s");
        for g in [b"matg", b"scnr"] {
            set.add_group(FourCC(*g), None, None);
        }
        let empty = || TagPayload::Built(Block::zeroed(0x10));
        set.globals = set
            .add_tag(FourCC(*b"matg"), "globals\\globals", empty())
            .unwrap();
        set.scenario = set
            .add_tag(FourCC(*b"scnr"), "levels\\m\\m", empty())
            .unwrap();
        let gone = TagKey::new(FourCC(*b"bitm"), "a\\gone");
        let node = |size: u32, count: u32, fields| Node {
            bytes: vec![0; (size * count) as usize],
            count,
            size,
            align: 4,
            fields,
        };
        let matg = LooseTag {
            key: TagKey::new(FourCC(*b"matg"), "globals\\globals"),
            body: Body::Tree(Tree {
                region: Region::Metadata,
                nodes: vec![
                    node(0x10, 1, vec![(0, Value::Block(1))]),
                    node(0xc, 2, vec![(0xc + 4, Value::Ref(gone))]),
                ],
                raws: Vec::new(),
                losses: Vec::new(),
            }),
        };
        let keep = Keep {
            palettes: &[],
            groups: &[],
            prefixes: &[],
        };
        let e = build(&mut Mem(vec![matg]), &set, &keep, &[], &HashSet::new(), None)
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(e.contains("m: 1 references to tags no source holds"), "{e}");
        assert!(
            e.contains("globals\\globals.matg +0x0[1]+0x4 -> a\\gone.bitm"),
            "{e}"
        );
    }

    /// A datum the scenario or a BSP names that the new set lacks is an
    /// error naming the field, not a null datum.
    #[test]
    fn a_datum_the_new_set_lacks_is_an_error_naming_the_field() {
        let mut set = TagSet::new(CacheType::Multiplayer, "m", "s");
        let shad = tag(&mut set, b"shad", "cliff");
        let none = |_: TagId| -> Option<TagId> { None };
        let mut cx = Transfer {
            set: &set,
            map_id: &none,
            raws: HashMap::new(),
            sections: HashMap::new(),
            words: 0,
            untyped: false,
            error: None,
            tag: "levels\\m\\m.sbsp".into(),
            path: vec![0x58],
        };
        assert_eq!(cx.id(shad, 0x24), TagId::NULL);
        let e = cx.error.expect("an error").to_string();
        assert!(
            e.contains("m: levels\\m\\m.sbsp +0x58+0x24 names cliff.shad, which the map does not hold"),
            "{e}"
        );
    }

    #[test]
    fn only_words_laid_out_as_references_name_tags_outside_the_scenario() {
        let mut set = TagSet::new(CacheType::Multiplayer, "m", "s");
        let shad = tag(&mut set, b"shad", "cliff");
        let mut bytes = Vec::new();
        // A word that happens to equal the datum (a pair of vertex indices).
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&shad.0.to_le_bytes());
        // The same datum laid out as a reference: its group, then the datum.
        bytes.extend_from_slice(&u32::from_le_bytes(*b"shad").to_le_bytes());
        bytes.extend_from_slice(&shad.0.to_le_bytes());
        let b = Block::new(bytes);
        assert_eq!(datum_words(&b, None, &set, false), vec![12]);
        // In the scenario's tree every word may be a datum.
        assert_eq!(datum_words(&b, None, &set, true), vec![0, 4, 8, 12]);
    }
}
