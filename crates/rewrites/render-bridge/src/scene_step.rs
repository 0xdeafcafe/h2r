//! The frame step's scene (docs/graphics/framescene.md, slice 1): a
//! `FrameScene` begun at `render_views`, a view opened at each base-layer
//! layer 3 with its camera, visible clusters and world state, its objects
//! extracted at the object walk, its lights at each light passes' entry
//! (`native_lights`), the items of its world layers 1, 5, 7, 8 and 20 at
//! each one's draw, and a first-person view with the items of its layers
//! at the first-person draw `0x0003fdf0`, sealed and published after
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
use frame_scene::{Camera, Cut, FrameScene, LightStep, SceneBuilder, SceneId, SceneLight, Stamp, View, ViewKey, ViewKind};
use native_render::object_walk::Entry;
use native_render::skip::{CURRENT_LAYER, LIGHTMAP_LAYER};
use native_render::world_layers::{Site, layer_site};
use scene_extract::GuestRead;
use scene_extract::extract;

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
    with_step(|step| match extract::layer(&m, layer, kind == ViewKind::FirstPerson, &mut step.builder) {
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

/// A light of the open view at its passes' entry, with its steps.
/// Opens the view first when none is.
pub fn light(mem: &dyn GuestMemory, light: SceneLight, steps: &[LightStep]) {
    if view(mem).is_none() {
        return;
    }
    with_step(|step| {
        if step.builder.push_light(light, steps).is_err() {
            REFUSED.fetch_add(1, Ordering::Relaxed);
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
    ]
}

/// The observer before a layer draw: the scene's layer 3 opens its view;
/// the scene's layers 1, 5, 7, 8 and 20 and the first person's layers
/// put their items in the scene.
fn at_layer(mem: &dyn GuestMemory, ret: u32) {
    let Ok(layer) = mem.u32(CURRENT_LAYER) else { return };
    match (layer_site(layer, ret), layer) {
        (Site::Scene, LIGHTMAP_LAYER) => {
            view(mem);
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
        let on = text("frame-scene.census");
        for label in [
            "entry_census_layer",
            "entry_census_layer_end",
            "entry_census_light_passes_end",
            "entry_census_object_shadow",
            "entry_census_screen_effects_end",
            "entry_census_animation_get",
            "entry_census_particle_update",
        ] {
            assert!(on.contains(label), "{label}\n{on}");
        }
    }
}
