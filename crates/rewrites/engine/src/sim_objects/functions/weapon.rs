//! `ff5f0` the weapon import handler `(object: stack 0, name: stack 1,
//! value out: stack 2, active out: stack 3) -> al; ret 10`
//! (docs/engine/object-update.md, "Slice 3 bodies"): slot `+4c` of the
//! `weap` descriptor, also called directly by the unit import `c6b90`
//! (at `c6dc8`) for the held weapon.
//!
//! A binary search on the name (signed compares against string IDs read
//! from the instruction bytes) picks one weapon value: a body float, a
//! barrel's float (body `+1b4`/`+1b8` + `34 × i`, definition `+2d0` the
//! barrel count), a trigger's value (`102070`, definition `+2c8` count,
//! `+2cc` records of `40` bytes, body `+20c` state of `0c` bytes), the
//! maximum over barrels and triggers, a magazine's fraction or a digit of
//! its rounds (body `+22c` words of `10` bytes, definition `+2c0` count,
//! `+2c4` records of `5c` bytes), a body flag as 0 or 1, or a multiplayer
//! value (`162b10`, `162b70`). Most values are clamped to `[0, 1]`
//! (`ffc16`). Every recognised name stores the value at `[stack 2]` and
//! "value > 0" at `[stack 3]` and returns `al = 1` (`eax = 1`); an unknown
//! name returns `al = 0` and stores nothing.
//!
//! Registers kept across calls: `ebp` the body, `edi` the object, `esi`
//! the name (or the trigger record cursor in the maximum), `ebx` the
//! definition (or the trigger state cursor); `xmm2` the value so far,
//! `xmm3` zero, `xmm4` one.
//!
//! Frame (`E` the entry `esp`, after the five pushes `esp = E - 14`):
//! `[esp+10]` the definition (over the saved `ecx`), `[esp+18]` the
//! object, `[esp+1c]` the name, overwritten with the trigger count by the
//! maximum, `[esp+20]` the value out, `[esp+24]` the active out.

use super::{
    Ss, above, byte, call, cvtsi2ss, imm, load, lo, movaps, pop, push, ret, slot, ss,
    ucomiss_lahf_jp, unowned, zero, OBJECTS, ONE, TAGS,
};
use guest_layout::datum::layout;
use crate::sim_objects::{Cx, Next, Result};
use runtime::{CpuAbi, Guest};

const ENTRY: u32 = 0x000f_f5f0;

/// A trigger's value `(object: eax, trigger: stack 0)`, `xmm0` out; `ret 4`.
const TRIGGER_VALUE: u32 = 0x0010_2070;
/// `(object: ecx) -> al`; `ret`.
const FLAG_10CF50: u32 = 0x0010_cf50;
/// Multiplayer gate `(object: ecx) -> al`; `ret`.
const MP_GATE: u32 = 0x0016_2b10;
/// Multiplayer value `(object: stack 0)`, `xmm0` out; `ret 4`.
const MP_VALUE: u32 = 0x0016_2b70;

/// A global whose byte `+82`, when set, answers zero for the `ff66d` name.
const GLOBAL_4DE2F4: u32 = 0x004d_e2f4;
/// The game globals; `+8` the game mode (2 multiplayer).
const GAME_GLOBALS: u32 = 0x004e_6948;
/// Game time: `+8` the tick (i32).
const GAME_TIME: u32 = 0x0051_0c54;
/// The scale of the byte at body `+175` (f32).
const BYTE_SCALE: u32 = 0x0045_dbb4;
/// One tenth (f32), the rounds digit scale.
const TENTH: u32 = 0x0045_dc68;

