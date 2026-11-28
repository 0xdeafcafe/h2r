//! Object function values (docs/engine/object-update.md, "Slice 3
//! bodies"): `bab40` OBJECT_FUNCTION_VALUE, the export evaluator `ba8c0`
//! and the import dispatcher `108d90` OBJECT_FUNCTION_IMPORT.
//!
//! `bab40 (object, name, value out) -> al` answers a named function of an
//! object: a scripted override (`10aac0`, when body `+c0` bit 9 is set),
//! the two built-in names (one and zero), an import through the type
//! ancestry (`108d90`), every export of the object's definition with that
//! name (`ba8c0`, definition `+64` count, `+68` records of `20` bytes, the
//! name at `+8`), and finally the parent's value when the body's flags bit
//! 26 is set (`bab40` on body `+14`). The result byte lives in the top
//! byte of the saved `ecx` slot, so `ecx` at return carries it.
//!
//! `ba8c0 (record: eax, object, value out) -> al` evaluates one export:
//! its input import (record `+4`) through `108d90`, the function
//! (`13bb90` on record `+14`, a guest callout), an optional time-driven
//! input (game time at `[510c54]`, offset per object by a hash of the
//! handle), a scale from another function value (`bab40` on record
//! `+1c`), a threshold (record `+10`), and a gate import (record `+c`).
//!
//! `108d90 (object, name, value: ebx, active: edi) -> al` zeroes both
//! outputs, then calls slot `+4c` of each ancestry level until one
//! answers non-zero; with no answer it zeroes them again.

use super::{
    Ss, above, byte, call, fpu, fpu_done, fp_error, imm, load, lo, logic, movaps,
    pop, push, ret, slot, ss, test8, unowned, zero, NONE, OBJECTS, ONE, ONE_BITS, TAGS, TYPES,
    ZERO,
};
use guest_layout::datum::layout;
use guest_layout::points;
use crate::sim_objects::{Cx, Next, Result};
use runtime::{CpuAbi, Guest};
use ::sim_objects::functions::{ExportCursor, ExportStop};

/// Scripted override lookup `(name, value, out) -> al`, object in `ebx`.
const OVERRIDE: u32 = 0x0010_aac0;
/// The export evaluator.
const EVALUATE: u32 = 0x000b_a8c0;
/// A tag function's value: `(function: eax, input, scale)`, `xmm0` out.
const FUNCTION: u32 = 0x0013_bb90;
/// Game time: `+4` seconds per tick (f32), `+8` the tick (i32).
pub(super) const GAME_TIME: u32 = 0x0051_0c54;
/// 2^32 (f32), the unsigned fix-up `ba8c0` never takes.
const TWO_32: u32 = 0x0045_dbb8;
/// The handle hash's scale (f32).
pub(super) const HASH_SCALE: u32 = 0x0045_dc50;

/// The imm32 of `cmp edi, imm` for the built-in "one" and "zero" names.
pub(super) const NAME_ONE: u32 = 0x000b_abab;
pub(super) const NAME_ZERO: u32 = 0x000b_abb7;
/// The imm32s of `imul eax, eax, imm` and `add eax, imm`: the handle hash.
pub(super) const HASH_MUL: u32 = 0x000b_a9e9;
pub(super) const HASH_ADD: u32 = 0x000b_a9ee;

/// Body `+c0` and its override bit (9, as a word).
const PHYSICS_FLAGS: u32 = 0xc0;
/// Definition `+64` export count, `+68` exports, `20` bytes each.
const EXPORT_COUNT: u32 = 0x64;

pub(in crate::sim_objects) const VALUE_SITES: [u32; 4] = [0xbab93, 0xbabe2, 0xbac3c, 0xbac74];
pub(in crate::sim_objects) const EVALUATE_SITES: [u32; 6] =
    [0xba92c, 0xba94e, 0xba991, 0xbaa27, 0xbaa4a, 0xbaacc];
pub(in crate::sim_objects) const IMPORT_SITES: [u32; 1] = [0x108de7];

/// `bab40`: the saved-register epilogue, `pop edi; pop esi; pop ebp;
/// [al]; pop ebx; pop ecx; ret 0c`.
fn value_ret(c: &mut CpuAbi, g: &Guest, al: Option<u8>) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    if let Some(al) = al {
        c.eax = byte(c.eax, al);
    }
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    ret(c, 0x0c)
}

