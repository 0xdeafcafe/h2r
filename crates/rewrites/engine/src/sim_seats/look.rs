//! Local-user stick response and magnetism, including the local-user view
//! a phantom's action builder supplies. The frame and input globals always
//! come from the calling executor.
use crate::light_constants::X87;
use super::device::arm;
use super::leaf::{self, Leaf};
use crate::sim_native::{Memory, Result, SegmentMemory, plan::segment, sub_flags};
use crate::sim_objects::alu;
use crate::sim_objects::sse::{self, Ss, lane0, load, movaps, ss, ss_reg};
use crate::sim_physics::{arithmetic, logic, pop, push, spatial::{finish_fpu, fp_error, load_fpu}};
use runtime::{CpuAbi, GuestFault, TransferAbi};

pub const CURVE: u32 = 0x0018_5b30;
const ONE: u32 = 0x0045_dbc0;
const ZERO: u32 = 0x0045_dbd8;
const TINY: u32 = 0x0045_dbdc;

type Checked = std::result::Result<Option<TransferAbi>, GuestFault>;
// The segment helpers that only store: their fault is the same as a
// body error, which the segment treats as a decline.
type Unit = std::result::Result<(), GuestFault>;

/// Sample a signed axis against its profile's evenly spaced response curve.
pub fn curve(cpu: &mut CpuAbi, ram: &mut impl SegmentMemory, pc: u32) -> Checked {
    segment::<4, _>(cpu, ram, pc, CURVE..0x0018_5bd4, &[], |c, p, pc| sample_curve(c, p, pc))
}

fn sample_curve(c: &mut CpuAbi, m: &mut impl Memory, _: u32) -> Result {
    let argument = c.esp.wrapping_add(4);
    push(c, m, c.ecx)?;
    sse::zero(c, 2);
    let negative = sse::comiss(c, 0, m.u32(argument)?) & 0x41 == 0;
    push(c, m, c.ebx)?;

    let count = c.eax as i16 as i32;
    let last = count - 1;
    // The argument slot doubles as the rounded sample position, and the
    // saved ECX slot holds the last index when the function returns.
    let mut x = load_fpu(c);
    x.fld(m.u32(argument)?).map_err(fp_error)?;
    x.unary(x87::abi::unary_op::ABS).map_err(fp_error)?;
    m.set_u32(c.esp.wrapping_add(4), last as u32)?;
    X87::ok(x87::abi::arith(&mut x.0, x87::abi::arith_op::MUL, 0,
        x87::abi::src_kind::I32, last as u32 as u64)).map_err(fp_error)?;
    m.set_u32(argument, x.fst_m32().map_err(fp_error)?)?;
    x.fld(m.u32(ZERO)?).map_err(fp_error)?;
    let below_zero = x.fcomip(1).map_err(fp_error)? & 0x41 == 0;
    x.fstp_st(0).map_err(fp_error)?;
    finish_fpu(c, m, x, 0x0018_5b62, ZERO)?;
    if below_zero {
        movaps(c, 1, 2);
    } else {
        load(c, 1, m.u32(argument)?);
        sse::cvtsi2ss(c, 0, count as u32);
        let upper = ss(c, Ss::Sub, 0, m.u32(ONE)?);
        let position = lane0(c, 1);
        if sse::comiss(c, position, upper) & 0x41 == 0 {
            movaps(c, 1, 0);
        }
    }
    let position = lane0(c, 1);
    let trunc = sse::cvttss2si(c, position) as i16 as i32;
    let index = if trunc < 0 { 0 } else { trunc.min(last) } as i16 as i32;
    let next = (index + 1).min(last) as i16 as i32;
    let sample = |i: i32| c.esi.wrapping_add((i as u32).wrapping_mul(4));
    let (lo, hi) = (m.u32(sample(index))?, m.u32(sample(next))?);
    load(c, 0, hi);
    ss(c, Ss::Sub, 0, lo);
    let integer = sse::cvtsi2ss(c, 3, index as u32);
    ss(c, Ss::Sub, 1, integer);
    ss_reg(c, Ss::Mul, 0, 1);
    ss(c, Ss::Add, 0, lo);
    if negative {
        ss_reg(c, Ss::Sub, 2, 0);
        movaps(c, 0, 2);
    }
    c.eax = index as u32;
    c.edx = index.wrapping_add(1) as u32;
    logic(c, u32::from(negative), 0x80);
    c.ebx = pop(c, m)?;
    c.ecx = pop(c, m)?;
    m.ret(c, 4)
}

pub const UPDATE: u32 = 0x0018_5be0;
pub const SITES: &[u32] = &[0x185ff9, 0x186012, 0x186105, 0x1861a6, 0x1863a6,
    0x1863c0, 0x1863f7, 0x186403, 0x1865c4, 0x1866b3, 0x186a94];
const OBJECTS: u32 = crate::points::OBJECTS.base;
const SETTINGS: u32 = 0x004e_034c;
const OBJECT_HEADER: u32 = 0x004e_0300;
const PLAYER_HEADER: u32 = 0x0051_e9b8;
const CONTROLLER_TABLE: u32 = 0x004e_61cc;
const BIPED: u32 = 0x000c_8880;
const PLAYER_LOOK: u32 = 0x001d_08e0;
const SIM_GLOBALS: u32 = 0x004e_6948;
const TICK_GLOBALS: u32 = 0x0051_0c50;

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}

// Return control to the generated segment without publishing its speculative
// stores. In particular, units that do not take the biped input path resume
// at the original selector before any input was consumed.
fn declined(pc: u32) -> GuestFault {
    GuestFault {
        addr: pc, len: 1, access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::InvalidBuffer,
    }
}

pub fn update(cpu: &mut CpuAbi, ram: &mut impl SegmentMemory, pc: u32) -> Checked {
    segment::<160, _>(cpu, ram, pc, UPDATE..0x0018_6aa1, SITES, |c, p, pc| control_step(c, p, pc))
}

