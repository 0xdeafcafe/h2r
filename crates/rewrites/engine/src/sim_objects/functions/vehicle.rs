//! The vehicle import handler `f6d10` (docs/engine/object-update.md,
//! "Slice 3 bodies"): slot `+4c` of the `vehi` descriptor.
//!
//! `f6d10 (object: stack 0, name: stack 1, value: stack 2, active:
//! stack 3) -> al; ret 10` finds the name in a binary-search tree of
//! string IDs (signed compares, some leaves reached by `sub edi` chains
//! that leave `edi` zero) and computes the value from the vehicle body
//! (`esi`), its definition (`ebp`, the tag of body `+0`) and its model
//! data. Most values are a ratio from `f78b0 (x, y) -> xmm0` (`x / y`
//! when `|y|` is above a threshold, else zero) of a speed-like quantity
//! (`24590`, a vector's length, or x87 dot products of body vectors)
//! against a definition limit (`f78e0` by index, or a field). The
//! result is clamped to `[0, 1]` (`f785a`), stored through the value
//! pointer, and the active flag is "value above zero, or `bl` set"; an
//! unknown name returns `al = 0` and stores nothing.
//!
//! Frame (`E` the entry `esp`): `E-18`..`E-1` locals (three cross-product
//! components at `E-18`, `E-14`, `E-10`; a vector difference at `E-18`);
//! the object's and the name's argument slots `E+4` and `E+8` are reused
//! as scratch floats once read.

use super::{
    Ss, above, byte, call, comiss, fp_error, fpu, fpu_done, imm, lo, load, movaps, pop, push,
    ret, slot, ss, unowned, zero, OBJECTS, ONE, TAGS,
};
use guest_layout::datum::layout;
use crate::light_constants::{Error, X87};
use crate::sim_objects::{Cx, Next, Result};
use runtime::{CpuAbi, Guest};
use x87::abi::{arith_op, unary_op};

const ENTRY: u32 = 0x000f_6d10;

/// Unit function value `(unit: eax) -> xmm0`, plain `ret` (a slice 3 body).
const D1210: u32 = 0x000d_1210;
/// `(vehicle: eax) -> eax` (0 or 1), plain `ret`.
const F5D70: u32 = 0x000f_5d70;
/// Unit flag test `(object: eax, bit: ecx) -> eax` (0 or 1), plain `ret`.
const E6FE0: u32 = 0x000e_6fe0;
/// A limit from the object's definition by index `(object: eax, index:
/// stack 0) -> xmm0` (`|+1f4|`, `|+1f8|`, their max, `|+22c|`, ...: a jump
/// table), `ret 4`.
const AXIS: u32 = 0x000f_78e0;
/// `(x: stack 0, y: stack 1) -> xmm0`: `x / y` when `|y|` is above a
/// threshold, else zero; `ret 8`.
const RATIO: u32 = 0x000f_78b0;
/// Vector length `(vector: eax) -> st(0)`, plain `ret`.
const MAGNITUDE: u32 = 0x0002_4590;
/// Flag word bit test `(bit: cl, word pointer: stack 0) -> al`, `ret 4`.
const FLAG: u32 = 0x000f_8070;
/// Vector difference `(out: esi, a: edx, b: ecx, eax)`, plain `ret`.
const DIFFERENCE: u32 = 0x0011_d580;

/// f32 constants in the image's `.rdata`.
const K_45DBBC: u32 = 0x0045_dbbc;
const K_45DC54: u32 = 0x0045_dc54;
const K_45DC5C: u32 = 0x0045_dc5c;
const K_45DC70: u32 = 0x0045_dc70;
const K_4407B8: u32 = 0x0044_07b8;

/// The imm32 of `push imm` at `f75fb` and `f77ae` (f32 arguments of
/// `f78b0`).
const PUSH_F75FB: u32 = 0x000f_75fc;
const PUSH_F77AE: u32 = 0x000f_77af;

// The string IDs of the name tree: each is the imm32 of the `cmp edi,
// imm32` (or `sub edi, imm32`) at the address in its name, two bytes in;
// the `sub edi, imm8` steps are the imm8 two bytes in.
const NAME_F6D4F: u32 = 0x000f_6d51;
const NAME_F6D64: u32 = 0x000f_6d66;
const NAME_F6D76: u32 = 0x000f_6d78;
const NAME_F6D88: u32 = 0x000f_6d8a;
const NAME_F6D92: u32 = 0x000f_6d94;
const NAME_F6D9A: u32 = 0x000f_6d9c;
const NAME_F6E01: u32 = 0x000f_6e03;
const NAME_F6E09: u32 = 0x000f_6e0b;
const NAME_F6E56: u32 = 0x000f_6e58;
/// `sub edi, imm32`.
const NAME_F6E64: u32 = 0x000f_6e66;
/// `sub edi, imm8`.
const STEP_F6E6C: u32 = 0x000f_6e6e;
const NAME_F6EEA: u32 = 0x000f_6eec;
const NAME_F6F57: u32 = 0x000f_6f59;
const NAME_F6F69: u32 = 0x000f_6f6b;
const NAME_F6F7B: u32 = 0x000f_6f7d;
const NAME_F6F87: u32 = 0x000f_6f89;
const NAME_F708F: u32 = 0x000f_7091;
/// The second compare against `f708f`'s ID, at `f71c0`.
const NAME_F71C0: u32 = 0x000f_71c2;
const NAME_F721A: u32 = 0x000f_721c;
/// `sub edi, imm32`.
const NAME_F722C: u32 = 0x000f_722e;
/// `sub edi, imm8`.
const STEP_F7234: u32 = 0x000f_7236;
const NAME_F731E: u32 = 0x000f_7320;
const NAME_F73E8: u32 = 0x000f_73ea;
const NAME_F73FA: u32 = 0x000f_73fc;
const NAME_F740C: u32 = 0x000f_740e;
/// `sub edi, imm32`.
const NAME_F741E: u32 = 0x000f_7420;
/// `sub edi, imm8`.
const STEP_F7426: u32 = 0x000f_7428;
const NAME_F756A: u32 = 0x000f_756c;
const NAME_F761F: u32 = 0x000f_7621;
/// `sub edi, imm32`.
const NAME_F7631: u32 = 0x000f_7633;
/// `sub edi, imm8`.
const STEP_F7639: u32 = 0x000f_763b;
const NAME_F7732: u32 = 0x000f_7734;
const NAME_F776B: u32 = 0x000f_776d;
const NAME_F7779: u32 = 0x000f_777b;
const NAME_F7787: u32 = 0x000f_7789;
const NAME_F778F: u32 = 0x000f_7791;
const NAME_F77F6: u32 = 0x000f_77f8;
const NAME_F780A: u32 = 0x000f_780c;
/// `sub edi, imm32`.
const NAME_F7814: u32 = 0x000f_7816;
/// `sub edi, imm8`.
const STEP_F781C: u32 = 0x000f_781e;
const NAME_F7821: u32 = 0x000f_7823;

