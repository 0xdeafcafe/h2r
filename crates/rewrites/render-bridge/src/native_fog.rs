//! `native-fog`: planar fog, the scene list's layer 16, drawn by the
//! native renderer instead of the game's entries
//! (docs/graphics/native-water-fog.md, "Stage 4b"). On with the native
//! renderer (`impl.render.native`, on by default).
//!
//! One observer before the layer draw `LAYER_DRAW` (`0x00044550`), an
//! opaque point, so nothing here replaces guest code:
//!
//! - first it sets back the bits it cleared at the last layer 16 (the
//!   layers after 16 on the scene list — 6 and up — come next in the
//!   same view, so the bits are back before anything reads them again),
//!   each only if the item still holds the mask it left
//!   ([`restore_pending`]);
//! - then, when the current layer is 16, the caller is the late passes'
//!   call (`[esp] == native_render::fog::SCENE_FOG_RETURN`, the only
//!   caller the lifted port names: other callers are counted and left to
//!   the D3D path) and the layer draw's own gate holds
//!   (`native_render::fog::layer_draws`: the layer would draw fog), it
//!   asks the [`Host`] (boot's glue: the GPU fog materials and the D3D
//!   device) for the native pass, and when it accepts, clears layer 16's
//!   bit on the structure items whose part it draws, in the layer's item
//!   order ([`native_render::fog::clear_fog`]): the layer then builds no
//!   entry for them. Objects' fog items are never cleared (stage 2's),
//!   and a placement draws natively when its rigid program and rows resolve.
//!
//! The fog the passes read is the device's at the layer (set by
//! `0x00025d50` before the layer begins): the glue reads it through
//! `d3d::Device::render_state` at the ask and resolves each material's
//! words over it. Nothing runs after the body, so the chain needs no
//! Rust continuation, and the observer is context-only: it reads and
//! writes the memory its context sees, so the pipelined render runs it
//! on its own thread (docs/graphics/pipelined-render.md,
//! "Render-safe hooks"). It sits inside `render-timing`'s observers at
//! the point (`after_feature`), after the world's (which restores its
//! own bits first).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::{GuestRead, GuestWrite};
use native_render::fog::{self, FOG_LAYER};
use native_render::skip::{self, Cleared};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-fog";
/// Whether the native fog is on.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// What draws the fog: boot's glue over the GPU world and the device.
pub trait Host: Send + Sync {
    /// Before layer 16 of a view (`mem` the guest): plans the skip over a
    /// preview, reserves the native replacement (the handoff's ordered
    /// reservation, recording on every copy before anything is
    /// surrendered), applies the mask edits and commits. Returns what was
    /// cleared for the restore at the next layer draw, or `None` to leave
    /// the fog to the D3D path (the guest items untouched).
    fn ask(&self, mem: &mut dyn GuestMemory) -> Option<Cleared>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Bits cleared at the last layer 16, with each item's mask as it was
/// left, until the next layer draw sets them back. One render runs at a
/// time (on the render thread, or on the game thread serially), so the
/// next layer draw that reads this is the same render's.
static CLEARED: Mutex<Option<Pending>> = Mutex::new(None);

/// What [`before_layer`] cleared and the masks it left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    pub cleared: Cleared,
    /// The mask each item of `cleared` was left with.
    pub left: Vec<u32>,
}
static VIEWS: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static ITEMS: AtomicU64 = AtomicU64::new(0);
static GATED: AtomicU64 = AtomicU64::new(0);
static WRONG_CALLER: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Layer-16 calls seen (any caller), views drawn natively, items skipped,
/// views the gate left to the D3D path (the layer draws no fog), calls
/// from another caller.
pub fn counts() -> (u64, u64, u64, u64, u64) {
    (
        VIEWS.load(Ordering::Relaxed),
        DRAWN.load(Ordering::Relaxed),
        ITEMS.load(Ordering::Relaxed),
        GATED.load(Ordering::Relaxed),
        WRONG_CALLER.load(Ordering::Relaxed),
    )
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

/// The observer before the layer draw, on any guest memory: the last
/// layer 16's bits back, then this layer's skip. `ret` is the layer
/// draw's return address (`[esp]` at entry).
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    let m = Mem(mem);
    if m.read_u32(skip::CURRENT_LAYER) != Some(FOG_LAYER as u32) {
        return;
    }
    VIEWS.fetch_add(1, Ordering::Relaxed);
    if ret != fog::SCENE_FOG_RETURN {
        let n = WRONG_CALLER.fetch_add(1, Ordering::Relaxed);
        if n % 1800 == 0 {
            eprintln!("native-fog: {} layer-16 calls from {ret:#010x}, not the late passes' ({:#010x}): left to the D3D path", n + 1, fog::SCENE_FOG_RETURN);
        }
        return;
    }
    // The layer draw's own gate (`0x00044582`): a layer that draws no
    // fog must not be drawn natively either. Unreadable, the D3D path.
    match fog::layer_draws(&m) {
        Some(true) => {}
        Some(false) => {
            GATED.fetch_add(1, Ordering::Relaxed);
            crate::scene_step::late_kept(&*m.0, FOG_LAYER as u8, false);
            return;
        }
        None => return,
    }
    // A layer 16 without a BSP item draws no world fog (objects only):
    // the native pass would have nothing to draw. The frame step's scene
    // records what the guest keeps (`scene_step::late_kept`); the glue
    // writes the fog it takes (docs/graphics/framescene.md, slice 5).
    if skip::has_structure_items(&m, FOG_LAYER as u32) != Some(true) {
        crate::scene_step::late_kept(&*m.0, FOG_LAYER as u8, true);
        return;
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    let Some(cleared) = host.ask(&mut *m.0) else {
        crate::scene_step::late_kept(&*m.0, FOG_LAYER as u8, true);
        return;
    };
    if cleared.items.is_empty() {
        return;
    }
    DRAWN.fetch_add(1, Ordering::Relaxed);
    ITEMS.fetch_add(cleared.items.len() as u64, Ordering::Relaxed);
    let views = VIEWS.load(Ordering::Relaxed);
    if views % 1800 == 1 {
        let (v, d, i, g, w) = counts();
        eprintln!("native-fog: {d} of {v} layer-16 views drawn natively ({g} gated off, {w} other callers), {i} items skipped");
    }
    let left = cleared.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
    *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { cleared, left });
}

pub(crate) fn has_claim() -> bool {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner())
        .as_ref().is_some_and(|claim| !claim.cleared.items.is_empty())
}

/// Sets back the bits the last layer 16 cleared, on the items that still
/// hold the mask it left: an item list built again since (a new frame
/// whose render never reached another layer draw) is left alone.
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

pub(crate) fn forget_pending() {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

pub struct NativeFog;

impl Feature for NativeFog {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Planar fog drawn by the native GPU renderer")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_fog_skip", |o| {
                if crate::native_map_work::Stage::Fog.standing_down() {
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
