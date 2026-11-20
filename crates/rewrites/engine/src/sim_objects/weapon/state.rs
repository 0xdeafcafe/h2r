//! `fdfb0..fe5ff`: the frame and the weapon-wide state, in order (field
//! names L, from what the code does with them):
//!
//! 1. `101690(object)` (`ret 4`): `bl`, the first result.
//! 2. `d1490` with `eax` body `+154` when body `+12c` bit 0 (else none) and
//!    `esi` the object: a value in `xmm0`, kept at `[S+38]`; replaced by
//!    1.0 or 0.0 (on `bl`) when definition `+12c` bit 27 is set.
//! 3. The owner: `101fb0(eax: object)`, and when it answers, that
//!    object's `+13c` into `[S+30]`. Body `+194` (a handle) is checked
//!    with `badc0(-1)` and cleared to none when it no longer resolves.
//! 4. `105e80(object)` (`ret 4`): `bl` again, kept at `[S+13]`.
//! 5. The animation block at body `+12a` (when the re-read body has one):
//!    `1daea0` on its graph, `1cb710(bf600, object, 0, 0, 0)`, then
//!    `105740(eax)` or `105800(ecx)` by the block's flags, and the result
//!    set when the block is still playing.
//! 6. The countdown at body `+19e` unless body `+16c` bit 5; at zero it
//!    sets that bit.
//! 7. `10d4e0(edx: object)` when definition `+12c` bit 10, no parent and
//!    in the world; `105840(ecx: object)` when body `+16e` bit 7.
//! 8. The value at body `+18c` moves toward body `+190` (toggled between
//!    0 and 1 when body `+16e` bit 0) by definition `+2d8` or `+2dc` ×
//!    the tick length, with `1039a0(-1, 0, 0; ecx: object)` the first
//!    time (body `+16c` bit 4) when definition `+248`/`+250` is set.
//! 9. The heat at body `+180`: above zero and at or over definition
//!    `+154` it starts overheating (`101f20`, `c86e0(0)` when that
//!    answers, `105fa0(0e)` unless definition `+292` is 3 with `+16c`
//!    bit 3); it cools by definition `+160` or `+168` × the tick
//!    (scaled by `1 - body +184 × definition +254` when that is
//!    positive) while body `+188` is zero, and `1685a6(8000071, 1; ecx:
//!    object)` decides when the overheat ends (`105fa0(0f)`).
//! 10. The effect at body `+244` follows body `+16c` bit 1 against body
//!     `+12c` bit 1 (`103a60` makes it, `177610` or `177260(1)` ends it)
//!     when definition `+170` is set.
//! 11. Body `+188` cleared, the timer at body `+17c` counted down, the
//!     body `+16e` bits copied to the frame bytes `[S+1c..+21]`, and
//!     `[S+16]` set (with body `+174` cleared) when the weapon is not
//!     ready (body `+16e` bit 5, `+17c` positive, or `+16c` bit 1).
//! 12. `fdd50(object, i)` (`ret 8`) for each of the definition's `+2c0`
//!     entries, OR-ed into the result.

use super::*;