fn control_step(c: &mut CpuAbi, m: &mut impl Memory, pc: u32) -> Result {
    leaf::at_site(c, m, pc);
    let sp = c.esp;
    match pc {
        UPDATE => enter(c, m),
        0x185ff9 => {
            let vertical = m.u32(at(sp, 0x10))?;
            movaps(c, 5, 0);
            push(c, m, vertical)?;
            let horizontal = ss_reg(c, Ss::Mul, 5, 7);
            c.eax = c.ebx;
            m.set_u32(at(sp, 0x18), horizontal)?;
            m.call(c, CURVE, 0x186012)
        }
        0x186012 => accelerate(c, m),
        0x186105 => {
            load(c, 4, m.u32(ONE)?);
            movaps(c, 1, 0);
            load(c, 0, m.u32(ONE)?);
            ss_reg(c, Ss::Div, 0, 1);
            movaps(c, 1, 0);
            let yaw = ss(c, Ss::Mul, 1, m.u32(at(sp, 0x18))?);
            let pitch = ss(c, Ss::Mul, 0, m.u32(at(sp, 0x1c))?);
            m.set_u32(at(sp, 0x18), yaw)?;
            m.set_u32(at(sp, 0x1c), pitch)?;
            movaps(c, 5, 1);
            search(c, m)
        }
        0x1861a6 => after_search(c, m),
        0x1863a6 | 0x1863c0 | 0x1863f7 | 0x186403 => tracked_object(c, m, pc),
        0x1865c4 => after_visibility(c, m, c.eax as u8 != 0),
        0x1866b3 => after_weapon(c, m),
        0x186a94 => {
            c.edi = pop(c, m)?;
            c.esi = pop(c, m)?;
            c.ebp = pop(c, m)?;
            c.ebx = pop(c, m)?;
            c.esp = arithmetic(c, c.esp, 0xe0, false);
            m.ret(c, 0x14)
        }
        _ => Err(declined(pc)),
    }
}

/// Unbound users still zero the action and collect any script input.
fn enter(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    if c.eflags & (1 << 10) != 0 {
        return Err(declined(UPDATE));
    }
    let frame = c.esp.wrapping_sub(0xe0);
    let settings = m.u32(at(m.u32(SETTINGS)?, 0xf4))?;
    let seat = m.u32(at(frame, 0xe4))?;
    sse::zero(c, 6);
    c.esp = frame;
    push(c, m, c.ebx)?;
    m.set_u32(at(c.esp, 0x38), settings)?;
    let control = m.u32(0x004e_d284)?;
    push(c, m, c.ebp)?;
    c.ebp = at(at(control, seat.wrapping_mul(0x94)), 0x14);
    push(c, m, c.esi)?;
    c.esi = m.u32(at(c.esp, 0x100))?;
    push(c, m, c.edi)?;
    let sp = c.esp;
    for i in 0..20 {
        m.set_u32(at(sp, 0x6c + i * 4), 0)?;
    }
    c.ebx = u32::MAX;
    arithmetic(c, c.edx, u32::MAX, true);
    for i in 0..19 {
        m.set_u32(at(c.esi, i * 4), 0)?;
    }
    c.edi = at(c.esi, 19 * 4);
    let aim = at(c.esi, 0x28);
    m.set_u32(at(aim, 0x1c), 0)?;
    m.set_u32(at(aim, 0x20), 0)?;
    for offset in [0, 4, 8] {
        m.set_u32(at(aim, offset), u32::MAX)?;
    }
    m.set_u16(at(aim, 0x18), 0)?;
    if c.edx != u32::MAX {
        let port = c.edx as i16 as i32 as u32;
        let handle = m.u32(at(0x004e_61cc, port.wrapping_mul(4)))?;
        m.set_u32(at(sp, 0x48), port)?;
        if handle != 0 {
            return bound(c, m, sp, port);
        }
        logic(c, handle, 1 << 31);
    }
    c.ecx = m.u32(at(sp, 0x104))?;
    c.eax = at(sp, 0x6c);
    leaf::call(c, m, Leaf::Submit, 0x186a94, |c, m| control_step(c, m, 0x186a94))
}

/// `0x185c6c`: a bound seat's device read, the kernel's answer laid
/// down and the transfer it leads to ([`arm`]).
fn bound(c: &mut CpuAbi, m: &mut impl Memory, sp: u32, port: u32) -> Result {
    let p = arm::enter(c, m, sp, port);
    lay(c, m, &p)
}

/// The prediction's stores in retail's order, its registers, then the
/// curve's call or the visibility tail.
fn lay(c: &mut CpuAbi, m: &mut impl Memory, p: &arm::Prediction) -> Result {
    m.set_u32(p.slot, p.input)?;
    let curve = p.site == arm::Site::Curve;
    for (a, v) in p.frame_stores(if curve { -4 } else { 0 }) {
        m.set_u32(a, v)?;
    }
    for (a, v) in p.record_stores() {
        m.set_u32(a, v)?;
    }
    for (i, half) in [0xc, 0xe].into_iter().enumerate() {
        m.set_u16(at(c.ebp, half), p.buttons[i])?;
    }
    let eip = c.eip;
    *c = p.cpu;
    c.eip = eip;
    if curve {
        c.esp = p.sp.wrapping_sub(4);
        m.call(c, CURVE, arm::CURVE_SITE)
    } else {
        visibility(c, m)
    }
}

// Compare an axis magnitude without rounding its x87 value back to a single.
// The last load's address, opcode and popped stack also survive the segment.
fn magnitude(c: &mut CpuAbi, m: &impl Memory, axis: u32, threshold: u32, end: u32)
    -> std::result::Result<u32, GuestFault>
{
    let mut x = load_fpu(c);
    x.fld(m.u32(axis)?).map_err(fp_error)?;
    x.unary(x87::abi::unary_op::ABS).map_err(fp_error)?;
    x.fld(m.u32(threshold)?).map_err(fp_error)?;
    x.fxch(1).map_err(fp_error)?;
    let flags = x.fcomip(1).map_err(fp_error)?;
    x.fstp_st(0).map_err(fp_error)?;
    finish_fpu(c, m, x, end, threshold)?;
    c.eflags = c.eflags & !crate::sim_physics::ARITH | flags;
    c.eflags_defined |= crate::sim_physics::ARITH;
    Ok(flags)
}