// The imm32 of `cmp esi, imm` (or `sub eax, imm`) at the instruction two
// (one) bytes before each address: the string IDs the name is compared
// with.
const NAME_FF632: u32 = 0x000f_f632; // cmp esi at ff630, jle ff7aa
const NAME_FF649: u32 = 0x000f_f649; // cmp esi at ff647, jle ffa0f
const NAME_FF655: u32 = 0x000f_f655; // cmp esi at ff653, jle ffb2e
const NAME_FF661: u32 = 0x000f_f661; // cmp esi at ff65f, je ffbf5
const NAME_FF66D: u32 = 0x000f_f66d; // cmp esi at ff66b, jne ff900
const NAME_FF7B2: u32 = 0x000f_f7b2; // cmp esi at ff7b0
const NAME_FF7C4: u32 = 0x000f_f7c4; // cmp esi at ff7c2
const NAME_FF7CE: u32 = 0x000f_f7ce; // cmp esi at ff7cc
const NAME_FF7D6: u32 = 0x000f_f7d6; // cmp esi at ff7d4
const NAME_FF7DE: u32 = 0x000f_f7de; // cmp esi at ff7dc
const NAME_FF82D: u32 = 0x000f_f82d; // cmp esi at ff82b
const NAME_FF835: u32 = 0x000f_f835; // cmp esi at ff833
const NAME_FF8D8: u32 = 0x000f_f8d8; // cmp esi at ff8d6
const NAME_FF8E2: u32 = 0x000f_f8e2; // cmp esi at ff8e0
const NAME_FF8EE: u32 = 0x000f_f8ee; // cmp esi at ff8ec
const NAME_FF8F6: u32 = 0x000f_f8f6; // cmp esi at ff8f4
const NAME_FF90C: u32 = 0x000f_f90c; // cmp esi at ff90a
const NAME_FF918: u32 = 0x000f_f918; // cmp esi at ff916
const NAME_FF955: u32 = 0x000f_f955; // cmp esi at ff953
const NAME_FF977: u32 = 0x000f_f977; // cmp esi at ff975
const NAME_FF9A1: u32 = 0x000f_f9a1; // cmp esi at ff99f
const NAME_FF9D2: u32 = 0x000f_f9d2; // cmp esi at ff9d0 (setne)
const NAME_FFA17: u32 = 0x000f_fa17; // cmp esi at ffa15
const NAME_FFA29: u32 = 0x000f_fa29; // cmp esi at ffa27
const NAME_FFA31: u32 = 0x000f_fa31; // cmp esi at ffa2f
const NAME_FFA39: u32 = 0x000f_fa39; // cmp esi at ffa37
const NAME_FFA8A: u32 = 0x000f_fa8a; // cmp esi at ffa88 (setne)
const NAME_FFAB3: u32 = 0x000f_fab3; // cmp esi at ffab1 (setne)
const NAME_FFAFE: u32 = 0x000f_fafe; // sub eax at ffafd
const NAME_FFB37: u32 = 0x000f_fb37; // sub eax at ffb36
const NAME_FFB43: u32 = 0x000f_fb43; // sub eax at ffb42 (a difference)
const NAME_FFBA4: u32 = 0x000f_fba4; // cmp esi at ffba2 (setne)
const NAME_FFBFF: u32 = 0x000f_fbff; // cmp esi at ffbfd (setne)
/// The imm8 of `sub eax, imm8` at `ffb08` and `ffb3d` (differences
/// between string IDs, sign-extended).
const STEP_FFB0A: u32 = 0x000f_fb0a;
const STEP_FFB3F: u32 = 0x000f_fb3f;

pub(in crate::sim_objects) const WEAPON_SITES: [u32; 8] =
    [0xff74a, 0xff7ef, 0xff88c, 0xff945, 0xff953, 0xffa6a, 0xffa78, 0xffaa1];

/// A string ID as the signed compares see it.
fn id(g: &Guest, at: u32) -> Result<i32> {
    Ok(imm(g, at)? as i32)
}

/// A sign-extended imm8.
fn step(g: &Guest, at: u32) -> Result<u32> {
    Ok(g.u8(at)? as i8 as i32 as u32)
}