pub(in crate::sim_objects) const VEHICLE_SITES: [u32; 34] = [
    0xf6dc5, 0xf6dd1, 0xf6e21, 0xf6f2d, 0xf7004, 0xf701a, 0xf707e, 0xf7087, 0xf70c6, 0xf70e1,
    0xf710a, 0xf7133, 0xf713f, 0xf7156, 0xf71bd, 0xf720a, 0xf7288, 0xf7491, 0xf749a, 0xf7559,
    0xf7562, 0xf75fb, 0xf7607, 0xf7610, 0xf76ae, 0xf770a, 0xf7721, 0xf775a, 0xf77bc, 0xf77cd,
    0xf77d6, 0xf77f1, 0xf783e, 0xf7857,
];

/// `f6d10 (object: stack 0, name: stack 1, value: stack 2, active: stack 3)
/// -> al`, `ret 10`.
///
/// Across its calls it keeps `bl` (the forced-active byte), `esi` (the
/// body), `ebp` (the definition, or a body vector on the `f709b` path),
/// `edi` (the name, or zero after a `sub` chain) and `ebx` (the handle on
/// the `f709b` path); `eax` and `ecx` survive `f78b0`, which writes only
/// `xmm0` and the x87 stack.
pub(in crate::sim_objects) fn vehicle(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        ENTRY => {
            prologue(c, g)?;
            dispatch(c, g)
        }
        0xf6dc5 => take(c, g),
        0xf6dd1 => {
            // `test al,al; jne f6e25`.
            if c.eax as u8 != 0 {
                return one(c, g);
            }
            if g.u8(c.esi.wrapping_add(0x148))? & 2 != 0 {
                return one(c, g);
            }
            let dl = g.u8(c.esi.wrapping_add(0x348))? >> 5;
            c.edx = byte(c.edx, dl);
            if dl & 1 != 0 {
                return one(c, g);
            }
            nothing(c, g)
        }
        0xf6e21 => {
            if c.eax as u8 == 0 {
                return nothing(c, g);
            }
            one(c, g)
        }
        0xf6f2d => {
            fld(c, g, 0xf6f2d, c.esi.wrapping_add(0x90))?;
            fop(c, g, 0xf6f33, arith_op::MUL, c.esi.wrapping_add(0x84))?;
            fld(c, g, 0xf6f39, c.esi.wrapping_add(0x8c))?;
            fop(c, g, 0xf6f3f, arith_op::MUL, c.esi.wrapping_add(0x80))?;
            fopp(c, g, 0xf6f45, arith_op::ADD)?;
            fld(c, g, 0xf6f47, c.esi.wrapping_add(0x7c))?;
            fop(c, g, 0xf6f4a, arith_op::MUL, c.esi.wrapping_add(0x88))?;
            fopp(c, g, 0xf6f50, arith_op::ADD)?;
            abs_ratio(c, g)
        }
        0xf7004 => {
            c.eax = g.u32(c.ebp.wrapping_add(0x264))?;
            c.ecx = g.u32(c.esi.wrapping_add(0x378))?;
            push(c, g, c.eax)?;
            push(c, g, c.ecx)?;
            movaps(c, 1, 0);
            call(c, RATIO, 0xf701a)
        }
        0xf701a => {
            ss(c, Ss::Add, 1, lo(c, 0))?;
            ss(c, Ss::Mul, 1, g.u32(K_45DBBC)?)?;
            clamp(c, g)
        }
        0xf707e => magnitude_ratio(c, g, 0xf707f, 0xf7087),
        0xf7087 | 0xf749a | 0xf7562 | 0xf77f1 => {
            zero(c, 2);
            take(c, g)
        }
        0xf70c6 => {
            let al = c.eax as u8;
            c.ebp = c.esi.wrapping_add(0x3f0);
            if al == 0 {
                c.ebp = c.esi.wrapping_add(0x88);
            }
            push(c, g, c.ebx)?;
            c.ecx = 7;
            call(c, FLAG, 0xf70e1)
        }
        0xf70e1 => {
            let at = if c.eax as u8 != 0 { 0x3dc } else { 0x354 };
            load(c, 0, g.u32(c.esi.wrapping_add(at))?);
            c.ebx = slot(c, g, 0x2c)?;
            push(c, g, 2)?;
            c.eax = c.ebx;
            g.set_u32(c.esp.wrapping_add(0x34), lo(c, 0))?;
            call(c, AXIS, 0xf710a)
        }
        0xf710a => {
            fld(c, g, 0xf710a, c.esi.wrapping_add(0x78))?;
            fop(c, g, 0xf710d, arith_op::MUL, c.ebp.wrapping_add(8))?;
            c.esp = c.esp.wrapping_sub(8);
            fld(c, g, 0xf7113, c.esi.wrapping_add(0x74))?;
            g.set_u32(c.esp.wrapping_add(4), lo(c, 0))?;
            fop(c, g, 0xf711c, arith_op::MUL, c.ebp.wrapping_add(4))?;
            fopp(c, g, 0xf711f, arith_op::ADD)?;
            fld(c, g, 0xf7121, c.esi.wrapping_add(0x70))?;
            fop(c, g, 0xf7124, arith_op::MUL, c.ebp)?;
            fopp(c, g, 0xf7127, arith_op::ADD)?;
            fabs(c, g, 0xf7129)?;
            fstp(c, g, 0xf712b, c.esp)?;
            call(c, RATIO, 0xf7133)
        }
        0xf7133 => {
            push(c, g, 0)?;
            c.eax = c.ebx;
            movaps(c, 3, 0);
            call(c, AXIS, 0xf713f)
        }
        0xf713f => {
            fld(c, g, 0xf713f, c.esp.wrapping_add(0x30))?;
            c.esp = c.esp.wrapping_sub(8);
            fabs(c, g, 0xf7146)?;
            g.set_u32(c.esp.wrapping_add(4), lo(c, 0))?;
            fstp(c, g, 0xf714e, c.esp)?;
            call(c, RATIO, 0xf7156)
        }
        0xf7156 => blend(c, g),
        0xf71bd => {
            movaps(c, 1, 0);
            gate(c, g)
        }
        0xf720a => {
            push(c, g, c.ecx)?;
            c.ecx = slot(c, g, 0x30)?;
            g.set_u32(c.esp, lo(c, 0))?;
            push(c, g, c.ecx)?;
            call(c, RATIO, 0xf7857)
        }
        0xf7288 => {
            fld(c, g, 0xf7288, c.esp.wrapping_add(0x18))?;
            fop(c, g, 0xf728c, arith_op::MUL, c.esi.wrapping_add(0x90))?;
            fld(c, g, 0xf7292, c.esp.wrapping_add(0x14))?;
            fop(c, g, 0xf7296, arith_op::MUL, c.esi.wrapping_add(0x8c))?;
            fopp(c, g, 0xf729c, arith_op::ADD)?;
            fld(c, g, 0xf729e, c.esp.wrapping_add(0x10))?;
            fop(c, g, 0xf72a2, arith_op::MUL, c.esi.wrapping_add(0x88))?;
            fopp(c, g, 0xf72a8, arith_op::ADD)?;
            abs_ratio(c, g)
        }
        0xf7491 => magnitude_ratio(c, g, 0xf7492, 0xf749a),
        0xf7559 => magnitude_ratio(c, g, 0xf755a, 0xf7562),
        0xf75fb => {
            let v = imm(g, PUSH_F75FB)?;
            push(c, g, v)?;
            c.eax = c.esi;
            call(c, MAGNITUDE, 0xf7607)
        }
        0xf7607 => magnitude_ratio(c, g, 0xf7608, 0xf7610),
        0xf7610 => {
            zero(c, 2);
            movaps(c, 1, 0);
            ss(c, Ss::Mul, 1, lo(c, 0))?;
            clamp(c, g)
        }
        0xf76ae => {
            c.edx = (g.u32(c.esi)? & 0xffff) << 4;
            c.eax = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(8))?;
            fld(c, g, 0xf76bd, c.eax.wrapping_add(0x22c))?;
            movaps(c, 1, 0);
            wider_rest(c, g, 0xf76c6, 0x230)?;
            // `f76ee`.
            fld(c, g, 0xf76ee, c.esi.wrapping_add(0x358))?;
            g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 0))?;
            c.eax = slot(c, g, 0x2c)?;
            fabs(c, g, 0xf76fe)?;
            push(c, g, c.eax)?;
            push(c, g, c.ecx)?;
            fstp(c, g, 0xf7702, c.esp)?;
            call(c, RATIO, 0xf770a)
        }
        0xf770a => {
            if above(c, lo(c, 1), lo(c, 0))? {
                return clamp(c, g);
            }
            take(c, g)
        }
        0xf7721 => {
            fld(c, g, 0xf7721, c.esi.wrapping_add(0x354))?;
            fop(c, g, 0xf7727, arith_op::SUB, c.esi.wrapping_add(0x35c))?;
            abs_ratio(c, g)
        }
        0xf775a => {
            fld(c, g, 0xf775a, c.esi.wrapping_add(0x35c))?;
            fop(c, g, 0xf7760, arith_op::ADD, c.esi.wrapping_add(0x354))?;
            abs_ratio(c, g)
        }
        0xf77bc => {
            push(c, g, c.ecx)?;
            c.eax = c.esi.wrapping_add(0x88);
            g.set_u32(c.esp, lo(c, 0))?;
            call(c, MAGNITUDE, 0xf77cd)
        }
        0xf77cd => magnitude_ratio(c, g, 0xf77ce, 0xf77d6),
        0xf77d6 => {
            ss(c, Ss::Mul, 0, g.u32(c.esi.wrapping_add(0x374))?)?;
            ss(c, Ss::Sub, 0, g.u32(K_4407B8)?)?;
            push(c, g, c.ecx)?;
            g.set_u32(c.esp, lo(c, 0))?;
            call(c, RATIO, 0xf77f1)
        }
        0xf783e => {
            fld(c, g, 0xf783e, c.esi.wrapping_add(0x354))?;
            abs_ratio(c, g)
        }
        0xf7857 => take(c, g),
        _ => Err(unowned(pc)),
    }
}

