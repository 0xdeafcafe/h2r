//! Particle submission observer, off by default. The helper and all its
//! parents still run: especially the state updates in `0x00174420`.
//! A host may replace only the helper's single, fully captured draw, after
//! every target copy is prepared. It must preserve attribute carry/state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{BuildError, Builder, Feature, Flow, Manifest};
use engine::memory::GuestMemory;
use engine::points::{Point, Sig};
use native_render::camera::GuestRead;
use native_render::particles::{self, Emission};

pub const ID: &str = "native-particles";
pub const SWITCH: &str = "impl.render.native_particles";
/// Lane-local named point; no changes to the sorted-transparent dispatcher.
pub const EMIT: Point = Point::new(
    "PARTICLE_EMIT_QUAD",
    particles::EMIT_QUAD,
    Sig::Opaque,
    "docs/graphics/native-particles.md",
);

pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, false)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Native,
    Compare,
}

#[derive(Clone, Debug)]
pub struct Call {
    pub esp: u32,
    pub return_address: u32,
    pub emission: Emission,
}

pub trait Host: Send + Sync {
    /// Arm one call-scoped capture, not a global "next quad" match. False
    /// leaves the entire call on D3D. True is *armed*, never proof of a
    /// replaced draw. The adapter cancels on return with no matching draw,
    /// or on any state/order/geometry mismatch. No prefix may be dropped.
    fn arm(&self, mem: &mut dyn GuestMemory, call: &Call, mode: Mode) -> bool;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CALLS: AtomicU64 = AtomicU64::new(0);
static DECODED: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicU64 = AtomicU64::new(0);

pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}
pub fn counts() -> (u64, u64, u64) {
    (
        CALLS.load(Ordering::Relaxed),
        DECODED.load(Ordering::Relaxed),
        ARMED.load(Ordering::Relaxed),
    )
}

/// H2_NATIVE_PARTICLES_COMPARE=N observes at most N calls without replacing
/// them. An empty value means eight. Invalid/zero values disable comparison.
pub fn parse_compare(value: &str) -> Option<u64> {
    if value.trim().is_empty() {
        Some(8)
    } else {
        value.trim().parse().ok().filter(|n| *n > 0)
    }
}
pub fn compare_budget() -> Option<u64> {
    static VALUE: OnceLock<Option<u64>> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("H2_NATIVE_PARTICLES_COMPARE")
            .ok()
            .and_then(|v| parse_compare(&v))
    })
}
struct Read<'a>(&'a dyn GuestMemory);
impl GuestRead for Read<'_> {
    fn read_u32(&self, a: u32) -> Option<u32> {
        self.0.u32(a).ok()
    }
}

/// Always lets the guest execute. Snapshot creation and successful command
/// recording are the adapter's responsibility, never inferred from arming.
pub fn on_emit(mem: &mut dyn GuestMemory, esp: u32, ebx: u32) {
    let ordinal = CALLS.fetch_add(1, Ordering::Relaxed);
    let compare = compare_budget();
    if compare.is_some_and(|budget| ordinal >= budget) {
        return;
    }
    let mode = if compare.is_some() {
        Mode::Compare
    } else {
        Mode::Native
    };
    let emission = particles::read_emission(&Read(mem), esp, ebx);
    let Ok(emission) = emission else {
        if mode == Mode::Compare {
            eprintln!("native-particles-compare: call={ordinal} decode={emission:?}");
        }
        return;
    };
    let Ok(return_address) = mem.u32(esp) else {
        return;
    };
    DECODED.fetch_add(1, Ordering::Relaxed);
    let call = Call {
        esp,
        return_address,
        emission,
    };
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let armed = host.as_ref().is_some_and(|h| h.arm(mem, &call, mode));
    if armed {
        ARMED.fetch_add(1, Ordering::Relaxed);
    }
    if mode == Mode::Compare {
        eprintln!(
            "native-particles-compare: call={ordinal} caller={return_address:#010x} decoded=true armed={armed} adapter={}",
            host.is_some()
        );
    }
}

pub struct NativeParticles;
impl Feature for NativeParticles {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "Native dynamic particle submissions; guest updates preserved",
        )
        .doc("docs/graphics/native-particles.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .before(&EMIT, "native_particles_emit", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                on_emit(&mut *mem, c.cpu.esp, c.cpu.ebx);
                Ok(Flow::Continue)
            })
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
