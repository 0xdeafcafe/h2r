//! The frame step's scene (docs/graphics/framescene.md, slice 1): a
//! `FrameScene` begun at `render_views`, a view opened at each base-layer
//! layer 3 with its camera, visible clusters and world state, its objects
//! extracted at the object walk, sealed and published after
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
use frame_scene::{Camera, Cut, FrameScene, SceneBuilder, SceneId, Stamp, View, ViewKey, ViewKind};
use native_render::object_walk::Entry;
use native_render::objects::SCENE_LAYER3_RETURN;
use native_render::skip::{CURRENT_LAYER, LIGHTMAP_LAYER};
use scene_extract::GuestRead;
use scene_extract::extract;

use crate::{BuildError, Builder, Feature, Manifest};

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
}

static STEP: Mutex<Option<Step>> = Mutex::new(None);
static LAST: Mutex<Option<Arc<FrameScene>>> = Mutex::new(None);
static SEALED: AtomicU64 = AtomicU64::new(0);
static UNSEALED: AtomicU64 = AtomicU64::new(0);
static VIEWS: AtomicU64 = AtomicU64::new(0);
static NO_CAMERA: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);

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
    let m = Ro(mem);
    let Some(camera) = extract::camera(&m) else {
        NO_CAMERA.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    let key = ViewKey { kind: ViewKind::Game, player: 0, view: m.read_u32(VIEW_INDEX).unwrap_or(0) as u8 };
    with_step(|step| {
        if !step.builder.is_building() {
            // A render this step did not see begin (no `render_views`
            // observer in this plan): the scene starts here.
            begin_step(step, &m);
        }
        if let Some(v) = step.builder.view().filter(|v| v.key == key && v.camera == camera) {
            return Some(v.clone());
        }
        if step.builder.open_view(key, camera).is_err() {
            REFUSED.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        VIEWS.fetch_add(1, Ordering::Relaxed);
        let clusters = extract::layer_clusters(&m, LIGHTMAP_LAYER).unwrap_or_default();
        let set = step.builder.set_clusters(clusters).and_then(|()| step.builder.set_world(extract::world(&m)));
        if set.is_err() {
            REFUSED.fetch_add(1, Ordering::Relaxed);
        }
        step.builder.view().cloned()
    })
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
    ]
}

/// The observer before a layer draw: a base-layer layer 3 opens its view.
fn at_layer(mem: &dyn GuestMemory, ret: u32) {
    if ret == SCENE_LAYER3_RETURN && mem.u32(CURRENT_LAYER).ok() == Some(LIGHTMAP_LAYER) {
        view(mem);
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
        if !crate::switches::native_render_on(b.switches()) {
            b.snapshot(crate::SnapshotPolicy::Stateless);
            return Ok(());
        }
        b.snapshot(crate::SnapshotPolicy::section(Vec::new, |_, _| {
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_step_is_on_with_the_native_renderer_only() {
        let on = |spec| crate::switches::native_render_on(&crate::Switches::parse(spec).unwrap());
        assert!(on(""));
        assert!(!on("impl.render.native=off"));
    }
}
