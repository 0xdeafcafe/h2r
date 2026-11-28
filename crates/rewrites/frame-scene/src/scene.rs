//! The scene: flat arrays and index ranges (docs/graphics/framescene.md,
//! section 4.2), so a scene is a handful of allocations that are recycled
//! ([`crate::FreeList`]) and can be walked without chasing pointers.
//!
//! Slice 1 fills the views, their cameras, clusters, object instances,
//! parts and palette rows; slice 3 the lights and their passes' steps,
//! the world layers' and the first person's items; slice 5 the fog, the
//! water and the post passes (`crate::late`).
//! Shadow maps, the carried part and texture uploads join in the slices
//! that type or carry them (3 and 6).

use std::ops::Range;
use std::time::Instant;

use serde::Serialize;

pub use frame_types::keys::{InstanceKey, ViewKey, ViewKind};

use crate::camera::Camera;
use crate::decal::{DecalItem, DecalVertex};
use crate::decorator::{DecoratorDraw, DecoratorInstance};
use crate::effect::Effects;
use crate::late::{FogState, LateMaterial, LatePart, PostEffect, PostParams, WaterState};
use crate::layer::{LayerItem, LayerPass};
use crate::light::{Light, LightStep};
use crate::light_group::{LightEntry, LightGroup, ObjectShadow};
use crate::node::NodeMatrix;
use crate::object::{Defaults, ItemKey, Lighting};
use crate::overlay::Overlay;

/// One vertex constant register: a palette row is three of them, world
/// space.
pub type Row = [f32; 4];

/// Which frame a scene is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct SceneId {
    /// The native map work's generation: a new map, BSP or load starts
    /// another.
    pub map_epoch: u64,
    /// The presents before this frame's.
    pub frame: u64,
    /// The game tick after this frame's ticks.
    pub tick: u32,
}

/// When a scene was published, and how many ticks ran before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Stamp {
    /// On the sim thread, at the publish.
    #[serde(skip)]
    pub published: Option<Instant>,
    /// 0 (paused, loading), 1 or 2.
    pub ticks_run: u8,
}

/// Why a scene does not follow on from the one before (a bitset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Cut(pub u8);

impl Cut {
    pub const NONE: Cut = Cut(0);
    pub const MAP: Cut = Cut(1);
    pub const BSP: Cut = Cut(1 << 1);
    pub const REVERT: Cut = Cut(1 << 2);
    pub const LOAD: Cut = Cut(1 << 3);
    pub const VIEWS: Cut = Cut(1 << 4);
    const NAMES: [(Cut, &'static str); 5] =
        [(Cut::MAP, "map"), (Cut::BSP, "bsp"), (Cut::REVERT, "revert"), (Cut::LOAD, "load"), (Cut::VIEWS, "views")];

    pub fn contains(self, other: Cut) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The reasons set, by name.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES.iter().filter(|(c, _)| !c.is_empty() && self.contains(*c)).map(|(_, n)| *n).collect()
    }
}

impl std::ops::BitOr for Cut {
    type Output = Cut;
    fn bitor(self, rhs: Cut) -> Cut {
        Cut(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Cut {
    fn bitor_assign(&mut self, rhs: Cut) {
        self.0 |= rhs.0;
    }
}

/// A view's world state the typed world stages read, by value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct WorldState {
    /// Whether the texture accumulation target was there to bind when the
    /// view was extracted (`scene_extract::layout::object_nodes::texaccum_bound`).
    pub texaccum: bool,
}

/// What the auxiliary state toggle `0x00012d50` sets for the view
/// (docs/graphics/framescene.md, slice 3): the view-projection rows it
/// uploads to `c[0..3]` and the program clip range (W-buffer units) it
/// stores, on, as a part with [`PartFlags::AUX_DEPTH`] draws, and off, as
/// the part's epilogue leaves the device. On is off times [`Self::scale`],
/// the rows whole and the clip's near end.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct AuxDepth {
    pub scale: f32,
    pub rows: [Row; 4],
    pub clip: [f32; 2],
    pub rest_rows: [Row; 4],
    pub rest_clip: [f32; 2],
}

/// What the geometry draw's prologue (`0x0004d640`) uploads for a shader
/// with flag bit 0 while the active structure has a parameter block
/// (`0x0003c650`, `0x0003d000`; `engine::render_loop::structure_parameters`):
/// the bump environment matrices of stages 1 to 3, `c[21..25]`, the fog
/// colour, and the engine words `0x0003c650` stores at `0x004c1b9c`. All
/// as words, as the device and the engine hold them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct StructureParameters {
    /// Per stage 1, 2 and 3, the texture-stage states 22 to 25
    /// (`BUMPENVMAT00`, `01`, `11`, `10`).
    pub bump_env: [[u32; 4]; 3],
    /// `c[21..25]` (hardware numbering).
    pub constants: [[u32; 4]; 5],
    /// `D3DRS_FOGCOLOR`.
    pub fog_colour: u32,
    /// The words at `0x004c1b9c`.
    pub scratch: [u32; 13],
}

impl StructureParameters {
    /// The first constant the upload sets.
    pub const FIRST_CONSTANT: usize = 21;