/// `f6d10..f6d55`: the frame, the body (`esi`), the definition (`ebp`),
/// the name (`edi`), `bl = 0`, `xmm1 = xmm2 = 0`; `eax` the tag entries,
/// `ecx` the definition's tag index `<< 4`, `edx` the handle.
fn prologue(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.edx = slot(c, g, 4)?;
    c.ecx = g.u32(OBJECTS)?;
    c.ecx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
    zero(c, 2);
    c.esp = c.esp.wrapping_sub(0x18);
    push(c, g, c.ebx)?;
    c.eax = c.edx & 0xffff;
    push(c, g, c.ebp)?;
    c.eax = c.eax.wrapping_mul(3);
    push(c, g, c.esi)?;
    c.esi = g.u32(c.ecx.wrapping_add(c.eax.wrapping_mul(4)).wrapping_add(8))?;
    c.ecx = g.u32(c.esi)?;
    c.eax = g.u32(TAGS)?;
    c.ecx &= 0xffff;
    push(c, g, c.edi)?;
    c.edi = slot(c, g, 0x30)?;
    c.ecx <<= 4;
    c.ebp = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    c.ebx = byte(c.ebx, 0);
    movaps(c, 1, 2);
    Ok(())
}

/// A string ID of the tree, for the signed compares.
fn id(g: &Guest, at: u32) -> Result<i32> {
    Ok(imm(g, at)? as i32)
}

/// `sub edi, imm8`'s sign-extended immediate.
fn imm8(g: &Guest, at: u32) -> Result<u32> {
    Ok(g.u8(at)? as i8 as i32 as u32)
}

/// `f6d4f`: the root of the name tree.
fn dispatch(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.edi as i32;
    let k = id(g, NAME_F6D4F)?;
    if n > k {
        return dispatch_f73e8(c, g);
    }
    if n == k {
        return case_f7380(c, g);
    }
    let k = id(g, NAME_F6D64)?;
    if n > k {
        return dispatch_f6f57(c, g);
    }
    if n == k {
        return case_f6f24(c, g);
    }
    let k = id(g, NAME_F6D76)?;
    if n > k {
        return dispatch_f6e56(c, g);
    }
    if n == k {
        return case_f6e3f(c, g);
    }
    let k = id(g, NAME_F6D88)?;
    if n > k {
        // `f6e01`.
        if n == id(g, NAME_F6E01)? {
            return case_f6e32(c, g);
        }
        if n == id(g, NAME_F6E09)? {
            return case_f6e15(c, g);
        }
        return miss(c, g);
    }
    if n == k {
        return case_f6df4(c, g);
    }
    if n == id(g, NAME_F6D92)? {
        return case_f6dca(c, g);
    }
    if n == id(g, NAME_F6D9A)? {
        return case_f6da6(c, g);
    }
    miss(c, g)
}

/// `f6e56`.
fn dispatch_f6e56(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.edi as i32;
    let k = id(g, NAME_F6E56)?;
    if n > k {
        // `f6eea`.
        if n == id(g, NAME_F6EEA)? {
            return case_f6ef6(c, g);
        }
        return miss(c, g);
    }
    if n == k {
        return case_f6ed7(c, g);
    }
    c.edi = c.edi.wrapping_sub(imm(g, NAME_F6E64)?);
    if c.edi == 0 {
        return case_f6ea3(c, g);
    }
    c.edi = c.edi.wrapping_sub(imm8(g, STEP_F6E6C)?);
    if c.edi == 0 {
        return case_f6e75(c, g);
    }
    miss(c, g)
}

