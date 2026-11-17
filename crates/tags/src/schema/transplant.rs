//! Copy a tag and everything it references from one map into another
//! map's [`TagSet`], offline: the core of building a fresh map from retail
//! parts.
//!
//! The tree is closed over tag references and datums. A referenced tag the
//! destination already has (same group and name) is reused, not copied, and
//! its own references are not followed. Every copied tag becomes a
//! depth-first [`Block`] tree ([`Relocator::to_built`], arrays it shares
//! with other tags copied in), with tag references, datums and string IDs
//! rewritten for the destination ([`remap`]). Raw resources in the source
//! map's own file are copied as new local resources; pointers into
//! `shared.map` and the other shared caches stay as they are (both maps
//! must use the same one).
//!
//! A raw resource's length is its paired [`Field::RawSize`] when the schema
//! has one, and at least the distance to the next raw pointer of the
//! source map that is not one of the element's own (or the end of the raw
//! data, or a BSP region); copying a little padding is harmless.
use super::{Detail, Field, Refusal, Relocator, Remapper, Schema, remap, visit};
use crate::Error;
use map::build::{RawId, RawRef, TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use std::collections::HashMap;
use std::io::{Read, Seek};

/// What a transplant did.
#[derive(Clone, Debug, Default)]
pub struct Transplanted {
    /// Source datum to destination datum for every tag of the tree.
    pub ids: HashMap<TagId, TagId>,
    /// Source tags copied (the rest of `ids` were reused).
    pub added: Vec<TagId>,
    /// Source tags referenced but left out (no data): references to them
    /// become null.
    pub dropped: Vec<TagId>,
    /// Local raw resources copied, and their total bytes.
    pub raw: (usize, u64),
    /// External raw pointers kept.
    pub external: usize,
}

/// A tag of the tree the schema cannot copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// Source datum.
    pub id: TagId,
    /// Its group.
    pub group: FourCC,
    /// Its name.
    pub name: String,
    /// Why.
    pub detail: Detail,
}

