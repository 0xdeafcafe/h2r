//! `bcc20` BASE_OBJECT_IMPORT (docs/engine/object-update.md, "Slice 3
//! bodies"): the `obje` level's slot `+4c` import handler.
//!
//! `bcc20 (object, name, value out, active out) -> al` answers the
//! imports every object has: constants (one, zero), body fields (`+ec`,
//! `+f0`, `+f4`, `+f8`), a body flag (`+10a` bit 2), the model variant
//! fraction, a per-object random value, a facing angle (`b8bd0` then
//! `bff60`), the world's globals (`[4de2f4] + 70..7c`), and callouts for
//! the rest (`bca70`, `bcbb0`, `bcad0`, `bcb10`, `bcb60`, `c18e0`,
//! `1588b0`, and `bc970` for the component names). Most values are
//! clamped to `[xmm2, 1]` (`xmm2` is zero, except where retail reads it
//! straight after a callout). The value goes to the out pointer; the
//! active flag is 1 when the component callout answered, else value > 0.
//!
//! Frame (`E` the entry `esp`): `E-4` (the saved `ecx`) the value, the
//! object argument's low byte `E+4` the result (1, or 0 for an unknown
//! name), `E+8` (the name argument) scratch for the random and facing
//! paths. `esi` (the body) and `bl` (the component answer) are kept
//! across every call.
//!
//! The string IDs the dispatch compares against are read from the
//! immediates of its `cmp`/`sub` instructions, and the random hash's
//! parameters from its `imul`/`add`.

use super::{
    above, byte, call, cvtsi2ss, fp_error, fpu, fpu_done, imm, load, lo, movaps, pop, push, ret,
    slot, ss, ucomiss_lahf_jp, unowned, zero, Ss, ONE,
};
use guest_layout::points;
use crate::sim_objects::{Cx, Next, Result};
use runtime::{CpuAbi, Guest};