/// `f6f57`.
fn dispatch_f6f57(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.edi as i32;
    let k = id(g, NAME_F6F57)?;
    if n > k {
        return dispatch_f721a(c, g);
    }
    if n == k {
        return case_f71e2(c, g);
    }
    let k = id(g, NAME_F6F69)?;
    if n > k {
        // `f708f`.
        if n == id(g, NAME_F708F)? {
            return case_f709b(c, g);
        }
        return miss(c, g);
    }
    if n == k {
        return case_f702b(c, g);
    }
    if n == id(g, NAME_F6F7B)? {
        return case_f709b(c, g);
    }
    if n == id(g, NAME_F6F87)? {
        return case_f6f93(c, g);
    }
    miss(c, g)
}

/// `f721a`.
fn dispatch_f721a(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.edi as i32;
    let k = id(g, NAME_F721A)?;
    if n > k {
        // `f731e`.
        if n == id(g, NAME_F731E)? {
            return case_f732a(c, g);
        }
        return miss(c, g);
    }
    if n == k {
        return case_f7311(c, g);
    }
    c.edi = c.edi.wrapping_sub(imm(g, NAME_F722C)?);
    if c.edi == 0 {
        return case_f72af(c, g);
    }
    c.edi = c.edi.wrapping_sub(imm8(g, STEP_F7234)?);
    if c.edi == 0 {
        return case_f723d(c, g);
    }
    miss(c, g)
}

/// `f73e8`.
fn dispatch_f73e8(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.edi as i32;
    let k = id(g, NAME_F73E8)?;
    if n > k {
        return dispatch_f776b(c, g);
    }
    if n == k {
        return case_f7751(c, g);
    }
    let k = id(g, NAME_F73FA)?;
    if n > k {
        return dispatch_f761f(c, g);
    }
    if n == k {
        return case_f75e5(c, g);
    }
    let k = id(g, NAME_F740C)?;
    if n > k {
        // `f756a`.
        if n == id(g, NAME_F756A)? {
            return case_f7576(c, g);
        }
        return miss(c, g);
    }
    if n == k {
        return case_f74f8(c, g);
    }
    c.edi = c.edi.wrapping_sub(imm(g, NAME_F741E)?);
    if c.edi == 0 {
        return case_f74a2(c, g);
    }
    c.edi = c.edi.wrapping_sub(imm8(g, STEP_F7426)?);
    if c.edi == 0 {
        return case_f742f(c, g);
    }
    miss(c, g)
}

/// `f761f`.
fn dispatch_f761f(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.edi as i32;
    let k = id(g, NAME_F761F)?;
    if n > k {
        // `f7732`.
        if n == id(g, NAME_F7732)? {
            return case_f773e(c, g);
        }
        return miss(c, g);
    }
    if n == k {
        return case_f7718(c, g);
    }
    c.edi = c.edi.wrapping_sub(imm(g, NAME_F7631)?);
    if c.edi == 0 {
        return case_f7655(c, g);
    }
    c.edi = c.edi.wrapping_sub(imm8(g, STEP_F7639)?);
    if c.edi == 0 {
        return case_f7642(c, g);
    }
    miss(c, g)
}

/// `f776b`.
fn dispatch_f776b(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.edi as i32;
    let k = id(g, NAME_F776B)?;
    if n > k {
        // `f780a`.
        let k = id(g, NAME_F780A)?;
        if n > k {
            // `f7821`.
            if n == id(g, NAME_F7821)? {
                return case_f7835(c, g);
            }
            return miss(c, g);
        }
        if n == k {
            return case_f779b(c, g);
        }
        c.edi = c.edi.wrapping_sub(imm(g, NAME_F7814)?);
        if c.edi == 0 {
            return case_f7835(c, g);
        }
        c.edi = c.edi.wrapping_sub(imm8(g, STEP_F781C)?);
        if c.edi == 0 {
            return case_f7835(c, g);
        }
        return miss(c, g);
    }
    if n == k {
        return case_f779b(c, g);
    }
    let k = id(g, NAME_F7779)?;
    if n > k {
        // `f77f6`; its miss is `f77fe`, the same code as `f7829`.
        if n == id(g, NAME_F77F6)? {
            return case_f779b(c, g);
        }
        return miss(c, g);
    }
    if n == k {
        return case_f7835(c, g);
    }
    if n == id(g, NAME_F7787)? {
        return case_f77ae(c, g);
    }
    if n == id(g, NAME_F778F)? {
        return case_f779b(c, g);
    }
    miss(c, g)
}

/// `f6da6`: one when body `+148` bit 12 is set, else the unit value
/// (`d1210`).
fn case_f6da6(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(c.esi.wrapping_add(0x148))?;
    if c.eax & 0x1000 != 0 {
        load(c, 1, g.u32(ONE)?);
        return finish(c, g);
    }
    c.eax = c.edx;
    call(c, D1210, 0xf6dc5)
}

/// `f6dca`: one when `f5d70` answers, body `+148` bit 1, or body `+348`
/// bit 5; else zero (the rest at site `f6dd1`).
fn case_f6dca(c: &mut CpuAbi, _g: &mut Guest) -> Result<Next> {
    c.eax = c.edx;
    call(c, F5D70, 0xf6dd1)
}

/// `f6df4`: body `+370`.
fn case_f6df4(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 1, g.u32(c.esi.wrapping_add(0x370))?);
    clamp(c, g)
}

/// `f6e15`: one when `e6fe0` bit `32h` is set, else zero.
fn case_f6e15(c: &mut CpuAbi, _g: &mut Guest) -> Result<Next> {
    c.ecx = 0x32;
    c.eax = c.edx;
    call(c, E6FE0, 0xf6e21)
}

/// `f6e32`: body `+374`.
fn case_f6e32(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 1, g.u32(c.esi.wrapping_add(0x374))?);
    clamp(c, g)
}

/// `mov r,[esi]; and r,ffff; shl r,4`: the body's tag index `<< 4`.
fn body_tag(c: &CpuAbi, g: &Guest) -> Result<u32> {
    Ok((g.u32(c.esi)? & 0xffff) << 4)
}

/// `f6e3f`: `|model +204 * k| / |body +35c|`.
fn case_f6e3f(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = body_tag(c, g)?;
    c.eax = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    fld(c, g, 0xf6e4e, c.eax.wrapping_add(0x204))?;
    scaled_f6eb8(c, g)
}

/// `f6ea3`: `|model +208 * k| / |body +35c|`.
fn case_f6ea3(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = body_tag(c, g)?;
    c.eax = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    fld(c, g, 0xf6eb2, c.eax.wrapping_add(0x208))?;
    scaled_f6eb8(c, g)
}

/// `f6eb8`.
fn scaled_f6eb8(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    fop(c, g, 0xf6eb8, arith_op::MUL, K_45DC54)?;
    fabs(c, g, 0xf6ebe)?;
    fstp(c, g, 0xf6ec0, c.esp.wrapping_add(0x2c))?;
    c.edx = slot(c, g, 0x2c)?;
    fld(c, g, 0xf6ec8, c.esi.wrapping_add(0x35c))?;
    push(c, g, c.edx)?;
    fabs(c, g, 0xf6ecf)?;
    push(c, g, c.ecx)?;
    ratio_f784f(c, g)
}

