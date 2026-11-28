//! The biped post-update's leaves: the forced-crouch test `e0dc0`, the
//! ground surface test `ddf60` and the landing update `e4330`. None calls
//! guest code; each reproduces every register, flag, stack store and x87
//! effect its retail code leaves.

use super::{NONE, above, and32, at, below_or_equal, cmp8, cmp32, flags8, store_ss, sx16};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_objects::alu;
use crate::sim_objects::biped::fld_fistp;
use crate::sim_objects::frame::slot;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, byte, logic};

/// `.rdata` constant: the landing fraction's upper clamp.
const K_45DBC0: u32 = 0x0045_dbc0;
/// Holds the structure surface flags `ddf60` reads (`+2c`, 8 bytes each).
const SURFACES: u32 = 0x004e_0340;

/// `e0dc0 (object: eax) -> al`; `ret`. 1 when body `+3dc` is state 6,
/// `+3f8` is non-zero and `+348` bit 13 is set, else 0. Exit `ecx` the
/// body or `+348 >> d`, `edx`'s low byte the last byte read, the flags of
/// the last `cmp` or `test`.
pub(in crate::sim_objects) fn crouch_forced(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::BIPED_CROUCH_FORCED.base {
        return Err(unowned(pc));
    }
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = g.u32(at(c.ecx, DATA))?;
    c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
    c.ecx = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
    let dl = g.u8(at(c.ecx, 0x3dc))?;
    c.edx = byte(c.edx, dl);
    c.eax = byte(c.eax, 0);
    logic(c, 0, 0x80);
    cmp8(c, dl, 6);
    if dl != 6 {
        return Ok(Next::Ret { pop: 0 });
    }
    let dl = g.u8(at(c.ecx, 0x3f8))?;
    c.edx = byte(c.edx, dl);
    if flags8(c, dl) == 0 {
        return Ok(Next::Ret { pop: 0 });
    }
    c.ecx = u32::from(g.u16(at(c.ecx, 0x348))?);
    c.ecx = alu::shr(c, c.ecx, 0xd, 32);
    if flags8(c, c.ecx as u8 & 1) == 0 {
        return Ok(Next::Ret { pop: 0 });
    }
    c.eax = byte(c.eax, 1);
    Ok(Next::Ret { pop: 0 })
}

/// `ddf60 (object: eax) -> al`; `ret`. With no contact object (body `+35c`
/// none), a surface (`+354`) and no second object (`+360` none): bit 0 of
/// the surface's flags (`[[4e0340] + 2c] + 8 × surface + 4`); otherwise 0.
/// On that path `eax` is the global's value with `al` replaced.
pub(in crate::sim_objects) fn ground_flag(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::BIPED_GROUND_FLAG.base {
        return Err(unowned(pc));
    }
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = g.u32(at(c.ecx, DATA))?;
    c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
    c.ecx = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
    c.edx = g.u32(at(c.ecx, 0x35c))?;
    c.eax = byte(c.eax, 0);
    logic(c, 0, 0x80);
    cmp32(c, c.edx, NONE);
    if c.edx != NONE {
        return Ok(Next::Ret { pop: 0 });
    }
    c.edx = g.u32(at(c.ecx, 0x354))?;
    cmp32(c, c.edx, NONE);
    if c.edx == NONE {
        return Ok(Next::Ret { pop: 0 });
    }
    let second = g.u32(at(c.ecx, 0x360))?;
    cmp32(c, second, NONE);
    if second != NONE {
        return Ok(Next::Ret { pop: 0 });
    }
    c.eax = g.u32(SURFACES)?;
    c.ecx = g.u32(at(c.eax, 0x2c))?;
    let al = g.u8(at(c.ecx.wrapping_add(c.edx.wrapping_mul(8)), 4))?;
    let al = flags8(c, al & 1);
    c.eax = byte(c.eax, al);
    Ok(Next::Ret { pop: 0 })
}