fn accelerate(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    let sp = c.esp;
    let pitch = ss(c, Ss::Mul, 0, m.u32(at(sp, 0x34))?);
    let config = c.edi;
    let threshold = at(config, 0x48);
    let yaw_flags = magnitude(c, m, at(sp, 0x14), threshold, 0x186025)?;
    m.set_u32(at(sp, 0x1c), pitch)?;
    for (axis, flags) in [(0, yaw_flags), (1, 0)] {
        let flags = if axis == 1 { magnitude(c, m, at(sp, 0x10), threshold, 0x186090)? } else { flags };
        let timer = at(c.ebp, 0x78 + 4 * axis);
        if flags & 1 != 0 {
            m.set_u32(timer, lane0(c, 6))?;
            continue;
        }
        load(c, 1, m.u32(timer)?);
        ss(c, Ss::Div, 1, m.u32(at(config, 0x4c + 8 * axis))?);
        let fraction = lane0(c, 1);
        let (zero, one) = (lane0(c, 6), lane0(c, 4));
        if sse::comiss(c, zero, fraction) & 0x41 == 0 {
            movaps(c, 1, 6);
        } else if sse::comiss(c, fraction, one) & 0x41 == 0 {
            movaps(c, 1, 4);
        }
        load(c, 2, m.u32(at(config, 0x50 + 8 * axis))?);
        ss_reg(c, Ss::Sub, 2, 4);
        ss_reg(c, Ss::Mul, 2, 1);
        let rate = if axis == 0 {5} else {0};
        let clock = if axis == 0 {1} else {0};
        if axis == 0 {
            load(c, clock, m.u32(timer)?);
            ss(c, Ss::Add, clock, m.u32(at(sp, 0xf8))?);
        }
        ss_reg(c, Ss::Add, 2, 4);
        ss_reg(c, Ss::Mul, 2, rate);
        if axis == 1 {
            load(c, clock, m.u32(timer)?);
            ss(c, Ss::Add, clock, m.u32(at(sp, 0xf8))?);
        }
        if axis == 0 {
            m.set_u32(at(sp, 0x18), lane0(c, 2))?;
            m.set_u32(timer, lane0(c, clock))?;
        } else {
            m.set_u32(timer, lane0(c, clock))?;
            m.set_u32(at(sp, 0x1c), lane0(c, 2))?;
        }
        movaps(c, rate, 2);
    }
    c.eax = m.u32(c.ebp)?;
    arithmetic(c, c.eax, u32::MAX, true);
    if c.eax != u32::MAX {
        c.ebx = u32::from(m.u16(at(c.ebp, 0x2e))?);
        if c.ebx != u32::from(u16::MAX) {
            // CMP BX,-1: 16-bit sign, carry and signed overflow.
            let result = (c.ebx as u16).wrapping_add(1);
            let flags = crate::sim_physics::status(u32::from(result), 0x8000)
                | u32::from(c.ebx != 0xffff) | ((c.ebx ^ 0xffff ^ u32::from(result)) & 0x10)
                | (u32::from(c.ebx == 0x7fff) << 11);
            c.eflags = c.eflags & !crate::sim_physics::ARITH | flags;
            c.eflags_defined |= crate::sim_physics::ARITH;
            return m.call(c, BIPED, 0x186105);
        }
    }
    search(c, m)
}

fn search(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    let sp = c.esp;
    c.eax = m.u32(c.ebp)?;
    arithmetic(c, c.eax, u32::MAX, true);
    if c.eax != u32::MAX {
        c.edx = m.u32(OBJECTS)?;
        let slot = c.eax & 0xffff;
        let entries = m.u32(at(c.edx, 0x44))?;
        c.ecx = m.u32(at(entries, slot * 12 + 8))?;
        c.edx = m.u32(SETTINGS)?;
        let params = m.u32(at(c.edx, 0x134))?;
        load(c, 1, m.u32(at(c.ecx, 0x2e4))?);
        ss(c, Ss::Mul, 1, m.u32(at(params, 0x7c))?);
        ss_reg(c, Ss::Sub, 4, 1);
        movaps(c, 1, 4);
        ss_reg(c, Ss::Mul, 1, 5);
        ss_reg(c, Ss::Mul, 4, 0);
        m.set_u32(at(sp, 0x18), lane0(c, 1))?;
        m.set_u32(at(sp, 0x1c), lane0(c, 4))?;
        logic(c, slot, 1 << 31);
    }
    c.edi = m.u32(at(sp, 0x104))?;
    c.eax = m.u32(at(sp, 0xf4))?;
    c.ecx = at(sp, 0x38);
    push(c, m, c.ecx)?;
    c.ebx = at(c.edi, 0x28);
    m.call(c, 0x001a_4900, 0x1861a6)
}

