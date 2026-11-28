//! The unit level's import handler `c6b90` (slot `+4c`) and its two
//! helpers `d1210` and `d1410` (docs/engine/object-update.md, "Slice 3
//! bodies").
//!
//! `c6b90 (object, name, value out, active out) -> al` answers the unit's
//! named imports: body fields read directly, two computed ratios (`d1210`,
//! `d1410` and its complement), and two names answered by the held weapon
//! through the weapon import `ff5f0`. A value answer is clamped to
//! `[0, 1]`, stored, and active is "value > 0"; `al` is `bl`, 1. An
//! unknown name returns `al` 0 with nothing stored.
//!
//! `d1210 (unit: eax) -> xmm0` and `d1410 (unit: eax) -> xmm0` are
//! plain-`ret` register helpers.

use super::{
    Ss, above, byte, call, cvtsi2ss, imm, lo, load, movaps, pop, push, ret, slot, ss,
    ucomiss_lahf_jp, unowned, zero, NONE, OBJECTS, ONE, TAGS, ZERO,
};
use guest_layout::datum::layout;
use crate::sim_objects::{Cx, Next, Result};
use runtime::{CpuAbi, Guest};

const ENTRY: u32 = 0x000c_6b90;
const D1210: u32 = 0x000d_1210;
const D1410: u32 = 0x000d_1410;
/// The weapon level's import handler.
const WEAPON_IMPORT: u32 = 0x000f_f5f0;
/// `d1410`'s callee: `(record: esi) -> xmm0`, plain `ret`.
const RECORD_RATIO: u32 = 0x001c_6e30;
/// A byte scale (f32) in the image's constants.
const BYTE_SCALE: u32 = 0x0045_dbb4;

/// `c6b90`'s name compares: the imm32 of each `cmp ecx, imm32` (at the
/// instruction address + 2), in address order.
const NAME_C6BB0: u32 = 0x000c_6bb2;
const NAME_C6BC5: u32 = 0x000c_6bc7;
const NAME_C6BCF: u32 = 0x000c_6bd1;
const NAME_C6BD7: u32 = 0x000c_6bd9;
const NAME_C6BDF: u32 = 0x000c_6be1;
const NAME_C6C2F: u32 = 0x000c_6c31;
const NAME_C6C37: u32 = 0x000c_6c39;
const NAME_C6C3F: u32 = 0x000c_6c41;
const NAME_C6CC6: u32 = 0x000c_6cc8;
const NAME_C6D27: u32 = 0x000c_6d29;
const NAME_C6D2F: u32 = 0x000c_6d31;
const NAME_C6D37: u32 = 0x000c_6d39;
const NAME_C6D41: u32 = 0x000c_6d43;
const NAME_C6D4A: u32 = 0x000c_6d4c;
/// The imm32 of `sub eax, imm32` at `c6cd2` and `c6cd9` (the address + 1).
const SUB_C6CD2: u32 = 0x000c_6cd3;
const SUB_C6CD9: u32 = 0x000c_6cda;
/// The imm32 of `mov esi, imm32` at `c6d52` and `c6d9a` (the address + 1):
/// the weapon's import names.
const WEAPON_NAME_C6D52: u32 = 0x000c_6d53;
const WEAPON_NAME_C6D9A: u32 = 0x000c_6d9b;
/// `d1410`'s compares: the imm32 of `cmp eax, imm32` at `d1444` and
/// `d145f` (the address + 1).
const KIND_D1444: u32 = 0x000d_1445;
const KIND_D145F: u32 = 0x000d_1460;

pub(in crate::sim_objects) const UNIT_SITES: [u32; 4] = [0xc6c26, 0xc6c2d, 0xc6cab, 0xc6dcd];
pub(in crate::sim_objects) const D1410_SITES: [u32; 2] = [0xd145d, 0xd1478];

