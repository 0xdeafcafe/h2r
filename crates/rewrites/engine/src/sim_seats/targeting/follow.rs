//! Search for the object under the user's reticle and its relative angular
//! velocity. Each transfer publishes the same frame to the next guest child.
use super::*;
use crate::light_constants::X87;
use crate::sim_physics::spatial::{finish_fpu, load_fpu};
use x87::abi::{arith_op, unary_op};

const FRAME: u32 = 0xb8;
const LOCAL_USERS: u32 = 0x4e8c20;
const PLAYERS: u32 = 0x4e8c24;
const CONTROLS: u32 = 0x4ed284;
const OBSERVERS: u32 = 0x4e9c8c;
const NONE: u32 = u32::MAX;
pub const UPDATE_SITES: &[u32] = &[
    0x1a490f, 0x1a497d, 0x1a49a6, 0x1a49d4, 0x1a4a45,
    0x1a4a6c, 0x1a4ac5, 0x1a4ad4,
];

fn player_index(g: &impl Memory, user: u32) -> Result<u32, GuestFault> {
    if user == NONE {
        Ok(NONE)
    } else {
        let users = g.u32(LOCAL_USERS)?;
        g.u32(at(at(users, 12), user.wrapping_mul(4)))
    }
}

fn leave(c: &mut CpuAbi, g: &impl Memory, saved_ebp: bool) -> Transfer {
    if saved_ebp { c.ebp = pop(c, g)?; }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    g.ret(c, 4)
}

fn begin_search(c: &mut CpuAbi, g: &mut impl Memory) -> Transfer {
    c.edi = g.u32(at(c.esp, 12))?;
    let team = g.u8(at(c.edi, 0xc0))? as i8 as i16 as u16;
    c.eax = (c.eax & 0xffff_0000) | u32::from(team);
    c.edx = at(c.esp, 0x40);
    push(c, g, c.edx)?;
    c.ecx = at(c.esp, 0x20); // facing vector
    c.edx = at(c.esp, 0xa4); // aim parameters
    for arg in [c.eax, c.ebp, c.ecx, c.edx] { push(c, g, arg)?; }
    c.ecx = c.esi;
    g.call(c, 0x1a5370, 0x1a4a6c)
}

fn facing(c: &mut CpuAbi, g: &mut impl Memory) -> Transfer {
    let sp = c.esp;
    c.ebp = c.eax;
    let player = player_index(g, c.edi)?;
    let controls = at(g.u32(CONTROLS)?, c.edi.wrapping_mul(0x94));
    let angles = at(controls, 0x24);
    c.ecx = angles;
    c.edx = g.u32(PLAYERS)?;
    c.edi = g.u32(at(c.edx, 0x44))?;
    let record = at(c.edi, (player & 0xffff) * 0x21c);
    let mut x = load_fpu(c);
    x.fld(g.u32(at(angles, 4))?).map_err(fp_error)?;
    x.fcos().map_err(fp_error)?;
    for (offset, op) in [(0x1c, unary_op::COS), (0x20, unary_op::SIN)] {
        x.fld(g.u32(angles)?).map_err(fp_error)?;
        x.unary(op).map_err(fp_error)?;
        x.arith_st(arith_op::MUL, 1).map_err(fp_error)?;
        g.set_u32(at(sp, offset), x.fstp_m32().map_err(fp_error)?)?;
    }
    x.fstp_st(0).map_err(fp_error)?;
    x.fld(g.u32(at(angles, 4))?).map_err(fp_error)?;
    x.fsin().map_err(fp_error)?;
    g.set_u32(at(sp, 0x24), x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c, g, x, 0x1a4a2f, at(sp, 0x24))?;
    c.eax = g.u32(at(record, 0x2c))?;
    arithmetic(c, c.eax, NONE, true);
    if c.eax == NONE { return begin_search(c, g); }
    c.ecx = at(sp, 0x1c);
    push(c, g, c.ecx)?;
    g.call(c, 0xcb810, 0x1a4a45)
}

