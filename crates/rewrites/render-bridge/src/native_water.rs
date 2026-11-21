//! `native-water`: the water edge blend, layer 6, drawn by the native
//! renderer instead of the game's items (docs/graphics/native-water-fog.md,
//! "Stage 4b: water and the rest"). On with the native renderer
//! (`impl.render.native`, on by default).
//!
//! **The skip** (layer 6 only): one context-only observer before the
//! layer draw `LAYER_DRAW` (`0x00044550`), after the fog's. It restores
//! the last skip's bits, then when the current layer is 6, the caller is
//! the late passes' call (`[esp] == [`SCENE_WATER_RETURN`]`), the
//! water gate holds (bit 4 of `0x004b9fe0` clear: the wrapper
//! `0x0003ba90` ran and the layer draws) and the list holds a structure
//! item in the layer, asks the [`Host`] (boot's glue) for the native
//! replacement. The host plans the skip over a preview, reserves the
//! replacement through the ordered handoff (recording on every copy
//! before anything is surrendered), applies the mask edits and commits;
//! the hook stores what was cleared for the restore at the next layer
//! draw. The wrappers themselves — `0x0003ba90` before, `0x0003c3c0`
//! with its alpha-only clear `0x3c515` after — are never skipped: the
//! native draws land inside their window, and the clear keeps its guest
//! order after them.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::{GuestRead, GuestWrite};
use native_render::skip::{self, Cleared, CURRENT_LAYER};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-water";
/// Whether the native water is on.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// Bit 4 of `0x004b9fe0`: set, `0x0003c3c0(0, 0)` runs instead of the
/// water setup and layer 6 does not draw (the lifted port's `0x0002c0ee`).
pub const WATER_GATE: u32 = 0x004b_9fe0;
/// The late passes' layer-6 call: the layer draw's return address
/// (`[esp]` at its entry, the lifted port's `0x0002c117` call). Other
/// callers of the layer draw with layer 6 current are not the scene's
/// water and stay on the D3D path.
pub const SCENE_WATER_RETURN: u32 = 0x0002_c120;
/// The layer the skip draws.
pub const WATER_LAYER: u32 = 6;

/// What draws the water edge blend: boot's glue over the GPU world and
/// the device. As the fog's ([`crate::native_fog::Host`]): plans the
/// skip over a preview, reserves the replacement (recording on every
/// copy before anything is surrendered), applies the mask edits and
/// commits; the wrappers `0x0003ba90`/`0x0003c3c0` are never skipped,
/// so the replacement lands inside their window and the alpha-only
/// clear `0x3c515` keeps its guest order after it.
pub trait Host: Send + Sync {
    fn ask(&self, mem: &mut dyn GuestMemory) -> Option<Cleared>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Bits cleared at the last layer 6, with each item's mask as it was
/// left, until the next layer draw sets them back.
static CLEARED: Mutex<Option<Pending>> = Mutex::new(None);

/// What the last skip cleared and the masks it left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    pub cleared: Cleared,
    /// The mask each item of `cleared` was left with.
    pub left: Vec<u32>,
}

static WATER_VIEWS: AtomicU64 = AtomicU64::new(0);
static WATER_DRAWN: AtomicU64 = AtomicU64::new(0);
static WATER_ITEMS: AtomicU64 = AtomicU64::new(0);
static WATER_GATED: AtomicU64 = AtomicU64::new(0);
static WATER_WRONG_CALLER: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Layer-6 calls seen, views replaced natively, items skipped, views the
/// water gate left to the D3D path, calls from another caller.
pub fn counts() -> (u64, u64, u64, u64, u64) {
    (
        WATER_VIEWS.load(Ordering::Relaxed),
        WATER_DRAWN.load(Ordering::Relaxed),
        WATER_ITEMS.load(Ordering::Relaxed),
        WATER_GATED.load(Ordering::Relaxed),
        WATER_WRONG_CALLER.load(Ordering::Relaxed),
    )
}

pub(crate) fn has_claim() -> bool {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner())
        .as_ref().is_some_and(|claim| !claim.cleared.items.is_empty())
}

/// Sets back the bits the last layer 6 cleared, on the items that still
/// hold the mask it left.
pub fn restore_pending(mem: &mut dyn GuestMemory) {
    let Some(p) = CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let mut m = crate::native_fog::Mem(mem);
    let bit = 1u32 << p.cleared.layer;
    for (&item, &left) in p.cleared.items.iter().zip(&p.left) {
        if m.read_u32(item + 0x10) == Some(left) {
            let _ = m.write_u32(item + 0x10, left | bit);
        }
    }
}

/// The observer before the layer draw, on any guest memory: the last
/// layer 6's bits back, then this layer's skip. `ret` is the
/// layer draw's return address (`[esp]` at entry).
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    let m = crate::native_fog::Mem(mem);
    if m.read_u32(CURRENT_LAYER) != Some(WATER_LAYER) {
        return;
    }
    WATER_VIEWS.fetch_add(1, Ordering::Relaxed);
    if ret != SCENE_WATER_RETURN {
        let n = WATER_WRONG_CALLER.fetch_add(1, Ordering::Relaxed);
        if n % 1800 == 0 {
            eprintln!(
                "native-water: {} layer-6 calls from {ret:#010x}, not the late passes' ({:#010x}): left to the D3D path",
                n + 1,
                SCENE_WATER_RETURN
            );
        }
        return;
    }
    // The water gate (bit 4 of `0x004b9fe0`, the lifted port's
    // `0x0002c0ee`): set, the wrapper does not draw and neither does the
    // native pass. Unreadable, the D3D path.
    match m.read_u32(WATER_GATE) {
        Some(word) if word & 0x10 == 0 => {}
        Some(_) => {
            WATER_GATED.fetch_add(1, Ordering::Relaxed);
            crate::scene_step::late_kept(&*m.0, WATER_LAYER as u8, false);
            return;
        }
        None => return,
    }
    // A layer 6 without a structure item draws no world water. As the
    // fog's: the scene records what the guest keeps, the glue what it
    // takes.
    if skip::has_structure_items(&m, WATER_LAYER) != Some(true) {
        crate::scene_step::late_kept(&*m.0, WATER_LAYER as u8, true);
        return;
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    let Some(cleared) = host.ask(&mut *m.0) else {
        crate::scene_step::late_kept(&*m.0, WATER_LAYER as u8, true);
        return;
    };
    if cleared.items.is_empty() {
        return;
    }
    WATER_DRAWN.fetch_add(1, Ordering::Relaxed);
    WATER_ITEMS.fetch_add(cleared.items.len() as u64, Ordering::Relaxed);
    let views = WATER_VIEWS.load(Ordering::Relaxed);
    if views % 1800 == 1 {
        let (v, d, i, g, w) = counts();
        eprintln!("native-water: {d} of {v} layer-6 views drawn natively ({g} gated off, {w} other callers), {i} items skipped");
    }
    let left = cleared.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
    *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { cleared, left });
}

pub(crate) fn forget_pending() {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

pub struct NativeWater;

impl Feature for NativeWater {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Native water edge blend")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_water_skip", |o| {
                if crate::native_map_work::Stage::Water.standing_down() {
                    return;
                }
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                before_layer(&mut *g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::native_world::ID)
            .after_feature(crate::native_fog::ID)
            .context_only();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