/// The yaw/pitch pair the target update leaves: the raw rates, or the
/// blend a target pulls them by. Stored scaled by the timestep at
/// `edi`, the input record being built.
fn blend(c: &mut CpuAbi, m: &mut impl Memory, assist: bool) -> Unit {
    let sp = c.esp;
    if !assist {
        // 0x1862f7: keep the rates as they are.
        load(c, 1, m.u32(at(sp, 0x1c))?);
        load(c, 0, m.u32(at(sp, 0x18))?);
    } else {
        // 0x18622b: both strengths, each clamped to [0, 1] on its own path.
        let params = m.u32(at(sp, 0x44))?;
        let slowdown = m.u32(params)?;
        let held = sse::comiss(c, 0, slowdown);
        load(c, 3, m.u32(at(sp, 0xfc))?);
        load(c, 4, m.u32(ONE)?);
        let scale = sse::ss(c, Ss::Div, 3, lane0(c, 6));
        if held & 0x41 == 0 {
            movaps(c, 1, 0);
        } else if sse::comiss(c, slowdown, m.u32(ONE)?) & 0x41 == 0 {
            load(c, 1, m.u32(ONE)?);
        } else {
            load(c, 1, slowdown);
        }
        let follow = m.u32(at(params, 4))?;
        let pulled = sse::comiss(c, 0, follow);
        let strength = lane0(c, 5);
        let held = sse::ss(c, Ss::Mul, 1, strength);
        load(c, 2, m.u32(ONE)?);
        sse::ss(c, Ss::Sub, 2, held);
        if pulled & 0x41 == 0 {
            load(c, 0, 0);
        } else if sse::comiss(c, follow, m.u32(ONE)?) & 0x41 == 0 {
            load(c, 0, m.u32(ONE)?);
        } else {
            load(c, 0, follow);
        }
        // 0x186281: the pull, scaled by the timestep inverse and clamped
        // to its window. The last compare decides the operand kept.
        sse::ss(c, Ss::Mul, 0, strength);
        movaps(c, 4, 0);
        load(c, 0, m.u32(at(sp, 0x38))?);
        load(c, 1, m.u32(at(sp, 0x3c))?);
        sse::ss(c, Ss::Mul, 0, scale);
        sse::ss(c, Ss::Mul, 1, scale);
        for (reg, floor, ceiling) in [(0, 0x0045_df50, 0x0045_dddc), (1, 0x0045_de34, 0x0041_23ec)] {
            let floor = m.u32(floor)?;
            let ceiling = m.u32(ceiling)?;
            if sse::comiss(c, floor, lane0(c, reg)) & 0x41 == 0 {
                load(c, reg, floor);
            } else if sse::comiss(c, lane0(c, reg), ceiling) & 0x41 == 0 {
                // `jbe` keeps the value; above the ceiling takes the ceiling.
                load(c, reg, ceiling);
            }
        }
        // 0x1862d6: the free share of the rate plus the clamped pull.
        movaps(c, 3, 2);
        sse::ss(c, Ss::Mul, 3, m.u32(at(sp, 0x18))?);
        sse::ss(c, Ss::Mul, 2, m.u32(at(sp, 0x1c))?);
        ss_reg(c, Ss::Mul, 0, 4);
        ss_reg(c, Ss::Mul, 1, 4);
        ss_reg(c, Ss::Add, 0, 3);
        ss_reg(c, Ss::Add, 1, 2);
    }
    // 0x186303: scale by the timestep and publish.
    ss_reg(c, Ss::Mul, 0, 6);
    m.set_u32(at(c.edi, 0x10), lane0(c, 0))?;
    load(c, 0, m.u32(at(sp, 0xb4))?);
    ss_reg(c, Ss::Mul, 1, 6);
    m.set_u32(at(c.edi, 0x14), lane0(c, 1))?;
    m.set_u32(at(c.edi, 0x24), lane0(c, 0))
}

/// The assist is on when the target update left a strength above zero, a
/// live timestep and any axis or look component in motion. Otherwise the
/// raw rates pass through unchanged.
fn after_search(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    let sp = c.esp;
    // 0x1861a6: strength above zero?
    load(c, 5, m.u32(at(sp, 0x40))?);
    sse::zero(c, 0);
    let active = sse::comiss(c, lane0(c, 5), 0) & 0x41 == 0;
    c.eax = u32::from(active);
    logic(c, c.eax & 0xff, 0x80);
    load(c, 6, m.u32(at(sp, 0xf8))?);
    m.set_u8(at(c.ebp, 0x68), c.eax as u8)?;
    let assist = active && sse::comiss(c, lane0(c, 6), 0) & 0x41 == 0
        && (magnitude(c, m, at(sp, 0x18), TINY, 0x1861ea)? & 0x41 == 0
            || magnitude(c, m, at(sp, 0x1c), TINY, 0x1861fe)? & 0x41 == 0
            || magnitude(c, m, c.edi, TINY, 0x186210)? & 0x41 == 0
            || magnitude(c, m, at(c.edi, 4), TINY, 0x186223)? & 0x41 == 0);
    blend(c, m, assist)?;
    // 0x186323: the unit the seat controls.
    c.eax = m.u32(c.ebp)?;
    arithmetic(c, c.eax, u32::MAX, true);
    if c.eax == u32::MAX {
        return selections(c, m);
    }
    let slot = c.eax & 0xffff;
    let header = m.u32(OBJECT_HEADER)?;
    let entries = m.u32(at(header, 0x44))?;
    let record = at(entries, slot * 12);
    c.esi = record;
    // 0x186347: only the odd object types take the tracked-object walk.
    let kind = m.u8(at(record, 3))?;
    let mask = alu::shl(c, 1, u32::from(kind), 32);
    logic(c, mask & 1, 0x80);
    if mask & 1 == 0 {
        return selections(c, m);
    }
    // 0x186356: the unit's tracked datum.
    let body = m.u32(at(record, 8))?;
    let tracked = m.u32(at(body, 0x3e4))?;
    m.set_u32(at(sp, 0x14), tracked)?;
    c.esi = tracked;
    arithmetic(c, tracked, u32::MAX, true);
    if tracked == u32::MAX {
        return no_target(c, m);
    }
    // 0x18636c: its owner's player record, if any.
    let owner_slot = tracked & 0xffff;
    let target_body = m.u32(at(entries, owner_slot * 12 + 8))?;
    let owner = m.u32(at(target_body, 0xb4))?;
    arithmetic(c, owner, u32::MAX, true);
    if owner == u32::MAX {
        return record_target(c, m);
    }
    // 0x186389: the player record, then the player index call.
    let players = m.u32(at(m.u32(PLAYER_HEADER)?, 0x44))?;
    let stride = (owner & 0xffff) * 5;
    c.ebx = alu::shl(c, stride, 5, 32);
    c.ebx = arithmetic(c, c.ebx, players, false);
    c.ecx = c.ebx;
    c.eax = owner & 0xffff;
    c.edx = players;
    leaf::call(c, m, Leaf::PlayerIndex, 0x1863a6, |c, m| control_step(c, m, 0x1863a6))
}