/// `c6b90 (object: stack 0, name: stack 1, value: stack 2, active:
/// stack 3) -> al`; `ret 10`.
pub(in crate::sim_objects) fn unit(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        ENTRY => {
            c.eax = slot(c, g, 4)?;
            c.edx = g.u32(OBJECTS)?;
            c.edx = g.u32(c.edx.wrapping_add(layout::DATA))?;
            c.ecx = (c.eax & 0xffff) * 3;
            c.edx = g.u32(c.edx.wrapping_add(c.ecx * 4).wrapping_add(8))?;
            c.ecx = slot(c, g, 8)?;
            push(c, g, c.ebx)?;
            c.ebx = byte(c.ebx, 1);
            let name = c.ecx as i32;
            let n = imm(g, NAME_C6BB0)? as i32;
            if name > n {
                return unit_high(c, g);
            }
            if name == n {
                // `c6cbc`.
                load(c, 0, g.u32(c.edx.wrapping_add(0x1b8))?);
                return unit_clamp(c, g);
            }
            let n = imm(g, NAME_C6BC5)? as i32;
            if name > n {
                // `c6c2f`.
                if c.ecx == imm(g, NAME_C6C2F)? {
                    // `c6ca6`.
                    return call(c, D1410, UNIT_SITES[2]);
                }
                if c.ecx == imm(g, NAME_C6C37)? {
                    // `c6c55`.
                    c.edx = u32::from(g.u8(c.edx.wrapping_add(0x243))?);
                    let v = c.edx;
                    cvtsi2ss(c, 0, v)?;
                    return unit_scale(c, g);
                }
                if c.ecx != imm(g, NAME_C6C3F)? {
                    return unit_weapon(c, g);
                }
                load(c, 0, g.u32(c.edx.wrapping_add(0x208))?);
                return unit_clamp(c, g);
            }
            if name == n {
                // `c6c28`.
                return call(c, D1410, UNIT_SITES[1]);
            }
            if c.ecx == imm(g, NAME_C6BCF)? {
                // `c6c21`.
                return call(c, D1210, UNIT_SITES[0]);
            }
            if c.ecx == imm(g, NAME_C6BD7)? {
                // `c6c17`.
                load(c, 0, g.u32(c.edx.wrapping_add(0x2c4))?);
                return unit_clamp(c, g);
            }
            if c.ecx != imm(g, NAME_C6BDF)? {
                return unit_weapon(c, g);
            }
            // `c6beb`.
            let al = g.u8(c.edx.wrapping_add(0x10a))? >> 2;
            c.eax = byte(c.eax, al);
            if c.ebx as u8 & al != 0 {
                return unit_zero(c, g);
            }
            c.ecx = g.u32(c.edx.wrapping_add(0x134))? >> 0x12;
            if c.ebx as u8 & c.ecx as u8 != 0 {
                return unit_zero(c, g);
            }
            load(c, 0, g.u32(ONE)?);
            unit_store(c, g)
        }
        // After `d1210` and `d1410`: `jmp c6c68`.
        0xc6c26 | 0xc6c2d => unit_clamp(c, g),
        0xc6cab => {
            movaps(c, 1, 0);
            load(c, 0, g.u32(ONE)?);
            let b = lo(c, 1);
            ss(c, Ss::Sub, 0, b)?;
            unit_clamp(c, g)
        }
        0xc6dcd => unit_ret_esi(c, g),
        _ => Err(unowned(pc)),
    }
}

/// `c6cc6`: the names above `c6bb0`'s.
fn unit_high(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let name = c.ecx as i32;
    let n = imm(g, NAME_C6CC6)? as i32;
    if name > n {
        // `c6d27`.
        if c.ecx == imm(g, NAME_C6D27)? {
            // `c6d76`.
            if g.u32(c.edx.wrapping_add(0x24c))? == NONE {
                return unit_zero(c, g);
            }
            return unit_one(c, g);
        }
        if c.ecx == imm(g, NAME_C6D2F)? {
            // `c6d69`.
            load(c, 0, g.u32(c.edx.wrapping_add(0x264))?);
            return unit_clamp(c, g);
        }
        if c.ecx == imm(g, NAME_C6D37)? {
            // `c6d59`.
            c.ecx = u32::from(g.u8(c.edx.wrapping_add(0x1f8))?);
            let v = c.ecx;
            cvtsi2ss(c, 0, v)?;
            return unit_scale(c, g);
        }
        return unit_weapon(c, g);
    }
    if name == n {
        // `c6d0a`.
        if g.u32(c.edx.wrapping_add(0x248))? != NONE {
            return unit_one(c, g);
        }
        c.eax = g.u32(c.edx.wrapping_add(0x134))? >> 1;
        if c.eax & 1 != 0 {
            return unit_one(c, g);
        }
        return unit_zero(c, g);
    }
    // `c6cd0`.
    c.eax = c.ecx.wrapping_sub(imm(g, SUB_C6CD2)?);
    if c.eax == 0 {
        // `c6cfd`.
        load(c, 0, g.u32(c.edx.wrapping_add(0x2b0))?);
        return unit_clamp(c, g);
    }
    c.eax = c.eax.wrapping_sub(imm(g, SUB_C6CD9)?);
    if c.eax == 0 {
        // `c6cf0`.
        load(c, 0, g.u32(c.edx.wrapping_add(0x25c))?);
        return unit_clamp(c, g);
    }
    c.eax = c.eax.wrapping_sub(1);
    if c.eax != 0 {
        return unit_weapon(c, g);
    }
    load(c, 0, g.u32(c.edx.wrapping_add(0x260))?);
    unit_clamp(c, g)
}

