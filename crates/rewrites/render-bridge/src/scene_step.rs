//! The frame step's scene (docs/graphics/framescene.md, slice 1): a
//! `FrameScene` begun at `render_views`, a view opened at each base-layer
//! layer 3 with its camera, visible clusters and world state, its objects
//! extracted at the object walk, its guest-selected lights at each pass
//! entry (`native_lights`), and all candidates at the end of the light
//! pass. The items of world layers 1, 5, 7, 8 and 20 come from each
//! layer's draw, the model decorators from layer 3
//! (`scene_extract::decorators`), a first-person view with the items
//! of its layers at the first-person draw `0x0003fdf0`, and the view's late
//! passes (slice 5: the fog at layer 16 and the water at layer 6 from their
//! glue, [`late_layer`]; the post passes at `render_screen_effects` and its
//! callees, [`post`]), sealed and published after
//! `render_frame_and_present`.
//!
//! Still game-thread, still packets, still masks: the world and object
//! stages take their camera, object frames and rows from the open view
//! instead of reading the guest themselves, at the same points and from
//! the same reads. Nothing here writes guest memory.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use engine::memory::GuestMemory;
use engine::points;
use engine::render_loop::occlusion::guest::{FRAME, VIEW_INDEX};
use frame_scene::{
    Camera, Cut, DeviceFog, FrameScene, GroupSite, LateInput, LateLayer, LateMaterial, LatePart,
    LightStep, SceneBuilder, SceneId, SceneLight, Stamp, View, ViewKey, ViewKind,
};
use native_render::object_walk::Entry;
use native_render::skip::{CURRENT_LAYER, LIGHTMAP_LAYER};
use native_render::world_layers::{Site, layer_site};
use scene_extract::GuestRead;
use scene_extract::extract;
use scene_extract::light_groups::{self, Cause, Sorted};
/// The shadow pass a light-group layer draws into, read from the guest
/// (for the glue that checks a caster group before skipping its handlers).
pub use scene_extract::light_groups::{shadow_filter as extract_shadow_filter, shadow_pass};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "frame-scene";

/// The game-time record's tick count (`[GAME_TIME] + 8`).
const GAME_TIME_TICK: u32 = 8;

/// The return address of `render_scene_passes`' decal mode 1 call
/// (`0x0002c048`), the next call after `render_dynamic_lights` returns.
const DECALS_AFTER_LIGHTS: u32 = 0x0002_c04d;

/// Guest memory, read only.
struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

pub type PresentPayload = Arc<dyn std::any::Any + Send + Sync>;

/// Read-only inputs to the render context's image capture.
pub trait PresentObserver: Send + Sync {
    fn capture(&self, guest: &dyn GuestMemory) -> Option<PresentPayload>;
    fn horizontal_factor(&self) -> f32;
}

#[derive(Clone, Copy)]
pub struct CapturedView {
    pub camera: Camera,
    pub horizontal_factor: f32,
}

pub struct CapturedImage {
    pub views: [Option<CapturedView>; 4],
    pub payload: PresentPayload,
}

struct PresentViews {
    observer: Weak<dyn PresentObserver>,
    image: CapturedImage,
}

/// Detach the completed sample at Swap on the current render owner,
/// including a render resumed on the game thread after worker fallback.
pub fn take_present_image() -> Option<CapturedImage> {
    if !PRESENT_READY.load(Ordering::Acquire) {
        return None;
    }
    with_step(|step| {
        PRESENT_READY.store(false, Ordering::Release);
        step.presentation.take().map(|present| present.image)
    })
}

#[derive(Default)]
struct Step {
    // SceneId can repeat after a snapshot load. A present belongs to one
    // particular begin/seal interval, even when guest frame numbers repeat.
    sequence: u64,
    builder: SceneBuilder,
    // In-progress image metadata outlives the diagnostic seal.
    presentation: Option<PresentViews>,
    /// The tick of the scene sealed last.
    tick: Option<u32>,
    /// The open view's clusters and world state are read: at its base
    /// layer 3, which its layer 1 can precede.
    based: bool,
    /// The light whose passes run is the open view's last: its groups
    /// may follow ([`light_group`]).
    light_open: bool,
    /// The object shadow drawing is the open view's last: its groups may
    /// follow.
    shadow_open: bool,
    /// The structure's portal graph for the Rust visibility, decoded once.
    visibility: scene_extract::visibility::GraphCache,
    visibility_lens: VisibilityLens,
    /// The main view's window volumes when the Rust visibility equalled the
    /// guest's, for the decorator placements ([`decorators`]).
    decorator_windows: Option<scene_extract::visibility::frusta::WindowFrusta>,
    /// The open game view's visible clusters, in the order reached, and
    /// the clip of its mode: the inputs of the Rust light gather
    /// ([`scene_extract::lights::gather::main_view`]).
    light_view: Option<(Vec<u16>, scene_extract::visibility::objects::Clip)>,
    /// The gather lens's objects for a game view whose window is wider
    /// than that lens, keyed by frame and view index
    /// ([`gather_lens_object`]).
    gathered: Option<GatherObjects>,
}

/// The gather lens's object set of one game view wider than it, and the
/// objects its guest setup left undrawn ([`gather_lens_object`]).
struct GatherObjects {
    key: (u32, u32),
    /// The gather lens's object handles, sorted; `None` when its
    /// visibility declined.
    objects: Option<Vec<u32>>,
    /// Gathered by the window's camera, outside the gather lens's set: the
    /// display supplement draws them ([`take_guest_lens_objects`]).
    skipped: Vec<u32>,
}

static STEP: Mutex<Option<Step>> = Mutex::new(None);
static PRESENT_READY: AtomicBool = AtomicBool::new(false);
/// A menu model view (`MODEL_VIEW_CALL` .. `MODEL_VIEW_RETURN`) is drawing:
/// its camera is the widget's, not the view record's, so the gather lens
/// has no say over its objects.
static IN_MODEL_VIEW: AtomicBool = AtomicBool::new(false);

/// Mark the open menu model view ([`gather_lens_object`] declines inside it).
pub(crate) fn set_model_view(open: bool) {
    IN_MODEL_VIEW.store(open, Ordering::Relaxed);
}
/// The object shadow being drawn: its index in the open scene's
/// `object_shadows`, or [`NO_SHADOW`] ([`object_shadow_at`]).
static SHADOW_AT: AtomicU32 = AtomicU32::new(NO_SHADOW);
const NO_SHADOW: u32 = u32::MAX;
static LAST: Mutex<Option<Arc<FrameScene>>> = Mutex::new(None);
static SEALED: AtomicU64 = AtomicU64::new(0);
static UNSEALED: AtomicU64 = AtomicU64::new(0);
static VIEWS: AtomicU64 = AtomicU64::new(0);
static NO_CAMERA: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);
static LAYERS: AtomicU64 = AtomicU64::new(0);
static LAYER_ITEMS: AtomicU64 = AtomicU64::new(0);
static LAYER_REMAINDER: AtomicU64 = AtomicU64::new(0);
static LAYER_UNREAD: AtomicU64 = AtomicU64::new(0);
static DECORATOR_PLACEMENTS: [AtomicU64; scene_extract::decorators::PlacementSource::ALL.len()] =
    [const { AtomicU64::new(0) }; scene_extract::decorators::PlacementSource::ALL.len()];
static DECORATOR_DRAWS: AtomicU64 = AtomicU64::new(0);
static DECORATOR_INSTANCES: AtomicU64 = AtomicU64::new(0);
static DECORATOR_REMAINDER: AtomicU64 = AtomicU64::new(0);
static DECAL_ITEMS: AtomicU64 = AtomicU64::new(0);
static DECAL_QUADS: AtomicU64 = AtomicU64::new(0);
static DECAL_REMAINDER: AtomicU64 = AtomicU64::new(0);
static WEATHER_PLATES: AtomicU64 = AtomicU64::new(0);
static WEATHER_UNREAD: AtomicU64 = AtomicU64::new(0);
static LIGHT_EXTRACT_VIEWS: AtomicU64 = AtomicU64::new(0);
static LIGHT_EXTRACT_NS: AtomicU64 = AtomicU64::new(0);
static LIGHT_EXTRACT_UNREAD: AtomicU64 = AtomicU64::new(0);
static LIGHT_GROUPS: AtomicU64 = AtomicU64::new(0);
static LIGHT_GROUPS_WHOLE: AtomicU64 = AtomicU64::new(0);
static LIGHT_ENTRIES: AtomicU64 = AtomicU64::new(0);
/// Shadow groups of layers 10, 11 and 14, and whole groups.
static SHADOW_GROUPS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static SHADOW_GROUPS_WHOLE: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
/// Per late layer (fog, water): views written, of those the gate held,
/// parts the stage took, and views it took.
static LATE_VIEWS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static LATE_GATED: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static LATE_PARTS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static LATE_DRAWN: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static POST_VIEWS: AtomicU64 = AtomicU64::new(0);
static POST_COLOUR: AtomicU64 = AtomicU64::new(0);
static POST_EFFECTS: AtomicU64 = AtomicU64::new(0);
/// Post callees' calls with no `render_screen_effects` of their view in
/// the scene.
static POST_UNPLACED: AtomicU64 = AtomicU64::new(0);
/// The Rust visibility beside the guest's (docs/graphics/native-visibility.md):
/// sampled game views compared, equal, differing, declined by reason,
/// views that carry the Rust sets, and the time spent computing. The
/// shadow arms count their own outcomes ([`visibility_arm_counters`]).
static VISIBILITY_CHECKED: AtomicU64 = AtomicU64::new(0);
static VISIBILITY_EQUAL: AtomicU64 = AtomicU64::new(0);
static VISIBILITY_DIFFER: AtomicU64 = AtomicU64::new(0);
static VISIBILITY_TAKEN: AtomicU64 = AtomicU64::new(0);
static VISIBILITY_NS: AtomicU64 = AtomicU64::new(0);
/// Game views by the lens their visibility used ([`VisibilityLens`]:
/// retail, the window-fitted guest lens, the display supplement), and the
/// views, clusters and objects the display supplement added.
static VISIBILITY_LENS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static VISIBILITY_DISPLAY_VIEWS: AtomicU64 = AtomicU64::new(0);
static VISIBILITY_DISPLAY_CLUSTERS: AtomicU64 = AtomicU64::new(0);
static VISIBILITY_DISPLAY_OBJECTS: AtomicU64 = AtomicU64::new(0);
/// Display-only clusters left out because a part draws in a layer the
/// display supplement has no draw for ([`keep_complete_clusters`]).
static VISIBILITY_DISPLAY_INCOMPLETE: AtomicU64 = AtomicU64::new(0);
/// Object setups a view wider than the gather lens left undrawn, outside
/// that lens's set, and such views whose set declined
/// ([`gather_lens_object`]).
static GUEST_LENS_OBJECTS_SKIPPED: AtomicU64 = AtomicU64::new(0);
static GUEST_LENS_GATHER_DECLINED: AtomicU64 = AtomicU64::new(0);
/// Calls of the projection-terms fit (`VIEW_PROJECTION_TERMS`) by outcome
/// ([`GuestFit`]), and the last caller it did not recognise.
static GUEST_FIT: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static GUEST_FIT_LAST_CALLER: AtomicU64 = AtomicU64::new(0);
/// Calls of the ocean's grid setup (`OCEAN_GRID_ASPECT`): the grid
/// widened to a fitted view's lens, and left retail.
static OCEAN_GRID: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static VISIBILITY_DECLINED: [AtomicU64; scene_extract::visibility::Decline::ALL.len()] =
    [const { AtomicU64::new(0) }; scene_extract::visibility::Decline::ALL.len()];
/// `render.scene`'s counter of each decline, in `Decline::ALL`'s order.
const VISIBILITY_DECLINED_NAMES: [&str; scene_extract::visibility::Decline::ALL.len()] = [
    "scene_visibility_declined_unread",
    "scene_visibility_declined_layout",
    "scene_visibility_declined_mode",
    "scene_visibility_declined_frusta",
    "scene_visibility_declined_overflow",
    "scene_visibility_declined_camera",
    "scene_visibility_declined_not_main",
];
/// The first view since the map loaded whose Rust sets differed from the
/// guest's, and how.
/// Game views whose Rust view record refused (`view::Decline`): the guest
/// answers their visibility that frame.
static VISIBILITY_DECLINED_RECORD: AtomicU64 = AtomicU64::new(0);
static VISIBILITY_FIRST_MISMATCH: Mutex<Option<String>> = Mutex::new(None);
/// The main view's records built in Rust beside the guest's
/// (`visibility::view`): views compared, equal, differing, declined by
/// reason, and the first difference since the map loaded.
static VIEW_RECORD_CHECKED: AtomicU64 = AtomicU64::new(0);
static VIEW_RECORD_EQUAL: AtomicU64 = AtomicU64::new(0);
static VIEW_RECORD_DIFFER: AtomicU64 = AtomicU64::new(0);
/// Guest-lens views by what the guest's projection holds
/// ([`GuestLensState`]): the widened scale, the retail one, or neither.
static VIEW_RECORD_GUEST_LENS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static VIEW_RECORD_DECLINED: [AtomicU64; scene_extract::visibility::view::Decline::ALL.len()] =
    [const { AtomicU64::new(0) }; scene_extract::visibility::view::Decline::ALL.len()];
/// `render.scene`'s counter of each view record decline, in
/// `view::Decline::ALL`'s order.
const VIEW_RECORD_DECLINED_NAMES: [&str; scene_extract::visibility::view::Decline::ALL.len()] = [
    "scene_view_record_declined_unread",
    "scene_view_record_declined_layout",
    "scene_view_record_declined_cinematic",
    "scene_view_record_declined_no_observer",
    "scene_view_record_declined_override",
];
static VIEW_RECORD_FIRST_MISMATCH: Mutex<Option<String>> = Mutex::new(None);
/// The light views' Rust visibility beside the guest's
/// (`visibility::light`): lights compared at their passes' entry, equal,
/// differing, declined by reason, and the time spent. A differential only.
static LIGHT_VISIBILITY_CHECKED: AtomicU64 = AtomicU64::new(0);
static LIGHT_VISIBILITY_EQUAL: AtomicU64 = AtomicU64::new(0);
static LIGHT_VISIBILITY_DIFFER: AtomicU64 = AtomicU64::new(0);
static LIGHT_VISIBILITY_NS: AtomicU64 = AtomicU64::new(0);
static LIGHT_VISIBILITY_DECLINED: [AtomicU64; scene_extract::visibility::Decline::ALL.len()] =
    [const { AtomicU64::new(0) }; scene_extract::visibility::Decline::ALL.len()];
/// `render.scene`'s counter of each light view decline, in
/// `Decline::ALL`'s order.
const LIGHT_VISIBILITY_DECLINED_NAMES: [&str; scene_extract::visibility::Decline::ALL.len()] = [
    "scene_light_visibility_declined_unread",
    "scene_light_visibility_declined_layout",
    "scene_light_visibility_declined_mode",
    "scene_light_visibility_declined_frusta",
    "scene_light_visibility_declined_overflow",
    "scene_light_visibility_declined_camera",
    "scene_light_visibility_declined_not_main",
];
/// The first light since the map loaded whose Rust sets differed from its
/// guest lists, and how.
static LIGHT_VISIBILITY_FIRST_MISMATCH: Mutex<Option<String>> = Mutex::new(None);
static LIGHT_ENTRY_REMAINDER: [AtomicU64; Cause::ALL.len()] =
    [const { AtomicU64::new(0) }; Cause::ALL.len()];
