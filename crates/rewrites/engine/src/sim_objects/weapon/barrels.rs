//! `fefee..ff576`: the barrel loop and the return. For barrel `i` (`edi`,
//! also at `[S+28]`): `esi` the barrel state at body `+1a4 + i × 34`
//! through the body pointer read at entry (`[S+34]`), `ebp` the barrel
//! definition (definition `+2d4` + `i` × `ec`), `ebx` the object. Field
//! names L.
//!
//! 1. Two timers at state `+14` and `+18` fall by definition `+d0` and
//!    `+cc` × the tick, floored at 0; the countdown word `+2` falls by
//!    one.
//! 2. The state byte `+1` (`ff5d4`): 0 idle (`1023d0(object)`, or the
//!    paired barrel's state when definition `+0` bits 9 and 10 ask for
//!    it, moves it to 1), 1 firing (`102100(object, &[S+14], &[S+17];
//!    eax: i)`, then `103e60(i; eax: object)` to stop, `102fb0(object,
//!    i)` to fire, or the shot counter `+0` up to `7f`), 2
//!    `103f60(object, i)` and 3 `103dd0(i; eax: object)` when the
//!    countdown is zero.
//! 3. The heat at state `+10`: up by definition `+d4` × the tick when the
//!    barrel fired (`[S+15]`), capped at 1, down by `+d8` otherwise; past
//!    definition `+18` it starts or stops the overheat effect
//!    `ba7f0(object or its parent, -1, 0, -1 / 0)` (state `+4` bit 3).
//! 4. While hot, `[body +175]` becomes the x87 `(heat × [43fde4] (× def
//!    `+14` when positive) + [body +175] × [45dbb4]) × [45dc38]` through
//!    `_ftol2` `322b0c`.
//! 5. The spin at state `+20` (x87 clamps to `[45dcc0]` above and
//!    `[45dbe0]` below) and the value at `+1c` (towards 1 while firing,
//!    towards 0 otherwise, blended by `[S+38]` between definition
//!    `+dc`/`+50` and `+e0`/`+54`).

use super::*;

/// The barrel states (`jmp [ff5d4 + state × 4]`).
const STATES: [u32; 4] = [0xff0c3, 0xff165, 0xff205, 0xff215];

const STOP: u32 = 0x103e60;
/// `_ftol2`: `st(0)` popped, truncated, into `edx:eax`.
const FTOL: u32 = 0x322b0c;
const OVERHEAT_EFFECT: u32 = 0xba7f0;

/// binary32 constants the heat and spin read (O: meaning).
const HEAT_SCALE: u32 = 0x0045_dbb4;
const HEAT_WEIGHT: u32 = 0x0043_fde4;
const HEAT_RANGE: u32 = 0x0045_dc38;
/// binary64 bounds of the spin.
const SPIN_MAX: u32 = 0x0045_dcc0;
const SPIN_MIN: u32 = 0x0045_dbe0;

/// `ff2a1..ff2e9` and `ff348..ff391`: who the overheat effect plays on.
/// `eax` the header, `bl` its flags byte, `ecx` the re-read body, `edi`
/// its definition, `edx` the object, or its parent (body `+14`) when the
/// header is a child (`10`), body `+04` bit 0 is set, or the
/// definition has no `+38`.
fn effect_owner(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    let data = g.u32(at(g.u32(points::OBJECTS.base)?, 0x44))?;
    let header = slot(c, g, 0x24)?;
    c.edi = g.u32(points::TAG_ENTRIES.base)?;
    c.ebx = byte(c.ebx, g.u8(at(at(data, header), 2))?);
    c.eax = at(data, header);
    c.ecx = g.u32(at(c.eax, 8))?;
    c.edx = (g.u32(c.ecx)? & 0xffff) << 4;
    let child = test8(c, c.ebx & 0x10);
    c.edi = g.u32(at(at(c.edx, c.edi), 8))?;
    c.edx = slot(c, g, 0x4c)?;
    let parent = child || test_m8(c, g, at(c.ecx, 4), 1)? || cmp32(c, g.u32(at(c.edi, 0x38))?, NONE);
    if parent {
        c.ecx = g.u32(at(c.ecx, 0x14))?;
        if !cmp32(c, c.ecx, NONE) {
            c.edx = c.ecx;
        }
    }
    Ok(())
}

