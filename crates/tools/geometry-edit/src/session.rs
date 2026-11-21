//! `GeometrySession`: the open targets, their meshes with snapshot undo,
//! the op log, write classes and the patches a save writes
//! (docs/editor/modeling-targets.md, "The session and the save").

use std::fmt;
use std::io::{Read, Seek};
use std::sync::Arc;

use editor_core::UndoStack;
use map::{AddressSpace, Map, RawPointer, ResourceFile, TagId};
use mesh_edit::{Export, Mesh, MeshError, MeshOp, OpResult, dirty, face_flags};
use tags::geometry::{Compression, RawSource, TagView};
use tags::geometry_write::changed_runs;
use tags::model::{MODEL_SECTION, RenderModelGeometry};
use tags::structure::{InstancedGeometryInstance, STRUCTURE_SECTION, StructureRender};

use crate::import::section_source;
use crate::target::{Target, TargetKind};
use crate::write::{SectionSource, block_runs, rewrite};

/// Why the session could not do something.
#[derive(Debug)]
pub enum Error {
    Tags(tags::Error),
    Map(map::Error),
    /// The target does not exist in the map, or has no raw block.
    NoTarget(String),
    /// The target is not open (`geometry.open` first).
    NotOpen(String),
    /// An op failed; nothing changed.
    Mesh(MeshError),
    /// An op list failed at this index; nothing changed.
    Op(usize, MeshError),
    /// A save cannot write the edit: the target and why.
    Refused {
        target: String,
        reason: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Tags(e) => write!(f, "{e}"),
            Error::Map(e) => write!(f, "{e}"),
            Error::NoTarget(t) => write!(f, "no geometry target {t} in this map"),
            Error::NotOpen(t) => write!(f, "{t} is not open: geometry.open it first"),
            Error::Mesh(e) => write!(f, "{e}"),
            Error::Op(i, e) => write!(f, "op {i}: {e}"),
            Error::Refused { target, reason } => write!(f, "{target}: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<tags::Error> for Error {
    fn from(e: tags::Error) -> Self {
        Error::Tags(e)
    }
}
impl From<map::Error> for Error {
    fn from(e: map::Error) -> Self {
        Error::Map(e)
    }
}

/// The write class of a target's edit (docs/editor/modeling-targets.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteClass {
    /// Nothing a save would write.
    Unchanged,
    /// No count changes: fixed patches of the vertices and bounds.
    InPlace,
    /// A count changed: a new raw block (not written by this build).
    Grown,
}

impl WriteClass {
    pub const fn name(self) -> &'static str {
        match self {
            WriteClass::Unchanged => "unchanged",
            WriteClass::InPlace => "in_place",
            WriteClass::Grown => "grown_raw",
        }
    }
}

/// A fixed-size patch of tag bytes at a virtual address: in the ordinary
/// metadata (`space` `None`) or a BSP region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionWrite {
    pub space: Option<AddressSpace>,
    pub address: u32,
    pub bytes: Vec<u8>,
}

/// A fixed-size patch inside a local raw block
/// (`map::MapEditor::patch_raw`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawWrite {
    pub block: RawPointer,
    pub block_size: u32,
    pub offset: u32,
    pub bytes: Vec<u8>,
}

/// What a save writes for the session's edits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patches {
    pub tags: Vec<RegionWrite>,
    pub raw: Vec<RawWrite>,
}

impl Patches {
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty() && self.raw.is_empty()
    }
    /// Bytes written.
    pub fn len(&self) -> usize {
        self.tags.iter().map(|w| w.bytes.len()).sum::<usize>()
            + self.raw.iter().map(|w| w.bytes.len()).sum::<usize>()
    }
}

/// What `geometry.open` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenInfo {
    pub verts: usize,
    pub edges: usize,
    pub faces: usize,
    /// Source vertices welded into others at the same position.
    pub welded: usize,
    /// Source triangles kept as they are (not faces).
    pub passthrough: usize,
    pub local: bool,
}

/// `geometry.check` for one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub target: Target,
    pub class: WriteClass,
    pub verts: usize,
    pub faces: usize,
    /// Faces whose baked light no longer fits them.
    pub lightmap_stale: usize,
    /// Vertices whose stream bytes the save rewrites.
    pub moved: usize,
    /// Why a save refuses this target's edit, if it does.
    pub refused: Option<String>,
    pub ops: usize,
}

