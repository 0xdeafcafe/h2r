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

    let starts = raw_starts(
        schema,
        &rel,
        src.tags(),
        metadata,
        src.header().index_offset,
    );

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
            let len = raw_len(self.starts, offset, size, siblings);
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

/// Every local raw pointer of the source map's movable tags, its BSP
/// regions and its tag index offset, sorted: the next one after a resource
/// bounds its length.
fn raw_starts(
    schema: &Schema,
    rel: &Relocator,
    tags: &[map::Tag],
    metadata: Option<map::AddressSpace>,
    index_offset: u32,
) -> Vec<u32> {
    let mut starts: Vec<u32> = Vec::new();
    for t in tags {
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
    starts.push(index_offset);
    starts.sort_unstable();
    starts.dedup();
    starts
}

/// Bytes to copy for the local resource at `offset`: its paired size, and
/// at least up to the next raw start that is not one of the element's own
/// pointers (`siblings`).
fn raw_len(starts: &[u32], offset: u32, size: Option<u32>, siblings: &[u32]) -> u32 {
    let i = starts.partition_point(|&x| x <= offset);
    let next = starts[i..]
        .iter()
        .copied()
        .find(|x| !siblings.contains(x))
        .unwrap_or(offset);
    size.unwrap_or(0).max(next - offset)
}

/// Turns a map's pinned tags into built ones **in place**: the destination
/// set has the same slots, datums and string table as the source map (it
/// was decomposed from it, or copies its index), so tag references, datums
/// and string IDs keep their values. Only local raw resources change: each
/// is copied out of the source file into a new resource of the set, since a
/// set with nothing pinned has no file prefix to point into. Pointers into
/// the shared caches stay.
pub struct Unpinner<'s> {
    rel: Relocator<'s>,
    starts: Vec<u32>,
    raws: HashMap<u32, RawId>,
    /// Local raw resources copied, and their bytes.
    pub raw: (usize, u64),
    /// External raw pointers kept.
    pub external: usize,
}

impl<'s> Unpinner<'s> {
    /// Read `src` once.
    pub fn new<R: Read + Seek>(schema: &'s Schema, src: &mut Map<R>) -> crate::Result<Self> {
        let rel = Relocator::new(schema, src)?;
        let starts = raw_starts(
            schema,
            &rel,
            src.tags(),
            src.metadata_space(),
            src.header().index_offset,
        );
        Ok(Unpinner {
            rel,
            starts,
            raws: HashMap::new(),
            raw: (0, 0),
            external: 0,
        })
    }