/// The `cmp eax, imm32` immediates of the dispatch, by instruction.
/// `bcc47`: the random name.
pub(super) const NAME_RANDOM: u32 = 0x000b_cc48;
/// `bcc66`: the name answered by `bcad0`.
pub(super) const NAME_BCAD0: u32 = 0x000b_cc67;
/// `bcc77`: the facing angle.
pub(super) const NAME_FACING: u32 = 0x000b_cc78;
/// `bcc84`: a name answering 1.
pub(super) const NAME_ONE: u32 = 0x000b_cc85;
/// `bcc8f`: a name answering 0.
pub(super) const NAME_ZERO: u32 = 0x000b_cc90;
/// `bcc9a`: body `+10a` bit 2 clear.
pub(super) const NAME_FLAG: u32 = 0x000b_cc9b;
/// `bcd12`: the model variant fraction.
pub(super) const NAME_VARIANT: u32 = 0x000b_cd13;
/// `bcd19`: the name answered by `bcbb0`.
pub(super) const NAME_BCBB0: u32 = 0x000b_cd1a;
/// `bcd20`: the name answered by `bca70`.
pub(super) const NAME_BCA70: u32 = 0x000b_cd21;
/// `bcdb2`: body `+ec`.
pub(super) const NAME_BODY_EC: u32 = 0x000b_cdb3;
/// `bcdbb` (`sub eax, imm32`): a second name answering 1.
pub(super) const NAME_ONE_B: u32 = 0x000b_cdbc;
/// `bcdc6` (`sub eax, imm32`): the delta to the `bcb10` name; one more is
/// the `bcb60` name.
pub(super) const DELTA_BCB10: u32 = 0x000b_cdc7;
/// `bcdfb`: a component name.
pub(super) const NAME_COMPONENT: u32 = 0x000b_cdfc;
/// `bce06`: body `+f0`.
pub(super) const NAME_BODY_F0: u32 = 0x000b_ce07;
/// `bce54`: body `+f0` minus 1.
pub(super) const NAME_F0_LESS_ONE: u32 = 0x000b_ce55;
/// `bce65`: the name answered by `c18e0`.
pub(super) const NAME_C18E0: u32 = 0x000b_ce66;
/// `bce76`: the `1588b0` / body `+f0` test.
pub(super) const NAME_SHIELD_TEST: u32 = 0x000b_ce77;
/// `bce7d`: a third name answering 1.
pub(super) const NAME_ONE_C: u32 = 0x000b_ce7e;
/// `bce84`: body `+f8`.
pub(super) const NAME_BODY_F8: u32 = 0x000b_ce85;
/// `bcf03`: body `+f4`.
pub(super) const NAME_BODY_F4: u32 = 0x000b_cf04;
/// `bcf0a`, `bcf11`: a component range `(lo, hi]`.
pub(super) const COMPONENT_LO: u32 = 0x000b_cf0b;
pub(super) const COMPONENT_HI: u32 = 0x000b_cf12;
/// `bcf45`: world `+78`.
pub(super) const NAME_WORLD_78: u32 = 0x000b_cf46;
/// `bcf50` (`sub edx, imm32`): a component name.
pub(super) const NAME_COMPONENT_B: u32 = 0x000b_cf52;
/// `bcf58` (`sub edx, imm32`): the delta to world `+70`; one more is
/// world `+74`.
pub(super) const DELTA_WORLD_70: u32 = 0x000b_cf5a;
/// `bcf89`: world `+7c`.
pub(super) const NAME_WORLD_7C: u32 = 0x000b_cf8a;
/// `bcf90`, `bcf97`: a second component range `(lo, hi]`.
pub(super) const COMPONENT_LO_B: u32 = 0x000b_cf91;
pub(super) const COMPONENT_HI_B: u32 = 0x000b_cf98;
/// `bce1e` (`imul ecx, ecx, imm32`) and `bce24` (`add ecx, imm32`): the
/// random hash.
pub(super) const HASH_MUL: u32 = 0x000b_ce20;
pub(super) const HASH_ADD: u32 = 0x000b_ce26;

/// The world globals pointer.
pub(super) const WORLD: u32 = 0x004d_e2f4;
/// The game globals pointer (`+8` the mode).
pub(super) const GAME: u32 = 0x004e_6948;
/// The pointer whose `+1c` the facing path passes to `bff60`.
const FACING_BASE: u32 = 0x004e_0350;
/// The tag entries global.
const TAGS: u32 = super::TAGS;
/// f32 constants: the facing threshold, its scale and offset, 2^32 and
/// the random scale.
const FACING_LIMIT: u32 = 0x0045_dbe8;
const FACING_SCALE: u32 = 0x0045_e0b4;
const FACING_OFFSET: u32 = 0x0045_dbbc;
const TWO_32: u32 = 0x0045_dbb8;
pub(super) const RANDOM_SCALE: u32 = 0x0045_dc50;

/// The callees.
const MARKER: u32 = 0x000b_8bd0;
const ANGLE: u32 = 0x000b_ff60;
const BCA70: u32 = 0x000b_ca70;
const BCBB0: u32 = 0x000b_cbb0;
const BCAD0: u32 = 0x000b_cad0;
const BCB60: u32 = 0x000b_cb60;
const BCB10: u32 = 0x000b_cb10;
const SHIELD_QUERY: u32 = 0x0015_88b0;
const C18E0: u32 = 0x000c_18e0;
const COMPONENT: u32 = 0x000b_c970;

pub(in crate::sim_objects) const BASE_SITES: [u32; 10] = [
    0xbccc4, 0xbccfa, 0xbcd30, 0xbcd3a, 0xbcdaa, 0xbcdd9, 0xbcde6, 0xbcecb, 0xbcefb, 0xbcfac,
];

fn key(g: &Guest, at: u32) -> Result<i32> {
    Ok(imm(g, at)? as i32)
}

