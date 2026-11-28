//! The device level's import handler `106d70` (slot `+4c`;
//! docs/engine/object-update.md, "Slice 3 bodies").
//!
//! `106d70 (object, name, value out, active out) -> al` answers seven
//! names from the device's body (`+12c` flags, `+134`..`+164`, the
//! power group `+13c` in the array at `[4e0328]`, `+1cc` flags) and
//! definition (`+c0`, `+c8`, `+10c`), some through x87 `fabs`/`fdiv`.
//! Most values are clamped to `[0, 1]`; the value is stored and active is
//! "value > 0"; `al` is `bl`, 1. An unknown name returns `al` 0 with
//! nothing stored. No calls.

use super::{
    Ss, above, byte, comiss, cvtsi2ss, fp_error, fpu, fpu_done, imm, lo, load, movaps, pop, push,
    ret, slot, ss, ucomiss_lahf_jp, unowned, zero, OBJECTS, ONE, TAGS,
};
use guest_layout::datum::layout;
use crate::sim_objects::{Cx, Next, Result};
use runtime::{CpuAbi, Guest};

const ENTRY: u32 = 0x0010_6d70;

/// Game time: `+4` seconds per tick (f32).
const GAME_TIME: u32 = 0x0051_0c54;
/// The device groups' array global; data at `+44`, `0xc` bytes each.
const GROUPS: u32 = 0x004e_0328;
/// Two f32 constants in the image: the compare at `106e1c` and the
/// small-magnitude floor at `106f1f`/`106f4f`.
const THRESHOLD: u32 = 0x0045_dc84;
const EPSILON: u32 = 0x0045_dbdc;

/// The imm32 of each `cmp eax, imm32` (the instruction address + 1).
const NAME_106DAC: u32 = 0x0010_6dad;
const NAME_106DC3: u32 = 0x0010_6dc4;
const NAME_106DCA: u32 = 0x0010_6dcb;
const NAME_106DD1: u32 = 0x0010_6dd2;
const NAME_106EC8: u32 = 0x0010_6ec9;
const NAME_106ED3: u32 = 0x0010_6ed4;
const NAME_106EDE: u32 = 0x0010_6edf;

/// `106d70 (object: stack 0, name: stack 1, value: stack 2, active:
/// stack 3) -> al`; `ret 10`.
pub(in crate::sim_objects) fn device(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != ENTRY {
        return Err(unowned(pc));
    }
    c.eax = slot(c, g, 4)?;
    c.ecx = g.u32(OBJECTS)?;
    c.edx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
    zero(c, 1);
    load(c, 2, g.u32(ONE)?);
    c.eax = (c.eax & 0xffff) * 3;
    c.ecx = g.u32(c.edx.wrapping_add(c.eax * 4).wrapping_add(8))?;
    c.eax = g.u32(c.ecx)?;
    c.edx = g.u32(TAGS)?;
    c.eax = (c.eax & 0xffff) << 4;
    c.edx = g.u32(c.eax.wrapping_add(c.edx).wrapping_add(8))?;
    c.eax = slot(c, g, 8)?;
    push(c, g, c.ebx)?;
    c.ebx = byte(c.ebx, 1);
    movaps(c, 0, 1);
    let name = c.eax as i32;
    let n = imm(g, NAME_106DAC)? as i32;
    if name > n {
        return high(c, g);
    }
    if name == n {
        return powered(c, g);
    }
    if c.eax == imm(g, NAME_106DC3)? {
        // `106e1c`: 1 or 0 by `[threshold] < [ecx+140]`.
        load(c, 0, g.u32(THRESHOLD)?);
        let b = g.u32(c.ecx.wrapping_add(0x140))?;
        let below = comiss(c, lo(c, 0), b)? & 1 != 0;
        c.eax = u32::from(!below);
        let v = c.eax;
        cvtsi2ss(c, 0, v)?;
        return clamp(c, g);
    }
    if c.eax == imm(g, NAME_106DCA)? {
        // `106e0f`.
        load(c, 0, g.u32(c.ecx.wrapping_add(0x134))?);
        return clamp(c, g);
    }
    if c.eax != imm(g, NAME_106DD1)? {
        return unknown(c, g);
    }
    // `106ddc`: game seconds over definition `+10c`, times body `+148`.
    load(c, 3, g.u32(c.edx.wrapping_add(0x10c))?);
    if !above(c, lo(c, 3), lo(c, 1))? {
        return store(c, g);
    }
    c.eax = g.u32(GAME_TIME)?;
    c.ecx = g.u16(c.ecx.wrapping_add(0x148))? as i16 as i32 as u32;
    load(c, 0, g.u32(c.eax.wrapping_add(4))?);
    let b = lo(c, 3);
    ss(c, Ss::Div, 0, b)?;
    let i = c.ecx;
    cvtsi2ss(c, 3, i)?;
    let b = lo(c, 3);
    ss(c, Ss::Mul, 0, b)?;
    clamp(c, g)
}

