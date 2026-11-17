//! Loose tags into a [`TagSet`] laid out from scratch.
//!
//! Every tag becomes a [`TagPayload::Built`] tree (or an alias). Datums are
//! the set's own; string IDs are interned after the starting table (the
//! string IDs every retail map begins with, which the engine may name by
//! index); local raw resources are copied into the map; pointers into the
//! shared caches stay pointers. Each scenario BSP descriptor names an
//! `sbsp` and `ltmp`: that pair becomes a built BSP region, and the
//! descriptor's words its [`FixupKind::Bsp`] fixups.
use super::{Body, Error, LooseTag, Origin, Region, Result, TagKey, TagSource, Tree, Value};
use map::build::{Block, BspField, FixupKind, RawId, RawRef, TagPayload, TagSet};
use map::{CacheType, FourCC, TagId};
use std::collections::HashMap;

/// Alignment for copied raw resources (the builder's minimum).
pub const RAW_ALIGN: u32 = 0x200;

/// What a map built from loose tags needs besides its tags.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Header `+0x140`.
    pub cache_type: CacheType,
    /// Internal map name.
    pub name: String,
    /// The scenario (written to the header and the index).
    pub scenario: TagKey,
    /// The globals (slot 0).
    pub globals: TagKey,
    /// String-ID table to start from; others are appended.
    pub strings: Vec<Option<String>>,
    /// A 0x800-byte header for the fields the builder does not own.
    pub header_template: Option<Vec<u8>>,
    /// The low-detail bitmap region.
    pub low_detail: Option<Vec<u8>>,
    /// Index header word `+0x14`.
    pub index_word_14: u32,
}

/// What [`to_tag_set`] did.
#[derive(Clone, Debug, Default)]
pub struct BuildReport {
    /// Tags placed.
    pub tags: usize,
    /// Of them, aliases.
    pub aliases: usize,
    /// BSP regions built.
    pub bsps: usize,
    /// Local raw resources copied, and their bytes.
    pub raw: (usize, u64),
    /// Pointers into the shared caches kept.
    pub external: usize,
    /// References to tags not in the set, nulled: (from, to).
    pub dangling: Vec<(TagKey, TagKey)>,
    /// BSP-region tags no descriptor names, left without data.
    pub orphan_bsps: Vec<TagKey>,
}

struct Ctx<'a> {
    ids: &'a HashMap<TagKey, TagId>,
    set: &'a mut TagSet,
    src: &'a mut dyn TagSource,
    raw_ids: HashMap<[u8; 32], RawId>,
    sections: HashMap<TagId, usize>,
    report: BuildReport,
    /// Content hash per (tag, node) for sharing keys.
    hashes: HashMap<(usize, u32), u64>,
}