/// `bcc20 (object: stack 0, name: stack 1, value: stack 2, active: stack
/// 3) -> al`, `ret 10`.
pub(in crate::sim_objects) fn base(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        p if p == points::BASE_OBJECT_IMPORT.base => {
            push(c, g, c.ecx)?;
            c.ecx = slot(c, g, 8)?;
            c.edx = g.u32(super::OBJECTS)?;
            c.edx = g.u32(c.edx.wrapping_add(0x44))?;
            zero(c, 2);
            c.eax = c.ecx & 0xffff;
            push(c, g, c.ebx)?;
            c.eax = c.eax.wrapping_mul(3);
            push(c, g, c.esi)?;
            c.esi = g.u32(c.edx.wrapping_add(c.eax.wrapping_mul(4)).wrapping_add(8))?;
            c.eax = slot(c, g, 0x14)?;
            c.ebx = byte(c.ebx, 0);
            movaps(c, 0, 2);
            g.set_u8(c.esp.wrapping_add(0x10), 1)?;
            g.set_u32(c.esp.wrapping_add(8), lo(c, 0))?;
            dispatch(c, g)
        }
        // `bccc4`: `eax` the marker; `fabs` of its `+c` against the limit.
        0xbccc4 => {
            let mut x = fpu(c);
            x.fld(g.u32(c.eax.wrapping_add(0x0c))?).map_err(fp_error)?;
            x.unary(x87::abi::unary_op::ABS).map_err(fp_error)?;
            x.fld(g.u32(FACING_LIMIT)?).map_err(fp_error)?;
            let f = x.fcomip(1).map_err(fp_error)?;
            x.fstp_st(0).map_err(fp_error)?;
            if f & 0x41 != 0 {
                // `jbe bcee9`: the last memory operand is the limit's.
                fpu_done(c, g, x, 0x000b_ccd1, 0)?;
                c.x87.data_pointer = FACING_LIMIT;
                c.x87.data_selector = 16;
                return one_out(c, g);
            }
            x.fld(g.u32(c.eax.wrapping_add(4))?).map_err(fp_error)?;
            c.ecx = g.u32(FACING_BASE)?;
            x.fld(g.u32(c.eax.wrapping_add(8))?).map_err(fp_error)?;
            c.edx = g.u32(c.ecx.wrapping_add(0x1c))?;
            x.unary(x87::abi::unary_op::PATAN).map_err(fp_error)?;
            push(c, g, c.edx)?;
            let at = c.esp.wrapping_add(0x18);
            let angle = x.fstp_m32().map_err(fp_error)?;
            g.set_u32(at, angle)?;
            fpu_done(c, g, x, 0x000b_cceb, at)?;
            load(c, 0, g.u32(at)?);
            call(c, ANGLE, BASE_SITES[1])
        }
        0xbccfa => {
            ss(c, Ss::Mul, 0, g.u32(FACING_SCALE)?)?;
            ss(c, Ss::Add, 0, g.u32(FACING_OFFSET)?)?;
            zero(c, 2);
            clamp(c, g)
        }
        // `bca70` leaves `xmm2` as it is.
        0xbcd30 => clamp(c, g),
        0xbcd3a | 0xbcdaa | 0xbcdd9 | 0xbcde6 | 0xbcefb => {
            zero(c, 2);
            clamp(c, g)
        }
        0xbcecb => {
            if !above(c, lo(c, 0), lo(c, 2))? {
                return zero_out(c, g);
            }
            shield_test(c, g)
        }
        0xbcfac => {
            let v = slot(c, g, 8)?;
            load(c, 0, v);
            c.ebx = byte(c.ebx, c.eax as u8);
            clamp(c, g)
        }
        _ => Err(unowned(pc)),
    }
}