    /// Stage `stage`'s (1 to 3) bump matrix as the pixel shader takes it:
    /// `00`, `01`, `10`, `11`.
    pub fn bump_matrix(&self, stage: usize) -> Option<[f32; 4]> {
        let [s00, s01, s11, s10] = *self.bump_env.get(stage.checked_sub(1)?)?;
        Some([s00, s01, s10, s11].map(f32::from_bits))
    }
}

/// Geometry admitted by the display lens outside the retail view. These
/// IDs belong to this BSP and this frame's object table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DisplayVisibility {
    /// Clusters wholly enclosed by one display portal window. Every subpart
    /// of these sections is in that window.
    pub whole_clusters: Vec<u16>,
    pub placements: Vec<u16>,
    pub clusters: Vec<u16>,
    pub objects: Vec<u32>,
    /// The objects are a widened guest camera's, outside the retail
    /// lens's set: their guest setup left them undrawn, and the display
    /// draws them at any device factor.
    pub guest_lens: bool,
}

/// One game view.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct View {
    pub key: ViewKey,
    /// Nothing of this view's camera can be blended from the scene
    /// before: no view of the same key there (slice 8 adds scripted cuts,
    /// respawns and the distance backstop).
    pub cut: bool,
    pub camera: Camera,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_visibility: Option<std::sync::Arc<DisplayVisibility>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_transparents: Option<std::sync::Arc<crate::transparent::DisplayTransparentQueue>>,
    /// Into [`FrameScene::clusters`].
    pub clusters: Range<u32>,
    /// Into [`FrameScene::instances`], in the guest's order.
    pub instances: Range<u32>,
    /// Into [`FrameScene::order`]: this view's parts in the guest's list
    /// order (by [`PartDraw::slot`]).
    pub order: Range<u32>,
    pub world: WorldState,
    /// Object items extraction refused: the guest still draws them.
    pub remainder: u32,
    /// What its [`PartFlags::AUX_DEPTH`] parts draw with; read when the
    /// view has one.
    pub aux_depth: Option<AuxDepth>,
    /// What its [`PartFlags::STRUCTURE_PARAMETERS`] parts' prologue
    /// uploads; read when the view has one.
    pub structure_parameters: Option<StructureParameters>,
    /// Into [`FrameScene::lights`], with guest selections in draw order.
    pub lights: Range<u32>,
    /// Into [`FrameScene::layer_passes`], in the guest's draw order.
    pub layers: Range<u32>,
    /// Into [`FrameScene::decorators`], in the list's order.
    pub decorators: Range<u32>,
    /// Decorator draws extraction refused: the guest still draws them.
    pub decorator_remainder: u32,
    /// Into [`FrameScene::decals`], in the guest's order.
    pub decals: Range<u32>,
    /// Decal records extraction refused: named drops, never a D3D draw.
    pub decal_remainder: u32,
    /// The view's effects in the guest's order: weather plates today.
    pub effects: Effects,
    /// The view's layer 16 (slice 5).
    pub fog: Option<FogState>,
    /// The view's layer 6.
    pub water: Option<WaterState>,
    /// The teardown's indexed quad mesh in the current vertex stream.
    pub water_composite: Option<crate::late::WaterComposite>,
    /// The water end's ocean.
    pub ocean: Option<crate::late::Ocean>,
    /// The view's post passes.
    pub post: Option<PostParams>,
}

