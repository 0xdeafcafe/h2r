//! Particle submission observer, off by default. The helper and all its
//! parents still run: especially the state updates in `0x00174420`.
//! A host may replace only the helper's single, fully captured draw, after
//! every target copy is prepared. It must preserve attribute carry/state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{BuildError, Builder, Feature, Manifest};
use engine::memory::GuestMemory;
use engine::points::{Point, Sig};
use native_render::camera::GuestRead;
use native_render::effect_geometry::{self, DynamicCall};
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

// Publication boundaries, observed before the indicated instruction. These
// keep every hook context-only: no Rust continuation around a guest body.
pub const EMIT_PUBLISH: Point = Point::new(
    "PARTICLE_PUBLISH",
    0x0023_ef77,
    Sig::Opaque,
    "docs/graphics/native-particles.md",
);
pub const EMIT_END: Point = Point::new(
    "PARTICLE_END",
    0x0023_ef79,
    Sig::Opaque,
    "docs/graphics/native-particles.md",
);
pub const DYNAMIC: Point = Point::new(
    "EFFECT_DYNAMIC",
    effect_geometry::DYNAMIC_DRAW,
    Sig::Opaque,
    "docs/graphics/native-particles.md",
);
pub const DYNAMIC_PUBLISH: Point = Point::new(
    "EFFECT_PUBLISH",
    0x0004_b286,
    Sig::Opaque,
    "docs/graphics/native-particles.md",
);
pub const DYNAMIC_END: Point = Point::new(
    "EFFECT_END",
    0x0004_b2a3,
    Sig::Opaque,
    "docs/graphics/native-particles.md",
);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Producer {
    Particle,
    Dynamic,
}

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
    fn arm_dynamic(&self, mem: &mut dyn GuestMemory, call: &DynamicCall, mode: Mode) -> bool;
    fn publish(&self, mem: &mut dyn GuestMemory, producer: Producer, data: u32);
    fn finish(&self, mem: &mut dyn GuestMemory, producer: Producer);
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CALLS: AtomicU64 = AtomicU64::new(0);
static DYNAMIC_CALLS: AtomicU64 = AtomicU64::new(0);
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

pub fn on_dynamic(mem: &mut dyn GuestMemory, esp: u32, stride: u32) {
    let Ok(call) = effect_geometry::read_call(&Read(mem), esp, stride) else {
        return;
    };
    let ordinal = DYNAMIC_CALLS.fetch_add(1, Ordering::Relaxed);
    let compare = compare_budget();
    if compare.is_some_and(|budget| ordinal >= budget) {
        return;
    }
    let mode = if compare.is_some() {
        Mode::Compare
    } else {
        Mode::Native
    };
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let armed = host
        .as_ref()
        .is_some_and(|h| h.arm_dynamic(mem, &call, mode));
    if mode == Mode::Compare {
        eprintln!(
            "native-particles-compare: dynamic={ordinal} kind={:?} vertices={} armed={armed}",
            call.kind, call.vertices
        );
    }
}
fn boundary(mem: &mut dyn GuestMemory, producer: Producer, data: Option<u32>) {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(h) = host {
        if let Some(data) = data {
            h.publish(mem, producer, data);
        } else {
            h.finish(mem, producer);
        }
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
            .observe_before(&EMIT, "native_particles_emit", |o| {
                let mut mem = o.guest();
                on_emit(&mut *mem, o.cpu.esp, o.cpu.ebx);
            })
            .context_only();
        b.raw()
            .observe_before(&DYNAMIC, "native_particles_dynamic", |o| {
                let mut mem = o.guest();
                on_dynamic(&mut *mem, o.cpu.esp, o.cpu.ecx);
            })
            .context_only();
        b.raw()
            .observe_before(&EMIT_PUBLISH, "native_particles_publish", |o| {
                let mut mem = o.guest();
                boundary(&mut *mem, Producer::Particle, Some(0));
            })
            .context_only();
        b.raw()
            .observe_before(&DYNAMIC_PUBLISH, "native_particles_dynamic_publish", |o| {
                let mut mem = o.guest();
                boundary(&mut *mem, Producer::Dynamic, Some(o.cpu.esi));
            })
            .context_only();
        for (point, producer) in [
            (&EMIT_END, Producer::Particle),
            (&DYNAMIC_END, Producer::Dynamic),
        ] {
            b.raw()
                .observe_before(point, "native_particles_end", move |o| {
                    let mut mem = o.guest();
                    boundary(&mut *mem, producer, None);
                })
                .context_only();
        }
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