/// `setne` after `xor r, r; cmp esi, imm`: 0 or 1.
fn not_name(c: &CpuAbi, g: &Guest, at: u32) -> Result<u32> {
    Ok(u32::from(c.esi != imm(g, at)?))
}

/// `ff5f0 (object: stack 0, name: stack 1, value: stack 2, active: stack
/// 3) -> al`.
pub(in crate::sim_objects) fn weapon(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        ENTRY => {
            push(c, g, c.ecx)?;
            c.ecx = g.u32(OBJECTS)?;
            c.edx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
            zero(c, 3);
            load(c, 4, g.u32(ONE)?);
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            push(c, g, c.esi)?;
            c.esi = slot(c, g, 0x18)?;
            push(c, g, c.edi)?;
            c.edi = slot(c, g, 0x18)?;
            c.eax = (c.edi & 0xffff) * 3;
            c.ebp = g.u32(c.edx.wrapping_add(c.eax * 4).wrapping_add(8))?;
            c.ecx = g.u32(c.ebp)?;
            c.eax = g.u32(TAGS)?;
            c.ecx = (c.ecx & 0xffff) << 4;
            c.ebx = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
            g.set_u32(c.esp.wrapping_add(0x10), c.ebx)?;
            movaps(c, 2, 3);
            names(c, g)
        }
        0xff74a => {
            let v = g.u32(c.esi)?;
            ss(c, Ss::Mul, 0, v)?;
            if !above(c, lo(c, 2), lo(c, 0))? {
                movaps(c, 2, 0);
            }
            triggers(c, g, true)
        }
        0xff7ef => {
            if c.eax as u8 != 0 {
                load(c, 2, g.u32(ONE)?);
            }
            finish(c, g)
        }
        0xff88c => {
            if c.eax as u8 != 0 {
                let al = g.u8(c.ebp.wrapping_add(0x16c))? >> 5;
                c.eax = byte(c.eax, al);
                if al & 1 == 0 {
                    return finish(c, g);
                }
            }
            // `ff8a1`.
            load(c, 0, g.u32(ONE)?);
            let m = g.u32(c.ebp.wrapping_add(0x184))?;
            if above(c, lo(c, 0), m)? {
                movaps(c, 2, 0);
            }
            finish(c, g)
        }
        0xff945 => mp_value(c, g, WEAPON_SITES[4]),
        0xff953 => mp_compare(c, g),
        0xffa6a => mp_value(c, g, WEAPON_SITES[6]),
        0xffa78 | 0xffaa1 => {
            movaps(c, 2, 0);
            clamp(c, g)
        }
        _ => Err(unowned(pc)),
    }
}

/// `ff900`: an unknown name, `al = 0`, nothing stored.
fn fail(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.eax = byte(c.eax, 0);
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    ret(c, 0x10)
}

/// `ffc16`: `xmm2` clamped to `[xmm3, xmm4]` (0 and 1).
fn clamp(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if above(c, lo(c, 3), lo(c, 2))? {
        movaps(c, 2, 3);
    } else if above(c, lo(c, 2), lo(c, 4))? {
        movaps(c, 2, 4);
    }
    finish(c, g)
}

/// `ffc28`: store `xmm2` to the value out and "above zero" to the active
/// out; `eax = 1` on both paths (`mov eax, 1`, or `xor eax, eax` then
/// `mov al, 1`).
fn finish(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let active = above(c, lo(c, 2), lo(c, 3))?;
    c.edx = slot(c, g, 0x20)?;
    g.set_u32(c.edx, lo(c, 2))?;
    c.ecx = slot(c, g, 0x24)?;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.eax = u32::from(active);
    c.ebp = pop(c, g)?;
    g.set_u8(c.ecx, c.eax as u8)?;
    c.eax = 1;
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    ret(c, 0x10)
}