/// Copy `root` of `src` and its tree into `dst`. The whole tree is checked
/// first: if any tag of it is not relocatable with `schema`, `dst` is left
/// unchanged and the refusals come back as `Ok(Err(..))`.
pub fn transplant<R: Read + Seek>(
    schema: &Schema,
    src: &mut Map<R>,
    root: TagId,
    dst: &mut TagSet,
) -> crate::Result<Result<Transplanted, Vec<Refused>>> {
    let rel = Relocator::new(schema, src)?;
    let tags: HashMap<TagId, map::Tag> = src.tags().iter().map(|t| (t.id, t.clone())).collect();
    let groups: HashMap<FourCC, map::TagGroup> =
        src.groups().iter().map(|g| (g.class, *g)).collect();
    let metadata = src.metadata_space();
    let mut out = Transplanted::default();

    // Raw starts of the source file, for lengths.
    let mut starts: Vec<u32> = Vec::new();
    for t in tags.values() {
        let (Some(root), Some(l)) = (schema.groups.get(&t.group), t.location) else {
            continue;
        };
        if Some(l.space) != metadata || l.size == 0 || rel.check(t.id).is_err() {
            continue;
        }
        visit(root, l.address, rel.memory(), &mut |s, e, _| {
            for (&o, f) in &s.fields {
                if matches!(f, Field::Raw) {
                    let v = super::u32_at(e, o as usize);
                    if v != u32::MAX && v != 0 && v >> 30 == 0 {
                        starts.push(v);
                    }
                }
            }
        });
    }
    for &(offset, _, _) in &rel.memory().bsps {
        starts.push(offset);
    }
    starts.push(src.header().index_offset);
    starts.sort_unstable();
    starts.dedup();

    // Close over references; reuse what the destination has. Nothing is
    // added until the whole tree checks.
    enum Plan {
        Copy(TagId),
        Alias(TagId, TagId),
    }
    let mut plan = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut reused = Vec::new();
    let mut refused = Vec::new();
    let mut todo = vec![root];
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        let t = tags
            .get(&id)
            .ok_or(Error::Invalid("reference to a missing tag"))?;
        let name = t.name.clone().unwrap_or_default();
        if let Some(have) = dst.find(t.group, &name) {
            reused.push((id, have));
            continue;
        }
        let Some(l) = t.location.filter(|l| Some(l.space) == metadata) else {
            out.dropped.push(id);
            continue;
        };
        let refuse = |detail| Refused {
            id,
            group: t.group,
            name: name.clone(),
            detail,
        };
        let plain = |refusal| Detail {
            refusal,
            path: Vec::new(),
            offset: 0,
            value: l.address,
        };
        if l.size == 0 {
            // An alias of the start of another tag (the only kind the
            // depth-first layout can keep).
            match rel.owner(l.address) {
                Some(owner) if rel.memory().region(l.address).map(|r| r.0) == Some(l.address) => {
                    plan.push(Plan::Alias(id, owner));
                    todo.push(owner);
                }
                _ => refused.push(refuse(plain(Refusal::Overlap))),
            }
            continue;
        }
        let Some(groot) = schema.groups.get(&t.group) else {
            refused.push(refuse(plain(Refusal::NoSchema)));
            continue;
        };
        if let Err(d) = rel.check(id) {
            refused.push(refuse(d));
            continue;
        }
        visit(groot, l.address, rel.memory(), &mut |s, e, _| {
            for (&o, f) in &s.fields {
                let d = match f {
                    Field::TagRef => super::u32_at(e, o as usize + 4),
                    Field::TagId => super::u32_at(e, o as usize),
                    _ => continue,
                };
                if d != u32::MAX && tags.contains_key(&TagId(d)) {
                    todo.push(TagId(d));
                }
            }
        });
        plan.push(Plan::Copy(id));
    }
    if !refused.is_empty() {
        return Ok(Err(refused));
    }
    out.ids.extend(reused);
    let mut order = Vec::new();
    let mut aliases = Vec::new();
    for p in &plan {
        let id = match *p {
            Plan::Copy(id) | Plan::Alias(id, _) => id,
        };
        let t = &tags[&id];
        if let Some(g) = groups.get(&t.group) {
            for c in [g.grandparent, g.parent].into_iter().flatten() {
                if let Some(a) = groups.get(&c) {
                    dst.add_group(a.class, a.parent, a.grandparent);
                }
            }
            dst.add_group(g.class, g.parent, g.grandparent);
        }
        let new = dst.add_tag(
            t.group,
            t.name.as_deref().unwrap_or_default(),
            TagPayload::Empty,
        )?;
        out.ids.insert(id, new);
        match *p {
            Plan::Copy(id) => order.push((id, new, &schema.groups[&t.group])),
            Plan::Alias(_, owner) => aliases.push((new, owner)),
        }
    }

    // Build and rewrite.
    struct Ids<'a> {
        ids: &'a HashMap<TagId, TagId>,
        strings: &'a [Option<String>],
        dst: &'a mut TagSet,
        src: &'a mut dyn FnMut(u32, u32) -> crate::Result<Vec<u8>>,
        starts: &'a [u32],
        raws: HashMap<u32, RawId>,
        copied: (usize, u64),
        external: usize,
        error: Option<Error>,
    }
    impl Remapper for Ids<'_> {
        fn datum(&mut self, d: u32) -> Option<u32> {
            self.ids.get(&TagId(d)).map(|t| t.0)
        }
        fn string(&mut self, v: u32) -> u32 {
            let s = self
                .strings
                .get((v & 0x00ff_ffff) as usize)
                .cloned()
                .flatten();
            match s.map(|s| self.dst.intern(&s)) {
                Some(Ok(n)) => n,
                _ => {
                    self.error
                        .get_or_insert(Error::Invalid("unknown string ID"));
                    0
                }
            }
        }
        fn raw(&mut self, v: u32, size: Option<u32>, siblings: &[u32]) -> RawRef {
            let (sel, offset) = (v >> 30, v & 0x3fff_ffff);
            if sel != 0 {
                self.external += 1;
                let file = match sel {
                    1 => map::ResourceFile::MainMenu,
                    2 => map::ResourceFile::Shared,
                    _ => map::ResourceFile::SinglePlayerShared,
                };
                return RawRef::External { file, offset };
            }
            if let Some(&id) = self.raws.get(&v) {
                return RawRef::Local(id);
            }
            let i = self.starts.partition_point(|&x| x <= offset);
            let next = self.starts[i..]
                .iter()
                .copied()
                .find(|x| !siblings.contains(x))
                .unwrap_or(offset);
            let len = size.unwrap_or(0).max(next - offset);
            match (self.src)(offset, len) {
                Ok(bytes) => {
                    self.copied.0 += 1;
                    self.copied.1 += u64::from(len);
                    let align = 1 << offset.trailing_zeros().min(12);
                    let id = self.dst.add_raw(bytes, align);
                    self.raws.insert(v, id);
                    RawRef::Local(id)
                }
                Err(e) => {
                    self.error.get_or_insert(e);
                    RawRef::Null
                }
            }
        }
    }
    let strings = src.strings().to_vec();
    let mut read = |offset: u32, len: u32| -> crate::Result<Vec<u8>> {
        Ok(src.read_file_range(offset.into(), len.into())?)
    };
    let mut ids = Ids {
        ids: &out.ids,
        strings: &strings,
        dst,
        src: &mut read,
        starts: &starts,
        raws: HashMap::new(),
        copied: (0, 0),
        external: 0,
        error: None,
    };
    let mut built = Vec::new();
    for &(id, new, groot) in &order {
        let mut block = rel
            .to_built(id)
            .map_err(|_| Error::Invalid("tag in the tree is not relocatable"))?;
        remap(groot, &mut block, 1, &mut ids);
        built.push((id, new, block));
    }
    if let Some(e) = ids.error {
        return Err(e);
    }
    let (copied, external) = (ids.copied, ids.external);
    for (id, new, block) in built {
        dst.tag_mut(new).expect("added").payload = TagPayload::Built(block);
        out.added.push(id);
    }
    for (new, owner) in aliases {
        let tag = *out.ids.get(&owner).expect("closed");
        dst.tag_mut(new).expect("added").payload = TagPayload::Alias { tag, offset: 0 };
    }
    out.raw = copied;
    out.external = external;
    Ok(Ok(out))
}