/// `bcc47`: the name dispatch, `eax` the name, `ecx` the object, `esi`
/// its body.
fn dispatch(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let name = c.eax as i32;
    let random = key(g, NAME_RANDOM)?;
    if name == random {
        return random_value(c, g);
    }
    if name > random {
        return dispatch_high(c, g, name);
    }
    let bcad0 = key(g, NAME_BCAD0)?;
    if name == bcad0 {
        return call(c, BCAD0, BASE_SITES[4]);
    }
    if name > bcad0 {
        return dispatch_mid(c, g, name);
    }
    let facing = key(g, NAME_FACING)?;
    if name == facing {
        // `bccbb`.
        push(c, g, 0)?;
        c.eax = c.ecx;
        return call(c, MARKER, BASE_SITES[0]);
    }
    if name > facing {
        // `bcd12`.
        if name == key(g, NAME_VARIANT)? {
            return variant(c, g);
        }
        if name == key(g, NAME_BCBB0)? {
            return call(c, BCBB0, BASE_SITES[3]);
        }
        if name != key(g, NAME_BCA70)? {
            return missing(c, g);
        }
        return call(c, BCA70, BASE_SITES[2]);
    }
    if name == key(g, NAME_ONE)? {
        return one_out(c, g);
    }
    if name == key(g, NAME_ZERO)? {
        return zero_out(c, g);
    }
    if name != key(g, NAME_FLAG)? {
        return missing(c, g);
    }
    // `bcca5`: `mov al, [esi+10a]; shr al, 2; test al, 1`.
    let al = g.u8(c.esi.wrapping_add(0x10a))? >> 2;
    c.eax = byte(c.eax, al);
    if al & 1 == 0 {
        return one_out(c, g);
    }
    epilogue(c, g)
}

/// `bcdb2`: names between the `bcad0` name and the random name.
fn dispatch_mid(c: &mut CpuAbi, g: &mut Guest, name: i32) -> Result<Next> {
    let body = key(g, NAME_BODY_EC)?;
    if name == body {
        load(c, 0, g.u32(c.esi.wrapping_add(0xec))?);
        return clamp(c, g);
    }
    if name > body {
        // `bcdfb`.
        if name == key(g, NAME_COMPONENT)? {
            return component(c, g);
        }
        if name != key(g, NAME_BODY_F0)? {
            return missing(c, g);
        }
        load(c, 0, g.u32(c.esi.wrapping_add(0xf0))?);
        return clamp(c, g);
    }
    // `bcdbb`: `sub eax, ..; je; sub eax, ..; je; dec eax; jne`.
    c.eax = c.eax.wrapping_sub(imm(g, NAME_ONE_B)?);
    if c.eax == 0 {
        return one_out(c, g);
    }
    c.eax = c.eax.wrapping_sub(imm(g, DELTA_BCB10)?);
    if c.eax == 0 {
        return call(c, BCB10, BASE_SITES[6]);
    }
    c.eax = c.eax.wrapping_sub(1);
    if c.eax != 0 {
        return missing(c, g);
    }
    call(c, BCB60, BASE_SITES[5])
}

/// `bce54`: names above the random name.
fn dispatch_high(c: &mut CpuAbi, g: &mut Guest, name: i32) -> Result<Next> {
    let less_one = key(g, NAME_F0_LESS_ONE)?;
    if name == less_one {
        // `bcf33`.
        load(c, 0, g.u32(c.esi.wrapping_add(0xf0))?);
        ss(c, Ss::Sub, 0, g.u32(ONE)?)?;
        return clamp(c, g);
    }
    if name > less_one {
        return dispatch_top(c, g, name);
    }
    let c18e0 = key(g, NAME_C18E0)?;
    if name == c18e0 {
        return call(c, C18E0, BASE_SITES[8]);
    }
    if name > c18e0 {
        // `bcf03`.
        if name == key(g, NAME_BODY_F4)? {
            load(c, 0, g.u32(c.esi.wrapping_add(0xf4))?);
            return clamp(c, g);
        }
        if name <= key(g, COMPONENT_LO)? {
            return missing(c, g);
        }
        if name <= key(g, COMPONENT_HI)? {
            return component(c, g);
        }
        return missing(c, g);
    }
    if name == key(g, NAME_SHIELD_TEST)? {
        return shield(c, g);
    }
    if name == key(g, NAME_ONE_C)? {
        return one_out(c, g);
    }
    if name != key(g, NAME_BODY_F8)? {
        return missing(c, g);
    }
    load(c, 0, g.u32(c.esi.wrapping_add(0xf8))?);
    clamp(c, g)
}

