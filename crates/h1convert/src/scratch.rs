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
    closure, to_tag_set,
};

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
    /// References to tags no source holds, nulled: (from, to).
    pub dangling: Vec<(String, String)>,
}

fn invalid(s: impl Into<String>) -> Error {
    Error::Fresh(s.into())
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

/// Every datum `b` names: reference fixups and every 4-aligned word equal
/// to a datum of `set` (the scripts' atoms, the simulation table).
fn datums(b: &Block, set: &TagSet, out: &mut Vec<TagId>) {
    for w in b.bytes.as_chunks::<4>().0 {
        let d = TagId(u32::from_le_bytes(*w));
        if d.0 != u32::MAX && key_of(set, d).is_some() {
            out.push(d);
        }
    }
    for f in &b.fixups {
        match &f.kind {
            FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) => {
                datums(child, set, out)
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
pub fn build(
    src: &mut dyn TagSource,
    set: &TagSet,
    keep: &Keep,
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
    let mut named = Vec::new();
    for (id, _) in &here {
        if let Some(TagPayload::Built(b)) = set.tag(*id).map(|t| &t.payload) {
            datums(b, set, &mut named);
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
    for (k, from) in &c.missing {
        report.dangling.push((from.to_string(), k.to_string()));
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
    for (from, to) in built.dangling {
        report.dangling.push((from.to_string(), to.to_string()));
    }
    // The builder shares equal blocks between tags. Each tag here keeps its
    // blocks inside its own bytes (sharing only within itself), as the
    // readers that take a tag's bytes alone expect: `tags::DecodedTag`,
    // `h1sounds::gestalt::SoundGestalt::decode`, the editor, a later
    // conversion.
    for slot in &mut out.slots {
        if let Some(t) = slot.tag.as_mut()
            && let TagPayload::Built(b) = &mut t.payload
        {
            // The gestalt reader takes no overlap at all.
            let ugh = t.group == FourCC(*b"ugh!");
            own_shares(b, t.id.0, ugh);
        }
    }
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
        error: None,
    };
    let mut payloads = Vec::new();
    for (id, key) in &here {
        let Some(TagPayload::Built(b)) = set.tag(*id).map(|t| &t.payload) else {
            continue;
        };
        let mut b = b.clone();
        cx.block(&mut b, &mut out);
        payloads.push((new_id[key], b));
    }
    if let Some(e) = cx.error {
        return Err(e);
    }
    report.words = cx.words;
    for (id, b) in payloads {
        out.tag_mut(id).expect("added").payload = TagPayload::Built(b);
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
    error: Option<Error>,
}

impl Transfer<'_> {
    fn id(&mut self, d: TagId) -> TagId {
        if d == TagId::NULL {
            return d;
        }
        (self.map_id)(d).unwrap_or(TagId::NULL)
    }

    /// `b` (a tree of `self.set`) rewritten for `out`.
    fn block(&mut self, b: &mut Block, out: &mut TagSet) {
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
        for k in (0..b.bytes.len() / 4 * 4).step_by(4) {
            if covered.contains(&(k as u32)) {
                continue;
            }
            let d = TagId(u32::from_le_bytes(b.bytes[k..k + 4].try_into().expect("4")));
            if d.0 != u32::MAX && key_of(self.set, d).is_some() {
                let n = self.id(d);
                b.bytes[k..k + 4].copy_from_slice(&n.0.to_le_bytes());
                self.words += 1;
            }
        }
        for f in &mut b.fixups {
            match &mut f.kind {
                FixupKind::Block { child, .. }
                | FixupKind::Data(child)
                | FixupKind::Pointer(child) => self.block(child, out),
                FixupKind::TagRef(t) | FixupKind::TagId(t) => *t = self.id(*t),
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
                            let bsp = self.id(s.bsp);
                            let lightmap = s.lightmap.map(|l| self.id(l));
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