/// No tracked unit: nothing to record, straight to the selection tail.
fn no_target(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    m.set_u32(at(c.ebp, 0x40), c.esi)?;
    selections(c, m)
}

/// The tracked target's direction. Each transfer re-reads its inputs from
/// the frame, as a child can change them. The direction pair is kept both
/// in the seat's record and the frame for the next tick's velocity.
fn tracked_object(c: &mut CpuAbi, m: &mut impl Memory, pc: u32) -> Result {
    let sp = c.esp;
    match pc {
        0x1863a6 => {
            // 0x1863a6: a player index the record's count accepts.
            c.eax = (c.eax & 0xffff) as u16 as i16 as i32 as u32;
            arithmetic(c, c.eax, u32::MAX, true);
            if c.eax == u32::MAX {
                return record_target(c, m);
            }
            c.esi = at(sp, 0xbc);
            c.ecx = c.ebx;
            m.call(c, PLAYER_LOOK, 0x1863c0)
        }
        0x1863c0 => {
            // 0x1863c0: publish the direction and compare with the last.
            for (offset, value) in [(0x38, 0xc0), (0x3c, 0xc4), (0x40, 0xc8)] {
                m.set_u32(at(sp, offset), m.u32(at(sp, value))?)?;
            }
            let tracked = m.u32(at(sp, 0x14))?;
            let last = m.u32(at(c.ebp, 0x40))?;
            arithmetic(c, tracked, last, true);
            if tracked != last {
                return record_target(c, m);
            }
            c.ecx = at(sp, 0x38);
            c.eax = at(sp, 0x30);
            leaf::call(c, m, Leaf::Direction, 0x1863f7, |c, m| control_step(c, m, 0x1863f7))
        }
        0x1863f7 => {
            // 0x1863f7: the seat's own recorded direction.
            c.ecx = at(c.ebp, 0x34);
            c.eax = at(sp, 0x18);
            leaf::call(c, m, Leaf::Direction, 0x186403, |c, m| control_step(c, m, 0x186403))
        }
        _ => {
            // 0x186403: turn the look by the difference of the two.
            for (look, angle, fresh) in [(0x10, 0x30, 0x18), (0x14, 0x34, 0x1c)] {
                load(c, 0, m.u32(at(sp, angle))?);
                ss(c, Ss::Sub, 0, m.u32(at(sp, fresh))?);
                ss(c, Ss::Add, 0, m.u32(at(c.edi, look))?);
                m.set_u32(at(c.edi, look), lane0(c, 0))?;
            }
            record_target(c, m)
        }
    }
}

/// The seat record's target datum and direction, then the selection tail.
fn record_target(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    let sp = c.esp;
    c.esi = m.u32(at(sp, 0x14))?;
    for (offset, value) in [(0x34, 0x38), (0x38, 0x3c), (0x3c, 0x40)] {
        m.set_u32(at(c.ebp, offset), m.u32(at(sp, value))?)?;
    }
    m.set_u32(at(c.ebp, 0x40), c.esi)?;
    selections(c, m)
}

/// The selection mask: bits whose button this frame did not press are
/// dropped from the seat's wanted set, and the state of the rest is
/// recopied into the frame. An odd-typed unit asks the visibility walk.
fn selections(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    let sp = c.esp;
    // 0x186465: the wanted mask, both halves.
    let wanted = u32::from(m.u16(at(c.ebp, 0xe))? & m.u16(at(c.ebp, 0xc))?);
    logic(c, wanted, 0x8000);
    if wanted != 0 {
        // 0x186476: the saved mask and the column counter. Each of the
        // four checks per column tests its own bit against its own byte.
        m.set_u32(at(sp, 0x14), wanted)?;
        m.set_u32(at(sp, 0x10), 0)?;
        for i in 0..16u32 {
            let bit = 1u32 << i;
            logic(c, wanted & bit, 1 << 31);
            let pressed = m.u8(at(sp, 0x6c + i))?;
            logic(c, u32::from(pressed), 0x80);
            if wanted & bit != 0 && pressed == 0 {
                let clear = !(bit as u16);
                for half in [0xc, 0xe] {
                    let kept = m.u16(at(c.ebp, half))? & clear;
                    m.set_u16(at(c.ebp, half), kept)?;
                    logic(c, u32::from(kept), 0x8000);
                }
            }
        }
        // 0x186548: the counters after the fourth column, and the loop
        // bound's last compare.
        m.set_u32(at(sp, 0x10), 0x10)?;
        c.ebx = 0x13;
        c.esi = 0x12;
        c.edx = 0x11;
        c.eax = 0x8000;
        arithmetic(c, 0x11, 0x11, true);
    }
    // 0x186565: the state of every button not wanted is recopied.
    let mask = u32::from(m.u16(at(c.ebp, 0xc))?);
    for i in 0..16u32 {
        let bit = 1u32 << i;
        logic(c, mask & bit, 1 << 31);
        if mask & bit == 0 {
            let state = m.u8(at(sp, 0x6c + i))?;
            m.set_u8(at(sp, 0x20 + i), state)?;
            let word = m.u16(at(sp, 0x7c + i * 2))?;
            m.set_u16(at(sp, 0x4c + i * 2), word)?;
        }
        arithmetic(c, i + 1, 0x10, true);
    }
    c.ecx = 0x10;
    c.edx = 0x8000;
    visibility(c, m)
}

