use super::*;
use guest_layout::points;
use mopp::boxvm::{Visit, diagonal, two_plane};
use mopp::code::terminal_key;

const BOX_CLASSES: u32 = 0x002f_ff20;
const BOX_NEXT: u32 = 0x002f_fac2;
const BOX_SINGLE_SPLIT: u32 = 0x002f_faf9;
const GROW_KEYS: u32 = 0x002d_91d0;
const AFTER_GROW: u32 = 0x002f_fe82;
pub const RAY_DISPATCH: u32 = 0x0030_0a10;
const RAY_CLASSES: u32 = 0x0030_16a8;
const INITIAL_AXIS: u32 = 0x0030_0a00;
const AFTER_FILTER: u32 = 0x0030_1587;
const AFTER_SHAPE: u32 = 0x0030_15a7;

fn operand(g: &impl Memory, pc: u32, len: usize) -> std::result::Result<u32, GuestFault> {
    let mut v = 0;
    for i in 1..=len {
        v = (v << 8) | u32::from(g.u8(at(pc, i as u32))?);
    }
    Ok(v)
}

fn box_return(c: &mut CpuAbi, g: &impl Memory) -> Result {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    add_flags(c, c.esp, 56);
    c.esp = at(c.esp, 56);
    ret(g, c, 8)
}

/// Localize the integer machine's state before changing a property or key
/// offset. REP MOVSD makes the order observable if the frames overlap.
fn localize(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    local: u32,
) -> std::result::Result<(), GuestFault> {
    if c.esi != local {
        c.edi = local;
        let step = if c.eflags & (1 << 10) == 0 {
            4
        } else {
            0u32.wrapping_sub(4)
        };
        for _ in 0..14 {
            let value = g.u32(c.esi)?;
            g.set_u32(c.edi, value)?;
            c.esi = at(c.esi, step);
            c.edi = at(c.edi, step);
        }
        c.esi = local;
        c.ecx = 0;
    }
    Ok(())
}

fn rescale(c: &mut CpuAbi, g: &mut impl Memory, shift: u32) -> std::result::Result<(), GuestFault> {
    let local = at(c.esp, 16);
    let mut offsets = [0; 3];
    for (i, offset) in offsets.iter_mut().enumerate() {
        let axis = i as u32 * 4;
        let delta = u32::from(g.u8(at(c.ebx, 1 + i as u32))?);
        *offset = at(delta, g.u32(at(c.esi, 32 + axis))?).wrapping_shl(shift);
        g.set_u32(at(local, 32 + axis), *offset)?;
    }
    let bits = at(g.u32(at(c.esi, 48))?, shift);
    g.set_u32(at(local, 48), bits)?;
    let remaining = mopp::code::ROOT_SHIFT.wrapping_sub(bits);
    for (src, dst, bias) in [(32, 16, 0), (16, 0, 1)] {
        for (i, offset) in offsets.into_iter().enumerate() {
            let axis = i as u32 * 4;
            let value = (g.u32(at(c.ebp, src + axis))? as i32).wrapping_shr(remaining) as u32;
            let bound = at(value, bias).wrapping_sub(offset);
            g.set_u32(at(local, dst + axis), bound)?;
            if dst == 0 && i == 0 {
                c.edi = bound;
            }
        }
    }
    c.edx = offsets[1];
    c.eax = g.u32(at(c.esi, 52))?;
    g.set_u32(at(local, 52), c.eax)?;
    c.ecx = g.u32(at(c.esi, 44))?;
    g.set_u32(at(local, 44), c.ecx)?;
    c.esi = local;
    c.ebx = at(c.ebx, 4);
    Ok(())
}

