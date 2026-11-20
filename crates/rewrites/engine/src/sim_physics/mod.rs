//! Native simulation entries for the base image. Calls push the real return
//! PC. The scheduler can stop in a child and recover the entire caller from
//! the guest stack, just as it can for the generated implementation.

use crate::points::{self, Point};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

mod colors;
mod havok;
mod solver_rows;
mod solver_pairs;
mod first_person;
mod pose_layers;
mod light;
mod support;
mod observer;
mod objects;
mod object_update;
mod node_matrices;
mod spatial;
mod visit;
mod unit_aim;
mod weld;
mod weapon_slots;

pub type Result<T> = std::result::Result<T, GuestFault>;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Controls { Integer, Sse, Mixed, Trig, Camera, Weld }

pub struct Port {
    pub point: &'static Point,
    /// Passed to Switches::implementation, which supplies the impl. prefix.
    pub switch: &'static str,
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
    switch: &'static str,
    controls: Controls,
    resumes: &'static [u32],
    run: fn(&mut CpuAbi, &mut Guest, u32) -> Result<TransferAbi>,
) -> Port {
    Port { point, switch, controls, resumes, run, generated_sites: &[] }
}

pub const PORTS: &[Port] = {
    use Controls::*;
    use points::*;
    &[
        Port {
            generated_sites: havok::SOLVER_REST,
            ..port(&HAVOK_SOLVER, "sim.native_havok_solver", Mixed, havok::SOLVER_SITES, havok::solve)
        },
        port(&HAVOK_SUPPORT_VERTICES, "sim.native_havok_support_vertices", Mixed, support::SITES, support::vertices),
        port(&HAVOK_WELD_CONTACTS, "sim.native_havok_weld", Weld, weld::SITES, weld::contacts),
        // The pose root runs under the pose-matrix switch: it inlines that
        // leaf and transform composition for the whole skeleton walk.
        port(&OBJECT_NODE_MATRICES, "sim.native_pose_matrix", Sse, &node_matrices::SITES, node_matrices::update),
        port(&OBJECT_CHANGE_COLORS, "sim.native_object_change_colors", Sse, &colors::SITES, colors::update),
        port(&OBJECT_SPATIAL_VISIT, "sim.native_spatial_visit", Sse, &visit::SITES, visit::objects),
        port(&OBJECT_SPATIAL_PORTAL, "sim.native_spatial_portal", Mixed, &spatial::SITES, spatial::portal),
        port(&UNIT_AIM_UPDATE, "sim.native_unit_aim", Trig, unit_aim::SITES, unit_aim::aim),
        port(&UNIT_UPDATE, "sim.native_unit_update", Integer, &objects::UNIT_SITES, objects::unit),
        port(&LIGHT_RECORD_UPDATE, "sim.native_light_record", Sse, light::SITES, light::update),
        port(&FIRST_PERSON_POSE_UPDATE, "sim.native_first_person_pose", Mixed, first_person::POSE_SITES, first_person::pose),
        port(&FIRST_PERSON_FILTER_UPDATE, "sim.native_first_person_filters", Mixed, first_person::FILTER_SITES, first_person::filters),
        port(&OBSERVER_RESULT, "sim.native_observer_camera", Camera, &[], observer::camera),
    ]
};

/// Ports kept as test oracles only, never installed: the subsystem that
/// replaces them owns the switch now (docs/engine/sim-rewrite.md section 8).
/// The additive-layer node evaluator runs under `0x2798a0`, which
/// `impl.sim.native.anim` replaces whole (docs/engine/anim-sampling.md).
/// The object walks and per-object updates are bodies under
/// `impl.sim.native.objects` (`crate::sim_objects`); their `oracle.*`
/// names are labels, not switches.
pub const ORACLES: &[Port] = {
    use Controls::*;
    use points::*;
    &[
        port(&OBJECTS_PRE_PHYSICS, "oracle.objects_pre_physics", Integer, &objects::PRE_SITES, objects::pre),
        port(&OBJECTS_POST_PHYSICS, "oracle.objects_post_physics", Integer, &objects::POST_SITES, objects::post),
        port(&OBJECT_PRE_UPDATE, "oracle.object_pre_update", Integer, &object_update::PRE, object_update::pre),
        port(&OBJECT_POST_UPDATE, "oracle.object_post_update", Integer, &object_update::POST, object_update::post),
        // Other codecs are guest calls that resume at the call's return.
        port(&ANIM_ADDITIVE_NODES, "sim.native.anim", Sse, &crate::sim_native::evaluator::ADDITIVE_SITES, crate::sim_native::evaluator::additive),
    ]
};

pub(crate) fn push(c: &mut CpuAbi, g: &mut Guest, word: u32) -> Result<()> {
    let sp = c.esp.wrapping_sub(4);
    g.set_u32(sp, word)?;
    c.esp = sp;
    Ok(())
}

pub(crate) fn pop(c: &mut CpuAbi, g: &Guest) -> Result<u32> {
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

fn call_arg(c: &mut CpuAbi, g: &mut Guest, arg: u32, target: u32, site: u32) -> Result<TransferAbi> {
    push(c, g, arg)?;
    g.call(c, target, site)
}

// A mapping can be an alias of another range of guest virtual addresses.
// A bulk copy or a deferred store needs disjoint *physical* spans. Fall
// back to ordered accesses for ranges that cross a mapping boundary.
fn physical_span(g: &Guest, addr: u32, len: u32) -> Option<std::ops::Range<u32>> {
    let maps = g.mappings();
    let end = maps.partition_point(|m| m.virtual_start <= addr);
    let mapping = maps.get(end.checked_sub(1)?)?;
    let offset = addr - mapping.virtual_start;
    if offset.checked_add(len)? > mapping.len { return None; }
    let start = mapping.physical_start + offset;
    Some(start..start+len)
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
        Sse => cpu.mxcsr & 0x1f80 == 0x1f80,
        Weld => x87_masked && weld::supported(guest),
        Mixed | Trig | Camera => {
            let masked = x87_masked && cpu.mxcsr & 0x1f80 == 0x1f80;
            masked && (port.controls == Mixed || cpu.x87.control & 0xc00 == 0)
                && (port.controls != Camera || observer::disjoint(cpu, guest))
        }
    }
}
