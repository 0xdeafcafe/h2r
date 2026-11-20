//! The frame step's scene (docs/graphics/framescene.md, slice 1): a
//! `FrameScene` begun at `render_views`, a view opened at each base-layer
//! layer 3 with its camera, visible clusters and world state, its objects
//! extracted at the object walk, its lights at each light passes' entry
//! (`native_lights`), the items of its world layers 1, 5, 7, 8 and 20 at
//! each one's draw, its model decorators at its layer 3
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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use engine::memory::GuestMemory;
use engine::points;
use engine::render_loop::occlusion::guest::{FRAME, VIEW_INDEX};
use frame_scene::{
    Camera, Cut, DeviceFog, FrameScene, GroupSite, LateInput, LateLayer, LateMaterial, LatePart, LightStep, SceneBuilder, SceneId,
    SceneLight, Stamp, View, ViewKey, ViewKind,
};
use native_render::object_walk::Entry;
use native_render::skip::{CURRENT_LAYER, LIGHTMAP_LAYER};
use native_render::world_layers::{Site, layer_site};
use scene_extract::GuestRead;
use scene_extract::extract;
use scene_extract::light_groups::{self, Cause, Sorted};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "frame-scene";

/// The game-time record's tick count (`[GAME_TIME] + 8`).
const GAME_TIME_TICK: u32 = 8;

/// Guest memory, read only.
struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

#[derive(Default)]
struct Step {
    builder: SceneBuilder,
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
}

static STEP: Mutex<Option<Step>> = Mutex::new(None);
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
static DECORATOR_DRAWS: AtomicU64 = AtomicU64::new(0);
static DECORATOR_INSTANCES: AtomicU64 = AtomicU64::new(0);
static DECORATOR_REMAINDER: AtomicU64 = AtomicU64::new(0);
static LIGHT_GROUPS: AtomicU64 = AtomicU64::new(0);
static LIGHT_GROUPS_WHOLE: AtomicU64 = AtomicU64::new(0);
static LIGHT_ENTRIES: AtomicU64 = AtomicU64::new(0);
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
static LIGHT_ENTRY_REMAINDER: [AtomicU64; Cause::ALL.len()] = [const { AtomicU64::new(0) }; Cause::ALL.len()];
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
    step.builder.begin(id(m));
    step.light_open = false;
    step.shadow_open = false;
}

/// `render_views`' entry: a new scene.
pub fn begin(mem: &dyn GuestMemory) {
    let m = Ro(mem);
    with_step(|step| begin_step(step, &m));
}

