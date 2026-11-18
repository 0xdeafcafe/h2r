//! `occlusion`: skips the render setup of objects hidden behind the
//! previous frame's depth (docs/graphics/occlusion-culling.md). Off by
//! default; `H2_FEATURES=impl.render.occlusion=on` turns it on (exactly
//! that key: `impl.render=on` does not, since this one changes what is
//! drawn).
//!
//! Two hooks, both on the main view's visibility struct only:
//!
//! - an observer before `OBJECT_RENDER_RECORDS` (`0x00133720`) starts the
//!   view's frame: the camera, the protected handles (the player, its
//!   vehicle, what it holds) and the newest depth frame from [`MAILBOX`];
//! - a `before` on `OBJECT_RENDER_SETUP` (`0x00133790`) tests the listed
//!   object and, when the tracker says skip, does the setup's own "not
//!   drawn" exit in Rust and returns: no LOD, nodes, children or render
//!   records for it, so none of its layer draws.
//!
//! The depth: an observer before `SCENE_TRANSPARENTS` (`0x0001e370`), the
//! first one after a main view's records were built, asks the capture
//! function boot installs ([`set_capture`]) for a grid of the depth as the
//! opaque layers left it, tagged with the view, frame and camera. The D3D
//! device records the reduction in order on the render thread
//! (`d3d::Device::request_depth_grid`) and hands the grid to [`deliver`],
//! which publishes it to [`MAILBOX`]. Without a capture function every
//! test is `unknown` and nothing is culled.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use engine::points;
use engine::render_loop::Regs;
use engine::render_loop::occlusion::guest::{self, Mailbox, Occlusion};
use engine::render_loop::occlusion::{
    Camera, DepthGrid, DepthMapping, Frame, GridSummary, HiZ, Meter, Rules, probe_line,
};
use runtime::guest::{Access, FaultReason};
use runtime::{CpuAbi, GuestFault};

use crate::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "occlusion";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.occlusion";

/// Where the GPU side hands over depth frames, by view index.
pub static MAILBOX: Mailbox = Mailbox::new();

