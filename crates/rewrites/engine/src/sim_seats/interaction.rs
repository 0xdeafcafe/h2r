//! The interaction search `0x151110` (docs/perf/sim-phantom-build.md,
//! "The builder body in Rust"): the weapons, vehicles, bipeds and
//! devices in reach of the player's unit, each one's test writing the
//! 0xc-byte record at `out` (`ebp + 0x6c` of the action builder).
//!
//! The body clears the record, asks the spatial query `0xbb050` for at
//! most 64 objects in a sphere about the unit, and walks them: a type in
//! `0x1430` is skipped; a type in `0x184` must reach the unit's sphere;
//! the weapon, device, vehicle and biped tests are guest calls; any other
//! type adds its children to the list while it has room. A register-level
//! body as [`super::builder`]; it declines (nothing written, the original
//! runs) on a float environment it does not take, a list count past 64,
//! a jump table entry the image does not have, or a faulting access.
use crate::sim_native::{Memory, Result, plan::segment, sub_flags};
use crate::sim_objects::sse::{self, Ss, lane0, load, movaps, ss, ss_reg};
use crate::sim_physics::{arithmetic, logic, pop, push};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

pub const SEARCH: u32 = 0x0015_1110;
/// The return sites, sorted.
pub const SITES: &[u32] = &[0x1511bc, 0x151281, 0x151291, 0x1512a1, 0x1512b1];
const TEXT: std::ops::Range<u32> = SEARCH..0x0015_12fc;

pub const SPATIAL_QUERY: u32 = 0x000b_b050;
/// The type tests, by the jump table's targets.
pub const WEAPON_TEST: u32 = 0x0015_1780;
pub const DEVICE_TEST: u32 = 0x0015_16c0;
pub const VEHICLE_TEST: u32 = 0x0015_1430;
pub const BIPED_TEST: u32 = 0x0015_15e0;
/// The list's room, and the query's limit.
pub const LIST: u32 = 0x40;

const PLAYERS: u32 = 0x004e_8c24;
const OBJECT_HEADER: u32 = 0x004e_0300;
const REACH: u32 = 0x0045_dd30;
const MARGIN: u32 = 0x0045_dbd4;
const JUMP_TABLE: u32 = 0x0015_12fc;
const FRAME: u32 = 0x10c;

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}

fn logic32(c: &mut CpuAbi, v: u32) {
    logic(c, v, 1 << 31);
}

fn declined(pc: u32) -> GuestFault {
    crate::sim_native::refuse(pc, crate::sim_native::Decline::Boundary)
}

