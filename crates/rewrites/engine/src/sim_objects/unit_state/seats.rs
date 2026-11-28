//! The powered seats `c5eb0` and the control refresh `ce0c0` it calls.
//!
//! **`c5eb0 (unit: eax) -> al`; `ret`.** `ecx` pushed first as a frame
//! slot: its top byte (`[esp+f]` after the next three pushes) is the
//! result, so `ecx` returns with the result in its top byte, as in
//! `c85c0`. Then `ebp`, `esi`, `edi`; `ebx` only past the early out. Across
//! the calls `edi` holds the unit, `esi` its body, `ebp` its definition
//! (the Halo 1 drive model's `before` hook on `ce0c0` reads `esi` and
//! `ebp` there and may set the result byte). Returns 0 at once when
//! definition `+bc` bit 11 is set. Otherwise it calls `d0c10` (`ebx` the
//! unit) and `ce0c0` (`edi` the unit, fused); when body `+134` bit 26 is
//! set, the game is not in mode 4 (`[[4e6948]+c]`), the unit has a parent
//! (`+14`) and a seat (`+1fc` not `ffff`), it clears the bit and calls
//! `e68c0(1e, unit)`. Then for each of the definition's powered seats
//! (`+1b8` entries of 8 bytes at `+1bc`, the first float the time) it
//! moves body `+25c + 4n` toward 1 when the seat is occupied (seat 0: a
//! driver at `+248`, or body `+134` bit 1; seat 1: a gunner at `+24c`
//! that is not the driver) and the unit is not dead (body `+10a` bit 2),
//! else toward 0, by one tick over the time (`[[510c54]+2]` ticks a
//! second), clamped; any seat that moves sets the result.
//!
//! **`ce0c0 (unit: edi)`; `ret`.** Frame `E` = `esp` after `sub esp, 6c`
//! and the pushes of `ebx` and `esi`; `esi` holds the body. It clears body
//! `+134` bits 25 and 26 (two stores), then asks `d1080(unit, E+40,
//! E+2c)` for the unit's control frame: `[E+2c..38]` three scales and a
//! threshold, `[E+3c]` a second threshold, `[E+40..74]` a basis and a
//! position. Unless that fails, or the unit is dead (body `+10a` bit 2)
//! and has a non-zero `+aa` or `e4050` answers 0, the position's change since body
//! `+270` and its change since the previous change (`+27c`), each times
//! the ticks a second, are taken into the basis: body `+294`, `+298`,
//! `+29c`. When a threshold is positive and any of the three absolute
//! values (x87 `fabs`, through `[E+8..10]`) exceeds it, `+134` bit 25 (the
//! first) or bit 26 (the second; only for a seated unit, and in game
//! mode 2 only when `138880` answers 0) is set. The position and the
//! change are stored at `+270` and `+27c`. Otherwise the three are zero.
//! Last, `+288..290` is blended toward `+294..29c` with one of two pairs
//! of weights from the image (the second, once, when `+134` bit 28 is
//! set, which it clears).

use super::{NONE, ONE, call, callee, owned, pop, push, ret, sx16};
use guest_layout::points;
use crate::sim_objects::functions::{fp_error, fpu, fpu_done};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_objects::{Cx, Next, Result, unowned};
use runtime::{CpuAbi, Guest};

/// The blend weights `ce0c0` reads: the usual pair, and the pair used once
/// after a reset (`+134` bit 28).
pub(super) const BLEND: (u32, u32) = (0x0045_dd1c, 0x0044_ae90);
pub(super) const BLEND_RESET: (u32, u32) = (0x0045_ddac, 0x0045_dc1c);

// ---------------------------------------------------------------- c5eb0

/// `c5eb0`'s return sites: `d0c10`, `ce0c0`, `e68c0`.
pub(in crate::sim_objects) const SEATS_SITES: [u32; 3] = [0xc5eff, 0xc5f04, 0xc5f47];

