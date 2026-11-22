//! `frame-scene.layers`: the scene's world layers 1, 5, 7, 8 and 20, and
//! the first person's layers 1, 7 and 8, drawn from the scene's plan
//! instead of the guest's draws under the layer call
//! (docs/graphics/framescene.md, slice 4). On with the native renderer;
//! `frame-scene.layers=off` leaves every layer to the D3D path.
//!
//! One observer before the layer draw `LAYER_DRAW` (`0x00044550`), as
//! `native_fog`'s:
//!
//! - first it sets back the bits it cleared at the last layer it took,
//!   each only if the item still holds the mask it left;
//! - then, when the current layer is one of [`LAYERS`] and the caller is
//!   the scene's, or one of [`FIRST_PERSON_LAYERS`] and the caller the
//!   first person's `0x0003fdf0` (`native_render::world_layers::layer_site`),
//!   it asks the [`Host`] (boot's glue: the plan, the GPU layer sets, the
//!   object models and the D3D device) for the native pass; the host
//!   clears the layer's bit on the items it draws, from the list's first,
//!   and the guest draws the rest.
//!
//! What the host refuses keeps its D3D draws (and so the admission of
//! `native_world_layers`). The plan's outcomes are counted by cause for
//! `render.scene` ([`counters`]).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use frame_scene::{FrameScene, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind};
use native_render::camera::{GuestRead, GuestWrite};
use native_render::skip::{self, Cleared};
use native_render::world_layers::{Site, layer_site};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "frame-scene.layers";

/// The layers taken.
pub const LAYERS: [u32; 5] = [1, 5, 7, 8, 20];

/// The first person's layers taken: `0x0003fdf0`'s calls besides layer
/// 3, which `native_first_person` owns.
pub const FIRST_PERSON_LAYERS: [u32; 3] = [1, 7, 8];

/// Which caller's layer draw is about to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerCaller {
    /// The scene's (`0x0002bcd0`): structure and object items.
    Scene,
    /// The first person's (`0x0003fdf0`): its arms' and weapon's items.
    FirstPerson,
}

/// The caller of `layer` this stage takes, from the layer draw's return
/// address; `None` for every other.
pub fn caller(layer: u32, ret: u32) -> Option<LayerCaller> {
    match layer_site(layer, ret) {
        Site::Scene if LAYERS.contains(&layer) => Some(LayerCaller::Scene),
        Site::FirstPerson if FIRST_PERSON_LAYERS.contains(&layer) => Some(LayerCaller::FirstPerson),
        _ => None,
    }
}

/// Whether the stage is placed: with the native renderer on, unless
/// `frame-scene.layers=off`.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches) && switches.feature(ID, true)
}

/// What draws the layers: boot's glue over the plan, the GPU world and
/// the device.
pub trait Host: Send + Sync {
    /// Before `layer` of a scene view or of the first person (`caller`;
    /// `mem` the guest; `scene` one view holding the layer's one pass,
    /// extracted here, [`extract`]): plans the layer's items, clears the
    /// bit of those it draws over a preview, reserves the native
    /// replacement, applies the masks and commits. Returns what was
    /// cleared for the restore at the next layer draw, or `None` to leave
    /// the layer to the D3D path.
    fn ask(&self, mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller, scene: &FrameScene) -> Option<Cleared>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static PLACED: AtomicBool = AtomicBool::new(false);
static CLEARED: Mutex<Option<(Cleared, Vec<u32>)>> = Mutex::new(None);
static ASKED: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static ITEMS: AtomicU64 = AtomicU64::new(0);
/// The plan's outcomes by cause (`taken`, `refused_<cause>`), per layer.
static OUTCOMES: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// Whether the hook is placed this session: the world's load compiles
/// the layers' materials only then.
pub fn placed() -> bool {
    PLACED.load(Ordering::Relaxed)
}

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Counts `n` items of `layer` under `outcome` (`taken`, or a refusal's
/// cause): the host reports each ask's plan.
pub fn count(layer: u32, outcome: &str, n: u64) {
    if n == 0 {
        return;
    }
    let mut o = OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
    *o.entry(format!("layer_plan_{layer}_{outcome}")).or_default() += n;
}

/// [`count`] for `caller`'s layer: the first person's outcomes under
/// `layer_plan_fp<layer>_<outcome>`.
pub fn count_for(caller: LayerCaller, layer: u32, outcome: &str, n: u64) {
    match caller {
        LayerCaller::Scene => count(layer, outcome, n),
        LayerCaller::FirstPerson => {
            if n == 0 {
                return;
            }
            let mut o = OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
            *o.entry(format!("layer_plan_fp{layer}_{outcome}")).or_default() += n;
        }
    }
}

/// The counters `render.scene` reports: layer calls asked, layer calls
/// drawn natively, items skipped, and the plan's outcomes by layer and
/// cause.
pub fn counters() -> Vec<(String, u64)> {
    let mut out = vec![
        ("layer_plan_asked".to_owned(), ASKED.load(Ordering::Relaxed)),
        ("layer_plan_drawn".to_owned(), DRAWN.load(Ordering::Relaxed)),
        ("layer_plan_items".to_owned(), ITEMS.load(Ordering::Relaxed)),
    ];
    out.extend(OUTCOMES.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, &v)| (k.clone(), v)));
    out
}