/// `ff63e`: the top of the name search.
fn names(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.esi as i32;
    if n <= id(g, NAME_FF632)? {
        return names_low(c, g);
    }
    if n <= id(g, NAME_FF649)? {
        return names_mid(c, g);
    }
    if n <= id(g, NAME_FF655)? {
        return names_high(c, g);
    }
    if n == id(g, NAME_FF661)? {
        return barrel_1b8(c, g);
    }
    if n != id(g, NAME_FF66D)? {
        return fail(c, g);
    }
    // `ff677`.
    c.edx = g.u32(GLOBAL_4DE2F4)?;
    let cl = g.u8(c.edx.wrapping_add(0x82))?;
    c.ecx = byte(c.ecx, cl);
    if cl != 0 {
        return finish(c, g);
    }
    c.ecx = (g.u32(c.ebp)? & 0xffff) << 4;
    c.ebx = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
    g.set_u32(c.esp.wrapping_add(0x10), c.ebx)?;
    maximum(c, g)
}

/// `ff7aa`: names at or below `NAME_FF632`.
fn names_low(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.esi as i32;
    if n == id(g, NAME_FF632)? {
        return barrel_age(c, g);
    }
    let m = id(g, NAME_FF7B2)?;
    if n > m {
        return names_low_upper(c, g);
    }
    if n == m {
        // `ff8be`.
        c.ecx = u32::from(g.u8(c.ebp.wrapping_add(0x175))?);
        cvtsi2ss(c, 2, c.ecx)?;
        ss(c, Ss::Mul, 2, g.u32(BYTE_SCALE)?)?;
        return clamp(c, g);
    }
    let m = id(g, NAME_FF7C4)?;
    if n > m {
        return names_ff82b(c, g);
    }
    if n == m {
        // `ff81e`.
        load(c, 2, g.u32(c.ebp.wrapping_add(0x18c))?);
        return clamp(c, g);
    }
    if n == id(g, NAME_FF7CE)? {
        // `ff811`.
        load(c, 2, g.u32(c.ebp.wrapping_add(0x184))?);
        return clamp(c, g);
    }
    if n == id(g, NAME_FF7D6)? {
        // `ff804`.
        load(c, 2, g.u32(c.ebp.wrapping_add(0x180))?);
        return clamp(c, g);
    }
    if n != id(g, NAME_FF7DE)? {
        return fail(c, g);
    }
    // `ff7e8`.
    c.ecx = c.edi;
    call(c, FLAG_10CF50, WEAPON_SITES[1])
}

/// `ff82b`.
fn names_ff82b(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.esi as i32;
    if n == id(g, NAME_FF82D)? {
        // `ff885`.
        c.ecx = c.edi;
        return call(c, FLAG_10CF50, WEAPON_SITES[2]);
    }
    if n != id(g, NAME_FF835)? {
        return fail(c, g);
    }
    // `ff83f`: body `+16c` bit 1, then `(+180 - d) / (1 - d)` with `d`
    // definition `+150`, unless `d` is 1.
    let dl = g.u8(c.ebp.wrapping_add(0x16c))? >> 1;
    c.edx = byte(c.edx, dl);
    if dl & 1 == 0 {
        return finish(c, g);
    }
    load(c, 0, g.u32(c.ebx.wrapping_add(0x150))?);
    // `jnp`: taken when ordered and equal.
    if !ucomiss_lahf_jp(c, lo(c, 0), lo(c, 4))? {
        return finish(c, g);
    }
    load(c, 2, g.u32(c.ebp.wrapping_add(0x180))?);
    ss(c, Ss::Sub, 2, lo(c, 0))?;
    movaps(c, 0, 4);
    ss(c, Ss::Sub, 0, g.u32(c.ebx.wrapping_add(0x150))?)?;
    ss(c, Ss::Div, 2, lo(c, 0))?;
    clamp(c, g)
}