/// `movss [addr], xmm[r]`.
fn st(c: &CpuAbi, g: &mut Guest, addr: u32, r: usize) -> Result<()> {
    g.set_u32(addr, sse::lane0(c, r))
}

/// `ucomiss a, b; lahf; test ah, 44h; jnp`: taken when ordered and equal.
fn equal(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    let f = sse::ucomiss(c, a, b);
    sse::lahf(c);
    f & 0x44 == 0x40
}

/// `comiss a, b; jbe` taken.
fn below_or_equal(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    sse::comiss(c, a, b) & 0x41 != 0
}

/// `c5eb0 (unit: eax) -> al`.
pub(in crate::sim_objects) fn seats(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::UNIT_POWERED_SEATS.base;
    owned(pc, entry, &SEATS_SITES)?;
    // The seat's power, `[esi + ecx*4 + 25c]`.
    let power = |c: &CpuAbi| c.esi.wrapping_add(c.ecx.wrapping_mul(4)).wrapping_add(0x25c);
    let mut at = pc;
    loop {
        at = match at {
            0xc5eb0 => {
                push(c, g, c.ecx)?;
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(c.ecx.wrapping_add(0x44))?;
                c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                push(c, g, c.ebp)?;
                push(c, g, c.esi)?;
                push(c, g, c.edi)?;
                c.edi = c.eax;
                c.eax = (c.eax & 0xffff).wrapping_mul(3);
                c.esi = g.u32(c.edx.wrapping_add(c.eax.wrapping_mul(4)).wrapping_add(8))?;
                c.eax = (g.u32(c.esi)? & 0xffff) << 4;
                c.ebp = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(8))?;
                c.edx = g.u32(c.ebp.wrapping_add(0xbc))?;
                c.eax &= !0xff;
                c.edx >>= 0xb;
                g.set_u8(c.esp.wrapping_add(0xf), c.eax as u8)?;
                if c.edx & 1 != 0 {
                    0xc60ac
                } else {
                    push(c, g, c.ebx)?;
                    c.ebx = c.edi;
                    return call(c, callee::D0C10, SEATS_SITES[0]);
                }
            }
            0xc5eff => return call(c, points::UNIT_CONTROL_REFRESH.base, SEATS_SITES[1]),
            0xc5f04 => {
                c.eax = g.u32(c.esi.wrapping_add(0x134))? >> 0x1a;
                if c.eax & 1 == 0 {
                    0xc5f47
                } else {
                    c.ecx = g.u32(points::GAME_GLOBALS.base)?;
                    if g.u8(c.ecx.wrapping_add(0xc))? == 4 {
                        0xc5f47
                    } else {
                        c.ecx = g.u32(c.esi.wrapping_add(0x14))?;
                        c.eax = NONE;
                        if c.ecx == c.eax || g.u16(c.esi.wrapping_add(0x1fc))? == 0xffff {
                            0xc5f47
                        } else {
                            let flags = c.esi.wrapping_add(0x134);
                            let v = g.u32(flags)? & 0xfbff_ffff;
                            g.set_u32(flags, v)?;
                            push(c, g, c.edi)?;
                            c.ecx = 0x1e;
                            return call(c, callee::STATE_SET, SEATS_SITES[2]);
                        }
                    }
                }
            }
            0xc5f47 => {
                c.eax = g.u32(c.ebp.wrapping_add(0x1b8))?;
                c.ecx = 0;
                if (c.eax as i32) <= 0 {
                    0xc60a7
                } else {
                    c.ebx = g.u32(points::GAME_TIME.base)?;
                    sse::load(c, 0, g.u32(ONE)?);
                    sse::zero(c, 1);
                    0xc5f70
                }
            }
            0xc5f70 => {
                c.edx = g.u32(c.ebp.wrapping_add(0x1bc))?;
                c.edi = c.edx.wrapping_add(c.ecx.wrapping_mul(8));
                c.edx = c.ecx;
                c.eax &= !0xff;
                let occupied = match c.edx {
                    0 => {
                        // c5f9c: the driver, or body `+134` bit 1.
                        if g.u32(c.esi.wrapping_add(0x248))? != NONE {
                            true
                        } else {
                            c.eax = g.u32(c.esi.wrapping_add(0x134))? >> 1;
                            c.eax & 1 != 0
                        }
                    }
                    1 => {
                        // c5f82: `dec edx` leaves 0. A gunner who is not
                        // the driver.
                        c.edx = 0;
                        c.eax = g.u32(c.esi.wrapping_add(0x24c))?;
                        c.eax != NONE && c.eax != g.u32(c.esi.wrapping_add(0x248))?
                    }
                    _ => {
                        // `dec edx` leaves edx - 1.
                        c.edx = c.edx.wrapping_sub(1);
                        false
                    }
                };
                // `mov al, 1` (c5fb5) or `xor al, al`.
                c.eax = (c.eax & !0xff) | u32::from(occupied);
                let dl = g.u8(c.esi.wrapping_add(0x10a))? >> 2;
                c.edx = (c.edx & !0xff) | u32::from(dl);
                if dl & 1 == 0 && occupied { 0xc5fc9 } else { 0xc6031 }
            }
            0xc5fc9 => {
                // Toward 1.
                let p = power(c);
                sse::load(c, 2, g.u32(p)?);
                let one = sse::lane0(c, 0);
                if equal(c, sse::lane0(c, 2), one) {
                    0xc6098
                } else {
                    sse::load(c, 2, g.u32(c.edi)?);
                    let zero = sse::lane0(c, 1);
                    if below_or_equal(c, sse::lane0(c, 2), zero) {
                        st(c, g, p, 0)?;
                    } else {
                        c.eax = sx16(g.u16(c.ebx.wrapping_add(2))?);
                        sse::cvtsi2ss(c, 2, c.eax);
                        sse::ss(c, Ss::Mul, 2, g.u32(c.edi)?);
                        sse::movaps(c, 3, 0);
                        sse::ss_reg(c, Ss::Div, 3, 2);
                        sse::ss(c, Ss::Add, 3, g.u32(p)?);
                        st(c, g, p, 3)?;
                    }
                    // c6018
                    sse::load(c, 2, g.u32(p)?);
                    let one = sse::lane0(c, 0);
                    if !below_or_equal(c, sse::lane0(c, 2), one) {
                        st(c, g, p, 0)?;
                    }
                    0xc6093
                }
            }
            0xc6031 => {
                // Toward 0.
                let p = power(c);
                sse::load(c, 2, g.u32(p)?);
                let zero = sse::lane0(c, 1);
                if equal(c, sse::lane0(c, 2), zero) {
                    0xc6098
                } else {
                    sse::load(c, 2, g.u32(c.edi)?);
                    let zero = sse::lane0(c, 1);
                    if below_or_equal(c, sse::lane0(c, 2), zero) {
                        st(c, g, p, 1)?;
                    } else {
                        c.edx = sx16(g.u16(c.ebx.wrapping_add(2))?);
                        sse::cvtsi2ss(c, 2, c.edx);
                        sse::ss(c, Ss::Mul, 2, g.u32(c.edi)?);
                        sse::movaps(c, 3, 0);
                        sse::ss_reg(c, Ss::Div, 3, 2);
                        sse::load(c, 2, g.u32(p)?);
                        sse::ss_reg(c, Ss::Sub, 2, 3);
                        st(c, g, p, 2)?;
                    }
                    // c6080: `comiss xmm1, [p]; jbe`.
                    let zero = sse::lane0(c, 1);
                    if !below_or_equal(c, zero, g.u32(p)?) {
                        st(c, g, p, 1)?;
                    }
                    0xc6093
                }
            }
            0xc6093 => {
                g.set_u8(c.esp.wrapping_add(0x13), 1)?;
                0xc6098
            }
            0xc6098 => {
                c.eax = g.u32(c.ebp.wrapping_add(0x1b8))?;
                c.ecx = c.ecx.wrapping_add(1);
                if (c.ecx as i32) < (c.eax as i32) { 0xc5f70 } else { 0xc60a7 }
            }
            0xc60a7 => {
                let al = g.u8(c.esp.wrapping_add(0x13))?;
                c.eax = (c.eax & !0xff) | u32::from(al);
                c.ebx = pop(c, g)?;
                0xc60ac
            }
            0xc60ac => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ecx = pop(c, g)?;
                return ret(c, 0);
            }
            _ => return Err(unowned(at)),
        };
    }
}