/// `bcf45`: names above the body `+f0` minus 1 name.
fn dispatch_top(c: &mut CpuAbi, g: &mut Guest, name: i32) -> Result<Next> {
    let world_78 = key(g, NAME_WORLD_78)?;
    if name == world_78 {
        // `bcf7d`.
        c.eax = g.u32(WORLD)?;
        load(c, 0, g.u32(c.eax.wrapping_add(0x78))?);
        return clamp(c, g);
    }
    if name > world_78 {
        // `bcf89`.
        if name == key(g, NAME_WORLD_7C)? {
            // `bcfb6`.
            c.eax = g.u32(WORLD)?;
            load(c, 0, g.u32(c.eax.wrapping_add(0x7c))?);
            return clamp(c, g);
        }
        if name <= key(g, COMPONENT_LO_B)? || name > key(g, COMPONENT_HI_B)? {
            return missing(c, g);
        }
        return component(c, g);
    }
    // `bcf4e`: `mov edx, eax; sub edx, ..; je; sub edx, ..; je; dec edx;
    // jne`.
    c.edx = c.eax.wrapping_sub(imm(g, NAME_COMPONENT_B)?);
    if c.edx == 0 {
        return component(c, g);
    }
    c.edx = c.edx.wrapping_sub(imm(g, DELTA_WORLD_70)?);
    if c.edx == 0 {
        // `bcf70`.
        c.edx = g.u32(WORLD)?;
        load(c, 0, g.u32(c.edx.wrapping_add(0x70))?);
        return clamp(c, g);
    }
    c.edx = c.edx.wrapping_sub(1);
    if c.edx != 0 {
        return missing(c, g);
    }
    // `bcf63`.
    c.ecx = g.u32(WORLD)?;
    load(c, 0, g.u32(c.ecx.wrapping_add(0x74))?);
    clamp(c, g)
}

/// `bcd42`: the model variant fraction, `(variant + 1) / count` with the
/// variant byte at body `+b1` and the count at the model's `+50`.
fn variant(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = g.u32(c.esi)?;
    c.eax = g.u32(TAGS)?;
    c.ecx = (c.ecx & 0xffff) << 4;
    c.edx = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    c.ecx = g.u32(c.edx.wrapping_add(0x38))?;
    c.ecx = (c.ecx & 0xffff) << 4;
    c.eax = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    c.eax = g.u32(c.eax.wrapping_add(0x50))?;
    if (c.eax as i32) <= 0 {
        return epilogue(c, g);
    }
    c.ecx = i32::from(g.u8(c.esi.wrapping_add(0xb1))? as i8) as u32;
    c.ecx = c.ecx.wrapping_add(1);
    if (c.ecx as i32) < 0 {
        c.ecx = 0;
    } else if (c.ecx as i32) > (c.eax as i32) {
        c.ecx = c.eax;
    }
    cvtsi2ss(c, 0, c.ecx)?;
    cvtsi2ss(c, 1, c.eax)?;
    ss(c, Ss::Div, 0, lo(c, 1))?;
    clamp(c, g)
}