/// `ff8d6`: names above `NAME_FF7B2` up to `NAME_FF632`.
fn names_low_upper(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.esi as i32;
    let m = id(g, NAME_FF8D8)?;
    if n > m {
        // `ff90a`.
        if n == id(g, NAME_FF90C)? {
            return trigger(c, g);
        }
        if n != id(g, NAME_FF918)? {
            return fail(c, g);
        }
        return multiplayer(c, g, WEAPON_SITES[3]);
    }
    if n == m {
        return multiplayer(c, g, WEAPON_SITES[3]);
    }
    if n == id(g, NAME_FF8E2)? {
        return maximum(c, g);
    }
    if n == id(g, NAME_FF8EE)? {
        return multiplayer(c, g, WEAPON_SITES[3]);
    }
    if n == id(g, NAME_FF8F6)? {
        return barrel_age(c, g);
    }
    fail(c, g)
}

/// `ffa0f`: names above `NAME_FF632` up to `NAME_FF649`.
fn names_mid(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let n = c.esi as i32;
    if n == id(g, NAME_FF649)? {
        return barrel_1b8(c, g);
    }
    let m = id(g, NAME_FFA17)?;
    if n > m {
        // `ffafb`.
        c.eax = c.esi.wrapping_sub(imm(g, NAME_FFAFE)?);
        if c.eax == 0 {
            return barrel_1b4(c, g);
        }
        c.eax = c.eax.wrapping_sub(step(g, STEP_FFB0A)?);
        if c.eax != 0 {
            return fail(c, g);
        }
        // `ffb11`: body `+16e` bit 1.
        let al = g.u8(c.ebp.wrapping_add(0x16e))? >> 1;
        c.eax = byte(c.eax, al);
        if al & 1 != 0 {
            load(c, 2, g.u32(ONE)?);
        }
        return finish(c, g);
    }
    if n == m {
        return magazine(c, g);
    }
    if n == id(g, NAME_FFA29)? {
        return trigger(c, g);
    }
    if n == id(g, NAME_FFA31)? {
        return magazine(c, g);
    }
    if n != id(g, NAME_FFA39)? {
        return fail(c, g);
    }
    // `ffa43`.
    multiplayer(c, g, WEAPON_SITES[5])
}

/// `ffb2e`: names above `NAME_FF649` up to `NAME_FF655`.
fn names_high(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if c.esi as i32 == id(g, NAME_FF655)? {
        return rounds_tens(c, g);
    }
    c.eax = c.esi.wrapping_sub(imm(g, NAME_FFB37)?);
    if c.eax == 0 {
        return barrel_1b4(c, g);
    }
    c.eax = c.eax.wrapping_sub(step(g, STEP_FFB3F)?);
    if c.eax == 0 {
        // `ffb7b`: body `+16e` bit 2.
        let dl = g.u8(c.ebp.wrapping_add(0x16e))? >> 2;
        c.edx = byte(c.edx, dl);
        if dl & 1 != 0 {
            load(c, 2, g.u32(ONE)?);
        }
        return finish(c, g);
    }
    c.eax = c.eax.wrapping_sub(imm(g, NAME_FFB43)?);
    if c.eax != 0 {
        return fail(c, g);
    }
    // `ffb4d`: the ones digit of magazine 0's rounds, times 0.1.
    c.eax = g.u32(c.ebx.wrapping_add(0x2c0))?;
    if c.eax as i32 <= 0 {
        return finish(c, g);
    }
    c.eax = g.u16(c.ebp.wrapping_add(0x22c))? as i16 as i32 as u32;
    divide_by_ten(c);
    cvtsi2ss(c, 2, c.edx)?;
    ss(c, Ss::Mul, 2, g.u32(TENTH)?)?;
    clamp(c, g)
}

/// `cdq; mov ecx, 0Ah; idiv ecx`.
fn divide_by_ten(c: &mut CpuAbi) {
    let a = c.eax as i32;
    c.ecx = 10;
    c.eax = (a / 10) as u32;
    c.edx = (a % 10) as u32;
}

