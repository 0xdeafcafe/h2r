//! Native simulation entries for the base image. Calls push the real return
//! PC. The scheduler can stop in a child and recover the entire caller from
//! the guest stack, just as it can for the generated implementation.

use crate::points::{self, Point};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

mod objects;
mod object_update;
mod spatial;

pub type Result<T> = std::result::Result<T, GuestFault>;
pub struct Port {
    pub point: &'static Point,
    /// Passed to Switches::implementation, which supplies the impl. prefix.
    pub switch: &'static str,
    pub resumes: &'static [u32],
    pub run: fn(&mut CpuAbi, &mut Guest, u32) -> Result<TransferAbi>,
}

pub const PORTS: &[Port] = &[
    Port {
        point: &points::OBJECT_PRE_UPDATE, switch: "sim.native_object_pre_update",
        resumes: &object_update::PRE, run: object_update::pre,
    },
    Port {
        point: &points::OBJECT_POST_UPDATE, switch: "sim.native_object_post_update",
        resumes: &object_update::POST, run: object_update::post,
    },
    Port {
        point: &points::OBJECT_SPATIAL_PORTAL,
        switch: "sim.native_spatial_portal",
        resumes: &spatial::SITES,
        run: spatial::portal,
    },
    Port {
        point: &points::OBJECTS_PRE_PHYSICS,
        switch: "sim.native_objects_pre_physics",
        resumes: &objects::PRE_SITES,
        run: objects::pre,
    },
    Port {
        point: &points::OBJECTS_POST_PHYSICS,
        switch: "sim.native_objects_post_physics",
        resumes: &objects::POST_SITES,
        run: objects::post,
    },
    Port {
        point: &points::UNIT_UPDATE,
        switch: "sim.native_unit_update",
        resumes: &objects::UNIT_SITES,
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
