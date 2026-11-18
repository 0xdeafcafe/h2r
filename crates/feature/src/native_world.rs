//! `native-world`: the static world drawn by the native renderer instead
//! of the game's layer-3 entries (docs/graphics/native-renderer.md, stage
//! 1). On by default ([`DEFAULT`]); `H2_FEATURES=impl.render.native_world=off`
//! turns it off (exactly that key: `impl.render=off` does not, since this
//! one changes what draws the world). While it is on, pipelining is off
//! (`pipeline_render`): these observers are not render-thread safe, so
//! every pipelined frame would fall back at its first layer draw.
//!
//! Two observers on the layer draw `LAYER_DRAW` (`0x00044550`), an opaque
//! point, so nothing here replaces guest code:
//!
//! - before, when the current layer is 3: read the render camera and the
//!   active BSP, ask the [`Host`] (boot's glue: the GPU world and the D3D
//!   device) for the native pass, and when it accepts, clear layer 3's bit
//!   on the BSP items whose shader it draws
//!   ([`native_render::skip::clear_structure`]): the layer then builds no
//!   entry for them;
//! - after: set the bits back, so guest memory after the layer is as it
//!   was.
//!
//! Both sit inside `render-timing`'s observers at the point
//! (`after_feature`), so its layer stage holds them.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::{GuestRead, GuestWrite, RENDER_CAMERA, RenderCamera};
use native_render::skip::{self, Cleared, LIGHTMAP_LAYER};

use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-world";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_world";
/// The switch's value when `H2_FEATURES` does not name it.
pub const DEFAULT: bool = true;

/// Whether the native world is on. Pipelining is off with it
/// (`pipeline_render`): its layer-draw observers are not render-thread safe,
/// so every pipelined frame would fall back at its first layer draw.
pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, DEFAULT)
}

/// Draws each world material that allows it with its own layer-3 pass
/// (vertex program, pixel shader, stage textures and constants) instead of
/// stage 1's base map times lightmap (`native_render::program`). Off by
/// default until the A/B against the D3D path is read; looked up exactly.
pub const PROGRAMS_SWITCH: &str = "impl.render.native_world_programs";

/// Whether [`PROGRAMS_SWITCH`] is on (only with the native world on).
pub fn programs_on(switches: &crate::Switches) -> bool {
    on(switches) && switches.feature(PROGRAMS_SWITCH, false)
}
/// The active structure BSP's tag data (docs/formats/structure-bsp.md).
pub const ACTIVE_BSP: u32 = 0x004e_0348;

/// The shaders the native world draws, by tag index (the low word of the
/// datum an item's `+8` holds).
pub type ShaderSet = HashSet<u16>;

/// What draws the world: boot's glue over the GPU world and the device.
pub trait Host: Send + Sync {
    /// Before layer 3 of a view (`mem` the guest, read only): `bsp` the active BSP's address, `camera`
    /// the view's render camera. Asks for the native pass when a world for
    /// `bsp` is ready and the device can draw it; returns the shaders it
    /// draws, or `None` to leave the world to the D3D path.
    fn ask(&self, mem: &dyn GuestMemory, bsp: u32, camera: &RenderCamera) -> Option<Arc<ShaderSet>>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Bits cleared by the observer before the layer draw in progress.
static CLEARED: Mutex<Option<Cleared>> = Mutex::new(None);
static VIEWS: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static ITEMS: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Layer-3 views seen, views drawn natively, items skipped.
pub fn counts() -> (u64, u64, u64) {
    (
        VIEWS.load(Ordering::Relaxed),
        DRAWN.load(Ordering::Relaxed),
        ITEMS.load(Ordering::Relaxed),
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

/// The observer before the layer draw, on any guest memory.
pub fn before_layer(mem: &mut dyn GuestMemory) {
    let mut m = Mem(mem);
    if m.read_u32(skip::CURRENT_LAYER) != Some(LIGHTMAP_LAYER) {
        return;
    }
    VIEWS.fetch_add(1, Ordering::Relaxed);
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    let (Some(camera), Some(bsp)) = (RenderCamera::read(&m, RENDER_CAMERA), m.read_u32(ACTIVE_BSP)) else {
        return;
    };
    let Some(shaders) = host.ask(&*m.0, bsp, &camera) else {
        return;
    };
    let cleared = skip::clear_structure(&mut m, LIGHTMAP_LAYER, &|shader| shaders.contains(&(shader as u16)));
    if let Some(c) = cleared {
        DRAWN.fetch_add(1, Ordering::Relaxed);
        ITEMS.fetch_add(c.items.len() as u64, Ordering::Relaxed);
        let views = VIEWS.load(Ordering::Relaxed);
        if views % 1800 == 1 {
            let (v, d, i) = counts();
            eprintln!("native-world: {d} of {v} layer-3 views drawn natively, {i} world items skipped");
        }
        *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
    }
}

/// The observer after the layer draw: the bits back.
pub fn after_layer(mem: &mut dyn GuestMemory) {
    let Some(c) = CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    skip::restore(&mut Mem(mem), &c);
}

pub struct NativeWorld;

impl Feature for NativeWorld {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The static world drawn by the native GPU renderer")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_world_skip", |o| {
                let mut g = o.guest();
                before_layer(&mut *g);
            })
            .after_feature(crate::render_timing::ID);
        b.raw()
            .observe_after(&points::LAYER_DRAW, "native_world_restore", |o| {
                let mut g = o.guest();
                after_layer(&mut *g);
            })
            .after_feature(crate::render_timing::ID);
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