/// `0x186593`: only an odd-typed controlled unit asks for visibility.
fn visibility(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    c.eax = m.u32(c.ebp)?;
    arithmetic(c, c.eax, u32::MAX, true);
    if c.eax != u32::MAX {
        c.edx = m.u32(at(m.u32(OBJECT_HEADER)?, 0x44))?;
        let slot = c.eax & 0xffff;
        let kind = m.u8(at(c.edx, slot * 12 + 3))?;
        c.ecx = (slot * 3) & !0xff | u32::from(kind);
        c.ebx = alu::shl(c, 1, u32::from(kind), 32);
        logic(c, c.ebx & 1, 0x80);
        if c.ebx & 1 != 0 {
            return leaf::call(c, m, Leaf::Visible, 0x1865c4, |c, m| control_step(c, m, 0x1865c4));
        }
    }
    after_visibility(c, m, false)
}

/// After the visibility walk: the assist-active byte of the input record,
/// then the dual-wield gate and its weapon call.
fn after_visibility(c: &mut CpuAbi, m: &mut impl Memory, visible: bool) -> Result {
    let sp = c.esp;
    // 0x1865c4: the answer decides only while the look is held.
    let held = m.u8(at(sp, 0x2a))?;
    sub_flags(c, u32::from(held), 1, 8);
    if held == 1 {
        // 0x1865d7: the walk's answer, or the look sitting inside the
        // dead zone.
        logic(c, u32::from(visible), 0x80);
        let active = visible || {
            // xmm1 keeps the horizontal rate and xmm2 the squared length.
            load(c, 0, m.u32(at(c.edi, 4))?);
            load(c, 1, m.u32(c.edi)?);
            movaps(c, 2, 0);
            ss_reg(c, Ss::Mul, 2, 0);
            movaps(c, 0, 1);
            ss_reg(c, Ss::Mul, 0, 1);
            ss_reg(c, Ss::Add, 2, 0);
            load(c, 0, m.u32(0x0045_dc20)?);
            sse::comiss(c, lane0(c, 0), lane0(c, 2)) & 0x41 == 0
        };
        if active {
            m.set_u8(at(c.ebp, 0x88), 1)?;
        }
    } else if held == 0 {
        // 0x18660c: the byte holds only while the look is held.
        logic(c, 0, 0x80);
        m.set_u8(at(c.ebp, 0x88), 0)?;
    } else {
        logic(c, u32::from(held), 0x80);
    }
    // 0x186616: bit 0 of the input's first flag word.
    let assist = m.u8(at(c.ebp, 0x88))? != 0;
    logic(c, u32::from(assist), 0x80);
    or_bit(c, m, 0x18, 1, assist)?;
    // 0x18662b: the zoom toggle, then the dual-wield check.
    let zooming = m.u8(at(sp, 0x2b))?;
    sub_flags(c, u32::from(zooming), 1, 8);
    let dual = zooming == 1;
    c.ebx = (c.ebx & !0xff) | u32::from(dual);
    logic(c, u32::from(dual), 0x80);
    if !dual {
        return zoom_bits(c, m, true);
    }
    // 0x18663e: the seat must hold no weapon and carry a device whose
    // dead-man bytes are quiet; the unit's second weapon must aim.
    let seat_weapon = u32::from(m.u16(at(c.ebp, 0x2e))?);
    alu::cmp16(c, seat_weapon, u32::from(u16::MAX));
    if seat_weapon != u32::from(u16::MAX) {
        // `jne 0x1866e4`: a held weapon skips the dual-wield check.
        return zoom_bits(c, m, true);
    }
    let device = m.u32(m.u32(at(sp, 0x100))?)?;
    logic(c, device, 1 << 31);
    if device == 0 {
        return zoom_bits(c, m, true);
    }
    // 0x18665a: the unit's second weapon, when its slots name one.
    let mut weapon = u32::MAX;
    c.eax = m.u32(c.ebp)?;
    arithmetic(c, c.eax, u32::MAX, true);
    if c.eax != u32::MAX {
        let entries = m.u32(at(m.u32(OBJECT_HEADER)?, 0x44))?;
        let body = m.u32(at(entries, (c.eax & 0xffff) * 12 + 8))?;
        let slot = u32::from(m.u8(at(body, 0x212))? as i8 as i16 as u16);
        logic(c, c.eax & 0xffff, 1 << 31);
        logic(c, u32::MAX, 1 << 31);
        alu::cmp16(c, slot, u32::from(u16::MAX));
        if slot != u32::from(u16::MAX) {
            let index = slot as i16 as i32 as u32;
            weapon = m.u32(at(body, 0x218 + index.wrapping_mul(4)))?;
        }
    } else {
        logic(c, u32::MAX, 1 << 31);
    }
    // 0x186697: the device's dead-man bytes clear the toggle.
    let first = m.u8(at(device, 6))?;
    sub_flags(c, u32::from(first), 0x7f, 8);
    let mut quiet = first <= 0x7f;
    if quiet {
        let second = m.u8(at(device, 7))?;
        sub_flags(c, u32::from(second), 0x7f, 8);
        quiet = second <= 0x7f;
    }
    if !quiet {
        c.ebx &= !0xff;
        logic(c, 0, 0x80);
    }
    arithmetic(c, weapon, u32::MAX, true);
    if weapon != u32::MAX {
        c.eax = weapon;
        c.edx = weapon;
        c.esi = device;
        return leaf::call(c, m, Leaf::WeaponReady, 0x1866b3, |c, m| control_step(c, m, 0x1866b3));
    }
    zoom_bits(c, m, true)
}

/// The distance gate after the weapon call, then the zoom bits. Far
/// enough away, the zoom hold bit clears without the toggle's say.
fn after_weapon(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    let sp = c.esp;
    let ready = c.eax as u8;
    logic(c, u32::from(ready), 0x80);
    if ready != 0 {
        return zoom_bits(c, m, true);
    }
    load(c, 0, m.u32(at(sp, 0xb0))?);
    let x = sse::ss(c, Ss::Mul, 0, m.u32(at(sp, 0xb0))?);
    load(c, 0, m.u32(at(sp, 0xac))?);
    let _y = sse::ss(c, Ss::Mul, 0, m.u32(at(sp, 0xac))?);
    let square = sse::ss(c, Ss::Add, 0, x);
    let near = sse::comiss(c, square, m.u32(0x0045_e578)?) & 0x41 != 0;
    zoom_bits(c, m, near)
}

