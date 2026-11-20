use super::pose::{FRAME_A, FRAME_B, ROTATION_BLOCK, ROTATION_WEIGHT};
use super::*;
use crate::function_eval::constant::{ONE, SMALL};

pub const STREAM_MODE: u32 = 0x0050_4450;
pub const CODECS: u32 = 0x0047_fb24;
const NODE_MASKS: u32 = 0x0055_e530;
const NEAR_ONE: u32 = 0x0045_dc78;
const APPLY_STREAM: u32 = 0x0027_9860;
const TEST_DIRECT: u32 = 0x0027_a100;
const AFTER_STATIC: u32 = 0x0027_9c37;
const AFTER_TEST: u32 = 0x0027_9c61;
const AFTER_DYNAMIC: u32 = 0x0027_9d2c;

fn clamp(index: i32, last: i32) -> u32 {
    if index < 0 { 0 } else { index.min(last) as u32 }
}

// Above zero the guest first stores the last frame, then overwrites it
// when the candidate is in range. Keep both writes for observers.
fn store_next(
    g: &mut impl Memory,
    addr: u32,
    index: i32,
    last: i32,
) -> std::result::Result<(), GuestFault> {
    if index < 0 {
        g.set_u32(addr, 0)
    } else {
        g.set_u32(addr, last as u32)?;
        if index <= last {
            g.set_u32(addr, index as u32)?;
        }
        Ok(())
    }
}

fn int_float(c: &mut CpuAbi, reg: usize, value: u32) -> u32 {
    let old = u128::from_le_bytes(c.xmm[reg]);
    let v = simd::cvtsi2ss(&mut c.mxcsr, old, value).expect("masked SSE exceptions");
    c.xmm[reg] = v.to_le_bytes();
    v as u32
}

fn subtract_low(c: &mut CpuAbi, reg: usize, value: u32) -> u32 {
    let mut v = xmm(c, reg);
    v[0] = op(c, Sub, v[0], value);
    put_xmm(c, reg, v);
    v[0]
}