/// `c6c60`: `mulss xmm0, [byte scale]`, then the clamp.
fn unit_scale(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    ss(c, Ss::Mul, 0, g.u32(BYTE_SCALE)?)?;
    unit_clamp(c, g)
}

/// `c6c68`: `xmm0` clamped to `[0, 1]`.
fn unit_clamp(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    zero(c, 1);
    if above(c, lo(c, 1), lo(c, 0))? {
        movaps(c, 0, 1);
        return unit_store(c, g);
    }
    load(c, 1, g.u32(ONE)?);
    if above(c, lo(c, 0), lo(c, 1))? {
        movaps(c, 0, 1);
    }
    unit_store(c, g)
}

/// `c6d1f`: zero.
fn unit_zero(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    zero(c, 0);
    unit_store(c, g)
}

/// `c6d7f`: one.
fn unit_one(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 0, g.u32(ONE)?);
    unit_store(c, g)
}

/// `c6c80`: store `xmm0` to the value out, active is "value > 0", `al` is
/// `bl`.
fn unit_store(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let positive = above(c, lo(c, 0), g.u32(ZERO)?)?;
    c.edx = slot(c, g, 0x10)?;
    g.set_u32(c.edx, lo(c, 0))?;
    c.ecx = slot(c, g, 0x14)?;
    c.eax = u32::from(positive);
    g.set_u8(c.ecx, c.eax as u8)?;
    c.eax = byte(c.eax, c.ebx as u8);
    c.ebx = pop(c, g)?;
    ret(c, 0x10)
}

/// `c6d3f`: the two names the held weapon answers, else `al` 0.
fn unit_weapon(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = byte(c.eax, 0);
    let first = c.ecx == imm(g, NAME_C6D41)?;
    push(c, g, c.esi)?;
    if first {
        // `c6d9a`.
        c.esi = imm(g, WEAPON_NAME_C6D9A)?;
    } else {
        if c.ecx != imm(g, NAME_C6D4A)? {
            return unit_ret_esi(c, g);
        }
        c.esi = imm(g, WEAPON_NAME_C6D52)?;
    }
    // `c6d9f`: `movsx cx, byte [edx+212]`, the held weapon's index.
    let cx = g.u8(c.edx.wrapping_add(0x212))? as i8 as i16 as u16;
    c.ecx = (c.ecx & 0xffff_0000) | u32::from(cx);
    if cx == 0xffff {
        return unit_ret_esi(c, g);
    }
    c.ecx = cx as i16 as i32 as u32;
    c.edx = g.u32(c.edx.wrapping_add(c.ecx.wrapping_mul(4)).wrapping_add(0x218))?;
    if c.edx == NONE {
        return unit_ret_esi(c, g);
    }
    c.eax = slot(c, g, 0x18)?;
    c.ecx = slot(c, g, 0x14)?;
    push(c, g, c.eax)?;
    push(c, g, c.ecx)?;
    push(c, g, c.esi)?;
    push(c, g, c.edx)?;
    call(c, WEAPON_IMPORT, UNIT_SITES[3])
}

/// `c6dcd`: `pop esi; pop ebx; ret 10`.
fn unit_ret_esi(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    ret(c, 0x10)
}