/// `f6e75`: `|model +22c| / |body +358|`.
fn case_f6e75(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = body_tag(c, g)?;
    c.eax = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    fld(c, g, 0xf6e84, c.eax.wrapping_add(0x22c))?;
    fabs(c, g, 0xf6e8a)?;
    fstp(c, g, 0xf6e8c, c.esp.wrapping_add(0x2c))?;
    c.edx = slot(c, g, 0x2c)?;
    fld(c, g, 0xf6e94, c.esi.wrapping_add(0x358))?;
    push(c, g, c.edx)?;
    fabs(c, g, 0xf6e9b)?;
    push(c, g, c.ecx)?;
    ratio_f784f(c, g)
}

/// `f6ed7`: `body +378 / definition +264`.
fn case_f6ed7(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(c.ebp.wrapping_add(0x264))?;
    c.ecx = g.u32(c.esi.wrapping_add(0x378))?;
    push(c, g, c.eax)?;
    push(c, g, c.ecx)?;
    call(c, RATIO, 0xf7857)
}

/// `f6ef6`: `|model +230| / |body +358|`.
fn case_f6ef6(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = body_tag(c, g)?;
    c.eax = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(8))?;
    fld(c, g, 0xf6f05, c.eax.wrapping_add(0x230))?;
    fabs(c, g, 0xf6f0b)?;
    fstp(c, g, 0xf6f0d, c.esp.wrapping_add(0x2c))?;
    c.eax = slot(c, g, 0x2c)?;
    fld(c, g, 0xf6f15, c.esi.wrapping_add(0x358))?;
    push(c, g, c.eax)?;
    fabs(c, g, 0xf6f1c)?;
    push(c, g, c.ecx)?;
    ratio_f784f(c, g)
}

/// `f6f24`: `|body +7c . body +88|` over limit 2 (`f78e0`), site `f6f2d`.
fn case_f6f24(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, 2)?;
    c.eax = c.edx;
    call(c, AXIS, 0xf6f2d)
}

/// `f6f93`: zero when body `+38b` is `fe` or `ff`; else the mean of two
/// ratios (sites `f7004`, `f701a`).
fn case_f6f93(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let cl = g.u8(c.esi.wrapping_add(0x38b))?;
    c.ecx = byte(c.ecx, cl);
    if cl == 0xfe || cl == 0xff {
        return finish(c, g);
    }
    c.ecx = body_tag(c, g)?;
    c.eax = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    wider(c, g, 0xf6fba, 0xf6fc0, 0x22c, 0x230)?;
    // `f6fe8`.
    fld(c, g, 0xf6fe8, c.esi.wrapping_add(0x358))?;
    g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 0))?;
    c.edx = slot(c, g, 0x2c)?;
    fabs(c, g, 0xf6ff8)?;
    push(c, g, c.edx)?;
    push(c, g, c.ecx)?;
    fstp(c, g, 0xf6ffc, c.esp)?;
    call(c, RATIO, 0xf7004)
}

/// `f702b`: `|body +88| / max(|model +1f4|, |model +1f8|)`.
fn case_f702b(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = body_tag(c, g)?;
    c.eax = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(8))?;
    wider(c, g, 0xf703a, 0xf7040, 0x1f4, 0x1f8)?;
    // `f7068`.
    g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 0))?;
    c.eax = slot(c, g, 0x2c)?;
    push(c, g, c.eax)?;
    c.eax = c.esi.wrapping_add(0x88);
    call(c, MAGNITUDE, 0xf707e)
}

/// `f709b`: with definition `+1f0` (word) 0 or 1, `body +378 /
/// definition +264` (`f71aa`); otherwise a blend of two ratios by body
/// `+34c`, the vector and limit picked by body `+348` bit 7 (`f8070`).
fn case_f709b(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let w = g.u16(c.ebp.wrapping_add(0x1f0))?;
    c.eax = (c.eax & 0xffff_0000) | u32::from(w);
    if w == 1 || w == 0 {
        // `f71aa`.
        c.edx = g.u32(c.ebp.wrapping_add(0x264))?;
        c.eax = g.u32(c.esi.wrapping_add(0x378))?;
        push(c, g, c.edx)?;
        push(c, g, c.eax)?;
        return call(c, RATIO, 0xf71bd);
    }
    c.ebx = c.esi.wrapping_add(0x348);
    push(c, g, c.ebx)?;
    c.ecx = 7;
    call(c, FLAG, 0xf70c6)
}

/// `f7156`: the blend factor `clamp((body +34c * k + 1) * k2, 0, 1)` mixes
/// the two ratios (`xmm3` the first, `xmm0` the second).
fn blend(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = u32::from(g.u8(c.esi.wrapping_add(0x34c))?);
    load(c, 4, g.u32(ONE)?);
    zero(c, 1);
    super::cvtsi2ss(c, 2, c.ecx)?;
    ss(c, Ss::Mul, 2, g.u32(K_45DC5C)?)?;
    ss(c, Ss::Add, 2, lo(c, 4))?;
    ss(c, Ss::Mul, 2, g.u32(K_45DBBC)?)?;
    if above(c, lo(c, 1), lo(c, 2))? {
        movaps(c, 2, 1);
    } else if above(c, lo(c, 2), lo(c, 4))? {
        // `f718a`.
        movaps(c, 2, 4);
    }
    // `f7192`.
    movaps(c, 1, 4);
    ss(c, Ss::Sub, 1, lo(c, 2))?;
    ss(c, Ss::Mul, 2, lo(c, 0))?;
    ss(c, Ss::Mul, 1, lo(c, 3))?;
    ss(c, Ss::Add, 1, lo(c, 2))?;
    zero(c, 2);
    gate(c, g)
}

/// `f71c0`: for the `f708f` name, a zero `xmm2` at or above body `+25c`
/// gives zero and `bl = 0`; otherwise `bl = 1` (always active).
fn gate(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if c.edi == imm(g, NAME_F71C0)? {
        let m = g.u32(c.esi.wrapping_add(0x25c))?;
        // `jb`: CF set (below or unordered).
        if comiss(c, lo(c, 2), m)? & 1 == 0 {
            movaps(c, 1, 2);
            c.ebx = byte(c.ebx, 0);
            return finish(c, g);
        }
    }
    // `f71db`.
    c.ebx = byte(c.ebx, 1);
    clamp(c, g)
}

/// `f71e2`: `min(body +354, xmm2)` (stored at `[esp+2c]`) over limit 0
/// (`f78e0`), site `f720a`.
fn case_f71e2(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let m = g.u32(c.esi.wrapping_add(0x354))?;
    if above(c, lo(c, 2), m)? {
        g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 2))?;
    } else {
        // `f71f3`.
        load(c, 0, g.u32(c.esi.wrapping_add(0x354))?);
        g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 0))?;
    }
    // `f7201`.
    push(c, g, 0)?;
    c.eax = c.edx;
    call(c, AXIS, 0xf720a)
}

