//! `native-transparents`: the sorted transparents' geometry drawn by the
//! native renderer, in the game's own order (docs/graphics/native-renderer.md,
//! "Stage 3: transparents"). Off by default;
//! `H2_FEATURES=impl.render.native_transparents=on` turns it on (exactly
//! that key).
//!
//! The transparents (`SCENE_TRANSPARENTS`, `0x0001e370`) sort their list
//! and call each entry's handler in that order. Geometry entries (BSP
//! clusters, instanced geometry, objects) have the handler
//! `TRANSPARENT_GEOMETRY` (`0x00041020`), which builds and draws the
//! template's layer-15 passes. Two hooks, both context-only and with
//! nothing after a body, so the pipelined render runs them on its own
//! thread (docs/graphics/pipelined-render.md, "Render-safe hooks"):
//!
//! - a `before` on the handler: when the [`Host`] draws the entry
//!   (`native_render::transparent::read_entry`), add it to the open
//!   [`Run`] and answer `ret 4` in place of the handler, so the entry
//!   builds and draws nothing. Without an open run, ask the host for one:
//!   it asks the device for an external pass that takes the run's entries
//!   when the device issues it, at its next entry, which is the next D3D
//!   transparent's draw (or what follows the transparents). A sealed run
//!   (a D3D draw came between) is replaced by a new one, so each run lands
//!   between the D3D draws it sorts between;
//! - an observer before the transparents: forgets the open run, so a run
//!   never spans two views (or a pass the device dropped).
//!
//! Nothing here writes guest memory: there is no restore. The entries are
//! read from the memory the hook's context sees, which on the render
//! thread is the render's snapshot of the frame.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::{GuestRead, RENDER_CAMERA, RenderCamera};
use native_render::transparent::{self, Entry, Run};

use crate::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-transparents";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_transparents";

/// Whether the native transparents are on (off by default).
pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, false)
}

/// What draws the transparents: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    /// Whether the host draws `entry` natively now (its geometry and its
    /// layer-15 material are loaded and supported).
    fn accepts(&self, mem: &dyn GuestMemory, entry: &Entry) -> bool;
    /// Asks the device for an external pass over `run` with `camera`; the
    /// pass seals the run when the device issues it. `false`: nothing
    /// asked (the entry goes to the D3D path).
    fn open(&self, camera: &RenderCamera, run: Arc<Run>) -> bool;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// The run native entries join until the device issues its pass.
static OPEN: Mutex<Option<Arc<Run>>> = Mutex::new(None);
static ENTRIES: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static RUNS: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Geometry entries met, entries drawn natively, runs opened.
pub fn counts() -> (u64, u64, u64) {
    (
        ENTRIES.load(Ordering::Relaxed),
        SKIPPED.load(Ordering::Relaxed),
        RUNS.load(Ordering::Relaxed),
    )
}

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The observer before the transparents: no run carries over.
pub fn new_view() {
    OPEN.lock().unwrap_or_else(|e| e.into_inner()).take();
}

/// The handler's `before`, on any guest memory: `data` its argument (the
/// entry's `+0x24`). `true` when the entry joined a native run and the
/// handler must not run.
pub fn on_entry(mem: &dyn GuestMemory, data: u32) -> bool {
    ENTRIES.fetch_add(1, Ordering::Relaxed);
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    let m = Ro(mem);
    let Some(entry) = transparent::read_entry(&m, data) else {
        return false;
    };
    if !host.accepts(mem, &entry) {
        return false;
    }
    let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(run) = open.as_ref()
        && run.push(entry)
    {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    let Some(camera) = RenderCamera::read(&m, RENDER_CAMERA) else {
        return false;
    };
    let run = Arc::new(Run::new());
    run.push(entry);
    if !host.open(&camera, Arc::clone(&run)) {
        *open = None;
        return false;
    }
    *open = Some(run);
    SKIPPED.fetch_add(1, Ordering::Relaxed);
    let runs = RUNS.fetch_add(1, Ordering::Relaxed) + 1;
    if runs % 1800 == 1 {
        let (e, s, r) = counts();
        eprintln!("native-transparents: {s} of {e} transparent geometry entries drawn natively, in {r} runs");
    }
    true
}

pub struct NativeTransparents;

impl Feature for NativeTransparents {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Sorted transparent geometry drawn by the native GPU renderer")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::SCENE_TRANSPARENTS, "native_transparents_view", |_| new_view())
            .context_only();
        b.raw()
            .before(&points::TRANSPARENT_GEOMETRY, "native_transparents_skip", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let skip = {
                    let mem = rt.memory();
                    match mem.u32(c.cpu.esp.wrapping_add(4)) {
                        Ok(data) => on_entry(&*mem, data),
                        Err(_) => false,
                    }
                };
                if skip {
                    c.ret(4).map(Flow::Return)
                } else {
                    Ok(Flow::Continue)
                }
            })
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