/// Clamp key frames and weights, select the codec and its per-channel
/// bitsets, then dispatch the selected stream.
pub fn select(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let cutoff = g.u32(SMALL)?;
    put_xmm(c, 2, scalar(cutoff));
    let weight = xmm(c, 4)[0];
    let active = compare(c, weight, cutoff) == 0;
    let secondary = g.u8(at(c.esp, 32))?;
    c.edx = low(c.edx, secondary);
    let saved = c.ebx;
    push(c, g, saved)?;
    c.ebx = g.u32(at(c.esp, 16))?;
    let saved = c.edi;
    push(c, g, saved)?;
    c.edi = g.u32(at(c.esp, 16))?;
    if active {
        let saved = c.ebp;
        push(c, g, saved)?;
        let sp = c.esp;
        let time = xmm(c, 1)[0];
        g.set_u32(at(sp, 44), time)?;
        let index = simd::cvttss2si(&mut c.mxcsr, time).expect("masked SSE exceptions") as i32;
        let last = i32::from(g.u16(at(c.edi, 20))? as i16) - 1;
        for i in 0..22 {
            g.set_u32(at(NODE_MASKS, 4 * i), 0)?;
        }
        for offset in [12, 16] {
            g.set_u32(at(STREAM_MODE, offset), c.ebx)?;
        }
        c.ebx = 0;
        for i in 22..24 {
            g.set_u32(at(NODE_MASKS, 4 * i), 0)?;
        }
        c.eax = clamp(index, last);
        c.ecx = last as u32;
        let one = g.u32(ONE)?;
        put_xmm(c, 0, scalar(0));
        g.set_u32(at(STREAM_MODE, 40), g.u32(at(sp, 28))?)?;
        g.set_u32(at(STREAM_MODE, 44), g.u32(at(sp, 32))?)?;
        for offset in [52, 56, 60] {
            g.set_u32(at(STREAM_MODE, offset), 0)?;
        }
        c.ebp = g.u32(at(sp, 36))?;
        g.set_u32(at(STREAM_MODE, 32), one)?;
        let near_one = g.u32(NEAR_ONE)?;
        put_xmm(c, 3, scalar(near_one));
        for (addr, v) in [
            (FRAME_A, c.eax),
            (FRAME_B, c.eax),
            (ROTATION_WEIGHT, 0),
            (at(STREAM_MODE, 36), c.edi),
            (ROTATION_BLOCK, 0),
            (at(STREAM_MODE, 64), 0),
            (at(STREAM_MODE, 68), 0),
            (at(STREAM_MODE, 72), 0),
            (at(STREAM_MODE, 76), c.ebp),
        ] {
            g.set_u32(addr, v)?;
        }
        g.set_u8(at(STREAM_MODE, 80), secondary)?;
        if secondary != 0 {
            let t = g.u32(at(sp, 48))?;
            g.set_u32(at(sp, 44), t)?;
            put_xmm(c, 0, scalar(t));
            put_xmm(c, 2, scalar(t));
            let i = simd::cvttss2si(&mut c.mxcsr, t).expect("masked SSE exceptions") as i32;
            let start = clamp(i, last);
            g.set_u32(at(STREAM_MODE, 84), start)?;
            store_next(g, at(STREAM_MODE, 88), i.wrapping_add(1), last)?;
            let integer = int_float(c, 0, start);
            let fraction = subtract_low(c, 2, integer);
            g.set_u32(at(STREAM_MODE, 92), fraction)?;
            c.edx = g.u32(FRAME_A)?;
            c.eax = at(c.edx, 1);
            store_next(g, FRAME_B, c.eax as i32, last)?;
            c.edi = g.u32(at(STREAM_MODE, 36))?;
            let integer = int_float(c, 0, c.edx);
            let fraction = subtract_low(c, 1, integer);
            let extra = g.u32(at(sp, 52))?;
            put_xmm(c, 0, scalar(extra));
            g.set_u32(ROTATION_WEIGHT, fraction)?;
            g.set_u32(at(STREAM_MODE, 96), extra)?;
        } else if g.u8(at(sp, 40))? != 0 {
            if c.eax as i32 == last {
                g.set_u8(at(sp, 40), 0)?;
            } else {
                c.edx = at(c.eax, 1);
                store_next(g, FRAME_B, c.edx as i32, last)?;
                let integer = int_float(c, 5, c.eax);
                let fraction = subtract_low(c, 1, integer);
                let tiny = compare(c, cutoff, fraction) == 0;
                g.set_u32(ROTATION_WEIGHT, fraction)?;
                if tiny || compare(c, fraction, near_one) == 0 {
                    if !tiny {
                        g.set_u32(FRAME_A, c.edx)?;
                    }
                    g.set_u32(ROTATION_WEIGHT, 0)?;
                    g.set_u8(at(sp, 40), 0)?;
                }
            }
        }
        let mask = g.u32(at(STREAM_MODE, 40))?;
        g.set_u32(at(STREAM_MODE, 4), 0)?;
        c.ecx = 1;
        if mask != 0 {
            c.eax = g.u32(at(sp, 28))?;
            c.edx = u32::from(g.u8(at(c.eax, 28))? & 1) + 1;
            g.set_u32(at(STREAM_MODE, 4), c.edx)?;
            c.eax = u32::from(g.u8(at(c.edi, 19))?);
            g.set_u32(at(STREAM_MODE, 12), c.eax)?;
        } else {
            c.eax = u32::from(g.u8(at(c.edi, 19))?);
            c.edx = g.u32(at(STREAM_MODE, 16))?;
            g.set_u32(at(STREAM_MODE, 12), c.eax)?;
            if c.edx as i32 <= c.eax as i32 {
                g.set_u32(at(STREAM_MODE, 12), c.edx)?;
            }
        }
        let full = compare(c, weight, near_one) == 0;
        g.set_u32(STREAM_MODE, 0)?;
        c.eax = low(c.eax, u8::from(full));
        c.edi = g.u32(at(sp, 20))?;
        let ty = g.u8(at(c.edi, 16))?;
        if ty == 1 || !full {
            let mode = if ty == 1 { 3 - u32::from(full) } else { 1 };
            if ty == 1 {
                c.eax = mode;
            }
            g.set_u32(STREAM_MODE, mode)?;
            g.set_u32(at(STREAM_MODE, 32), weight)?;
        }
        if ty == 0 {
            c.ecx = g.u32(at(c.esi, 4))?;
            if g.u16(at(c.ecx, 6))? != 0 {
                c.eax = g.u32(c.esi)?;
                return dispatch(c, g, true, 0);
            }
        }
        c.eax = g.u32(at(c.esi, 4))?;
        let size = g.u32(at(c.eax, 12))?;
        sub_flags(c, size, 0, 32);
        if size != 0 {
            let mode = g.u32(STREAM_MODE)?;
            sub_flags(c, mode, 0, 32);
            if mode == 0 {
                c.ecx = g.u32(at(sp, 24))?;
                c.edx = g.u32(at(sp, 28))?;
                c.eax = g.u32(at(sp, 16))?;
                for arg in [c.ebp, c.ecx, c.edx, c.edi, c.eax] {
                    push(c, g, arg)?;
                }
                return call(c, g, TEST_DIRECT, AFTER_TEST);
            }
            let header = g.u32(at(c.esi, 4))?;
            c.ebp = g.u32(c.esi)?;
            let blend = g.u8(at(sp, 40))?;
            c.eax = at(c.ebp, g.u16(at(header, 6))? as i16 as u32);
            return dispatch(c, g, false, blend);
        }
        c.ebp = pop(c, g)?;
    }
    c.edi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    ret(g, c, 40)
}