/// `f7311`: body `+3fc`.
fn case_f7311(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 1, g.u32(c.esi.wrapping_add(0x3fc))?);
    clamp(c, g)
}

/// `f72af`: `max(|model +204 * k|, |model +208 * k|) / |body +35c|`.
fn case_f72af(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = body_tag(c, g)?;
    c.eax = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(8))?;
    let a = c.esp.wrapping_add(0x2c);
    let b = c.esp.wrapping_add(0x30);
    fld(c, g, 0xf72be, c.eax.wrapping_add(0x204))?;
    fop(c, g, 0xf72c4, arith_op::MUL, K_45DC54)?;
    fabs(c, g, 0xf72ca)?;
    fstp(c, g, 0xf72cc, a)?;
    fld(c, g, 0xf72d0, c.eax.wrapping_add(0x208))?;
    load(c, 0, g.u32(a)?);
    fop(c, g, 0xf72dc, arith_op::MUL, K_45DC54)?;
    fabs(c, g, 0xf72e2)?;
    fst(c, g, 0xf72e4, b)?;
    fld(c, g, 0xf72e8, a)?;
    let ja = fcomip(c, g, 0xf72ec)?;
    fpop(c, g, 0xf72ee)?;
    if !ja {
        load(c, 0, g.u32(b)?);
    }
    // `f72f8`.
    fld(c, g, 0xf72f8, c.esi.wrapping_add(0x35c))?;
    g.set_u32(a, lo(c, 0))?;
    c.eax = slot(c, g, 0x2c)?;
    fabs(c, g, 0xf7308)?;
    push(c, g, c.eax)?;
    push(c, g, c.ecx)?;
    ratio_f784f(c, g)
}

/// `f723d`: the cross product of body `+70` and `+7c` into `E-18`..`E-10`
/// (`fsubp`: `st(1) - st(0)`), then its dot with body `+88` over limit 2 (site `f7288`).
fn case_f723d(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let s = c.esi;
    fld(c, g, 0xf723d, s.wrapping_add(0x78))?;
    push(c, g, 2)?;
    fop(c, g, 0xf7242, arith_op::MUL, s.wrapping_add(0x80))?;
    c.eax = c.edx;
    fld(c, g, 0xf724a, s.wrapping_add(0x74))?;
    fop(c, g, 0xf724d, arith_op::MUL, s.wrapping_add(0x84))?;
    fopp(c, g, 0xf7253, arith_op::SUB)?;
    fstp(c, g, 0xf7255, c.esp.wrapping_add(0x14))?;
    fld(c, g, 0xf7259, s.wrapping_add(0x70))?;
    fop(c, g, 0xf725c, arith_op::MUL, s.wrapping_add(0x84))?;
    fld(c, g, 0xf7262, s.wrapping_add(0x7c))?;
    fop(c, g, 0xf7265, arith_op::MUL, s.wrapping_add(0x78))?;
    fopp(c, g, 0xf7268, arith_op::SUB)?;
    fstp(c, g, 0xf726a, c.esp.wrapping_add(0x18))?;
    fld(c, g, 0xf726e, s.wrapping_add(0x7c))?;
    fop(c, g, 0xf7271, arith_op::MUL, s.wrapping_add(0x74))?;
    fld(c, g, 0xf7274, s.wrapping_add(0x70))?;
    fop(c, g, 0xf7277, arith_op::MUL, s.wrapping_add(0x80))?;
    fopp(c, g, 0xf727d, arith_op::SUB)?;
    fstp(c, g, 0xf727f, c.esp.wrapping_add(0x1c))?;
    call(c, AXIS, 0xf7288)
}

/// `f732a`: `max(|model +1f4|, |model +1f8|) / |body +354|`.
fn case_f732a(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = body_tag(c, g)?;
    c.eax = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    wider(c, g, 0xf7339, 0xf733f, 0x1f4, 0x1f8)?;
    // `f7367`.
    fld(c, g, 0xf7367, c.esi.wrapping_add(0x354))?;
    g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 0))?;
    c.edx = slot(c, g, 0x2c)?;
    fabs(c, g, 0xf7377)?;
    push(c, g, c.edx)?;
    push(c, g, c.ecx)?;
    ratio_f784f(c, g)
}

/// `f7380`: `|min(body +354, 0)| / |model +1f8|` when `|model +1f8|` is
/// above the threshold, else zero; no call.
fn case_f7380(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 0, g.u32(c.esi.wrapping_add(0x354))?);
    if above(c, lo(c, 0), lo(c, 2))? {
        g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 2))?;
    } else {
        // `f7395`.
        load(c, 0, g.u32(c.esi.wrapping_add(0x354))?);
        g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 0))?;
    }
    // `f73a3`.
    c.ecx = body_tag(c, g)?;
    c.eax = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    fld(c, g, 0xf73b2, c.eax.wrapping_add(0x1f8))?;
    movaps(c, 1, 2);
    fabs(c, g, 0xf73bb)?;
    fpu_op(c, g, 0xf73bd, 0, |x| x.fld_st(0))?;
    fabs(c, g, 0xf73bf)?;
    fld(c, g, 0xf73c1, K_45DC70)?;
    fpu_op(c, g, 0xf73c7, 0, |x| x.fxch(1))?;
    let ja = fcomip(c, g, 0xf73c9)?;
    fpop(c, g, 0xf73cb)?;
    if ja {
        let at = c.esp.wrapping_add(0x2c);
        fld(c, g, 0xf73cf, at)?;
        fabs(c, g, 0xf73d3)?;
        fpu_op(c, g, 0xf73d5, 0, |x| x.arith_st(arith_op::DIV, 1))?;
        fstp(c, g, 0xf73d7, at)?;
        load(c, 1, g.u32(at)?);
    }
    // `f73e1`.
    fpop(c, g, 0xf73e1)?;
    clamp(c, g)
}

/// `f742f`: zero unless body `+4` has a bit of `0c`; else `|body +88| /
/// max(|model +1f4|, |model +1f8|)`.
fn case_f742f(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let set = g.u8(c.esi.wrapping_add(4))? & 0x0c != 0;
    c.ecx = byte(c.ecx, u8::from(set));
    if !set {
        return nothing(c, g);
    }
    c.edx = body_tag(c, g)?;
    c.eax = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(8))?;
    wider(c, g, 0xf744d, 0xf7453, 0x1f4, 0x1f8)?;
    // `f747b`.
    g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 0))?;
    c.eax = slot(c, g, 0x2c)?;
    push(c, g, c.eax)?;
    c.eax = c.esi.wrapping_add(0x88);
    call(c, MAGNITUDE, 0xf7491)
}

/// `f74a2`: `max(|model +22c|, |model +230|) / |body +358|`.
fn case_f74a2(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = body_tag(c, g)?;
    c.eax = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    wider(c, g, 0xf74b1, 0xf74b7, 0x22c, 0x230)?;
    // `f74df`.
    fld(c, g, 0xf74df, c.esi.wrapping_add(0x358))?;
    g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 0))?;
    c.edx = slot(c, g, 0x2c)?;
    fabs(c, g, 0xf74ef)?;
    push(c, g, c.edx)?;
    push(c, g, c.ecx)?;
    ratio_f784f(c, g)
}

