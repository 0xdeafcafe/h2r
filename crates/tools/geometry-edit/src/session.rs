//! `GeometrySession`: the open targets, their meshes with snapshot undo,
//! the op log, write classes and the patches a save writes
//! (docs/editor/modeling-targets.md, "The session and the save").

use std::fmt;
use std::io::{Read, Seek};
use std::sync::Arc;

use collision_build::coll;
use collision_build::from_render::{FromRender, from_render};
use editor_core::UndoStack;
use map::{AddressSpace, FourCC, Map, RawPointer, ResourceFile, TagId};
use mesh_edit::{Export, Mesh, MeshError, MeshOp, OpResult, PolygonFace, dirty, face_flags};
use tags::geometry::{Compression, RawSource, TagView};
use tags::geometry_write::changed_runs;
use tags::model::{MODEL_SECTION, RenderModelGeometry};
use tags::structure::{
    BUCKET_FLAG_PATCHED, InstancedGeometryDefinition, InstancedGeometryInstance, Lightmap,
    STRUCTURE_SECTION, StructureRender,
};

use crate::coll_target::CollSource;
use crate::grow::{self, Grown};
use crate::import::section_source;
use crate::level_collision::GrownBlock;
use crate::target::{Target, TargetKind};
use crate::write::{SectionSource, block_runs, rewrite};

/// Alignment of an appended section block in the file (`map::build` places
/// local raw resources at least 0x200-aligned, docs/formats/map-builder.md).
pub const RAW_ALIGN: u32 = 0x200;
/// Offset of the block info in a structure record (after the 0x28-byte
/// section info) and in a model section record.
const STRUCTURE_BLOCK_INFO: usize = 0x28;
const MODEL_BLOCK_INFO: usize = 0x38;
/// Model section record stride, and a permutation's (`ModePermutation`,
/// LOD section indices at `+4`).
const MODEL_SECTION_RECORD: usize = 0x5c;
const PERMUTATION_RECORD: usize = 0x10;
/// The sky tag's render model reference datum (`sky +0x4`).
const SKY_MODEL_DATUM: usize = 0x4;

/// One section the native world draws a structure target as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawn {
    /// A cluster, by index in `sbsp+0x9c`.
    Cluster(u32),
    /// An instanced-geometry placement, by index in `sbsp+0x140`.
    Instance(u32),
}

/// What the live preview redraws for one open target
/// ([`GeometrySession::previews`]): positions per source vertex in the
/// section's own space.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub bsp: TagId,
    pub drawn: Vec<Drawn>,
    pub positions: Vec<[f32; 3]>,
}

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
    /// A count changed: a new raw block appended to the map, and the
    /// owner's block info, counts and bounds pointed at it.
    Grown,
    /// A collision BSP compiled again: its blocks patched where the count
    /// holds, else grown in the ordinary metadata.
    GrownBlocks,
}

impl WriteClass {
    pub const fn name(self) -> &'static str {
        match self {
            WriteClass::Unchanged => "unchanged",
            WriteClass::InPlace => "in_place",
            WriteClass::Grown => "grown_raw",
            WriteClass::GrownBlocks => "grown_blocks",
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
    /// The bytes the map holds there, as long as `bytes`: what a live
    /// write sends as `was`, and what undo sends back
    /// (docs/editor/realtime-edits.md, "The editor").
    pub was: Vec<u8>,
}

/// A fixed-size patch inside a local raw block
/// (`map::MapEditor::patch_raw`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawWrite {
    /// The section whose block this is: a live write's owner (a model's
    /// requantised sections name their own section, not the edited one).
    pub owner: Target,
    pub block: RawPointer,
    pub block_size: u32,
    pub offset: u32,
    pub bytes: Vec<u8>,
    /// The block's bytes there in the map, as long as `bytes`.
    pub was: Vec<u8>,
}