/// Turn `tags` (a closure: every reference should be among them) into a set.
/// Slot 0 is the globals, then the tags in the order given.
pub fn to_tag_set(
    src: &mut dyn TagSource,
    tags: &[LooseTag],
    opts: &BuildOptions,
) -> Result<(TagSet, BuildReport)> {
    let mut set = TagSet::new(opts.cache_type, &opts.name, &opts.scenario.path);
    set.strings = opts.strings.clone();
    set.header_template = opts.header_template.clone();
    set.index_word_14 = opts.index_word_14;
    let used: std::collections::HashSet<FourCC> = tags.iter().map(|t| t.key.group).collect();
    let all = src.groups();
    for g in &all {
        if used.contains(&g.class) {
            for c in [g.grandparent, g.parent].into_iter().flatten() {
                if let Some(a) = all.iter().find(|a| a.class == c) {
                    set.add_group(a.class, a.parent, a.grandparent);
                }
            }
            set.add_group(g.class, g.parent, g.grandparent);
        }
    }
    for t in tags {
        if !set.groups.iter().any(|g| g.class == t.key.group) {
            set.add_group(t.key.group, None, None);
        }
    }
    let mut order: Vec<usize> = (0..tags.len()).collect();
    order.sort_by_key(|&i| tags[i].key != opts.globals);
    let mut ids = HashMap::new();
    for &i in &order {
        let id = set.add_tag(tags[i].key.group, &tags[i].key.path, TagPayload::Empty)?;
        ids.insert(tags[i].key.clone(), id);
    }
    let find = |k: &TagKey| {
        ids.get(k)
            .copied()
            .ok_or_else(|| Error::Invalid(format!("{k} is not in the set")))
    };
    set.scenario = find(&opts.scenario)?;
    set.globals = find(&opts.globals)?;
    if let Some(ld) = &opts.low_detail {
        set.low_detail = Some(set.add_raw(ld.clone(), RAW_ALIGN));
    }
    let mut cx = Ctx {
        ids: &ids,
        set: &mut set,
        src,
        raw_ids: HashMap::new(),
        sections: HashMap::new(),
        report: BuildReport::default(),
        hashes: HashMap::new(),
    };
    let mut payloads = Vec::new();
    for (ti, t) in tags.iter().enumerate() {
        let id = ids[&t.key];
        let payload = match &t.body {
            Body::Alias { owner, offset } => {
                cx.report.aliases += 1;
                let tag = *ids.get(owner).ok_or_else(|| {
                    Error::Invalid(format!("{}: alias of {owner}, not in the set", t.key))
                })?;
                TagPayload::Alias {
                    tag,
                    offset: *offset,
                }
            }
            Body::Tree(tree) => {
                let mut root = cx.block(ti, &t.key, tree, 0, 0)?;
                root.share = None;
                TagPayload::Built(root)
            }
        };
        payloads.push((id, payload));
        cx.report.tags += 1;
    }
    let sections = cx.sections.clone();
    let mut report = cx.report;
    for (id, payload) in payloads {
        set.tag_mut(id).expect("added").payload = payload;
    }
    // BSP-region tags must belong to a section.
    let in_section: std::collections::HashSet<TagId> = set
        .bsps
        .iter()
        .flat_map(|s| [Some(s.bsp), s.lightmap])
        .flatten()
        .collect();
    for t in tags {
        if t.tree().is_some_and(|tr| tr.region == Region::Bsp) && !in_section.contains(&ids[&t.key])
        {
            set.tag_mut(ids[&t.key]).expect("added").payload = TagPayload::Empty;
            report.orphan_bsps.push(t.key.clone());
        }
    }
    // An alias needs an owner with metadata of its own.
    for t in tags {
        if let Body::Alias { owner, .. } = &t.body {
            let ok = matches!(
                set.tag(ids[owner]).map(|e| &e.payload),
                Some(TagPayload::Built(_))
            );
            if !ok {
                return Err(Error::Invalid(format!(
                    "{} is an alias of {owner}, which has no metadata of its own",
                    t.key
                )));
            }
        }
    }
    report.bsps = sections.len();
    Ok((set, report))
}

