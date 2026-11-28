//! Native simulation entries for the base image. Calls push the real return
//! PC. The scheduler can stop in a child and recover the entire caller from
//! the guest stack, just as it can for the generated implementation.

use guest_layout::points::{self, Point};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

mod alloc;
mod answer;
mod colors;
mod havok;
mod island_solve;
pub mod contact_commit;
pub mod contact_point;
pub mod convex_agent;
pub mod convex_query;
pub mod deactivation;
pub mod integrate_phase;
pub mod island_integrate;
pub mod structure_polygon;
mod solver_rows;
mod solver_pairs;
pub(crate) mod first_person;
mod pose_layers;
mod light;
mod support;
pub(crate) mod observer;
mod objects;
mod object_update;
pub(crate) mod node_matrices;
pub(crate) mod spatial;
pub(crate) mod visit;
pub(crate) mod unit_aim;
mod weld;
mod weapon_slots;
mod broadphase;
mod motion;
pub mod parity;
mod world;

pub type Result<T> = std::result::Result<T, GuestFault>;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Controls { Integer, Sse, Mixed, Trig, Camera, Weld, World, Memory, Island }

pub struct Port {
    pub point: &'static Point,
    /// The hook label and shadow name (`sim.port.<name>`); not a switch.
    pub name: &'static str,
    pub resumes: &'static [u32],
    pub generated_sites: &'static [u32],
    run: fn(&mut CpuAbi, &mut Guest, u32) -> Result<TransferAbi>,
    controls: Controls,
}

impl Port {
    /// Run the entry or a return continuation. Arithmetic faults have no
    /// guest memory operand, so report the selector that entered the kernel.
    pub fn invoke(&self, cpu: &mut CpuAbi, guest: &mut Guest, pc: u32) -> Result<TransferAbi> {
        (self.run)(cpu, guest, pc).map_err(|mut fault| {
            if fault.len == 0 && fault.reason == runtime::guest::FaultReason::InvalidOperation {
                fault.addr = pc;
            }
            fault
        })
    }
}

const fn port(
    point: &'static Point,
    name: &'static str,
    controls: Controls,
    resumes: &'static [u32],
    run: fn(&mut CpuAbi, &mut Guest, u32) -> Result<TransferAbi>,
) -> Port {
    Port { point, name, controls, resumes, run, generated_sites: &[] }
}

pub const PORTS: &[Port] = {
    use Controls::*;
    use points::*;
    &[
        // Labels, not switches. A port installs where no subsystem body owns
        // its point (`sim_bridge::sim_physics`): the first-person pose; the
        // pose root `bd090` is an object body, the aim and filters anim's.
        // The pose root inlines the pose matrix and transform composition
        // for the whole skeleton walk.
        port(&OBJECT_NODE_MATRICES, "sim.port.pose_matrix", Sse, &node_matrices::SITES, node_matrices::update),
        port(&UNIT_AIM_UPDATE, "sim.port.unit_aim", Trig, unit_aim::SITES, unit_aim::aim),
        port(&FIRST_PERSON_POSE_UPDATE, "sim.port.first_person_pose", Mixed, first_person::POSE_SITES, first_person::pose),
        port(&FIRST_PERSON_FILTER_UPDATE, "sim.port.first_person_filters", Mixed, first_person::FILTER_SITES, first_person::filters),
    ]
};

/// The island integrate's frame replay: runs a call the integrate arm
/// (`parity::integrate_enter`) declines. Installed by the arm's implementation, not with
/// [`PHYSICS_PORTS`], because both share the entry.
pub const ISLAND_INTEGRATE: Port =
    port(&points::HAVOK_ISLAND_INTEGRATE, "sim.native.physics", Controls::World, world::INTEGRATE_SITES, world::integrate_island);

