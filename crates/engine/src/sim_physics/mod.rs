//! Native simulation entries for the base image. Calls push the real return
//! PC. The scheduler can stop in a child and recover the entire caller from
//! the guest stack, just as it can for the generated implementation.

use crate::points::{self, Point};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

mod colors;
mod havok;
mod first_person;
mod pose_layers;
mod light;
mod support;
mod observer;
mod objects;
mod object_update;
mod spatial;
mod visit;
mod unit_aim;
mod weld;
mod weapon_slots;

pub type Result<T> = std::result::Result<T, GuestFault>;
pub struct Port {
    pub point: &'static Point,
    /// Passed to Switches::implementation, which supplies the impl. prefix.
    pub switch: &'static str,
    pub resumes: &'static [u32],
    pub generated_sites: &'static [u32],
    pub run: fn(&mut CpuAbi, &mut Guest, u32) -> Result<TransferAbi>,
}

pub const PORTS: &[Port] = &[
    Port { point: &points::UNIT_AIM_UPDATE, switch: "sim.native_unit_aim",
           resumes: unit_aim::SITES, generated_sites: &[], run: unit_aim::aim },
    Port { point: &points::HAVOK_WELD_CONTACTS, switch: "sim.native_havok_weld",
           resumes: weld::SITES, generated_sites: &[], run: weld::contacts },
    Port { point: &points::FIRST_PERSON_POSE_UPDATE, switch: "sim.native_first_person_pose",
           resumes: first_person::POSE_SITES, generated_sites: first_person::POSE_REST, run: first_person::pose },
    Port { point: &points::FIRST_PERSON_FILTER_UPDATE, switch: "sim.native_first_person_filters",
           resumes: first_person::FILTER_SITES, generated_sites: first_person::FILTER_REST, run: first_person::filters },
    Port { point: &points::LIGHT_RECORD_UPDATE, switch: "sim.native_light_record",
           resumes: light::SITES, generated_sites: &[], run: light::update },
    Port { point: &points::HAVOK_SOLVER, switch: "sim.native_havok_solver",
           resumes: havok::SOLVER_SITES, generated_sites: havok::SOLVER_REST, run: havok::solve },
    Port { point: &points::HAVOK_SUPPORT_VERTICES, switch: "sim.native_havok_support_vertices",
           resumes: support::SITES, generated_sites: support::REST, run: support::vertices },
    Port { point: &points::OBSERVER_RESULT, switch: "sim.native_observer_camera",
           generated_sites: &[], resumes: &[], run: observer::camera },
    Port { point: &points::OBJECT_CHANGE_COLORS, switch: "sim.native_object_change_colors",
           generated_sites: &[], resumes: &colors::SITES, run: colors::update },
    Port { point: &points::OBJECT_SPATIAL_VISIT, switch: "sim.native_spatial_visit",
           generated_sites: &[], resumes: &visit::SITES, run: visit::objects },
    Port {
        point: &points::OBJECT_PRE_UPDATE, switch: "sim.native_object_pre_update",
        generated_sites: &[], resumes: &object_update::PRE, run: object_update::pre,
    },
    Port {
        point: &points::OBJECT_POST_UPDATE, switch: "sim.native_object_post_update",
        generated_sites: &[], resumes: &object_update::POST, run: object_update::post,
    },
    Port {
        point: &points::OBJECT_SPATIAL_PORTAL,
        switch: "sim.native_spatial_portal",
        generated_sites: &[], resumes: &spatial::SITES,
        run: spatial::portal,
    },
    Port {
        point: &points::OBJECTS_PRE_PHYSICS,
        switch: "sim.native_objects_pre_physics",
        generated_sites: &[], resumes: &objects::PRE_SITES,
        run: objects::pre,
    },
    Port {
        point: &points::OBJECTS_POST_PHYSICS,
        switch: "sim.native_objects_post_physics",
        generated_sites: &[], resumes: &objects::POST_SITES,
        run: objects::post,
    },
    Port {
        point: &points::UNIT_UPDATE,
        switch: "sim.native_unit_update",
        generated_sites: &[], resumes: &objects::UNIT_SITES,
        run: objects::unit,
    },
];

fn push(c: &mut CpuAbi, g: &mut Guest, word: u32) -> Result<()> {
    let sp = c.esp.wrapping_sub(4);
    g.set_u32(sp, word)?;
    c.esp = sp;
    Ok(())
}

fn pop(c: &mut CpuAbi, g: &Guest) -> Result<u32> {
    let v = g.u32(c.esp)?;
    c.esp = c.esp.wrapping_add(4);
    Ok(v)
}

fn byte(word: u32, value: u8) -> u32 {
    (word & !0xff) | u32::from(value)
}

// EFLAGS at externally visible transfers. Undefined bits clear in both
// EFLAGS and the defined mask, following the IR contract.
const ARITH: u32 = 0x8d5;
fn status(result: u32, sign: u32) -> u32 {
    (u32::from(result == 0) << 6)
        | (u32::from((result as u8).count_ones().is_multiple_of(2)) << 2)
        | (u32::from(result & sign != 0) << 7)
}

fn logic(c: &mut CpuAbi, result: u32, sign: u32) {
    c.eflags = (c.eflags & !ARITH) | status(result, sign);
    c.eflags_defined = (c.eflags_defined | ARITH) & !0x10;
}

fn arithmetic(c: &mut CpuAbi, a: u32, b: u32, sub: bool) -> u32 {
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

/// The FP kernels use the masked arithmetic path. Take the generated path
/// for a control word that can deliver a synchronous FP exception. The
/// x87 transcendental oracle supports nearest rounding.
pub fn supported(port: &Port, cpu: &CpuAbi, guest: &Guest) -> bool {
    let entry = port.point.base;
    if entry == points::UNIT_AIM_UPDATE.base && cpu.x87.control & 0xc00 != 0 {return false;}
    if entry == points::HAVOK_WELD_CONTACTS.base && !weld::supported(guest) {return false;}
    if entry == points::OBSERVER_RESULT.base {
        if cpu.x87.control & 0xc00 != 0 || !observer::disjoint(cpu, guest) {
            return false;
        }
    }
    if [points::FIRST_PERSON_POSE_UPDATE.base, points::HAVOK_SUPPORT_VERTICES.base, points::UNIT_AIM_UPDATE.base, points::HAVOK_WELD_CONTACTS.base, points::FIRST_PERSON_FILTER_UPDATE.base, points::HAVOK_SOLVER.base, points::OBJECT_SPATIAL_PORTAL.base, points::OBSERVER_RESULT.base].contains(&entry)
        && cpu.x87.control & 0x3f != 0x3f { return false; }
    ![points::FIRST_PERSON_POSE_UPDATE.base, points::UNIT_AIM_UPDATE.base, points::FIRST_PERSON_FILTER_UPDATE.base, points::LIGHT_RECORD_UPDATE.base, points::HAVOK_SOLVER.base, points::HAVOK_SUPPORT_VERTICES.base, points::OBJECT_SPATIAL_PORTAL.base, points::OBJECT_SPATIAL_VISIT.base,
      points::OBJECT_CHANGE_COLORS.base, points::OBSERVER_RESULT.base].contains(&entry)
        || cpu.mxcsr & 0x1f80 == 0x1f80
}
