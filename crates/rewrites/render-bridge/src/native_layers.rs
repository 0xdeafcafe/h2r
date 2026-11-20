//! `frame-scene.layers`: the scene's world layers 1, 5, 7, 8 and 20 drawn
//! from the scene's plan instead of the guest's draws under the layer
//! call (docs/graphics/framescene.md, slice 4). An option, with the native
//! renderer on, until `render.compare` holds over the gate corpus.
//!
//! One observer before the layer draw `LAYER_DRAW` (`0x00044550`), as
//! `native_fog`'s:
//!
//! - first it sets back the bits it cleared at the last layer it took,
//!   each only if the item still holds the mask it left;
//! - then, when the current layer is one of [`LAYERS`] and the caller is
//!   the scene's (`native_render::world_layers::layer_site`), it asks the
//!   [`Host`] (boot's glue: the plan, the GPU layer sets and the D3D
//!   device) for the native pass; the host clears the layer's bit on the
//!   items it draws, from the list's first, and the guest draws the rest.
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

/// Whether the stage is placed: its option, with the native renderer on.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches) && switches.feature(ID, false)
}

/// What draws the layers: boot's glue over the plan, the GPU world and
/// the device.
pub trait Host: Send + Sync {
    /// Before `layer` of a scene view (`mem` the guest; `scene` one view
    /// holding the layer's one pass, extracted here, [`extract`]): plans
    /// the layer's items, clears the bit of those it draws over a preview,
    /// reserves the native replacement, applies the masks and commits.
    /// Returns what was cleared for the restore at the next layer draw, or
    /// `None` to leave the layer to the D3D path.
    fn ask(&self, mem: &mut dyn GuestMemory, layer: u32, scene: &FrameScene) -> Option<Cleared>;
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
pub fn extract(mem: &dyn GuestMemory, layer: u8) -> Option<Arc<FrameScene>> {
    let ro = Ro(mem);
    let camera = scene_extract::extract::camera(&ro)?;
    let mut b = SceneBuilder::default();
    b.begin(SceneId::default());
    b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera).ok()?;
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
    if !LAYERS.contains(&layer) || layer_site(layer, ret) != Site::Scene {
        return;
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    ASKED.fetch_add(1, Ordering::Relaxed);
    let Some(scene) = extract(&*mem, layer as u8) else {
        count(layer, "refused_unread", 1);
        return;
    };
    let Some(cleared) = host.ask(mem, layer, &scene) else { return };
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
        Manifest::new(ID, "World layers 1, 5, 7, 8 and 20 drawn from the scene's plan")
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
    fn the_plans_outcomes_count_by_layer_and_cause() {
        count(5, "taken", 3);
        count(5, "refused_object", 2);
        count(7, "taken", 0);
        let c: BTreeMap<String, u64> = counters().into_iter().collect();
        assert!(c["layer_plan_5_taken"] >= 3);
        assert!(c["layer_plan_5_refused_object"] >= 2);
        assert!(!c.contains_key("layer_plan_7_taken"), "nothing counted, no row");
    }
}