impl Ctx<'_> {
    fn raw_ref(&mut self, tree: &Tree, i: u32) -> Result<RawRef> {
        let r = &tree.raws[i as usize];
        match r.origin {
            Origin::External { file, offset } => {
                self.report.external += 1;
                Ok(RawRef::External { file, offset })
            }
            Origin::Local => {
                if let Some(&id) = self.raw_ids.get(&r.hash) {
                    return Ok(RawRef::Local(id));
                }
                let bytes = self.src.raw(r)?;
                self.report.raw.0 += 1;
                self.report.raw.1 += bytes.len() as u64;
                let id = self.set.add_raw(bytes, RAW_ALIGN);
                self.raw_ids.insert(r.hash, id);
                Ok(RawRef::Local(id))
            }
        }
    }

    /// Content hash of node `n` of tag `ti` and everything below it.
    fn hash(&mut self, ti: usize, tree: &Tree, n: u32, depth: u32) -> u64 {
        use std::hash::{Hash, Hasher};
        if let Some(&h) = self.hashes.get(&(ti, n)) {
            return h;
        }
        let node = &tree.nodes[n as usize];
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (node.align, node.size, node.count, &node.bytes).hash(&mut h);
        for (o, v) in &node.fields {
            o.hash(&mut h);
            match v {
                Value::Block(c) | Value::Data(c) => {
                    (0u8, matches!(v, Value::Data(_))).hash(&mut h);
                    if depth < 64 {
                        self.hash(ti, tree, *c, depth + 1).hash(&mut h);
                    }
                }
                Value::Ref(k) => (1u8, k).hash(&mut h),
                Value::String(s) => (2u8, s).hash(&mut h),
                Value::Raw(r) => (3u8, &tree.raws[*r as usize]).hash(&mut h),
                // Descriptors are per scenario: never share them.
                Value::Bsp => (4u8, ti, n).hash(&mut h),
            }
        }
        let h = h.finish();
        self.hashes.insert((ti, n), h);
        h
    }

    fn block(&mut self, ti: usize, key: &TagKey, tree: &Tree, n: u32, depth: u32) -> Result<Block> {
        if depth > 64 {
            return Err(Error::Invalid(format!(
                "{key}: blocks nested too deep (a cycle?)"
            )));
        }
        let node = &tree.nodes[n as usize];
        let share = self.hash(ti, tree, n, 0);
        let mut b = Block::new(node.bytes.clone())
            .aligned(node.align.max(1))
            .shared(share);
        let size = node.size.max(1);
        for (o, v) in &node.fields {
            let o = *o;
            match v {
                Value::Block(c) => {
                    let count = u32::from_le_bytes(
                        node.bytes[o as usize..o as usize + 4]
                            .try_into()
                            .expect("4"),
                    );
                    let child = self.block(ti, key, tree, *c, depth + 1)?;
                    b.block(o, count, child);
                }
                Value::Data(c) => {
                    let child = self.block(ti, key, tree, *c, depth + 1)?;
                    b.data(o, child);
                }
                Value::Ref(k) => match self.ids.get(k) {
                    Some(&id) => {
                        b.tag_id(o, id);
                    }
                    None => {
                        b.bytes[o as usize..o as usize + 4].fill(0xff);
                        self.report.dangling.push((key.clone(), k.clone()));
                    }
                },
                Value::String(s) => {
                    b.string_id(o, s);
                }
                Value::Raw(r) => {
                    let r = self.raw_ref(tree, *r)?;
                    b.raw(o, r);
                }
                Value::Bsp => {
                    // The element's references name the BSP and lightmap.
                    let e = o - o % size;
                    let mut bsp = None;
                    let mut lightmap = None;
                    for (k, v) in &node.fields {
                        if (e..e + size).contains(k)
                            && let Value::Ref(r) = v
                        {
                            match &r.group.0 {
                                b"sbsp" => bsp = self.ids.get(r).copied(),
                                b"ltmp" => lightmap = self.ids.get(r).copied(),
                                _ => {}
                            }
                        }
                    }
                    let Some(bsp) = bsp else {
                        return Err(Error::Invalid(format!(
                            "{key}: a BSP descriptor names no structure BSP in the set"
                        )));
                    };
                    let section = match self.sections.get(&bsp) {
                        Some(&s) => s,
                        None => {
                            let s = self.set.add_bsp(bsp, lightmap);
                            self.sections.insert(bsp, s);
                            s
                        }
                    };
                    for (k, field) in [
                        (0, BspField::FileOffset),
                        (4, BspField::Size),
                        (8, BspField::Address),
                    ] {
                        b.fixup(o + k, FixupKind::Bsp { section, field });
                    }
                }
            }
        }
        Ok(b)
    }
}

/// A map's scenario, closed over and laid out from a repository: options
/// from the manifest of `map` (or of the map whose scenario this is), its
/// variants, the repository's starting string IDs. Roots are the scenario
/// and the globals.
pub fn from_repo(
    repo_root: &std::path::Path,
    scenario: &str,
    map: Option<&str>,
) -> Result<(TagSet, BuildReport, super::Closure)> {
    let base = super::TagRepo::open(repo_root)?;
    let map = match map {
        Some(m) => m.to_owned(),
        None => base.map_of_scenario(scenario)?.ok_or_else(|| {
            Error::Invalid(format!(
                "no manifest names {scenario}; pass the map to take options from"
            ))
        })?,
    };
    let m = base.manifest(&map)?;
    let mut repo = super::TagRepo::for_map(repo_root, &map)?;
    let scenario = TagKey::new(FourCC(*b"scnr"), scenario);
    let globals = m
        .globals
        .clone()
        .unwrap_or_else(|| TagKey::new(FourCC(*b"matg"), "globals\\globals"));
    let low_detail = match &m.low_detail {
        Some(h) => Some(repo.raw_by_hash(h)?),
        None => None,
    };
    let opts = BuildOptions {
        cache_type: m.cache_type(),
        name: m.name.clone(),
        scenario: scenario.clone(),
        globals: globals.clone(),
        strings: repo.strings()?,
        header_template: Some(repo.header(&map)?),
        low_detail,
        index_word_14: m.index_word_14,
    };
    let closure = super::closure(&mut repo, &[globals, scenario])?;
    if !closure.missing.is_empty() {
        let list: Vec<String> = closure
            .missing
            .iter()
            .take(8)
            .map(|(k, from)| format!("{k} (named by {from})"))
            .collect();
        return Err(Error::Invalid(format!(
            "{} tags missing from the repository: {}",
            closure.missing.len(),
            list.join(", ")
        )));
    }
    let (set, report) = to_tag_set(&mut repo, &closure.tags, &opts)?;
    Ok((set, report, closure))
}