/// `bab40`'s exits that store `xmm0` through `ebp` between `pop esi` and
/// `pop ebp`.
fn value_store_ret(c: &mut CpuAbi, g: &mut Guest, al: u8) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    g.set_u32(c.ebp, lo(c, 0))?;
    c.ebp = pop(c, g)?;
    c.eax = byte(c.eax, al);
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    ret(c, 0x0c)
}

/// `baca5`: the result byte from the frame.
fn value_frame_ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    let al = g.u8(c.esp.wrapping_add(0x13))?;
    value_ret(c, g, Some(al))
}

/// `bab40 (object: stack 0, name: stack 1, value: stack 2) -> al`.
pub(in crate::sim_objects) fn value(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        p if p == points::OBJECT_FUNCTION_VALUE.base => {
            push(c, g, c.ecx)?;
            zero(c, 4);
            push(c, g, c.ebx)?;
            c.ebx = slot(c, g, 0x0c)?;
            push(c, g, c.ebp)?;
            c.ebp = slot(c, g, 0x18)?;
            c.eax = c.ebx & 0xffff;
            push(c, g, c.esi)?;
            c.esi = c.eax * 3;
            c.eax = g.u32(OBJECTS)?;
            g.set_u32(c.ebp, lo(c, 4))?;
            c.ecx = g.u32(c.eax.wrapping_add(layout::DATA))?;
            c.esi <<= 2;
            g.set_u8(c.esp.wrapping_add(0x0f), 0)?;
            c.edx = g.u32(c.ecx.wrapping_add(c.esi).wrapping_add(8))?;
            c.eax = u32::from(g.u16(c.edx.wrapping_add(PHYSICS_FLAGS))?);
            push(c, g, c.edi)?;
            c.edi = slot(c, g, 0x1c)?;
            c.eax >>= 9;
            if c.eax & 1 != 0 {
                c.ecx = c.esp.wrapping_add(0x20);
                push(c, g, c.ecx)?;
                push(c, g, c.ebp)?;
                push(c, g, c.edi)?;
                return call(c, OVERRIDE, VALUE_SITES[0]);
            }
            value_names(c, g)
        }
        0xbab93 => {
            if c.eax as u8 != 0 {
                return value_ret(c, g, Some(1));
            }
            value_names(c, g)
        }
        0xbabe2 => {
            if c.eax as u8 != 0 {
                return value_frame_ret(c, g);
            }
            value_exports(c, g)
        }
        0xbac3c => {
            let al = c.eax as u8;
            g.set_u8(c.esp.wrapping_add(0x13), al)?;
            c.eax = byte(c.eax, 1);
            value_export_next(c, g)
        }
        0xbac74 => value_ret(c, g, None),
        _ => Err(unowned(pc)),
    }
}

/// `baba1`: the built-in names, then the import.
fn value_names(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if c.edi == 0 || c.edi == imm(g, NAME_ONE)? {
        // `bac8e`.
        load(c, 0, g.u32(ONE)?);
        return value_store_ret(c, g, 1);
    }
    if c.edi == imm(g, NAME_ZERO)? {
        // `babbd`: stores `xmm4`, which is zero.
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
        g.set_u32(c.ebp, lo(c, 4))?;
        c.ebp = pop(c, g)?;
        c.eax = byte(c.eax, 0);
        c.ebx = pop(c, g)?;
        c.ecx = pop(c, g)?;
        return ret(c, 0x0c);
    }
    if c.ebx == NONE {
        return value_frame_ret(c, g);
    }
    push(c, g, c.edi)?;
    push(c, g, c.ebx)?;
    c.edi = c.esp.wrapping_add(0x1b);
    c.ebx = c.ebp;
    call(c, points::OBJECT_FUNCTION_IMPORT.base, VALUE_SITES[1])
}