/// `106e46`.
fn powered(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 3, g.u32(c.ecx.wrapping_add(0x134))?);
    if !ucomiss_lahf_jp(c, lo(c, 3), lo(c, 1))? {
        movaps(c, 0, 2);
    }
    // `106e5a`.
    if g.u8(c.ecx.wrapping_add(0xaa))? != 7 {
        return clamp(c, g);
    }
    c.eax = g.u32(c.ecx.wrapping_add(0x13c))?;
    if c.eax == u32::MAX {
        return clamp(c, g);
    }
    c.eax &= 0xffff;
    c.edx = c.eax * 3;
    c.eax = g.u32(GROUPS)?;
    c.eax = g.u32(c.eax.wrapping_add(layout::DATA))?;
    c.eax = c.eax.wrapping_add(c.edx * 4);
    c.edx = g.u32(c.ecx.wrapping_add(0x1cc))?;
    if c.edx & 0x0d != 0 {
        movaps(c, 0, 2);
    }
    let ax = g.u16(c.eax.wrapping_add(2))?;
    c.eax = (c.eax & 0xffff_0000) | u32::from(ax);
    if ax & 1 != 0 && ax & 2 != 0 {
        movaps(c, 0, 2);
    }
    // `106ea6`.
    load(c, 3, g.u32(c.ecx.wrapping_add(0x140))?);
    if ucomiss_lahf_jp(c, lo(c, 3), lo(c, 2))? && c.edx & 0x10 == 0 {
        return clamp(c, g);
    }
    // `106ec0`.
    movaps(c, 0, 1);
    store(c, g)
}