fn dispatch(c: &mut CpuAbi, g: &mut impl Memory, fixed: bool, blend: u8) -> Result {
    g.set_u32(ROTATION_BLOCK, c.eax)?;
    let codec = u32::from(g.u8(c.eax)?);
    let entry = at(CODECS, 40 * codec + 12 * u32::from(!fixed && blend != 0));
    let funcs = [g.u32(entry)?, g.u32(at(entry, 4))?, g.u32(at(entry, 8))?];
    let order = if fixed { [1, 0, 2] } else { [0, 2, 1] };
    for i in order {
        g.set_u32(at(STREAM_MODE, 52 + 4 * i as u32), funcs[i])?;
    }
    for channel in 0..3 {
        let header = g.u32(at(c.esi, 4))?;
        let mut offset = g.u16(at(header, 6))? as i16 as u32;
        let size = g.u32(at(header, 12))?;
        if !fixed {
            offset = at(offset, g.u8(header)? as i8 as u32);
        }
        if channel != 0 {
            let bytes = ((u32::from(g.u8(at(c.esi, 8))?) + 31) >> 3) & !3;
            offset = at(offset, channel * bytes);
        }
        let base = g.u32(c.esi)?;
        let ptr = at(at(base, size), offset);
        g.set_u32(at(STREAM_MODE, 64 + 4 * channel), ptr)?;
        if channel == 2 {
            if fixed {
                c.eax = base;
                c.ecx = size;
                c.edx = ptr;
            } else {
                c.edi = g.u16(at(header, 6))? as i16 as u32;
                c.ebx = size;
                c.eax = g.u8(header)? as i8 as u32;
                c.edx = low(c.edx, blend);
                c.ecx = ptr;
            }
            // Last ADD before the call: fixed adds block base to EDX;
            // dynamic also adds it last (to ECX).
            add_flags(c, ptr.wrapping_sub(base), base);
        }
    }
    g.set_u8(at(STREAM_MODE, 8), if fixed { 0 } else { blend })?;
    call(
        c,
        g,
        APPLY_STREAM,
        if fixed { AFTER_STATIC } else { AFTER_DYNAMIC },
    )
}