// ---------------------------------------------------------------- ce0c0

/// `ce0c0`'s return sites: `e4050`, `d1080`, `138880`.
pub(in crate::sim_objects) const REFRESH_SITES: [u32; 3] = [0xce125, 0xce140, 0xce340];

/// `ce0c0 (unit: edi)`.
pub(in crate::sim_objects) fn refresh(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::UNIT_CONTROL_REFRESH.base;
    owned(pc, entry, &REFRESH_SITES)?;
    // Every site is at `E`.
    let mut at = pc;
    loop {
        at = match at {
            0xce0c0 => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(c.ecx.wrapping_add(0x44))?;
                c.eax = c.edi & 0xffff;
                c.esp = c.esp.wrapping_sub(0x6c);
                push(c, g, c.ebx)?;
                c.eax = c.eax.wrapping_mul(3) << 2;
                push(c, g, c.esi)?;
                c.esi = g.u32(c.eax.wrapping_add(c.edx).wrapping_add(8))?;
                let flags = c.esi.wrapping_add(0x134);
                c.ebx = g.u32(flags)? & 0xfbff_ffff;
                c.edx = c.ebx & 0xfdff_ffff;
                g.set_u32(flags, c.ebx)?;
                g.set_u32(flags, c.edx)?;
                let dl = g.u8(c.esi.wrapping_add(0x10a))? >> 2;
                c.edx = (c.edx & !0xff) | u32::from(dl);
                if dl & 1 == 0 {
                    0xce12d
                } else {
                    c.ecx = g.u32(c.ecx.wrapping_add(0x44))?;
                    c.edx = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(8))?;
                    let al = g.u8(c.edx.wrapping_add(0xaa))?;
                    c.eax = (c.eax & !0xff) | u32::from(al);
                    if al != 0 {
                        0xce129
                    } else {
                        c.eax = c.edi;
                        return call(c, callee::E4050, REFRESH_SITES[0]);
                    }
                }
            }
            0xce125 => {
                if c.eax as u8 != 0 {
                    0xce12d
                } else {
                    0xce129
                }
            }
            0xce129 => {
                c.ebx = (c.ebx & !0xff) | 1;
                0xce12f
            }
            0xce12d => {
                c.ebx &= !0xff;
                0xce12f
            }
            0xce12f => {
                c.eax = c.esp.wrapping_add(0x2c);
                push(c, g, c.eax)?;
                c.ecx = c.esp.wrapping_add(0x44);
                push(c, g, c.ecx)?;
                c.eax = c.edi;
                return call(c, callee::D1080, REFRESH_SITES[1]);
            }
            0xce140 => {
                if c.eax as u8 == 0 || c.ebx as u8 != 0 {
                    0xce39b
                } else {
                    return frame(c, g);
                }
            }
            0xce340 => {
                if c.eax as u8 != 0 {
                    0xce35d
                } else {
                    0xce344
                }
            }
            0xce344 => {
                let limit = sse::lane0(c, 0);
                let (a, b, d) = (sse::lane0(c, 2), sse::lane0(c, 3), sse::lane0(c, 4));
                if !below_or_equal(c, a, limit) || !below_or_equal(c, b, limit) || !below_or_equal(c, d, limit) {
                    let flags = c.esi.wrapping_add(0x134);
                    let v = g.u32(flags)? | 0x0400_0000;
                    g.set_u32(flags, v)?;
                }
                0xce35d
            }
            0xce35d => {
                store_position(c, g)?;
                0xce3b6
            }
            0xce39b => {
                sse::zero(c, 0);
                st(c, g, c.esi.wrapping_add(0x29c), 0)?;
                st(c, g, c.esi.wrapping_add(0x298), 0)?;
                st(c, g, c.esi.wrapping_add(0x294), 0)?;
                0xce3b6
            }
            0xce3b6 => return blend(c, g),
            _ => return Err(unowned(at)),
        };
    }
}