/// `babea`: the definition's exports with this name.
fn value_exports(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = g.u32(OBJECTS)?;
    c.ecx = g.u32(c.edx.wrapping_add(layout::DATA))?;
    c.ecx = g.u32(c.ecx.wrapping_add(c.esi).wrapping_add(8))?;
    c.edx = g.u32(c.ecx)?;
    c.esi = g.u32(TAGS)?;
    c.edx = (c.edx & 0xffff) << 4;
    c.edi = g.u32(c.edx.wrapping_add(c.esi).wrapping_add(8))?;
    c.edx = g.u32(c.edi.wrapping_add(EXPORT_COUNT))?;
    c.esi = 0;
    g.set_u32(c.esp.wrapping_add(0x20), c.ecx)?;
    if (c.edx as i32) <= 0 {
        return value_parent(c, g);
    }
    c.ebx = 0;
    value_export(c, g)
}

/// `bac20`: scan consecutive export names up to the next evaluator call.
fn value_export(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let mut cursor = ExportCursor { index: c.esi, offset: c.ebx };
    let stop = cursor.find(c.edi, c.esp.wrapping_add(0x1c), |addr| g.u32(addr))?;
    c.esi = cursor.index;
    c.ebx = cursor.offset;
    match stop {
        ExportStop::Evaluate { record, name } => {
            c.ecx = record;
            c.edx = name;
            c.eax = slot(c, g, 0x18)?;
            push(c, g, c.ebp)?;
            push(c, g, c.eax)?;
            c.eax = record;
            call(c, EVALUATE, VALUE_SITES[2])
        }
        ExportStop::End { count, last_name } => {
            c.ecx = count as u32;
            c.edx = last_name;
            value_exports_done(c, g)
        }
    }
}

/// `bac42`: re-read the count after a matching export has run.
fn value_export_next(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = g.u32(c.edi.wrapping_add(EXPORT_COUNT))?;
    let mut cursor = ExportCursor { index: c.esi, offset: c.ebx };
    cursor.advance();
    c.esi = cursor.index;
    c.ebx = cursor.offset;
    if (c.esi as i32) < (c.ecx as i32) {
        return value_export(c, g);
    }
    value_exports_done(c, g)
}

fn value_exports_done(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if c.eax as u8 != 0 {
        return value_frame_ret(c, g);
    }
    c.ecx = slot(c, g, 0x20)?;
    value_parent(c, g)
}

/// `bac55`: the parent's value when body flags bit 26 is set.
fn value_parent(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = g.u32(c.ecx.wrapping_add(4))? >> 0x1a;
    if c.edx & 1 != 0 {
        c.eax = g.u32(c.ecx.wrapping_add(0x14))?;
        if c.eax != NONE {
            c.ecx = slot(c, g, 0x1c)?;
            push(c, g, c.ebp)?;
            push(c, g, c.ecx)?;
            push(c, g, c.eax)?;
            return call(c, points::OBJECT_FUNCTION_VALUE.base, VALUE_SITES[3]);
        }
    }
    // `bac7c`.
    zero(c, 0);
    value_store_ret(c, g, 0)
}

/// `ba8c0`'s epilogue: `pop edi` (when pushed), `pop esi; pop ebp;
/// [al]; [store xmm0 to the out pointer]; pop ebx; add esp, 10h; ret 8`.
fn evaluate_ret(c: &mut CpuAbi, g: &mut Guest, store: bool, al: Option<u8>) -> Result<Next> {
    let out = if store { Some(slot(c, g, 0x28)?) } else { None };
    if store {
        c.ecx = out.unwrap();
        c.edi = pop(c, g)?;
    }
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    if let Some(al) = al {
        c.eax = byte(c.eax, al);
    }
    if let Some(out) = out {
        g.set_u32(out, lo(c, 0))?;
    }
    c.ebx = pop(c, g)?;
    c.esp = c.esp.wrapping_add(0x10);
    ret(c, 8)
}