/// `f74f8`: zero when body `+34f` is zero; else `|body +88| /
/// max(|model +1f4|, |model +1f8|)`.
fn case_f74f8(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let cl = g.u8(c.esi.wrapping_add(0x34f))?;
    c.ecx = byte(c.ecx, cl);
    if cl == 0 {
        return nothing(c, g);
    }
    c.ecx = body_tag(c, g)?;
    c.eax = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    wider(c, g, 0xf7515, 0xf751b, 0x1f4, 0x1f8)?;
    // `f7543`.
    g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 0))?;
    c.edx = slot(c, g, 0x2c)?;
    push(c, g, c.edx)?;
    c.eax = c.esi.wrapping_add(0x88);
    call(c, MAGNITUDE, 0xf7559)
}

/// `f7576`: `|body +70 . body +88| / max(|model +1f4|, |model +1f8|)`.
fn case_f7576(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = body_tag(c, g)?;
    c.eax = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    wider(c, g, 0xf7585, 0xf758b, 0x1f4, 0x1f8)?;
    // `f75b3`.
    fld(c, g, 0xf75b3, c.esi.wrapping_add(0x78))?;
    g.set_u32(c.esp.wrapping_add(0x2c), lo(c, 0))?;
    fop(c, g, 0xf75bc, arith_op::MUL, c.esi.wrapping_add(0x90))?;
    c.edx = slot(c, g, 0x2c)?;
    fld(c, g, 0xf75c6, c.esi.wrapping_add(0x74))?;
    push(c, g, c.edx)?;
    fop(c, g, 0xf75ca, arith_op::MUL, c.esi.wrapping_add(0x8c))?;
    push(c, g, c.ecx)?;
    fopp(c, g, 0xf75d1, arith_op::ADD)?;
    fld(c, g, 0xf75d3, c.esi.wrapping_add(0x88))?;
    fop(c, g, 0xf75d9, arith_op::MUL, c.esi.wrapping_add(0x70))?;
    fopp(c, g, 0xf75dc, arith_op::ADD)?;
    fabs(c, g, 0xf75de)?;
    ratio_f784f(c, g)
}

/// `f75e5`: `11d580` writes body `+88` minus body `+70` at `E-18`
/// (`esi` becomes that frame address), then the squared ratio of its
/// length (sites `f75fb`, `f7607`, `f7610`).
fn case_f75e5(c: &mut CpuAbi, _g: &mut Guest) -> Result<Next> {
    c.ecx = c.esi.wrapping_add(0x70);
    c.edx = c.esi.wrapping_add(0x88);
    c.esi = c.esp.wrapping_add(0x10);
    c.eax = c.esp.wrapping_add(0x1c);
    call(c, DIFFERENCE, 0xf75fb)
}

/// `f7642`: `body +368 / definition +20c`.
fn case_f7642(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(c.ebp.wrapping_add(0x20c))?;
    c.ecx = g.u32(c.esi.wrapping_add(0x368))?;
    push(c, g, c.eax)?;
    push(c, g, c.ecx)?;
    call(c, RATIO, 0xf7857)
}

/// `f7655`: the larger of `max(|model +1f4|, |model +1f8|) / |body +354|`
/// and `max(|model +22c|, |model +230|) / |body +358|` (sites `f76ae`,
/// `f770a`).
fn case_f7655(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = body_tag(c, g)?;
    c.ecx = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(8))?;
    let at = c.esp.wrapping_add(0x2c);
    fld(c, g, 0xf7664, c.ecx.wrapping_add(0x1f4))?;
    fld(c, g, 0xf766a, c.ecx.wrapping_add(0x1f8))?;
    fabs(c, g, 0xf7670)?;
    fstp(c, g, 0xf7672, at)?;
    fabs(c, g, 0xf7676)?;
    fld(c, g, 0xf7678, at)?;
    fpu_op(c, g, 0xf767c, 0, |x| x.fxch(1))?;
    let ja = fcomip(c, g, 0xf767e)?;
    fpop(c, g, 0xf7680)?;
    if ja {
        fld(c, g, 0xf7684, c.ecx.wrapping_add(0x1f4))?;
    } else {
        // `f768c`.
        fld(c, g, 0xf768c, c.ecx.wrapping_add(0x1f8))?;
    }
    // `f7692`.
    fabs(c, g, 0xf7692)?;
    fstp(c, g, 0xf7694, at)?;
    c.ecx = slot(c, g, 0x2c)?;
    fld(c, g, 0xf769c, c.esi.wrapping_add(0x354))?;
    push(c, g, c.ecx)?;
    fabs(c, g, 0xf76a3)?;
    push(c, g, c.ecx)?;
    fstp(c, g, 0xf76a6, c.esp)?;
    call(c, RATIO, 0xf76ae)
}

/// `f7718`: `|body +354 - body +35c|` over limit 2 (`f78e0`) (site
/// `f7721`).
fn case_f7718(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, 2)?;
    c.eax = c.edx;
    call(c, AXIS, 0xf7721)
}

/// `f773e`: `body +36c / definition +20c`.
fn case_f773e(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = g.u32(c.ebp.wrapping_add(0x20c))?;
    c.edx = g.u32(c.esi.wrapping_add(0x36c))?;
    push(c, g, c.ecx)?;
    push(c, g, c.edx)?;
    call(c, RATIO, 0xf7857)
}

/// `f7751`: `|body +35c + body +354|` over limit 2 (`f78e0`) (site
/// `f775a`).
fn case_f7751(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, 2)?;
    c.eax = c.edx;
    call(c, AXIS, 0xf775a)
}

/// `f779b`: `body +360 / definition +20c`.
fn case_f779b(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(c.ebp.wrapping_add(0x20c))?;
    c.ecx = g.u32(c.esi.wrapping_add(0x360))?;
    push(c, g, c.eax)?;
    push(c, g, c.ecx)?;
    call(c, RATIO, 0xf7857)
}

/// `f77ae`: `(|body +88| / limit 0) * body +374 - k`, over the
/// pushed constant (sites `f77bc`..`f77f1`).
fn case_f77ae(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let v = imm(g, PUSH_F77AE)?;
    push(c, g, v)?;
    push(c, g, 0)?;
    c.eax = c.edx;
    call(c, AXIS, 0xf77bc)
}

/// `f7835`: `|body +354|` over limit 2 (`f78e0`) (site `f783e`).
fn case_f7835(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, 2)?;
    c.eax = c.edx;
    call(c, AXIS, 0xf783e)
}

/// `push ecx; fstp dword [esp]; call f78b0`: the ratio of the length
/// `24590` left in `st(0)` (`f707e`, `f7491`, `f7559`, `f7607`, `f77cd`).
fn magnitude_ratio(c: &mut CpuAbi, g: &mut Guest, fstp_pc: u32, site: u32) -> Result<Next> {
    push(c, g, c.ecx)?;
    fstp(c, g, fstp_pc, c.esp)?;
    call(c, RATIO, site)
}