fn pitch_rate(x: &mut X87, g: &impl Memory, sp: u32) -> Result<u32, GuestFault> {
    // ST(0) starts with the length of the planar target direction. Project
    // the relative velocity onto the vertical tangent, then divide by r².
    x.fld(g.u32(at(sp, 0x18))?).map_err(fp_error)?;
    x.arith_st(arith_op::MUL, 1).map_err(fp_error)?;
    x.fld(g.u32(at(sp, 0x70))?).map_err(fp_error)?;
    x.fmul_m(g.u32(at(sp, 0x14))?).map_err(fp_error)?;
    x.fld(g.u32(at(sp, 0x6c))?).map_err(fp_error)?;
    x.fmul_m(g.u32(at(sp, 0x10))?).map_err(fp_error)?;
    x.arith_reg(arith_op::ADD, 1, 0, true).map_err(fp_error)?;
    x.arith_st(arith_op::DIV, 2).map_err(fp_error)?;
    x.fmul_m(g.u32(at(sp, 0x74))?).map_err(fp_error)?;
    x.arith_reg(arith_op::SUB, 1, 0, true).map_err(fp_error)?;
    x.fld(g.u32(at(sp, 0x74))?).map_err(fp_error)?;
    x.fmul_m(g.u32(at(sp, 0x74))?).map_err(fp_error)?;
    x.fadd_m(g.u32(at(sp, 12))?).map_err(fp_error)?;
    x.arith_reg(arith_op::DIV, 1, 0, true).map_err(fp_error)?;
    x.fstp_m32().map_err(fp_error)
}

fn publish(c: &mut CpuAbi, g: &mut impl Memory) -> Transfer {
    let sp = c.esp;
    let mut s = Sse { mxcsr: c.mxcsr };
    let mut velocity = |a, b| s.sub(g.u32(at(sp, a))?, g.u32(at(sp, b))?).map_err(fp_error);
    let dz = velocity(0x30, 0x3c)?;
    let y = g.u32(at(sp, 0x70))?;
    let dx = velocity(0x28, 0x34)?;
    let dy = velocity(0x2c, 0x38)?;
    g.set_u32(at(sp, 0x18), dz)?;
    let x = g.u32(at(sp, 0x6c))?;
    let xx = s.mul(x, x).map_err(fp_error)?;
    let yy = s.mul(y, y).map_err(fp_error)?;
    let r2 = s.add(xx, yy).map_err(fp_error)?;
    let usable = s.above(r2, g.u32(0x45dbdc)?).map_err(fp_error)?;
    for (offset, word) in [(0x10, dx), (0x14, dy), (12, r2)] {
        g.set_u32(at(sp, offset), word)?;
    }
    let mut y_component = y;
    if usable {
        let mut fpu = load_fpu(c);
        fpu.fld(g.u32(at(sp, 12))?).map_err(fp_error)?;
        fpu.unary(unary_op::SQRT).map_err(fp_error)?;
        let x_component = s.mul(x, dy).map_err(fp_error)?;
        y_component = s.mul(y, dx).map_err(fp_error)?;
        let cross = s.sub(x_component, y_component).map_err(fp_error)?;
        g.set_u32(c.esi, s.div(cross, r2).map_err(fp_error)?)?;
        g.set_u32(at(c.esi, 4), pitch_rate(&mut fpu, g, sp)?)?;
        fpu.fstp_st(0).map_err(fp_error)?;
        finish_fpu(c, g, fpu, 0x1a4b85, at(c.esi, 4))?;
    } else {
        g.set_u32(c.esi, 0)?;
        g.set_u32(at(c.esi, 4), 0)?;
    }

    let mut strength = g.u32(at(c.ebx, 0x1c))?;
    let mut acquired = s.above(strength, 0).map_err(fp_error)?;
    if !acquired {
        strength = g.u32(at(c.ebx, 0x20))?;
        acquired = s.above(strength, 0).map_err(fp_error)? || g.u8(at(sp, 0x68))? != 0;
    }
    for (i, word) in [0, strength, y_component, dx, dy, yy].into_iter().enumerate() {
        scalar(c, i, word);
    }
    c.mxcsr = s.mxcsr;
    if acquired {
        c.eax = g.u32(at(sp, 0x44))?;
        g.set_u32(c.ebx, c.edi)?;
        g.set_u32(at(c.ebx, 4), c.eax)?;
    }
    c.eax = g.u32(c.ebx)?;
    if c.eax != NONE {
        c.edx = g.u32(guest_layout::points::OBJECTS.base)?;
        c.ecx = (c.eax & 0xffff) * 3;
        c.eax = g.u32(at(c.edx, 0x44))?;
        c.esi = at(c.eax, c.ecx * 4);
        c.ecx = byte(c.ecx, g.u8(at(c.esi, 3))?);
        c.edx = 1u32.wrapping_shl(c.ecx);
        if c.edx & 3 != 0 {
            c.eax = g.u32(at(c.esi, 8))?;
            c.ecx = g.u32(at(c.eax, 0x13c))?;
            g.set_u32(at(c.ebx, 8), c.ecx)?;
        }
    }
    leave(c, g, true)
}