/// `ba8c0 (record: eax, object: stack 0, value: stack 1) -> al`.
///
/// Frame (`E` the entry `esp`): `E-0c` the value, `E-8` the scale or
/// time input, `E-4` the hash, `E-0d` "input is time" (record input type
/// 3), `E-0e` "not range" (record flags bit 1 clear); the object's
/// argument slot's low byte `E+4` is the active flag.
pub(in crate::sim_objects) fn evaluate(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        EVALUATE => {
            c.esp = c.esp.wrapping_sub(0x10);
            zero(c, 0);
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = slot(c, g, 0x1c)?;
            push(c, g, c.esi)?;
            c.esi = c.eax;
            c.eax = g.u32(c.esi)? >> 1;
            c.eax = byte(c.eax, !(c.eax as u8) & 1);
            g.set_u8(c.esp.wrapping_add(0x0e), c.eax as u8)?;
            c.eax = g.u32(c.esi.wrapping_add(0x18))?;
            g.set_u8(c.esp.wrapping_add(0x20), 0)?;
            g.set_u32(c.esp.wrapping_add(0x10), lo(c, 0))?;
            let kind = g.u8(c.eax)?;
            c.ecx = g.u32(c.esi.wrapping_add(4))?;
            g.set_u8(c.esp.wrapping_add(0x0f), u8::from(kind == 3))?;
            c.eax = c.ebp & 0xffff;
            c.edx = c.eax * 3;
            c.eax = g.u32(OBJECTS)?;
            c.eax = g.u32(c.eax.wrapping_add(layout::DATA))?;
            c.edx = g.u32(c.eax.wrapping_add(c.edx * 4).wrapping_add(8))?;
            c.eax = u32::from(g.u16(c.edx.wrapping_add(PHYSICS_FLAGS))?) >> 9;
            if c.eax & 1 != 0 {
                c.eax = slot(c, g, 0x24)?;
                c.edx = c.esp.wrapping_add(0x14);
                push(c, g, c.edx)?;
                push(c, g, c.eax)?;
                push(c, g, c.ecx)?;
                c.ebx = c.ebp;
                return call(c, OVERRIDE, EVALUATE_SITES[0]);
            }
            evaluate_import(c, g)
        }
        0xba92c => {
            if c.eax as u8 != 0 {
                return evaluate_ret(c, g, false, Some(1));
            }
            evaluate_import(c, g)
        }
        0xba94e => {
            c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x24))?);
            if c.eax as u8 == 0 {
                let not_range = g.u8(c.esp.wrapping_add(0x12))?;
                c.ecx = byte(c.ecx, not_range);
                if not_range == 0 {
                    // `ba9bf`.
                    let time = g.u8(c.esp.wrapping_add(0x13))?;
                    c.ecx = byte(c.ecx, time);
                    if time == 0 {
                        // `baa70`.
                        load(c, 0, slot(c, g, 0x14)?);
                        return evaluate_scale(c, g);
                    }
                    return evaluate_time(c, g);
                }
            }
            // `ba95e`.
            c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x13))?);
            if c.eax as u8 != 0 {
                return evaluate_time(c, g);
            }
            if g.u8(c.esi)? & 1 != 0 {
                load(c, 0, g.u32(ONE)?);
                let v = ss(c, Ss::Sub, 0, slot(c, g, 0x14)?)?;
                g.set_u32(c.esp.wrapping_add(0x14), v)?;
            }
            c.edx = slot(c, g, 0x14)?;
            push(c, g, ONE_BITS)?;
            push(c, g, c.edx)?;
            c.eax = c.esi.wrapping_add(0x14);
            call(c, FUNCTION, EVALUATE_SITES[2])
        }
        0xba991 => {
            c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x12))?);
            g.set_u32(c.esp.wrapping_add(0x14), lo(c, 0))?;
            if c.eax as u8 == 0 {
                // `baa33`.
                c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x24))?);
                return evaluate_scale(c, g);
            }
            let zero_bits = g.u32(ZERO)?;
            let al = u8::from(above(c, lo(c, 0), zero_bits)?);
            c.eax = byte(c.eax, al);
            g.set_u8(c.esp.wrapping_add(0x24), al)?;
            evaluate_scale(c, g)
        }
        0xbaa27 => {
            let v = ss(c, Ss::Mul, 0, slot(c, g, 0x14)?)?;
            g.set_u32(c.esp.wrapping_add(0x14), v)?;
            c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x24))?);
            evaluate_scale(c, g)
        }
        0xbaa4a => {
            let active = g.u8(c.esp.wrapping_add(0x24))?;
            c.ecx = byte(c.ecx, active);
            load(c, 0, slot(c, g, 0x18)?);
            let v = ss(c, Ss::Mul, 0, slot(c, g, 0x14)?)?;
            g.set_u32(c.esp.wrapping_add(0x14), v)?;
            if active == 0 || c.eax as u8 == 0 {
                // `baa93`.
                c.eax = byte(c.eax, 0);
                g.set_u8(c.esp.wrapping_add(0x24), 0)?;
                return evaluate_gate(c, g);
            }
            c.eax = byte(c.eax, 1);
            g.set_u8(c.esp.wrapping_add(0x24), 1)?;
            evaluate_threshold(c, g)
        }
        0xbaacc => {
            c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x13))?);
            if c.eax as u8 == 0 {
                zero(c, 0);
                return evaluate_ret(c, g, true, Some(0));
            }
            c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x24))?);
            load(c, 0, slot(c, g, 0x14)?);
            evaluate_clamp(c, g)
        }
        _ => Err(unowned(pc)),
    }
}