/// One body segment.
pub fn run(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> std::result::Result<Option<TransferAbi>, GuestFault> {
    // A segment can walk the whole list: one index store and up to one
    // child store an object.
    segment::<192, _>(c, g, pc, TEXT, SITES, |c, p, pc| step(c, p, pc))
}

fn step(c: &mut CpuAbi, m: &mut impl Memory, pc: u32) -> Result {
    match pc {
        SEARCH => enter(c, m),
        0x1511bc => {
            c.ebx = c.eax as u16 as i16 as i32 as u32;
            logic32(c, c.ebx);
            m.set_u32(at(c.esp, 0xc), 0)?;
            if c.ebx as i32 <= 0 {
                c.ebx = pop(c, m)?;
                return leave(c, m);
            }
            if c.ebx > LIST {
                return Err(crate::sim_native::unported(pc, "more objects than the search record holds"));
            }
            push(c, m, c.edi)?;
            walk(c, m, false)
        }
        0x151281 | 0x151291 | 0x1512a1 | 0x1512b1 => walk(c, m, true),
        _ => Err(declined(pc)),
    }
}

/// `0x151110`..`0x1511b7`: the record cleared, then the query about the
/// unit, which must have no parent.
fn enter(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    c.eax = m.u32(at(c.esp, 4))?;
    c.ecx = m.u32(PLAYERS)?;
    c.esp = c.esp.wrapping_sub(FRAME);
    push(c, m, c.ebp)?;
    c.ebp = m.u32(at(c.esp, 0x118))?;
    c.eax &= 0xffff;
    c.edx = 0;
    // `imul eax, 21ch` after the `xor`: replaced by the `or` below.
    c.eax = c.eax.wrapping_mul(0x21c);
    push(c, m, c.esi)?;
    c.esi = m.u32(at(c.ecx, 0x44))?;
    c.ecx = c.ebp;
    for offset in [0, 4, 8, 0, 4] {
        m.set_u32(at(c.ecx, offset), c.edx)?;
    }
    c.ecx = u32::MAX;
    logic32(c, c.ecx);
    c.eax = arithmetic(c, c.eax, c.esi, false);
    m.set_u32(at(c.ebp, 4), c.ecx)?;
    m.set_u32(at(c.ebp, 8), c.ecx)?;
    c.eax = m.u32(at(c.eax, 0x2c))?;
    sub_flags(c, c.eax, c.ecx, 32);
    if c.eax == c.ecx {
        return leave(c, m);
    }
    c.eax &= 0xffff;
    logic32(c, c.eax);
    c.edx = c.eax.wrapping_mul(3);
    c.eax = m.u32(OBJECT_HEADER)?;
    c.eax = m.u32(at(c.eax, 0x44))?;
    c.eax = m.u32(at(at(c.eax, c.edx.wrapping_mul(4)), 8))?;
    let parent = m.u32(at(c.eax, 0x14))?;
    sub_flags(c, parent, c.ecx, 32);
    if parent != c.ecx {
        return leave(c, m);
    }
    load(c, 0, m.u32(at(c.eax, 0x3c))?);
    ss(c, Ss::Add, 0, m.u32(REACH)?);
    push(c, m, c.ebx)?;
    push(c, m, LIST)?;
    c.ecx = at(c.eax, 0x30);
    c.edx = at(c.esp, 0x1c);
    push(c, m, c.edx)?;
    push(c, m, c.ecx)?;
    m.set_u32(at(c.esp, 0x1c), lane0(c, 0))?;
    ss(c, Ss::Add, 0, m.u32(MARGIN)?);
    m.set_u32(c.esp, lane0(c, 0))?;
    push(c, m, c.ecx)?;
    c.eax = arithmetic(c, c.eax, 0x28, false);
    push(c, m, c.eax)?;
    push(c, m, 0xffff_ebcf)?;
    push(c, m, 0)?;
    m.set_u32(at(c.esp, 0x30), c.ecx)?;
    m.call(c, SPATIAL_QUERY, 0x1511bc)
}

/// `0x1511d0`..`0x1512ef`, from the loop's head or, `next`, from a type
/// test's return (`0x1512de`): each listed object in turn until a type
/// test is called or the list ends.
fn walk(c: &mut CpuAbi, m: &mut impl Memory, mut next: bool) -> Result {
    loop {
        if next {
            // 0x1512de; `inc eax`: replaced by the `cmp` after it.
            c.eax = m.u32(at(c.esp, 0x10))?.wrapping_add(1);
            sub_flags(c, c.eax, c.ebx, 32);
            m.set_u32(at(c.esp, 0x10), c.eax)?;
            if (c.eax as i32) >= (c.ebx as i32) {
                c.edi = pop(c, m)?;
                c.ebx = pop(c, m)?;
                return leave(c, m);
            }
        }
        next = true;
        // 0x1511d0
        let sp = c.esp;
        c.eax = m.u32(at(sp, 0x10))?;
        c.ecx = m.u32(OBJECT_HEADER)?;
        c.edi = m.u32(at(c.ecx, 0x44))?;
        c.eax = m.u32(at(at(sp, c.eax.wrapping_mul(4)), 0x1c))?;
        c.ecx = c.eax & 0xffff;
        logic32(c, c.ecx);
        c.edx = c.ecx.wrapping_mul(3);
        c.esi = m.u32(at(at(c.edi, c.edx.wrapping_mul(4)), 8))?;
        c.ecx = m.u8(at(c.esi, 0xaa))? as i8 as i32 as u32;
        // `shl edx, cl`: replaced by the `test` below.
        c.edx = 1u32 << (c.ecx & 31);
        let skipped = c.edx & 0x1430;
        logic32(c, skipped);
        if skipped != 0 {
            continue;
        }
        let near = c.edx & 0x184;
        logic32(c, near);
        if near != 0 && !reaches(c, m, sp)? {
            continue;
        }
        // 0x151267
        sub_flags(c, c.ecx, 8, 32);
        let target = if c.ecx > 8 { 0x1512b3 } else { m.u32(at(JUMP_TABLE, c.ecx.wrapping_mul(4)))? };
        let (test, site) = match target {
            0x151273 => (WEAPON_TEST, 0x151281),
            0x151283 => (DEVICE_TEST, 0x151291),
            0x151293 => (VEHICLE_TEST, 0x1512a1),
            0x1512a3 => (BIPED_TEST, 0x1512b1),
            0x1512b3 => {
                children(c, m, sp)?;
                continue;
            }
            0x1512de => continue,
            _ => return Err(crate::sim_native::unported(0x15126c, "an object type the walk does not take")),
        };
        let player = m.u32(at(sp, 0x120))?;
        if test == WEAPON_TEST || test == VEHICLE_TEST {
            c.ecx = player;
        } else {
            c.edx = player;
        }
        push(c, m, c.ebp)?;
        push(c, m, player)?;
        return m.call(c, test, site);
    }
}

/// `0x151212`..`0x151265`: whether the object's sphere, grown by the
/// unit's, holds the unit's centre (`jb` not taken).
fn reaches(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> std::result::Result<bool, GuestFault> {
    c.edx = m.u32(at(sp, 0x18))?;
    load(c, 1, m.u32(at(c.esi, 0x30))?);
    ss(c, Ss::Sub, 1, m.u32(c.edx)?);
    load(c, 3, m.u32(at(c.esi, 0x38))?);
    ss(c, Ss::Sub, 3, m.u32(at(c.edx, 8))?);
    load(c, 2, m.u32(at(c.esi, 0x34))?);
    ss(c, Ss::Sub, 2, m.u32(at(c.edx, 4))?);
    load(c, 0, m.u32(at(c.esi, 0x3c))?);
    ss(c, Ss::Add, 0, m.u32(at(sp, 0x14))?);
    movaps(c, 4, 1);
    ss_reg(c, Ss::Mul, 4, 1);
    movaps(c, 1, 3);
    ss_reg(c, Ss::Mul, 1, 3);
    ss_reg(c, Ss::Add, 4, 1);
    movaps(c, 1, 2);
    ss_reg(c, Ss::Mul, 1, 2);
    ss_reg(c, Ss::Add, 4, 1);
    movaps(c, 1, 0);
    ss_reg(c, Ss::Mul, 1, 0);
    let (reach, distance) = (lane0(c, 1), lane0(c, 4));
    Ok(sse::comiss(c, reach, distance) & 1 == 0)
}

/// `0x1512b3`: the object's children onto the list while it has room.
fn children(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> std::result::Result<(), GuestFault> {
    c.eax = m.u32(at(c.esi, 0x10))?;
    sub_flags(c, c.eax, u32::MAX, 32);
    while c.eax != u32::MAX {
        // 0x1512c0
        sub_flags(c, c.ebx, LIST, 32);
        if c.ebx >= LIST {
            break;
        }
        m.set_u32(at(at(sp, c.ebx.wrapping_mul(4)), 0x1c), c.eax)?;
        c.eax &= 0xffff;
        logic32(c, c.eax);
        c.eax = c.eax.wrapping_mul(3);
        c.ecx = m.u32(at(at(c.edi, c.eax.wrapping_mul(4)), 8))?;
        c.eax = m.u32(at(c.ecx, 0xc))?;
        // `inc ebx`: replaced by the `cmp` after it.
        c.ebx = c.ebx.wrapping_add(1);
        sub_flags(c, c.eax, u32::MAX, 32);
    }
    Ok(())
}

/// `0x1512f1`: `esi` and `ebp` back, the frame dropped, `ret 8`.
fn leave(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    c.esi = pop(c, m)?;
    c.ebp = pop(c, m)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    m.ret(c, 8)
}
