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
//! The depth frames come from the GPU side through [`MAILBOX`]; until
//! something publishes one, every test is `unknown` and nothing is culled,
//! and the `occlusion:` line measures the hooks alone.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use engine::points;
use engine::render_loop::Regs;
use engine::render_loop::occlusion::guest::{self, Mailbox, Occlusion};
use engine::render_loop::occlusion::{Meter, Rules};
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
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

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
                if main {
                    st.meter.frame();
                    st.meter.spent(t0.elapsed().as_nanos() as u64);
                    let now = epoch().elapsed().as_nanos() as u64;
                    if let Some(line) = st.meter.tick(now, st.occ.counts()) {
                        eprintln!("{line}");
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