/// `ce150..ce33b`: the change in the control frame, its absolute values
/// against the two thresholds; ends at the `138880` call or falls into
/// `ce35d`.
fn frame(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let (e, base) = (c.esp, c.esi);
    let s = |o: u32| e.wrapping_add(o);
    let b = |o: u32| base.wrapping_add(o);
    let (b270, b274, b278, b27c, b280, b284) = (b(0x270), b(0x274), b(0x278), b(0x27c), b(0x280), b(0x284));
    let (b294, b298, b29c) = (b(0x294), b(0x298), b(0x29c));
    sse::load(c, 1, g.u32(s(0x4c))?);
    sse::load(c, 2, g.u32(s(0x64))?);
    sse::load(c, 4, g.u32(s(0x60))?);
    sse::load(c, 5, g.u32(s(0x5c))?);
    c.edx = g.u32(points::GAME_TIME.base)?;
    c.eax = sx16(g.u16(c.edx.wrapping_add(2))?);
    sse::movaps(c, 7, 1);
    sse::ss_reg(c, Ss::Mul, 7, 4);
    sse::ss(c, Ss::Mul, 4, g.u32(s(0x44))?);
    sse::movaps(c, 0, 2);
    sse::ss(c, Ss::Mul, 0, g.u32(s(0x48))?);
    sse::ss_reg(c, Ss::Sub, 7, 0);
    sse::movaps(c, 0, 1);
    sse::ss_reg(c, Ss::Mul, 0, 5);
    sse::movaps(c, 3, 2);
    sse::ss(c, Ss::Mul, 3, g.u32(s(0x44))?);
    sse::ss_reg(c, Ss::Sub, 3, 0);
    sse::load(c, 0, g.u32(s(0x48))?);
    sse::ss_reg(c, Ss::Mul, 0, 5);
    sse::load(c, 5, g.u32(s(0x70))?);
    sse::ss(c, Ss::Sub, 5, g.u32(b278)?);
    sse::ss_reg(c, Ss::Sub, 0, 4);
    sse::load(c, 4, g.u32(s(0x6c))?);
    sse::ss(c, Ss::Sub, 4, g.u32(b274)?);
    st(c, g, s(0x24), 3)?;
    sse::load(c, 3, g.u32(s(0x68))?);
    sse::ss(c, Ss::Sub, 3, g.u32(b270)?);
    sse::cvtsi2ss(c, 6, c.eax);
    sse::ss_reg(c, Ss::Mul, 3, 6);
    sse::ss_reg(c, Ss::Mul, 4, 6);
    sse::ss_reg(c, Ss::Mul, 5, 6);
    st(c, g, s(0x14), 3)?;
    sse::ss(c, Ss::Sub, 3, g.u32(b27c)?);
    sse::ss_reg(c, Ss::Mul, 3, 6);
    st(c, g, s(0x18), 4)?;
    sse::ss(c, Ss::Sub, 4, g.u32(b280)?);
    sse::ss_reg(c, Ss::Mul, 4, 6);
    st(c, g, s(0x1c), 5)?;
    sse::ss(c, Ss::Sub, 5, g.u32(b284)?);
    sse::ss_reg(c, Ss::Mul, 5, 6);
    sse::load(c, 6, g.u32(s(0x48))?);
    sse::ss_reg(c, Ss::Mul, 1, 5);
    sse::ss_reg(c, Ss::Mul, 6, 4);
    sse::ss_reg(c, Ss::Add, 1, 6);
    sse::load(c, 6, g.u32(s(0x44))?);
    sse::ss_reg(c, Ss::Mul, 0, 5);
    sse::ss_reg(c, Ss::Mul, 6, 3);
    sse::ss_reg(c, Ss::Add, 1, 6);
    sse::ss(c, Ss::Mul, 1, g.u32(s(0x2c))?);
    st(c, g, s(8), 1)?;
    // ce254: `fld [E+8]`, interleaved with the SSE work.
    let mut x = fpu(c);
    x.fld(g.u32(s(8))?).map_err(fp_error)?;
    st(c, g, b294, 1)?;
    sse::load(c, 1, g.u32(s(0x24))?);
    super::fabs(&mut x).map_err(fp_error)?;
    sse::ss_reg(c, Ss::Mul, 1, 4);
    sse::ss_reg(c, Ss::Add, 0, 1);
    sse::ss_reg(c, Ss::Mul, 7, 3);
    sse::ss_reg(c, Ss::Add, 0, 7);
    sse::ss(c, Ss::Mul, 0, g.u32(s(0x30))?);
    st(c, g, s(0xc), 0)?;
    st(c, g, b298, 0)?;
    sse::load(c, 0, g.u32(s(0x60))?);
    sse::ss_reg(c, Ss::Mul, 2, 5);
    sse::ss_reg(c, Ss::Mul, 0, 4);
    sse::ss_reg(c, Ss::Add, 2, 0);
    sse::load(c, 0, g.u32(s(0x5c))?);
    // ce2a4: `fstp [E+8]; fld [E+c]`.
    g.set_u32(s(8), x.fstp_m32().map_err(fp_error)?)?;
    x.fld(g.u32(s(0xc))?).map_err(fp_error)?;
    sse::ss_reg(c, Ss::Mul, 0, 3);
    super::fabs(&mut x).map_err(fp_error)?;
    sse::ss_reg(c, Ss::Add, 2, 0);
    sse::ss(c, Ss::Mul, 2, g.u32(s(0x34))?);
    st(c, g, s(0x10), 2)?;
    // ce2c2: `fstp [E+c]; fld [E+10]`.
    g.set_u32(s(0xc), x.fstp_m32().map_err(fp_error)?)?;
    x.fld(g.u32(s(0x10))?).map_err(fp_error)?;
    st(c, g, b29c, 2)?;
    super::fabs(&mut x).map_err(fp_error)?;
    sse::load(c, 0, g.u32(s(0x38))?);
    sse::zero(c, 1);
    let positive = !below_or_equal(c, sse::lane0(c, 0), 0);
    sse::load(c, 2, g.u32(s(8))?);
    sse::load(c, 3, g.u32(s(0xc))?);
    // ce2ec: `fstp [E+10]`, the last x87 instruction.
    g.set_u32(s(0x10), x.fstp_m32().map_err(fp_error)?)?;
    fpu_done(c, g, x, 0xce2ec, s(0x10))?;
    sse::load(c, 4, g.u32(s(0x10))?);
    if positive {
        let limit = sse::lane0(c, 0);
        let (a, b, d) = (sse::lane0(c, 2), sse::lane0(c, 3), sse::lane0(c, 4));
        if !below_or_equal(c, a, limit) || !below_or_equal(c, b, limit) || !below_or_equal(c, d, limit) {
            let flags = c.esi.wrapping_add(0x134);
            let v = g.u32(flags)? | 0x0200_0000;
            g.set_u32(flags, v)?;
        }
    }
    // ce311: the second threshold.
    sse::load(c, 0, g.u32(s(0x3c))?);
    let zero = sse::lane0(c, 1);
    if below_or_equal(c, sse::lane0(c, 0), zero) {
        store_position(c, g)?;
        return blend(c, g);
    }
    c.ecx = g.u32(c.esi.wrapping_add(0x14))?;
    c.eax = NONE;
    if c.ecx == c.eax || g.u16(c.esi.wrapping_add(0x1fc))? == 0xffff {
        store_position(c, g)?;
        return blend(c, g);
    }
    c.ecx = g.u32(points::GAME_GLOBALS.base)?;
    if g.u32(c.ecx.wrapping_add(8))? == 2 {
        return call(c, callee::F138880, REFRESH_SITES[2]);
    }
    // ce344
    let limit = sse::lane0(c, 0);
    let (a, b, d) = (sse::lane0(c, 2), sse::lane0(c, 3), sse::lane0(c, 4));
    if !below_or_equal(c, a, limit) || !below_or_equal(c, b, limit) || !below_or_equal(c, d, limit) {
        let flags = c.esi.wrapping_add(0x134);
        let v = g.u32(flags)? | 0x0400_0000;
        g.set_u32(flags, v)?;
    }
    store_position(c, g)?;
    blend(c, g)
}