struct State {
    occ: Occlusion,
    meter: Meter,
    /// The main view whose depth the next transparents call captures.
    armed: Option<Shot>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// What a capture was taken for.
#[derive(Clone, Copy, Debug)]
struct Shot {
    view: u32,
    frame: u64,
    camera: Camera,
}

/// The grid size asked for.
pub const GRID: (u32, u32) = (64, 32);
/// Cells nearer than this (world units) occlude nothing: the first-person
/// weapon and the player's body.
pub const MIN_OCCLUDER: f32 = 0.5;

/// Asks the D3D device for a grid: `(tag, width, height)`.
pub type CaptureFn = Box<dyn Fn(u64, u32, u32) + Send + Sync>;

static CAPTURE: Mutex<Option<CaptureFn>> = Mutex::new(None);
/// Captures asked for and not delivered yet, by tag (oldest first).
static INFLIGHT: Mutex<Vec<(u64, Shot)>> = Mutex::new(Vec::new());
static NEXT_TAG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static PUBLISHED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The newest published grid at a glance and its mapping, for the
/// `occlusion probe:` line.
static LAST_GRID: Mutex<Option<(GridSummary, DepthMapping)>> = Mutex::new(None);

/// Installs the capture function (boot: the D3D device's
/// `request_depth_grid`). Called once; only used with the switch on.
pub fn set_capture(f: CaptureFn) {
    *CAPTURE.lock().unwrap() = Some(f);
}

/// A finished grid for `tag`: raw depths (row 0 the top) and their mapping
/// to view depth. Publishes the view's [`Frame`]; unknown tags (too old,
/// or from before a restart) are dropped. Never blocks on the game.
pub fn deliver(tag: u64, width: u32, height: u32, raw: &[f32], map: DepthMapping) {
    let shot = {
        let mut f = INFLIGHT.lock().unwrap();
        let Some(i) = f.iter().position(|(t, _)| *t == tag) else {
            return;
        };
        // Older ones will not come (the backend delivers in order).
        f.drain(..=i).next_back().map(|(_, s)| s)
    };
    let Some(shot) = shot else { return };
    let Some(mut grid) = DepthGrid::from_raw(width, height, raw, map) else {
        return;
    };
    let mut summary = grid.summary();
    summary.dropped = grid.ignore_nearer(MIN_OCCLUDER);
    *LAST_GRID.lock().unwrap() = Some((summary, map));
    MAILBOX.publish(
        shot.view,
        Frame {
            hiz: HiZ::build(grid),
            camera: shot.camera,
            frame: shot.frame,
        },
    );
    PUBLISHED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The newest published grid at a glance and its mapping (the
/// `occlusion probe:` line's second half).
pub fn last_grid() -> Option<(GridSummary, DepthMapping)> {
    *LAST_GRID.lock().unwrap()
}

/// Grids published so far.
pub fn published() -> u64 {
    PUBLISHED.load(std::sync::atomic::Ordering::Relaxed)
}

fn epoch() -> Instant {
    static E: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *E.get_or_init(Instant::now)
}

pub struct OcclusionCull;

impl Feature for OcclusionCull {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "Occlusion culling of objects against the previous frame's depth",
        )
        .doc("docs/graphics/occlusion-culling.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !b.switches().feature(SWITCH, false) {
            return Ok(());
        }
        *STATE.lock().unwrap() = Some(State {
            occ: Occlusion::new(Rules::default()),
            meter: Meter::default(),
            armed: None,
        });
        b.raw()
            .observe_before(&points::OBJECT_RENDER_RECORDS, "occlusion_begin", |o| {
                let t0 = Instant::now();
                let mem = o.guest();
                let mut st = STATE.lock().unwrap();
                let Some(st) = st.as_mut() else { return };
                let main = o.cpu.eax == guest::MAIN_VIS;
                // A fault reading the camera or the player leaves the view
                // without a frame: nothing is culled in it.
                let _ = st.occ.begin_records(&*mem, o.cpu.eax, &MAILBOX);
                if main && let Some(view) = st.occ.current_view() {
                    // This view's depth, once its opaque layers are drawn.
                    if let (Ok(Some(camera)), Ok(frame)) =
                        (guest::read_camera(&*mem), mem.u32(guest::FRAME))
                    {
                        st.armed = Some(Shot {
                            view,
                            frame: frame as u64,
                            camera,
                        });
                    }
                }
                if main {
                    st.meter.frame();
                    st.meter.spent(t0.elapsed().as_nanos() as u64);
                    let now = epoch().elapsed().as_nanos() as u64;
                    if let Some(line) = st.meter.tick(now, st.occ.counts()) {
                        eprintln!("{line} grids {}", published());
                        // One object's test and what the newest grid held,
                        // so a run can tell a grid that occludes nothing
                        // from a test that never reaches it.
                        let probe = st.occ.take_probe();
                        let grid = last_grid();
                        eprintln!(
                            "{}",
                            probe_line(probe.as_ref(), grid.as_ref().map(|(g, m)| (g, *m)))
                        );
                    }
                }
            });
        b.raw()
            .before(&points::OBJECT_RENDER_SETUP, "occlusion_skip", |c| {
                let t0 = Instant::now();
                let rt = Arc::clone(c.ctx.runtime());
                let skip = {
                    let mut mem = rt.memory();
                    let mut st = STATE.lock().unwrap();
                    let Some(st) = st.as_mut() else {
                        return Ok(Flow::Continue);
                    };
                    let mut r = regs(c.cpu);
                    let skip = match st.occ.decide(&*mem, &r) {
                        Ok(true) => {
                            guest::skip_setup(&mut r, &mut *mem).map_err(fault)?;
                            c.cpu.eax = r.eax;
                            c.cpu.ecx = r.ecx;
                            c.cpu.edx = r.edx;
                            true
                        }
                        // A fault in the read: let the original run (it
                        // reads the same memory and faults as it would).
                        Ok(false) | Err(_) => false,
                    };
                    st.meter.spent(t0.elapsed().as_nanos() as u64);
                    skip
                };
                if skip {
                    c.ret(0x14).map(Flow::Return)
                } else {
                    Ok(Flow::Continue)
                }
            });
        b.raw()
            .observe_before(&points::SCENE_TRANSPARENTS, "occlusion_capture", |_| {
                let Some(shot) = STATE
                    .lock()
                    .unwrap()
                    .as_mut()
                    .and_then(|st| st.armed.take())
                else {
                    return;
                };
                let tag = NEXT_TAG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                {
                    let mut f = INFLIGHT.lock().unwrap();
                    f.push((tag, shot));
                    // Nothing comes back (no capture function, a backend
                    // that cannot): keep the list short.
                    if f.len() > 8 {
                        f.remove(0);
                    }
                }
                if let Some(capture) = CAPTURE.lock().unwrap().as_ref() {
                    capture(tag, GRID.0, GRID.1);
                }
            });
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}

fn regs(cpu: &CpuAbi) -> Regs {
    Regs {
        eax: cpu.eax,
        ecx: cpu.ecx,
        edx: cpu.edx,
        ebx: cpu.ebx,
        esp: cpu.esp,
        ebp: cpu.ebp,
        esi: cpu.esi,
        edi: cpu.edi,
        xmm0: u128::from_le_bytes(cpu.xmm[0]),
        xmm1: u128::from_le_bytes(cpu.xmm[1]),
    }
}

fn fault(f: engine::memory::MemFault) -> GuestFault {
    GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { Access::Write } else { Access::Read },
        reason: FaultReason::Unmapped,
    }
}
