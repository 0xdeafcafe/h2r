//! Unit aim in object space. The forward and up vectors come from the
//! object's basis call; each resolved aim track supplies angular bounds.
use super::*;
use crate::light_constants::{Error, X87};
use x87::abi::{arith_op, unary_op};

const FIND_TRACK: u32 = 0x001d_af30;
const AFTER_BASIS: u32 = 0x0011_24e6;
const FIRST_TRACK: u32 = 0x0011_259f;
const SECOND_TRACK: u32 = 0x0011_25c8;
const NO_TRACKS: u32 = 0x0011_29c5;
const HAVE_ANGLES: u32 = 0x0011_2769;

type Checked<T> = std::result::Result<T, GuestFault>;

/// Up cross forward. The two returned track IDs share the scratch frame
/// with the layer setup and have to be reloaded across the lookups.
fn basis(c: &mut CpuAbi, g: &mut impl Memory) -> Checked<()> {
    let sp = c.esp;
    let fz = g.u32(at(sp, 0x7c))?;
    let uz = g.u32(at(sp, 0x94))?;
    let uy = g.u32(at(sp, 0x90))?;
    let fy = g.u32(at(sp, 0x78))?;
    c.eax = g.u32(at(c.ebp, 0xfc))?;
    c.ecx = g.u32(at(c.ebp, 0x100))?;

    let uz_fy = op(c, Mul, uz, fy);
    let fz_uy = op(c, Mul, fz, uy);
    let side_x = op(c, Sub, fz_uy, uz_fy);
    let fx = g.u32(at(sp, 0x74))?;
    let ux = g.u32(at(sp, 0x8c))?;
    g.set_u32(at(sp, 0x24), c.eax)?;
    c.eax >>= 16;
    sub_flags(c, c.eax, c.ebx, 16);

    let uz_fx = op(c, Mul, uz, fx);
    let fz_ux = op(c, Mul, fz, ux);
    let fy_ux = op(c, Mul, fy, ux);
    let uy_fx = op(c, Mul, uy, fx);
    let side_y = op(c, Sub, uz_fx, fz_ux);
    let side_z = op(c, Sub, fy_ux, uy_fx);
    for (i, v) in [side_z, uy_fx, ux, side_x, side_y, fx, fz_ux]
        .into_iter()
        .enumerate()
    {
        put_xmm(c, i, scalar(v));
    }
    for (off, v) in [(0x80, side_x), (0x84, side_y), (0x88, side_z)] {
        g.set_u32(at(sp, off), v)?;
    }
    g.set_u8(at(sp, 0x12), 0)?;
    g.set_u32(at(sp, 0x14), c.ecx)
}

fn reload_side(c: &mut CpuAbi, g: &impl Memory) -> Checked<()> {
    put_xmm(c, 3, scalar(g.u32(at(c.esp, 0x80))?));
    put_xmm(c, 4, scalar(g.u32(at(c.esp, 0x84))?));
    Ok(())
}