/// `ba93b`: the input import, `108d90(object, record +4)` with the value
/// out at `E-0c` (`ebx`) and the active flag at `E+4` (`edi`).
fn evaluate_import(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = g.u32(c.esi.wrapping_add(4))?;
    push(c, g, c.edi)?;
    push(c, g, c.ecx)?;
    push(c, g, c.ebp)?;
    c.edi = c.esp.wrapping_add(0x2c);
    c.ebx = c.esp.wrapping_add(0x1c);
    call(c, points::OBJECT_FUNCTION_IMPORT.base, EVALUATE_SITES[1])
}

/// `ba9cb`: the time input, `ticks * seconds per tick`, plus the hash of
/// the handle when record flags bit 3 is set; then the function on it.
fn evaluate_time(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(GAME_TIME)?;
    cvtsi2ss(c, g.u32(c.eax.wrapping_add(8))?)?;
    let v = ss(c, Ss::Mul, 0, g.u32(c.eax.wrapping_add(4))?)?;
    let hashed = g.u8(c.esi)? & 8 != 0;
    g.set_u32(c.esp.wrapping_add(0x18), v)?;
    if hashed {
        c.eax = c
            .ebp
            .wrapping_mul(imm(g, HASH_MUL)?)
            .wrapping_add(imm(g, HASH_ADD)?)
            >> 16;
        logic(c, c.eax, 1 << 31);
        g.set_u32(c.esp.wrapping_add(0x1c), c.eax)?;
        let mut x = fpu(c);
        x.fild(c.eax as i32).map_err(fp_error)?;
        if (c.eax as i32) < 0 {
            x.fadd_m(g.u32(TWO_32)?).map_err(fp_error)?;
        }
        x.fmul_m(g.u32(HASH_SCALE)?).map_err(fp_error)?;
        x.fadd_m(slot(c, g, 0x18)?).map_err(fp_error)?;
        let sum = x.fstp_m32().map_err(fp_error)?;
        let at = c.esp.wrapping_add(0x18);
        g.set_u32(at, sum)?;
        fpu_done(c, g, x, 0x000b_aa11, at)?;
    }
    c.ecx = slot(c, g, 0x18)?;
    push(c, g, ONE_BITS)?;
    push(c, g, c.ecx)?;
    c.eax = c.esi.wrapping_add(0x14);
    call(c, FUNCTION, EVALUATE_SITES[3])
}

/// `cvtsi2ss xmm0, i`.
fn cvtsi2ss(c: &mut CpuAbi, i: u32) -> Result<u32> {
    super::cvtsi2ss(c, 0, i)
}

/// `baa37`: the scale function value (`bab40` on record `+1c`).
fn evaluate_scale(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = g.u32(c.esi.wrapping_add(0x1c))?;
    if c.ecx != 0 {
        c.edx = c.esp.wrapping_add(0x18);
        push(c, g, c.edx)?;
        push(c, g, c.ecx)?;
        push(c, g, c.ebp)?;
        return call(c, points::OBJECT_FUNCTION_VALUE.base, EVALUATE_SITES[4]);
    }
    // `baa78`.
    if c.eax as u8 == 0 {
        return evaluate_gate(c, g);
    }
    evaluate_threshold(c, g)
}

/// `baa7c`: active only above the record's threshold (`+10`) when it is
/// positive.
fn evaluate_threshold(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 1, g.u32(c.esi.wrapping_add(0x10))?);
    let zero_bits = g.u32(ZERO)?;
    if !above(c, lo(c, 1), zero_bits)? {
        return evaluate_gate(c, g);
    }
    let al = u8::from(above(c, lo(c, 0), lo(c, 1))?);
    c.eax = byte(c.eax, al);
    g.set_u8(c.esp.wrapping_add(0x24), al)?;
    evaluate_gate(c, g)
}

