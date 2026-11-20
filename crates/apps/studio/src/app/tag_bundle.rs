//! Tags out of a map into a tag bundle, and back in
//! ([`editor_core::tag_bundle`], docs/editor/editor.md "Tag bundles").
//!
//! **Export** reads a tag of the document shown (edits included) through
//! the registry's layouts: each element's bytes, and a fixup for every
//! reference, string ID, block, data reference and raw pointer. With
//! `dependencies`, the tags it references go in too, and theirs, up to
//! [`DEPENDENCIES`]. With `resources`, the raw resources whose size the
//! element gives (a `RawSize` field naming the pointer) are copied as
//! blobs, up to [`BLOB_BYTES`] in all.
//!
//! **Import** replaces tags of the open map: each bundle tag replaces the
//! map's tag of the same group and name (or, for a bundle of one tag, the
//! tag named). The map writer patches tags in place and appends grown
//! blocks; it does not add tags, so a bundle tag the map lacks is refused
//! (or, asked, skipped). Blocks take the bundle's element counts (within
//! their room, or grown while growth is on); references resolve by
//! `group:name` in the map, and one the map lacks refuses the import;
//! string IDs by text, a string the map lacks added as the field editor
//! adds one. Data keeps its length, so data of another length refuses.
//! A raw pointer into a shared cache is carried when the bundle came from
//! a map of the same build; one into the bundle's own map only into that
//! map; otherwise the map's own pointer and size stay, and the report
//! counts them. Sounds (`snd!`) index the map's `ugh!` gestalt by number,
//! so they import only into the map they came from. The import is one
//! undo step; one that fails part way leaves the document as it was.
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use editor_core::tag_bundle::{self as tb, Blob, Bundle, BundleTag, Element, Fixup, Origin};
use map::{FourCC, RawPointer, ResourceFile, TagId};
use tags::registry::{Kind, Layout};

use super::tag_doc::{Access, BlockOp, TagDoc, TagWrite};
use super::{App, tag_api};

/// Tags an export with dependencies takes at most.
pub const DEPENDENCIES: usize = 256;
/// Raw resource bytes an export copies at most.
pub const BLOB_BYTES: u64 = 256 << 20;
/// Elements an export or import walks at most (a guard on bad counts).
pub const ELEMENTS: usize = 500_000;
/// Blocks nested deeper than this are refused.
const DEPTH: usize = 24;

/// Groups whose tags hold indices into another tag of their map (not
/// references), so they import only into the map they came from.
const MAP_BOUND: [&[u8; 4]; 2] = [b"snd!", b"ugh!"];

/// Reads raw resources for an export's blobs.
pub trait RawReader {
    fn read_raw(&mut self, file: ResourceFile, offset: u32, size: u32) -> Result<Vec<u8>, String>;
}

impl RawReader for assets::ResourceFiles {
    fn read_raw(&mut self, file: ResourceFile, offset: u32, size: u32) -> Result<Vec<u8>, String> {
        self.read(file, offset, size).map_err(|e| e.to_string())
    }
}

/// What an export takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// The tags they reference too, and theirs.
    pub dependencies: bool,
    /// Raw resources as blobs.
    pub resources: bool,
}

/// What an export did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportReport {
    /// `group:name` of each tag in the bundle, in order.
    pub tags: Vec<String>,
    pub blobs: usize,
    pub blob_bytes: u64,
    /// What was left out, and why (a dependency with nothing to export, a
    /// resource that did not read, the limits).
    pub left_out: Vec<String>,
}