/// Where a tag field lives, for its patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FieldAt {
    space: Option<AddressSpace>,
    address: u32,
}

/// A render model's sections and box, shared by its open sections.
#[derive(Debug)]
struct ModelSource {
    tag: TagId,
    /// Each section, `None` when its block is not in this map.
    sections: Vec<Option<SectionSource>>,
    compression: Option<Compression>,
    /// The position bounds' 24 bytes (`ModeCompression +0`).
    box_at: Option<FieldAt>,
}

enum Source {
    Structure {
        section: SectionSource,
        /// A cluster's bounds (`+0x54`) and where they are.
        bounds: Option<([f32; 6], FieldAt)>,
        /// An instanced definition's placements.
        instances: Vec<(u32, InstancedGeometryInstance)>,
    },
    Model {
        model: Arc<ModelSource>,
        section: usize,
        local: bool,
    },
}

/// One open target: its source and the mesh as imported.
struct Opened {
    target: Target,
    name: String,
    source: Source,
    original: Arc<Mesh>,
}

/// The undone and redone state: every edited target's mesh and op log.
/// A target without an entry is as imported.
#[derive(Clone, Default)]
struct State {
    edits: Vec<(Target, Arc<Mesh>, Arc<Vec<MeshOp>>)>,
}

impl State {
    fn get(&self, t: Target) -> Option<&(Target, Arc<Mesh>, Arc<Vec<MeshOp>>)> {
        self.edits.iter().find(|e| e.0 == t)
    }
}

/// The open targets of one map, with undo.
#[derive(Default)]
pub struct GeometrySession {
    opened: Vec<Opened>,
    state: State,
    undo: UndoStack<State>,
    /// The target the window shows in edit mode.
    pub active: Option<Target>,
    /// Bumps on every change of a mesh (undo included), for redraws.
    rev: u64,
}

fn field_at<R: Read + Seek>(map: &Map<R>, space: AddressSpace, address: u32) -> FieldAt {
    FieldAt {
        space: (Some(space) != map.metadata_space()).then_some(space),
        address,
    }
}

fn floats(f: &[f32]) -> Vec<u8> {
    f.iter().flat_map(|v| v.to_bits().to_le_bytes()).collect()
}

/// The patches of one field: the runs where `new` differs from `old`.
fn field_writes(at: FieldAt, old: &[f32], new: &[f32], out: &mut Vec<RegionWrite>) {
    for (offset, bytes) in changed_runs(&floats(old), &floats(new)) {
        out.push(RegionWrite {
            space: at.space,
            address: at.address + offset as u32,
            bytes,
        });
    }
}

fn raw_writes(src: &SectionSource, runs: Vec<(usize, Vec<u8>)>, out: &mut Vec<RawWrite>) {
    for (offset, bytes) in runs {
        out.push(RawWrite {
            block: src.block,
            block_size: src.block_size,
            offset: offset as u32,
            bytes,
        });
    }
}

