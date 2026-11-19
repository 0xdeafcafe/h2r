//! `native-world`: the static world drawn by the native renderer instead
//! of the game's layer-3 entries (docs/graphics/native-renderer.md, stage
//! 1). Off by default ([`DEFAULT`]) until it matches the D3D path on
//! screen; `H2_FEATURES=impl.render.native_world=on` turns it on, `=off`
//! off (exactly that key: `impl.render=off` does not, since this
//! one changes what draws the world).
//!
//! One observer before the layer draw `LAYER_DRAW` (`0x00044550`), an
//! opaque point, so nothing here replaces guest code:
//!
//! - first it sets back the bits it cleared at the last layer 3 (the
//!   layers after 3 on the scene list, 5 and up, come next in the same
//!   view, so the bits are back before anything reads them again), each
//!   only if the item still holds the mask it left ([`restore_pending`]);
//! - then, when the current layer is 3: read the render camera and the
//!   active BSP, ask the [`Host`] (boot's glue: the GPU world and the D3D
//!   device) for the native pass, and when it accepts, clear layer 3's bit
//!   on the BSP items whose shader it draws
//!   ([`native_render::skip::clear_structure`]): the layer then builds no
//!   entry for them.
//!
//! Nothing runs after the body, so the chain needs no Rust continuation,
//! and the observer is context-only: it reads and writes the memory its
//! context sees (on the pipelined render thread, the render's view), and
//! the host's pass request goes to the D3D device in render order, as the
//! render's own D3D calls do. So the pipelined render runs it on its own
//! thread (docs/graphics/pipelined-render.md, "Render-safe hooks").
//!
//! It sits inside `render-timing`'s observers at the point
//! (`after_feature`), so its layer stage holds it.

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
pub const DEFAULT: bool = false;

/// Whether the native world is on.
pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, DEFAULT)
}

/// The world's materials drawn with their own layer-3 pass (vertex
/// program, pixel shader, stage textures, constants and render states,
/// `native_render::program`); a material whose pass does not translate
/// keeps its D3D draws. On with the world ([`PROGRAMS_DEFAULT`]): it is the
/// native world. `=off` draws every material whose base map decodes as
/// stage 1's base map times lightmap instead, for the A/B against stage 1
/// only. Looked up exactly.
pub const PROGRAMS_SWITCH: &str = "impl.render.native_world_programs";
/// [`PROGRAMS_SWITCH`]'s value when `H2_FEATURES` does not name it.
pub const PROGRAMS_DEFAULT: bool = true;

/// Whether [`PROGRAMS_SWITCH`] is on (only with the native world on).
pub fn programs_on(switches: &crate::Switches) -> bool {
    on(switches) && switches.feature(PROGRAMS_SWITCH, PROGRAMS_DEFAULT)
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
/// Bits cleared at the last layer 3, with each item's mask as it was left,
/// until the next layer draw sets them back. One render runs at a time
/// (on the render thread, or on the game thread serially or after a
/// fallback, which carries on the same render against live RAM), so the
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

/// The observer before the layer draw, on any guest memory: the last layer
/// 3's bits back, then this layer's skip.
pub fn before_layer(mem: &mut dyn GuestMemory) {
    restore_pending(mem);
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
        let left = c.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
        *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { cleared: c, left });
    }
}

/// Sets back the bits the last layer 3 cleared, on the items that still
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
            .after_feature(crate::render_timing::ID)
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
