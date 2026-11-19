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
//! Objects' entries (with `impl.render.native_objects` on as well): the
//! hook copies the object's node matrices and sphere out of guest memory
//! into the run (`native_render::objects::read_object`), as stage 2 does
//! at layer 3, so the pass draws the frame's pose. An object's entry
//! stands for all its parts with that shader (S: which section and part an
//! object entry names is not read), so a later entry of the same object
//! and shader in the same view is skipped without drawing again.
//!
//! Nothing here writes guest memory: there is no restore. The entries are
//! read from the memory the hook's context sees, which on the render
//! thread is the render's snapshot of the frame.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::{GuestRead, RENDER_CAMERA, RenderCamera};
use native_render::objects::{self, ObjectFrame};
use native_render::transparent::{self, Entry, Run, Source};

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
    /// `object`: an object entry's copy of the object (definition, node
    /// matrices, sphere).
    fn accepts(&self, mem: &dyn GuestMemory, entry: &Entry, object: Option<&ObjectFrame>) -> bool;
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
/// Objects' entries are considered (the native objects on too).
static OBJECTS_ON: AtomicBool = AtomicBool::new(false);
/// (object handle, shader index) drawn natively in this view.
static OBJECTS_DRAWN: Mutex<Option<HashSet<(u32, u16)>>> = Mutex::new(None);

/// Whether objects' entries are considered (the build sets it: both this
/// switch and `impl.render.native_objects` on).
pub fn set_objects(on: bool) {
    OBJECTS_ON.store(on, Ordering::Relaxed);
}

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
    OBJECTS_DRAWN.lock().unwrap_or_else(|e| e.into_inner()).take();
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
    let object = match entry.source {
        Source::Object(handle) => {
            if !OBJECTS_ON.load(Ordering::Relaxed) {
                return false;
            }
            let key = (handle, entry.shader as u16);
            if OBJECTS_DRAWN
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .is_some_and(|d| d.contains(&key))
            {
                // Its parts with this shader are in an earlier run.
                SKIPPED.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            let Some(frame) = objects::read_object(&m, handle) else {
                return false;
            };
            Some(frame)
        }
        Source::Cluster(_) | Source::Placement(_) => None,
    };
    if !host.accepts(mem, &entry, object.as_ref()) {
        return false;
    }
    let drawn = object.as_ref().map(|o| (o.handle, entry.shader as u16));
    if !join(&*host, &m, entry, object) {
        return false;
    }
    if let Some(key) = drawn {
        OBJECTS_DRAWN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashSet::new)
            .insert(key);
    }
    SKIPPED.fetch_add(1, Ordering::Relaxed);
    true
}

/// Adds the entry to the open run, or opens one; `false` when no pass
/// could be asked for.
fn join(host: &dyn Host, m: &Ro<'_>, entry: Entry, object: Option<ObjectFrame>) -> bool {
    // OPEN is only held to read or store the run: never across the run's
    // own lock or the host's (open takes the device lock).
    let current = OPEN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(run) = current
        && !run.is_sealed()
    {
        return run.push(entry, object);
    }
    let Some(camera) = RenderCamera::read(m, RENDER_CAMERA) else {
        return false;
    };
    let run = Arc::new(Run::new());
    run.push(entry, object);
    let opened = host.open(&camera, Arc::clone(&run));
    *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = opened.then_some(run);
    if !opened {
        return false;
    }
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
        // Stateless, on or off: what it holds (`OPEN`, the run entries
        // join, and `OBJECTS_DRAWN`) lasts one view and every view's
        // first observer forgets it ([`new_view`]); a save comes between
        // renders, with the pipelined render drained. It changes no guest
        // memory, and the host's materials are native-objects' models,
        // keyed by map name.
        b.snapshot(crate::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        // Only ever turned on here (a plan built without the objects, as the
        // tests build several, leaves an earlier one's choice alone).
        if b.switches().feature(crate::native_objects::SWITCH, false) {
            set_objects(true);
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
        Ok(())
    }
}