fn label(doc: &TagDoc, id: TagId) -> String {
    match doc.tag(id) {
        Some(t) => format!("{}:{}", tags::defs::fourcc_text(t.group), t.name),
        None => format!("{:#010x}", id.0),
    }
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

struct Capture<'a, 'r> {
    doc: &'a TagDoc,
    raw: Option<&'r mut dyn RawReader>,
    blobs: Vec<Blob>,
    blob_at: HashMap<(ResourceFile, u32, u32), usize>,
    blob_bytes: u64,
    /// Tags referenced, in the order met.
    refs: Vec<TagId>,
    left_out: Vec<String>,
    elements: usize,
}

impl Capture<'_, '_> {
    fn element(&mut self, at: u32, l: &Layout, depth: usize) -> Result<Element, String> {
        if depth > DEPTH {
            return Err(format!("{}: blocks nest deeper than {DEPTH}", l.name));
        }
        self.elements += 1;
        if self.elements > ELEMENTS {
            return Err(format!("more than {ELEMENTS} elements"));
        }
        let bytes = self
            .doc
            .read(at, l.size)
            .ok_or_else(|| format!("{} at {at:#x} is outside the metadata", l.name))?;
        let mut fixups = Vec::new();
        for f in &l.fields {
            let o = f.offset as usize;
            match &f.kind {
                Kind::TagRef { .. } | Kind::Datum { .. } => {
                    let id = if matches!(f.kind, Kind::TagRef { .. }) {
                        u32_at(&bytes, o + 4)
                    } else {
                        u32_at(&bytes, o)
                    };
                    let target = if id == u32::MAX {
                        None
                    } else {
                        match self.doc.tag(TagId(id)) {
                            Some(_) => {
                                self.refs.push(TagId(id));
                                Some(label(self.doc, TagId(id)))
                            }
                            None => None,
                        }
                    };
                    fixups.push(Fixup::Tag {
                        at: f.offset,
                        target,
                    });
                }
                Kind::StringId => {
                    let v = u32_at(&bytes, o);
                    fixups.push(Fixup::StringId {
                        at: f.offset,
                        text: self.doc.string(v).map(str::to_owned),
                    });
                }
                Kind::Block(child) => {
                    let (n, a) = self
                        .doc
                        .block(at + f.offset, child)
                        .ok_or_else(|| format!("{}: the block's elements are outside the metadata", f.name))?;
                    let mut elements = Vec::with_capacity(n as usize);
                    for i in 0..n {
                        elements.push(self.element(a + i * child.size, child, depth + 1)?);
                    }
                    fixups.push(Fixup::Block {
                        at: f.offset,
                        elements,
                    });
                }
                Kind::Data => {
                    let (n, a) = (u32_at(&bytes, o), u32_at(&bytes, o + 4));
                    let data = if n == 0 {
                        Vec::new()
                    } else {
                        self.doc
                            .read(a, n)
                            .ok_or_else(|| format!("{}: the data is outside the metadata", f.name))?
                    };
                    fixups.push(Fixup::Data {
                        at: f.offset,
                        bytes: data,
                    });
                }
                Kind::Raw => {
                    let v = u32_at(&bytes, o);
                    // Zero and all-ones are no resource, the same anywhere.
                    let Some(p) = RawPointer::decode(v).filter(|_| v != 0) else {
                        continue;
                    };
                    let size = l
                        .fields
                        .iter()
                        .find(|g| matches!(g.kind, Kind::RawSize { of } if of == f.offset))
                        .map(|g| u32_at(&bytes, g.offset as usize));
                    let blob = match size {
                        Some(n) if n > 0 => self.blob(p, n, &f.name),
                        _ => None,
                    };
                    fixups.push(Fixup::Raw {
                        at: f.offset,
                        file: tb::file_name(p.file).to_owned(),
                        offset: p.offset,
                        size,
                        blob,
                    });
                }
                _ => {}
            }
        }
        Ok(Element { bytes, fixups })
    }

    /// The blob of `size` bytes at `p`, read once however many fields
    /// point there; `None` when not asked for or it did not read.
    fn blob(&mut self, p: RawPointer, size: u32, field: &str) -> Option<usize> {
        let raw = self.raw.as_mut()?;
        if let Some(&i) = self.blob_at.get(&(p.file, p.offset, size)) {
            return Some(i);
        }
        if self.blob_bytes + u64::from(size) > BLOB_BYTES {
            self.left_out.push(format!(
                "{field}: {size} bytes in {} past the {BLOB_BYTES}-byte limit",
                tb::file_name(p.file)
            ));
            return None;
        }
        match raw.read_raw(p.file, p.offset, size) {
            Ok(bytes) => {
                self.blob_bytes += u64::from(size);
                self.blobs.push(Blob {
                    file: tb::file_name(p.file).to_owned(),
                    offset: p.offset,
                    bytes,
                });
                let i = self.blobs.len() - 1;
                self.blob_at.insert((p.file, p.offset, size), i);
                Some(i)
            }
            Err(e) => {
                self.left_out
                    .push(format!("{field}: {} at {:#x}: {e}", tb::file_name(p.file), p.offset));
                None
            }
        }
    }
}

/// Tags `roots` of `doc` (and with `dependencies` what they reference) as
/// a bundle from map `origin`. `raw` reads the blobs when `resources`.
pub fn export(
    doc: &TagDoc,
    roots: &[TagId],
    origin: Origin,
    opts: &ExportOptions,
    raw: Option<&mut dyn RawReader>,
) -> Result<(Bundle, ExportReport), String> {
    let mut cap = Capture {
        doc,
        raw: if opts.resources { raw } else { None },
        blobs: Vec::new(),
        blob_at: HashMap::new(),
        blob_bytes: 0,
        refs: Vec::new(),
        left_out: Vec::new(),
        elements: 0,
    };
    let mut bundle = Bundle::new(origin);
    let mut queue: VecDeque<TagId> = roots.iter().copied().collect();
    let mut seen: HashSet<TagId> = roots.iter().copied().collect();
    let roots: HashSet<TagId> = roots.iter().copied().collect();
    let mut over = 0usize;
    while let Some(id) = queue.pop_front() {
        let t = doc.tag(id).ok_or_else(|| format!("no tag {:#010x}", id.0))?;
        let name = label(doc, id);
        let layout = match (&t.access, doc.layout(id)) {
            (Access::None(why), _) => Err(why.to_string()),
            (_, None) => Err(super::tag_doc::NO_LAYOUT.to_owned()),
            (_, Some(l)) => Ok(l.clone()),
        };
        let layout = match layout {
            Ok(l) => l,
            // A root must export; a dependency is left out, said so.
            Err(why) if roots.contains(&id) => return Err(format!("{name}: {why}")),
            Err(why) => {
                cap.left_out.push(format!("{name}: {why}"));
                continue;
            }
        };
        if t.size < layout.size {
            let why = super::tag_doc::TOO_SHORT;
            if roots.contains(&id) {
                return Err(format!("{name}: {why}"));
            }
            cap.left_out.push(format!("{name}: {why}"));
            continue;
        }
        cap.refs.clear();
        let root = cap
            .element(t.address, &layout, 0)
            .map_err(|e| format!("{name}: {e}"))?;
        bundle.tags.push(BundleTag {
            group: tags::defs::fourcc_text(t.group),
            name: t.name.clone(),
            layout: tb::fingerprint(&layout),
            root,
            fields: tag_api::read_tag(doc, id)
                .map(|v| v["fields"].clone())
                .unwrap_or_default(),
        });
        if opts.dependencies {
            for r in std::mem::take(&mut cap.refs) {
                if seen.insert(r) {
                    if seen.len() > DEPENDENCIES {
                        over += 1;
                        continue;
                    }
                    queue.push_back(r);
                }
            }
        }
    }
    if over > 0 {
        cap.left_out.push(format!(
            "{over} more referenced tags past the {DEPENDENCIES}-tag limit"
        ));
    }
    let report = ExportReport {
        tags: bundle.tags.iter().map(BundleTag::label).collect(),
        blobs: cap.blobs.len(),
        blob_bytes: cap.blob_bytes,
        left_out: cap.left_out,
    };
    bundle.blobs = cap.blobs;
    Ok((bundle, report))
}

/// What an import replaces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportOptions {
    /// For a bundle of one tag: the map's tag it replaces (else the tag of
    /// the same group and name).
    pub target: Option<TagId>,
    /// Import the tags the map has and skip the rest, rather than refuse.
    pub skip_missing: bool,
}