/// One object drawn in a view.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Instance {
    pub key: InstanceKey,
    /// The object definition's tag datum.
    pub definition: u32,
    /// The render model's tag datum (`ModelId`).
    pub model: u32,
    /// Into [`FrameScene::nodes`]: world-space node matrices, model node
    /// order.
    pub nodes: Range<u32>,
    /// World bounding sphere: centre, radius.
    pub sphere: ([f32; 3], f32),
    pub lighting: Option<Lighting>,
    pub defaults: Option<Defaults>,
    /// Into [`FrameScene::parts`].
    pub parts: Range<u32>,
    /// Set by extraction when the engine says so (slice 8).
    pub teleported: bool,
}

/// How a part draws beyond its material: what its render record's `+0`
/// flags make the guest's geometry draw (`0x0004d0b0`) do around and in
/// its draw call (docs/graphics/framescene.md, slice 3). A bitset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct PartFlags(pub u8);

impl PartFlags {
    pub const NONE: PartFlags = PartFlags(0);
    /// Record bit 14 in layers 1 and 3: the draw's prologue (`0x0004d640`)
    /// sets stencil mode 2 (the structure mode with reference `0xff`) and
    /// its epilogue (`0x0004d720`) leaves mode 1, unless the layer draws
    /// with the stencil off.
    pub const STENCIL_MARK: PartFlags = PartFlags(1);
    /// Record bit 9 on a part that is not a triangle list: the emitter
    /// (`0x0004daa0`) draws its indices as a strip. Without the bit it
    /// draws every part as a list.
    pub const STRIP: PartFlags = PartFlags(1 << 1);
    /// Record bit 6 with a visible-subpart mask, in a section with
    /// subparts: the emitter draws only the visible subparts
    /// ([`PartDraw::subparts`]), each its own strip with strip topology,
    /// consecutive ones as one list otherwise.
    pub const SUBPARTS: PartFlags = PartFlags(1 << 2);
    /// Record bit 11: the draw's prologue and epilogue call the auxiliary
    /// state toggle `0x00012d50`, and the part draws with its view's
    /// [`AuxDepth`] (its view-projection rows and clip range scaled).
    pub const AUX_DEPTH: PartFlags = PartFlags(1 << 3);
    /// The record's fade byte (`+0x1a`) is below `0xff`: the prologue
    /// (`0x0004d640`) sets the polygon stipple pattern the byte selects
    /// and the epilogue (`0x0004d720`) the solid one, each only when the
    /// engine's cached fade (`0x004b8494`) differs. The pattern masks the
    /// draw only while the device's stipple is on; the host takes the part
    /// only with the stipple off and the solid pattern and a cached one
    /// held, so drawing it unmasked and skipping its handler leave what
    /// the guest's would.
    pub const FADE: PartFlags = PartFlags(1 << 4);
    /// The shader's flag bit 0 (`+0x16`), in the case the scene draws:
    /// the draw neither binds a lightmap (layer 3) nor adds a PRT stream
    /// (layer 5), and its prologue uploads nothing unless the part also
    /// has [`PartFlags::STRUCTURE_PARAMETERS`].
    pub const SHADER_FLAG: PartFlags = PartFlags(1 << 5);
    /// With [`PartFlags::SHADER_FLAG`]: the active structure has a
    /// parameter block, so the prologue uploads its view's
    /// [`StructureParameters`]. The part draws with them (`c[21..25]` and
    /// the bump matrices); the host takes it only where the device and
    /// the engine already hold them, so skipping its handler leaves what
    /// the upload would.
    pub const STRUCTURE_PARAMETERS: PartFlags = PartFlags(1 << 6);
    /// With [`PartFlags::SHADER_FLAG`], outside layer 3's walk: the
    /// frame's structure target copy is not made at the layer call
    /// (`0x00509430` clear). The draw of a pass whose program has flag bit
    /// 20 then calls `0x0003bea0`, which clears the shader cache and, with
    /// a structure parameter block, copies and binds the structure's
    /// target, and sets the flag; the world layers' host draws such an
    /// item ahead of every kept item, making the flag and the copy; a part
    /// whose passes' programs lack the bit makes no call and draws.
    pub const TARGET_UNCOPIED: PartFlags = PartFlags(1 << 7);
    const NAMES: [(PartFlags, &'static str); 8] = [
        (PartFlags::STENCIL_MARK, "stencil_mark"),
        (PartFlags::STRIP, "strip"),
        (PartFlags::SUBPARTS, "subparts"),
        (PartFlags::AUX_DEPTH, "aux_depth"),
        (PartFlags::FADE, "fade"),
        (PartFlags::SHADER_FLAG, "shader_flag"),
        (PartFlags::STRUCTURE_PARAMETERS, "structure_parameters"),
        (PartFlags::TARGET_UNCOPIED, "target_uncopied"),
    ];

    pub fn contains(self, other: PartFlags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The flags set, by name (`render.scene`'s `part_flags`).
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES.iter().filter(|(f, _)| self.contains(*f)).map(|(_, n)| *n).collect()
    }
}

impl std::ops::BitOr for PartFlags {
    type Output = PartFlags;
    fn bitor(self, rhs: PartFlags) -> PartFlags {
        PartFlags(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for PartFlags {
    fn bitor_assign(&mut self, rhs: PartFlags) {
        self.0 |= rhs.0;
    }
}

/// Which of a part's subparts the guest's emitter (`0x0004daa0`) draws:
/// the record's visible-subpart mask (record `+0x14`, a bit per subpart of
/// the section) read over the part's own subparts, however many. Bit
/// `n % 64` of `visible[n / 64]` is the section's subpart `first + n`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Subparts {
    /// The part's first subpart (part `+0xa`).
    pub first: u16,
    /// Its subparts (part `+0xc`).
    pub count: u16,
    /// The mask's bits over them, `count.div_ceil(64)` words.
    pub visible: Vec<u64>,
}

impl Subparts {
    /// The subparts of `first`, `count` of them, of which those in
    /// `drawn` (by index among the part's own) draw.
    pub fn of(first: u16, count: u16, drawn: impl IntoIterator<Item = u16>) -> Self {
        let mut visible = vec![0u64; usize::from(count).div_ceil(64)];
        for n in drawn.into_iter().filter(|&n| n < count) {
            visible[usize::from(n / 64)] |= 1 << (n % 64);
        }
        Self { first, count, visible }
    }

    /// The part's subparts that draw, by index among its own (0 the
    /// part's first), ascending.
    pub fn drawn(&self) -> impl Iterator<Item = u16> + '_ {
        (0..self.count).filter(move |&n| self.visible.get(usize::from(n / 64)).is_some_and(|w| *w >> (n % 64) & 1 != 0))
    }
}

/// What one instance draws for one layer item.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PartDraw {
    /// The item's place in the guest's sorted layer list.
    pub slot: u32,
    /// The record, the material (shader tag datum), section, part and
    /// implementation the guest picked.
    pub key: ItemKey,
    pub layer: u8,
    /// Into [`FrameScene::instances`]: the instance it belongs to.
    pub instance: u32,
    /// Into [`FrameScene::rows`]: the palette rows its record uploads,
    /// already remapped for its section.
    pub rows: Range<u32>,
    pub flags: PartFlags,
    /// With [`PartFlags::SUBPARTS`]: the subparts it draws; `None`, the
    /// whole part.
    pub subparts: Option<Subparts>,
}

/// The main visibility lists before layer masks and part selection. Cluster
/// IDs have the BSP's numbering in both lists and in the layer-3 cluster list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ClusterGather {
    /// Guest clipping mode (0 portal flood, 1 sphere, 2 the potentially
    /// visible set).
    pub mode: u32,
    /// The visible clusters: the guest's region order, or ascending when
    /// [`VisibilitySource::Rust`] supplied them.
    pub visible: Vec<u16>,
    pub structure: Vec<u16>,
    /// The view's object list (handles): the guest's list order, or
    /// ascending when [`VisibilitySource::Rust`] supplied it.
    pub objects: Vec<u32>,
    /// Who computed `visible` and `objects` (docs/graphics/native-visibility.md).
    pub source: VisibilitySource,
}

/// Which visibility a view's [`ClusterGather`] carries. The Rust sets are
/// the default; the guest's lists are the fallback a declined computation
/// takes (docs/graphics/native-visibility.md).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilitySource {
    /// The guest's portal flood and object gather, read.
    #[default]
    Guest,
    /// `scene_extract::visibility`, computed for this view.
    Rust,
}

