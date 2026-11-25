//! Keep the requested playable range if it contains the pitch. Otherwise
//! visit ranges in tag order, taking the first enclosing range, or the
//! closest usable one when none encloses it.

use super::*;
use crate::sim_physics::{ARITH, status};

const PROMOTED: Point = Point::new(
    "SOUND_PROMOTED_PITCH_RANGE",
    0x218f10,
    crate::point_sig!(fn(sound: edx, rule: ecx) -> eax ret 0),
    DOC,
);
// ECX holds the tag, AX the preferred range, XMM1 the pitch; result in AX.
const PITCH: Point = Point::new(
    "SOUND_SELECT_PITCH_RANGE",
    0x218f50,
    crate::points::Sig::Opaque,
    DOC,
);

pub(super) const PROMOTED_BODY: Body = Body {
    point: &PROMOTED,
    sites: &[],
    run: promoted,
};
pub(super) const PITCH_BODY: Body = Body {
    point: &PITCH,
    sites: &[],
    run: pitch,
};

fn compare(c: &mut CpuAbi, a: u32, b: u32, sign: u32) {
    let mask = sign.wrapping_mul(2).wrapping_sub(1);
    let (a, b) = (a & mask, b & mask);
    let r = a.wrapping_sub(b) & mask;
    let overflow = (a ^ b) & (a ^ r) & sign != 0;
    c.eflags = (c.eflags & !ARITH)
        | status(r, sign)
        | u32::from(a < b)
        | ((a ^ b ^ r) & 0x10)
        | (u32::from(overflow) << 11);
    c.eflags_defined |= ARITH;
}

fn promoted(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let bank = g.u8(plus(c.edx, 12))?;
    c.edx = byte(c.edx, bank);
    c.eax = 0;
    compare(c, bank.into(), 0xff, 0x80);
    if bank != 0xff {
        push(c, g, c.esi)?;
        c.esi = g.u32(GESTALT)?;
        c.edx = arithmetic(
            c,
            (bank as i8 as i32 as u32).wrapping_mul(28),
            g.u32(plus(c.esi, 0x4c))?,
            false,
        );
        if c.edx != 0 {
            c.esi = g.u32(plus(c.edx, 4))?;
            c.ecx = plus(c.ecx.wrapping_mul(16), c.esi);
            c.ecx = low_word(c.ecx, u32::from(g.u16(c.ecx)?));
            compare(c, c.ecx, 0xffff, 0x8000);
            if c.ecx as u16 != 0xffff {
                c.eax = c.ecx & 0xffff;
            }
        }
        c.esi = pop(c, g)?;
    }
    Ok(Next::Ret { pop: 0 })
}

fn signed(g: &Guest, at: u32) -> Result<u32> {
    Ok(g.u16(at)? as i16 as i32 as u32)
}

/// The bounds are integral pitch units. Keep the SSE conversion and the
/// unordered compare even on NaN inputs: their flags are part of the ABI.
fn contains(c: &mut CpuAbi, g: &Guest, bounds: u32, preferred: bool) -> Result<bool> {
    let requested = sse::lane0(c, 1);
    let lo = signed(g, plus(bounds, 6))?;
    if preferred {
        c.ebx = lo;
    } else {
        c.eax = lo;
    }
    let low = sse::cvtsi2ss(c, 0, lo);
    if sse::comiss(c, low, requested) & 0x41 != 0 {
        c.eax = signed(g, plus(bounds, 8))?;
        let high = sse::cvtsi2ss(c, 0, c.eax);
        if sse::comiss(c, requested, high) & 0x41 != 0 {
            sse::movaps(c, 0, 1);
        }
    }
    sse::ucomiss(c, sse::lane0(c, 0), requested);
    sse::lahf(c);
    logic(c, (c.eax >> 8) & 0x44, 0x80);
    Ok(!sse::parity(c))
}

fn pitch(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    push(c, g, c.ecx)?;
    push(c, g, c.ebx)?;
    push(c, g, c.ebp)?;
    c.ebp = g.u8(plus(c.ecx, 10))? as i8 as i32 as u32;
    push(c, g, c.esi)?;
    c.esi = g.u32(GESTALT)?;
    push(c, g, c.edi)?;
    c.edi = c.eax;
    let preferred = c.eax as i16;
    set_slot(c, g, 16, u32::MAX)?;
    if preferred != -1 {
        c.edx = preferred as i32 as u32;
        if i32::from(preferred) < c.ebp as i32 {
            let idx = plus(signed(g, plus(c.ecx, 8))?, c.edx);
            c.ebx = g.u32(plus(c.esi, 0x1c))?;
            c.edx = plus(g.u32(plus(c.esi, 0x24))?, idx.wrapping_mul(12));
            let bounds = plus(c.ebx, signed(g, plus(c.edx, 2))?.wrapping_mul(10));
            c.eax = plus(bounds, 6);
            if contains(c, g, bounds, true)? {
                let count = g.u16(plus(c.edx, 10))?;
                compare(c, count.into(), 0, 0x8000);
                if count as i16 > 0 {
                    c.eax = low_word(c.eax, c.edi);
                    return restore(c, g);
                }
            }
        }
    }
    sse::load(c, 3, g.u32(0x44aed4)?);
    c.ebx = 0;
    logic(c, c.ebp, 1 << 31);
    if c.ebp as i32 > 0 {
        c.edx = signed(g, plus(c.ecx, 8))?;
        c.edi = g.u32(plus(c.esi, 0x24))?;
        loop {
            let row = plus(c.edi, plus(c.ebx, c.edx).wrapping_mul(12));
            c.eax = row;
            c.ecx = plus(c.ebx, c.edx).wrapping_mul(3);
            if g.u16(plus(row, 10))? as i16 > 0 {
                c.ecx = plus(
                    g.u32(plus(c.esi, 0x1c))?,
                    signed(g, plus(row, 2))?.wrapping_mul(10),
                );
                if contains(c, g, c.ecx, false)? {
                    c.eax = low_word(c.eax, c.ebx);
                    return restore(c, g);
                }
                let requested = sse::lane0(c, 1);
                c.eax = signed(g, plus(c.ecx, 4))?;
                let high = sse::cvtsi2ss(c, 2, c.eax);
                if sse::comiss(c, requested, high) & 0x41 == 0 {
                    sse::movaps(c, 0, 1);
                    sse::ss(c, sse::Ss::Sub, 0, high);
                } else {
                    c.ecx = signed(g, plus(c.ecx, 2))?;
                    sse::cvtsi2ss(c, 0, c.ecx);
                    sse::ss(c, sse::Ss::Sub, 0, requested);
                }
                if sse::comiss(c, sse::lane0(c, 3), sse::lane0(c, 0)) & 0x41 == 0 {
                    set_slot(c, g, 16, c.ebx)?;
                    sse::movaps(c, 3, 0);
                }
            }
            c.ebx += 1;
            c.eax = c.ebx;
            arithmetic(c, c.eax, c.ebp, true);
            if c.ebx >= c.ebp {
                break;
            }
        }
    }
    c.eax = low_word(c.eax, slot(c, g, 16)?);
    restore(c, g)
}

fn restore(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    // The local result occupies the caller's saved ECX slot.
    c.ecx = pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}