/// The Havok world entries of phase 5, always installed
/// (docs/engine/sim-rewrite-havok.md §7.8): the step wrapper and the
/// island collide, which replay the retail frame, and the leaves below.
/// The world phases between them are lifted code and drive these entries
/// through the dispatcher. The island integrate is [`ISLAND_INTEGRATE`].
pub const PHYSICS_PORTS: &[Port] = {
    use points::*;
    &[
        port(&HAVOK_WORLD_STEP, "sim.native.physics", Controls::World, world::WORLD_SITES, world::step),
        port(&HAVOK_ISLAND_COLLIDE, "sim.native.physics", Controls::World, world::ISLAND_SITES, world::collide_island),
        // The island solve: no guest call inside; an island the model does
        // not cover runs the lifted entry and is counted
        // (`sim_havok::island_solve::fallbacks`).
        port(&HAVOK_ISLAND_SOLVE, "sim.native.physics", Controls::Island, &[], island_solve::run),
        // The memory system's virtuals: Halo's builders call them too
        // (docs/engine/havok-step.md, "Allocator"). No guest call inside.
        port(&HAVOK_ALLOC, "sim.native.physics", Controls::Memory, &[], alloc::run),
        port(&HAVOK_FREE, "sim.native.physics", Controls::Memory, &[], alloc::run),
        port(&HAVOK_LIFO_ALLOC, "sim.native.physics", Controls::Memory, &[], alloc::run),
        port(&HAVOK_LIFO_FREE, "sim.native.physics", Controls::Memory, &[], alloc::run),
    ]
};

/// Ports kept as test oracles only, never installed: the subsystem that
/// replaces them owns the switch now (docs/engine/sim-rewrite.md section 8).
/// The additive-layer node evaluator runs under `0x2798a0`, which
/// the animation group replaces whole (docs/engine/anim-sampling.md).
/// The object walks, the per-object updates, change colours, the light
/// record update and the unit update's shell are object bodies
/// (`crate::sim_objects`); their `oracle.*` names
/// are labels, not switches. The unit aim port stays in `PORTS` under its
/// label, and the unit movement body runs the same code
/// (`unit_aim::step`). The observer camera is a seat
/// (`crate::sim_seats::camera`), always installed.
pub const ORACLES: &[Port] = {
    use Controls::*;
    use points::*;
    &[
        // The Havok per-function ports retired by the physics subsystem
        // (sim-rewrite-havok.md, slice 8): their bodies are the differential
        // oracles for the phase 5 kernels (sim_havok::solver, support, weld).
        Port {
            generated_sites: havok::SOLVER_REST,
            ..port(&HAVOK_SOLVER, "oracle.havok_solver", Mixed, havok::SOLVER_SITES, havok::solve)
        },
        port(&HAVOK_CLOSEST_QUERY, "oracle.havok_support_vertices", Mixed, support::SITES, support::vertices),
        port(&HAVOK_WELD_CONTACTS, "oracle.havok_weld", Weld, weld::SITES, weld::contacts),
        port(&OBJECT_SPATIAL_VISIT, "oracle.spatial_visit", Sse, &visit::SITES, visit::objects),
        port(&OBJECT_SPATIAL_PORTAL, "oracle.spatial_portal", Mixed, &spatial::SITES, spatial::portal),
        port(&OBJECTS_PRE_PHYSICS, "oracle.objects_pre_physics", Integer, &objects::PRE_SITES, objects::pre),
        port(&OBJECTS_POST_PHYSICS, "oracle.objects_post_physics", Integer, &objects::POST_SITES, objects::post),
        port(&OBJECT_PRE_UPDATE, "oracle.object_pre_update", Integer, &object_update::PRE, object_update::pre),
        port(&OBJECT_POST_UPDATE, "oracle.object_post_update", Integer, &object_update::POST, object_update::post),
        port(&OBJECT_CHANGE_COLORS, "oracle.object_change_colors", Sse, &colors::SITES, colors::update),
        // Slice 5: the light record update is a body (`sim_objects::lights`).
        port(&LIGHT_RECORD_UPDATE, "oracle.light_record", Sse, light::SITES, light::update),
        // Slice 6a: the unit update's shell is a body (`crate::sim_objects::unit`).
        port(&UNIT_UPDATE, "oracle.unit_update", Integer, &objects::UNIT_SITES, objects::unit),
        // Other codecs are guest calls that resume at the call's return.
        port(&ANIM_ADDITIVE_NODES, "sim.native.anim", Sse, &crate::sim_native::evaluator::ADDITIVE_SITES, crate::sim_native::evaluator::additive),
        port(&OBSERVER_RESULT, "oracle.observer_camera", Camera, &[], observer::camera),
    ]
};