impl VisibilitySource {
    pub fn name(self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::Rust => "rust",
        }
    }
}

/// A root object outside the frustum whose shadow can reach this view.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NearbyCaster {
    pub handle: u32,
    pub definition: u32,
    pub nodes: Vec<NodeMatrix>,
    pub sphere: ([f32; 3], f32),
    pub towards_light: [f32; 3],
    pub strength: f32,
}

/// One rendered frame, typed. Built on the sim thread by
/// [`crate::SceneBuilder`], sealed, then read only.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FrameScene {
    pub id: SceneId,
    pub stamp: Stamp,
    pub cut: Cut,
    /// At most four game views plus the alternate view.
    pub views: Vec<View>,
    /// Visibility at the base layer, indexed by view ordinal. Kept outside
    /// the View so the stage hooks can copy a View without copying its lists.
    pub cluster_gathers: Vec<Option<ClusterGather>>,
    /// Visible cluster ids; views index ranges of it.
    pub clusters: Vec<u16>,
    pub instances: Vec<Instance>,
    /// Shadow-only poses indexed by scene view, in guest pool order.
    pub nearby_casters: Vec<(u32, NearbyCaster)>,
    pub nodes: Vec<NodeMatrix>,
    pub parts: Vec<PartDraw>,
    /// Part indices, each view's in its guest list order.
    pub order: Vec<u32>,
    pub rows: Vec<Row>,
    pub lights: Vec<Light>,
    /// Each light's steps; lights index ranges of it.
    pub light_steps: Vec<LightStep>,
    pub layer_passes: Vec<LayerPass>,
    /// Each layer pass's items; passes index ranges of it.
    pub layer_items: Vec<LayerItem>,
    /// The views' decorator draws; views index ranges of it.
    pub decorators: Vec<DecoratorDraw>,
    /// Each decorator draw's instances; draws index ranges of it.
    pub decorator_instances: Vec<DecoratorInstance>,
    /// The views' decals; views index ranges of it.
    pub decals: Vec<DecalItem>,
    /// Each decal's quad-list vertices; decals index ranges of it.
    pub decal_vertices: Vec<DecalVertex>,
    /// Each light's gathered groups, in light order then the passes'
    /// order ([`crate::light_group`]).
    pub light_groups: Vec<LightGroup>,
    /// Each group's sorted entries; groups index ranges of it.
    pub light_entries: Vec<LightEntry>,
    /// The object shadows, in view order then the guest's.
    pub object_shadows: Vec<ObjectShadow>,
    /// Each object shadow's groups; shadows index ranges of it.
    pub shadow_groups: Vec<LightGroup>,
    /// The late layers' resolved materials; late layers index ranges of it.
    pub late_materials: Vec<LateMaterial>,
    /// The parts the late layers' stages draw; late layers index ranges of it.
    pub late_parts: Vec<LatePart>,
    /// The screen effects of the views' post passes; posts index ranges of it.
    pub post_effects: Vec<PostEffect>,
    /// What is drawn over the views, in order: the HUD's and the UI's
    /// quads, text runs and fills (`crate::overlay`).
    pub overlay: Overlay,
    /// The visual effect parts created since the last scene
    /// ([`crate::effect::PartEvent`]).
    pub part_events: Vec<crate::effect::PartEvent>,
}