/// `f7844`: `sub esp,8; fabs; movss [esp+4],xmm0`, then `f784f`.
fn abs_ratio(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.esp = c.esp.wrapping_sub(8);
    fabs(c, g, 0xf7847)?;
    g.set_u32(c.esp.wrapping_add(4), lo(c, 0))?;
    ratio_f784f(c, g)
}

/// `f784f`: `fstp dword [esp]; call f78b0`.
fn ratio_f784f(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    fstp(c, g, 0xf784f, c.esp)?;
    call(c, RATIO, 0xf7857)
}

/// `f7857`: `movaps xmm1, xmm0`, then the clamp.
fn take(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    movaps(c, 1, 0);
    clamp(c, g)
}

/// `f785a`: clamp `xmm1` to `[xmm2, 1]` (`xmm0` is 1.0 after the upper
/// check runs).
fn clamp(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if above(c, lo(c, 2), lo(c, 1))? {
        movaps(c, 1, 2);
        return finish(c, g);
    }
    // `f7864`.
    load(c, 0, g.u32(ONE)?);
    if above(c, lo(c, 1), lo(c, 0))? {
        movaps(c, 1, 0);
    }
    finish(c, g)
}

/// `f6dec`: `movaps xmm1, xmm2` (zero).
fn nothing(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    movaps(c, 1, 2);
    finish(c, g)
}

/// `f6e25`: one.
fn one(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 1, g.u32(ONE)?);
    finish(c, g)
}

/// `f7874`: store `xmm1` through the value pointer; the active byte is
/// `xmm1 > xmm2` or `bl != 0`; `eax = 1` either way.
fn finish(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let positive = above(c, lo(c, 1), lo(c, 2))?;
    c.edx = slot(c, g, 0x34)?;
    g.set_u32(c.edx, lo(c, 1))?;
    let active = positive || c.ebx as u8 != 0;
    c.ecx = slot(c, g, 0x38)?;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.eax = u32::from(active);
    c.ebp = pop(c, g)?;
    g.set_u8(c.ecx, c.eax as u8)?;
    c.eax = byte(c.eax, 1);
    c.ebx = pop(c, g)?;
    c.esp = c.esp.wrapping_add(0x18);
    ret(c, 0x10)
}

/// `f7829` (and `f77fe`): an unknown name, `al = 0`.
fn miss(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.eax = byte(c.eax, 0);
    c.ebx = pop(c, g)?;
    c.esp = c.esp.wrapping_add(0x18);
    ret(c, 0x10)
}

/// The larger of `|model +a|` and `|model +b|` (`eax` the model data)
/// into `xmm0`, through `[esp+2c]` and `[esp+30]`: `fld [eax+a]` at
/// `first`, the rest from the `fabs` at `q` ([`wider_rest`]).
fn wider(c: &mut CpuAbi, g: &mut Guest, first: u32, q: u32, a: u32, b: u32) -> Result<()> {
    fld(c, g, first, c.eax.wrapping_add(a))?;
    wider_rest(c, g, q, b)
}

/// `fabs; fstp [esp+2c]; fld [eax+b]; movss xmm0,[esp+2c]; fabs; fst
/// [esp+30]; fld [esp+2c]; fcomip st,st(1); fstp st(0); ja +; movss
/// xmm0,[esp+30]`, the `fabs` at `q`.
fn wider_rest(c: &mut CpuAbi, g: &mut Guest, q: u32, b: u32) -> Result<()> {
    let first = c.esp.wrapping_add(0x2c);
    let second = c.esp.wrapping_add(0x30);
    fabs(c, g, q)?;
    fstp(c, g, q + 0x02, first)?;
    fld(c, g, q + 0x06, c.eax.wrapping_add(b))?;
    load(c, 0, g.u32(first)?);
    fabs(c, g, q + 0x12)?;
    fst(c, g, q + 0x14, second)?;
    fld(c, g, q + 0x18, first)?;
    let ja = fcomip(c, g, q + 0x1c)?;
    fpop(c, g, q + 0x1e)?;
    if !ja {
        load(c, 0, g.u32(second)?);
    }
    Ok(())
}

/// One x87 instruction at `pc` (memory operand `mem`, 0 for none) on the
/// guest's x87 state, written back as that instruction leaves it.
fn fpu_op<T>(
    c: &mut CpuAbi,
    g: &Guest,
    pc: u32,
    mem: u32,
    f: impl FnOnce(&mut X87) -> std::result::Result<T, Error>,
) -> Result<T> {
    let mut x = fpu(c);
    let r = f(&mut x).map_err(fp_error)?;
    fpu_done(c, g, x, pc, mem)?;
    Ok(r)
}

/// `fld dword [at]`.
fn fld(c: &mut CpuAbi, g: &Guest, pc: u32, at: u32) -> Result<()> {
    let v = g.u32(at)?;
    fpu_op(c, g, pc, at, |x| x.fld(v))
}

/// `fadd`/`fsub`/`fmul dword [at]`.
fn fop(c: &mut CpuAbi, g: &Guest, pc: u32, op: u32, at: u32) -> Result<()> {
    let v = g.u32(at)?;
    fpu_op(c, g, pc, at, |x| x.arith_m(op, v))
}

/// `faddp`/`fsubp st(1), st(0)` (`de c1`, `de e9`): `st(1) = st(1) op
/// st(0)`, pop.
fn fopp(c: &mut CpuAbi, g: &Guest, pc: u32, op: u32) -> Result<()> {
    fpu_op(c, g, pc, 0, |x| x.arith_reg(op, 1, 0, true))
}

/// `fabs`.
fn fabs(c: &mut CpuAbi, g: &Guest, pc: u32) -> Result<()> {
    fpu_op(c, g, pc, 0, |x| x.unary(unary_op::ABS))
}

/// `fstp st(0)`.
fn fpop(c: &mut CpuAbi, g: &Guest, pc: u32) -> Result<()> {
    fpu_op(c, g, pc, 0, |x| x.fstp_st(0))
}

/// `fcomip st, st(1)`, then `ja` taken (CF and ZF clear; false on
/// unordered).
fn fcomip(c: &mut CpuAbi, g: &Guest, pc: u32) -> Result<bool> {
    let f = fpu_op(c, g, pc, 0, |x| x.fcomip(1))?;
    Ok(f & 0x41 == 0)
}

/// `fstp dword [at]`.
fn fstp(c: &mut CpuAbi, g: &mut Guest, pc: u32, at: u32) -> Result<()> {
    let v = fpu_op(c, g, pc, at, |x| x.fstp_m32())?;
    g.set_u32(at, v)
}

/// `fst dword [at]`.
fn fst(c: &mut CpuAbi, g: &mut Guest, pc: u32, at: u32) -> Result<()> {
    let v = fpu_op(c, g, pc, at, |x| x.fst_m32())?;
    g.set_u32(at, v)
}