/// `push edi; mov eax, ebx; call 103e60`.
fn stop(c: &mut CpuAbi, g: &mut Guest, site: u32) -> R {
    push(c, g, c.edi)?;
    c.eax = c.ebx;
    call(STOP, site)
}

/// `movss xmm0, [addr]; comiss xmm0, xmm1; jbe skip; movss xmm2, [rate];
/// mulss xmm2, [ecx+4]; subss xmm0, xmm2; comiss xmm1, xmm0; movss
/// [addr], xmm0; jb keep; movss [addr], xmm1; keep: mov byte [esp+13h],
/// 1`: a timer falls by `rate` × the tick, floored at 0 (`xmm1`).
fn fall(c: &mut CpuAbi, g: &mut Guest, addr: u32, rate: u32) -> Result<()> {
    sse::load(c, 0, g.u32(addr)?);
    sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
    if sse::below_or_equal(c) {
        return Ok(());
    }
    sse::load(c, 2, g.u32(rate)?);
    sse::ss(c, sse::Ss::Mul, 2, g.u32(at(c.ecx, 4))?);
    sse::ss_reg(c, sse::Ss::Sub, 0, 2);
    sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0));
    store_ss(c, g, addr, 0)?;
    if !sse::below(c) {
        store_ss(c, g, addr, 1)?;
    }
    set_slot8(c, g, 0x13, 1)
}

/// `fld; fld dword [esp+off]; fcomip st, st(1); fstp st(0); ...; fld
/// either; fstp dword [esi+20h]`: the spin clamped to the binary64 bound
/// at `bound`, above (`max`) or below. The guest's x87 state, with the
/// last-instruction registers of the final `fstp`.
fn clamp_spin(c: &mut CpuAbi, g: &mut Guest, off: u32, bound: u32, max: bool, pcs: [u32; 6]) -> Result<()> {
    let [ld_bound, ld_value, compare, ld_bound2, ld_value2, store] = pcs;
    let value = at(c.esp, off);
    let mut x = x87_in(c);
    fld_f64(&mut x, g, bound, ld_bound)?;
    fld_f32(&mut x, g, value, ld_value)?;
    fcomip_fstp(c, &mut x, compare)?;
    // `jbe`: the value is at or below the bound.
    let below = sse::below_or_equal(c);
    if below == max {
        fld_f32(&mut x, g, value, ld_value2)?;
    } else {
        fld_f64(&mut x, g, bound, ld_bound2)?;
    }
    fstp_f32(&mut x, g, at(c.esi, 0x20), store)?;
    x87_out(c, g, &x, store, at(c.esi, 0x20))
}