fn slice<'a, T>(v: &'a [T], r: &Range<u32>) -> &'a [T] {
    v.get(r.start as usize..r.end as usize).unwrap_or(&[])
}

impl FrameScene {
    /// Empties every array, keeping its allocation.
    pub fn clear(&mut self) {
        self.id = SceneId::default();
        self.stamp = Stamp::default();
        self.cut = Cut::NONE;
        self.views.clear();
        self.cluster_gathers.clear();
        self.clusters.clear();
        self.instances.clear();
        self.nearby_casters.clear();
        self.nodes.clear();
        self.parts.clear();
        self.order.clear();
        self.rows.clear();
        self.lights.clear();
        self.light_steps.clear();
        self.layer_passes.clear();
        self.layer_items.clear();
        self.decorators.clear();
        self.decorator_instances.clear();
        self.decals.clear();
        self.decal_vertices.clear();
        self.light_groups.clear();
        self.light_entries.clear();
        self.object_shadows.clear();
        self.shadow_groups.clear();
        self.late_materials.clear();
        self.late_parts.clear();
        self.post_effects.clear();
        self.overlay.clear();
        self.part_events.clear();
    }

    pub fn view(&self, key: ViewKey) -> Option<&View> {
        self.views.iter().find(|v| v.key == key)
    }