/// `render.scene`'s counter of each [`Cause`], in [`Cause::ALL`]'s order.
const LIGHT_ENTRY_REMAINDER_NAMES: [&str; Cause::ALL.len()] = [
    "scene_light_entry_remainder_memory",
    "scene_light_entry_remainder_callback",
    "scene_light_entry_remainder_owner",
    "scene_light_entry_remainder_section",
    "scene_light_entry_remainder_range",
    "scene_light_entry_remainder_state",
    "scene_light_entry_remainder_subparts",
    "scene_light_entry_remainder_material",
    "scene_light_entry_remainder_pose",
];

fn with_step<R>(f: impl FnOnce(&mut Step) -> R) -> R {
    let mut step = STEP.lock().unwrap_or_else(|e| e.into_inner());
    f(step.get_or_insert_with(Step::default))
}

/// Horizontal lens used by the view's portal flood and object planes.
#[derive(Clone, Copy, Default)]
pub enum VisibilityLens {
    #[default]
    Retail,
    /// The window fit applied to the guest camera, as factors on its
    /// horizontal terms (the smaller, the wider): `projection` the
    /// window's, `gather` the fixed gather lens's and `frustum` the one
    /// the guest's tangent bounds took, the wider of the two
    /// (docs/graphics/high-resolution.md, "Widescreen projection").
    Guest { projection: f32, frustum: f32, gather: f32 },
    /// Extra visible space on the exact-mode display copy.
    Display(f32),
}

impl VisibilityLens {
    /// The `scene_visibility_lens_*` counter this lens counts under.
    fn counter(self) -> usize {
        match self {
            Self::Retail => 0,
            Self::Guest { .. } => 1,
            Self::Display(_) => 2,
        }
    }
}

/// What the projection-terms fit did with one call: widened the terms,
/// kept retail for a game of several machines, kept retail for a caller
/// that is not a displayed camera (`caller`), or kept retail because the
/// window is 4:3 or the terms were unusable.
#[derive(Clone, Copy, Debug)]
pub enum GuestFit {
    Applied,
    Networked,
    Caller(u32),
    Unchanged,
}

/// Counts one call of the projection-terms fit
/// (`scene_guest_lens_fit_*` in `render.scene`).
pub fn count_guest_fit(fit: GuestFit) {
    let slot = match fit {
        GuestFit::Applied => 0,
        GuestFit::Networked => 1,
        GuestFit::Caller(caller) => {
            GUEST_FIT_LAST_CALLER.store(u64::from(caller), Ordering::Relaxed);
            2
        }
        GuestFit::Unchanged => 3,
    };
    GUEST_FIT[slot].fetch_add(1, Ordering::Relaxed);
}

/// Counts one call of the ocean's grid setup
/// (`scene_guest_lens_ocean_widened` and `_retail` in `render.scene`).
pub fn count_ocean_grid(widened: bool) {
    OCEAN_GRID[usize::from(!widened)].fetch_add(1, Ordering::Relaxed);
}

pub fn set_visibility_lens(lens: VisibilityLens) {
    with_step(|step| step.visibility_lens = lens);
}

fn tick(m: &Ro<'_>) -> u32 {
    m.read_u32(points::GAME_TIME.base)
        .and_then(|t| m.read_u32(t.checked_add(GAME_TIME_TICK)?))
        .unwrap_or(0)
}

fn id(m: &Ro<'_>) -> SceneId {
    SceneId {
        map_epoch: crate::native_map_work::generation(),
        frame: u64::from(m.read_u32(FRAME).unwrap_or(0)),
        tick: tick(m),
    }
}

fn begin_step(step: &mut Step, m: &Ro<'_>) {
    if step.builder.is_building() {
        UNSEALED.fetch_add(1, Ordering::Relaxed);
    }
    step.sequence = step.sequence.wrapping_add(1);
    step.builder.begin(id(m));
    step.decorator_windows = None;
    step.light_view = None;
    step.light_open = false;
    step.shadow_open = false;
    SHADOW_AT.store(NO_SHADOW, Ordering::Relaxed);
}

/// `render_views`' entry: a new scene. `present` samples the image the
/// scene is drawn for; a scene-lifecycle test opens one with `None`.
pub fn begin(mem: &dyn GuestMemory, present: Option<&Arc<dyn PresentObserver>>) {
    let m = Ro(mem);
    let presentation = present.and_then(|observer| {
        let payload = observer.capture(mem)?;
        Some(PresentViews {
            observer: Arc::downgrade(observer),
            image: CapturedImage {
                views: [None; 4],
                payload,
            },
        })
    });
    crate::native_particle_walk::frame_begin();
    with_step(|step| {
        PRESENT_READY.store(presentation.is_some(), Ordering::Release);
        step.presentation = presentation;
        begin_step(step, &m);
    });
}

/// The open view's camera at a base-layer layer 3, opening the view
/// (camera, visible clusters, world state) when none is open for the
/// current guest view and camera. `None` when the render camera does
/// not read as a camera: the stages leave the view to the guest.
pub fn view(mem: &dyn GuestMemory) -> Option<Camera> {
    open_view(mem).map(|v| v.camera)
}

/// Additions from the base layer of this same view. A late-layer hook may
/// run without a new base view, so check its camera and view index.
pub(crate) fn late_display_visibility(mem: &dyn GuestMemory, layer: u8) -> Option<Arc<frame_scene::DisplayVisibility>> {
    with_step(|step| {
        let view = step.builder.view()?;
        let extra = view.display_visibility.as_ref()?;
        (view.key.kind == ViewKind::Game && u32::from(view.key.view) == mem.u32(VIEW_INDEX).ok()?
            && !extra.whole_clusters.is_empty() && view.camera == extract::camera(&Ro(mem))?
            && scene_extract::late::layer_items(&Ro(mem), layer)? == 0).then(|| Arc::clone(extra))
    })
}

/// [`view`], returning the view itself (a copy: its ranges index the
/// scene being written).
pub fn open_view(mem: &dyn GuestMemory) -> Option<View> {
    open_kind(mem, ViewKind::Game)
}

/// The open view of `kind` for the current guest view and camera, opened
/// when none is. At a layer-3 draw the view's clusters and world state
/// are read, once: they are layer 3's (its list and its texture
/// accumulation target), and the view can open earlier, at its layer 1.
pub(crate) fn open_kind(mem: &dyn GuestMemory, kind: ViewKind) -> Option<View> {
    open_kind_at_layer(mem, kind, false)
}