pub(crate) fn push(c: &mut CpuAbi, g: &mut impl crate::sim_native::Memory, word: u32) -> Result<()> {
    let sp = c.esp.wrapping_sub(4);
    g.set_u32(sp, word)?;
    c.esp = sp;
    Ok(())
}

pub(crate) fn pop(c: &mut CpuAbi, g: &impl crate::sim_native::Memory) -> Result<u32> {
    let v = g.u32(c.esp)?;
    c.esp = c.esp.wrapping_add(4);
    Ok(v)
}

pub(crate) fn byte(word: u32, value: u8) -> u32 {
    (word & !0xff) | u32::from(value)
}

// EFLAGS at externally visible transfers. Undefined bits clear in both
// EFLAGS and the defined mask, following the IR contract.
pub(crate) const ARITH: u32 = 0x8d5;
pub(crate) fn status(result: u32, sign: u32) -> u32 {
    (u32::from(result == 0) << 6)
        | (u32::from((result as u8).count_ones().is_multiple_of(2)) << 2)
        | (u32::from(result & sign != 0) << 7)
}

pub(crate) fn logic(c: &mut CpuAbi, result: u32, sign: u32) {
    c.eflags = (c.eflags & !ARITH) | status(result, sign);
    c.eflags_defined = (c.eflags_defined | ARITH) & !0x10;
}

pub(crate) fn arithmetic(c: &mut CpuAbi, a: u32, b: u32, sub: bool) -> u32 {
    let (v, carry) = if sub { a.overflowing_sub(b) } else { a.overflowing_add(b) };
    let overflow = (if sub { a ^ b } else { !(a ^ b) }) & (a ^ v);
    let flags = status(v, 1 << 31) | u32::from(carry)
        | ((a ^ b ^ v) & 0x10) | ((overflow >> 20) & 0x800);
    c.eflags = (c.eflags & !ARITH) | flags;
    c.eflags_defined |= ARITH;
    v
}

fn call_arg(c: &mut CpuAbi, g: &mut impl crate::sim_native::Memory, arg: u32, target: u32, site: u32) -> Result<TransferAbi> {
    push(c, g, arg)?;
    g.call(c, target, site)
}

// A mapping can be an alias of another range of guest virtual addresses.
// A bulk copy or a deferred store needs disjoint *physical* spans. A range
// across page mappings qualifies when its physical pages run on
// (`sim_native::plan::contiguous`); others take ordered accesses.
pub(crate) fn physical_span(g: &Guest, addr: u32, len: u32) -> Option<std::ops::Range<u32>> {
    let start = crate::sim_native::plan::contiguous(g, addr, len as usize)?;
    Some(start..start.checked_add(len)?)
}

fn overlap(a: &std::ops::Range<u32>, b: &std::ops::Range<u32>) -> bool {
    a.start < b.end && b.start < a.end
}

/// Native FP kernels handle masked exceptions. The original owns synchronous
/// exception delivery and the non-nearest x87 transcendental paths.
pub fn supported(port: &Port, cpu: &CpuAbi, guest: &Guest) -> bool {
    use Controls::*;
    let x87_masked = cpu.x87.control & 0x3f == 0x3f && cpu.x87.status & 0x80 == 0;
    match port.controls {
        Integer => true,
        Memory => alloc::supported(port.point.base, cpu, guest),
        // The entry appends a monitor record while the stream is on.
        Island => world::monitor_off(guest) && island_solve::supported(cpu, guest),
        Sse => cpu.mxcsr & 0x1f80 == 0x1f80,
        Weld => x87_masked && weld::supported(guest),
        World => {
            let masked = x87_masked && cpu.mxcsr & 0x1f80 == 0x1f80;
            masked && cpu.x87.control & 0xc00 == 0 && world::monitor_off(guest)
        }
        Mixed | Trig | Camera => {
            let masked = x87_masked && cpu.mxcsr & 0x1f80 == 0x1f80;
            masked && (port.controls == Mixed || cpu.x87.control & 0xc00 == 0)
                && (port.controls != Camera || observer::disjoint(cpu, guest))
        }
    }
}