/// A grown section's new raw block (`map::MapEditor::append_raw`) and the
/// fixed-size tag writes that only hold with it.
///
/// A save appends `bytes`, writes the new raw pointer word at `pointer`
/// (the owner's block info `+0`), then `writes`. The live path sends the
/// same bytes in the same order (docs/editor/realtime-edits.md,
/// "Geometry"): `edit.raw.append` with `bytes` and `align`, then one
/// `edit.batch` with the pointer word it returned and `writes`, evicting
/// `target`'s section. The old block stays in the file, unreferenced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawAppend {
    pub target: Target,
    pub bytes: Vec<u8>,
    pub align: u32,
    /// The block info's raw pointer word: ordinary metadata (`space`
    /// `None`) or a BSP region.
    pub pointer_space: Option<AddressSpace>,
    pub pointer_address: u32,
    /// The block the pointer named before.
    pub old: RawPointer,
    pub old_size: u32,
    /// Block info sizes and resource entries, section counts, part and
    /// cluster bounds: the runs that change.
    pub writes: Vec<RegionWrite>,
}

/// What a save writes for the session's edits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patches {
    pub tags: Vec<RegionWrite>,
    pub raw: Vec<RawWrite>,
    /// Grown sections' new blocks, in target order.
    pub appended: Vec<RawAppend>,
    /// Tag blocks with a new element count, appended to the ordinary
    /// metadata with their pairs pointed there (a recompiled `coll` BSP).
    pub grown: Vec<GrownBlock>,
}