pub(super) fn step(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> R {
    match pc {
        0xfefee => {
            c.ecx = g.u32(at(c.edi, 0x2d0))?;
            c.eax = 0;
            arithmetic(c, c.ecx, c.eax, true);
            set_slot(c, g, 0x28, c.eax)?;
            if less_or_equal(c) {
                return go(0xff56b);
            }
            sse::zero(c, 1);
            c.ebx = slot(c, g, 0x4c)?;
            c.edi = c.eax;
            go(0xff00b)
        }
        0xff00b => {
            c.esi = slot(c, g, 0x34)?;
            c.ecx = g.u32(points::GAME_TIME.base)?;
            c.edx = c.eax.wrapping_mul(0x34);
            sse::load(c, 0, g.u32(at(at(c.edx, c.esi), 0x1b8))?);
            c.ebp = c.eax.wrapping_mul(0xec);
            c.esi = at(at(c.edx, c.esi), 0x1a4);
            c.edx = slot(c, g, 0x2c)?;
            c.ebp = arithmetic(c, c.ebp, g.u32(at(c.edx, 0x2d4))?, false);
            sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
            set_slot8(c, g, 0x15, 0)?;
            if !sse::below_or_equal(c) {
                sse::load(c, 2, g.u32(at(c.ebp, 0xd0))?);
                sse::ss(c, sse::Ss::Mul, 2, g.u32(at(c.ecx, 4))?);
                sse::ss_reg(c, sse::Ss::Sub, 0, 2);
                sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0));
                store_ss(c, g, at(c.esi, 0x14), 0)?;
                if !sse::below(c) {
                    store_ss(c, g, at(c.esi, 0x14), 1)?;
                }
                set_slot8(c, g, 0x13, 1)?;
            }
            // `ff06b`.
            fall(c, g, at(c.esi, 0x18), at(c.ebp, 0xcc))?;
            // `ff09a`.
            c.ecx = u32::from(g.u16(at(c.esi, 2))?);
            super::super::alu::test16(c, c.ecx);
            if !less_or_equal(c) {
                c.ecx = super::super::alu::dec32(c, c.ecx);
                g.set_u16(at(c.esi, 2), c.ecx as u16)?;
                set_slot8(c, g, 0x13, 1)?;
            }
            // `ff0af`.
            c.ecx = g.u8(at(c.esi, 1))? as i8 as i32 as u32;
            arithmetic(c, c.ecx, 3, true);
            if above(c) {
                return go(0xff224);
            }
            go(STATES[c.ecx as usize])
        }
        // State 0.
        0xff0c3 => {
            c.ecx = byte(c.ecx, slot8(c, g, 0x16)?);
            if test8(c, c.ecx) {
                rmw8(c, g, at(c.esi, 4), |f| f & 0xfe)?;
                return go(0xff15b);
            }
            if !test_m8(c, g, at(c.esi, 4), 1)? {
                push(c, g, c.ebx)?;
                return call(0x1023d0, 0xff152);
            }
            c.edx = g.u32(c.ebp)?;
            c.eax = c.edx >> 9;
            c.ebx = byte(c.ebx, 0);
            set_al(c, (c.eax as u8) & 1);
            let paired = test8(c, c.eax) || {
                c.ecx = c.edx >> 0x0a;
                test8(c, c.ecx & 1)
            };
            // `ff138`: the barrel moves to state 1; `ff142`: it waits.
            let mut wait = false;
            if paired {
                c.ecx = slot(c, g, 0x2c)?;
                arithmetic(c, g.u32(at(c.ecx, 0x2d0))?, 1, true);
                if !less_or_equal(c) {
                    super::super::alu::test16(c, c.edi);
                    let first = c.edi & 0xffff == 0;
                    c.edi = slot(c, g, 0x34)?;
                    c.ecx = u32::from(first).wrapping_mul(0x34);
                    c.ecx = at(at(c.ecx, c.edi), 0x1a4);
                    c.ecx = byte(c.ecx, g.u8(at(c.ecx, 1))?);
                    if cmp8(c, c.ecx, 1) && test8(c, c.eax) {
                        c.ebx = byte(c.ebx, c.ecx as u8);
                    }
                    // `ff123`.
                    if cmp8(c, c.ecx, 3) {
                        c.edx >>= 0x0a;
                        wait = test8(c, c.edx & 1);
                    }
                    if !wait {
                        // `ff130`.
                        wait = test8(c, c.ebx);
                    }
                    c.edi = slot(c, g, 0x28)?;
                }
            }
            c.ebx = slot(c, g, 0x4c)?;
            if !wait {
                g.set_u8(at(c.esi, 1), 1)?;
            }
            go(0xff156)
        }
        0xff152 => {
            if !test8(c, c.eax) {
                return go(0xff15b);
            }
            go(0xff156)
        }
        0xff156 => {
            set_slot8(c, g, 0x13, 1)?;
            go(0xff15b)
        }
        0xff15b => {
            if !cmp8(c, u32::from(g.u8(at(c.esi, 1))?), 1) {
                return go(0xff224);
            }
            go(0xff165)
        }
        // State 1.
        0xff165 => {
            set_al(c, slot8(c, g, 0x16)?);
            let not_ready = test8(c, c.eax);
            set_slot8(c, g, 0x14, 0)?;
            set_slot8(c, g, 0x17, 0)?;
            if not_ready {
                return stop(c, g, 0xff17f);
            }
            c.edx = at(c.esp, 0x17);
            push(c, g, c.edx)?;
            c.eax = at(c.esp, 0x18);
            push(c, g, c.eax)?;
            push(c, g, c.ebx)?;
            c.eax = c.edi;
            call(0x102100, 0xff196)
        }
        0xff17f | 0xff1b0 | 0xff1c2 | 0xff1d4 | 0xff1ec | 0xff213 => go(0xff224),
        0xff196 => {
            if test8(c, c.eax) {
                if flag(c, g, 0x14)? && !test_m8(c, g, at(c.esi, 4), 1)? {
                    return stop(c, g, 0xff1b0);
                }
                // `ff1b2`: fire.
                rmw8(c, g, at(c.esi, 4), |f| f & 0xfe)?;
                push(c, g, c.edi)?;
                push(c, g, c.ebx)?;
                set_slot8(c, g, 0x1d, 1)?;
                return call(0x102fb0, 0xff1c2);
            }
            if flag(c, g, 0x17)? {
                return stop(c, g, 0xff1d4);
            }
            if flag(c, g, 0x14)? && !test_m8(c, g, at(c.esi, 4), 1)? {
                return stop(c, g, 0xff1ec);
            }
            // `ff1ee`: one more shot, up to `7f`.
            c.eax = g.u8(c.esi)? as i8 as i32 as u32;
            c.eax = c.eax.wrapping_add(1);
            arithmetic(c, c.eax, 0x7f, true);
            set_slot8(c, g, 0x15, 1)?;
            if !less_or_equal(c) {
                c.eax = 0x7f;
            }
            g.set_u8(c.esi, c.eax as u8)?;
            go(0xff224)
        }
        // States 2 and 3.
        0xff205 => {
            if !cmp_m16(c, g, at(c.esi, 2), 0)? {
                return go(0xff224);
            }
            push(c, g, c.edi)?;
            push(c, g, c.ebx)?;
            call(0x103f60, 0xff213)
        }
        0xff215 => {
            if !cmp_m16(c, g, at(c.esi, 2), 0)? {
                return go(0xff224);
            }
            push(c, g, c.edi)?;
            c.eax = c.ebx;
            call(0x103dd0, 0xff224)
        }
        0xff224 => {
            set_al(c, g.u8(at(c.esi, 1))?);
            let busy = test8(c, c.eax) || {
                cmp_m16(c, g, at(c.esi, 2), 0)?;
                !less_or_equal(c)
            };
            if busy {
                set_slot8(c, g, 0x13, 1)?;
            }
            // `ff237`: the heat.
            if !flag(c, g, 0x15)? {
                return go(0xff300);
            }
            sse::load(c, 0, g.u32(at(c.ebp, 0xd4))?);
            c.ecx = g.u32(points::GAME_TIME.base)?;
            sse::ss(c, sse::Ss::Mul, 0, g.u32(at(c.ecx, 4))?);
            sse::ss(c, sse::Ss::Add, 0, g.u32(at(c.esi, 0x10))?);
            sse::load(c, 1, g.u32(ONE)?);
            sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
            store_ss(c, g, at(c.esi, 0x10), 0)?;
            if !sse::below_or_equal(c) {
                store_ss(c, g, at(c.esi, 0x10), 1)?;
            }
            // `ff272`.
            sse::load(c, 0, g.u32(at(c.ebp, 0x18))?);
            sse::ucomiss(c, sse::lane0(c, 0), g.u32(ZERO)?);
            if !lahf_jp(c) {
                return go(0xff3ab);
            }
            if test_m8(c, g, at(c.esi, 4), 8)? {
                return go(0xff3ab);
            }
            sse::load(c, 0, g.u32(at(c.esi, 0x10))?);
            sse::comiss(c, sse::lane0(c, 0), g.u32(at(c.ebp, 0x18))?);
            if sse::below_or_equal(c) {
                return go(0xff3ab);
            }
            effect_owner(c, g)?;
            push(c, g, NONE)?;
            push(c, g, 0)?;
            push(c, g, NONE)?;
            push(c, g, c.edx)?;
            call(OVERHEAT_EFFECT, 0xff2f7)
        }
        0xff2f7 => {
            rmw8(c, g, at(c.esi, 4), |f| f | 8)?;
            go(0xff3a3)
        }
        0xff300 => {
            sse::load(c, 0, g.u32(at(c.esi, 0x10))?);
            sse::zero(c, 1);
            sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
            if !sse::below_or_equal(c) {
                sse::load(c, 2, g.u32(at(c.ebp, 0xd8))?);
                c.eax = g.u32(points::GAME_TIME.base)?;
                sse::ss(c, sse::Ss::Mul, 2, g.u32(at(c.eax, 4))?);
                sse::ss_reg(c, sse::Ss::Sub, 0, 2);
                sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0));
                store_ss(c, g, at(c.esi, 0x10), 0)?;
                if !sse::below_or_equal(c) {
                    store_ss(c, g, at(c.esi, 0x10), 1)?;
                }
                set_slot8(c, g, 0x13, 1)?;
            }
            // `ff337`.
            if !test_m8(c, g, at(c.esi, 4), 8)? {
                return go(0xff3ab);
            }
            sse::load(c, 0, g.u32(at(c.ebp, 0x18))?);
            sse::comiss(c, sse::lane0(c, 0), g.u32(at(c.esi, 0x10))?);
            if sse::below_or_equal(c) {
                return go(0xff3ab);
            }
            effect_owner(c, g)?;
            push(c, g, 0)?;
            push(c, g, 0)?;
            push(c, g, NONE)?;
            push(c, g, c.edx)?;
            call(OVERHEAT_EFFECT, 0xff39f)
        }
        0xff39f => {
            rmw8(c, g, at(c.esi, 4), |f| f & 0xf7)?;
            go(0xff3a3)
        }
        0xff3a3 => {
            c.edi = slot(c, g, 0x28)?;
            c.ebx = slot(c, g, 0x4c)?;
            go(0xff3ab)
        }
        0xff3ab => {
            sse::load(c, 0, g.u32(at(c.esi, 0x10))?);
            sse::zero(c, 1);
            sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
            if sse::below_or_equal(c) {
                return go(0xff419);
            }
            c.eax = slot(c, g, 0x34)?;
            c.ecx = u32::from(g.u8(at(c.eax, 0x175))?);
            sse::load(c, 2, g.u32(at(c.ebp, 0x14))?);
            sse::comiss(c, sse::lane0(c, 2), sse::lane0(c, 1));
            sse::cvtsi2ss(c, 0, c.ecx);
            sse::ss(c, sse::Ss::Mul, 0, g.u32(HEAT_SCALE)?);
            store_ss(c, g, at(c.esp, 0x40), 0)?;
            sse::load(c, 0, g.u32(at(c.esi, 0x10))?);
            sse::ss(c, sse::Ss::Mul, 0, g.u32(HEAT_WEIGHT)?);
            store_ss(c, g, at(c.esp, 0x30), 0)?;
            if !sse::below_or_equal(c) {
                sse::ss_reg(c, sse::Ss::Mul, 0, 2);
                store_ss(c, g, at(c.esp, 0x30), 0)?;
            }
            // `ff3fc`: `fld; fadd; fmul`, then `_ftol2` takes `st(0)`.
            let mut x = x87_in(c);
            fld_f32(&mut x, g, at(c.esp, 0x30), 0xff3fc)?;
            fp_ok(fp::arith(&mut x, fp::arith_op::ADD, 0, fp::src_kind::F32, u64::from(slot(c, g, 0x40)?)), 0xff400)?;
            fp_ok(fp::arith(&mut x, fp::arith_op::MUL, 0, fp::src_kind::F32, u64::from(g.u32(HEAT_RANGE)?)), 0xff404)?;
            x87_out(c, g, &x, 0xff404, HEAT_RANGE)?;
            call(FTOL, 0xff40f)
        }
        0xff40f => {
            c.edx = slot(c, g, 0x34)?;
            g.set_u8(at(c.edx, 0x175), c.eax as u8)?;
            go(0xff419)
        }
        0xff419 => {
            if flag(c, g, 0x15)? {
                // `ff425`: the spin up, the value toward 1.
                sse::load(c, 0, g.u32(at(c.ebp, 0xc4))?);
                c.eax = g.u32(points::GAME_TIME.base)?;
                sse::ss(c, sse::Ss::Mul, 0, g.u32(at(c.eax, 4))?);
                sse::ss(c, sse::Ss::Add, 0, g.u32(at(c.esi, 0x20))?);
                store_ss(c, g, at(c.esp, 0x20), 0)?;
                // The `movss [esi+20h], xmm0` between the compare and the
                // `fld` is stored first; the clamp's `fstp` replaces it.
                let x0 = sse::lane0(c, 0);
                clamp_spin_with(c, g, 0x20, SPIN_MAX, true, [0xff42d, 0xff448, 0xff44e, 0xff457, 0xff45f, 0xff46b], x0)?;
                sse::load(c, 2, g.u32(ONE)?);
                sse::load(c, 1, slot(c, g, 0x38)?);
                sse::load(c, 3, g.u32(at(c.ebp, 0x50))?);
                sse::movaps(c, 0, 2);
                sse::ss_reg(c, sse::Ss::Sub, 0, 1);
                sse::ss(c, sse::Ss::Mul, 0, g.u32(at(c.ebp, 0xdc))?);
                sse::ss_reg(c, sse::Ss::Mul, 3, 1);
                sse::ss_reg(c, sse::Ss::Add, 0, 3);
                sse::ss(c, sse::Ss::Mul, 0, g.u32(at(c.eax, 4))?);
                sse::ss(c, sse::Ss::Add, 0, g.u32(at(c.esi, 0x1c))?);
                sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 2));
                store_ss(c, g, at(c.esi, 0x1c), 0)?;
                if !sse::below_or_equal(c) {
                    sse::movaps(c, 0, 2);
                }
                sse::zero(c, 1);
                return go(0xff547);
            }
            // `ff4af`: the spin down, the value toward 0.
            sse::load(c, 0, g.u32(at(c.esi, 0x1c))?);
            sse::zero(c, 1);
            sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
            if sse::below_or_equal(c) {
                return go(0xff551);
            }
            sse::load(c, 2, g.u32(at(c.ebp, 0xc8))?);
            c.eax = g.u32(points::GAME_TIME.base)?;
            sse::ss(c, sse::Ss::Mul, 2, g.u32(at(c.eax, 4))?);
            sse::load(c, 0, g.u32(at(c.esi, 0x20))?);
            sse::ss_reg(c, sse::Ss::Sub, 0, 2);
            store_ss(c, g, at(c.esp, 0x1c), 0)?;
            let x0 = sse::lane0(c, 0);
            clamp_spin_with(c, g, 0x1c, SPIN_MIN, false, [0xff4c8, 0xff4e7, 0xff4ed, 0xff4fc, 0xff4f6, 0xff508], x0)?;
            sse::load(c, 0, slot(c, g, 0x38)?);
            sse::load(c, 2, g.u32(ONE)?);
            sse::load(c, 3, g.u32(at(c.ebp, 0x54))?);
            sse::ss_reg(c, sse::Ss::Sub, 2, 0);
            sse::ss(c, sse::Ss::Mul, 2, g.u32(at(c.ebp, 0xe0))?);
            sse::ss_reg(c, sse::Ss::Mul, 3, 0);
            sse::load(c, 0, g.u32(at(c.esi, 0x1c))?);
            sse::ss_reg(c, sse::Ss::Add, 2, 3);
            sse::ss(c, sse::Ss::Mul, 2, g.u32(at(c.eax, 4))?);
            sse::ss_reg(c, sse::Ss::Sub, 0, 2);
            sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
            store_ss(c, g, at(c.esi, 0x1c), 0)?;
            if sse::below_or_equal(c) {
                sse::movaps(c, 0, 1);
            }
            go(0xff547)
        }
        0xff547 => {
            store_ss(c, g, at(c.esi, 0x1c), 0)?;
            set_slot8(c, g, 0x13, 1)?;
            go(0xff551)
        }
        0xff551 => {
            c.ecx = slot(c, g, 0x2c)?;
            c.edx = g.u32(at(c.ecx, 0x2d0))?;
            c.edi = c.edi.wrapping_add(1);
            c.eax = c.edi as u16 as i16 as i32 as u32;
            arithmetic(c, c.eax, c.edx, true);
            set_slot(c, g, 0x28, c.edi)?;
            if less(c) {
                return go(0xff00b);
            }
            go(0xff56b)
        }
        0xff56b => {
            set_al(c, slot8(c, g, 0x13)?);
            c.edi = super::super::frame::pop(c, g)?;
            c.esi = super::super::frame::pop(c, g)?;
            c.ebp = super::super::frame::pop(c, g)?;
            c.ebx = super::super::frame::pop(c, g)?;
            c.esp = arithmetic(c, c.esp, 0x38, false);
            Ok(Step::Out(Next::Ret { pop: 4 }))
        }
        _ => Err(unowned(pc)),
    }
}

/// [`clamp_spin`] after `movss [esi+20h], xmm0` (`x0`, the unclamped
/// value), which retail stores between the compare and the second `fld`.
fn clamp_spin_with(c: &mut CpuAbi, g: &mut Guest, off: u32, bound: u32, max: bool, pcs: [u32; 6], x0: u32) -> Result<()> {
    g.set_u32(at(c.esi, 0x20), x0)?;
    clamp_spin(c, g, off, bound, max, pcs)
}

/// `mov al, [esp+off]; test al, al`.
fn flag(c: &mut CpuAbi, g: &Guest, off: u32) -> Result<bool> {
    set_al(c, slot8(c, g, off)?);
    Ok(test8(c, c.eax))
}