/// 0x1866e4: the zoom hold bit, then the toggle bit, of the second word.
/// `test_dual`: the hold follows the dual-wield toggle; without it the
/// bit clears whatever the toggle says.
fn zoom_bits(c: &mut CpuAbi, m: &mut impl Memory, test_dual: bool) -> Result {
    let hold = test_dual && c.ebx & 1 != 0;
    if test_dual {
        logic(c, u32::from(c.ebx & 1 != 0), 0x80);
    }
    or_bit(c, m, 0x1c, 4, hold)?;
    let zooming = m.u8(at(c.esp, 0x2b))? != 0;
    or_bit(c, m, 0x1c, 8, zooming)?;
    publish(c, m)
}

/// Publish the completed input record and hand the action to the
/// submitter: the swapped triggers, the weapon counts, the button bits,
/// the profile's player-control block and the two zoom counters.
fn publish(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    let sp = c.esp;
    // 0x18670f: the swapped trigger pair.
    let swapped = m.u8(at(sp, 0xba))? != 0;
    logic(c, u32::from(m.u8(at(sp, 0xba))?), 0x80);
    let (first, second) = if swapped {
        (m.u32(at(sp, 0x9c))?, m.u32(at(sp, 0xa0))?)
    } else {
        (m.u32(at(sp, 0xa0))?, m.u32(at(sp, 0x9c))?)
    };
    m.set_u32(at(c.edi, 8), first)?;
    load(c, 0, second);
    // 0x18674a: the weapon in the other hand and its button state.
    c.ebx = m.u32(at(sp, 0xb8))?;
    let index = c.ebx & 0xff;
    let button = at(sp, index + 0x20);
    c.edx = u32::from(m.u8(button)?);
    m.set_u32(at(c.edi, 0xc), lane0(c, 0))?;
    c.eax = m.u32(c.ebp)?;
    arithmetic(c, c.eax, u32::MAX, true);
    if c.eax != u32::MAX {
        let entries = m.u32(at(m.u32(OBJECT_HEADER)?, 0x44))?;
        let body = m.u32(at(entries, (c.eax & 0xffff) * 12 + 8))?;
        logic(c, c.eax & 0xffff, 1 << 31);
        if m.u8(at(body, 0x212))? != 0xff && m.u8(at(body, 0x213))? != 0xff {
            // 0x186791: the button state moves with the weapon's count.
            logic(c, c.edx, 1 << 31);
            m.set_u8(button, 0)?;
            let count = (c.edx as i32) > 0;
            c.ecx = (c.ecx & !0xff) | u32::from(count);
            m.set_u8(at(c.ebp, 0x90), count as u8)?;
        } else {
            held_button(c, m, button)?;
        }
    } else {
        held_button(c, m, button)?;
    }
    // 0x1867bc: bit 16, the primary trigger held.
    or_bit(c, m, 0x18, 0x1_0000, m.u8(at(sp, 0x27))? != 0)?;
    // 0x1867d3: bits 21 and 17, a weapon of the count.
    or_bit(c, m, 0x18, 0x22_0000, c.edx != 0)?;
    // 0x1867f9: bit 29, only outside mode 2, from the second button pair.
    let mode = m.u32(at(m.u32(SIM_GLOBALS)?, 8))?;
    arithmetic(c, mode, 2, true);
    if mode != 2 {
        let held = m.u8(at(sp, ((c.ebx >> 8) & 0xff) + 0x20))?;
        or_bit(c, m, 0x18, 0x2000_0000, held != 0)?;
    }
    // 0x186827: bits 11 and 14.
    or_bit(c, m, 0x18, 0x4800, c.edx != 0)?;
    // 0x18684d: bit 26.
    or_bit(c, m, 0x18, 0x400_0000, m.u8(at(sp, 0x26))? != 0)?;
    // 0x186867: bits 7 and 8, the two halves of the look word.
    let look = m.u32(at(sp, 0x2c))?;
    or_bit(c, m, 0x18, 0x80, look & 0xff != 0)?;
    or_bit(c, m, 0x18, 0x100, look >> 8 & 0xff != 0)?;
    // 0x186898: bits 2, 20 and 25.
    or_bit(c, m, 0x18, 0x210_0004, m.u8(at(sp, 0x25))? != 0)?;
    // 0x1868cb: bit 1, the auto-look; the state byte keeps the answer.
    let auto = {
        let asked = m.u8(at(sp, 0x20))?;
        logic(c, u32::from(asked), 0x80);
        let mut on = false;
        if asked != 0 {
            let tick = m.u32(TICK_GLOBALS)?;
            logic(c, tick, 1 << 31);
            on = tick != 0 && m.u8(at(tick, 5))? != 0;
            if !on {
                let held = m.u8(at(c.ebp, 0x93))?;
                logic(c, u32::from(held), 0x80);
                on = held != 0;
            }
        }
        m.set_u8(at(c.ebp, 0x93), u8::from(on))?;
        asked != 0 && !on
    };
    or_bit(c, m, 0x18, 2, auto)?;
    // 0x18691b: bits 4 and 5, and the pair the next blocks keep.
    c.ecx = 0xffff_ffef;
    c.esi = 0x20;
    or_bit(c, m, 0x18, 0x30, m.u8(at(sp, 0x24))? != 0)?;
    // 0x186945: bit 6, with the register pair the block keeps.
    c.ebx = 0x40;
    or_bit(c, m, 0x18, 0x40, m.u8(at(sp, 0x27))? != 0)?;
    // 0x18695f: bit 2 of the second word, the zoom held exactly.
    let zoomed = m.u8(at(sp, 0x21))?;
    sub_flags(c, u32::from(zoomed), 1, 8);
    or_bit(c, m, 0x1c, 2, zoomed == 1)?;
    // 0x186973: bit 4 of the second word.
    or_bit(c, m, 0x1c, 0x10, m.u8(at(sp, 0x2e))? != 0)?;
    // 0x186984: bit 5 and the second byte's bit 0.
    let zoom_byte = m.u8(at(sp, 0x2f))? != 0;
    or_bit(c, m, 0x1c, 0x20, zoom_byte)?;
    let grenade = m.u8(at(sp, 0x2e))?;
    sub_flags(c, u32::from(grenade), 1, 8);
    let thrown = grenade == 1;
    let byte = m.u8(at(c.edi, 0x21))?;
    let byte = if thrown { byte | 1 } else { byte & !1 };
    logic(c, u32::from(byte), 0x80);
    m.set_u8(at(c.edi, 0x21), byte)?;
    // 0x1869a6: the controller's profile mode, bit 6 of the second word.
    let port = m.u32(at(sp, 0x48))?;
    let handle = m.u32(at(CONTROLLER_TABLE, port.wrapping_mul(4)))?;
    logic(c, handle, 1 << 31);
    if handle != 0 {
        let entry = if m.u8(0x004e_61b9)? != 0 {
            0x004e_630c
        } else {
            port.wrapping_mul(0x40).wrapping_add(0x004e_61dc)
        };
        logic(c, entry, 1 << 31);
        let mode = if entry != 0 { m.u8(at(entry, 0x19))? } else { 0 };
        logic(c, u32::from(mode), 0x80);
        if mode != 0 {
            or_bit(c, m, 0x1c, 0x40, true)?;
        }
    }
    // 0x1869de: the profile's player-control block and the zoom counters.
    let params = m.u32(at(m.u32(SETTINGS)?, 0xf4))?;
    load(c, 0, m.u32(at(params, 0x7c))?);
    let scaled = sse::ss(c, Ss::Mul, 0, m.u32(0x0045_dccc)?);
    m.set_u32(at(sp, 0x18), scaled)?;
    let ticks = sse::cvttss2si(c, scaled);
    // 0x186a07: the four button bits of the third byte.
    or_byte(c, m, 1, m.u8(at(sp, 0x22))? != 0)?;
    c.ecx = u32::from(m.u16(at(sp, 0x50))?);
    arithmetic(c, c.ecx, ticks, true);
    or_byte(c, m, 2, (c.ecx as i32) >= (ticks as i32))?;
    or_byte(c, m, 0x10, m.u8(at(sp, 0x23))? != 0)?;
    c.edx = u32::from(m.u16(at(sp, 0x52))?);
    arithmetic(c, c.edx, ticks, true);
    or_word16(c, m, 0x20, (c.edx as i32) >= (ticks as i32))?;
    // 0x186a51: the two zoom latches.
    let held = m.u8(at(sp, 0x23))?;
    logic(c, u32::from(held), 0x80);
    if held != 0 {
        m.set_u8(at(c.ebp, 0x8e), 1)?;
    } else {
        let first = m.u8(at(c.ebp, 0x8e))?;
        logic(c, u32::from(first), 0x80);
        if first != 0 {
            let second = m.u8(at(c.ebp, 0x8f))?;
            logic(c, u32::from(second), 0x80);
            if second == 0 {
                or_bit(c, m, 0x1c, 1, true)?;
            }
        }
        // 0x186a76: only the released path clears both latches.
        m.set_u8(at(c.ebp, 0x8e), 0)?;
        m.set_u8(at(c.ebp, 0x8f), 0)?;
    }
    // 0x186a84: the action and the input record.
    c.ecx = m.u32(at(sp, 0x104))?;
    c.eax = at(sp, 0x6c);
    leaf::call(c, m, Leaf::Submit, 0x186a94, |c, m| control_step(c, m, 0x186a94))
}