impl Patches {
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty() && self.raw.is_empty() && self.appended.is_empty() && self.grown.is_empty()
    }
    /// Bytes written.
    pub fn len(&self) -> usize {
        self.tags.iter().map(|w| w.bytes.len()).sum::<usize>()
            + self.raw.iter().map(|w| w.bytes.len()).sum::<usize>()
            + self
                .appended
                .iter()
                .map(|a| a.bytes.len() + 4 + a.writes.iter().map(|w| w.bytes.len()).sum::<usize>())
                .sum::<usize>()
            + self.grown.iter().map(|g| g.bytes.len() + 4).sum::<usize>()
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

/// A section's owner record (a cluster, a definition, a model section) as
/// read, and its block info's resource entries: what a grown write
/// rewrites.
#[derive(Clone, Debug)]
struct RecordAt {
    at: FieldAt,
    bytes: Vec<u8>,
    /// Offset of the block info in the record.
    info: usize,
    resources_at: FieldAt,
    resources: Vec<u8>,
}

/// A render model's sections and box, shared by its open sections.
#[derive(Debug)]
struct ModelSource {
    tag: TagId,
    /// Each section, `None` when its block is not in this map.
    sections: Vec<Option<SectionSource>>,
    /// Each local section's record.
    records: Vec<Option<RecordAt>>,
    compression: Option<Compression>,
    /// The position bounds' 24 bytes (`ModeCompression +0`).
    box_at: Option<FieldAt>,
    /// A sky tag draws this model (its parts as strips).
    sky: bool,
    /// Every permutation's record and LOD section indices (-1 = none).
    lods: Vec<(FieldAt, [i16; 6])>,
}

enum Source {
    Structure {
        section: SectionSource,
        /// A cluster's bounds (`+0x54`) and where they are.
        bounds: Option<([f32; 6], FieldAt)>,
        /// An instanced definition's placements.
        instances: Vec<(u32, InstancedGeometryInstance)>,
        /// The cluster's or definition's record.
        record: RecordAt,
        /// Why a lightmap bucket reaches the section, if one does.
        bucket_lit: Option<String>,
    },
    Model {
        model: Arc<ModelSource>,
        section: usize,
        local: bool,
    },
    /// A `coll` tag's BSP `index` (its blocks and slots as read).
    Coll { source: Arc<CollSource>, index: usize },
}

/// One open target: its source and the mesh as imported.
struct Opened {
    target: Target,
    name: String,
    source: Source,
    original: Arc<Mesh>,
    /// Why the map cannot take new raw data, if it cannot
    /// (`map::Map::raw_append_refusal`).
    no_append: Option<String>,
}

/// The undone and redone state: every edited target's mesh and op log,
/// and the model sections used for their permutations' lower LODs. A
/// target without an entry is as imported.
#[derive(Clone, Default)]
struct State {
    edits: Vec<(Target, Arc<Mesh>, Arc<Vec<MeshOp>>)>,
    lower_lods: Vec<Target>,
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
    diff_writes(at, &floats(old), &floats(new), out);
}

/// The patches of a byte range at `at`: the runs where `new` differs from
/// `old`.
fn diff_writes(at: FieldAt, old: &[u8], new: &[u8], out: &mut Vec<RegionWrite>) {
    for (offset, bytes) in changed_runs(old, new) {
        let was = old[offset..offset + bytes.len()].to_vec();
        out.push(RegionWrite {
            space: at.space,
            address: at.address + offset as u32,
            bytes,
            was,
        });
    }
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

/// `len` bytes at `address` of `space`.
fn read_at<R: Read + Seek>(
    map: &mut Map<R>,
    space: AddressSpace,
    address: u32,
    len: usize,
) -> Result<Vec<u8>, Error> {
    let at = space.translate(address, len as u32)?;
    Ok(map.read_file_range(at, len as u64)?)
}

/// The record of `len` bytes at `address`, its block info at `info`.
fn record_at<R: Read + Seek>(
    map: &mut Map<R>,
    space: AddressSpace,
    address: u32,
    len: usize,
    info: usize,
) -> Result<RecordAt, Error> {
    let bytes = read_at(map, space, address, len)?;
    let count = u32_at(&bytes, info + 0x10) as usize;
    let table = u32_at(&bytes, info + 0x14);
    let resources = if count == 0 {
        Vec::new()
    } else {
        read_at(map, space, table, count * tags::geometry::BlockResource::SIZE)?
    };
    Ok(RecordAt {
        at: field_at(map, space, address),
        bytes,
        info,
        resources_at: field_at(map, space, table),
        resources,
    })
}

/// Why a lightmap bucket reaches structure target `t`, if one does: a
/// bucket-lit section keeps its vertex count and order
/// (docs/editor/modeling-targets.md, "Structure BSP render geometry"). A
/// definition is reached through its placements' instance references; a
/// cluster's reference is chosen by a render-key field this reader does
/// not map, so any cluster reference with an offset reaches every cluster
/// (**Design**).
fn bucket_lit<R: Read + Seek>(
    map: &mut Map<R>,
    t: Target,
    render: &StructureRender,
) -> Option<String> {
    let lightmap = (|| -> Option<TagId> {
        let scenario = map.scenario()?.clone();
        let location = scenario.location?;
        let bytes = map.read_tag_bytes(scenario.id).ok()?;
        let view = TagView::new(&bytes, location.address);
        let entries = view.block(view.at(0, 0x218).ok()?, 0x210, 0x44).ok()?;
        entries
            .into_iter()
            .find(|e| TagId(u32_at(e, 0x14)) == t.tag)
            .map(|e| TagId(u32_at(e, 0x1c)))
            .filter(|&id| id != TagId::NULL)
    })()?;
    let Ok(lightmap) = Lightmap::read(map, lightmap) else {
        return Some("its lightmap does not read, so its bucket lighting is unknown".into());
    };
    let group = lightmap
        .groups
        .iter()
        .find(|g| g.structure_checksum == render.checksum)?;
    let flagged = |bucket: u16| {
        group
            .buckets
            .get(usize::from(bucket))
            .is_some_and(|b| b.flags & BUCKET_FLAG_PATCHED != 0)
    };
    let reached = |r: &tags::structure::BucketReference| {
        flagged(r.bucket) && r.section_offsets.iter().any(Option::is_some)
    };
    let lit = match t.kind {
        TargetKind::Cluster => group.cluster_bucket_refs.iter().any(reached),
        _ => render.instances.iter().enumerate().any(|(k, p)| {
            p.definition as u32 == t.index && group.instance_bucket_refs.get(k).is_some_and(reached)
        }),
    };
    lit.then(|| {
        "a lightmap bucket lights its vertices by their order, so its vertex count and order stay"
            .into()
    })
}

/// The raw patches of `owner`'s section, each with the source bytes it
/// replaces.
fn raw_writes(
    owner: Target,
    src: &SectionSource,
    runs: Vec<(usize, Vec<u8>)>,
    out: &mut Vec<RawWrite>,
) {
    for (offset, bytes) in runs {
        let was = src.bytes[offset..offset + bytes.len()].to_vec();
        out.push(RawWrite {
            owner,
            block: src.block,
            block_size: src.block_size,
            offset: offset as u32,
            bytes,
            was,
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
        // A collision BSP is a mesh built face by face: nothing welded.
        let (welded, passthrough) = o.original.source().map_or((0, 0), |source| {
            (
                source.positions.len().saturating_sub(o.original.vert_count()),
                source.triangles.len().saturating_sub(o.original.face_count()),
            )
        });
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
            Some(Source::Coll { .. }) => true,
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
        self.state.get(t).is_some() || self.state.lower_lods.contains(&t)
    }

    /// **Use for lower LODs**: point every lower level of detail of each
    /// permutation that draws model section `t` at `t` (`on`), or leave
    /// them as they were (`!on`). One undo step; saved as fixed patches of
    /// the permutations' LOD indices (docs/editor/modeling-targets.md,
    /// "Render models"). Returns whether anything changed.
    pub fn set_use_for_lower_lods(&mut self, t: Target, on: bool) -> Result<bool, Error> {
        if t.kind != TargetKind::ModeSection {
            return Err(Error::Refused {
                target: t.id(),
                reason: "only a model section has levels of detail".into(),
            });
        }
        if !self.is_open(t) {
            return Err(Error::NotOpen(t.id()));
        }
        if self.state.lower_lods.contains(&t) == on {
            return Ok(false);
        }
        let before = self.state.clone();
        self.undo.record(before);
        if on {
            self.state.lower_lods.push(t);
        } else {
            self.state.lower_lods.retain(|&x| x != t);
        }
        self.rev += 1;
        Ok(true)
    }

    /// Whether model section `t` is used for its permutations' lower LODs.
    pub fn uses_for_lower_lods(&self, t: Target) -> bool {
        self.state.lower_lods.contains(&t)
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
                let bytes = map.read_tag_bytes(target.tag)?;
                let head = TagView::new(&bytes, location.address).at(0, 0x140)?.to_vec();
                let (block_at, stride) = match target.kind {
                    TargetKind::Cluster => (0xa0, tags::structure::Cluster::SIZE),
                    _ => (
                        tags::structure::SBSP_INSTANCE_DEFINITIONS + 4,
                        InstancedGeometryDefinition::SIZE,
                    ),
                };
                let address = u32_at(&head, block_at) + (i * stride) as u32;
                let record = record_at(map, location.space, address, stride, STRUCTURE_BLOCK_INFO)?;
                let bounds = (target.kind == TargetKind::Cluster).then(|| {
                    let at = field_at(map, location.space, address + 0x54);
                    (render.clusters[i].bounds, at)
                });
                let bucket_lit = bucket_lit(map, target, &render);
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
                        record,
                        bucket_lit,
                    },
                    original: Arc::new(mesh),
                    no_append: map.raw_append_refusal().map(str::to_owned),
                })
            }
            TargetKind::Coll => {
                let source = Arc::new(CollSource::read(map, target.tag)?);
                let mesh = source.mesh(i)?;
                Ok(Opened {
                    target,
                    name,
                    source: Source::Coll { source, index: i },
                    original: Arc::new(mesh),
                    no_append: map.raw_append_refusal().map(str::to_owned),
                })
            }
            TargetKind::ModeSection => {
                let geometry = RenderModelGeometry::read(map, target.tag, raw)?;
                let model = match self.model(target.tag) {
                    Some(m) => m,
                    None => Arc::new(model_source(map, &geometry, location.space)?),
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
                    no_append: map.raw_append_refusal().map(str::to_owned),
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

    /// Replace an open collision BSP with polygons from an open render
    /// section's current mesh. The caller chooses sections in the same space.
    /// An empty material map assigns every triangle to collision material 0.
    pub fn collision_from_render(
        &mut self,
        render: Target,
        target: Target,
        materials: &[u16],
    ) -> Result<OpResult, Error> {
        let refused = |reason| Error::Refused {
            target: target.id(),
            reason,
        };
        if render.kind == TargetKind::Coll || target.kind != TargetKind::Coll {
            return Err(refused(
                "From render needs a render section and a collision BSP".into(),
            ));
        }
        if !self.is_open(target) {
            return Err(Error::NotOpen(target.id()));
        }
        let mesh = self.mesh(render).ok_or_else(|| Error::NotOpen(render.id()))?;
        let export = mesh.export();
        let mapped = if materials.is_empty() {
            Vec::new()
        } else {
            export
                .materials
                .iter()
                .map(|&m| {
                    materials.get(usize::from(m)).copied().ok_or_else(|| {
                        refused(format!("render material {m} has no collision material mapping"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        let polygons = from_render(
            &export.positions,
            &export.triangles,
            &mapped,
            &FromRender::default(),
        );
        if polygons.is_empty() {
            return Err(refused("the render section has no usable triangles".into()));
        }
        let faces = polygons
            .into_iter()
            .map(|p| PolygonFace {
                points: p.points,
                material: p.material,
                group: coll::group_of(p.flags, p.breakable_surface),
            })
            .collect();
        self.apply(target, &[MeshOp::ReplacePolygons { faces }], None)
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
        if ops.iter().any(|op| matches!(op, MeshOp::ReplacePolygons { .. })) {
            let opened = self.find(t).expect("open target");
            let Source::Coll { source, index } = &opened.source else {
                return Err(Error::Refused {
                    target: t.id(),
                    reason: "polygon replacement requires a collision BSP".into(),
                });
            };
            source
                .writes(*index, &coll::polygons_from_mesh(&mesh))
                .map_err(|reason| Error::Refused {
                    target: t.id(),
                    reason,
                })?;
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
        if let Source::Coll { .. } = &o.source {
            // Compiled again whole, so any change to a polygon is a write.
            let changed = collision_build::coll::polygons_from_mesh(mesh)
                != collision_build::coll::polygons_from_mesh(&o.original);
            let class = if changed {
                WriteClass::GrownBlocks
            } else {
                WriteClass::Unchanged
            };
            return (class, None, export);
        }
        let source = o.original.source().expect("imported");
        if export.positions.len() != source.positions.len()
            || export.triangles != source.triangles
            || export.materials != source.materials
        {
            let refused = if !self.is_local(o.target) {
                Some("its raw block is in a shared cache, which the editor never writes".to_owned())
            } else {
                match &o.source {
                    Source::Structure { bucket_lit, .. } => bucket_lit.clone(),
                    Source::Model { .. } | Source::Coll { .. } => None,
                }
            };
            return (WriteClass::Grown, refused, export);
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
            if refused.is_none() && class != WriteClass::Unchanged {
                refused = match &o.source {
                    Source::Structure { .. } => self.target_patches(o, &export, class).err(),
                    Source::Model { model, .. } => self
                        .model_patches(model, &self.edited_sections(model), &mut Patches::default())
                        .err()
                        .map(|e| e.to_string()),
                    Source::Coll { source, index } => source
                        .writes(*index, &collision_build::coll::polygons_from_mesh(mesh))
                        .err(),
                };
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
                moved: if matches!(o.source, Source::Coll { .. }) {
                    0
                } else {
                    export
                        .corners
                        .iter()
                        .filter(|c| c.dirty & dirty::POSITION != 0)
                        .count()
                },
                refused,
                ops: self.ops(o.target).len(),
            });
        }
        out
    }

    /// Whether any open target has an edit a save would write or refuse.
    pub fn has_changes(&self) -> bool {
        self.opened.iter().any(|o| {
            (self.state.get(o.target).is_some() && self.classify(o).0 != WriteClass::Unchanged)
                || self.state.lower_lods.contains(&o.target)
        })
    }

    /// The patches of one structure target (a model section's are the
    /// model's, in [`Self::model_patches`]).
    fn target_patches(
        &self,
        o: &Opened,
        export: &Export,
        class: WriteClass,
    ) -> Result<Patches, String> {
        let mut p = Patches::default();
        match &o.source {
            Source::Structure {
                section,
                bounds,
                record,
                ..
            } => {
                if class == WriteClass::Grown {
                    let g = grow::grow(
                        section,
                        export,
                        &grow::Options {
                            compression: None,
                            requantise: false,
                            sky: false,
                            visibility: true,
                        },
                    )?;
                    grown_writes(
                        o.target,
                        o.no_append.as_deref(),
                        section,
                        record,
                        &g,
                        true,
                        &mut p,
                    )?;
                } else {
                    let data = rewrite(
                        section,
                        &export.positions,
                        Some(&export.corners),
                        None,
                        false,
                    )?;
                    raw_writes(o.target, section, block_runs(section, &data)?, &mut p.raw);
                }
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
                    // Bounds that only hold with a new block go with it.
                    match p.appended.last_mut() {
                        Some(a) => field_writes(*at, old, &new, &mut a.writes),
                        None => field_writes(*at, old, &new, &mut p.tags),
                    }
                }
            }
            Source::Model { .. } => {
                return Err("a model section is written with its model".into());
            }
            Source::Coll { .. } => {
                return Err("a collision BSP is compiled whole, not patched".into());
            }
        }
        Ok(p)
    }

    /// The edited sections of a model, with their exports and classes.
    fn edited_sections(&self, model: &Arc<ModelSource>) -> Vec<(usize, Export, WriteClass)> {
        self.opened
            .iter()
            .filter(|o| self.state.get(o.target).is_some())
            .filter_map(|o| match &o.source {
                Source::Model {
                    model: m, section, ..
                } if Arc::ptr_eq(m, model) => {
                    let (class, _, export) = self.classify(o);
                    (class != WriteClass::Unchanged).then_some((*section, export, class))
                }
                _ => None,
            })
            .collect()
    }

    /// One model's patches: each edited section in place or grown. The
    /// compression box grows to hold the edited sections' vertices, and
    /// then every section of the model is requantised into it (each at its
    /// own size, or grown).
    fn model_patches(
        &self,
        model: &Arc<ModelSource>,
        edited: &[(usize, Export, WriteClass)],
        out: &mut Patches,
    ) -> Result<(), Error> {
        let target = |s: usize| Target::new(TargetKind::ModeSection, model.tag, s as u32);
        let refuse = |s: usize, reason: String| Error::Refused {
            target: target(s).id(),
            reason,
        };
        let no_append = self.opened.iter().find_map(|o| match &o.source {
            Source::Model { model: m, .. } if Arc::ptr_eq(m, model) => Some(o.no_append.clone()),
            _ => None,
        });
        let no_append = no_append.flatten();
        let old = model.compression.unwrap_or_default();
        let mut grown = old.position;
        for (_, e, _) in edited {
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
        let regrown = grown != old.position;
        if regrown && model.box_at.is_none() {
            return Err(refuse(
                edited[0].0,
                "the model has no compression box to grow".into(),
            ));
        }
        let boxed = Compression {
            position: grown,
            texcoord: old.texcoord,
        };
        let compression = if regrown {
            Some(&boxed)
        } else {
            model.compression.as_ref()
        };
        for (s, src) in model.sections.iter().enumerate() {
            let edit = edited.iter().find(|(k, ..)| *k == s);
            if edit.is_none() && !regrown {
                continue;
            }
            let src = src.as_ref().ok_or_else(|| {
                let why = if edit.is_some() {
                    "its raw block is in a shared cache, which the editor never writes"
                } else {
                    "growing the model's compression box rewrites every section, and this one is in a shared cache"
                };
                refuse(s, why.into())
            })?;
            match edit {
                Some((_, e, WriteClass::Grown)) => {
                    let record = model.records[s]
                        .as_ref()
                        .ok_or_else(|| refuse(s, "its section record was not read".into()))?;
                    let g = grow::grow(
                        src,
                        e,
                        &grow::Options {
                            compression,
                            requantise: regrown,
                            sky: model.sky,
                            visibility: false,
                        },
                    )
                    .map_err(|r| refuse(s, r))?;
                    grown_writes(target(s), no_append.as_deref(), src, record, &g, false, out)
                        .map_err(|r| refuse(s, r))?;
                }
                Some((_, e, _)) => {
                    let data = rewrite(src, &e.positions, Some(&e.corners), compression, regrown)
                        .map_err(|r| refuse(s, r))?;
                    raw_writes(
                        target(s),
                        src,
                        block_runs(src, &data).map_err(|r| refuse(s, r))?,
                        &mut out.raw,
                    );
                }
                None => {
                    let data = rewrite(src, &src.positions, None, compression, true)
                        .map_err(|r| refuse(s, r))?;
                    raw_writes(
                        target(s),
                        src,
                        block_runs(src, &data).map_err(|r| refuse(s, r))?,
                        &mut out.raw,
                    );
                }
            }
        }
        if regrown && let Some(at) = model.box_at {
            field_writes(at, &old.position, &grown, &mut out.tags);
        }
        Ok(())
    }

    /// The LOD index patches of the model sections used for lower LODs:
    /// in each permutation that draws section `s` at level `k`, every
    /// present level below `k` is pointed at `s`.
    fn lod_patches(&self, out: &mut Patches) {
        for &t in &self.state.lower_lods {
            let Some(Source::Model { model, section, .. }) = self.find(t).map(|o| &o.source)
            else {
                continue;
            };
            let s = *section as i16;
            for (at, lods) in &model.lods {
                let Some(k) = lods.iter().rposition(|&x| x == s) else {
                    continue;
                };
                let mut new = *lods;
                for slot in new.iter_mut().take(k) {
                    if *slot != -1 {
                        *slot = s;
                    }
                }
                let bytes = |l: &[i16; 6]| -> Vec<u8> { l.iter().flat_map(|v| v.to_le_bytes()).collect() };
                let field = FieldAt {
                    space: at.space,
                    address: at.address + 4,
                };
                diff_writes(field, &bytes(lods), &bytes(&new), &mut out.tags);
            }
        }
    }

    /// What a save writes: every open target's patches, in place or
    /// grown. A model's compression box grows to hold its edited sections'
    /// vertices, and then every section of the model is requantised into
    /// it ([`Self::model_patches`]); a section used for lower LODs points
    /// its permutations' lower levels at itself. Refused whole, naming the
    /// target, when an edit cannot be written.
    pub fn changes(&self) -> Result<Patches, Error> {
        let mut out = Patches::default();
        let refuse = |t: Target, reason: String| Error::Refused {
            target: t.id(),
            reason,
        };
        let mut models: Vec<Arc<ModelSource>> = Vec::new();
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
                        .target_patches(o, &export, class)
                        .map_err(|r| refuse(o.target, r))?;
                    out.tags.extend(p.tags);
                    out.raw.extend(p.raw);
                    out.appended.extend(p.appended);
                }
                Source::Coll { source, index } => {
                    let mesh = self.mesh(o.target).expect("open");
                    let w = source
                        .writes(*index, &collision_build::coll::polygons_from_mesh(mesh))
                        .map_err(|r| refuse(o.target, r))?;
                    out.tags.extend(w.patches);
                    out.grown.extend(w.grown);
                }
                Source::Model { model, .. } => {
                    if !models.iter().any(|m| Arc::ptr_eq(m, model)) {
                        models.push(model.clone());
                    }
                }
            }
        }
        for model in &models {
            self.model_patches(model, &self.edited_sections(model), &mut out)?;
        }
        self.lod_patches(&mut out);
        Ok(out)
    }

    /// The positions a native preview draws for an open target, per
    /// source vertex in section space, when its edit is in place (a
    /// grown edit has no source vertex order to update).
    pub fn preview_positions(&self, t: Target) -> Option<Vec<[f32; 3]>> {
        let o = self.find(t)?;
        let (class, _, export) = self.classify(o);
        (!matches!(class, WriteClass::Grown | WriteClass::GrownBlocks)).then_some(export.positions)
    }

    /// The native previews of the open structure targets: a cluster
    /// draws as its own section, a definition as each of its placements.
    /// A render model's section has none (the native world does not draw
    /// it), nor has a grown edit.
    pub fn previews(&self) -> Vec<Preview> {
        self.opened
            .iter()
            .filter_map(|o| {
                let t = o.target;
                let drawn = match t.kind {
                    TargetKind::Cluster => vec![Drawn::Cluster(t.index)],
                    TargetKind::InstanceDef => self
                        .instances(t)
                        .iter()
                        .map(|(i, _)| Drawn::Instance(*i))
                        .collect(),
                    TargetKind::ModeSection | TargetKind::Coll => return None,
                };
                Some(Preview {
                    bsp: t.tag,
                    drawn,
                    positions: self.preview_positions(t)?,
                })
            })
            .collect()
    }

    /// The write class of an open target.
    pub fn class(&self, t: Target) -> Option<WriteClass> {
        self.find(t).map(|o| self.classify(o).0)
    }
}

/// A grown section's writes: over its old block when the encoding still
/// fits it (fixed patches), else a [`RawAppend`] with the owner record's
/// counts and block info sizes and the resource entries. Structure records
/// take the section info counts ([`Grown::structure_info`]); model section
/// records their vertex, triangle and part counts (`+4`, `+6`, `+8`).
fn grown_writes(
    target: Target,
    no_append: Option<&str>,
    src: &SectionSource,
    record: &RecordAt,
    g: &Grown,
    structure: bool,
    p: &mut Patches,
) -> Result<(), String> {
    let mut rec = record.bytes.clone();
    if structure {
        for (at, v) in g.structure_info() {
            put16(&mut rec, at, v);
        }
    } else {
        put16(&mut rec, 4, g.vertices as u16);
        put16(&mut rec, 6, (g.indices / 3) as u16);
        put16(&mut rec, 8, g.parts.len() as u16);
    }
    if g.fits(src) {
        let runs = changed_runs(&src.bytes, &g.encoded.block);
        raw_writes(target, src, runs, &mut p.raw);
        diff_writes(record.at, &record.bytes, &rec, &mut p.tags);
        return Ok(());
    }
    if let Some(why) = no_append {
        return Err(format!(
            "a grown write needs new raw data, and this map cannot take it: {why}"
        ));
    }
    let info = record.info;
    let block_size =
        u32::try_from(g.encoded.block.len()).map_err(|_| "the section's block is too large")?;
    put32(&mut rec, info + 4, block_size);
    put32(&mut rec, info + 8, g.encoded.section_size);
    put32(&mut rec, info + 0xc, g.encoded.resource_size);
    let entries: Vec<u8> = g.encoded.resources.iter().flat_map(|r| r.encode()).collect();
    if entries.len() != record.resources.len() {
        return Err("the section's resource table changed length".into());
    }
    let mut writes = Vec::new();
    diff_writes(record.at, &record.bytes, &rec, &mut writes);
    diff_writes(record.resources_at, &record.resources, &entries, &mut writes);
    p.appended.push(RawAppend {
        target,
        bytes: g.encoded.block.clone(),
        align: RAW_ALIGN,
        pointer_space: record.at.space,
        pointer_address: record.at.address + info as u32,
        old: src.block,
        old_size: src.block_size,
        writes,
    });
    Ok(())
}

/// Whether a sky tag of the map draws model `id`.
fn drawn_by_a_sky<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> bool {
    let skies: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"sky ") && t.location.is_some())
        .map(|t| t.id)
        .collect();
    skies.into_iter().any(|sky| {
        map.read_tag_bytes(sky).ok().is_some_and(|b| {
            b.get(SKY_MODEL_DATUM..SKY_MODEL_DATUM + 4)
                .is_some_and(|w| TagId(u32_at(w, 0)) == id)
        })
    })
}

/// A render model's sections, records, compression box, sky use and
/// permutations' levels of detail.
fn model_source<R: Read + Seek>(
    map: &mut Map<R>,
    m: &RenderModelGeometry,
    space: AddressSpace,
) -> Result<ModelSource, Error> {
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
    let records = m
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if s.raw.file != ResourceFile::Local {
                return Ok(None);
            }
            let address = m.header.sections.address + (i * MODEL_SECTION_RECORD) as u32;
            record_at(map, space, address, MODEL_SECTION_RECORD, MODEL_BLOCK_INFO).map(Some)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let mut lods = Vec::new();
    for r in &m.regions {
        for (j, p) in r.permutations.iter().enumerate() {
            let address = r.wire.permutations.address + (j * PERMUTATION_RECORD) as u32;
            let indices = p.lod_sections.map(|s| s.map_or(-1, |s| s as i16));
            lods.push((field_at(map, space, address), indices));
        }
    }
    Ok(ModelSource {
        tag: m.id,
        sections,
        records,
        compression,
        box_at,
        sky: drawn_by_a_sky(map, m.id),
        lods,
    })
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
