//! `native-decorators`: the model decorators of the scene's layer 3 drawn
//! by the native renderer, instanced, instead of the guest's build and
//! per-instance draws (docs/graphics/native-decorators.md). On with the
//! native renderer (`impl.render.native`).
//!
//! One observer before the layer draw `LAYER_DRAW` (`0x00044550`), after
//! the frame scene's (which puts the view's decorators in the open view,
//! `scene_step`) and the native world's (so the decorators' pass is
//! reserved after the world's and draws over it), so nothing here
//! replaces guest code:
//!
//! - first it sets back the bits it cleared at the last layer 3, each only
//!   if the item still holds the mask it left ([`restore_pending`]);
//! - then, at the scene's layer 3 (`world_layers::layer_site`), it asks
//!   the [`Host`] (boot's glue) to have each material of the open view's
//!   decorator draws ready ([`Host::prepare`]: the set's geometry and the
//!   pass's program), plans the view over the ready ones
//!   (`native_render::decorators::plan`) and asks the host to reserve the
//!   native pass and clear layer 3's bit on the planned items
//!   ([`Host::ask`]): the layer then builds no entry for them.
//!
//! Context-only, like the fog's: it reads and writes the memory its
//! context sees, so the pipelined render runs it on its own thread
//! (docs/graphics/pipelined-render.md, "Render-safe hooks").

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::{GuestRead, GuestWrite};
use native_render::decorators::{Material, Plan, plan};
use native_render::skip::{CURRENT_LAYER, Cleared, LIGHTMAP_LAYER};
use native_render::world_layers::{Site, layer_site};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-decorators";

/// Whether the native decorators are on.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// What draws the decorators: boot's glue over the loaded world and the
/// D3D device.
pub trait Host: Send + Sync {
    /// Has each of `materials` ready to draw, where it can be: the set's
    /// geometry read and the pass compiled, once per loaded world. Returns
    /// those that are.
    fn prepare(&self, mem: &dyn GuestMemory, materials: &[Material]) -> HashSet<Material>;
    /// Reserves the native pass for `plan` (the handoff's ordered
    /// reservation, recording on every copy before anything is
    /// surrendered), then clears layer 3's bit on the plan's items and
    /// commits. Returns what was cleared, or `None` to leave the view's
    /// decorators to the D3D path (the guest items untouched).
    fn ask(&self, mem: &mut dyn GuestMemory, plan: Plan) -> Option<Cleared>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Bits cleared at the last layer 3, with each item's mask as left, until
/// the next layer draw sets them back.
static CLEARED: Mutex<Option<Pending>> = Mutex::new(None);

/// What [`before_layer`] cleared and the masks it left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    pub cleared: Cleared,
    pub left: Vec<u32>,
}

static VIEWS: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static ITEMS: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);
static DECLINED: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// The stage's counters, for `render.scene`: views with decorators,
/// views drawn natively, items skipped, draws left to the guest because
/// their material is not ready, views the host declined.
pub fn counters() -> [(&'static str, u64); 5] {
    [
        ("scene_decorator_views", VIEWS.load(Ordering::Relaxed)),
        ("scene_decorator_views_native", DRAWN.load(Ordering::Relaxed)),
        ("scene_decorator_items_skipped", ITEMS.load(Ordering::Relaxed)),
        ("scene_decorator_draws_unready", REFUSED.load(Ordering::Relaxed)),
        ("scene_decorator_views_declined", DECLINED.load(Ordering::Relaxed)),
    ]
}

/// The guest's memory as `native_render` reads and writes it.
pub struct Mem<'a>(pub &'a mut dyn GuestMemory);

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

/// The open view's decorator materials, each once.
fn open_materials() -> Vec<Material> {
    crate::scene_step::with_open(|open| {
        let Some((scene, view)) = open else { return Vec::new() };
        let mut seen = HashSet::new();
        scene.decorators_of(view).iter().map(Material::of).filter(|m| seen.insert(*m)).collect()
    })
}

/// The observer before the layer draw, on any guest memory: the last
/// layer 3's bits back, then this layer's skip. `ret` is the layer draw's
/// return address (`[esp]` at entry).
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    let Ok(layer) = mem.u32(CURRENT_LAYER) else { return };
    if layer != LIGHTMAP_LAYER || layer_site(layer, ret) != Site::Scene {
        return;
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    // The scene step's lock is never held while the host runs: the host
    // takes the device's.
    let materials = open_materials();
    if materials.is_empty() {
        return;
    }
    VIEWS.fetch_add(1, Ordering::Relaxed);
    let ready = host.prepare(&*mem, &materials);
    let planned = crate::scene_step::with_open(|open| open.map(|(scene, view)| plan(scene, view, |m| ready.contains(m))));
    let Some(planned) = planned else { return };
    REFUSED.fetch_add(u64::from(planned.refused), Ordering::Relaxed);
    if planned.slots.is_empty() {
        return;
    }
    let Some(cleared) = host.ask(mem, planned) else {
        DECLINED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if cleared.items.is_empty() {
        return;
    }
    DRAWN.fetch_add(1, Ordering::Relaxed);
    ITEMS.fetch_add(cleared.items.len() as u64, Ordering::Relaxed);
    let m = Mem(mem);
    let left = cleared.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
    *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { cleared, left });
}

/// Sets back the bits the last layer 3 cleared, on the items that still
/// hold the mask it left: an item list built again since is left alone.
pub fn restore_pending(mem: &mut dyn GuestMemory) {
    let Some(p) = CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let mut m = Mem(mem);
    let bit = 1u32 << p.cleared.layer;
    for (&item, &left) in p.cleared.items.iter().zip(&p.left) {
        if m.read_u32(item + 0x10) == Some(left) {
            let _ = m.write_u32(item + 0x10, left | bit);
        }
    }
}

/// Drops the pending restore (a map activation or a snapshot restore:
/// the guest RAM it named is gone).
pub fn forget_pending() {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

pub struct NativeDecorators;

impl Feature for NativeDecorators {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Model decorators drawn instanced by the native GPU renderer")
            .doc("docs/graphics/native-decorators.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_decorators_skip", |o| {
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                before_layer(&mut *g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::scene_step::ID)
            .after_feature(crate::native_world::ID)
            .context_only();
        Ok(())
    }
}