/// The button state of a weapon the unit does not hold: it returns to the
/// seat only while the previous state said so, and the count is positive.
fn held_button(c: &mut CpuAbi, m: &mut impl Memory, button: u32) -> Unit {
    let held = m.u8(at(c.ebp, 0x90))?;
    logic(c, u32::from(held), 0x80);
    if held != 0 {
        logic(c, c.edx, 1 << 31);
        if (c.edx as i32) > 0 {
            c.edx = 0;
            logic(c, 0, 1 << 31);
            m.set_u8(button, 0)?;
        } else {
            m.set_u8(at(c.ebp, 0x90), 0)?;
        }
    }
    Ok(())
}

/// One bit of the input's flag words: the conditional or/and pair.
fn or_bit(c: &mut CpuAbi, m: &mut impl Memory, offset: u32, mask: u32, on: bool) -> Unit {
    let word = m.u32(at(c.edi, offset))?;
    let word = if on { word | mask } else { word & !mask };
    logic(c, word, 1 << 31);
    m.set_u32(at(c.edi, offset), word)
}

/// One bit of the input's third byte: the conditional or/and pair.
fn or_byte(c: &mut CpuAbi, m: &mut impl Memory, mask: u8, on: bool) -> Unit {
    let byte = m.u8(at(c.edi, 0x20))?;
    let byte = if on { byte | mask } else { byte & !mask };
    logic(c, u32::from(byte), 0x80);
    m.set_u8(at(c.edi, 0x20), byte)
}

/// The 16-bit or of the third word the zoom counters use; its clear is
/// the matching 8-bit and.
fn or_word16(c: &mut CpuAbi, m: &mut impl Memory, mask: u16, on: bool) -> Unit {
    if on {
        let word = m.u16(at(c.edi, 0x20))? | mask;
        logic(c, u32::from(word), 0x8000);
        m.set_u16(at(c.edi, 0x20), word)
    } else {
        or_byte(c, m, mask as u8, false)
    }
}