fn open_kind_at_layer(mem: &dyn GuestMemory, kind: ViewKind, layer_hook: bool) -> Option<View> {
    let m = Ro(mem);
    let Some(camera) = extract::camera(&m) else {
        NO_CAMERA.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    let key = ViewKey {
        kind,
        player: 0,
        view: m.read_u32(VIEW_INDEX).unwrap_or(0) as u8,
    };
    let base = m.read_u32(CURRENT_LAYER) == Some(LIGHTMAP_LAYER);
    let (view, observer) = with_step(|step| {
        if !step.builder.is_building() {
            // A render this step did not see begin (no `render_views`
            // observer in this plan): the scene starts here.
            begin_step(step, &m);
        }
        if step
            .builder
            .view()
            .is_none_or(|v| v.key != key || v.camera != camera)
        {
            if step.builder.open_view(key, camera).is_err() {
                REFUSED.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            VIEWS.fetch_add(1, Ordering::Relaxed);
            step.based = false;
            step.decorator_windows = None;
            step.light_view = None;
            step.light_open = false;
            step.shadow_open = false;
        }
        if base && !step.based {
            step.based = true;
            if kind == ViewKind::Game {
                let casters = scene_extract::layout::nearby_casters::extract(&m, &camera);
                let _ = step.builder.add_nearby_casters(casters);
            }
            let clusters = extract::layer_clusters(&m, LIGHTMAP_LAYER).unwrap_or_default();
            let set = step
                .builder
                .set_clusters(clusters)
                .and_then(|()| step.builder.set_world(extract::world(&m)))
                .and_then(|()| {
                    if kind == ViewKind::Game {
                        let record = view_record_arm(&m, &camera, key, step.visibility_lens);
                        VISIBILITY_LENS[step.visibility_lens.counter()].fetch_add(1, Ordering::Relaxed);
                        let display_factor = match step.visibility_lens {
                            VisibilityLens::Display(factor) => Some(factor),
                            _ => None,
                        };
                        // The guest's lists, when they read: the
                        // comparison's other side and the arm's fallback.
                        let guest = extract::cluster_gather(&m);
                        let (gather, mut extra, windows) = with_arms(|arms| {
                            visibility_arm(&m, record.as_ref(), key, &mut step.visibility, guest, display_factor, arms)
                        });
                        step.light_view = gather
                            .as_ref()
                            .and_then(|g| light_clip(&m, g.mode, windows.as_ref()).map(|clip| (g.visible.clone(), clip)));
                        step.decorator_windows = windows;
                        if let Some(gather) = gather {
                            if let VisibilityLens::Guest { .. } = step.visibility_lens
                                && let (Some(frame), Some(view)) = (m.read_u32(FRAME), m.read_u32(VIEW_INDEX))
                            {
                                let objects = take_guest_lens_objects(step, (frame, view));
                                if !objects.is_empty() {
                                    extra = Some(frame_scene::DisplayVisibility {
                                        whole_clusters: Vec::new(), placements: Vec::new(), clusters: Vec::new(),
                                        objects, guest_lens: true,
                                    });
                                }
                            }
                            if let Some(extra) = extra {
                                VISIBILITY_DISPLAY_VIEWS.fetch_add(1, Ordering::Relaxed);
                                VISIBILITY_DISPLAY_CLUSTERS.fetch_add(extra.clusters.len() as u64, Ordering::Relaxed);
                                VISIBILITY_DISPLAY_OBJECTS.fetch_add(extra.objects.len() as u64, Ordering::Relaxed);
                                step.builder.set_display_visibility(extra)?;
                            }
                            step.builder.set_cluster_gather(gather)?;
                        }
                    }
                    Ok(())
                });
            if set.is_err() {
                REFUSED.fetch_add(1, Ordering::Relaxed);
            }
        }
        let observer = if layer_hook && kind == ViewKind::Game {
            step.presentation.as_ref().and_then(|present| {
                let old = present.image.views.get(usize::from(key.view))?;
                if old.is_some_and(|old| old.camera == camera) {
                    return None;
                }
                present.observer.upgrade()
            })
        } else {
            None
        };
        Some((step.builder.view()?.clone(), observer))
    })?;
    if let Some(observer) = observer {
        // Read the per-view lens outside the scene lock. Swap takes the
        // locks in device-then-scene order when it detaches the sample.
        let horizontal_factor = observer.horizontal_factor();
        with_step(|step| {
            if let Some(present) = &mut step.presentation
                && let Some(slot) = present.image.views.get_mut(usize::from(key.view))
            {
                *slot = Some(CapturedView {
                    camera,
                    horizontal_factor,
                });
            }
        });
    }
    Some(view)
}

/// The current list's `layer` items into the first person's view, for
/// the cut's answer (`native_first_person::build`).
pub(crate) fn first_person_layer(mem: &dyn GuestMemory, layer: u8) {
    layer_items(mem, ViewKind::FirstPerson, u32::from(layer));
}

/// The current list's `layer` items into the open view of `kind` (a
/// layer draw of the scene's world layers or of the first person's).
fn layer_items(mem: &dyn GuestMemory, kind: ViewKind, layer: u32) {
    if open_kind_at_layer(mem, kind, true).is_none() {
        return;
    }
    let m = Ro(mem);
    let Ok(layer) = u8::try_from(layer) else {
        return;
    };
    // Every object item takes its owner's lighting: the layer stage's
    // lighting inputs read it, and the lightmap cycle reaches the draw
    // only through it (`frame_scene::LayerInputs`).
    with_step(
        |step| match extract::layer(&m, layer, true, &mut step.builder) {
            Ok(Some(got)) => {
                LAYERS.fetch_add(1, Ordering::Relaxed);
                LAYER_ITEMS.fetch_add(u64::from(got.items), Ordering::Relaxed);
                LAYER_REMAINDER.fetch_add(u64::from(got.remainder), Ordering::Relaxed);
            }
            Ok(None) => {
                LAYER_UNREAD.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                REFUSED.fetch_add(1, Ordering::Relaxed);
            }
        },
    );
}

/// The objects of the base layer's sorted walk into the open view (its
/// first geometry handler call). Opens the view first when none is.
pub(crate) fn objects(mem: &dyn GuestMemory, kind: ViewKind, entries: &[Entry]) -> bool {
    if open_kind(mem, kind).is_none() {
        return false;
    }
    let m = Ro(mem);
    with_step(|step| {
        if extract::objects(&m, entries, &mut step.builder).is_err() {
            REFUSED.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    })
}

/// The model decorators of the base layer 3 into the open view
/// (docs/graphics/native-decorators.md).
fn decorators(mem: &dyn GuestMemory) {
    let m = Ro(mem);
    with_step(|step| {
        match scene_extract::decorators::extract_with_windows(&m, LIGHTMAP_LAYER as u8, step.decorator_windows.as_ref(), &mut step.builder) {
            Ok(Some((got, source))) => {
                DECORATOR_PLACEMENTS[source as usize].fetch_add(1, Ordering::Relaxed);
                DECORATOR_DRAWS.fetch_add(u64::from(got.draws), Ordering::Relaxed);
                DECORATOR_INSTANCES.fetch_add(u64::from(got.instances), Ordering::Relaxed);
                DECORATOR_REMAINDER.fetch_add(u64::from(got.remainder), Ordering::Relaxed);
            }
            Ok(None) => {}
            Err(_) => {
                REFUSED.fetch_add(1, Ordering::Relaxed);
            }
        }
    });
}

/// The decals (`scene_extract::decals`) and weather plates
/// (`scene_extract::weather`) of the open view, at its layer 3: the
/// visible-cluster list and the view's window are set by then, and the
/// decal chains are tick state.
fn decals_and_weather(mem: &dyn GuestMemory) {
    let m = Ro(mem);
    with_step(|step| {
        match scene_extract::decals::extract(&m, &mut step.builder) {
            Ok(Some(got)) => {
                DECAL_ITEMS.fetch_add(u64::from(got.items), Ordering::Relaxed);
                DECAL_QUADS.fetch_add(u64::from(got.quads), Ordering::Relaxed);
                DECAL_REMAINDER.fetch_add(u64::from(got.remainder), Ordering::Relaxed);
            }
            Ok(None) => {}
            Err(_) => {
                REFUSED.fetch_add(1, Ordering::Relaxed);
            }
        }
        match scene_extract::weather::extract(&m, &mut step.builder) {
            Ok(Some((plates, unread))) => {
                WEATHER_PLATES.fetch_add(u64::from(plates), Ordering::Relaxed);
                WEATHER_UNREAD.fetch_add(u64::from(unread), Ordering::Relaxed);
            }
            Ok(None) => {}
            Err(_) => {
                REFUSED.fetch_add(1, Ordering::Relaxed);
            }
        }
    });
}

/// A light of the open view at its passes' entry, with its steps.
/// Opens the view first when none is.
pub fn light(
    mem: &dyn GuestMemory,
    light: SceneLight,
    steps: &[LightStep],
    call: &engine::light_constants::Call,
) {
    light_visibility_arm(mem, call);
    if view(mem).is_none() {
        return;
    }
    with_step(|step| {
        step.light_open = step.builder.push_light(light, steps).is_ok();
        if !step.light_open {
            REFUSED.fetch_add(1, Ordering::Relaxed);
        }
    });
}

/// The scene hand-off and the just-built lists must describe the same
/// invocation before its typed groups can replace any handlers.
pub(crate) fn current_light_matches(light: &SceneLight, steps: &[LightStep]) -> bool {
    with_step(|step| {
        let Some((scene, view)) = step.builder.open() else {
            return false;
        };
        let Some(at) = view.lights.clone().next_back() else {
            return false;
        };
        let current = &scene.lights[at as usize];
        step.light_open
            && current.guest_selected
            && current.handle == light.handle
            && current.definition == light.definition
            && current.guest_weight.map(f32::to_bits) == light.guest_weight.map(f32::to_bits)
            && current.flags == light.flags
            && current.definition_flags == light.definition_flags
            && current.additional_casters == light.additional_casters
            && scene.steps_of(current) == steps
    })
}

/// Append groups only after the host has reserved the entire invocation.
/// The builder assigns this view's indices and remaps every object's pose.
pub(crate) fn typed_light_groups(groups: Vec<crate::native_lights::SceneGroup>) {
    with_step(|step| {
        if !step.light_open {
            return;
        }
        for group in groups {
            let entries: Vec<_> = group.entries.into_iter().zip(group.rows).collect();
            if step.builder.push_light_group(group.group, &entries).is_err() {
                REFUSED.fetch_add(1, Ordering::Relaxed);
                return;
            }
            LIGHT_GROUPS.fetch_add(1, Ordering::Relaxed);
            LIGHT_GROUPS_WHOLE.fetch_add(1, Ordering::Relaxed);
            LIGHT_ENTRIES.fetch_add(entries.len() as u64, Ordering::Relaxed);
        }
    });
}

/// A first-person light (`0x000404b0`) begins: it goes into the first
/// person's view (opened when none is for the current guest view and
/// camera) with its steps, and its groups follow
/// ([`light_group`]). Its object shadows and candidates are not read.
pub fn first_person_light(mem: &dyn GuestMemory, light: SceneLight, steps: &[LightStep]) -> bool {
    if open_kind(mem, ViewKind::FirstPerson).is_none() {
        return false;
    }
    with_step(|step| {
        step.shadow_open = false;
        step.light_open = step.builder.push_light(light, steps).is_ok();
        if !step.light_open {
            REFUSED.fetch_add(1, Ordering::Relaxed);
        }
        step.light_open
    })
}

/// The host's own lights for a view: lights with no datum and no `ligh`
/// tag that the scene carries after the game's candidates
/// (docs/game/forge-monitor-appearance.md, ML5, the Forge monitors'
/// bodies). Called once a game view, outside the step's lock.
pub type HostLights = fn(&dyn GuestMemory) -> Vec<frame_scene::Light>;

static HOST_LIGHTS: Mutex<Option<HostLights>> = Mutex::new(None);

/// Sets the source of [`HostLights`]; `None` adds none.
pub fn set_host_lights(source: Option<HostLights>) {
    *HOST_LIGHTS.lock().unwrap_or_else(|e| e.into_inner()) = source;
}

fn host_lights(mem: &dyn GuestMemory) -> Vec<frame_scene::Light> {
    let source = *HOST_LIGHTS.lock().unwrap_or_else(|e| e.into_inner());
    source.map(|f| f(mem)).unwrap_or_default()
}

/// The main view's light pass is over (at the decal mode that follows
/// it). Fill the candidate data alongside the original draw steps, which
/// already hold their final group indices, then add the host's lights
/// ([`set_host_lights`]) after them. Returns what the light passes' owner
/// draws next: the host lights it added
/// ([`crate::native_lights::host::draw`]) and E3's chosen lights beyond
/// the guest's ([`chosen_lights`]); none for a view that is not a game
/// view.
fn light_candidates(mem: &dyn GuestMemory, fpu: engine::light_constants::Fpu) -> ViewLights {
    let host = host_lights(mem);
    let mut added = ViewLights::default();
    with_step(|step| {
        let Some((scene, view)) = step.builder.open() else {
            return;
        };
        if view.key.kind != ViewKind::Game {
            return;
        }
        step.light_open = false;
        let start = Instant::now();
        let drawn: Vec<u32> = scene
            .lights_of(view)
            .iter()
            .filter(|l| l.guest_selected)
            .map(|l| l.handle)
            .collect();
        let eligible = |record, definition, flags| {
            crate::native_scene_lists::light_candidate_eligible(mem, record, definition, flags)
        };
        // The visible-light list from Rust (docs/graphics/native-lights.md,
        // "The visible-light gather in Rust"); the guest's own list is not
        // read. A view whose visibility or light data did not read lists
        // only the lights the guest drew.
        let gathered = step.light_view.as_ref().and_then(|(clusters, clip)| {
            let view = scene_extract::visibility::guest::view(&Ro(mem)).ok()?;
            scene_extract::lights::gather::main_view(mem, view.flags, view.start, clusters, clip)
        });
        if let Some(candidates) = scene_extract::lights::candidates(mem, fpu, gathered.as_deref(), &drawn, eligible) {
            if step.builder.add_light_candidates(candidates).is_err() {
                REFUSED.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            LIGHT_EXTRACT_UNREAD.fetch_add(1, Ordering::Relaxed);
        }
        if !host.is_empty() {
            if step.builder.add_light_candidates(host.clone()).is_err() {
                REFUSED.fetch_add(1, Ordering::Relaxed);
            } else {
                added.host = host;
            }
        }
        (added.chosen, added.unread) = chosen_lights(&step.builder);
        LIGHT_EXTRACT_VIEWS.fetch_add(1, Ordering::Relaxed);
        LIGHT_EXTRACT_NS.fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    });
    added
}

/// The open view's last light, while it is open: the light whose passes
/// or first-person slot are running.
pub(crate) fn current_light() -> Option<frame_scene::Light> {
    with_step(|step| {
        let (scene, view) = step.builder.open()?;
        if !step.light_open {
            return None;
        }
        let at = view.lights.clone().next_back()?;
        scene.lights.get(at as usize).cloned()
    })
}

/// The open game view's object shadows, owned, outside the step's lock
/// ([`crate::native_lights::object_shadow_scene::gather`]).
pub(crate) fn open_object_shadow_passes() -> Vec<crate::native_lights::object_shadow_scene::ObjectShadowPass> {
    with_step(|step| {
        let Some((scene, view)) = step.builder.open() else { return Vec::new() };
        if view.key.kind != ViewKind::Game {
            return Vec::new();
        }
        crate::native_lights::object_shadow_scene::gather(scene, view)
    })
}

/// The clip the light gather takes for the guest's visibility `mode`: the
/// window frusta of the mode 0 flood (when the Rust visibility held), the
/// view's sphere (mode 1), none (mode 2). `None` when the mode's inputs
/// did not read.
fn light_clip(
    m: &Ro<'_>,
    mode: u32,
    windows: Option<&scene_extract::visibility::frusta::WindowFrusta>,
) -> Option<scene_extract::visibility::objects::Clip> {
    use scene_extract::visibility::objects::Clip;
    match mode {
        0 => windows.cloned().map(Clip::Frusta),
        1 => {
            let (centre, radius) = scene_extract::visibility::guest::view_sphere(m, engine::render_loop::occlusion::guest::MAIN_VIS).ok()?;
            Some(Clip::Sphere { centre, radius })
        }
        2 => Some(Clip::All),
        _ => None,
    }
}

/// What the light passes' owner draws once a game view's light pass is
/// over ([`light_candidates`]).
#[derive(Default)]
struct ViewLights {
    host: Vec<frame_scene::Light>,
    chosen: Vec<crate::native_lights::chosen::ChosenLight>,
    /// Chosen lights with nothing to draw from.
    unread: usize,
}

/// The open view's chosen lights beyond the guest's
/// ([`crate::native_lights::chosen::beyond_guest`]), under its share of
/// the cap and of the shadow slots among the views of this scene so far
/// and the scene before ([`native_render::light_choice::share_of`],
/// [`native_render::cone_shadows::slot_share_of`]): the shares the render
/// thread's choice gives it when the frame has the views the last one
/// had.
fn chosen_lights(
    builder: &frame_scene::SceneBuilder,
) -> (Vec<crate::native_lights::chosen::ChosenLight>, usize) {
    let Some((scene, view)) = builder.open() else {
        return (Vec::new(), 0);
    };
    let keys: Vec<frame_scene::ViewKey> = scene
        .views
        .iter()
        .map(|v| v.key)
        .chain(builder.previous_views().iter().copied())
        .collect();
    let cap = native_render::light_choice::share_of(&keys, view.key);
    let slots = native_render::cone_shadows::slot_share_of(&keys, view.key);
    crate::native_lights::chosen::beyond_guest(scene.lights_of(view), cap, slots)
}

/// Retail's first-person pass (`0x000404b0`) begins: the game view's
/// first-person lights it did not record
/// ([`crate::native_lights::chosen::first_person::choose`]), chosen under
/// the view's share of the cap as [`chosen_lights`] chooses it, the eye
/// the first-person view's camera, with that view's object parts so far
/// ([`crate::native_lights::chosen::first_person::parts`]). `None` when
/// the slots do not read or the scene holds no game or first-person view
/// of the guest's current view.
pub fn first_person_chosen(
    mem: &dyn GuestMemory,
) -> Option<crate::native_lights::chosen::first_person::FirstPersonPass> {
    use crate::native_lights::chosen::first_person::{self, FirstPersonPass};
    let recorded = crate::native_lights::slot_visibility::replay(mem)
        .or_else(|| engine::render::light_gather::first_person::recorded(mem).ok())?;
    let view = Ro(mem).read_u32(VIEW_INDEX).unwrap_or(0) as u8;
    let game_key = ViewKey {
        kind: ViewKind::Game,
        player: 0,
        view,
    };
    let fp_key = ViewKey {
        kind: ViewKind::FirstPerson,
        ..game_key
    };
    with_step(|step| {
        let scene = step.builder.scene()?;
        let game = scene.views.iter().rev().find(|v| v.key == game_key)?;
        let fp = scene.views.iter().rev().find(|v| v.key == fp_key)?;
        let keys: Vec<ViewKey> = scene
            .views
            .iter()
            .map(|v| v.key)
            .chain(step.builder.previous_views().iter().copied())
            .collect();
        let cap = native_render::light_choice::share_of(&keys, game_key);
        let set = first_person::choose(scene.lights_of(game), cap, &recorded, fp.camera.position);
        let items = scene.layers_of(fp).iter().flat_map(|pass| {
            scene.items_of(pass).iter().map(move |item| {
                let rows = scene
                    .rows
                    .get(item.rows.start as usize..item.rows.end as usize)
                    .unwrap_or(&[]);
                (item, rows)
            })
        });
        let (parts, left) = first_person::parts(items);
        Some(FirstPersonPass { set, parts, left })
    })
}

/// A light's passes begin: until [`light`] puts it in the view, its
/// groups have no light to join.
pub fn light_entry() {
    with_step(|step| {
        step.light_open = false;
        step.shadow_open = false;
    });
    SHADOW_AT.store(NO_SHADOW, Ordering::Relaxed);
}

/// The object shadow whose guest ops are being recorded: its index in the
/// open scene's `object_shadows`, from its `0x00032350` call until its
/// layer 11 ends ([`object_shadow_end`]), the next shadow or light, or the
/// scene's end. The scene sink marks each shadow's ops with it
/// (`d3d::SceneSink::with_object_shadows`) so the frame graph can find
/// them; the marks draw nothing (docs/graphics/e3-lighting.md, L5).
pub fn object_shadow_at() -> Option<u32> {
    let at = SHADOW_AT.load(Ordering::Relaxed);
    (at != NO_SHADOW).then_some(at)
}

/// An object shadow's layer 11 (its receivers) ends: the ops after it are
/// not the shadow's.
pub fn object_shadow_end() {
    SHADOW_AT.store(NO_SHADOW, Ordering::Relaxed);
}

/// An object shadow `0x00032350` of the open view begins, with its call's
/// two arguments (`identity`) when the caller matched its caster entry;
/// `None` ends the one before without a new one. Opens the view first
/// when none is. A creature caster is marked: it takes the ground-shadow
/// branch.
pub fn object_shadow(mem: &dyn GuestMemory, identity: Option<[u32; 2]>) {
    with_step(|step| step.shadow_open = false);
    SHADOW_AT.store(NO_SHADOW, Ordering::Relaxed);
    let Some(identity) = identity else { return };
    if view(mem).is_none() {
        return;
    }
    let creature = scene_extract::layout::objects::kind(&Ro(mem), identity[0])
        == Some(scene_extract::layout::objects::CREATURE);
    with_step(|step| {
        step.light_open = false;
        let pushed = step.builder.push_object_shadow(identity, creature);
        step.shadow_open = pushed.is_ok();
        if let Ok(at) = pushed {
            SHADOW_AT.store(at, Ordering::Relaxed);
        }
        if !step.shadow_open {
            REFUSED.fetch_add(1, Ordering::Relaxed);
        }
    });
}

/// The current object shadow's filter, extracted and copied out of the
/// scene for the native quad pass. `None` when its reads fail or no object
/// shadow is open.
pub fn shadow_filter(
    mem: &dyn GuestMemory,
    mxcsr: u32,
    x87_control: u16,
) -> Option<frame_scene::ShadowFilter> {
    let filter = extract_shadow_filter(mem, mxcsr, x87_control)?;
    with_step(|step| {
        if !step.shadow_open {
            return None;
        }
        step.builder.set_shadow_filter(filter.clone()).ok()?;
        Some(filter)
    })
}

/// The groups of the open object shadow as the cut caster planned them
/// from its item lists ([`crate::native_lights::object_shadow_scene::cut`]),
/// with its light weight and filter. Ends the shadow's ops: no layer of
/// it runs on the guest. False when no shadow is open or the builder
/// refuses a group.
pub(crate) fn object_shadow_groups(
    groups: Vec<crate::native_lights::SceneGroup>,
    weight: Option<f32>,
    filter: Option<frame_scene::ShadowFilter>,
) -> bool {
    SHADOW_AT.store(NO_SHADOW, Ordering::Relaxed);
    with_step(|step| {
        if !step.shadow_open || step.builder.view().is_none() {
            return false;
        }
        step.shadow_open = false;
        for group in groups {
            let entries: Vec<_> = group.entries.into_iter().zip(group.rows).collect();
            if step.builder.push_shadow_group(group.group, &entries).is_err() {
                REFUSED.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            LIGHT_GROUPS.fetch_add(1, Ordering::Relaxed);
            LIGHT_GROUPS_WHOLE.fetch_add(1, Ordering::Relaxed);
            LIGHT_ENTRIES.fetch_add(entries.len() as u64, Ordering::Relaxed);
        }
        if let Some(weight) = weight
            && step.builder.set_shadow_weight(weight).is_err()
        {
            return false;
        }
        if let Some(filter) = filter
            && step.builder.set_shadow_filter(filter).is_err()
        {
            return false;
        }
        true
    })
}

/// A light layer's sorted entries (`sorted`, as the layer build leaves
/// them on `list`) into a group of the open view's last light, or of its
/// last object shadow for [`GroupSite::ObjectShadow`], with what the first
/// geometry handler would supply (`scene_extract::light_groups`). The
/// light is in the view from its passes' entry, the object shadow from
/// its call; without one, nothing is written. The group written, copied
/// out, when every sorted entry is in it.
pub fn light_group(
    mem: &dyn GuestMemory,
    list: u32,
    layer: u32,
    site: GroupSite,
    sorted: &[Sorted],
) -> Option<crate::native_lights::SceneGroup> {
    let layer = u8::try_from(layer).ok()?;
    let m = Ro(mem);
    with_step(|step| {
        let open = if site == GroupSite::ObjectShadow {
            step.shadow_open
        } else {
            step.light_open
        };
        if !open || step.builder.view().is_none() {
            return None;
        }
        match light_groups::push(&m, list, layer, site, sorted, &mut step.builder) {
            Ok(got) => {
                LIGHT_GROUPS.fetch_add(1, Ordering::Relaxed);
                LIGHT_ENTRIES.fetch_add(u64::from(got.entries), Ordering::Relaxed);
                if got.remainder() == 0 {
                    LIGHT_GROUPS_WHOLE.fetch_add(1, Ordering::Relaxed);
                }
                if let Some(n) = match layer {
                    10 => Some(0),
                    11 => Some(1),
                    14 => Some(2),
                    _ => None,
                } {
                    SHADOW_GROUPS[n].fetch_add(1, Ordering::Relaxed);
                    if got.remainder() == 0 {
                        SHADOW_GROUPS_WHOLE[n].fetch_add(1, Ordering::Relaxed);
                    }
                }
                for (counter, n) in LIGHT_ENTRY_REMAINDER.iter().zip(got.remainder) {
                    counter.fetch_add(u64::from(n), Ordering::Relaxed);
                }
                let scene = step.builder.scene()?;
                let groups = if site == GroupSite::ObjectShadow {
                    &scene.shadow_groups
                } else {
                    &scene.light_groups
                };
                let group = groups.last()?.clone();
                if !group.whole() {
                    return None;
                }
                let entries = scene.entries_of(&group).to_vec();
                let rows = entries
                    .iter()
                    .map(|e| scene.pose_rows(e).to_vec())
                    .collect();
                Some(crate::native_lights::SceneGroup {
                    group,
                    entries,
                    rows,
                })
            }
            Err(_) => {
                REFUSED.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    })
}

/// The game view's key of the guest view being drawn, with a view of
/// that key in the scene: opened when it has none (a late pass with no
/// layer 3 before it this frame). `None` when no view opens.
fn late_key(mem: &dyn GuestMemory) -> Option<ViewKey> {
    let m = Ro(mem);
    let key = ViewKey {
        kind: ViewKind::Game,
        player: 0,
        view: m.read_u32(VIEW_INDEX).unwrap_or(0) as u8,
    };
    let has = with_step(|step| {
        step.builder
            .scene()
            .is_some_and(|s| s.views.iter().any(|v| v.key == key))
    });
    if !has && open_kind(mem, ViewKind::Game)?.key != key {
        return None;
    }
    Some(key)
}

fn late_index(layer: u8) -> Option<usize> {
    match layer {
        16 => Some(0),
        6 => Some(1),
        _ => None,
    }
}

/// What a late layer's stage reads, copied out of the scene: the layer as
/// written, its resolved materials and the parts it draws.
#[derive(Clone, Debug, PartialEq)]
pub struct LateStage {
    pub key: ViewKey,
    pub state: LateLayer,
    pub materials: Vec<LateMaterial>,
    pub parts: Vec<LatePart>,
}

/// The mesh the teardown draws, owned by the keyed view. Its stage
/// consumes this copy even when the layer-6 edge blend was empty.
pub fn water_composite(mem: &dyn GuestMemory, indices: u32, count: u32) -> Option<frame_scene::late::WaterComposite> {
    let key = late_key(mem)?;
    let geometry = scene_extract::late::water_composite(&Ro(mem), indices, count)?;
    with_step(|step| step.builder.set_water_composite(key, geometry).ok().cloned())
}

/// The late layer `layer` (16 the fog, 6 the water) of the view being
/// drawn (docs/graphics/framescene.md, slice 5): whether its gate held,
/// the device's fog words, the items the current list holds in it, and
/// what its stage would draw (`materials`, `parts`), not yet taken
/// ([`late_drawn`]). Returns the scene's copy, which the stage records
/// from; `None` when no view opens or the builder refuses, and then the
/// stage leaves the layer to the guest.
pub fn late_layer(
    mem: &dyn GuestMemory,
    layer: u8,
    draws: bool,
    device: DeviceFog,
    materials: &[LateMaterial],
    parts: &[LatePart],
) -> Option<LateStage> {
    let i = late_index(layer)?;
    let key = late_key(mem)?;
    let items = scene_extract::late::layer_items(&Ro(mem), layer);
    with_step(|step| {
        let input = LateInput {
            draws,
            device,
            items,
            materials,
            parts,
        };
        let state = match step.builder.set_late(key, layer, input) {
            Ok(s) => s,
            Err(_) => {
                REFUSED.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        LATE_VIEWS[i].fetch_add(1, Ordering::Relaxed);
        if draws {
            LATE_GATED[i].fetch_add(1, Ordering::Relaxed);
        }
        let scene = step.builder.scene()?;
        Some(LateStage {
            key,
            materials: scene.late_materials_of(&state).to_vec(),
            parts: scene.late_parts_of(&state).to_vec(),
            state,
        })
    })
}

/// The late layer `layer` of the view being drawn ran with its gate
/// `draws` and its stage did not take it: written as the guest draws it
/// (no device words, no parts) unless the stage's glue wrote it already.
pub fn late_kept(mem: &dyn GuestMemory, layer: u8, draws: bool) {
    let Some(key) = late_key(mem) else { return };
    let written = with_step(|step| {
        step.builder
            .scene()
            .and_then(|s| s.views.iter().rev().find(|v| v.key == key))
            .is_some_and(|v| v.late(layer).is_some())
    });
    if !written {
        late_layer(mem, layer, draws, [None; frame_scene::FOG_WORDS], &[], &[]);
    }
}

/// The stage took the late layer `layer` of the view `key`: the guest
/// skips its parts.
pub fn late_drawn(key: ViewKey, layer: u8) {
    let Some(i) = late_index(layer) else { return };
    with_step(|step| match step.builder.late_drawn(key, layer) {
        Ok(()) => {
            LATE_DRAWN[i].fetch_add(1, Ordering::Relaxed);
            let parts = step.builder.scene().and_then(|s| {
                s.views
                    .iter()
                    .rev()
                    .find(|v| v.key == key)?
                    .late(layer)
                    .cloned()
            });
            LATE_PARTS[i].fetch_add(parts.map_or(0, |l| l.parts.len() as u64), Ordering::Relaxed);
        }
        Err(_) => {
            REFUSED.fetch_add(1, Ordering::Relaxed);
        }
    });
}

/// `render_screen_effects`' entry with its stack bytes: the view's post
/// passes.
pub fn post(mem: &dyn GuestMemory, late: bool, colour: bool) {
    let Some(params) = scene_extract::late::post(&Ro(mem), late, colour) else {
        POST_UNPLACED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let Some(key) = late_key(mem) else { return };
    with_step(|step| match step.builder.set_post(key, params) {
        Ok(()) => {
            POST_VIEWS.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            REFUSED.fetch_add(1, Ordering::Relaxed);
        }
    });
}

/// The colour pass `0x000214f0` with its seven arguments, under the view's
/// post passes.
pub fn post_colour(mem: &dyn GuestMemory, args: [u32; 7]) {
    let key = ViewKey {
        kind: ViewKind::Game,
        player: 0,
        view: mem.u32(VIEW_INDEX).unwrap_or(0) as u8,
    };
    with_step(|step| match step.builder.post_colour(key, args) {
        Ok(()) => {
            POST_COLOUR.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            POST_UNPLACED.fetch_add(1, Ordering::Relaxed);
        }
    });
}

/// A screen effect `0x0002d140` (its datum, values address, columns and
/// rows), under the view's post passes.
pub fn post_effect(mem: &dyn GuestMemory, datum: u32, values: u32, columns: u32, rows: u32) {
    let effect = scene_extract::late::post_effect(&Ro(mem), datum, values, columns, rows);
    let key = ViewKey {
        kind: ViewKind::Game,
        player: 0,
        view: mem.u32(VIEW_INDEX).unwrap_or(0) as u8,
    };
    with_step(|step| match step.builder.push_post_effect(key, effect) {
        Ok(()) => {
            POST_EFFECTS.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            POST_UNPLACED.fetch_add(1, Ordering::Relaxed);
        }
    });
}

/// Runs `f` with the scene being written and its open view (`None`
/// without one). The step is held for the call: `f` must not come back
/// here.
pub fn with_open<R>(f: impl FnOnce(Option<(&FrameScene, &View)>) -> R) -> R {
    with_step(|step| f(step.builder.open()))
}

/// Builds the view's display queue and keeps it in the scene; returns it for
/// the sorted dispatch's display runs (`native_water::begin_display_runs`).
pub(crate) fn display_transparents(
    build: impl FnOnce(&[u16]) -> Option<frame_scene::transparent::DisplayTransparentQueue>,
) -> Option<Arc<frame_scene::transparent::DisplayTransparentQueue>> {
    with_step(|step| {
        let view = step.builder.view()?;
        if view.key.kind != ViewKind::Game { return None; }
        let visibility = view.display_visibility.as_ref()?;
        if visibility.clusters.is_empty() { return None; }
        let queue = build(&visibility.clusters)?;
        // `render.stats`: `layer_plan_15_display_queue_*` (docs/graphics/native-renderer.md).
        let count = |outcome, n| crate::native_layers::count(15, outcome, n);
        count("display_queue_built", 1);
        count("display_queue_geometry", queue.geometry().count() as u64);
        count("display_queue_unresolved_clusters", queue.unresolved_clusters.len() as u64);
        count("display_queue_inexact", u64::from(!queue.order_exact));
        let queue = Arc::new(queue);
        let _ = step.builder.set_display_transparents(Arc::clone(&queue));
        Some(queue)
    })
}

/// Runs `f` on the overlay of the scene being written (`FrameScene::overlay`,
/// docs/graphics/retire-d3d.md): `None` with no scene open. The step is
/// held for the call: `f` must not come back here.
pub fn with_overlay<R>(f: impl FnOnce(&mut frame_scene::Overlay) -> R) -> Option<R> {
    with_step(|step| step.builder.overlay_mut().ok().map(f))
}

/// A host which joins the typed scene to the owned device work seen
/// during the same begin/seal interval. Invoked without the builder lock.
#[derive(Clone, Copy)]
pub struct SealObserver {
    pub sealed: fn(u64, Arc<FrameScene>),
    pub reset: fn(),
}

static OBSERVER: Mutex<Option<SealObserver>> = Mutex::new(None);

pub fn set_seal_observer(observer: Option<SealObserver>) {
    *OBSERVER.lock().unwrap_or_else(|e| e.into_inner()) = observer;
}

/// Identifies the scene being built at a guest Swap. A present outside a
/// render frame cannot be joined to the last scene by tick number alone.
pub fn open_identity() -> Option<(u64, SceneId)> {
    with_step(|step| Some((step.sequence, step.builder.scene()?.id)))
}

fn part_event(e: sim_effects::scene::PartEvent) -> frame_scene::effect::PartEvent {
    use frame_scene::effect::PartKind;
    use sim_effects::scene::Kind;
    frame_scene::effect::PartEvent {
        seq: e.seq,
        tick: e.part.tick,
        kind: match e.kind {
            Kind::Decal => PartKind::Decal,
            Kind::Light => PartKind::Light,
            Kind::Flare => PartKind::Flare,
        },
        tag: e.part.tag,
        effect: e.part.definition,
        position: e.part.position,
        direction: e.part.direction,
        scale: e.part.scale,
        guest_made: e.guest_made,
    }
}

/// After `render_frame_and_present`: seal and publish the scene.
pub fn seal() {
    SHADOW_AT.store(NO_SHADOW, Ordering::Relaxed);
    // The frame's GPU particle systems, published before the frame graph
    // plans this scene's slot 9 (docs/graphics/native-particles.md).
    crate::native_particle_walk::frame_end();
    let sealed = with_step(|step| {
        let id = step.builder.scene()?.id;
        if id.map_epoch != crate::native_map_work::generation() {
            return None;
        }
        let tick = id.tick;
        let ticks_run = step
            .tick
            .map_or(0, |t| tick.wrapping_sub(t).min(u32::from(u8::MAX)) as u8);
        // The visual effect parts the sim made since the last scene
        // (docs/engine/effects-parts.md).
        let _ = step.builder.push_part_events(sim_effects::scene::take().into_iter().map(part_event));
        let scene = step.builder.seal(Stamp {
            published: Some(Instant::now()),
            ticks_run,
        })?;
        step.tick = Some(tick);
        // Publish under the same lock that retires the open and sealed scenes.
        // A slow observer may return after a map change.
        let old = LAST
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(Arc::clone(&scene));
        Some((step.sequence, scene, old, step.builder.free_list().clone()))
    });
    let Some((sequence, scene, old, free)) = sealed else {
        return;
    };
    SEALED.fetch_add(1, Ordering::Relaxed);
    let observer = *OBSERVER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(o) = observer {
        (o.sealed)(sequence, scene);
    }
    if let Some(old) = old {
        free.recycle(old);
    }
}

fn reset_timeline(why: Cut) {
    with_step(|step| {
        if step.builder.is_building() {
            UNSEALED.fetch_add(1, Ordering::Relaxed);
        }
        step.builder.discard(why);
        step.visibility_lens = VisibilityLens::Retail;
        step.tick = None;
        SHADOW_AT.store(NO_SHADOW, Ordering::Relaxed);
        LAST.lock().unwrap_or_else(|e| e.into_inner()).take();
    });
    let observer = *OBSERVER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(o) = observer {
        (o.reset)();
    }
}

/// Map activation invalidates renderer pins even before its first frame.
pub(crate) fn map_changed() {
    reset_timeline(Cut::MAP);
    visibility_arms_new_map();
}

/// Snapshot restoration also abandons any guest entry/slot scopes.
pub fn forget() {
    reset_timeline(Cut::LOAD);
    with_step(|step| {
        step.presentation = None;
        PRESENT_READY.store(false, Ordering::Release);
        step.visibility.forget();
    });
    *VISIBILITY_FIRST_MISMATCH.lock().unwrap_or_else(|e| e.into_inner()) = None;
    visibility_arms_new_map();
    *LIGHT_VISIBILITY_FIRST_MISMATCH.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *VIEW_RECORD_FIRST_MISMATCH.lock().unwrap_or_else(|e| e.into_inner()) = None;
    crate::entry_census::forget();
    crate::effects_shadow::forget();
    crate::residual_census::forget();
    crate::carried_slots::forget();
    crate::native_hud::ui_overlay::forget();
}

/// Rerun the portal flood with the display lens, keeping only geometry
/// absent from both the retail projection and the actual guest lists.
fn display_additions(
    mem: &Ro<'_>, gather: &frame_scene::scene::ClusterGather,
    factor: f32, cache: &mut scene_extract::visibility::GraphCache,
    retail: &scene_extract::visibility::frusta::WindowFrusta,
    baseline: &scene_extract::visibility::VisibilitySets,
    origin: scene_extract::visibility::Origin,
) -> Option<frame_scene::DisplayVisibility> {
    if gather.mode != 0 || origin.mode != 0 {
        return None;
    }
    let (mut visible, windows) = scene_extract::visibility::portal_walk::scene_visibility(
        mem, cache, crate::native_map_work::bsp_generation(), factor, origin,
    ).ok()?;
    let placements = cache.display_placements(mem, retail, &windows).unwrap_or_default();
    visible.retain_display_additions(baseline, gather);
    let refused = keep_complete_clusters(&mut visible.clusters, |id| crate::native_world::display_cluster_complete(mem.0, id));
    VISIBILITY_DISPLAY_INCOMPLETE.fetch_add(refused, Ordering::Relaxed);
    let whole_clusters = visible.clusters.iter().copied().filter(|&id| {
        crate::native_scene_lists::late_cluster_bounds(mem.0, id)
            .is_some_and(|bounds| windows.contains_cluster(id, bounds))
    }).collect();
    (!visible.clusters.is_empty() || !visible.objects.is_empty() || !placements.is_empty()).then_some(frame_scene::DisplayVisibility {
        whole_clusters, clusters: visible.clusters, objects: visible.objects, placements, guest_lens: false,
    })
}

/// Whether the object `handle` is in the gather lens's set of the open
/// game view (docs/graphics/high-resolution.md, "Determinism"). `None`
/// when the guest's frustum is the gather lens itself — every window no
/// wider than it, where each listed object is in the set — when the guest
/// camera took no fit, or when the set's visibility declined. In a wider
/// window the guest's object setup (`native_scene_lists::setup`) leaves
/// an object outside the set undrawn, so the setup's pose evaluation,
/// animation selection and render-state writes run for the same objects
/// in every window; the display supplement draws the rest. `None` too
/// inside a menu model view, whose camera is not the view record's.
pub(crate) fn gather_lens_object(mem: &dyn GuestMemory, handle: u32) -> Option<bool> {
    if IN_MODEL_VIEW.load(Ordering::Relaxed) {
        return None;
    }
    with_step(|step| {
        let VisibilityLens::Guest { frustum, gather, .. } = step.visibility_lens else { return None };
        if !window_exceeds_gather(frustum, gather) {
            return None;
        }
        let m = Ro(mem);
        let key = (m.read_u32(FRAME)?, m.read_u32(VIEW_INDEX)?);
        if step.gathered.as_ref().is_none_or(|g| g.key != key) {
            let objects = gather_lens_objects(&m, &mut step.visibility, gather);
            if objects.is_none() {
                GUEST_LENS_GATHER_DECLINED.fetch_add(1, Ordering::Relaxed);
            }
            step.gathered = Some(GatherObjects { key, objects, skipped: Vec::new() });
        }
        let gathered = step.gathered.as_mut()?;
        let inside = gathered.objects.as_ref()?.binary_search(&handle).is_ok();
        if !inside {
            GUEST_LENS_OBJECTS_SKIPPED.fetch_add(1, Ordering::Relaxed);
            gathered.skipped.push(handle);
        }
        Some(inside)
    })
}

/// Whether the guest's frustum at `frustum` holds more than the gather
/// lens at `gather`: the window is the wider, so the guest lists objects
/// outside the gather lens's set.
fn window_exceeds_gather(frustum: f32, gather: f32) -> bool {
    frustum < gather
}

/// The gather lens's object set of the current game view: the Rust
/// visibility (docs/graphics/native-visibility.md) at the view record's
/// retail camera widened by `gather`, which depends on the simulation's
/// state and the view alone, never on the window.
fn gather_lens_objects(
    m: &Ro<'_>, cache: &mut scene_extract::visibility::GraphCache, gather: f32,
) -> Option<Vec<u32>> {
    let camera = extract::camera(m)?;
    let (rust, _) = scene_extract::visibility::view::build(m, &camera).ok()?;
    let lens = native_render::camera::display_camera(rust.camera, gather);
    let origin = scene_extract::visibility::Origin::of(&rust);
    let mut objects = scene_extract::visibility::compute(m, &lens, origin, cache, crate::native_map_work::bsp_generation())
        .ok()?.objects;
    objects.sort_unstable();
    Some(objects)
}

/// The objects the open view's guest setup left undrawn, for the display
/// supplement: sorted, once.
fn take_guest_lens_objects(step: &mut Step, key: (u32, u32)) -> Vec<u32> {
    let Some(gathered) = step.gathered.as_mut().filter(|g| g.key == key) else { return Vec::new() };
    let mut objects = std::mem::take(&mut gathered.skipped);
    objects.sort_unstable();
    objects.dedup();
    objects
}

/// Keep the display-only clusters the supplement draws in every layer
/// they use (`native_world::display_cluster_complete`); a cluster it
/// cannot complete, or whose parts do not read, is left out whole, with
/// its water edge and fog. Returns how many were left out.
fn keep_complete_clusters(clusters: &mut Vec<u16>, complete: impl Fn(u16) -> Option<bool>) -> u64 {
    let before = clusters.len();
    clusters.retain(|&id| complete(id) == Some(true));
    (before - clusters.len()) as u64
}

/// The main view's visibility, walked from the Rust view `record`
/// ([`view_record_arm`]) and audited by its shadow arms
/// (docs/graphics/native-visibility.md, "The shadow arms"): clusters,
/// window frusta and objects. A sampled frame (one in
/// [`shadow::SAMPLE_EVERY`]) computes the Rust sets and compares each part
/// with the guest's: the cluster and object sets with `guest`'s lists, the
/// frusta with the guest's region windows (mode 0). An arm answers its
/// part after [`shadow::PROVEN_AFTER`] clean samples while
/// `impl.render.native.visibility` is on, and falls back to the guest for
/// the map on any difference. A frame no arm needs computes nothing,
/// unless the display supplement wants the retail sets. A refusal (no
/// record: `scene_visibility_declined_record`; or the computation's
/// decline) keeps the guest's lists for that frame only; a view whose
/// lists did not read carries nothing. The frusta returned are the Rust
/// ones when that arm answers, else the guest's, read from its region
/// windows. `arms` are the main view's ([`VISIBILITY_ARMS`]).
///
/// [`shadow::SAMPLE_EVERY`]: scene_extract::visibility::shadow::SAMPLE_EVERY
/// [`shadow::PROVEN_AFTER`]: scene_extract::visibility::shadow::PROVEN_AFTER
fn visibility_arm(
    m: &Ro<'_>,
    record: Option<&scene_extract::visibility::view::ViewRecord>,
    key: ViewKey,
    cache: &mut scene_extract::visibility::GraphCache,
    guest: Option<frame_scene::scene::ClusterGather>,
    display_factor: Option<f32>,
    arms: &mut VisibilityArms,
) -> (
    Option<frame_scene::scene::ClusterGather>,
    Option<frame_scene::DisplayVisibility>,
    Option<scene_extract::visibility::frusta::WindowFrusta>,
) {
    use scene_extract::visibility::shadow::{self, Kind, Plan};
    use scene_extract::visibility::{self, Decline, frusta};
    let on = arms.on;
    let arms = &mut arms.shadow;
    let frame = m.read_u32(FRAME).unwrap_or(0);
    let sampled = shadow::sampled(frame);
    let plans = arms.map(|a| a.plan(frame, on));
    let guest_mode0 = guest.as_ref().is_some_and(|g| g.mode == 0);
    // The frusta arm counts only the views that have window frusta: the
    // record's mode 0, or the guest's when the record refused.
    let mode0 = record.map_or(guest_mode0, |r| r.mode == 0);
    let applies = |kind: Kind| kind != Kind::Frusta || mode0;
    let guest_frusta = || if guest_mode0 { visibility::guest::window_frusta(m).ok() } else { None };
    if plans.iter().all(|&p| p == Plan::Guest) && display_factor.is_none() {
        for kind in Kind::ALL.into_iter().filter(|&k| applies(k)) {
            arms[kind as usize].unsampled(Plan::Guest);
        }
        let windows = guest_frusta();
        return (guest, None, windows);
    }
    // No Rust record: the guest answers this frame, counted.
    let Some(record) = record else {
        VISIBILITY_DECLINED_RECORD.fetch_add(1, Ordering::Relaxed);
        for kind in Kind::ALL.into_iter().filter(|&k| applies(k)) {
            arms[kind as usize].declined();
        }
        return (guest, None, guest_frusta());
    };
    let origin = visibility::Origin::of(record);
    let start = Instant::now();
    let epoch = crate::native_map_work::bsp_generation();
    let rust = if mode0 {
        // The guest's single-copy Hor+ hook has already widened this
        // frustum. Exact-mode display additions widen their own copy.
        visibility::portal_walk::scene_visibility(m, cache, epoch, 1., origin)
            .map(|(sets, windows)| (sets, Some(windows)))
    } else {
        visibility::compute_with_windows(m, &record.camera, origin, cache, epoch)
    };
    VISIBILITY_NS.fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    let (rust, windows) = match rust {
        Ok(rust) => rust,
        Err(why) => {
            let i = Decline::ALL.iter().position(|&d| d == why).unwrap_or(0);
            VISIBILITY_DECLINED[i].fetch_add(1, Ordering::Relaxed);
            for kind in Kind::ALL.into_iter().filter(|&k| applies(k)) {
                arms[kind as usize].declined();
            }
            let windows = guest_frusta();
            return (guest, None, windows);
        }
    };
    let Some(guest) = guest else {
        for kind in [Kind::Clusters, Kind::Objects] {
            arms[kind as usize].declined();
        }
        return (None, None, None);
    };
    let extra = display_factor.and_then(|factor| {
        display_additions(m, &guest, factor, cache, windows.as_ref()?, &rust, origin)
    });
    let mut answers = [false; 3];
    if sampled {
        VISIBILITY_CHECKED.fetch_add(1, Ordering::Relaxed);
        let parity = visibility::compare(&rust, &visibility::guest::guest_sets(&guest));
        let clusters_equal = parity.clusters_guest_only + parity.clusters_rust_only == 0;
        let objects_equal = parity.objects_guest_only + parity.objects_rust_only == 0;
        // The frusta compare in mode 0, against the guest's region
        // windows; a side that did not read is the arm's decline.
        let frusta_parity = match (mode0, windows.as_ref()) {
            (false, _) => None,
            (true, Some(rust)) => Some(visibility::guest::window_frusta(m).map(|g| frusta::compare(rust, &g))),
            (true, None) => Some(Err(Decline::Unread)),
        };
        answers[Kind::Clusters as usize] = arms[Kind::Clusters as usize].compared(clusters_equal, on).1;
        answers[Kind::Objects as usize] = arms[Kind::Objects as usize].compared(objects_equal, on).1;
        if let Some(p) = &frusta_parity {
            let arm = &mut arms[Kind::Frusta as usize];
            answers[Kind::Frusta as usize] = match p {
                Ok(p) => arm.compared(p.holds(), on).1,
                Err(_) => arm.declined().1,
            };
        }
        let frusta_differ = frusta_parity.as_ref().and_then(|p| p.as_ref().ok()).filter(|p| !p.holds());
        if parity.first.is_none() && frusta_differ.is_none() {
            VISIBILITY_EQUAL.fetch_add(1, Ordering::Relaxed);
        } else {
            VISIBILITY_DIFFER.fetch_add(1, Ordering::Relaxed);
            let mut slot = VISIBILITY_FIRST_MISMATCH.lock().unwrap_or_else(|e| e.into_inner());
            slot.get_or_insert_with(|| {
                let at = format!("{key} frame {frame} tick {}", tick(m));
                match (parity.first, frusta_differ) {
                    (Some(first), _) => format!(
                        "{at}: {first} ({} clusters and {} objects guest only, {} and {} rust only)",
                        parity.clusters_guest_only,
                        parity.objects_guest_only,
                        parity.clusters_rust_only,
                        parity.objects_rust_only,
                    ),
                    (None, Some(p)) => format!(
                        "{at}: {p} ({} frusta guest only, {} rust only)",
                        p.guest_only, p.rust_only,
                    ),
                    (None, None) => at,
                }
            });
        }
    } else {
        for kind in Kind::ALL.into_iter().filter(|&k| applies(k)) {
            answers[kind as usize] = arms[kind as usize].unsampled(plans[kind as usize]).1;
        }
    }
    let gather = visibility::answer(
        guest,
        rust,
        record.mode,
        answers[Kind::Clusters as usize],
        answers[Kind::Objects as usize],
    );
    if gather.source == frame_scene::VisibilitySource::Rust {
        VISIBILITY_TAKEN.fetch_add(1, Ordering::Relaxed);
    }
    let windows = if !mode0 {
        None
    } else if answers[Kind::Frusta as usize] {
        windows
    } else {
        guest_frusta()
    };
    (Some(gather), extra, windows)
}

/// The main view's shadow arms, in [`Kind::ALL`]'s order
/// (`scene_extract::visibility::shadow`), and
/// `impl.render.native.visibility` (docs/runtime/switches.md): off, the
/// arms compare their samples and never answer.
///
/// [`Kind::ALL`]: scene_extract::visibility::shadow::Kind::ALL
struct VisibilityArms {
    shadow: [scene_extract::visibility::shadow::Shadow; 3],
    on: bool,
}

static VISIBILITY_ARMS: Mutex<VisibilityArms> = Mutex::new(VisibilityArms {
    shadow: [scene_extract::visibility::shadow::Shadow::NEW; 3],
    on: true,
});

fn with_arms<R>(f: impl FnOnce(&mut VisibilityArms) -> R) -> R {
    f(&mut VISIBILITY_ARMS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Set from the switch when the scene-list feature installs.
pub(crate) fn set_native_visibility(on: bool) {
    with_arms(|arms| arms.on = on);
}

/// A new map, or a snapshot restore: every arm learns again.
fn visibility_arms_new_map() {
    with_arms(|arms| arms.shadow.iter_mut().for_each(|a| a.new_map()));
    crate::native_scene_lists::view_records::new_map();
    crate::native_scene_lists::visibility_setup::new_map();
    engine::light_frusta::arm::new_map();
}

/// `render.scene`'s counters of each arm, in [`Kind::ALL`]'s order: its
/// views by outcome, and whether it answers (`proven`) or fell back
/// (`fallen`) on this map.
///
/// [`Kind::ALL`]: scene_extract::visibility::shadow::Kind::ALL
const VISIBILITY_ARM_NAMES: [[&str; 7]; 3] = [
    [
        "scene_visibility_clusters_matched",
        "scene_visibility_clusters_differ",
        "scene_visibility_clusters_answered",
        "scene_visibility_clusters_guest",
        "scene_visibility_clusters_declined",
        "scene_visibility_clusters_proven",
        "scene_visibility_clusters_fallen",
    ],
    [
        "scene_visibility_frusta_matched",
        "scene_visibility_frusta_differ",
        "scene_visibility_frusta_answered",
        "scene_visibility_frusta_guest",
        "scene_visibility_frusta_declined",
        "scene_visibility_frusta_proven",
        "scene_visibility_frusta_fallen",
    ],
    [
        "scene_visibility_objects_matched",
        "scene_visibility_objects_differ",
        "scene_visibility_objects_answered",
        "scene_visibility_objects_guest",
        "scene_visibility_objects_declined",
        "scene_visibility_objects_proven",
        "scene_visibility_objects_fallen",
    ],
];

fn visibility_arm_counters() -> Vec<(&'static str, u64)> {
    let arms = with_arms(|arms| arms.shadow);
    arms.iter()
        .zip(VISIBILITY_ARM_NAMES)
        .flat_map(|(a, names)| {
            let c = a.counts;
            names.into_iter().zip([
                c.matched,
                c.differed,
                c.answered,
                c.guest,
                c.declined,
                u64::from(a.proven()),
                u64::from(a.fallen()),
            ])
        })
        .collect()
}

/// What the guest's horizontal projection scale holds in a view the
/// window-fit hook reported fitted, against the Rust record's retail
/// scale: the hook's write reached the camera the guest built
/// (`Applied`), the guest built the retail lens anyway (`Retail`), or
/// neither within a thousandth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuestLensState {
    Applied = 0,
    Retail = 1,
    Other = 2,
}

impl GuestLensState {
    fn of(retail: f32, guest: f32, factor: f32) -> Self {
        let near = |a: f32, b: f32| b != 0.0 && ((a / b) - 1.0).abs() < 1e-3;
        if near(guest, retail * factor) {
            Self::Applied
        } else if near(guest, retail) {
            Self::Retail
        } else {
            Self::Other
        }
    }
}

/// The main view's records in Rust (docs/graphics/native-visibility.md,
/// "The view records"): built from the observer camera, they are what the
/// visibility walks from. Each is compared field by field with the render
/// camera `camera`, the start cluster and the mode, and counted: the
/// audit. `None` on a decline, counted by name: the guest answers that
/// frame's visibility.
fn view_record_arm(
    m: &Ro<'_>,
    camera: &Camera,
    key: ViewKey,
    lens: VisibilityLens,
) -> Option<scene_extract::visibility::view::ViewRecord> {
    use scene_extract::visibility::view::{self, Decline};
    let (mut rust, guest) = match view::build(m, camera) {
        Ok(pair) => pair,
        Err(why) => {
            let i = Decline::ALL.iter().position(|&d| d == why).unwrap_or(0);
            VIEW_RECORD_DECLINED[i].fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    VIEW_RECORD_CHECKED.fetch_add(1, Ordering::Relaxed);
    if let VisibilityLens::Guest { projection: factor, .. } = lens {
        if let (Some(retail), Some(guest)) = (rust.camera.projection_xy, guest.camera.projection_xy) {
            // The term the factor scaled: X below 1, Y above.
            let state = if factor > 1.0 {
                GuestLensState::of(retail[1], guest[1], canvas_fit::vertical_term(factor))
            } else {
                GuestLensState::of(retail[0], guest[0], factor)
            };
            VIEW_RECORD_GUEST_LENS[state as usize].fetch_add(1, Ordering::Relaxed);
        }
        rust.camera = native_render::camera::display_camera(rust.camera, factor);
    }
    let parity = view::compare(&rust, &guest);
    match parity.first {
        None => {
            VIEW_RECORD_EQUAL.fetch_add(1, Ordering::Relaxed);
        }
        Some(first) => {
            VIEW_RECORD_DIFFER.fetch_add(1, Ordering::Relaxed);
            let mut slot = VIEW_RECORD_FIRST_MISMATCH.lock().unwrap_or_else(|e| e.into_inner());
            slot.get_or_insert_with(|| {
                format!(
                    "{key} frame {} tick {}: {first} ({} fields differ)",
                    m.read_u32(FRAME).unwrap_or(0),
                    tick(m),
                    parity.differ,
                )
            });
        }
    }
    Some(rust)
}

/// The first view since the map loaded whose Rust records differed from
/// the guest's (`render.scene`'s `view_record_first_mismatch`).
pub fn view_record_first_mismatch() -> Option<String> {
    VIEW_RECORD_FIRST_MISMATCH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The first view since the map loaded whose Rust visibility differed
/// from the guest's (`render.scene`'s `visibility_first_mismatch`).
pub fn visibility_first_mismatch() -> Option<String> {
    VISIBILITY_FIRST_MISMATCH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The Rust visibility of the light `light` (its datum) beside its guest
/// lists (docs/graphics/native-visibility.md): at its passes' entry the
/// light's struct `0x0054aa68` holds what its gather `0x001320f0` listed,
/// after the extra caster and before the next light's gather. Computed,
/// compared as sets and counted; nothing goes into the scene.
fn light_visibility_arm(mem: &dyn GuestMemory, call: &engine::light_constants::Call) {
    use scene_extract::visibility::{Decline, light as light_visibility};
    let light = call.light;
    if light == frame_scene::NO_DATUM {
        return;
    }
    let m = Ro(mem);
    let start = Instant::now();
    let parity = with_step(|step| {
        light_visibility::light_arm(
            &m,
            &mut step.visibility,
            crate::native_map_work::bsp_generation(),
            light,
            call.frame,
            call.context,
        )
    });
    LIGHT_VISIBILITY_NS.fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    let parity = match parity {
        Ok(parity) => parity,
        Err(why) => {
            let i = Decline::ALL.iter().position(|&d| d == why).unwrap_or(0);
            LIGHT_VISIBILITY_DECLINED[i].fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    LIGHT_VISIBILITY_CHECKED.fetch_add(1, Ordering::Relaxed);
    let Some(first) = parity.first else {
        LIGHT_VISIBILITY_EQUAL.fetch_add(1, Ordering::Relaxed);
        return;
    };
    LIGHT_VISIBILITY_DIFFER.fetch_add(1, Ordering::Relaxed);
    let mut slot = LIGHT_VISIBILITY_FIRST_MISMATCH
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    slot.get_or_insert_with(|| {
        format!(
            "light {light:#010x} frame {} tick {}: {first} ({} clusters and {} objects guest only, {} and {} rust only)",
            m.read_u32(FRAME).unwrap_or(0),
            tick(&m),
            parity.clusters_guest_only,
            parity.objects_guest_only,
            parity.clusters_rust_only,
            parity.objects_rust_only,
        )
    });
}

/// The first light since the map loaded whose Rust visibility differed
/// from its guest lists.
pub fn light_visibility_first_mismatch() -> Option<String> {
    LIGHT_VISIBILITY_FIRST_MISMATCH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// The light views' counters, for `render.scene`.
fn light_visibility_counters() -> impl Iterator<Item = (&'static str, u64)> {
    [
        (
            "scene_light_visibility_checked",
            LIGHT_VISIBILITY_CHECKED.load(Ordering::Relaxed),
        ),
        (
            "scene_light_visibility_equal",
            LIGHT_VISIBILITY_EQUAL.load(Ordering::Relaxed),
        ),
        (
            "scene_light_visibility_differ",
            LIGHT_VISIBILITY_DIFFER.load(Ordering::Relaxed),
        ),
        (
            "scene_light_visibility_ns",
            LIGHT_VISIBILITY_NS.load(Ordering::Relaxed),
        ),
    ]
    .into_iter()
    .chain(
        LIGHT_VISIBILITY_DECLINED_NAMES.into_iter().zip(
            LIGHT_VISIBILITY_DECLINED
                .iter()
                .map(|c| c.load(Ordering::Relaxed)),
        ),
    )
}

/// The scene sealed last.
pub fn last() -> Option<Arc<FrameScene>> {
    LAST.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|s| s.id.map_epoch == crate::native_map_work::generation())
        .cloned()
}

/// The step's counters, for `render.scene`.
pub fn counters() -> Vec<(&'static str, u64)> {
    vec![
        ("scenes_sealed", SEALED.load(Ordering::Relaxed)),
        ("scenes_unsealed", UNSEALED.load(Ordering::Relaxed)),
        ("scene_views", VIEWS.load(Ordering::Relaxed)),
        ("scene_no_camera", NO_CAMERA.load(Ordering::Relaxed)),
        ("scene_refused_writes", REFUSED.load(Ordering::Relaxed)),
        ("scene_layers", LAYERS.load(Ordering::Relaxed)),
        ("scene_layer_items", LAYER_ITEMS.load(Ordering::Relaxed)),
        (
            "scene_layer_remainder",
            LAYER_REMAINDER.load(Ordering::Relaxed),
        ),
        ("scene_layers_unread", LAYER_UNREAD.load(Ordering::Relaxed)),
        (
            "scene_decorator_draws",
            DECORATOR_DRAWS.load(Ordering::Relaxed),
        ),
        (
            "scene_decorator_instances",
            DECORATOR_INSTANCES.load(Ordering::Relaxed),
        ),
        (
            "scene_decorator_remainder",
            DECORATOR_REMAINDER.load(Ordering::Relaxed),
        ),
        ("scene_decal_items", DECAL_ITEMS.load(Ordering::Relaxed)),
        ("scene_decal_quads", DECAL_QUADS.load(Ordering::Relaxed)),
        ("scene_decal_remainder", DECAL_REMAINDER.load(Ordering::Relaxed)),
        ("scene_weather_plates", WEATHER_PLATES.load(Ordering::Relaxed)),
        ("scene_weather_unread", WEATHER_UNREAD.load(Ordering::Relaxed)),
        (
            "scene_light_extract_views",
            LIGHT_EXTRACT_VIEWS.load(Ordering::Relaxed),
        ),
        (
            "scene_light_extract_ns",
            LIGHT_EXTRACT_NS.load(Ordering::Relaxed),
        ),
        (
            "scene_light_extract_unread",
            LIGHT_EXTRACT_UNREAD.load(Ordering::Relaxed),
        ),
        ("scene_light_groups", LIGHT_GROUPS.load(Ordering::Relaxed)),
        (
            "scene_light_groups_whole",
            LIGHT_GROUPS_WHOLE.load(Ordering::Relaxed),
        ),
        ("scene_light_entries", LIGHT_ENTRIES.load(Ordering::Relaxed)),
        (
            "scene_light_layers_alone",
            crate::native_lights::drawn_alone(),
        ),
        (
            "scene_light_objects_alone",
            crate::native_lights::objects_drawn_alone(),
        ),
        (
            "scene_host_lights_drawn",
            crate::native_lights::host::drawn(),
        ),
        (
            "scene_host_lights_kept",
            crate::native_lights::host::kept(),
        ),
        (
            "scene_host_lights_unread",
            crate::native_lights::host::unread(),
        ),
        (
            "scene_e3_lights_drawn",
            crate::native_lights::chosen::drawn(),
        ),
        (
            "scene_e3_lights_kept",
            crate::native_lights::chosen::kept(),
        ),
        (
            "scene_e3_lights_unread",
            crate::native_lights::chosen::unread(),
        ),
        (
            "scene_e3_lights_shadowed",
            crate::native_lights::chosen::shadowed(),
        ),
        (
            "scene_e3_first_person_lights_drawn",
            crate::native_lights::chosen::first_person::drawn(),
        ),
        (
            "scene_e3_first_person_lights_kept",
            crate::native_lights::chosen::first_person::kept(),
        ),
        (
            "scene_e3_first_person_lights_unread",
            crate::native_lights::chosen::first_person::unread(),
        ),
        (
            "scene_e3_first_person_lights_far",
            crate::native_lights::chosen::first_person::far(),
        ),
        (
            "scene_e3_first_person_parts",
            crate::native_lights::chosen::first_person::parts_offered(),
        ),
        (
            "scene_e3_first_person_parts_left",
            crate::native_lights::chosen::first_person::parts_left(),
        ),
        (
            "scene_first_person_light_layers_alone",
            crate::native_lights::first_person_drawn_alone(),
        ),
        (
            "scene_shadow_groups",
            SHADOW_GROUPS
                .iter()
                .map(|c| c.load(Ordering::Relaxed))
                .sum(),
        ),
        (
            "scene_shadow_groups_whole",
            SHADOW_GROUPS_WHOLE
                .iter()
                .map(|c| c.load(Ordering::Relaxed))
                .sum(),
        ),
        (
            "scene_shadow_groups_10",
            SHADOW_GROUPS[0].load(Ordering::Relaxed),
        ),
        (
            "scene_shadow_groups_whole_10",
            SHADOW_GROUPS_WHOLE[0].load(Ordering::Relaxed),
        ),
        (
            "scene_shadow_groups_11",
            SHADOW_GROUPS[1].load(Ordering::Relaxed),
        ),
        (
            "scene_shadow_groups_whole_11",
            SHADOW_GROUPS_WHOLE[1].load(Ordering::Relaxed),
        ),
        (
            "scene_shadow_groups_14",
            SHADOW_GROUPS[2].load(Ordering::Relaxed),
        ),
        (
            "scene_shadow_groups_whole_14",
            SHADOW_GROUPS_WHOLE[2].load(Ordering::Relaxed),
        ),
        ("scene_fog_views", LATE_VIEWS[0].load(Ordering::Relaxed)),
        ("scene_fog_gate_held", LATE_GATED[0].load(Ordering::Relaxed)),
        ("scene_fog_drawn", LATE_DRAWN[0].load(Ordering::Relaxed)),
        ("scene_fog_parts", LATE_PARTS[0].load(Ordering::Relaxed)),
        ("scene_water_views", LATE_VIEWS[1].load(Ordering::Relaxed)),
        (
            "scene_water_gate_held",
            LATE_GATED[1].load(Ordering::Relaxed),
        ),
        ("scene_water_drawn", LATE_DRAWN[1].load(Ordering::Relaxed)),
        ("scene_water_parts", LATE_PARTS[1].load(Ordering::Relaxed)),
        ("scene_post_views", POST_VIEWS.load(Ordering::Relaxed)),
        ("scene_post_colour", POST_COLOUR.load(Ordering::Relaxed)),
        ("scene_post_effects", POST_EFFECTS.load(Ordering::Relaxed)),
        ("scene_post_unplaced", POST_UNPLACED.load(Ordering::Relaxed)),
        ("scene_visibility_checked", VISIBILITY_CHECKED.load(Ordering::Relaxed)),
        ("scene_visibility_equal", VISIBILITY_EQUAL.load(Ordering::Relaxed)),
        ("scene_visibility_differ", VISIBILITY_DIFFER.load(Ordering::Relaxed)),
        ("scene_visibility_rust_taken", VISIBILITY_TAKEN.load(Ordering::Relaxed)),
        ("scene_visibility_declined_record", VISIBILITY_DECLINED_RECORD.load(Ordering::Relaxed)),
        ("scene_visibility_ns", VISIBILITY_NS.load(Ordering::Relaxed)),
        ("scene_visibility_lens_retail", VISIBILITY_LENS[0].load(Ordering::Relaxed)),
        ("scene_visibility_lens_guest", VISIBILITY_LENS[1].load(Ordering::Relaxed)),
        ("scene_visibility_lens_display", VISIBILITY_LENS[2].load(Ordering::Relaxed)),
        ("scene_visibility_display_views", VISIBILITY_DISPLAY_VIEWS.load(Ordering::Relaxed)),
        ("scene_visibility_display_clusters", VISIBILITY_DISPLAY_CLUSTERS.load(Ordering::Relaxed)),
        ("scene_visibility_display_objects", VISIBILITY_DISPLAY_OBJECTS.load(Ordering::Relaxed)),
        ("scene_visibility_display_incomplete", VISIBILITY_DISPLAY_INCOMPLETE.load(Ordering::Relaxed)),
        ("scene_guest_lens_objects_skipped", GUEST_LENS_OBJECTS_SKIPPED.load(Ordering::Relaxed)),
        ("scene_guest_lens_gather_declined", GUEST_LENS_GATHER_DECLINED.load(Ordering::Relaxed)),
        ("scene_guest_lens_fit_applied", GUEST_FIT[0].load(Ordering::Relaxed)),
        ("scene_guest_lens_fit_networked", GUEST_FIT[1].load(Ordering::Relaxed)),
        ("scene_guest_lens_fit_other_caller", GUEST_FIT[2].load(Ordering::Relaxed)),
        ("scene_guest_lens_fit_unchanged", GUEST_FIT[3].load(Ordering::Relaxed)),
        ("scene_guest_lens_fit_last_caller", GUEST_FIT_LAST_CALLER.load(Ordering::Relaxed)),
        ("scene_guest_lens_ocean_widened", OCEAN_GRID[0].load(Ordering::Relaxed)),
        ("scene_guest_lens_ocean_retail", OCEAN_GRID[1].load(Ordering::Relaxed)),
    ]
    .into_iter()
    .chain(
        VISIBILITY_DECLINED_NAMES
            .into_iter()
            .zip(VISIBILITY_DECLINED.iter().map(|c| c.load(Ordering::Relaxed))),
    )
    .chain(visibility_arm_counters())
    .chain(crate::native_scene_lists::view_records::counters())
    .chain(engine::visibility_setup::arm::counters())
    .chain(engine::light_frusta::arm::counters())
    .chain(scene_extract::decorators::PlacementSource::ALL.map(|source| {
        (source.counter(), DECORATOR_PLACEMENTS[source as usize].load(Ordering::Relaxed))
    }))
    .chain(crate::native_world::geometry_list_counters())
    .chain(light_visibility_counters())
    .chain(crate::native_lights::slot_visibility::counters())
    .chain(
        [
            ("scene_view_record_checked", VIEW_RECORD_CHECKED.load(Ordering::Relaxed)),
            ("scene_view_record_equal", VIEW_RECORD_EQUAL.load(Ordering::Relaxed)),
            ("scene_view_record_differ", VIEW_RECORD_DIFFER.load(Ordering::Relaxed)),
            ("scene_view_record_guest_lens_applied", VIEW_RECORD_GUEST_LENS[0].load(Ordering::Relaxed)),
            ("scene_view_record_guest_lens_retail", VIEW_RECORD_GUEST_LENS[1].load(Ordering::Relaxed)),
            ("scene_view_record_guest_lens_other", VIEW_RECORD_GUEST_LENS[2].load(Ordering::Relaxed)),
        ]
        .into_iter()
        .chain(
            VIEW_RECORD_DECLINED_NAMES
                .into_iter()
                .zip(VIEW_RECORD_DECLINED.iter().map(|c| c.load(Ordering::Relaxed))),
        ),
    )
    .chain(crate::native_decorators::counters())
    .chain(native_render::decal_scene::counters())
    .chain(crate::native_decals::cut::rows())
    .chain(crate::native_decal_decorators::rows())
    .chain(native_render::weather_scene::counters())
    .chain(crate::native_weather::rows())
    .chain(crate::native_shadows::alone_counters())
    .chain(native_render::light_tiles::counters())
    .chain([("scene_first_person_cut_slots", crate::native_lights::first_person_scene_slots())])
    .chain(crate::native_shadows::filter_counters())
    .chain(crate::native_shadows::chain_counters())
    .chain(
        LIGHT_ENTRY_REMAINDER_NAMES.into_iter().zip(
            LIGHT_ENTRY_REMAINDER
                .iter()
                .map(|c| c.load(Ordering::Relaxed)),
        ),
    )
    .collect()
}

/// The observer before a layer draw: the scene's layer 3 opens its view;
/// the scene's layers 1, 2, 5, 7, 8 and 20 (which draw only from the
/// scene, `layer_pass::world_slot`) and the first person's layers put their
/// items in the scene.
fn at_layer(mem: &dyn GuestMemory, ret: u32) {
    let Ok(layer) = mem.u32(CURRENT_LAYER) else {
        return;
    };
    match (layer_site(layer, ret), layer) {
        (Site::Scene, LIGHTMAP_LAYER) => {
            if open_kind_at_layer(mem, ViewKind::Game, true).is_some() {
                decorators(mem);
                decals_and_weather(mem);
            }
        }
        (Site::Scene, 1 | 2 | 5 | 7 | 8 | 20) => layer_items(mem, ViewKind::Game, layer),
        (Site::AlternateView, LIGHTMAP_LAYER) => {
            let _ = open_kind_at_layer(mem, ViewKind::Alternate, true);
        }
        (Site::AlternateView, 1 | 2 | 5) => layer_items(mem, ViewKind::Alternate, layer),
        (Site::FirstPerson, _) => layer_items(mem, ViewKind::FirstPerson, layer),
        _ => {}
    }
}

#[derive(Default)]
pub struct FrameSceneStep {
    pub present: Option<Arc<dyn PresentObserver>>,
}

impl Feature for FrameSceneStep {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "The frame step's scene: camera, world and objects extracted for the native renderer",
        )
        .doc("docs/graphics/framescene.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Off with the native renderer: no hooks, nothing held. On: the
        // scene being written and the last one sealed, both rebuilt every
        // frame from guest memory; a load marks the next one a cut.
        if !hooks::switches::native_render_on(b.switches()) {
            b.snapshot(hooks::SnapshotPolicy::Stateless);
            return Ok(());
        }
        b.snapshot(hooks::SnapshotPolicy::section(Vec::new, |_, _| {
            forget();
            Ok(())
        }));
        // Context-only like the layer-draw observer: it reads guest
        // memory and writes only the step's own state, behind its lock.
        let present = self.present.clone();
        b.raw()
            .observe_before(&points::RENDER_VIEWS, "frame_scene_begin", move |o| {
                let g = o.guest();
                begin(&*g, present.as_ref());
            })
            .context_only();
        b.raw()
            .observe_before(&points::LAYER_DRAW, "frame_scene_view", |o| {
                let g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                at_layer(&*g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .context_only();
        // The light candidates once the light pass is over: before the
        // decal mode 1 call, the next call after `render_dynamic_lights`
        // returns, with nothing between them. A before there, not an
        // after on the lights, keeps the lights' chain on the render
        // thread. Render-timing's scene-call observer on the same point
        // goes first, like at the layer draw.
        b.raw()
            .observe_before(
                &points::RENDER_DECAL_MODE,
                "frame_scene_light_candidates",
                |o| {
                    let mut g = o.guest();
                    if g.u32(o.cpu.esp).ok() != Some(DECALS_AFTER_LIGHTS) {
                        return;
                    }
                    let fpu = engine::light_constants::Fpu {
                        mxcsr: o.cpu.mxcsr,
                        x87_control: o.cpu.x87.control,
                    };
                    let lights = light_candidates(&*g, fpu);
                    // The host's own lights and E3's chosen lights have no
                    // guest passes: the light passes' owner draws their
                    // layers here, where the pass is over and the view's
                    // target is still bound.
                    let mem: &mut dyn GuestMemory = &mut *g;
                    if !lights.host.is_empty() {
                        crate::native_lights::host::draw(mem, &lights.host, fpu);
                    }
                    crate::native_lights::chosen::draw(mem, &lights.chosen, lights.unread);
                    // The cut frame's dynamic lights drew at their own
                    // passes (`native_lights::scene_frame`).
                },
            )
            .after_feature(crate::render_timing::ID)
            .context_only();
        // The view's post passes: `render_screen_effects`' two stack
        // bytes (`late`, `colour`). Its callees' calls are put in the
        // scene by `native_post`'s hooks.
        b.raw()
            .observe_before(&points::RENDER_SCREEN_EFFECTS, "frame_scene_post", |o| {
                let g = o.guest();
                let byte = |n: u32| {
                    o.cpu
                        .esp
                        .checked_add(4 + 4 * n)
                        .and_then(|a| g.u32(a).ok())
                        .is_some_and(|w| w as u8 != 0)
                };
                let (late, colour) = (byte(0), byte(1));
                post(&*g, late, colour);
            })
            .context_only()
            .optional();
        b.raw()
            .observe_after(&points::RENDER_FRAME, "frame_scene_seal", |_| seal());
        // The census of the entries slices 4 and 5 replace: an option,
        // since its exit observers give those entries an after part.
        if b.switches().feature(crate::entry_census::ID, false) {
            crate::entry_census::install(b);
            // render.residual's producers ride the same option
            // (docs/graphics/retire-d3d.md, "New method: render.residual").
            crate::residual_census::install(b);
        }
        // R2's shadow arm: the sorted dispatch's state made in Rust and
        // compared with the guest's (docs/graphics/retire-d3d.md, "R2's
        // shadow arm"); on unless `frame-scene.effects-shadow=off`.
        if b.switches().feature(crate::effects_shadow::ID, true) {
            crate::effects_shadow::install(b);
        }
        // The slot of the frame the guest's D3D work is in, for the
        // device's scene sink (slice 6): on unless `frame-scene.sink=off`,
        // since a paced display's frame owner draws from it.
        if b.switches().feature(crate::carried_slots::ID, true) {
            crate::carried_slots::install(b);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fitted_view_tells_a_widened_guest_scale_from_a_retail_one() {
        let factor = 0.758;
        assert_eq!(GuestLensState::of(1.2, 1.2 * factor, factor), GuestLensState::Applied);
        assert_eq!(GuestLensState::of(1.2, 1.2, factor), GuestLensState::Retail);
        assert_eq!(GuestLensState::of(1.2, 0.6, factor), GuestLensState::Other);
        assert_eq!(GuestLensState::of(0.0, 0.0, factor), GuestLensState::Other);
    }

    #[test]
    fn an_object_in_a_menu_model_view_is_never_skipped_by_the_lens() {
        let mut mem = engine::testing::FakeGuest::new();
        mem.map(FRAME & !0xfff, 0x1000).map(VIEW_INDEX & !0xfff, 0x1000)
            .put_u32(FRAME, 4).put_u32(VIEW_INDEX, 0);
        with_step(|step| {
            step.visibility_lens = VisibilityLens::Guest { projection: 0.7, frustum: 0.8, gather: 1.0 };
            step.gathered = Some(GatherObjects { key: (4, 0), objects: Some(vec![1]), skipped: Vec::new() });
        });
        set_model_view(false);
        assert_eq!(gather_lens_object(&mem, 9), Some(false), "a main-view object outside the set");
        set_model_view(true);
        assert_eq!(gather_lens_object(&mem, 9), None, "no lens test in a model view");
        set_model_view(false);
        with_step(|step| *step = Step::default());
    }

    #[test]
    fn choose_profile_arm_parts_are_not_culled_by_the_lens_at_21_by_9() {
        let mut mem = engine::testing::FakeGuest::new();
        mem.map(FRAME & !0xfff, 0x1000).map(VIEW_INDEX & !0xfff, 0x1000)
            .put_u32(FRAME, 7).put_u32(VIEW_INDEX, 0);
        with_step(|step| {
            step.visibility_lens = VisibilityLens::Guest { projection: 0.55, frustum: 0.55, gather: 1.0 };
            step.gathered = Some(GatherObjects { key: (7, 0), objects: Some(vec![1]), skipped: Vec::new() });
        });
        let before = GUEST_LENS_OBJECTS_SKIPPED.load(Ordering::Relaxed);
        set_model_view(true);
        for arm in [20, 21, 22] {
            assert_eq!(gather_lens_object(&mem, arm), None);
        }
        set_model_view(false);
        assert_eq!(GUEST_LENS_OBJECTS_SKIPPED.load(Ordering::Relaxed), before);
        with_step(|step| *step = Step::default());
    }

    #[test]
    fn a_widened_views_undrawn_objects_reach_the_display_once() {
        let mut step = Step::default();
        step.gathered = Some(GatherObjects { key: (4, 0), objects: Some(vec![1, 3]), skipped: vec![9, 5, 9] });
        assert!(take_guest_lens_objects(&mut step, (4, 1)).is_empty(), "another view");
        assert_eq!(take_guest_lens_objects(&mut step, (4, 0)), [5, 9]);
        assert!(take_guest_lens_objects(&mut step, (4, 0)).is_empty());
    }

    #[test]
    fn the_display_supplement_leaves_out_whole_the_clusters_it_cannot_complete() {
        let mut clusters = vec![2, 5, 7, 9];
        let refused = keep_complete_clusters(&mut clusters, |id| match id {
            5 => Some(false),
            9 => None,
            _ => Some(true),
        });
        assert_eq!((clusters, refused), (vec![2, 7], 2));
    }

    /// Only a window wider than the gather lens leaves any object to the
    /// display supplement: in every other window the guest's frustum is
    /// the gather lens and every listed object is set up.
    #[test]
    fn only_a_window_wider_than_the_gather_lens_skips_setups() {
        let gather: f32 = (4.0 / 3.0) / (12.0 / 5.0);
        // The frustum a window takes: the wider of its own lens and the
        // gather lens.
        let frustum = |w: f32, h: f32| gather.min((4.0 / 3.0) / (w / h));
        for (w, h) in [(640.0, 480.0), (1920.0, 1080.0), (3024.0, 1898.0), (3440.0, 1440.0), (3840.0, 1600.0)] {
            assert!(!window_exceeds_gather(frustum(w, h), gather), "{w}x{h}");
        }
        for (w, h) in [(5120.0, 1440.0), (7680.0, 1440.0)] {
            assert!(window_exceeds_gather(frustum(w, h), gather), "{w}x{h}");
        }
    }

    #[test]
    fn each_lens_counts_under_its_own_name() {
        assert_eq!(
            [
                VisibilityLens::Retail,
                VisibilityLens::Guest { projection: 0.75, frustum: 0.5625, gather: 0.5625 },
                VisibilityLens::Display(0.75),
            ]
                .map(VisibilityLens::counter),
            [0, 1, 2]
        );
    }

    #[test]
    fn worker_view_metadata_survives_the_game_thread_scene_seal() {
        use scene_extract::layout::camera::RENDER_CAMERA;
        use scene_extract::testing::{Mem, camera_record};

        struct Observer;

        impl PresentObserver for Observer {
            fn capture(&self, _: &dyn GuestMemory) -> Option<PresentPayload> {
                Some(Arc::new(19_u32))
            }

            fn horizontal_factor(&self) -> f32 {
                0.75
            }
        }

        let mut record = Mem::default();
        camera_record(&mut record, RENDER_CAMERA);
        let mut guest = engine::FlatMemory::new(RENDER_CAMERA, 0x100);
        for (addr, word) in record.0 {
            guest.set_u32(addr, word).unwrap();
        }
        let observer: Arc<dyn PresentObserver> = Arc::new(Observer);
        begin(&guest, Some(&observer));
        let first = open_kind_at_layer(&guest, ViewKind::Game, true).unwrap();
        seal();
        assert_eq!(last().unwrap().views[0].camera, first.camera);

        // The worker reaches another view draw after the game-thread seal.
        guest.set_u32(RENDER_CAMERA, 7.0_f32.to_bits()).unwrap();
        let next = open_kind_at_layer(&guest, ViewKind::Game, true).unwrap();
        assert_ne!(first.camera, next.camera);
        let image = take_present_image().unwrap();
        let captured = image.views[0].unwrap();
        assert_eq!(captured.camera, next.camera);
        assert_eq!(captured.horizontal_factor, 0.75);
        assert_eq!(image.payload.downcast_ref::<u32>(), Some(&19));
        assert!(take_present_image().is_none());
    }

    #[test]
    fn the_step_is_on_with_the_native_renderer_only() {
        let on = |spec| hooks::switches::native_render_on(&hooks::Switches::parse(spec).unwrap());
        assert!(on(""));
        assert!(!on("impl.render.native=off"));
    }

    #[test]
    fn the_entry_census_is_placed_only_with_its_option() {
        let text = |spec| {
            let mut r = hooks::Registry::new(hooks::Switches::parse(spec).unwrap());
            assert!(r.add(&FrameSceneStep::default()).unwrap());
            r.build(&hooks::plan::Base).unwrap().text()
        };
        let off = text("");
        assert!(!off.contains("entry_census"), "{off}");
        assert!(
            off.contains("frame_scene_post"),
            "the post passes are extracted with or without it\n{off}"
        );
        let on = text("frame-scene.census");
        assert!(!off.contains("residual_census"), "{off}");
        for label in [
            "residual_census_frame",
            "residual_census_frame_end",
            "residual_census_layer",
            "residual_census_layer_end",
            "residual_census_game_view",
            "residual_census_hud_end",
            "residual_census_light_passes_end",
            "residual_census_frame_tail_end",
            "entry_census_layer",
            "entry_census_layer_end",
            "entry_census_light_passes_end",
            "entry_census_object_shadow",
            "entry_census_screen_effects_end",
            "entry_census_colour_pass_end",
            "entry_census_screen_effect_end",
            "entry_census_animation_get",
            "entry_census_particle_update",
        ] {
            assert!(on.contains(label), "{label}\n{on}");
        }
    }

    #[test]
    fn the_carried_slots_are_placed_unless_their_option_is_off() {
        let text = |spec| {
            let mut r = hooks::Registry::new(hooks::Switches::parse(spec).unwrap());
            assert!(r.add(&FrameSceneStep::default()).unwrap());
            r.build(&hooks::plan::Base).unwrap().text()
        };
        let off = text("frame-scene.sink=off");
        assert!(!off.contains("carried_slot"), "{off}");
        let on = text("");
        for label in [
            "carried_slot_frame",
            "carried_slot_frame_end",
            "carried_slot_game_view",
            "carried_slot_game_view_end",
            "carried_slot_lights",
            "carried_slot_lights_end",
            "carried_slot_transparents",
            "carried_slot_post_end",
            "carried_slot_hud",
            "carried_slot_views_end",
        ] {
            assert!(on.contains(label), "{label}\n{on}");
        }
    }

    #[test]
    fn each_light_entry_cause_has_its_counter() {
        for (name, cause) in LIGHT_ENTRY_REMAINDER_NAMES.iter().zip(Cause::ALL) {
            assert_eq!(
                name.strip_prefix("scene_light_entry_remainder_"),
                Some(cause.name())
            );
        }
    }

    /// Guest memory the Rust visibility computes over: the main struct of a
    /// view whose start cluster is outside the structure (`0xffff`), a
    /// structure of no clusters and no portals, empty guest lists, and the
    /// view's player word at none. Its Rust sets are empty and its lists
    /// read; the lists live at `LISTS`.
    fn visibility_memory() -> engine::FlatMemory {
        use engine::render_loop::occlusion::guest::{MAIN_VIS, VIEW_PLAYER, VIS_KIND, VIS_OBJECTS};
        use scene_extract::visibility::graph::layout as structure;
        use scene_extract::visibility::guest::{ACTIVE_STRUCTURE, ACTIVE_VIS, vis};

        const LISTS: u32 = 0x0054_8000;
        const STRUCTURE: u32 = 0x004e_1000;
        const BSP: u32 = 0x004e_1100;
        let (regions, structure_list, object_list) = (LISTS, LISTS + 0x1000, LISTS + 0x2000);

        let mut guest = engine::FlatMemory::new(0x004b_0000, (0x0054_b000 - 0x004b_0000) as usize);
        let mut w = |at: u32, word: u32| guest.set_u32(at, word).unwrap();
        w(ACTIVE_VIS, MAIN_VIS);
        w(MAIN_VIS + VIS_KIND, 0);
        w(MAIN_VIS, regions);
        // One view frustum, no regions and no windows.
        w(regions, 1);
        w(regions + 0xa6c, 0);
        w(regions + 0x1970, 0);
        w(MAIN_VIS + 0xc, structure_list);
        w(structure_list, 64);
        w(structure_list + 4, 0);
        w(MAIN_VIS + VIS_OBJECTS, object_list);
        w(object_list, 64);
        w(object_list + 4, 0);
        w(MAIN_VIS + vis::START_CLUSTER, 0xffff);
        w(MAIN_VIS + vis::FLAGS, 0);
        w(MAIN_VIS + vis::FRUSTA, 1);
        w(MAIN_VIS + vis::MODE, 0);
        w(ACTIVE_STRUCTURE, STRUCTURE);
        w(STRUCTURE + structure::PORTAL_COUNT, 0);
        w(STRUCTURE + structure::CLUSTER_COUNT, 0);
        w(STRUCTURE + structure::COLLISION_BSP, BSP);
        w(BSP + structure::BSP_PLANE_COUNT, 0);
        w(VIEW_PLAYER, u32::MAX);
        guest
    }

    fn visibility_camera() -> Camera {
        Camera {
            position: [0.0, 0.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: std::f32::consts::FRAC_PI_2,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.0625,
            far: 1024.0,
        }
    }

/// The Rust view record of `visibility_memory`'s view: outside the
    /// structure, mode 0.
    fn visibility_record() -> scene_extract::visibility::view::ViewRecord {
        scene_extract::visibility::view::ViewRecord { camera: visibility_camera(), start: 0xffff, mode: 0 }
    }

        fn guest_lists(visible: Vec<u16>, objects: Vec<u32>) -> frame_scene::scene::ClusterGather {
        frame_scene::scene::ClusterGather {
            mode: 0,
            visible,
            structure: vec![5],
            objects,
            source: frame_scene::VisibilitySource::Guest,
        }
    }

    /// The shadow arms beside the guest's lists: the guest answers while
    /// they learn, a sampled frame compares, the arms answer after their
    /// clean samples, a difference falls one back to the guest, the switch
    /// off never answers, a view whose lists did not read carries nothing,
    /// and a decline keeps the guest's lists.
    #[test]
    fn the_shadow_arms_answer_after_clean_samples_and_fall_back_on_a_difference() {
        use scene_extract::visibility::shadow::{Kind, PROVEN_AFTER, SAMPLE_EVERY, Shadow};
        use frame_scene::VisibilitySource::{Guest, Rust};
        let key = ViewKey { kind: ViewKind::Game, player: 0, view: 0 };
        let record = visibility_record();
        let mut cache = scene_extract::visibility::GraphCache::default();
        let mut arms = VisibilityArms { shadow: [Shadow::NEW; 3], on: true };
        let mut guest = visibility_memory();
        let at = |guest: &mut engine::FlatMemory, frame: u32| {
            guest.set_u32(engine::render_loop::occlusion::guest::FRAME, frame).unwrap();
        };
        let listed = || guest_lists(vec![2], vec![7]);

        // Learning, unsampled: the guest's lists, nothing computed.
        at(&mut guest, 1);
        let (gather, extra, windows) =
            visibility_arm(&Ro(&guest), Some(&record), key, &mut cache, Some(listed()), None, &mut arms);
        assert_eq!(gather, Some(listed()));
        assert!(extra.is_none());
        assert_eq!(windows, Some(Default::default()), "the guest's frusta, read from its regions");
        assert_eq!(arms.shadow[Kind::Clusters as usize].counts.guest, 1);

        // Clean samples prove every arm.
        for k in 0..PROVEN_AFTER as u32 {
            at(&mut guest, k * SAMPLE_EVERY);
            let (gather, ..) = visibility_arm(
                &Ro(&guest), Some(&record), key, &mut cache,
                Some(guest_lists(Vec::new(), Vec::new())), None, &mut arms,
            );
            let source = gather.unwrap().source;
            assert_eq!(source, if k + 1 == PROVEN_AFTER as u32 { Rust } else { Guest });
        }
        assert!(arms.shadow.iter().all(|a| a.proven()), "{:?}", arms.shadow);

        // Proven, unsampled: Rust answers uncompared.
        at(&mut guest, SAMPLE_EVERY + 1);
        let (gather, _, windows) =
            visibility_arm(&Ro(&guest), Some(&record), key, &mut cache, Some(listed()), None, &mut arms);
        let gather = gather.unwrap();
        assert_eq!((gather.source, gather.visible.len(), gather.objects.len()), (Rust, 0, 0));
        assert_eq!(gather.structure, [5], "the structure list stays the guest's");
        assert_eq!(windows, Some(Default::default()));
        assert_eq!(arms.shadow[Kind::Objects as usize].counts.answered, 1);

        // The switch off: the guest's lists.
        arms.on = false;
        let (gather, ..) =
            visibility_arm(&Ro(&guest), Some(&record), key, &mut cache, Some(listed()), None, &mut arms);
        assert_eq!(gather, Some(listed()));
        arms.on = true;

        // A sampled difference in the objects: that arm falls back, the
        // clusters arm keeps answering.
        at(&mut guest, 2 * SAMPLE_EVERY);
        let (gather, ..) = visibility_arm(
            &Ro(&guest), Some(&record), key, &mut cache,
            Some(guest_lists(Vec::new(), vec![7])), None, &mut arms,
        );
        assert_eq!(gather.unwrap().objects, [7], "the guest answers the sample that differed");
        assert!(arms.shadow[Kind::Objects as usize].fallen());
        assert!(arms.shadow[Kind::Clusters as usize].proven());
        at(&mut guest, 2 * SAMPLE_EVERY + 1);
        let (gather, ..) =
            visibility_arm(&Ro(&guest), Some(&record), key, &mut cache, Some(listed()), None, &mut arms);
        let gather = gather.unwrap();
        assert_eq!((gather.source, gather.visible, gather.objects), (Guest, vec![], vec![7]));

        // The lists did not read: nothing carries, and it is counted.
        at(&mut guest, 3 * SAMPLE_EVERY);
        let declined = arms.shadow[Kind::Clusters as usize].counts.declined;
        let (gather, extra, windows) =
            visibility_arm(&Ro(&guest), Some(&record), key, &mut cache, None, None, &mut arms);
        assert!(gather.is_none() && extra.is_none() && windows.is_none());
        assert_eq!(arms.shadow[Kind::Clusters as usize].counts.declined, declined + 1);

        // The struct is not the main view's on empty memory: the decline
        // keeps the guest's lists.
        let empty = engine::FlatMemory::new(0x004b_0000, 0x100);
        let lists = guest_lists(vec![2], vec![7]);
        let (gather, extra, _) =
            visibility_arm(&Ro(&empty), Some(&record), key, &mut cache, Some(lists.clone()), None, &mut arms);
        assert_eq!(gather, Some(lists), "the guest's lists, untouched");
        assert!(extra.is_none());

        // A new map learns again.
        arms.shadow.iter_mut().for_each(|a| a.new_map());
        assert!(arms.shadow.iter().all(|a| !a.proven() && !a.fallen()));
    }

    /// A view whose Rust record refused (a cinematic, an override, no
    /// observer) is counted by name and the guest answers that frame
    /// only: the proven arms stay proven, and the next frame with a record
    /// is Rust's again.
    #[test]
    fn a_refused_view_counts_and_the_guest_answers_that_frame() {
        use scene_extract::visibility::shadow::{Kind, PROVEN_AFTER, SAMPLE_EVERY, Shadow};
        use frame_scene::VisibilitySource::{Guest, Rust};
        let key = ViewKey { kind: ViewKind::Game, player: 0, view: 0 };
        let record = visibility_record();
        let mut cache = scene_extract::visibility::GraphCache::default();
        let mut arms = VisibilityArms { shadow: [Shadow::NEW; 3], on: true };
        let mut guest = visibility_memory();
        let at = |guest: &mut engine::FlatMemory, frame: u32| {
            guest.set_u32(engine::render_loop::occlusion::guest::FRAME, frame).unwrap();
        };
        for k in 0..PROVEN_AFTER as u32 {
            at(&mut guest, k * SAMPLE_EVERY);
            visibility_arm(
                &Ro(&guest), Some(&record), key, &mut cache,
                Some(guest_lists(Vec::new(), Vec::new())), None, &mut arms,
            );
        }
        assert!(arms.shadow.iter().all(|a| a.proven()));

        let listed = || guest_lists(vec![2], vec![7]);
        let refused = VISIBILITY_DECLINED_RECORD.load(Ordering::Relaxed);
        let declined = arms.shadow[Kind::Clusters as usize].counts.declined;
        at(&mut guest, SAMPLE_EVERY + 1);
        let (gather, extra, windows) =
            visibility_arm(&Ro(&guest), None, key, &mut cache, Some(listed()), None, &mut arms);
        assert_eq!(gather, Some(listed()), "the guest's lists answer the refused frame");
        assert_eq!(gather.unwrap().source, Guest);
        assert!(extra.is_none());
        assert_eq!(windows, Some(Default::default()), "the guest's frusta");
        assert!(VISIBILITY_DECLINED_RECORD.load(Ordering::Relaxed) > refused);
        assert_eq!(arms.shadow[Kind::Clusters as usize].counts.declined, declined + 1);
        assert!(arms.shadow.iter().all(|a| a.proven()), "a refusal is not a difference");

        at(&mut guest, SAMPLE_EVERY + 2);
        let (gather, ..) =
            visibility_arm(&Ro(&guest), Some(&record), key, &mut cache, Some(listed()), None, &mut arms);
        assert_eq!(gather.unwrap().source, Rust, "the next frame is Rust's again");
    }
}
