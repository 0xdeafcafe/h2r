//! The biped level's import handler `de3b0` (slot `+4c`;
//! docs/engine/object-update.md, "Slice 3 bodies").
//!
//! `de3b0 (object, name, value out, active out) -> al` answers three
//! names: a body state byte compared with 2, a body flag byte, and the
//! body's `+88` vector length (x87, `fsqrt`) over a definition scale. The
//! value is clamped to `[0, 1]`, stored, and active is "value > 0"; `al` 1.
//! An unknown name, or a scale not above zero, returns `al` 0 with nothing
//! stored. No calls.

use super::{
    above, byte, fp_error, fpu, fpu_done, imm, lo, load, movaps, pop, push, ret, slot, unowned,
    zero, OBJECTS, ONE, TAGS,
};
use guest_layout::datum::layout;
use crate::sim_objects::{Cx, Next, Result};
use runtime::{CpuAbi, Guest};

const ENTRY: u32 = 0x000d_e3b0;

/// The imm32 of each `cmp esi, imm32` (the instruction address + 2).
const NAME_DE3EF: u32 = 0x000d_e3f1;
const NAME_DE3FA: u32 = 0x000d_e3fc;
const NAME_DE402: u32 = 0x000d_e404;

/// `de3b0 (object: stack 0, name: stack 1, value: stack 2, active:
/// stack 3) -> al`; `ret 10`.
pub(in crate::sim_objects) fn biped(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != ENTRY {
        return Err(unowned(pc));
    }
    c.eax = slot(c, g, 4)?;
    c.ecx = g.u32(OBJECTS)?;
    c.edx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
    zero(c, 2);
    load(c, 1, g.u32(ONE)?);
    c.eax = (c.eax & 0xffff) * 3;
    c.ecx = g.u32(c.edx.wrapping_add(c.eax * 4).wrapping_add(8))?;
    c.eax = g.u32(c.ecx)?;
    c.edx = g.u32(TAGS)?;
    c.eax = (c.eax & 0xffff) << 4;
    c.edx = g.u32(c.eax.wrapping_add(c.edx).wrapping_add(8))?;
    push(c, g, c.esi)?;
    c.esi = slot(c, g, 0x0c)?;
    c.eax = byte(c.eax, 0);
    movaps(c, 0, 2);
    if c.esi == imm(g, NAME_DE3EF)? {
        // `de471`.
        if g.u8(c.ecx.wrapping_add(0x3dc))? == 2 {
            movaps(c, 0, 1);
        }
        return clamp(c, g);
    }
    if c.esi == imm(g, NAME_DE3FA)? {
        // `de45b`.
        let dl = g.u8(c.ecx.wrapping_add(0x39d))?;
        c.edx = byte(c.edx, dl);
        if dl == 0 {
            return ret_esi(c, g);
        }
        load(c, 0, g.u32(ONE)?);
        c.eax = byte(c.eax, 1);
        // `de48e`.
        movaps(c, 1, 0);
        return store(c, g);
    }
    if c.esi != imm(g, NAME_DE402)? {
        return ret_esi(c, g);
    }
    load(c, 0, g.u32(c.edx.wrapping_add(0x2dc))?);
    if !above(c, lo(c, 0), lo(c, 2))? {
        return ret_esi(c, g);
    }
    // `de41f`: the length of body `+88` over definition `+2dc`.
    let mut x = fpu(c);
    x.fld(g.u32(c.ecx.wrapping_add(0x90))?).map_err(fp_error)?;
    x.fld(g.u32(c.ecx.wrapping_add(0x8c))?).map_err(fp_error)?;
    x.fld(g.u32(c.ecx.wrapping_add(0x88))?).map_err(fp_error)?;
    x.fld_st(0).map_err(fp_error)?;
    x.arith_st(x87::abi::arith_op::MUL, 1).map_err(fp_error)?;
    x.fld_st(2).map_err(fp_error)?;
    x.arith_st(x87::abi::arith_op::MUL, 3).map_err(fp_error)?;
    x.arith_reg(x87::abi::arith_op::ADD, 1, 0, true).map_err(fp_error)?;
    x.fld_st(3).map_err(fp_error)?;
    x.arith_st(x87::abi::arith_op::MUL, 4).map_err(fp_error)?;
    x.arith_reg(x87::abi::arith_op::ADD, 1, 0, true).map_err(fp_error)?;
    x.unary(x87::abi::unary_op::SQRT).map_err(fp_error)?;
    x.fstp_st(3).map_err(fp_error)?;
    x.fstp_st(0).map_err(fp_error)?;
    x.fstp_st(0).map_err(fp_error)?;
    x.arith_m(x87::abi::arith_op::DIV, g.u32(c.edx.wrapping_add(0x2dc))?)
        .map_err(fp_error)?;
    let v = x.fstp_m32().map_err(fp_error)?;
    let at = c.esp.wrapping_add(8);
    g.set_u32(at, v)?;
    fpu_done(c, g, x, 0x000d_e44f, at)?;
    load(c, 0, g.u32(at)?);
    clamp(c, g)
}

/// `de47d`: `xmm1` is `xmm0` clamped to `[0, 1]` (`xmm1` holds one,
/// `xmm2` zero); `al` 1.
fn clamp(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let below = above(c, lo(c, 2), lo(c, 0))?;
    c.eax = byte(c.eax, 1);
    if below {
        movaps(c, 1, 2);
    } else if !above(c, lo(c, 0), lo(c, 1))? {
        // `de48e`.
        movaps(c, 1, 0);
    }
    store(c, g)
}

/// `de491`: store `xmm1` to the value out; active is "`xmm0` > 0".
fn store(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let positive = above(c, lo(c, 0), lo(c, 2))?;
    c.ecx = slot(c, g, 0x10)?;
    g.set_u32(c.ecx, lo(c, 1))?;
    c.edx = slot(c, g, 0x14)?;
    c.ecx = u32::from(positive);
    g.set_u8(c.edx, c.ecx as u8)?;
    ret_esi(c, g)
}

/// `de4b5`: `pop esi; ret 10`.
fn ret_esi(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.esi = pop(c, g)?;
    ret(c, 0x10)
}