pub(super) fn step(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> R {
    match pc {
        0xfdfb0 => {
            c.esp = arithmetic(c, c.esp, 0x38, true);
            c.ecx = slot(c, g, 0x3c)?;
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            push(c, g, c.esi)?;
            c.eax = c.ecx & 0xffff;
            logic(c, c.eax, 1 << 31);
            push(c, g, c.edi)?;
            c.edi = g.u32(points::OBJECTS.base)?;
            c.esi = c.eax.wrapping_mul(3);
            c.eax = g.u32(at(c.edi, 0x44))?;
            c.esi <<= 2;
            c.ebp = g.u32(at(at(c.esi, c.eax), 8))?;
            c.edx = g.u32(c.ebp)?;
            c.eax = g.u32(points::TAG_ENTRIES.base)?;
            c.edx &= 0xffff;
            c.edx = super::super::alu::shl(c, c.edx, 4, 32);
            c.edx = g.u32(at(at(c.edx, c.eax), 8))?;
            push(c, g, c.ecx)?;
            set_slot(c, g, 0x28, c.esi)?;
            set_slot(c, g, 0x38, c.ebp)?;
            set_slot(c, g, 0x30, c.edx)?;
            call(0x101690, 0xfdffc)
        }
        0xfdffc => {
            c.ebx = byte(c.ebx, c.eax as u8);
            c.eax = g.u32(at(c.edi, 0x44))?;
            c.ecx = g.u32(at(at(c.esi, c.eax), 8))?;
            c.edx = byte(c.edx, g.u8(at(c.ecx, 0x12c))?);
            c.eax = NONE;
            if test8(c, c.edx & 1) {
                c.eax = g.u32(at(c.ecx, 0x154))?;
            }
            c.esi = slot(c, g, 0x4c)?;
            call(0xd1490, 0xfe022)
        }
        0xfe022 => {
            c.ecx = slot(c, g, 0x2c)?;
            c.edx = g.u32(at(c.ecx, 0x12c))? >> 0x1b;
            let set = test8(c, c.edx & 1);
            store_ss(c, g, at(c.esp, 0x38), 0)?;
            set_slot8(c, g, 0x16, 0)?;
            set_slot(c, g, 0x30, NONE)?;
            if set {
                let bl = test8(c, c.ebx);
                sse::load(c, 0, g.u32(ONE)?);
                if !bl {
                    sse::zero(c, 0);
                }
                store_ss(c, g, at(c.esp, 0x38), 0)?;
            }
            c.ebx = slot(c, g, 0x4c)?;
            c.eax = c.ebx;
            call(0x101fb0, 0xfe067)
        }
        0xfe067 => {
            c.esi = NONE;
            if !cmp32(c, c.eax, c.esi) {
                c.ecx = g.u32(at(c.edi, 0x44))?;
                c.eax &= 0xffff;
                logic(c, c.eax, 1 << 31);
                c.eax = c.eax.wrapping_mul(3);
                c.eax = g.u32(at(c.ecx, c.eax.wrapping_mul(4).wrapping_add(8)))?;
                c.eax = g.u32(at(c.eax, 0x13c))?;
                if !cmp32(c, c.eax, c.esi) {
                    set_slot(c, g, 0x30, c.eax)?;
                }
            }
            c.eax = g.u32(at(c.ebp, 0x194))?;
            if cmp32(c, c.eax, c.esi) {
                return go(0xfe0a5);
            }
            push(c, g, c.esi)?;
            call(0xbadc0, 0xfe09b)
        }
        0xfe09b => {
            if !test32(c, c.eax) {
                g.set_u32(at(c.ebp, 0x194), c.esi)?;
            }
            go(0xfe0a5)
        }
        0xfe0a5 => {
            push(c, g, c.ebx)?;
            call(0x105e80, 0xfe0ab)
        }
        0xfe0ab => {
            c.edx = g.u32(points::OBJECTS.base)?;
            c.ecx = slot(c, g, 0x24)?;
            c.ebx = byte(c.ebx, c.eax as u8);
            c.eax = g.u32(at(c.edx, 0x44))?;
            c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
            let block = u32::from(g.u16(at(c.edx, 0x12a))?);
            super::super::alu::cmp16(c, block, c.esi);
            set_slot8(c, g, 0x13, c.ebx as u8)?;
            if block == c.esi & 0xffff {
                return go(0xfe167);
            }
            c.esi = g.u16(at(c.ebp, 0x12a))? as i16 as i32 as u32;
            c.esi = arithmetic(c, c.esi, c.ebp, false);
            c.edi = 0;
            logic(c, 0, 1 << 31);
            if cmp_m16(c, g, at(c.esi, 6), 0xffff)? {
                return go(0xfe100);
            }
            c.eax = g.u32(c.esi)?;
            c.ecx = g.u32(points::TAG_ENTRIES.base)?;
            c.eax &= 0xffff;
            c.eax = super::super::alu::shl(c, c.eax, 4, 32);
            c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
            c.eax = g.u32(at(c.esi, 4))?;
            push(c, g, c.eax)?;
            call(0x1daea0, 0xfe0fe)
        }
        0xfe0fe => {
            c.edi = c.eax;
            go(0xfe100)
        }
        0xfe100 => {
            c.ecx = slot(c, g, 0x4c)?;
            push(c, g, 0)?;
            push(c, g, 0)?;
            push(c, g, 0)?;
            push(c, g, c.ecx)?;
            push(c, g, 0x000b_f600)?;
            call(0x1cb710, 0xfe115)
        }
        0xfe115 => {
            let playing = test_m8(c, g, at(c.esi, 0x14), 1)?;
            c.eax = u32::from(g.u8(at(c.esi, 0x11))?);
            if playing {
                c.eax = slot(c, g, 0x4c)?;
                return call(0x105740, 0xfe128);
            }
            if !test8(c, c.eax & 2) {
                if !test32(c, c.edi) {
                    return go(0xfe142);
                }
                if !cmp_m16(c, g, at(c.edi, 0x14), 1)? {
                    return go(0xfe142);
                }
            }
            c.ecx = slot(c, g, 0x4c)?;
            call(0x105800, 0xfe142)
        }
        0xfe128 => go(0xfe142),
        0xfe142 => {
            if cmp32(c, g.u32(c.esi)?, NONE) {
                return go(0xfe167);
            }
            if cmp_m16(c, g, at(c.esi, 6), 0xffff)? {
                return go(0xfe167);
            }
            if !test_m8(c, g, at(c.esi, 0x12), 1)? {
                return go(0xfe167);
            }
            if test_m8(c, g, at(c.esi, 0x11), 9)? {
                return go(0xfe167);
            }
            cmp_m16(c, g, at(c.edi, 0x14), 1)?;
            if less_or_equal(c) {
                return go(0xfe167);
            }
            c.ebx = byte(c.ebx, 1);
            set_slot8(c, g, 0x13, 1)?;
            go(0xfe167)
        }
        0xfe167 => {
            c.edx = byte(c.edx, g.u8(at(c.ebp, 0x16c))? >> 5);
            if !test8(c, c.edx & 1) {
                let timer = at(c.ebp, 0x19e);
                g.set_u16(timer, g.u16(timer)?.wrapping_sub(1))?;
                cmp_m16(c, g, timer, 0)?;
                if less_or_equal(c) {
                    rmw8(c, g, at(c.ebp, 0x16c), |f| f | 0x20)?;
                }
                c.ebx = byte(c.ebx, 1);
                set_slot8(c, g, 0x13, 1)?;
            }
            c.ecx = slot(c, g, 0x2c)?;
            c.eax = g.u32(at(c.ecx, 0x12c))? >> 0x0a;
            if !test8(c, c.eax & 1) {
                return go(0xfe1c2);
            }
            if !cmp32(c, g.u32(at(c.ebp, 0x14))?, NONE) {
                return go(0xfe1c2);
            }
            c.edx = g.u32(at(c.ebp, 4))? >> 8;
            if !test8(c, c.edx & 1) {
                return go(0xfe1c2);
            }
            c.edx = slot(c, g, 0x4c)?;
            call(0x10d4e0, 0xfe1be)
        }
        0xfe1be => {
            c.ecx = slot(c, g, 0x2c)?;
            go(0xfe1c2)
        }
        0xfe1c2 => {
            set_al(c, g.u8(at(c.ebp, 0x16e))? >> 7);
            if !test8(c, c.eax & 1) {
                return go(0xfe1dc);
            }
            c.ecx = slot(c, g, 0x4c)?;
            call(0x105840, 0xfe1d8)
        }
        0xfe1d8 => {
            c.ecx = slot(c, g, 0x2c)?;
            go(0xfe1dc)
        }
        0xfe1dc => {
            if test_m8(c, g, at(c.ebp, 0x16e), 1)? {
                // The target toggles between 0 and 1.
                sse::load(c, 1, g.u32(at(c.ebp, 0x190))?);
                sse::zero(c, 0);
                sse::ucomiss(c, sse::lane0(c, 1), 0);
                sse::lahf(c);
                rmw8(c, g, at(c.ebp, 0x16c), |f| f & 0xef)?;
                logic(c, (c.eax >> 8) & 0x44, 0x80);
                if !sse::parity(c) {
                    sse::load(c, 0, g.u32(ONE)?);
                }
                store_ss(c, g, at(c.ebp, 0x190), 0)?;
            }
            // `fe210`.
            sse::load(c, 0, g.u32(at(c.ebp, 0x18c))?);
            sse::ucomiss(c, sse::lane0(c, 0), g.u32(at(c.ebp, 0x190))?);
            if !lahf_jp(c) {
                return go(0xfe2f6);
            }
            sse::load(c, 0, g.u32(at(c.ebp, 0x190))?);
            sse::comiss(c, sse::lane0(c, 0), g.u32(at(c.ebp, 0x18c))?);
            let rising = !sse::below_or_equal(c);
            if rising {
                c.ebx = byte(c.ebx, 1);
            } else {
                c.ebx = byte(c.ebx, 0);
                logic(c, 0, 0x80);
            }
            // `fe240`.
            c.edx = byte(c.edx, g.u8(at(c.ebp, 0x16c))? >> 4);
            if test8(c, c.edx & 1) {
                return go(0xfe27f);
            }
            let started = if test8(c, c.ebx) { 0x248 } else { 0x250 };
            c.eax = g.u32(at(c.ecx, started))?;
            if cmp32(c, c.eax, NONE) {
                return go(0xfe278);
            }
            c.ecx = slot(c, g, 0x4c)?;
            push(c, g, 0)?;
            push(c, g, 0)?;
            push(c, g, NONE)?;
            call(0x1039a0, 0xfe274)
        }
        0xfe274 => {
            c.ecx = slot(c, g, 0x2c)?;
            go(0xfe278)
        }
        0xfe278 => {
            rmw8(c, g, at(c.ebp, 0x16c), |f| f | 0x10)?;
            go(0xfe27f)
        }
        0xfe27f => {
            let target = g.u32(at(c.ebp, 0x190))?;
            let clamp = if test8(c, c.ebx) {
                sse::load(c, 0, g.u32(at(c.ecx, 0x2d8))?);
                c.eax = g.u32(points::GAME_TIME.base)?;
                sse::ss(c, sse::Ss::Mul, 0, g.u32(at(c.eax, 4))?);
                sse::ss(c, sse::Ss::Add, 0, g.u32(at(c.ebp, 0x18c))?);
                sse::comiss(c, sse::lane0(c, 0), target);
                store_ss(c, g, at(c.ebp, 0x18c), 0)?;
                !sse::below_or_equal(c)
            } else {
                // `fe2b0`.
                sse::load(c, 1, g.u32(at(c.ecx, 0x2dc))?);
                c.edx = g.u32(points::GAME_TIME.base)?;
                sse::ss(c, sse::Ss::Mul, 1, g.u32(at(c.edx, 4))?);
                sse::load(c, 0, g.u32(at(c.ebp, 0x18c))?);
                sse::ss_reg(c, sse::Ss::Sub, 0, 1);
                sse::comiss(c, sse::lane0(c, 0), target);
                store_ss(c, g, at(c.ebp, 0x18c), 0)?;
                // `ja fe2e8`: past the target stays, else clamp.
                sse::below_or_equal(c)
            };
            if clamp {
                sse::load(c, 0, g.u32(at(c.ebp, 0x190))?);
            }
            // `fe2e8`.
            c.ebx = byte(c.ebx, 1);
            store_ss(c, g, at(c.ebp, 0x18c), 0)?;
            set_slot8(c, g, 0x13, 1)?;
            go(0xfe2f6)
        }
        0xfe2f6 => {
            sse::load(c, 0, g.u32(at(c.ebp, 0x180))?);
            sse::comiss(c, sse::lane0(c, 0), g.u32(ZERO)?);
            if sse::below_or_equal(c) {
                return go(0xfe4a7);
            }
            sse::comiss(c, sse::lane0(c, 0), g.u32(at(c.ecx, 0x154))?);
            if sse::below(c) {
                return go(0xfe36a);
            }
            set_al(c, g.u8(at(c.ebp, 0x16c))? >> 1);
            if test8(c, c.eax & 1) {
                return go(0xfe36a);
            }
            c.ebx = slot(c, g, 0x4c)?;
            rmw8(c, g, at(c.ebp, 0x16c), |f| f | 2)?;
            c.eax = c.ebx;
            call(0x101f20, 0xfe332)
        }
        0xfe332 => {
            if cmp32(c, c.eax, NONE) {
                return go(0xfe33e);
            }
            push(c, g, 0)?;
            call(0xc86e0, 0xfe33e)
        }
        0xfe33e => {
            c.ecx = slot(c, g, 0x2c)?;
            if cmp_m16(c, g, at(c.ecx, 0x292), 3)? {
                c.edx = byte(c.edx, g.u8(at(c.ebp, 0x16c))? >> 3);
                if test8(c, c.edx & 1) {
                    rmw8(c, g, at(c.ebp, 0x16c), |f| f & 0xf7)?;
                    return go(0xfe36a);
                }
            }
            push(c, g, 0x0e)?;
            call(0x105fa0, 0xfe36a)
        }
        0xfe36a => {
            sse::load(c, 0, g.u32(at(c.ebp, 0x188))?);
            sse::zero(c, 2);
            sse::ucomiss(c, sse::lane0(c, 0), sse::lane0(c, 2));
            if lahf_jp(c) {
                return go(0xfe475);
            }
            c.ecx = word(c.ecx, g.u16(at(c.ebp, 0x16c))?);
            c.edx = slot(c, g, 0x2c)?;
            set_al(c, ((c.ecx as u8) >> 1) & 1);
            let overheated = test8(c, c.eax);
            let mut rate = 0x160;
            if overheated {
                sse::load(c, 0, g.u32(at(c.edx, 0x168))?);
                sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 2));
                if !sse::below_or_equal(c) {
                    rate = 0;
                }
            }
            if rate != 0 {
                sse::load(c, 0, g.u32(at(c.edx, rate))?);
            }
            // `fe3ac`.
            c.esi = g.u32(points::GAME_TIME.base)?;
            sse::load(c, 1, g.u32(at(c.esi, 4))?);
            sse::ss_reg(c, sse::Ss::Mul, 1, 0);
            sse::load(c, 0, g.u32(at(c.edx, 0x254))?);
            sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 2));
            store_ss(c, g, at(c.esp, 0x28), 1)?;
            if !sse::below_or_equal(c) {
                sse::load(c, 0, g.u32(at(c.ebp, 0x184))?);
                sse::ss(c, sse::Ss::Mul, 0, g.u32(at(c.edx, 0x254))?);
                sse::load(c, 3, g.u32(ONE)?);
                sse::ss_reg(c, sse::Ss::Sub, 3, 0);
                sse::ss_reg(c, sse::Ss::Mul, 3, 1);
                store_ss(c, g, at(c.esp, 0x28), 3)?;
                sse::movaps(c, 1, 3);
            }
            // `fe3f7`.
            sse::load(c, 0, g.u32(at(c.ebp, 0x180))?);
            sse::ss_reg(c, sse::Ss::Sub, 0, 1);
            sse::comiss(c, sse::lane0(c, 2), sse::lane0(c, 0));
            store_ss(c, g, at(c.ebp, 0x180), 0)?;
            if !sse::below_or_equal(c) {
                store_ss(c, g, at(c.ebp, 0x180), 2)?;
            }
            if !test8(c, c.eax) {
                return go(0xfe475);
            }
            c.ecx = byte(c.ecx, (c.ecx as u8) >> 2);
            if test8(c, c.ecx & 1) {
                return go(0xfe475);
            }
            c.ebx = slot(c, g, 0x4c)?;
            push(c, g, 1)?;
            push(c, g, 0x0800_0071)?;
            c.ecx = c.ebx;
            call(0x1685a6, 0xfe436)
        }
        0xfe436 => {
            super::super::alu::cmp16(c, c.eax, 1);
            if less_or_equal(c) {
                c.eax = 1;
            }
            c.ecx = slot(c, g, 0x2c)?;
            sse::load(c, 0, g.u32(at(c.ebp, 0x180))?);
            sse::ss(c, sse::Ss::Sub, 0, g.u32(at(c.ecx, 0x150))?);
            sse::ss(c, sse::Ss::Div, 0, slot(c, g, 0x28)?);
            c.edx = c.eax as u16 as i16 as i32 as u32;
            sse::cvtsi2ss(c, 1, c.edx);
            sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0));
            if sse::below(c) {
                return go(0xfe475);
            }
            rmw8(c, g, at(c.ebp, 0x16c), |f| f | 4)?;
            push(c, g, 0x0f)?;
            call(0x105fa0, 0xfe475)
        }
        0xfe475 => {
            set_al(c, g.u8(at(c.ebp, 0x16c))? >> 1);
            if test8(c, c.eax & 1) {
                c.ecx = slot(c, g, 0x2c)?;
                sse::load(c, 0, g.u32(at(c.ecx, 0x150))?);
                sse::comiss(c, sse::lane0(c, 0), g.u32(at(c.ebp, 0x180))?);
                if !sse::below_or_equal(c) {
                    rmw8(c, g, at(c.ebp, 0x16c), |f| f & 0xf9)?;
                }
            }
            // `fe49d`.
            c.ecx = slot(c, g, 0x2c)?;
            c.ebx = byte(c.ebx, 1);
            set_slot8(c, g, 0x13, 1)?;
            go(0xfe4a7)
        }
        0xfe4a7 => {
            c.esi = g.u32(at(c.ecx, 0x170))?;
            if cmp32(c, c.esi, NONE) {
                return go(0xfe517);
            }
            let owner_bit = (g.u8(at(c.ebp, 0x12c))? >> 1) & 1;
            let on = (g.u8(at(c.ebp, 0x16c))? >> 1) & 1;
            c.ecx = byte(c.ecx, owner_bit);
            c.edx = byte(c.edx, on);
            let want = if test8(c, c.edx) && !test8(c, c.ecx) {
                1
            } else {
                logic(c, 0, 0x80);
                0
            };
            c.edx = byte(c.edx, want);
            // `fe4d4`.
            c.edi = g.u32(at(c.ebp, 0x244))?;
            let have = u8::from(!cmp32(c, c.edi, NONE));
            set_al(c, have);
            if cmp8(c, c.edx, c.eax) {
                return go(0xfe517);
            }
            if test8(c, c.edx) {
                c.eax = slot(c, g, 0x4c)?;
                return call(0x103a60, 0xfe4f1);
            }
            if test8(c, c.ecx) {
                c.eax = c.edi;
                return call(0x177610, 0xfe504);
            }
            push(c, g, 1)?;
            call(0x177260, 0xfe50d)
        }
        0xfe4f1 => {
            g.set_u32(at(c.ebp, 0x244), c.eax)?;
            go(0xfe517)
        }
        0xfe504 | 0xfe50d => {
            g.set_u32(at(c.ebp, 0x244), NONE)?;
            go(0xfe517)
        }
        0xfe517 => {
            sse::zero(c, 0);
            c.eax = u32::from(g.u16(at(c.ebp, 0x17c))?);
            super::super::alu::test16(c, c.eax);
            store_ss(c, g, at(c.ebp, 0x188), 0)?;
            if !less_or_equal(c) {
                c.eax = super::super::alu::dec32(c, c.eax);
                c.ebx = byte(c.ebx, 1);
                g.set_u16(at(c.ebp, 0x17c), c.eax as u16)?;
                set_slot8(c, g, 0x13, 1)?;
            }
            // `fe53e`: the body `+16e` bits into the frame bytes.
            c.eax = word(c.eax, g.u16(at(c.ebp, 0x16e))?);
            let flags = c.eax as u8;
            set_slot8(c, g, 0x20, (flags >> 3) & 1)?;
            set_slot8(c, g, 0x21, (flags >> 4) & 1)?;
            let high = ((c.eax >> 8) as u8) & 1;
            c.eax = byte(c.eax, high);
            set_slot8(c, g, 0x1e, high)?;
            c.ecx = byte(c.ecx, (flags >> 1) & 1);
            c.edx = byte(c.edx, (flags >> 2) & 1);
            c.eax = g.u32(points::OBJECTS.base)?;
            set_slot8(c, g, 0x1c, c.ecx as u8)?;
            c.ecx = g.u32(at(c.eax, 0x44))?;
            set_slot8(c, g, 0x1d, c.edx as u8)?;
            c.edx = slot(c, g, 0x24)?;
            c.eax = g.u32(at(at(c.ecx, c.edx), 8))?;
            c.ecx = byte(c.ecx, g.u8(at(c.eax, 0x16e))? >> 5);
            let busy = test8(c, c.ecx & 1);
            set_slot8(c, g, 0x22, 0)?;
            let not_ready = busy || {
                cmp_m16(c, g, at(c.eax, 0x17c), 0)?;
                !less_or_equal(c) || {
                    c.edx = byte(c.edx, g.u8(at(c.eax, 0x16c))? >> 1);
                    test8(c, c.edx & 1)
                }
            };
            if not_ready {
                c.ebx = byte(c.ebx, 1);
                g.set_u8(at(c.ebp, 0x174), 0)?;
                set_slot8(c, g, 0x16, 1)?;
                set_slot8(c, g, 0x13, 1)?;
            }
            // `fe5c8`.
            c.edi = slot(c, g, 0x2c)?;
            c.eax = g.u32(at(c.edi, 0x2c0))?;
            c.esi = 0;
            if !test32(c, c.eax) || less_or_equal(c) {
                return go(TRIGGERS);
            }
            c.eax = 0;
            logic(c, 0, 1 << 31);
            go(0xfe5e0)
        }
        0xfe5e0 => {
            push(c, g, c.eax)?;
            c.eax = slot(c, g, 0x50)?;
            push(c, g, c.eax)?;
            call(0xfdd50, 0xfe5eb)
        }
        0xfe5eb => {
            c.ecx = g.u32(at(c.edi, 0x2c0))?;
            c.ebx = byte(c.ebx, (c.ebx | c.eax) as u8);
            c.esi = c.esi.wrapping_add(1);
            c.eax = c.esi as u16 as i16 as i32 as u32;
            arithmetic(c, c.eax, c.ecx, true);
            if less(c) {
                return go(0xfe5e0);
            }
            set_slot8(c, g, 0x13, c.ebx as u8)?;
            go(TRIGGERS)
        }
        _ => Err(unowned(pc)),
    }
}