/// `baa99`: record flags bit 2 forces active; then the gate import
/// (record `+c`).
fn evaluate_gate(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if g.u8(c.esi)? & 4 != 0 {
        c.eax = byte(c.eax, 1);
        g.set_u8(c.esp.wrapping_add(0x24), 1)?;
    }
    c.esi = g.u32(c.esi.wrapping_add(0x0c))?;
    if c.esi == 0 {
        // `baaf5`.
        return evaluate_clamp(c, g);
    }
    if c.eax as u8 == 0 {
        // `baaf9`.
        zero(c, 0);
        return evaluate_ret(c, g, true, None);
    }
    zero(c, 0);
    push(c, g, c.esi)?;
    push(c, g, c.ebp)?;
    c.edi = c.esp.wrapping_add(0x1b);
    c.ebx = c.esp.wrapping_add(0x20);
    g.set_u32(c.esp.wrapping_add(0x20), lo(c, 0))?;
    g.set_u8(c.esp.wrapping_add(0x1b), 0)?;
    call(c, points::OBJECT_FUNCTION_IMPORT.base, EVALUATE_SITES[5])
}

/// `baaf5`: zero when inactive, else the value clamped to `[0, 1]`.
fn evaluate_clamp(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if c.eax as u8 == 0 {
        zero(c, 0);
        return evaluate_ret(c, g, true, None);
    }
    zero(c, 1);
    if !above(c, lo(c, 1), lo(c, 0))? {
        load(c, 1, g.u32(ONE)?);
        if !above(c, lo(c, 0), lo(c, 1))? {
            return evaluate_ret(c, g, true, None);
        }
    }
    movaps(c, 0, 1);
    evaluate_ret(c, g, true, None)
}

/// `108d90 (object: stack 0, name: stack 1, value: ebx, active: edi) -> al`.
pub(in crate::sim_objects) fn import(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::OBJECT_FUNCTION_IMPORT.base {
        c.ecx = g.u32(OBJECTS)?;
        c.edx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
        zero(c, 0);
        push(c, g, c.ebp)?;
        c.ebp = slot(c, g, 8)?;
        c.eax = (c.ebp & 0xffff) * 3;
        c.eax = g.u32(c.edx.wrapping_add(c.eax * 4).wrapping_add(8))?;
        c.ecx = g.u8(c.eax.wrapping_add(0xaa))? as i8 as i32 as u32;
        c.ecx = g.u32(TYPES.wrapping_add(c.ecx.wrapping_mul(4)))?;
        c.eax = byte(c.eax, 0);
        push(c, g, c.esi)?;
        g.set_u32(c.ebx, lo(c, 0))?;
        g.set_u8(c.edi, c.eax as u8)?;
        c.esi = c.ecx.wrapping_add(0x84);
    } else if pc == IMPORT_SITES[0] {
        c.esi = c.esi.wrapping_add(4);
        test8(c, c.eax);
        if c.eax as u8 != 0 {
            return import_ret(c, g);
        }
    } else {
        return Err(unowned(pc));
    }
    loop {
        c.ecx = g.u32(c.esi)?;
        if c.ecx == 0 {
            // `108df3`.
            if c.eax as u8 == 0 {
                zero(c, 0);
                g.set_u32(c.ebx, lo(c, 0))?;
                g.set_u8(c.edi, c.eax as u8)?;
            }
            return import_ret(c, g);
        }
        c.ecx = g.u32(c.ecx.wrapping_add(0x4c))?;
        if c.ecx != 0 {
            c.edx = slot(c, g, 0x10)?;
            push(c, g, c.edi)?;
            push(c, g, c.ebx)?;
            push(c, g, c.edx)?;
            push(c, g, c.ebp)?;
            return call(c, c.ecx, IMPORT_SITES[0]);
        }
        // `108de7` without a call: `al` is still the entry's zero.
        c.esi = c.esi.wrapping_add(4);
        if c.eax as u8 != 0 {
            return import_ret(c, g);
        }
    }
}

fn import_ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    ret(c, 8)
}