    /// Each view's key and camera, in the scene's order: what the device
    /// classifies its carried draws against (`d3d::scene_sink::ReplayFrame::classify_cameras`).
    pub fn view_cameras(&self) -> impl Iterator<Item = (ViewKey, &Camera)> {
        self.views.iter().map(|v| (v.key, &v.camera))
    }

    pub fn clusters_of(&self, view: &View) -> &[u16] {
        slice(&self.clusters, &view.clusters)
    }

    pub fn instances_of(&self, view: &View) -> &[Instance] {
        slice(&self.instances, &view.instances)
    }

    pub fn nodes_of(&self, instance: &Instance) -> &[NodeMatrix] {
        slice(&self.nodes, &instance.nodes)
    }

    pub fn parts_of(&self, instance: &Instance) -> &[PartDraw] {
        slice(&self.parts, &instance.parts)
    }

    pub fn rows_of(&self, part: &PartDraw) -> &[Row] {
        slice(&self.rows, &part.rows)
    }

    pub fn lights_of(&self, view: &View) -> &[Light] {
        slice(&self.lights, &view.lights)
    }

    pub fn steps_of(&self, light: &Light) -> &[LightStep] {
        slice(&self.light_steps, &light.steps)
    }

    pub fn layers_of(&self, view: &View) -> &[LayerPass] {
        slice(&self.layer_passes, &view.layers)
    }

    pub fn items_of(&self, pass: &LayerPass) -> &[LayerItem] {
        slice(&self.layer_items, &pass.items)
    }

    /// The view whose layer draws hold `pass` (one of
    /// [`Self::layer_passes`], by address); `None` for a pass that is not
    /// this scene's.
    pub fn view_of_pass(&self, pass: &LayerPass) -> Option<&View> {
        let at = self.layer_passes.iter().position(|p| std::ptr::eq(p, pass))? as u32;
        self.views.iter().find(|v| v.layers.contains(&at))
    }

    /// The part of `view` at `slot` of the guest's list, with its
    /// instance.
    pub fn part_at(&self, view: &View, slot: u32) -> Option<(&Instance, &PartDraw)> {
        let order = slice(&self.order, &view.order);
        let i = order.binary_search_by_key(&slot, |&p| self.parts[p as usize].slot).ok()?;
        let part = &self.parts[order[i] as usize];
        Some((self.instances.get(part.instance as usize)?, part))
    }

    /// The instance of `view` with `key`.
    pub fn instance(&self, view: &View, key: InstanceKey) -> Option<&Instance> {
        self.instances_of(view).iter().find(|i| i.key == key)
    }