/// The layer draw about to run as a scene of its own: one view with the
/// render camera and the layer's pass, its items and material inputs
/// (`scene_extract::extract::layer`, each object item with its owner's
/// lighting). Independent of the frame step's scene, whose observer at
/// the same point may run before or after this one.
pub fn extract(mem: &dyn GuestMemory, layer: u8, caller: LayerCaller) -> Option<Arc<FrameScene>> {
    let ro = Ro(mem);
    let camera = scene_extract::extract::camera(&ro)?;
    let mut b = SceneBuilder::default();
    b.begin(SceneId::default());
    let kind = match caller {
        LayerCaller::Scene => ViewKind::Game,
        LayerCaller::FirstPerson => ViewKind::FirstPerson,
    };
    b.open_view(ViewKey { kind, player: 0, view: 0 }, camera).ok()?;
    scene_extract::extract::layer(&ro, layer, true, &mut b).ok()??;
    b.seal(Stamp::default())
}

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

struct Mem<'a>(&'a mut dyn GuestMemory);

impl GuestRead for Mem<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

impl GuestWrite for Mem<'_> {
    fn write_u32(&mut self, addr: u32, value: u32) -> Option<()> {
        self.0.set_u32(addr, value).ok()
    }
}

/// The observer before the layer draw: the last bits back, then this
/// layer's skip. `ret` is the layer draw's return address.
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    let Some(layer) = Mem(mem).read_u32(skip::CURRENT_LAYER) else { return };
    let Some(caller) = caller(layer, ret) else { return };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    ASKED.fetch_add(1, Ordering::Relaxed);
    let Some(scene) = extract(&*mem, layer as u8, caller) else {
        count_for(caller, layer, "refused_unread", 1);
        return;
    };
    let Some(cleared) = host.ask(mem, layer, caller, &scene) else { return };
    if cleared.items.is_empty() {
        return;
    }
    DRAWN.fetch_add(1, Ordering::Relaxed);
    ITEMS.fetch_add(cleared.items.len() as u64, Ordering::Relaxed);
    let m = Mem(mem);
    let left = cleared.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
    *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some((cleared, left));
}

/// Sets back the bits the last taken layer cleared, on the items that
/// still hold the mask it left.
pub fn restore_pending(mem: &mut dyn GuestMemory) {
    let Some((cleared, left)) = CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let mut m = Mem(mem);
    let bit = 1u32 << cleared.layer;
    for (&item, &was) in cleared.items.iter().zip(&left) {
        if m.read_u32(item + 0x10) == Some(was) {
            let _ = m.write_u32(item + 0x10, was | bit);
        }
    }
}

pub(crate) fn forget_pending() {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

pub struct NativeLayers;

impl Feature for NativeLayers {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "World layers 1, 5, 7, 8 and 20 and the first person's 1, 7 and 8 drawn from the scene's plan")
            .doc("docs/graphics/framescene.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        PLACED.store(true, Ordering::Relaxed);
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_layers_skip", |o| {
                if crate::native_map_work::Stage::World.standing_down() {
                    return;
                }
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                before_layer(&mut *g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::native_world::ID)
            .context_only();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_is_on_with_the_native_renderer_unless_turned_off() {
        let on = |spec: &str| super::on(&hooks::Switches::parse(spec).unwrap());
        assert!(on(""), "on by default");
        assert!(!on("frame-scene.layers=off"));
        assert!(!on("impl.render.native=off"), "the D3D path is the off arm");
    }

    #[test]
    fn the_plans_outcomes_count_by_layer_and_cause() {
        count(5, "taken", 3);
        count(5, "refused_object", 2);
        count(7, "taken", 0);
        let c: BTreeMap<String, u64> = counters().into_iter().collect();
        assert!(c["layer_plan_5_taken"] >= 3);
        assert!(c["layer_plan_5_refused_object"] >= 2);
        assert!(!c.contains_key("layer_plan_7_taken"), "nothing counted, no row");
        count_for(LayerCaller::FirstPerson, 8, "taken", 2);
        let c: BTreeMap<String, u64> = counters().into_iter().collect();
        assert!(c["layer_plan_fp8_taken"] >= 2, "the first person's apart");
    }

    #[test]
    fn the_stage_takes_the_scenes_layers_and_the_first_persons_besides_layer_3() {
        assert_eq!(caller(5, 0x0002_bf39), Some(LayerCaller::Scene));
        assert_eq!(caller(20, 0x0002_c010), Some(LayerCaller::Scene));
        assert_eq!(caller(1, 0x0004_00ab), Some(LayerCaller::FirstPerson));
        assert_eq!(caller(8, 0x0004_01a7), Some(LayerCaller::FirstPerson));
        assert_eq!(caller(7, 0x0004_0271), Some(LayerCaller::FirstPerson));
        assert_eq!(caller(3, 0x0004_0136), None, "native_first_person's");
        assert_eq!(caller(3, 0x0002_bed5), None, "native_objects' and the world's");
        assert_eq!(caller(1, 0x0002_bd5c), None, "an alternate view's");
    }
}