pub(super) fn run(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Segment {
    match pc {
        AFTER_BASIS => {
            basis(c, g)?;
            let have_first = c.eax as u16 != c.ebx as u16;
            if !have_first {
                c.ecx >>= 16;
                sub_flags(c, c.ecx, c.ebx, 16);
                if c.ecx as u16 == c.ebx as u16 {
                    return Ok(Step::Next(NO_TRACKS));
                }
            }
            c.esi = 0;
            c.edi = 0;
            sub_flags(c, c.eax, c.ebx, 16);
            if have_first {
                c.edx = g.u32(at(c.esp, 0x24))?;
                c.eax = g.u32(at(c.esp, 0x68))?;
                push(c, g, c.edx)?;
                return invoke(c, g, FIND_TRACK, FIRST_TRACK);
            }
        }
        FIRST_TRACK => {
            reload_side(c, g)?;
            c.esi = c.eax;
        }
        SECOND_TRACK => {
            reload_side(c, g)?;
            c.edi = c.eax;
        }
        _ => unreachable!("unit aim continuation {pc:#x}"),
    }

    if pc != SECOND_TRACK {
        let index = u32::from(g.u16(at(c.esp, 0x16))?);
        sub_flags(c, index, c.ebx, 16);
        if index as u16 != c.ebx as u16 {
            c.eax = g.u32(at(c.esp, 0x14))?;
            push(c, g, c.eax)?;
            c.eax = g.u32(at(c.esp, 0x6c))?;
            return invoke(c, g, FIND_TRACK, SECOND_TRACK);
        }
    }

    logic_flags(c, c.esi);
    if c.esi == 0 {
        logic_flags(c, c.edi);
        if c.edi == 0 {
            return Ok(Step::Next(NO_TRACKS));
        }
    }
    project(c, g)?;
    limits(c, g)?;
    Ok(Step::Next(HAVE_ANGLES))
}

fn float_fault(_: Error) -> GuestFault {
    GuestFault {
        addr: 0,
        len: 0,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::InvalidOperation,
    }
}

// Each axis is a scalar SSE dot product with an intermediate binary32
// store. x87 takes atan2(side, forward) and atan2(up, horizontal length).
fn project(c: &mut CpuAbi, g: &mut impl Memory) -> Checked<()> {
    let sp = c.esp;
    c.eax = g.u32(at(sp, 0x2c))?;
    let dz = g.u32(at(c.eax, 0x170))?;
    let dy = g.u32(at(c.eax, 0x16c))?;
    let dx = g.u32(at(c.eax, 0x168))?;

    let forward_z = op(c, Mul, g.u32(at(sp, 0x7c))?, dz);
    let forward_y = op(c, Mul, g.u32(at(sp, 0x78))?, dy);
    let forward_zy = op(c, Add, forward_z, forward_y);
    let forward_x = op(c, Mul, dx, g.u32(at(sp, 0x74))?);
    let forward = op(c, Add, forward_zy, forward_x);
    g.set_u32(at(sp, 0x3c), forward)?;

    let side_z = op(c, Mul, g.u32(at(sp, 0x88))?, dz);
    let side_y = op(c, Mul, xmm(c, 4)[0], dy);
    let side_zy = op(c, Add, side_z, side_y);
    let side_x = op(c, Mul, dx, xmm(c, 3)[0]);
    let side = op(c, Add, side_zy, side_x);
    g.set_u32(at(sp, 0x40), side)?;

    let mut x = X87(ray::load_fpu(c));
    x.fld(g.u32(at(sp, 0x40))?).map_err(float_fault)?;
    x.fld(g.u32(at(sp, 0x3c))?).map_err(float_fault)?;
    let uz = g.u32(at(sp, 0x94))?;
    x.unary(unary_op::PATAN).map_err(float_fault)?;

    let up_x = op(c, Mul, dx, g.u32(at(sp, 0x8c))?);
    let up_z = op(c, Mul, uz, dz);
    let up_y = op(c, Mul, g.u32(at(sp, 0x90))?, dy);
    let up_zy = op(c, Add, up_z, up_y);
    let up = op(c, Add, up_zy, up_x);
    for (i, v) in [up_x, dy, up_y, up, side_x, side, forward_x]
        .into_iter()
        .enumerate()
    {
        put_xmm(c, i, scalar(v));
    }
    g.set_u32(at(sp, 0x44), up)?;
    g.set_u8(c.ebp, g.u8(c.ebp)? | (1 << 5))?;
    logic_flags(c, c.esi);
    g.set_u32(at(sp, 0x34), x.fstp_m32().map_err(float_fault)?)?;
    x.fld(g.u32(at(sp, 0x44))?).map_err(float_fault)?;
    for off in [0x3c, 0x40] {
        let axis = g.u32(at(sp, off))?;
        x.fld(axis).map_err(float_fault)?;
        x.fmul_m(axis).map_err(float_fault)?;
    }
    x.arith_reg(arith_op::ADD, 1, 0, true)
        .map_err(float_fault)?;
    x.unary(unary_op::SQRT).map_err(float_fault)?;
    // Each exchange sets x87 condition state even though the pair restores
    // operand order, including on a masked stack fault.
    x.fxch(1).map_err(float_fault)?;
    x.fxch(1).map_err(float_fault)?;
    x.unary(unary_op::PATAN).map_err(float_fault)?;
    let pitch = at(sp, 0x38);
    g.set_u32(pitch, x.fstp_m32().map_err(float_fault)?)?;
    ray::finish_fpu(c, g, &x.0, 0x0011_26b8, pitch)
}

fn scaled_limit(c: &mut CpuAbi, frames: u32, step: u32) -> u32 {
    let bits = simd::cvtsi2ss(&mut c.mxcsr, 0, frames).expect("masked SSE exceptions") as u32;
    op(c, Mul, bits, step)
}

// The first resolved track supplies the bounds; use the second if only it
// resolved. The two paths leave different integer registers live.
fn limits(c: &mut CpuAbi, g: &mut impl Memory) -> Checked<()> {
    let first = c.esi != 0;
    let track = if first { c.esi } else { c.edi };
    if !first {
        logic_flags(c, c.edi);
    }
    let left = g.u16(at(track, 8))? as i16 as i32 as u32;
    let v = scaled_limit(c, left, g.u32(track)?);
    let lower_yaw = op(c, Sub, 0, v);
    g.set_u32(at(c.ebp, 4), lower_yaw)?;
    let right = g.u16(at(track, 0x0a))? as i16 as i32 as u32;
    let upper_yaw = scaled_limit(c, right, g.u32(at(track, 4))?);
    g.set_u32(at(c.ebp, 8), upper_yaw)?;
    let down = g.u16(at(track, 0x14))? as i16 as i32 as u32;
    let down_angle = scaled_limit(c, down, g.u32(at(track, 0x0c))?);
    let lower_pitch = op(c, Sub, 0, down_angle);
    g.set_u32(at(c.ebp, 0x0c), lower_pitch)?;
    let up = g.u16(at(track, 0x16))? as i16 as i32 as u32;
    let upper_pitch = scaled_limit(c, up, g.u32(at(track, 0x10))?);
    g.set_u32(at(c.ebp, 0x10), upper_pitch)?;
    put_xmm(c, 0, scalar(upper_pitch));
    put_xmm(c, 1, scalar(down_angle));
    put_xmm(c, 2, scalar(lower_yaw));
    if first {
        c.eax = down;
        c.ecx = up;
        c.edx = right;
    } else {
        c.eax = right;
        c.ecx = down;
        c.edx = up;
    }
    Ok(())
}