/// `e4330 (object: eax)`, with a speed in `xmm2`; `ret`. Frame `sub esp,
/// c` (`E` = that `esp`): `[E+3]` body `+348` bit 6 (a dead store),
/// `[E+4]` the product, `[E+8]` its `fistp`. From the definition's three
/// speeds `a` `+204`, `b` `+208`, `c` `+20c` and two times `+1fc`,
/// `+200`: with `+348` bit 6 set (cleared here), kind 1 and the fraction
/// `c / (c − b)` of `+200`; else nothing below `a`, kind 0 and
/// `(speed − a) / (b − a)` of `+1fc` below `b`, otherwise kind 1 and
/// `speed / (c − b)` of `+200`. The fraction is clamped to
/// `[0, [45dbc0]]`, the time is in ticks (`fistp`). A positive tick count
/// stores the kind at body `+34c`, 0 at `+34e` and the count (at most
/// `7f`) at `+34f`.
pub(in crate::sim_objects) fn landing_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::BIPED_LANDING_UPDATE.base {
        return Err(unowned(pc));
    }
    c.esp = arithmetic(c, c.esp, 0xc, true);
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = g.u32(at(c.ecx, DATA))?;
    c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
    c.ecx = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
    c.eax = g.u32(c.ecx)?;
    c.edx = g.u32(points::TAG_ENTRIES.base)?;
    c.eax = and32(c, c.eax & 0xffff);
    c.eax = alu::shl(c, c.eax, 4, 32);
    c.eax = g.u32(at(at(c.eax, c.edx), 8))?;
    c.edx = byte(c.edx, g.u8(at(c.ecx, 0x348))?);
    sse::load(c, 3, g.u32(at(c.eax, 0x204))?);
    sse::load(c, 1, g.u32(at(c.eax, 0x208))?);
    sse::load(c, 0, g.u32(at(c.eax, 0x20c))?);
    let dl = alu::shr(c, c.edx & 0xff, 6, 8) as u8;
    let dl = flags8(c, dl & 1);
    c.edx = byte(c.edx, dl);
    g.set_u8(at(c.esp, 3), dl)?;
    let next = if dl != 0 {
        // `e43ab`: the third band.
        sse::movaps(c, 2, 0);
        sse::ss_reg(c, Ss::Sub, 0, 1);
        sse::load(c, 1, g.u32(at(c.eax, 0x200))?);
        alu::rmw8(c, g, at(c.ecx, 0x348), |v| v & 0xbf)?;
        c.eax = 1;
        0xe43d4
    } else {
        let (x2, x3) = (sse::lane0(c, 2), sse::lane0(c, 3));
        if above(c, x3, x2) {
            0xe4444
        } else {
            let x1 = sse::lane0(c, 1);
            if below_or_equal(c, x1, x2) {
                // `e43c3`.
                sse::ss_reg(c, Ss::Sub, 0, 1);
                sse::load(c, 1, g.u32(at(c.eax, 0x200))?);
                c.eax = 1;
            } else {
                sse::ss_reg(c, Ss::Sub, 1, 3);
                sse::movaps(c, 0, 1);
                sse::load(c, 1, g.u32(at(c.eax, 0x1fc))?);
                sse::ss_reg(c, Ss::Sub, 2, 3);
                c.eax = and32(c, 0);
            }
            0xe43d4
        }
    };
    if next == 0xe43d4 {
        sse::zero(c, 3);
        let x0 = sse::lane0(c, 0);
        if !below_or_equal(c, x0, 0) {
            sse::ss_reg(c, Ss::Div, 2, 0);
            let x2 = sse::lane0(c, 2);
            if above(c, 0, x2) {
                sse::movaps(c, 2, 3);
            } else {
                sse::load(c, 0, g.u32(K_45DBC0)?);
                let x0 = sse::lane0(c, 0);
                if above(c, x2, x0) {
                    sse::movaps(c, 2, 0);
                }
            }
            // `e43fa`.
            c.edx = g.u32(points::GAME_TIME.base)?;
            c.edx = sx16(g.u16(at(c.edx, 2))?);
            sse::ss_reg(c, Ss::Mul, 1, 2);
            sse::cvtsi2ss(c, 0, c.edx);
            sse::ss_reg(c, Ss::Mul, 1, 0);
            store_ss(c, g, at(c.esp, 4), 1)?;
            fld_fistp(c, g, at(c.esp, 4), at(c.esp, 8), 0xe441a)?;
            c.edx = slot(c, g, 8)?;
            logic(c, c.edx, 1 << 31);
            if !alu::less_or_equal(c) {
                cmp32(c, c.edx, 0x7f);
                g.set_u16(at(c.ecx, 0x34c), c.eax as u16)?;
                g.set_u8(at(c.ecx, 0x34e), 0)?;
                if !alu::less_or_equal(c) {
                    c.edx = 0x7f;
                }
                g.set_u8(at(c.ecx, 0x34f), c.edx as u8)?;
            }
        }
    }
    c.esp = arithmetic(c, c.esp, 0xc, false);
    Ok(Next::Ret { pop: 0 })
}