/// `ce35d`: the position (`[E+68..70]`) and its change (`[E+14..1c]`)
/// kept at body `+270` and `+27c`.
fn store_position(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let (e, base) = (c.esp, c.esi);
    let s = |o: u32| e.wrapping_add(o);
    let b = |o: u32| base.wrapping_add(o);
    c.edx = g.u32(s(0x68))?;
    c.eax = g.u32(s(0x6c))?;
    c.ecx = g.u32(s(0x70))?;
    g.set_u32(b(0x270), c.edx)?;
    c.edx = g.u32(s(0x14))?;
    g.set_u32(b(0x274), c.eax)?;
    c.eax = g.u32(s(0x18))?;
    g.set_u32(b(0x278), c.ecx)?;
    c.ecx = g.u32(s(0x1c))?;
    g.set_u32(b(0x27c), c.edx)?;
    g.set_u32(b(0x280), c.eax)?;
    g.set_u32(b(0x284), c.ecx)?;
    Ok(())
}

/// `ce3b6` to the `ret`: `+288..290` blended toward `+294..29c`.
fn blend(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let base = c.esi;
    let b = |o: u32| base.wrapping_add(o);
    c.edx = g.u32(b(0x134))?;
    sse::load(c, 0, g.u32(BLEND.0)?);
    sse::load(c, 1, g.u32(BLEND.1)?);
    c.edx >>= 0x1c;
    if c.edx & 1 != 0 {
        c.eax = g.u32(b(0x134))?;
        sse::load(c, 0, g.u32(BLEND_RESET.0)?);
        sse::load(c, 1, g.u32(BLEND_RESET.1)?);
        c.eax &= 0xefff_ffff;
        g.set_u32(b(0x134), c.eax)?;
    }
    sse::load(c, 2, g.u32(b(0x288))?);
    sse::load(c, 3, g.u32(b(0x294))?);
    sse::ss_reg(c, Ss::Mul, 2, 0);
    sse::ss_reg(c, Ss::Mul, 3, 1);
    sse::ss_reg(c, Ss::Add, 2, 3);
    st(c, g, b(0x288), 2)?;
    sse::load(c, 2, g.u32(b(0x298))?);
    sse::ss_reg(c, Ss::Mul, 2, 1);
    sse::movaps(c, 3, 0);
    sse::ss(c, Ss::Mul, 3, g.u32(b(0x28c))?);
    sse::ss_reg(c, Ss::Add, 2, 3);
    st(c, g, b(0x28c), 2)?;
    sse::load(c, 2, g.u32(b(0x290))?);
    sse::ss_reg(c, Ss::Mul, 2, 0);
    sse::load(c, 0, g.u32(b(0x29c))?);
    sse::ss_reg(c, Ss::Mul, 0, 1);
    sse::ss_reg(c, Ss::Add, 2, 0);
    st(c, g, b(0x290), 2)?;
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = c.esp.wrapping_add(0x6c);
    ret(c, 0)
}