/// `ffbbd`: the tens digit of magazine 0's rounds, times 0.1.
fn rounds_tens(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(c.ebx.wrapping_add(0x2c0))?;
    if c.eax as i32 <= 0 {
        return finish(c, g);
    }
    c.ecx = g.u16(c.ebp.wrapping_add(0x22c))? as i16 as i32 as u32;
    // `mov eax, 66666667h; imul ecx; sar edx, 2; mov eax, edx; shr eax,
    // 1Fh; add eax, edx`: the quotient by ten.
    let product = i64::from(c.ecx as i32) * 0x6666_6667;
    c.edx = (product >> 32) as u32;
    c.edx = ((c.edx as i32) >> 2) as u32;
    c.eax = (c.edx >> 31).wrapping_add(c.edx);
    divide_by_ten(c);
    cvtsi2ss(c, 2, c.edx)?;
    ss(c, Ss::Mul, 2, g.u32(TENTH)?)?;
    clamp(c, g)
}

/// `ffbf5`: barrel 0 or 1's float at body `+1b8`.
fn barrel_1b8(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = g.u32(c.ebx.wrapping_add(0x2d0))?;
    c.eax = not_name(c, g, NAME_FFBFF)?;
    if c.eax as i32 >= c.ecx as i32 {
        return finish(c, g);
    }
    c.eax = c.eax.wrapping_mul(0x34);
    load(c, 2, g.u32(c.eax.wrapping_add(c.ebp).wrapping_add(0x1b8))?);
    clamp(c, g)
}

/// `ffb9a`: barrel 0 or 1's float at body `+1b4`.
fn barrel_1b4(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = g.u32(c.ebx.wrapping_add(0x2d0))?;
    c.eax = not_name(c, g, NAME_FFBA4)?;
    if c.eax as i32 >= c.ecx as i32 {
        return finish(c, g);
    }
    c.eax = c.eax.wrapping_mul(0x34);
    load(c, 2, g.u32(c.eax.wrapping_add(c.ebp).wrapping_add(0x1b4))?);
    clamp(c, g)
}

/// `ff9c8`: barrel 0 or 1's float at body `+1b4`, zero unless the body's
/// tick at `+248` is at most one tick old.
fn barrel_age(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = g.u32(c.ebx.wrapping_add(0x2d0))?;
    c.eax = not_name(c, g, NAME_FF9D2)?;
    if c.eax as i32 >= c.ecx as i32 {
        return finish(c, g);
    }
    c.ecx = g.u32(GAME_TIME)?;
    c.eax = c.eax.wrapping_mul(0x34);
    c.edx = g.u32(c.ecx.wrapping_add(8))?;
    c.ecx = g.u32(c.ebp.wrapping_add(0x248))?;
    load(c, 2, g.u32(c.eax.wrapping_add(c.ebp).wrapping_add(0x1b4))?);
    c.edx = c.edx.wrapping_sub(c.ecx);
    if c.edx as i32 <= 1 {
        return clamp(c, g);
    }
    movaps(c, 2, 3);
    finish(c, g)
}

/// `ffa80`: trigger 0 or 1's value (`102070`).
fn trigger(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = g.u32(c.ebx.wrapping_add(0x2c8))?;
    c.eax = not_name(c, g, NAME_FFA8A)?;
    if c.eax as i32 >= c.ecx as i32 {
        return finish(c, g);
    }
    push(c, g, c.eax)?;
    c.eax = c.edi;
    call(c, TRIGGER_VALUE, WEAPON_SITES[7])
}