fn follow(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Transfer {
    let sp = c.esp;
    match pc {
        0x1a4900 => {
            c.esp = arithmetic(c, sp, FRAME, true);
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = c.eax;
            g.call(c, 0x155760, 0x1a490f)
        }
        0x1a490f => {
            c.edx = g.u32(at(sp, 0xc4))?;
            scalar(c, 0, 0);
            c.ecx = 0;
            for offset in [0, 4, 8] { g.set_u32(at(c.edx, offset), 0)?; }
            c.esi = NONE;
            for (offset, value) in [(0x1c, 0), (0x20, 0), (0, NONE), (4, NONE), (8, NONE)] {
                g.set_u32(at(c.ebx, offset), value)?;
            }
            g.set_u16(at(c.ebx, 0x18), 0)?;
            if c.eax as u16 > 1 { return leave(c, g, false); }
            let player = player_index(g, c.edi)?;
            c.ecx = g.u32(PLAYERS)?;
            c.edx = g.u32(at(c.ecx, 0x44))?;
            c.eax = arithmetic(c, (player & 0xffff) * 0x21c, c.edx, false);
            g.set_u32(at(sp, 8), c.eax)?;
            c.eax = g.u32(at(c.eax, 0x2c))?;
            push(c, g, c.ebp)?;
            g.call(c, 0xcb6d0, 0x1a497d)
        }
        0x1a497d => {
            arithmetic(c, c.edi, c.esi, true);
            c.ebp = c.eax;
            c.eax = c.esi;
            if c.edi != c.esi {
                let wide = i64::from(c.edi as i32) * 0x94;
                c.edx = wide as u32;
                c.eflags = (c.eflags & !ARITH) | if wide == i64::from(c.edx as i32) { 0 } else { 0x801 };
                c.eflags_defined = (c.eflags_defined & !ARITH) | 0x801;
                let controls = at(g.u32(CONTROLS)?, c.edx);
                c.eax = g.u16(at(controls, 0x42))? as i16 as i32 as u32;
            }
            push(c, g, c.eax)?;
            c.esi = at(c.esp, 0xa4);
            c.ecx = c.ebp;
            g.call(c, 0x1a50a0, 0x1a49a6)
        }
        0x1a49a6 => {
            if c.eax as u8 == 0 { return leave(c, g, true); }
            c.esi = 0;
            arithmetic(c, c.edi, NONE, true);
            if c.edi != NONE {
                let enabled = g.u16(0x4686c4)?;
                crate::sim_native::sub_flags(c, u32::from(enabled), 0xffff, 16);
                if enabled != u16::MAX {
                    c.esi = arithmetic(c, c.edi.wrapping_mul(0x358), OBSERVERS, false);
                }
            }
            c.eax = c.ebp;
            g.call(c, 0xbaf40, 0x1a49d4)
        }
        0x1a49d4 => facing(c, g),
        0x1a4a45 => begin_search(c, g),
        0x1a4a6c => {
            if c.eax as u8 == 0 { return leave(c, g, true); }
            g.set_u32(at(c.ebx, 0x1c), g.u32(at(sp, 0x8c))?)?;
            let fixed = g.u8(at(sp, 0xb4))? & 2;
            logic(c, u32::from(fixed), 0x80);
            g.set_u32(at(c.ebx, 0x20), g.u32(at(sp, 0x90))?)?;
            let mut weight = g.u32(0x45dbc0)?;
            if fixed == 0 { weight = g.u32(at(sp, 0x94))?; }
            scalar(c, 0, weight);
            c.esi = g.u32(at(sp, 0xc8))?;
            c.eax = at(sp, 0x34);
            g.set_u32(at(c.esi, 8), weight)?;
            c.ecx = g.u32(at(c.edi, 0x2c))?;
            push(c, g, c.eax)?;
            push(c, g, c.ecx)?;
            g.call(c, 0x1a68a0, 0x1a4ac5)
        }
        0x1a4ac5 => {
            c.edi = g.u32(at(sp, 0x40))?;
            c.edx = at(sp, 0x28);
            push(c, g, c.edx)?;
            push(c, g, c.edi)?;
            g.call(c, 0x1a68a0, 0x1a4ad4)
        }
        0x1a4ad4 => publish(c, g),
        _ => unreachable!("undeclared follow continuation"),
    }
}

pub fn update(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Boundary {
    if let Some(why) = super::super::guard(c, super::super::ROUNDING) {
        return crate::sim_native::decline(pc, why);
    }
    segment::<16, _>(c, g, pc, 0x1a4900..0x1a4bfc, UPDATE_SITES, |c, p, pc| follow(c, p, pc))
}