/// Where the bundle's raw pointers may be carried.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fit {
    /// The bundle came from this very map: its local pointers hold.
    pub same_map: bool,
    /// The bundle came from a map of the same build: its shared caches'
    /// pointers hold.
    pub same_build: bool,
}

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// `group:name` of each tag replaced.
    pub replaced: Vec<String>,
    /// Bundle tags the map lacks, skipped (`skip_missing`).
    pub skipped: Vec<String>,
    /// Raw pointers that kept the map's own (not carried).
    pub raw_kept: usize,
    /// References to tags the bundle's map lacked, written as none.
    pub nulled: Vec<String>,
    /// String IDs added to the map.
    pub strings_added: usize,
}

struct Apply<'a> {
    doc: &'a mut TagDoc,
    fit: Fit,
    by_name: HashMap<String, (TagId, FourCC)>,
    report: ImportReport,
    elements: usize,
}

fn keep(out: &mut [u8], now: &[u8], o: usize, w: usize) {
    out[o..o + w].copy_from_slice(&now[o..o + w]);
}

impl Apply<'_> {
    /// Make the block at `pair` hold `want` elements: removals from the
    /// end, blank additions (growing it when it is full and growth is on).
    fn resize(&mut self, tag: TagId, pair: u32, child: &Layout, want: u32) -> Result<(), String> {
        let (mut n, _) = self
            .doc
            .block(pair, child)
            .ok_or("the block's elements are outside the map's metadata")?;
        while n > want {
            self.doc.block_op(tag, pair, child, &BlockOp::Remove(n - 1))?;
            n -= 1;
        }
        while n < want {
            self.doc.block_op(tag, pair, child, &BlockOp::Add)?;
            n += 1;
        }
        Ok(())
    }

    fn element(
        &mut self,
        tag: TagId,
        at: u32,
        l: &Layout,
        e: &Element,
        depth: usize,
    ) -> Result<(), String> {
        if depth > DEPTH {
            return Err(format!("{}: blocks nest deeper than {DEPTH}", l.name));
        }
        self.elements += 1;
        if self.elements > ELEMENTS {
            return Err(format!("more than {ELEMENTS} elements"));
        }
        if e.bytes.len() != l.size as usize {
            return Err(format!(
                "{}: {} bytes in the bundle, {} in the layout",
                l.name,
                e.bytes.len(),
                l.size
            ));
        }
        let fix: HashMap<u32, &Fixup> = e.fixups.iter().map(|f| (f.at(), f)).collect();
        let missing = |name: &str| format!("{name}: the bundle has nothing for this field");
        // Blocks first: their counts change, and maybe where they are.
        for f in &l.fields {
            if let Kind::Block(child) = &f.kind {
                let Some(Fixup::Block { elements, .. }) = fix.get(&f.offset) else {
                    return Err(missing(&f.name));
                };
                let want = u32::try_from(elements.len()).map_err(|_| "too many elements")?;
                self.resize(tag, at + f.offset, child, want)
                    .map_err(|e| format!("{}: {e}", f.name))?;
            }
        }
        let now = self
            .doc
            .read(at, l.size)
            .ok_or_else(|| format!("{} at {at:#x} is outside the metadata", l.name))?;
        let mut out = e.bytes.clone();
        let mut strings: Vec<(u32, String, String)> = Vec::new();
        let mut kept_raw: Vec<u32> = Vec::new();
        for f in &l.fields {
            let o = f.offset as usize;
            let w = f.kind.width() as usize;
            match &f.kind {
                // Counts and addresses are the map's; their contents
                // follow.
                Kind::Block(_) | Kind::Data | Kind::Bsp => keep(&mut out, &now, o, w),
                Kind::TagRef { groups } | Kind::Datum { groups } => {
                    let is_ref = matches!(f.kind, Kind::TagRef { .. });
                    let datum_at = if is_ref { o + 4 } else { o };
                    match fix.get(&f.offset) {
                        Some(Fixup::Tag {
                            target: Some(name), ..
                        }) => {
                            let &(id, g) = self.by_name.get(name).ok_or_else(|| {
                                format!(
                                    "{}: this map has no {name} (import it first: the map writer \
                                     does not add tags)",
                                    f.name
                                )
                            })?;
                            if !self.doc.fits(g, groups) {
                                return Err(format!("{}: {name} is not of an allowed group", f.name));
                            }
                            out[datum_at..datum_at + 4].copy_from_slice(&id.0.to_le_bytes());
                            if is_ref {
                                out[o..o + 4].copy_from_slice(&g.as_u32().to_le_bytes());
                            }
                        }
                        Some(Fixup::Tag { target: None, .. }) => {
                            // None, or an ID the bundle's map lacked: its
                            // value holds only in that map.
                            let v = u32_at(&out, datum_at);
                            if v != u32::MAX && !self.fit.same_map {
                                out[datum_at..datum_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
                                self.report.nulled.push(f.name.clone());
                            }
                        }
                        _ => return Err(missing(&f.name)),
                    }
                }
                Kind::StringId => match fix.get(&f.offset) {
                    Some(Fixup::StringId { text: Some(s), .. }) => match self.doc.string_id(s) {
                        Some(v) => out[o..o + 4].copy_from_slice(&v.to_le_bytes()),
                        None if s.is_empty() => out[o..o + 4].copy_from_slice(&0u32.to_le_bytes()),
                        None => {
                            keep(&mut out, &now, o, 4);
                            strings.push((at + f.offset, s.clone(), f.name.clone()));
                        }
                    },
                    Some(Fixup::StringId { text: None, .. }) => {
                        if !self.fit.same_map {
                            keep(&mut out, &now, o, 4);
                        }
                    }
                    _ => return Err(missing(&f.name)),
                },
                Kind::Raw => {
                    let carry = match fix.get(&f.offset) {
                        Some(Fixup::Raw { file, .. }) => match tb::parse_file(file) {
                            Some(ResourceFile::Local) => self.fit.same_map,
                            Some(_) => self.fit.same_build,
                            None => return Err(format!("{}: no resource file {file:?}", f.name)),
                        },
                        // Zero or none in the bundle: the same anywhere.
                        None => true,
                        _ => return Err(missing(&f.name)),
                    };
                    if !carry {
                        keep(&mut out, &now, o, w);
                        kept_raw.push(f.offset);
                        if u32_at(&now, o) != u32_at(&e.bytes, o) {
                            self.report.raw_kept += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        // A kept pointer keeps its size too.
        for f in &l.fields {
            if let Kind::RawSize { of } = f.kind
                && kept_raw.contains(&of)
            {
                keep(&mut out, &now, f.offset as usize, 4);
            }
        }
        if out != now {
            self.doc.write(TagWrite {
                tag,
                address: at,
                bytes: out,
                label: l.name.clone(),
                merge: None,
            })?;
        }
        for (address, text, name) in strings {
            self.doc.set_string(tag, address, &text, &name)?;
            self.report.strings_added += 1;
        }
        for f in &l.fields {
            match &f.kind {
                Kind::Data => {
                    let Some(Fixup::Data { bytes, .. }) = fix.get(&f.offset) else {
                        return Err(missing(&f.name));
                    };
                    let (n, a) = self.doc.pair(at + f.offset).ok_or("unreadable")?;
                    if n as usize != bytes.len() {
                        return Err(format!(
                            "{}: {} bytes in the bundle and {n} in the map (data keeps its length)",
                            f.name,
                            bytes.len()
                        ));
                    }
                    if n > 0 && self.doc.read(a, n).as_deref() != Some(bytes.as_slice()) {
                        self.doc.write(TagWrite {
                            tag,
                            address: a,
                            bytes: bytes.clone(),
                            label: f.name.clone(),
                            merge: None,
                        })?;
                    }
                }
                Kind::Block(child) => {
                    let Some(Fixup::Block { elements, .. }) = fix.get(&f.offset) else {
                        return Err(missing(&f.name));
                    };
                    let (n, a) = self
                        .doc
                        .block(at + f.offset, child)
                        .ok_or_else(|| format!("{}: the block is outside the metadata", f.name))?;
                    if n as usize != elements.len() {
                        return Err(format!("{}: the block did not take its elements", f.name));
                    }
                    for (i, el) in elements.iter().enumerate() {
                        self.element(tag, a + i as u32 * child.size, child, el, depth + 1)
                            .map_err(|e| format!("{}[{i}].{e}", f.name))?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Replace tags of `doc` with `bundle`'s, as one undo step (named after
/// them). Checks every tag before writing; a failure part way leaves the
/// document as it was.
pub fn import(
    doc: &mut TagDoc,
    bundle: &Bundle,
    fit: Fit,
    opts: &ImportOptions,
) -> Result<ImportReport, String> {
    bundle.check_blobs()?;
    if opts.target.is_some() && bundle.tags.len() != 1 {
        return Err(format!(
            "a target tag takes a bundle of one tag; this one has {}",
            bundle.tags.len()
        ));
    }
    let by_name: HashMap<String, (TagId, FourCC)> = doc
        .tags()
        .iter()
        .map(|t| (label(doc, t.id), (t.id, t.group)))
        .collect();
    let mut plan: Vec<(usize, TagId)> = Vec::new();
    let mut missing = Vec::new();
    for (k, t) in bundle.tags.iter().enumerate() {
        match opts.target.or_else(|| by_name.get(&t.label()).map(|x| x.0)) {
            Some(id) => plan.push((k, id)),
            None => missing.push(t.label()),
        }
    }
    if !missing.is_empty() && !opts.skip_missing {
        return Err(format!(
            "this map has no {}: the map writer replaces tags and does not add them (skip_missing \
             imports the rest)",
            missing.join(", ")
        ));
    }
    if plan.is_empty() {
        return Err("nothing in the bundle to import into this map".into());
    }
    let mut layouts: Vec<Arc<Layout>> = Vec::new();
    for &(k, id) in &plan {
        let t = &bundle.tags[k];
        let here = doc.tag(id).ok_or("no such tag")?;
        let name = label(doc, id);
        if let Some(why) = doc.refusal(id) {
            return Err(format!("{name}: {why}"));
        }
        let group = tags::defs::fourcc_text(here.group);
        if group != t.group {
            return Err(format!("{name} is a {group}; the bundle's {} is a {}", t.label(), t.group));
        }
        if MAP_BOUND.contains(&&here.group.0) && !fit.same_map {
            return Err(format!(
                "{name}: a {group} indexes its map's sound gestalt (ugh!) by number, so it imports \
                 only into the map it came from"
            ));
        }
        let layout = doc
            .layout(id)
            .ok_or_else(|| format!("{name}: {}", super::tag_doc::NO_LAYOUT))?
            .clone();
        let fp = tb::fingerprint(&layout);
        if fp != t.layout {
            return Err(format!(
                "{name}: the bundle was read with another layout of {group} ({}; this editor's is \
                 {fp}): export it again with this editor",
                t.layout
            ));
        }
        layouts.push(layout);
    }
    let mark = doc.undo_len();
    let mut run = Apply {
        doc: &mut *doc,
        fit,
        by_name,
        report: ImportReport::default(),
        elements: 0,
    };
    for (&(k, id), layout) in plan.iter().zip(&layouts) {
        let t = &bundle.tags[k];
        let name = label(run.doc, id);
        let at = run.doc.tag(id).map(|e| e.address).ok_or("no such tag")?;
        if let Err(e) = run.element(id, at, layout, &t.root, 0) {
            run.doc.rollback(mark);
            return Err(format!("{name}: {e}"));
        }
        run.report.replaced.push(name);
    }
    let mut report = run.report;
    report.skipped = missing;
    let what = match report.replaced.as_slice() {
        [one] => one.clone(),
        many => format!("{} tags", many.len()),
    };
    doc.squash(mark, format!("import {what}"));
    Ok(report)
}

/// Read a bundle file.
pub fn read(path: &Path) -> Result<Bundle, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Bundle::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Where exports go when no path is given: `tag-bundles/` under the data
/// root (user data, never a checkout).
pub fn default_dir(data_root: &Path) -> PathBuf {
    data_root.join("tag-bundles")
}

impl App {
    /// [`super::Action::TagExport`]: write tags of the shown document as a
    /// bundle to `out` (refused inside a Git checkout).
    pub(super) fn export_tags(
        &mut self,
        ids: &[TagId],
        out: Option<&Path>,
        opts: &ExportOptions,
    ) -> Result<(PathBuf, ExportReport), String> {
        let doc = self.shown_tags();
        let first = *ids.first().ok_or("no tag to export")?;
        let path = match out {
            Some(p) => p.to_owned(),
            None => {
                let t = doc.tag(first).ok_or("no such tag")?;
                default_dir(&self.data_root()).join(tb::default_file_name(
                    &tags::defs::fourcc_text(t.group),
                    &t.name,
                ))
            }
        };
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
        if let Some(repo) = super::ui_screens::git_checkout(dir.unwrap_or(Path::new("."))) {
            return Err(format!(
                "{} is inside the Git checkout {}: tags from your maps stay out of repositories; \
                 pick a path outside it",
                path.display(),
                repo.display()
            ));
        }
        let build = self
            .open_shown_map()
            .map(|m| m.header().build.clone())
            .unwrap_or_default();
        let origin = Origin {
            map: self.shown_map_name().unwrap_or_default().to_owned(),
            build,
        };
        let mut files = if opts.resources {
            Some(self.shown_raw_files()?)
        } else {
            None
        };
        let raw = files.as_mut().map(|f| f as &mut dyn RawReader);
        let (bundle, report) = export(self.shown_tags(), ids, origin, opts, raw)?;
        if let Some(d) = dir {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(&path, bundle.to_json()).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok((path, report))
    }

    /// [`super::Action::TagImport`]: replace tags of the open map with a
    /// bundle's, as one undo step of the Tags tool.
    pub(super) fn import_tags(
        &mut self,
        path: &Path,
        opts: &ImportOptions,
    ) -> Result<ImportReport, String> {
        if let Some(b) = self.browsing() {
            return Err(format!(
                "{b} is browsed read-only: import into the open map (Back to the open map)"
            ));
        }
        self.sync_tags();
        if !self.tags.is_open() {
            return Err("open a map first: tags import into the map the Scenario tool has open".into());
        }
        let bundle = read(path)?;
        let build = self
            .open_shown_map()
            .map(|m| m.header().build.clone())
            .unwrap_or_default();
        let here = self.loaded.clone().unwrap_or_default();
        let same_build = !build.is_empty() && bundle.origin.build == build;
        let fit = Fit {
            same_map: same_build && bundle.origin.map.eq_ignore_ascii_case(&here),
            same_build,
        };
        import(&mut self.tags, &bundle, fit, opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_folder_is_under_the_data_root() {
        let d = default_dir(Path::new("/data"));
        assert_eq!(d, Path::new("/data/tag-bundles"));
    }

    #[test]
    fn an_unknown_resource_file_name_reads_as_none() {
        assert!(tb::parse_file("elsewhere").is_none());
        let b = Bundle::new(Origin::default());
        assert!(b.tags.is_empty());
    }
}