/// Interpret the integer box program using its live query frame. Children
/// receive the real stack frame, including its local rescale and key state.
pub fn box_query(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    c.esp = c.esp.wrapping_sub(56);
    let saved = c.ebx;
    push(c, g, saved)?;
    c.ebx = g.u32(at(c.esp, 68))?;
    let saved = c.ebp;
    push(c, g, saved)?;
    c.ebp = c.ecx;
    let first = g.u8(c.ebx)?;
    for saved in [c.esi, c.edi] {
        push(c, g, saved)?;
    }
    if first <= 0x6b {
        c.esi = g.u32(at(c.esp, 76))?;
    }
    for _ in 0..256 {
        let pc = c.ebx;
        let op = g.u8(pc)?;
        c.ecx = u32::from(op);
        if op > 0x6b {
            break;
        }
        c.eax = u32::from(g.u8(at(BOX_CLASSES, c.ecx))?);
        match op {
            0 => return box_return(c, g),
            1..=4 => rescale(c, g, u32::from(op))?,
            0x09..=0x0b => {
                let n = 1usize << (op - 9);
                c.eax = operand(g, pc, n)?;
                if n > 1 {
                    c.ecx = c.eax & 255;
                }
                c.edx = at(c.esp, 16);
                localize(c, g, c.edx)?;
                let slot = at(c.esp, 60);
                let value = if n == 4 {
                    c.eax
                } else {
                    at(g.u32(slot)?, c.eax)
                };
                g.set_u32(slot, value)?;
                c.ebx = at(pc, 1 + n as u32);
            }
            0x60 | 0x64 | 0x68 => {
                let n = 1usize << ((op - 0x60) / 4);
                c.eax = operand(g, pc, n)?;
                if n > 1 {
                    c.edx = c.eax & 255;
                }
                let property = at(c.esp, 68);
                g.set_u32(property, c.eax)?;
                c.eax = g.u32(property)?;
                c.ecx = at(c.esp, 16);
                localize(c, g, c.ecx)?;
                g.set_u32(property, c.eax)?;
                c.ebx = at(pc, 1 + n as u32);
            }
            0x05..=0x07 => {
                let n = usize::from(op - 4);
                let skip = operand(g, pc, n)?;
                if n == 1 {
                    c.eax = skip;
                } else {
                    c.edx = at(pc, skip & 255);
                    if n == 3 {
                        c.eax = skip & !255;
                    }
                }
                c.ebx = at(at(pc, n as u32 + 1), skip);
            }
            0x10..=0x12 => {
                let axis = u32::from(op - 0x10) * 4;
                c.eax = u32::from(g.u8(at(pc, 2))?);
                c.edx = u32::from(g.u8(at(pc, 1))?);
                let lower = g.u32(at(c.esi, 16 + axis))? as i32;
                let upper = g.u32(at(c.esi, axis))? as i32;
                c.edi = lower as u32;
                c.ebx = at(pc, 4);
                match two_plane(
                    i64::from(lower),
                    i64::from(upper),
                    i64::from(c.edx),
                    i64::from(c.eax),
                ) {
                    Visit::None => return box_return(c, g),
                    Visit::Low => (),
                    Visit::High => c.ebx = at(c.ebx, u32::from(g.u8(at(pc, 3))?)),
                    Visit::Both => {
                        let child = c.ebx;
                        c.eax = u32::from(g.u8(at(pc, 3))?);
                        c.ebx = at(child, c.eax);
                        sub_flags(c, c.ebx, c.eax, 32);
                        for arg in [child, c.esi] {
                            push(c, g, arg)?;
                        }
                        c.ecx = c.ebp;
                        return call(c, g, points::SIM_MOPP_BOX.base, BOX_NEXT);
                    }
                }
            }
            0x13..=0x1c => {
                let mut hi = [0; 3];
                let mut lo = [0; 3];
                for i in 0..3 {
                    hi[i] = i64::from(g.u32(at(c.esi, 4 * i as u32))? as i32);
                    lo[i] = i64::from(g.u32(at(c.esi, 16 + 4 * i as u32))? as i32);
                }
                let (lower, upper, m, k) = diagonal(lo, hi, op);
                let low_max = i64::from(g.u8(at(pc, 1))?) * m - k;
                let high_min = i64::from(g.u8(at(pc, 2))?) * m - k;
                c.eax = lower as u32;
                c.ecx = upper as u32;
                c.edx = low_max as u32;
                c.edi = high_min as u32;
                c.ebx = at(pc, 4);
                match two_plane(
                    i64::from(c.eax as i32),
                    i64::from(c.ecx as i32),
                    low_max,
                    high_min,
                ) {
                    Visit::None => return box_return(c, g),
                    Visit::Low => (),
                    Visit::High => {
                        c.edi = u32::from(g.u8(at(pc, 3))?);
                        c.ebx = at(c.ebx, c.edi);
                    }
                    Visit::Both => {
                        c.edi = u32::from(g.u8(at(pc, 3))?);
                        sub_flags(c, c.eax, c.edx, 32);
                        for arg in [c.ebx, c.esi] {
                            push(c, g, arg)?;
                        }
                        c.ecx = c.ebp;
                        const AFTER_DIAGONAL: u32 = 0x002f_fa7e;
                        return call(c, g, points::SIM_MOPP_BOX.base, AFTER_DIAGONAL);
                    }
                }
            }
            0x20..=0x22 => {
                let axis = u32::from(op - 0x20) * 4;
                c.edx = u32::from(g.u8(at(pc, 1))?);
                c.eax = g.u32(at(c.esi, axis))?;
                c.ebx = at(pc, 3);
                if c.eax as i32 > c.edx as i32 {
                    c.eax = u32::from(g.u8(at(pc, 2))?);
                    c.edi = g.u32(at(c.esi, 16 + axis))?;
                    let child = c.ebx;
                    c.ebx = at(c.ebx, c.eax);
                    if c.edi as i32 <= c.edx as i32 {
                        c.edx = child;
                        sub_flags(c, c.ebx, c.eax, 32);
                        for arg in [child, c.esi] {
                            push(c, g, arg)?;
                        }
                        c.ecx = c.ebp;
                        return call(c, g, points::SIM_MOPP_BOX.base, BOX_SINGLE_SPLIT);
                    }
                }
            }
            0x23..=0x25 => {
                let axis = u32::from(op - 0x23) * 4;
                c.eax = u32::from(g.u8(at(pc, 1))?);
                c.edi = u32::from(g.u8(at(pc, 4))?);
                c.edx = u32::from(g.u8(at(pc, 2))?);
                g.set_u32(at(c.esp, 80), c.eax)?;
                c.eax = g.u32(at(c.esi, 16 + axis))?;
                g.set_u32(at(c.esp, 76), c.eax)?;
                c.eax = u32::from(g.u8(at(pc, 3))?) * 256 + c.edi;
                c.edi = g.u32(at(c.esi, axis))?;
                c.ebx = at(pc, 7);
                if c.edi as i32 <= c.edx as i32 {
                    c.ecx = g.u32(at(c.esp, 80))?;
                    if g.u32(at(c.esp, 76))? as i32 >= c.ecx as i32 {
                        return box_return(c, g);
                    }
                    c.ebx = at(c.ebx, c.eax);
                } else {
                    c.ecx = operand(g, at(pc, 4), 2)?;
                    c.edi = g.u32(at(c.esp, 76))?;
                    c.edx = g.u32(at(c.esp, 80))?;
                    c.ebx = at(c.ebx, c.ecx);
                    if (c.edi as i32) < c.edx as i32 {
                        c.eax = c.eax.wrapping_sub(c.ecx);
                        add_flags(c, c.eax, c.ebx);
                        c.eax = at(c.eax, c.ebx);
                        for arg in [c.eax, c.esi] {
                            push(c, g, arg)?;
                        }
                        c.ecx = c.ebp;
                        const AFTER_WIDE_SPLIT: u32 = 0x002f_fb6e;
                        return call(c, g, points::SIM_MOPP_BOX.base, AFTER_WIDE_SPLIT);
                    }
                }
            }
            0x26..=0x28 => {
                let axis = u32::from(op - 0x26) * 4;
                c.eax = u32::from(g.u8(at(pc, 1))?);
                if (g.u32(at(c.esi, axis))? as i32) < c.eax as i32 {
                    return box_return(c, g);
                }
                c.edx = u32::from(g.u8(at(pc, 2))?);
                if g.u32(at(c.esi, 16 + axis))? as i32 >= c.edx as i32 {
                    return box_return(c, g);
                }
                c.ebx = at(pc, 3);
            }
            0x29..=0x2b => {
                let axis = u32::from(op - 0x29) * 4;
                c.eax = operand(g, pc, 3)?;
                c.edx = c.eax & 255;
                if (g.u32(at(c.ebp, 16 + axis))? as i32) < c.eax as i32 {
                    return box_return(c, g);
                }
                c.eax = operand(g, at(pc, 3), 3)?;
                c.edx = c.eax & 255;
                if g.u32(at(c.ebp, 32 + axis))? as i32 > c.eax as i32 {
                    return box_return(c, g);
                }
                c.ebx = at(pc, 7);
            }
            0x30..=0x53 => {
                let key = terminal_key(op, |n| operand(g, pc, n))?;
                if matches!(op, 0x51 | 0x52) {
                    c.edx = key & 255;
                }
                if op == 0x53 {
                    c.edx = (key >> 8) & 255;
                }
                c.esi = at(g.u32(at(c.esi, 44))?, key);
                c.ebp = g.u32(c.ebp)?;
                c.eax = g.u32(at(c.ebp, 4))?;
                c.ecx = g.u32(at(c.ebp, 8))? & 0x7fff_ffff;
                sub_flags(c, c.eax, c.ecx, 32);
                if c.eax == c.ecx {
                    for arg in [4, c.ebp] {
                        push(c, g, arg)?;
                    }
                    return call(c, g, GROW_KEYS, AFTER_GROW);
                }
                c.edx = g.u32(at(c.ebp, 4))?;
                c.eax = g.u32(c.ebp)?;
                g.set_u32(at(c.eax, c.edx << 2), c.esi)?;
                c.eax = at(g.u32(at(c.ebp, 4))?, 1);
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                g.set_u32(at(c.ebp, 4), c.eax)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                add_flags(c, c.esp, 56);
                c.esp = at(c.esp, 56);
                return ret(g, c, 8);
            }
            _ => break,
        }
    }
    Ok(TransferAbi::resume(BOX_NEXT))
}