/// `ffaa9`: magazine 0 or 1's rounds (body `+22c + 10 × i`, i16) over its
/// definition's word at `+0a` (records `+2c4`, `5c` bytes), when non-zero.
fn magazine(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(c.ebx.wrapping_add(0x2c0))?;
    c.ecx = not_name(c, g, NAME_FFAB3)?;
    if c.ecx as i32 >= c.eax as i32 {
        return finish(c, g);
    }
    c.esi = g.u32(c.ebx.wrapping_add(0x2c4))?;
    c.eax = c.ecx.wrapping_mul(0x5c).wrapping_add(c.esi);
    let ax = g.u16(c.eax.wrapping_add(0x0a))?;
    c.eax = (c.eax & 0xffff_0000) | u32::from(ax);
    if ax == 0 {
        return finish(c, g);
    }
    c.ecx <<= 4;
    c.ecx = g.u16(c.ecx.wrapping_add(c.ebp).wrapping_add(0x22c))? as i16 as i32 as u32;
    c.edx = ax as i16 as i32 as u32;
    cvtsi2ss(c, 2, c.ecx)?;
    cvtsi2ss(c, 0, c.edx)?;
    ss(c, Ss::Div, 2, lo(c, 0))?;
    clamp(c, g)
}

/// `ff91e` and `ffa43`: zero unless the game mode is 2, body `+17e` is
/// not `ffff` and `162b10(object)` answers; then `162b70(object)`.
/// Returns to `site` (`ff945` or `ffa6a`).
fn multiplayer(c: &mut CpuAbi, g: &mut Guest, site: u32) -> Result<Next> {
    c.eax = g.u32(GAME_GLOBALS)?;
    let mode = g.u32(c.eax.wrapping_add(8))?;
    movaps(c, 2, 3);
    if mode != 2 {
        return finish(c, g);
    }
    if g.u16(c.ebp.wrapping_add(0x17e))? == 0xffff {
        return finish(c, g);
    }
    c.ecx = c.edi;
    call(c, MP_GATE, site)
}

/// `ff945` and `ffa6a`: after the gate, the value (`162b70`), returning
/// to `site` (`ff953` or `ffa78`).
fn mp_value(c: &mut CpuAbi, g: &mut Guest, site: u32) -> Result<Next> {
    if c.eax as u8 == 0 {
        return finish(c, g);
    }
    push(c, g, c.edi)?;
    call(c, MP_VALUE, site)
}

/// `ff953`: the three names of `ff91e` make the value 0 or 1: equal to
/// zero, strictly inside `(0, 1)`, or equal to one (each `xmm0` against
/// the constant; NaN fails all three).
fn mp_compare(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if c.esi == imm(g, NAME_FF955)? {
        if ucomiss_lahf_jp(c, lo(c, 0), lo(c, 3))? {
            return finish(c, g);
        }
        load(c, 2, g.u32(ONE)?);
        return finish(c, g);
    }
    if c.esi == imm(g, NAME_FF977)? {
        if !above(c, lo(c, 0), lo(c, 3))? {
            return finish(c, g);
        }
        load(c, 1, g.u32(ONE)?);
        if !above(c, lo(c, 1), lo(c, 0))? {
            return finish(c, g);
        }
        movaps(c, 2, 1);
        return finish(c, g);
    }
    if c.esi != imm(g, NAME_FF9A1)? {
        return finish(c, g);
    }
    load(c, 1, g.u32(ONE)?);
    if ucomiss_lahf_jp(c, lo(c, 0), lo(c, 1))? {
        return finish(c, g);
    }
    movaps(c, 2, 1);
    finish(c, g)
}

