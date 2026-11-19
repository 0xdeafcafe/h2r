//! `native-fog`: planar fog, the scene list's layer 16, drawn by the
//! native renderer instead of the game's entries
//! (docs/graphics/native-renderer.md, "Stage 4b"). Off by default
//! ([`DEFAULT`]) until it matches the D3D path on screen;
//! `H2_FEATURES=impl.render.native_fog=on` turns it on (exactly that
//! key).
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
//!   and a placement's fog needs the rigid program and stays on the D3D
//!   path in this slice.
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

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::{GuestRead, GuestWrite};
use native_render::fog::{self, FOG_LAYER};
use native_render::skip::{self, Cleared};
use native_render::world::Owner;

use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-fog";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_fog";
/// The switch's value when `H2_FEATURES` does not name it.
pub const DEFAULT: bool = false;

/// Whether the native fog is on.
pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, DEFAULT)
}

/// The fog parts the native pass draws, by owner (a cluster; placements
/// stay on the D3D path in this slice) and part index, each with its
/// material's shader by tag index (the low word of the datum an item's
/// `+8` holds): the world's part set's shape.
pub type PartSet = HashMap<(Owner, u8), u16>;

/// What draws the fog: boot's glue over the GPU world and the device.
pub trait Host: Send + Sync {
    /// Before layer 16 of a view (`mem` the guest, read only): asks for
    /// the native pass when a world with fog materials is ready and the
    /// device can draw it (it reads the fog render states itself, and
    /// refuses a view whose do not read). Returns the parts it draws, or
    /// `None` to leave the fog to the D3D path.
    fn ask(&self, mem: &dyn GuestMemory) -> Option<Arc<PartSet>>;
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
    let mut m = Mem(mem);
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
            return;
        }
        None => return,
    }
    // A layer 16 without a BSP item draws no world fog (objects only):
    // the native pass would have nothing to draw.
    if skip::has_structure_items(&m, FOG_LAYER as u32) != Some(true) {
        return;
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    let Some(parts) = host.ask(&*m.0) else {
        return;
    };
    let Some((drawn, cleared)) = fog::clear_fog(&mut m, FOG_LAYER as u32, &|item| {
        parts.get(&(item.owner, item.part)) == Some(&(item.shader))
    }) else {
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
        eprintln!("native-fog: {d} of {v} layer-16 views drawn natively ({g} gated off, {w} other callers), {i} items skipped, {} parts", drawn.len());
    }
    let left = cleared.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
    *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { cleared, left });
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
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                before_layer(&mut *g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::native_world::ID)
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