    /// Counts per view, totals across views.
    pub fn counts(&self, view: &View) -> ViewCounts {
        let instances = self.instances_of(view);
        let parts: u32 = instances.iter().map(|i| i.parts.len() as u32).sum();
        let rows: u32 = instances
            .iter()
            .flat_map(|i| self.parts_of(i))
            .map(|p| p.rows.len() as u32)
            .sum();
        let mut flagged = Vec::<(&'static str, u32)>::new();
        for p in instances.iter().flat_map(|i| self.parts_of(i)) {
            for name in p.flags.names() {
                match flagged.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, c)) => *c += 1,
                    None => flagged.push((name, 1)),
                }
            }
        }
        let lights = self.lights_of(view);
        ViewCounts {
            clusters: view.clusters.len() as u32,
            instances: instances.len() as u32,
            parts,
            rows,
            remainder: view.remainder,
            flagged,
            lights: lights.iter().filter(|l| l.guest_selected).count() as u32,
            lights_candidates: lights.len() as u32,
            lights_with_constants: lights.iter()
                .filter_map(|l| l.candidate.as_ref()?.constants.as_ref())
                .filter(|sets| sets.complete())
                .count() as u32,
            shadows: lights.iter().filter(|l| l.shadowed()).count() as u32,
            layers: self
                .layers_of(view)
                .iter()
                .map(|p| (p.layer, p.items.len() as u32, p.remainder))
                .collect(),
        }
    }

    /// Every range in bounds, each view's ranges disjoint from the other
    /// views', each instance's parts and nodes inside the arrays, each
    /// part pointing back at its instance, each view's order sorted by
    /// slot. An error names the first that is not.
    pub fn check(&self) -> Result<(), String> {
        fn within(r: &Range<u32>, len: usize, what: &str) -> Result<(), String> {
            if r.start > r.end || r.end as usize > len {
                return Err(format!("{what} {r:?} outside 0..{len}"));
            }
            Ok(())
        }
        let mut last = [0u32; 5];
        for (n, v) in self.views.iter().enumerate() {
            within(&v.clusters, self.clusters.len(), &format!("view {n} clusters"))?;
            within(&v.instances, self.instances.len(), &format!("view {n} instances"))?;
            within(&v.order, self.order.len(), &format!("view {n} order"))?;
            within(&v.lights, self.lights.len(), &format!("view {n} lights"))?;
            within(&v.layers, self.layer_passes.len(), &format!("view {n} layers"))?;
            let ranges = [&v.clusters, &v.instances, &v.order, &v.lights, &v.layers];
            for (k, (r, l)) in ranges.into_iter().zip(&mut last).enumerate() {
                if r.start < *l {
                    return Err(format!("view {n} range {k} {r:?} overlaps the view before"));
                }
                *l = r.end;
            }
            let mut previous = None;
            for &p in slice(&self.order, &v.order) {
                let part = self.parts.get(p as usize).ok_or_else(|| format!("view {n} orders part {p}, not in the scene"))?;
                if !v.instances.contains(&part.instance) {
                    return Err(format!("view {n} orders part {p} of another view's instance {}", part.instance));
                }
                if previous.is_some_and(|s| s >= part.slot) {
                    return Err(format!("view {n} order is not by slot at part {p}"));
                }
                previous = Some(part.slot);
            }
            let parts: usize = self.instances_of(v).iter().map(|i| i.parts.len()).sum();
            if parts != v.order.len() {
                return Err(format!("view {n} orders {} parts of {parts}", v.order.len()));
            }
        }
        for (n, i) in self.instances.iter().enumerate() {
            within(&i.nodes, self.nodes.len(), &format!("instance {n} nodes"))?;
            within(&i.parts, self.parts.len(), &format!("instance {n} parts"))?;
            for (k, p) in self.parts_of(i).iter().enumerate() {
                if p.instance as usize != n {
                    return Err(format!("instance {n} part {k} names instance {}", p.instance));
                }
                within(&p.rows, self.rows.len(), &format!("instance {n} part {k} rows"))?;
            }
        }
        for (n, l) in self.lights.iter().enumerate() {
            within(&l.steps, self.light_steps.len(), &format!("light {n} steps"))?;
        }
        self.check_light_groups()?;
        for (n, p) in self.layer_passes.iter().enumerate() {
            within(&p.items, self.layer_items.len(), &format!("layer pass {n} items"))?;
            for (k, i) in self.items_of(p).iter().enumerate() {
                within(&i.rows, self.rows.len(), &format!("layer pass {n} item {k} rows"))?;
            }
        }
        crate::decorator::check(self)?;
        crate::decal::check(self)?;
        crate::late::check(self)?;
        self.overlay.check()?;
        Ok(())
    }
}

/// One view's sizes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewCounts {
    pub clusters: u32,
    pub instances: u32,
    pub parts: u32,
    pub rows: u32,
    pub remainder: u32,
    /// Parts with each [`PartFlags`] flag, by name; flags no part has are
    /// left out.
    pub flagged: Vec<(&'static str, u32)>,
    pub lights: u32,
    pub lights_candidates: u32,
    pub lights_with_constants: u32,
    /// Lights that cast their shadow: the shadow maps the view draws.
    pub shadows: u32,
    /// Each layer pass: its layer, items and remainder.
    pub layers: Vec<(u8, u32, u32)>,
}