/// `ff69f`: the maximum of every barrel's float at body `+1bc + 34 × i`
/// (definition `+2d0` barrels, four at a time then one at a time), then
/// of the triggers.
fn maximum(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = g.u32(c.ebx.wrapping_add(0x2d0))?;
    c.esi = 0;
    if c.edx as i32 >= 4 {
        c.ecx = (c.edx.wrapping_sub(4) >> 2).wrapping_add(1);
        c.eax = c.ebp.wrapping_add(0x1f0);
        c.esi = c.ecx.wrapping_mul(4);
        loop {
            for off in [0u32.wrapping_sub(0x34), 0, 0x34, 0x68] {
                let v = g.u32(c.eax.wrapping_add(off))?;
                if !above(c, lo(c, 2), v)? {
                    load(c, 2, v);
                }
            }
            c.eax = c.eax.wrapping_add(0xd0);
            c.ecx = c.ecx.wrapping_sub(1);
            if c.ecx == 0 {
                break;
            }
        }
    }
    // `ff6f2`.
    if (c.esi as i32) < (c.edx as i32) {
        c.eax = c.esi.wrapping_mul(0x34).wrapping_add(c.ebp).wrapping_add(0x1bc);
        c.edx = c.edx.wrapping_sub(c.esi);
        loop {
            let v = g.u32(c.eax)?;
            if !above(c, lo(c, 2), v)? {
                load(c, 2, v);
            }
            c.eax = c.eax.wrapping_add(0x34);
            c.edx = c.edx.wrapping_sub(1);
            if c.edx == 0 {
                break;
            }
        }
    }
    // `ff713`: the triggers (definition `+2c8` count, `+2cc` records of
    // `40` bytes; body `+20c` states of `0c` bytes).
    c.ebx = g.u32(c.ebx.wrapping_add(0x2c8))?;
    c.edi = 0;
    g.set_u32(c.esp.wrapping_add(0x1c), c.ebx)?;
    if c.ebx as i32 <= 0 {
        return heat(c, g);
    }
    c.ecx = slot(c, g, 0x10)?;
    c.esi = g.u32(c.ecx.wrapping_add(0x2cc))?.wrapping_add(0x28);
    c.ebx = c.ebp.wrapping_add(0x20c);
    triggers(c, g, false)
}

/// `ff736` (from the top when `resumed` is false) or `ff756` (after the
/// `102070` value): per trigger, its value times record `+28` when record
/// `+1c` is positive, and for a state of 2 `f + (1 - f) × body +188` with
/// `f` record `+28`.
fn triggers(c: &mut CpuAbi, g: &mut Guest, mut resumed: bool) -> Result<Next> {
    loop {
        if !resumed {
            // `ff736`.
            load(c, 0, g.u32(c.esi.wrapping_sub(0x0c))?);
            if above(c, lo(c, 0), lo(c, 3))? {
                c.eax = slot(c, g, 0x18)?;
                push(c, g, c.edi)?;
                return call(c, TRIGGER_VALUE, WEAPON_SITES[0]);
            }
        }
        resumed = false;
        // `ff756`.
        if g.u8(c.ebx)? == 2 {
            movaps(c, 0, 4);
            ss(c, Ss::Sub, 0, g.u32(c.esi)?)?;
            ss(c, Ss::Mul, 0, g.u32(c.ebp.wrapping_add(0x188))?)?;
            ss(c, Ss::Add, 0, g.u32(c.esi)?)?;
            if !above(c, lo(c, 2), lo(c, 0))? {
                movaps(c, 2, 0);
            }
        }
        // `ff776`.
        c.eax = slot(c, g, 0x1c)?;
        c.edi = c.edi.wrapping_add(1);
        c.esi = c.esi.wrapping_add(0x40);
        c.ebx = c.ebx.wrapping_add(0x0c);
        if (c.edi as i32) >= (c.eax as i32) {
            return heat(c, g);
        }
    }
}

/// `ff785`: then body `+180` times definition `+164`, and clamp.
fn heat(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = slot(c, g, 0x10)?;
    load(c, 0, g.u32(c.ebp.wrapping_add(0x180))?);
    ss(c, Ss::Mul, 0, g.u32(c.edx.wrapping_add(0x164))?)?;
    if !above(c, lo(c, 2), lo(c, 0))? {
        movaps(c, 2, 0);
    }
    clamp(c, g)
}