impl GeometrySession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every target and step (a newly opened map).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The open targets, in the order they were opened.
    pub fn open_targets(&self) -> Vec<Target> {
        self.opened.iter().map(|o| o.target).collect()
    }

    pub fn is_open(&self, t: Target) -> bool {
        self.opened.iter().any(|o| o.target == t)
    }

    /// The owning tag's name of an open target.
    pub fn name(&self, t: Target) -> Option<&str> {
        self.find(t).map(|o| o.name.as_str())
    }

    /// Changes on every mesh change, undo and redo included.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    fn find(&self, t: Target) -> Option<&Opened> {
        self.opened.iter().find(|o| o.target == t)
    }

    /// Open `target` (reading its section through `raw`), and make it
    /// the active one. Opening an open target only makes it active.
    pub fn open<R: Read + Seek>(
        &mut self,
        map: &mut Map<R>,
        raw: &mut dyn RawSource,
        target: Target,
    ) -> Result<OpenInfo, Error> {
        if !self.is_open(target) {
            let opened = self.read(map, raw, target)?;
            self.opened.push(opened);
        }
        self.active = Some(target);
        let o = self.find(target).expect("just opened");
        let mesh = self.mesh(target).expect("open");
        let (verts, edges, faces) = mesh.counts();
        let source = o.original.source().expect("imported");
        let welded = source
            .positions
            .len()
            .saturating_sub(o.original.vert_count());
        let passthrough = source
            .triangles
            .len()
            .saturating_sub(o.original.face_count());
        Ok(OpenInfo {
            verts,
            edges,
            faces,
            welded,
            passthrough,
            local: self.is_local(target),
        })
    }

    fn is_local(&self, t: Target) -> bool {
        match self.find(t).map(|o| &o.source) {
            Some(Source::Structure { section, .. }) => section.block.file == ResourceFile::Local,
            Some(Source::Model { local, .. }) => *local,
            None => false,
        }
    }

    /// Leave edit mode on a target. An edited target stays open (its edits
    /// still save); an unedited one is dropped.
    pub fn close(&mut self, t: Target) -> bool {
        if self.active == Some(t) {
            self.active = None;
        }
        if self.edited(t) {
            return self.is_open(t);
        }
        let before = self.opened.len();
        self.opened.retain(|o| o.target != t);
        self.opened.len() != before
    }

    fn edited(&self, t: Target) -> bool {
        self.state.get(t).is_some()
    }

    fn read<R: Read + Seek>(
        &self,
        map: &mut Map<R>,
        raw: &mut dyn RawSource,
        target: Target,
    ) -> Result<Opened, Error> {
        let missing = || Error::NoTarget(target.id());
        let tag = map.tag(target.tag).map_err(|_| missing())?.clone();
        let name = tag
            .name
            .clone()
            .unwrap_or_else(|| format!("{:08x}", tag.id.0));
        let location = tag.location.ok_or_else(missing)?;
        let i = target.index as usize;
        match target.kind {
            TargetKind::Cluster | TargetKind::InstanceDef => {
                let render = StructureRender::read(map, target.tag)?;
                let header = match target.kind {
                    TargetKind::Cluster => render.clusters.get(i).map(|c| &c.geometry),
                    _ => render.instance_definitions.get(i).map(|d| &d.geometry),
                }
                .ok_or_else(missing)?;
                let section = header
                    .load(raw, Some(render.material_count as usize))?
                    .ok_or_else(missing)?;
                let source = section_source(&section, None)?;
                let src = SectionSource::new(
                    &section,
                    &header.block,
                    STRUCTURE_SECTION,
                    source.positions.clone(),
                    true,
                )
                .ok_or_else(missing)?;
                let bounds = if target.kind == TargetKind::Cluster {
                    let bytes = map.read_tag_bytes(target.tag)?;
                    let view = TagView::new(&bytes, location.address);
                    let head = view.at(0, 0xa4)?;
                    let address = u32::from_le_bytes(head[0xa0..0xa4].try_into().expect("4 bytes"));
                    let at = field_at(
                        map,
                        location.space,
                        address + (i * tags::structure::Cluster::SIZE) as u32 + 0x54,
                    );
                    Some((render.clusters[i].bounds, at))
                } else {
                    None
                };
                let instances = if target.kind == TargetKind::InstanceDef {
                    render
                        .instances
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.definition as usize == i)
                        .map(|(k, p)| (k as u32, p.clone()))
                        .collect()
                } else {
                    Vec::new()
                };
                let (mesh, _) = Mesh::from_triangles(source).map_err(Error::Mesh)?;
                Ok(Opened {
                    target,
                    name,
                    source: Source::Structure {
                        section: src,
                        bounds,
                        instances,
                    },
                    original: Arc::new(mesh),
                })
            }
            TargetKind::ModeSection => {
                let geometry = RenderModelGeometry::read(map, target.tag, raw)?;
                let model = match self.model(target.tag) {
                    Some(m) => m,
                    None => Arc::new(model_source(map, &geometry, location.space)),
                };
                let s = geometry.sections.get(i).ok_or_else(missing)?;
                let source = section_source(&s.geometry, model.compression.as_ref())?;
                let (mesh, _) = Mesh::from_triangles(source).map_err(Error::Mesh)?;
                Ok(Opened {
                    target,
                    name,
                    source: Source::Model {
                        local: model.sections.get(i).is_some_and(Option::is_some),
                        model,
                        section: i,
                    },
                    original: Arc::new(mesh),
                })
            }
        }
    }

    /// A model already read for another of its open sections.
    fn model(&self, tag: TagId) -> Option<Arc<ModelSource>> {
        self.opened.iter().find_map(|o| match &o.source {
            Source::Model { model, .. } if model.tag == tag => Some(model.clone()),
            _ => None,
        })
    }

    /// The current mesh of an open target.
    pub fn mesh(&self, t: Target) -> Option<&Arc<Mesh>> {
        match self.state.get(t) {
            Some((_, m, _)) => Some(m),
            None => self.find(t).map(|o| &o.original),
        }
    }

    /// The op log: every `MeshOp` applied since the target was opened.
    /// Replaying it on the imported mesh gives the current one.
    pub fn ops(&self, t: Target) -> &[MeshOp] {
        self.state.get(t).map_or(&[], |e| e.2.as_slice())
    }

    /// An open structure definition's placements (index, placement).
    pub fn instances(&self, t: Target) -> &[(u32, InstancedGeometryInstance)] {
        match self.find(t).map(|o| &o.source) {
            Some(Source::Structure { instances, .. }) => instances,
            _ => &[],
        }
    }

    /// Apply `ops` to an open target as one undo step: all of them or,
    /// when one fails, none. With `merge`, a run of calls with the same
    /// key is one step (a gizmo drag), until [`Self::seal`].
    pub fn apply(
        &mut self,
        t: Target,
        ops: &[MeshOp],
        merge: Option<u64>,
    ) -> Result<OpResult, Error> {
        let mut mesh = Mesh::clone(self.mesh(t).ok_or_else(|| Error::NotOpen(t.id()))?);
        let mut last = OpResult::default();
        for (i, op) in ops.iter().enumerate() {
            last = mesh.apply(op).map_err(|e| {
                if ops.len() == 1 {
                    Error::Mesh(e)
                } else {
                    Error::Op(i, e)
                }
            })?;
        }
        if ops.is_empty() {
            let (verts, edges, faces) = mesh.counts();
            last = OpResult {
                verts,
                edges,
                faces,
                selected: mesh.selection().len(),
                ..OpResult::default()
            };
        }
        let before = self.state.clone();
        match merge {
            Some(key) => self.undo.record_coalesced(before, key),
            None => self.undo.record(before),
        }
        let mut log = self.ops(t).to_vec();
        log.extend_from_slice(ops);
        let entry = (t, Arc::new(mesh), Arc::new(log));
        match self.state.edits.iter_mut().find(|e| e.0 == t) {
            Some(e) => *e = entry,
            None => self.state.edits.push(entry),
        }
        self.rev += 1;
        Ok(last)
    }

    /// End a run of merged steps.
    pub fn seal(&mut self) {
        self.undo.break_coalescing();
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }
    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }
    pub fn undo(&mut self) -> bool {
        let done = self.undo.undo(&mut self.state);
        self.rev += u64::from(done);
        done
    }
    pub fn redo(&mut self) -> bool {
        let done = self.undo.redo(&mut self.state);
        self.rev += u64::from(done);
        done
    }

    /// The write class of a target's edit, and why it cannot be written
    /// when it cannot.
    fn classify(&self, o: &Opened) -> (WriteClass, Option<String>, Export) {
        let mesh = self.mesh(o.target).expect("open");
        let export = mesh.export();
        let source = o.original.source().expect("imported");
        if export.positions.len() != source.positions.len() || export.triangles != source.triangles
        {
            let reason = format!(
                "the edit changes the section's counts ({} to {} vertices, {} to {} triangles): a grown write, not built yet",
                source.positions.len(),
                export.positions.len(),
                source.triangles.len(),
                export.triangles.len()
            );
            return (WriteClass::Grown, Some(reason), export);
        }
        if export.materials != source.materials {
            let reason = "a face's material changed: materials are per part, and a part split is a grown write".to_owned();
            return (WriteClass::Grown, Some(reason), export);
        }
        let changed = export.corners.iter().any(|c| c.dirty != 0);
        let class = if changed {
            WriteClass::InPlace
        } else {
            WriteClass::Unchanged
        };
        let refused = (changed && !self.is_local(o.target)).then(|| {
            "its raw block is in a shared cache, which the editor never writes".to_owned()
        });
        (class, refused, export)
    }

    /// `geometry.check` for every open target, or one.
    pub fn check(&self, only: Option<Target>) -> Vec<Check> {
        let mut out = Vec::new();
        for o in &self.opened {
            if only.is_some_and(|t| t != o.target) {
                continue;
            }
            let mesh = self.mesh(o.target).expect("open");
            let (class, mut refused, export) = self.classify(o);
            if refused.is_none() && class == WriteClass::InPlace {
                refused = self.target_patches(o, &export).err();
            }
            out.push(Check {
                target: o.target,
                class,
                verts: mesh.vert_count(),
                faces: mesh.face_count(),
                lightmap_stale: mesh
                    .face_ids()
                    .filter(|&f| mesh.face_flags(f) & face_flags::LIGHTMAP_STALE != 0)
                    .count(),
                moved: export
                    .corners
                    .iter()
                    .filter(|c| c.dirty & dirty::POSITION != 0)
                    .count(),
                refused,
                ops: self.ops(o.target).len(),
            });
        }
        out
    }

    /// Whether any open target has an edit a save would write or refuse.
    pub fn has_changes(&self) -> bool {
        self.opened.iter().any(|o| {
            self.state.get(o.target).is_some() && self.classify(o).0 != WriteClass::Unchanged
        })
    }

    /// The patches of one structure target or one model section (no box
    /// growth: that is the model's, in [`Self::changes`]).
    fn target_patches(&self, o: &Opened, export: &Export) -> Result<Patches, String> {
        let mut p = Patches::default();
        match &o.source {
            Source::Structure {
                section, bounds, ..
            } => {
                let data = rewrite(
                    section,
                    &export.positions,
                    Some(&export.corners),
                    None,
                    false,
                )?;
                raw_writes(section, block_runs(section, &data)?, &mut p.raw);
                if let Some((old, at)) = bounds {
                    let mut new = *old;
                    for (i, c) in export.corners.iter().enumerate() {
                        if c.dirty & dirty::POSITION == 0 {
                            continue;
                        }
                        let q = export.positions[i];
                        for k in 0..3 {
                            new[2 * k] = new[2 * k].min(q[k]);
                            new[2 * k + 1] = new[2 * k + 1].max(q[k]);
                        }
                    }
                    field_writes(*at, old, &new, &mut p.tags);
                }
            }
            Source::Model { model, section, .. } => {
                let src = model.sections[*section]
                    .as_ref()
                    .ok_or("its raw block is in a shared cache, which the editor never writes")?;
                let data = rewrite(
                    src,
                    &export.positions,
                    Some(&export.corners),
                    model.compression.as_ref(),
                    false,
                )?;
                raw_writes(src, block_runs(src, &data)?, &mut p.raw);
            }
        }
        Ok(p)
    }

    /// What a save writes: every open target's in-place patches. A
    /// model's compression box grows to hold its edited sections'
    /// vertices, and then every section of the model is requantised into
    /// it (each at its own size). Refused whole, naming the target, when
    /// an edit is not in place or cannot be encoded.
    pub fn changes(&self) -> Result<Patches, Error> {
        let mut out = Patches::default();
        let refuse = |t: Target, reason: String| Error::Refused {
            target: t.id(),
            reason,
        };
        let mut models: Vec<(Arc<ModelSource>, Vec<(usize, Export)>)> = Vec::new();
        for o in &self.opened {
            if self.state.get(o.target).is_none() {
                continue;
            }
            let (class, refused, export) = self.classify(o);
            if let Some(r) = refused {
                return Err(refuse(o.target, r));
            }
            if class == WriteClass::Unchanged {
                continue;
            }
            match &o.source {
                Source::Structure { .. } => {
                    let p = self
                        .target_patches(o, &export)
                        .map_err(|r| refuse(o.target, r))?;
                    out.tags.extend(p.tags);
                    out.raw.extend(p.raw);
                }
                Source::Model { model, section, .. } => {
                    match models.iter_mut().find(|(m, _)| Arc::ptr_eq(m, model)) {
                        Some((_, list)) => list.push((*section, export)),
                        None => models.push((model.clone(), vec![(*section, export)])),
                    }
                }
            }
        }
        for (model, edited) in models {
            let target = |s: usize| Target::new(TargetKind::ModeSection, model.tag, s as u32);
            let old = model.compression.unwrap_or_default();
            let mut grown = old.position;
            for (_, e) in &edited {
                for (i, c) in e.corners.iter().enumerate() {
                    if c.dirty & dirty::POSITION == 0 {
                        continue;
                    }
                    for k in 0..3 {
                        grown[2 * k] = grown[2 * k].min(e.positions[i][k]);
                        grown[2 * k + 1] = grown[2 * k + 1].max(e.positions[i][k]);
                    }
                }
            }
            if grown == old.position {
                for (s, e) in &edited {
                    let src = model.sections[*s].as_ref().ok_or_else(|| {
                        refuse(
                            target(*s),
                            "its raw block is in a shared cache, which the editor never writes"
                                .into(),
                        )
                    })?;
                    let data = rewrite(
                        src,
                        &e.positions,
                        Some(&e.corners),
                        model.compression.as_ref(),
                        false,
                    )
                    .map_err(|r| refuse(target(*s), r))?;
                    raw_writes(
                        src,
                        block_runs(src, &data).map_err(|r| refuse(target(*s), r))?,
                        &mut out.raw,
                    );
                }
                continue;
            }
            // The box grows: every section is requantised into it.
            let at = model.box_at.ok_or_else(|| {
                refuse(
                    target(edited[0].0),
                    "the model has no compression box to grow".into(),
                )
            })?;
            let boxed = Compression {
                position: grown,
                texcoord: old.texcoord,
            };
            for (s, src) in model.sections.iter().enumerate() {
                let src = src.as_ref().ok_or_else(|| {
                    refuse(
                        target(s),
                        "growing the model's compression box rewrites every section, and this one is in a shared cache".into(),
                    )
                })?;
                let edit = edited.iter().find(|(k, _)| *k == s).map(|(_, e)| e);
                let data = match edit {
                    Some(e) => rewrite(src, &e.positions, Some(&e.corners), Some(&boxed), true),
                    None => rewrite(src, &src.positions, None, Some(&boxed), true),
                }
                .map_err(|r| refuse(target(s), r))?;
                raw_writes(
                    src,
                    block_runs(src, &data).map_err(|r| refuse(target(s), r))?,
                    &mut out.raw,
                );
            }
            field_writes(at, &old.position, &grown, &mut out.tags);
        }
        Ok(out)
    }

    /// The positions a native preview draws for an open target, per
    /// source vertex in section space, when its edit is in place (a
    /// grown edit has no source vertex order to update).
    pub fn preview_positions(&self, t: Target) -> Option<Vec<[f32; 3]>> {
        let o = self.find(t)?;
        let (class, _, export) = self.classify(o);
        (class != WriteClass::Grown).then_some(export.positions)
    }

    /// The write class of an open target.
    pub fn class(&self, t: Target) -> Option<WriteClass> {
        self.find(t).map(|o| self.classify(o).0)
    }
}

/// A render model's sections and compression box.
fn model_source<R: Read + Seek>(
    map: &Map<R>,
    m: &RenderModelGeometry,
    space: AddressSpace,
) -> ModelSource {
    let compression = m.compression.as_ref().map(|c| Compression {
        position: c.position_bounds,
        texcoord: c.texcoord_bounds,
    });
    let box_at = (m.header.compression_infos.count == 1)
        .then(|| field_at(map, space, m.header.compression_infos.address));
    let sections = m
        .sections
        .iter()
        .map(|s| {
            if s.raw.file != ResourceFile::Local {
                return None;
            }
            SectionSource::new(
                &s.geometry,
                &s.block,
                MODEL_SECTION,
                s.vertices.positions.clone(),
                false,
            )
        })
        .collect();
    ModelSource {
        tag: m.id,
        sections,
        compression,
        box_at,
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