/// `d1210 (unit: eax) -> xmm0`; plain `ret`. No calls. Returns `0` unless
/// body `+334` > 0; then, with body `+338` zero, `+334 / def +1d4` (or one
/// when `def +1d4` is not above `+334`), else `(+334 - +338) / def +1dc`
/// when `+334 > +338` (otherwise `0`).
pub(in crate::sim_objects) fn d1210(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != D1210 {
        return Err(unowned(pc));
    }
    c.ecx = g.u32(OBJECTS)?;
    c.edx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
    zero(c, 0);
    c.eax = (c.eax & 0xffff) * 3;
    c.ecx = g.u32(c.edx.wrapping_add(c.eax * 4).wrapping_add(8))?;
    load(c, 1, g.u32(c.ecx.wrapping_add(0x334))?);
    if !above(c, lo(c, 1), lo(c, 0))? {
        return ret(c, 0);
    }
    c.eax = g.u32(c.ecx)?;
    c.edx = g.u32(TAGS)?;
    load(c, 1, g.u32(c.ecx.wrapping_add(0x338))?);
    c.eax = (c.eax & 0xffff) << 4;
    c.edx = g.u32(c.eax.wrapping_add(c.edx).wrapping_add(8))?;
    if !ucomiss_lahf_jp(c, lo(c, 1), lo(c, 0))? {
        // `[ecx+338]` is zero.
        load(c, 0, g.u32(c.edx.wrapping_add(0x1d4))?);
        let v = g.u32(c.ecx.wrapping_add(0x334))?;
        if !above(c, lo(c, 0), v)? {
            // `d127c`.
            load(c, 0, g.u32(ONE)?);
            return ret(c, 0);
        }
        load(c, 0, g.u32(c.ecx.wrapping_add(0x334))?);
        ss(c, Ss::Div, 0, g.u32(c.edx.wrapping_add(0x1d4))?)?;
        return ret(c, 0);
    }
    // `d1285`.
    load(c, 1, g.u32(c.ecx.wrapping_add(0x334))?);
    let v = g.u32(c.ecx.wrapping_add(0x338))?;
    if !above(c, lo(c, 1), v)? {
        return ret(c, 0);
    }
    movaps(c, 0, 1);
    ss(c, Ss::Sub, 0, g.u32(c.ecx.wrapping_add(0x338))?)?;
    ss(c, Ss::Div, 0, g.u32(c.edx.wrapping_add(0x1dc))?)?;
    ret(c, 0)
}

/// `d1410 (unit: eax) -> xmm0`; plain `ret`. Calls `1c6e30` with the
/// record in `esi` (pushed and kept across the call).
pub(in crate::sim_objects) fn d1410(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        D1410 => {
            load(c, 1, g.u32(ONE)?);
            c.ecx = NONE;
            movaps(c, 0, 1);
            if c.eax == c.ecx {
                return ret(c, 0);
            }
            c.edx = g.u32(OBJECTS)?;
            c.edx = g.u32(c.edx.wrapping_add(layout::DATA))?;
            c.eax = (c.eax & 0xffff) * 3;
            c.eax = g.u32(c.edx.wrapping_add(c.eax * 4).wrapping_add(8))?;
            push(c, g, c.esi)?;
            c.esi = (g.u16(c.eax.wrapping_add(0x12a))? as i16 as i32 as u32).wrapping_add(c.eax);
            c.eax = g.u32(c.esi.wrapping_add(0x7c))?;
            if c.eax == imm(g, KIND_D1444)? {
                let first = g.u32(c.esi)?;
                zero(c, 0);
                if first == c.ecx || g.u16(c.esi.wrapping_add(6))? == c.ecx as u16 {
                    return d1410_ret(c, g);
                }
                return call(c, RECORD_RATIO, D1410_SITES[0]);
            }
            // `d145f`.
            if c.eax != imm(g, KIND_D145F)? {
                return d1410_ret(c, g);
            }
            let first = g.u32(c.esi)?;
            zero(c, 0);
            if first == c.ecx || g.u16(c.esi.wrapping_add(6))? == c.ecx as u16 {
                // `d1480`: `xmm1` is still one.
                return d1410_complement(c, g);
            }
            call(c, RECORD_RATIO, D1410_SITES[1])
        }
        0xd145d => d1410_ret(c, g),
        0xd1478 => {
            load(c, 1, g.u32(ONE)?);
            d1410_complement(c, g)
        }
        _ => Err(unowned(pc)),
    }
}

/// `d1480`: `xmm0 = xmm1 - xmm0`, through `xmm1`.
fn d1410_complement(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    let b = lo(c, 0);
    ss(c, Ss::Sub, 1, b)?;
    movaps(c, 0, 1);
    d1410_ret(c, g)
}

/// `d1487`: `pop esi; ret`.
fn d1410_ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.esi = pop(c, g)?;
    ret(c, 0)
}