/// The open view's camera at a base-layer layer 3, opening the view
/// (camera, visible clusters, world state) when none is open for the
/// current guest view and camera. `None` when the render camera does
/// not read as a camera: the stages leave the view to the guest.
pub fn view(mem: &dyn GuestMemory) -> Option<Camera> {
    open_view(mem).map(|v| v.camera)
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
fn open_kind(mem: &dyn GuestMemory, kind: ViewKind) -> Option<View> {
    let m = Ro(mem);
    let Some(camera) = extract::camera(&m) else {
        NO_CAMERA.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    let key = ViewKey { kind, player: 0, view: m.read_u32(VIEW_INDEX).unwrap_or(0) as u8 };
    let base = m.read_u32(CURRENT_LAYER) == Some(LIGHTMAP_LAYER);
    with_step(|step| {
        if !step.builder.is_building() {
            // A render this step did not see begin (no `render_views`
            // observer in this plan): the scene starts here.
            begin_step(step, &m);
        }
        if step.builder.view().is_none_or(|v| v.key != key || v.camera != camera) {
            if step.builder.open_view(key, camera).is_err() {
                REFUSED.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            VIEWS.fetch_add(1, Ordering::Relaxed);
            step.based = false;
            step.light_open = false;
            step.shadow_open = false;
        }
        if base && !step.based {
            step.based = true;
            let clusters = extract::layer_clusters(&m, LIGHTMAP_LAYER).unwrap_or_default();
            let set = step.builder.set_clusters(clusters).and_then(|()| step.builder.set_world(extract::world(&m)));
            if set.is_err() {
                REFUSED.fetch_add(1, Ordering::Relaxed);
            }
        }
        step.builder.view().cloned()
    })
}

/// The current list's `layer` items into the open view of `kind` (a
/// layer draw of the scene's world layers or of the first person's).
fn layer_items(mem: &dyn GuestMemory, kind: ViewKind, layer: u32) {
    if open_kind(mem, kind).is_none() {
        return;
    }
    let m = Ro(mem);
    let Ok(layer) = u8::try_from(layer) else { return };
    // Every object item takes its owner's lighting: the layer stage's
    // lighting inputs read it, and the lightmap cycle reaches the draw
    // only through it (`frame_scene::LayerInputs`).
    with_step(|step| match extract::layer(&m, layer, true, &mut step.builder) {
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
    });
}

/// The objects of the base layer's sorted walk into the open view (its
/// first geometry handler call). Opens the view first when none is.
pub fn objects(mem: &dyn GuestMemory, entries: &[Entry]) {
    if view(mem).is_none() {
        return;
    }
    let m = Ro(mem);
    with_step(|step| {
        if extract::objects(&m, entries, &mut step.builder).is_err() {
            REFUSED.fetch_add(1, Ordering::Relaxed);
        }
    });
}

/// The model decorators of the base layer 3 into the open view
/// (docs/graphics/native-decorators.md).
fn decorators(mem: &dyn GuestMemory) {
    let m = Ro(mem);
    with_step(|step| match scene_extract::decorators::extract(&m, LIGHTMAP_LAYER as u8, &mut step.builder) {
        Ok(Some(got)) => {
            DECORATOR_DRAWS.fetch_add(u64::from(got.draws), Ordering::Relaxed);
            DECORATOR_INSTANCES.fetch_add(u64::from(got.instances), Ordering::Relaxed);
            DECORATOR_REMAINDER.fetch_add(u64::from(got.remainder), Ordering::Relaxed);
        }
        Ok(None) => {}
        Err(_) => {
            REFUSED.fetch_add(1, Ordering::Relaxed);
        }
    });
}

/// A light of the open view at its passes' entry, with its steps.
/// Opens the view first when none is.
pub fn light(mem: &dyn GuestMemory, light: SceneLight, steps: &[LightStep]) {
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

/// A light's passes begin: until [`light`] puts it in the view, its
/// groups have no light to join.
pub fn light_entry() {
    with_step(|step| {
        step.light_open = false;
        step.shadow_open = false;
    });
}

/// An object shadow `0x00032350` of the open view begins, with its call's
/// two arguments (`identity`) when the caller matched its caster entry;
/// `None` ends the one before without a new one. Opens the view first
/// when none is.
pub fn object_shadow(mem: &dyn GuestMemory, identity: Option<[u32; 2]>) {
    with_step(|step| step.shadow_open = false);
    let Some(identity) = identity else { return };
    if view(mem).is_none() {
        return;
    }
    with_step(|step| {
        step.light_open = false;
        step.shadow_open = step.builder.push_object_shadow(identity).is_ok();
        if !step.shadow_open {
            REFUSED.fetch_add(1, Ordering::Relaxed);
        }
    });
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
        let open = if site == GroupSite::ObjectShadow { step.shadow_open } else { step.light_open };
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
                for (counter, n) in LIGHT_ENTRY_REMAINDER.iter().zip(got.remainder) {
                    counter.fetch_add(u64::from(n), Ordering::Relaxed);
                }
                let scene = step.builder.scene()?;
                let groups = if site == GroupSite::ObjectShadow { &scene.shadow_groups } else { &scene.light_groups };
                let group = groups.last()?.clone();
                if !group.whole() {
                    return None;
                }
                let entries = scene.entries_of(&group).to_vec();
                let rows = entries.iter().map(|e| scene.pose_rows(e).to_vec()).collect();
                Some(crate::native_lights::SceneGroup { group, entries, rows })
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
    let key = ViewKey { kind: ViewKind::Game, player: 0, view: m.read_u32(VIEW_INDEX).unwrap_or(0) as u8 };
    let has = with_step(|step| step.builder.scene().is_some_and(|s| s.views.iter().any(|v| v.key == key)));
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

/// The late layer `layer` (16 the fog, 6 the water) of the view being
/// drawn (docs/graphics/framescene.md, slice 5): whether its gate held,
/// the device's fog words, the items the current list holds in it, and
/// what its stage would draw (`materials`, `parts`), not yet taken
/// ([`late_drawn`]). Returns the scene's copy, which the stage records
/// from; `None` when no view opens or the builder refuses, and then the
/// stage leaves the layer to the guest.
pub fn late_layer(
    mem: &dyn GuestMemory, layer: u8, draws: bool, device: DeviceFog, materials: &[LateMaterial], parts: &[LatePart],
) -> Option<LateStage> {
    let i = late_index(layer)?;
    let key = late_key(mem)?;
    let items = scene_extract::late::layer_items(&Ro(mem), layer);
    with_step(|step| {
        let input = LateInput { draws, device, items, materials, parts };
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
        step.builder.scene().and_then(|s| s.views.iter().rev().find(|v| v.key == key)).is_some_and(|v| v.late(layer).is_some())
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
            let parts = step.builder.scene().and_then(|s| s.views.iter().rev().find(|v| v.key == key)?.late(layer).cloned());
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
    let key = ViewKey { kind: ViewKind::Game, player: 0, view: mem.u32(VIEW_INDEX).unwrap_or(0) as u8 };
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
    let key = ViewKey { kind: ViewKind::Game, player: 0, view: mem.u32(VIEW_INDEX).unwrap_or(0) as u8 };
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

/// After `render_frame_and_present`: seal and publish the scene.
pub fn seal() {
    let sealed = with_step(|step| {
        let tick = step.builder.scene()?.id.tick;
        let ticks_run = step.tick.map_or(0, |t| tick.wrapping_sub(t).min(u32::from(u8::MAX)) as u8);
        let scene = step.builder.seal(Stamp { published: Some(Instant::now()), ticks_run })?;
        step.tick = Some(tick);
        Some((scene, step.builder.free_list().clone()))
    });
    let Some((scene, free)) = sealed else { return };
    SEALED.fetch_add(1, Ordering::Relaxed);
    let old = LAST.lock().unwrap_or_else(|e| e.into_inner()).replace(scene);
    if let Some(old) = old {
        free.recycle(old);
    }
}

/// A snapshot load: the next scene is a cut, the one being written is
/// dropped at the next begin.
pub fn forget() {
    with_step(|step| step.builder.cut(Cut::LOAD));
    crate::entry_census::forget();
}

/// The scene sealed last.
pub fn last() -> Option<Arc<FrameScene>> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
        ("scene_layer_remainder", LAYER_REMAINDER.load(Ordering::Relaxed)),
        ("scene_layers_unread", LAYER_UNREAD.load(Ordering::Relaxed)),
        ("scene_decorator_draws", DECORATOR_DRAWS.load(Ordering::Relaxed)),
        ("scene_decorator_instances", DECORATOR_INSTANCES.load(Ordering::Relaxed)),
        ("scene_decorator_remainder", DECORATOR_REMAINDER.load(Ordering::Relaxed)),
        ("scene_light_groups", LIGHT_GROUPS.load(Ordering::Relaxed)),
        ("scene_light_groups_whole", LIGHT_GROUPS_WHOLE.load(Ordering::Relaxed)),
        ("scene_light_entries", LIGHT_ENTRIES.load(Ordering::Relaxed)),
        ("scene_light_layers_alone", crate::native_lights::drawn_alone()),
        ("scene_fog_views", LATE_VIEWS[0].load(Ordering::Relaxed)),
        ("scene_fog_gate_held", LATE_GATED[0].load(Ordering::Relaxed)),
        ("scene_fog_drawn", LATE_DRAWN[0].load(Ordering::Relaxed)),
        ("scene_fog_parts", LATE_PARTS[0].load(Ordering::Relaxed)),
        ("scene_water_views", LATE_VIEWS[1].load(Ordering::Relaxed)),
        ("scene_water_gate_held", LATE_GATED[1].load(Ordering::Relaxed)),
        ("scene_water_drawn", LATE_DRAWN[1].load(Ordering::Relaxed)),
        ("scene_water_parts", LATE_PARTS[1].load(Ordering::Relaxed)),
        ("scene_post_views", POST_VIEWS.load(Ordering::Relaxed)),
        ("scene_post_colour", POST_COLOUR.load(Ordering::Relaxed)),
        ("scene_post_effects", POST_EFFECTS.load(Ordering::Relaxed)),
        ("scene_post_unplaced", POST_UNPLACED.load(Ordering::Relaxed)),
    ]
    .into_iter()
    .chain(LIGHT_ENTRY_REMAINDER_NAMES.into_iter().zip(LIGHT_ENTRY_REMAINDER.iter().map(|c| c.load(Ordering::Relaxed))))
    .collect()
}

/// The observer before a layer draw: the scene's layer 3 opens its view;
/// the scene's layers 1, 5, 7, 8 and 20 and the first person's layers
/// put their items in the scene.
fn at_layer(mem: &dyn GuestMemory, ret: u32) {
    let Ok(layer) = mem.u32(CURRENT_LAYER) else { return };
    match (layer_site(layer, ret), layer) {
        (Site::Scene, LIGHTMAP_LAYER) => {
            if view(mem).is_some() {
                decorators(mem);
            }
        }
        (Site::Scene, 1 | 5 | 7 | 8 | 20) => layer_items(mem, ViewKind::Game, layer),
        (Site::FirstPerson, _) => layer_items(mem, ViewKind::FirstPerson, layer),
        _ => {}
    }
}

pub struct FrameSceneStep;

impl Feature for FrameSceneStep {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The frame step's scene: camera, world and objects extracted for the native renderer")
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
        // Under impl.render.pipelined_frame the views are written on the
        // render thread and sealed on the game thread after the launch,
        // so a sealed scene there can be the frame before's.
        b.raw()
            .observe_before(&points::RENDER_VIEWS, "frame_scene_begin", |o| {
                let g = o.guest();
                begin(&*g);
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
        // The view's post passes: `render_screen_effects`' two stack
        // bytes (`late`, `colour`). Its callees' calls are put in the
        // scene by `native_post`'s hooks.
        b.raw()
            .observe_before(&points::RENDER_SCREEN_EFFECTS, "frame_scene_post", |o| {
                let g = o.guest();
                let byte = |n: u32| o.cpu.esp.checked_add(4 + 4 * n).and_then(|a| g.u32(a).ok()).is_some_and(|w| w as u8 != 0);
                let (late, colour) = (byte(0), byte(1));
                post(&*g, late, colour);
            })
            .context_only()
            .optional();
        b.raw().observe_after(&points::RENDER_FRAME, "frame_scene_seal", |_| seal());
        // The census of the entries slices 4 and 5 replace: an option,
        // since its exit observers give those entries an after part.
        if b.switches().feature(crate::entry_census::ID, false) {
            crate::entry_census::install(b);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(r.add(&FrameSceneStep).unwrap());
            r.build(&hooks::plan::Base).unwrap().text()
        };
        let off = text("");
        assert!(!off.contains("entry_census"), "{off}");
        assert!(off.contains("frame_scene_post"), "the post passes are extracted with or without it\n{off}");
        let on = text("frame-scene.census");
        for label in [
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
    fn each_light_entry_cause_has_its_counter() {
        for (name, cause) in LIGHT_ENTRY_REMAINDER_NAMES.iter().zip(Cause::ALL) {
            assert_eq!(name.strip_prefix("scene_light_entry_remainder_"), Some(cause.name()));
        }
    }
}