/// `106ec8`: the names above `106dac`'s.
fn high(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if c.eax == imm(g, NAME_106EC8)? {
        // `106fb4`.
        load(c, 0, g.u32(c.ecx.wrapping_add(0x140))?);
        return clamp(c, g);
    }
    if c.eax == imm(g, NAME_106ED3)? {
        // `106f75`: `|body +138| / definition +c0`, zero when either is zero.
        load(c, 3, g.u32(c.ecx.wrapping_add(0x138))?);
        if !ucomiss_lahf_jp(c, lo(c, 3), lo(c, 1))? {
            return store(c, g);
        }
        load(c, 3, g.u32(c.edx.wrapping_add(0xc0))?);
        let jp = ucomiss_lahf_jp(c, lo(c, 3), lo(c, 1))?;
        movaps(c, 0, 1);
        if !jp {
            return store(c, g);
        }
        let mut x = fpu(c);
        x.fld(g.u32(c.ecx.wrapping_add(0x138))?).map_err(fp_error)?;
        x.unary(x87::abi::unary_op::ABS).map_err(fp_error)?;
        x.arith_m(x87::abi::arith_op::DIV, g.u32(c.edx.wrapping_add(0xc0))?)
            .map_err(fp_error)?;
        let v = x.fstp_m32().map_err(fp_error)?;
        let at = c.esp.wrapping_add(8);
        g.set_u32(at, v)?;
        fpu_done(c, g, x, 0x0010_6fa8, at)?;
        load(c, 0, g.u32(at)?);
        return clamp(c, g);
    }
    if c.eax != imm(g, NAME_106EDE)? {
        return unknown(c, g);
    }
    // `106eeb`: `|body +144|` over a body or definition magnitude.
    load(c, 3, g.u32(c.ecx.wrapping_add(0x144))?);
    if !ucomiss_lahf_jp(c, lo(c, 3), lo(c, 1))? {
        return store(c, g);
    }
    c.eax = g.u32(c.ecx.wrapping_add(0x12c))?;
    movaps(c, 0, 1);
    if c.eax & 8 == 0 {
        // `106f47`: over `|definition +c8|`.
        let mut x = fpu(c);
        x.fld(g.u32(c.edx.wrapping_add(0xc8))?).map_err(fp_error)?;
        x.unary(x87::abi::unary_op::ABS).map_err(fp_error)?;
        x.fld(g.u32(EPSILON)?).map_err(fp_error)?;
        let f = x.fcomip(1).map_err(fp_error)?;
        x.fstp_st(0).map_err(fp_error)?;
        if f & 0x41 == 0 {
            fpu_done(c, g, x, 0x0010_6f57, EPSILON)?;
            return store(c, g);
        }
        x.fld(g.u32(c.ecx.wrapping_add(0x144))?).map_err(fp_error)?;
        x.unary(x87::abi::unary_op::ABS).map_err(fp_error)?;
        x.arith_m(x87::abi::arith_op::DIV, g.u32(c.edx.wrapping_add(0xc8))?)
            .map_err(fp_error)?;
        let v = x.fstp_m32().map_err(fp_error)?;
        let at = c.esp.wrapping_add(8);
        g.set_u32(at, v)?;
        fpu_done(c, g, x, 0x0010_6f69, at)?;
        load(c, 0, g.u32(at)?);
        return clamp(c, g);
    }
    if c.eax & 0x20 != 0 {
        return store(c, g);
    }
    // `106f15`: over `|body +164|`.
    let mut x = fpu(c);
    x.fld(g.u32(c.ecx.wrapping_add(0x164))?).map_err(fp_error)?;
    x.unary(x87::abi::unary_op::ABS).map_err(fp_error)?;
    x.fld_st(0).map_err(fp_error)?;
    x.fld(g.u32(EPSILON)?).map_err(fp_error)?;
    let f = x.fcomip(1).map_err(fp_error)?;
    x.fstp_st(0).map_err(fp_error)?;
    if f & 0x41 == 0 {
        // `106fd0`.
        x.fstp_st(0).map_err(fp_error)?;
        fpu_done(c, g, x, 0x0010_6fd0, EPSILON)?;
        return store(c, g);
    }
    x.fld(g.u32(c.ecx.wrapping_add(0x144))?).map_err(fp_error)?;
    x.unary(x87::abi::unary_op::ABS).map_err(fp_error)?;
    x.arith_st(x87::abi::arith_op::DIV, 1).map_err(fp_error)?;
    let v = x.fstp_m32().map_err(fp_error)?;
    let at = c.esp.wrapping_add(8);
    g.set_u32(at, v)?;
    load(c, 0, g.u32(at)?);
    x.fstp_st(0).map_err(fp_error)?;
    fpu_done(c, g, x, 0x0010_6f43, at)?;
    clamp(c, g)
}

/// `106ee5`: `al` 0, nothing stored.
fn unknown(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.eax = byte(c.eax, 0);
    c.ebx = pop(c, g)?;
    ret(c, 0x10)
}

/// `106fbc`: `xmm0` clamped to `[0, 1]` (`xmm1` zero, `xmm2` one).
fn clamp(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if above(c, lo(c, 1), lo(c, 0))? {
        movaps(c, 0, 1);
    } else if above(c, lo(c, 0), lo(c, 2))? {
        movaps(c, 0, 2);
    }
    store(c, g)
}

/// `106fd2`: store `xmm0`; active is "value > 0"; `al` is `bl`.
fn store(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let positive = above(c, lo(c, 0), lo(c, 1))?;
    c.ecx = slot(c, g, 0x10)?;
    g.set_u32(c.ecx, lo(c, 0))?;
    c.edx = slot(c, g, 0x14)?;
    c.eax = u32::from(positive);
    g.set_u8(c.edx, c.eax as u8)?;
    c.eax = byte(c.eax, c.ebx as u8);
    c.ebx = pop(c, g)?;
    ret(c, 0x10)
}