fn ray_return(c: &mut CpuAbi, g: &impl Memory) -> Result {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = c.ebp;
    c.ebp = pop(c, g)?;
    ret(g, c, 12)
}

/// Traverse the ray program, rescaling and clipping its endpoints in
/// place. Recursive clips and shape callbacks retain the guest frame.
pub fn ray_query(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let saved = c.ebp;
    push(c, g, saved)?;
    c.ebp = c.esp;
    c.esp = (c.esp & !15).wrapping_sub(0x194);
    for saved in [c.ebx, c.esi] {
        push(c, g, saved)?;
    }
    c.esi = g.u32(at(c.ebp, 12))?;
    c.eax = u32::from(g.u8(c.esi)?);
    let saved = c.edi;
    push(c, g, saved)?;
    g.set_u32(at(c.esp, 40), c.ecx)?;
    let axis = g.u32(INITIAL_AXIS)?;
    g.set_u32(at(c.esp, 60), axis)?;
    if c.eax <= 0x6b {
        c.edi = g.u32(at(c.ebp, 16))?;
        c.ebx = g.u32(at(c.ebp, 8))?;
    }
    for processed in 0.. {
        let pc = c.esi;
        let op = c.eax as u8;
        sub_flags(c, c.eax, 0x6b, 32);
        if op > 0x6b {
            const ERRORS: u32 = 0x0048_017c;
            const ERROR_TEXT_OPERAND: u32 = 0x0030_0d90;
            c.ecx = g.u32(ERRORS)?;
            c.edx = g.u32(c.ecx)?;
            let message = g.u32(ERROR_TEXT_OPERAND)?;
            for arg in [c.eax, message] {
                push(c, g, arg)?;
            }
            let target = g.u32(at(c.edx, 4))?;
            return call(c, g, target, 0x0030_0d97);
        }
        if processed == 256 {
            return Ok(TransferAbi::resume(RAY_DISPATCH));
        }
        c.ecx = u32::from(g.u8(at(RAY_CLASSES, c.eax))?);
        match op {
            0 => return ray_return(c, g),
            1..=4 => ray::rescale(c, g, op.into())?,
            9..=11 | 0x60 | 0x64 | 0x68 => {
                let n = 1usize << if op < 12 { op - 9 } else { (op - 0x60) / 4 };
                let value = operand(g, pc, n)?;
                ray::state_key(c, g, op, value)?;
            }
            0x10..=0x1c | 0x20..=0x25 => {
                if let Some(transfer) = ray::split(c, g, op)? {
                    return Ok(transfer);
                }
            }
            0x26..=0x2b => {
                let n = if op < 0x29 { 1 } else { 3 };
                let lo = operand(g, pc, n)?;
                let hi = operand(g, at(pc, n as u32), n)?;
                if !ray::bounds(c, g, op, lo, hi)? {
                    return ray_return(c, g);
                }
            }
            0x05..=0x07 => {
                let n = usize::from(op - 4);
                let skip = operand(g, pc, n)?;
                if n > 1 {
                    c.edx = at(pc, skip & 255);
                }
                c.esi = at(at(pc, 1 + n as u32), skip);
            }
            0x30..=0x53 => {
                let key = terminal_key(op, |n| operand(g, pc, n))?;
                c.eax = g.u32(at(c.esp, 40))?;
                c.esi = g.u32(at(c.ebx, 24))?;
                c.ecx = g.u32(at(c.eax, 68))?;
                c.edi = at(c.esi, key);
                logic_flags(c, c.ecx);
                if c.ecx != 0 {
                    c.esi = g.u32(at(c.eax, 100))?;
                    c.edx = g.u32(c.ecx)?;
                    push(c, g, c.edi)?;
                    push(c, g, c.esi)?;
                    add_flags(c, c.eax, 32);
                    c.eax = at(c.eax, 32);
                    push(c, g, c.eax)?;
                    c.eax = at(c.esp, 71);
                    push(c, g, c.eax)?;
                    let target = g.u32(c.edx)?;
                    return call(c, g, target, AFTER_FILTER);
                }
                c.eax = g.u32(at(c.eax, 100))?;
                c.edx = g.u32(c.eax)?;
                c.ecx = at(c.esp, 160);
                for arg in [c.ecx, c.edi] {
                    push(c, g, arg)?;
                }
                c.ecx = c.eax;
                let target = g.u32(at(c.edx, 52))?;
                return call(c, g, target, AFTER_SHAPE);
            }
            _ => return Ok(TransferAbi::resume(RAY_DISPATCH)),
        }
        c.eax = u32::from(g.u8(c.esi)?);
        g.set_u32(at(c.esp, 60), axis)?;
    }
    unreachable!()
}