    /// The source map's relocator.
    pub fn relocator(&self) -> &Relocator<'s> {
        &self.rel
    }

    /// More local raw starts the schema does not reach (a sound gestalt's
    /// chunks), so [`Unpinner::extent`] bounds resources by them too.
    pub fn add_starts(&mut self, more: impl IntoIterator<Item = u32>) {
        self.starts.extend(
            more.into_iter()
                .filter(|&v| v != u32::MAX && v != 0 && v >> 30 == 0),
        );
        self.starts.sort_unstable();
        self.starts.dedup();
    }

    /// Bytes to copy for the local resource at `v` whose length is not
    /// stored: up to the next known raw start.
    pub fn extent(&self, v: u32) -> u32 {
        raw_len(&self.starts, v & 0x3fff_ffff, None, &[])
    }

    /// Copy the local raw resource at source pointer `v` (`len` bytes)
    /// into `dst`, once per pointer.
    pub fn copy_raw<R: Read + Seek>(
        &mut self,
        src: &mut Map<R>,
        dst: &mut TagSet,
        v: u32,
        len: u32,
    ) -> crate::Result<RawId> {
        if let Some(&id) = self.raws.get(&v) {
            return Ok(id);
        }
        let offset = v & 0x3fff_ffff;
        let bytes = src.read_file_range(offset.into(), len.into())?;
        self.raw.0 += 1;
        self.raw.1 += u64::from(len);
        let id = dst.add_raw(bytes, 1 << offset.trailing_zeros().min(12));
        self.raws.insert(v, id);
        Ok(id)
    }

    /// Rewrite the raw pointers of `count` elements of `s` in `block` (a
    /// tree built from the source map) for `dst`; everything else stays.
    pub fn localise<R: Read + Seek>(
        &mut self,
        s: &super::Struct,
        block: &mut map::build::Block,
        count: u32,
        src: &mut Map<R>,
        dst: &mut TagSet,
    ) -> crate::Result<()> {
        struct Same<'a, 'b, R> {
            u: &'a mut Unpinner<'b>,
            src: &'a mut Map<R>,
            dst: &'a mut TagSet,
            error: Option<Error>,
        }
        impl<R: Read + Seek> Remapper for Same<'_, '_, R> {
            fn datum(&mut self, d: u32) -> Option<u32> {
                Some(d)
            }
            fn string(&mut self, v: u32) -> u32 {
                v
            }
            fn raw(&mut self, v: u32, size: Option<u32>, siblings: &[u32]) -> RawRef {
                if v >> 30 != 0 {
                    self.u.external += 1;
                    let file = match v >> 30 {
                        1 => map::ResourceFile::MainMenu,
                        2 => map::ResourceFile::Shared,
                        _ => map::ResourceFile::SinglePlayerShared,
                    };
                    return RawRef::External {
                        file,
                        offset: v & 0x3fff_ffff,
                    };
                }
                let len = raw_len(&self.u.starts, v, size, siblings);
                match self.u.copy_raw(self.src, self.dst, v, len) {
                    Ok(id) => RawRef::Local(id),
                    Err(e) => {
                        self.error.get_or_insert(e);
                        RawRef::Null
                    }
                }
            }
        }
        let mut same = Same {
            u: self,
            src,
            dst,
            error: None,
        };
        remap(s, block, count, &mut same);
        same.error.map_or(Ok(()), Err)
    }

    /// Rehome the source blocks of a tag a conversion built from a source
    /// tag's bytes (a template copied, some fields replaced): every block
    /// field of `count` elements of `s` in `block` that still holds a raw
    /// address into the source metadata, not a fixup, becomes a depth-first
    /// tree of what it points at, local raw copied. Fixup children are
    /// walked the same way. Words that are no metadata address stay.
    /// Returns the fields rehomed and, as `(offset, address)`, the ones
    /// that point into the metadata but whose subtree the schema refuses
    /// (left as they are).
    pub fn rehome<R: Read + Seek>(
        &mut self,
        s: &super::Struct,
        block: &mut map::build::Block,
        count: u32,
        src: &mut Map<R>,
        dst: &mut TagSet,
    ) -> crate::Result<(usize, Vec<(u32, u32)>)> {
        use map::build::FixupKind;
        let mut done = 0;
        let mut refused = Vec::new();
        for i in 0..count {
            for (&o, f) in &s.fields {
                let Field::Block(child) = f else { continue };
                let off = i * s.size + o;
                if off as usize + 8 > block.bytes.len() {
                    continue;
                }
                if let Some(fx) = block.fixups.iter_mut().find(|x| x.offset == off) {
                    if let FixupKind::Block { count, child: sub } = &mut fx.kind {
                        let (n, r) = self.rehome(child, sub, *count, src, dst)?;
                        done += n;
                        refused.extend(r);
                    }
                    continue;
                }
                let c = super::u32_at(&block.bytes, off as usize);
                let a = super::u32_at(&block.bytes, off as usize + 4);
                if c == 0 || a == 0 || self.rel.memory.region(a).is_none() {
                    continue;
                }
                if super::check_subtree(child, a, c, (0, 0), &self.rel.memory, &mut vec![off])
                    .is_err()
                {
                    refused.push((off, a));
                    continue;
                }
                let cx = super::TreeCtx {
                    mem: &self.rel.memory,
                    salt: self.rel.salt,
                    own: (0, 0),
                    stays: &|_| false,
                };
                let mut sub = super::tree(child, &cx, a, c);
                self.localise(child, &mut sub, c, src, dst)?;
                if child.size == 1 && child.fields.is_empty() {
                    block.fixup(off, FixupKind::Data(sub));
                } else {
                    block.block(off, c, sub);
                }
                done += 1;
            }
        }
        Ok((done, refused))
    }

    /// Make every tag of `ids` built, in its own slot of `dst`: a
    /// depth-first tree ([`Relocator::to_built`]) with local raw copied, or
    /// an alias when it has no bytes of its own. Checks every tag first;
    /// if any is refused, `dst` is unchanged and the refusals come back.
    pub fn unpin<R: Read + Seek>(
        &mut self,
        src: &mut Map<R>,
        ids: &[TagId],
        dst: &mut TagSet,
    ) -> crate::Result<Result<usize, Vec<Refused>>> {
        let metadata = src.metadata_space();
        let mut refused = Vec::new();
        let mut plan = Vec::new();
        for &id in ids {
            let t = src.tag(id)?.clone();
            let name = t.name.clone().unwrap_or_default();
            let Some(l) = t.location.filter(|l| Some(l.space) == metadata) else {
                continue;
            };
            let detail = |refusal| Detail {
                refusal,
                path: Vec::new(),
                offset: 0,
                value: l.address,
            };
            if l.size == 0 {
                match self.rel.owner(l.address) {
                    Some(owner) => {
                        let start = self.rel.memory().region(l.address).expect("owned").0;
                        plan.push((id, None, Some((owner, l.address - start))));
                    }
                    None => refused.push(Refused {
                        id,
                        group: t.group,
                        name,
                        detail: detail(Refusal::NoData),
                    }),
                }
                continue;
            }
            match self.rel.check(id) {
                Ok(_) => plan.push((id, Some(t.group), None)),
                Err(d) => refused.push(Refused {
                    id,
                    group: t.group,
                    name,
                    detail: d,
                }),
            }
        }
        if !refused.is_empty() {
            return Ok(Err(refused));
        }
        let mut done = Vec::new();
        for &(id, group, alias) in &plan {
            if let Some((tag, offset)) = alias {
                done.push((id, TagPayload::Alias { tag, offset }));
                continue;
            }
            let group = group.expect("a built tag");
            let mut block = self
                .rel
                .to_built(id)
                .map_err(|_| Error::Invalid("tag is not relocatable"))?;
            let root = &self.rel.schema.groups[&group];
            self.localise(root, &mut block, 1, src, dst)?;
            done.push((id, TagPayload::Built(block)));
        }
        let n = done.len();
        for (id, payload) in done {
            dst.tag_mut(id)
                .ok_or(Error::Invalid("tag not in the set"))?
                .payload = payload;
        }
        Ok(Ok(n))
    }
}