/// `bce1e`: the per-object random value, a hash of the handle in `ecx`
/// through `fild`, scaled.
fn random_value(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = c
        .ecx
        .wrapping_mul(imm(g, HASH_MUL)?)
        .wrapping_add(imm(g, HASH_ADD)?)
        >> 16;
    g.set_u32(c.esp.wrapping_add(0x14), c.ecx)?;
    let mut x = fpu(c);
    x.fild(c.ecx as i32).map_err(fp_error)?;
    if (c.ecx as i32) < 0 {
        x.fadd_m(g.u32(TWO_32)?).map_err(fp_error)?;
    }
    x.fmul_m(g.u32(RANDOM_SCALE)?).map_err(fp_error)?;
    let at = c.esp.wrapping_add(8);
    let v = x.fstp_m32().map_err(fp_error)?;
    g.set_u32(at, v)?;
    fpu_done(c, g, x, 0x000b_ce45, at)?;
    load(c, 0, g.u32(at)?);
    clamp(c, g)
}

/// `bce9c`: in game mode 2, with body `+aa` (a type index) one of the
/// first two and body `+13c` set, ask `1588b0` first.
fn shield(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = g.u32(GAME)?;
    if g.u32(c.edx.wrapping_add(8))? == 2 {
        c.ecx = byte(c.ecx, g.u8(c.esi.wrapping_add(0xaa))?);
        c.eax = 1u32.wrapping_shl(c.ecx & 0xff);
        if c.eax & 3 != 0 {
            c.ecx = g.u32(c.esi.wrapping_add(0x13c))?;
            if c.ecx != u32::MAX {
                push(c, g, 2)?;
                return call(c, SHIELD_QUERY, BASE_SITES[7]);
            }
        }
    }
    shield_test(c, g)
}

/// `bced4`: 1 when body `+f0` equals `xmm2` (ordered), else `xmm2`.
fn shield_test(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 0, g.u32(c.esi.wrapping_add(0xf0))?);
    if ucomiss_lahf_jp(c, lo(c, 0), lo(c, 2))? {
        return zero_out(c, g);
    }
    one_out(c, g)
}

/// `bcfa2`: `bc970 (handle: ecx, value: stack 0) -> al`, into the frame's
/// value.
fn component(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = c.esp.wrapping_add(8);
    push(c, g, c.edx)?;
    call(c, COMPONENT, BASE_SITES[9])
}

/// `bcf1c`: an unknown name, result 0.
fn missing(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    g.set_u8(c.esp.wrapping_add(0x10), 0)?;
    epilogue(c, g)
}

/// `bcee9`.
fn one_out(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 0, g.u32(ONE)?);
    epilogue(c, g)
}

/// `bcfc5`.
fn zero_out(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    movaps(c, 0, 2);
    epilogue(c, g)
}

/// `bcfc0`: `xmm0` clamped to `[xmm2, 1]` (`xmm1` is 1 past the low test).
fn clamp(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if above(c, lo(c, 2), lo(c, 0))? {
        return zero_out(c, g);
    }
    load(c, 1, g.u32(ONE)?);
    if above(c, lo(c, 0), lo(c, 1))? {
        movaps(c, 0, 1);
    }
    epilogue(c, g)
}

/// `bcfda`: store the value, then the active flag (1 when `bl`, else
/// value above `xmm2`), `al` the frame's result byte.
fn epilogue(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = slot(c, g, 0x18)?;
    c.esi = pop(c, g)?;
    let bl = c.ebx as u8;
    g.set_u32(c.ecx, lo(c, 0))?;
    c.ebx = pop(c, g)?;
    if bl != 0 {
        c.edx = slot(c, g, 0x14)?;
        c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(8))?);
        g.set_u8(c.edx, 1)?;
    } else {
        let active = above(c, lo(c, 0), lo(c, 2))?;
        c.ecx = slot(c, g, 0x14)?;
        c.eax = u32::from(active);
        g.set_u8(c.ecx, c.eax as u8)?;
        c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(8))?);
    }
    c.ecx = pop(c, g)?;
    ret(c, 0x10)
}
