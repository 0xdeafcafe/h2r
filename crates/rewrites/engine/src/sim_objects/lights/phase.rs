//! The light record's modulation step `c0b00` (`c0c80` calls it once per
//! update, at `c0f70`).
//!
//! **`c0b00 (colour: eax, block: ecx, light: stack 0, input: stack 1,
//! brightness: stack 2, out: stack 3)`; `ret 10`.** `c0c80` passes the
//! light element's `+d4` colour triple in `eax`, its definition's `+c0`
//! block in `ecx`, its handle, its intensity (element `+d0`), and the
//! addresses of element `+ec` and `+f0`. Saves `ebx` (the block), `ebp`
//! (the input from `c0b49`), `esi` (the colour) and `edi`.
//!
//! 1. The phase, in `xmm0`, stored over the handle's argument slot: game
//!    time `+8` × `+4` when block flags bit 0 is set; otherwise
//!    `((handle × 1387) & 7fffffff) mod (ticks per second × 3c) + +8`
//!    (`idiv`), × `+4`, a per-light offset into a one-minute cycle.
//! 2. Block `+4` positive: the brightness function `13b390(phase, input)`
//!    (`edi` block `+8`, `ret 8`) scales `[brightness]`; unless the
//!    function's flags byte `+1` has a bit of `f0` set, the result is
//!    clamped to `[0, 1]` and mapped to `[+4, +8]` of the function first.
//! 3. Block `+c` positive: the colour function `13b390`, then `13bc00`
//!    (`ecx` block `+10`, the value in a pushed slot, `ret 4`) returns a
//!    packed RGB in `eax`; each byte scales one colour channel through the
//!    x87 (`fild`, `fmul` by the channel, `fmul` by the image's byte
//!    scale, `fstp`). The bytes are stored over the input's argument slot
//!    first.
//! 4. Block `+14` positive: two `13bb90(phase, input)` calls (`eax` block
//!    `+18`, then `+20`, `ret 8`) store `[out]` and `[out + 4]`.
//!
//! An `idiv` by zero (a game time record with no ticks per second) is the
//! guest's divide error; the body reports it as a fault at `c0b3b`.

use super::{at, flags_undefined, fpu, fpu_done};
use guest_layout::points;
use crate::sim_objects::{Cx, Next, Result, sse, unowned};
use crate::sim_physics::{pop, push};
use runtime::{CpuAbi, Guest, GuestFault};
use sse::Ss::{Add, Mul, Sub};
use x87::abi::{self as fp, arith_op, src_kind};

/// The function evaluators.
const FUNCTION: u32 = 0x0013_b390;
const COLOUR: u32 = 0x0013_bc00;
const SOURCE: u32 = 0x0013_bb90;
/// Binary32 one; the x87 correction for a negative `fild` (never taken:
/// each byte is masked to `0..ff`); the byte scale.
const ONE: u32 = 0x0045_dbc0;
const UNSIGNED: u32 = 0x0045_dbb8;
const BYTE_SCALE: u32 = 0x0045_dbb4;

/// After the five calls.
pub(in crate::sim_objects) const PHASE_SITES: [u32; 5] = [0xc0b68, 0xc0bcb, 0xc0bd8, 0xc0c62, 0xc0c74];

fn divide_error() -> GuestFault {
    GuestFault {
        addr: 0xc0b3b,
        len: 0,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::InvalidOperation,
    }
}

/// `fild dword [esp+18]; jge; fadd [UNSIGNED]; fmul dword [channel];
/// fmul dword [BYTE_SCALE]; fstp dword [channel]`, with the sign test
/// on `v` as the preceding `test` left it.
fn channel(c: &mut CpuAbi, g: &mut Guest, v: u32, channel: u32, fstp_pc: u32) -> Result<()> {
    let mut x = fpu(c);
    fp::ld_int(&mut x, i64::from(v as i32));
    if (v as i32) < 0 {
        fp::arith(&mut x, arith_op::ADD, 0, src_kind::F32, g.u32(UNSIGNED)?.into());
    }
    fp::arith(&mut x, arith_op::MUL, 0, src_kind::F32, g.u32(channel)?.into());
    fp::arith(&mut x, arith_op::MUL, 0, src_kind::F32, g.u32(BYTE_SCALE)?.into());
    let (_, s) = fp::st_f32(&x, 0);
    g.set_u32(channel, s.value)?;
    fp::store_commit(&mut x, s.status, true);
    fpu_done(c, g, &x, fstp_pc, channel)
}

/// `c0c79`: restore and `ret 10`.
fn leave(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    flags_undefined(c);
    Ok(Next::Ret { pop: 0x10 })
}

/// `c0b00`; `ret 10`.
pub(in crate::sim_objects) fn phase(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::LIGHT_RECORD_PHASE.base;
    if pc != entry && !PHASE_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut next = pc;
    loop {
        next = match next {
            _ if next == entry => {
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                push(c, g, c.esi)?;
                c.ebx = c.ecx;
                c.esi = c.eax;
                let timed = g.u8(c.ebx)? & 1 != 0;
                push(c, g, c.edi)?;
                if timed {
                    c.eax = g.u32(points::GAME_TIME.base)?;
                    sse::cvtsi2ss(c, 0, g.u32(at(c.eax, 8))?);
                    sse::ss(c, Mul, 0, g.u32(at(c.eax, 4))?);
                } else {
                    c.ecx = g.u32(points::GAME_TIME.base)?;
                    c.eax = g.u32(at(c.esp, 0x14))?;
                    c.edi = g.u16(at(c.ecx, 2))? as i16 as i32 as u32;
                    c.eax = c.eax.wrapping_mul(0x1387);
                    c.edi = c.edi.wrapping_mul(0x3c);
                    c.eax &= 0x7fff_ffff;
                    // `cdq; idiv edi`: the dividend is not negative.
                    if c.edi == 0 {
                        return Err(divide_error());
                    }
                    let (n, d) = (c.eax as i32, c.edi as i32);
                    c.eax = n.wrapping_div(d) as u32;
                    c.edx = n.wrapping_rem(d) as u32;
                    c.edx = c.edx.wrapping_add(g.u32(at(c.ecx, 8))?);
                    sse::cvtsi2ss(c, 0, c.edx);
                    sse::ss(c, Mul, 0, g.u32(at(c.ecx, 4))?);
                }
                // c0b49
                c.eax = g.u32(at(c.ebx, 4))?;
                c.ebp = g.u32(at(c.esp, 0x18))?;
                g.set_u32(at(c.esp, 0x14), sse::lane0(c, 0))?;
                if (c.eax as i32) <= 0 {
                    0xc0bb2
                } else {
                    c.eax = g.u32(at(c.esp, 0x14))?;
                    c.edi = g.u32(at(c.ebx, 8))?;
                    push(c, g, c.ebp)?;
                    push(c, g, c.eax)?;
                    flags_undefined(c);
                    return Ok(Next::Call { target: FUNCTION, site: 0xc0b68 });
                }
            }
            0xc0b68 => {
                c.edi = g.u32(at(c.edi, 4))?;
                if g.u8(at(c.edi, 1))? & 0xf0 == 0 {
                    sse::zero(c, 3);
                    sse::comiss(c, sse::lane0(c, 3), sse::lane0(c, 0));
                    let below = !sse::below_or_equal(c);
                    sse::load(c, 2, g.u32(at(c.edi, 4))?);
                    sse::load(c, 1, g.u32(at(c.edi, 8))?);
                    let clamp = below || {
                        sse::load(c, 3, g.u32(ONE)?);
                        sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 3));
                        !sse::below_or_equal(c)
                    };
                    if clamp {
                        sse::movaps(c, 0, 3);
                    }
                    sse::ss_reg(c, Sub, 1, 2);
                    sse::ss_reg(c, Mul, 1, 0);
                    sse::ss_reg(c, Add, 1, 2);
                    sse::movaps(c, 0, 1);
                }
                // c0ba2
                c.eax = g.u32(at(c.esp, 0x1c))?;
                sse::load(c, 1, g.u32(c.eax)?);
                sse::ss_reg(c, Mul, 1, 0);
                g.set_u32(c.eax, sse::lane0(c, 1))?;
                0xc0bb2
            }
            0xc0bb2 => {
                c.eax = g.u32(at(c.ebx, 0xc))?;
                if (c.eax as i32) <= 0 {
                    0xc0c4b
                } else {
                    c.ecx = g.u32(at(c.esp, 0x14))?;
                    c.edi = g.u32(at(c.ebx, 0x10))?;
                    push(c, g, c.ebp)?;
                    push(c, g, c.ecx)?;
                    flags_undefined(c);
                    return Ok(Next::Call { target: FUNCTION, site: 0xc0bcb });
                }
            }
            0xc0bcb => {
                push(c, g, c.ecx)?;
                c.ecx = c.edi;
                g.set_u32(c.esp, sse::lane0(c, 0))?;
                flags_undefined(c);
                return Ok(Next::Call { target: COLOUR, site: 0xc0bd8 });
            }
            0xc0bd8 => {
                c.edx = (c.eax >> 16) & 0xff;
                g.set_u32(at(c.esp, 0x18), c.edx)?;
                channel(c, g, c.edx, c.esi, 0xc0c0e)?;
                c.ecx = (c.eax >> 8) & 0xff;
                g.set_u32(at(c.esp, 0x18), c.ecx)?;
                channel(c, g, c.ecx, at(c.esi, 4), 0xc0c30)?;
                c.eax &= 0xff;
                g.set_u32(at(c.esp, 0x18), c.eax)?;
                channel(c, g, c.eax, at(c.esi, 8), 0xc0c48)?;
                0xc0c4b
            }
            0xc0c4b => {
                c.eax = g.u32(at(c.ebx, 0x14))?;
                if (c.eax as i32) <= 0 {
                    return leave(c, g);
                }
                c.ebx = g.u32(at(c.ebx, 0x18))?;
                c.esi = g.u32(at(c.esp, 0x14))?;
                push(c, g, c.ebp)?;
                push(c, g, c.esi)?;
                c.eax = c.ebx;
                flags_undefined(c);
                return Ok(Next::Call { target: SOURCE, site: 0xc0c62 });
            }
            0xc0c62 => {
                c.edi = g.u32(at(c.esp, 0x20))?;
                push(c, g, c.ebp)?;
                push(c, g, c.esi)?;
                c.eax = at(c.ebx, 8);
                g.set_u32(c.edi, sse::lane0(c, 0))?;
                flags_undefined(c);
                return Ok(Next::Call { target: SOURCE, site: 0xc0c74 });
            }
            0xc0c74 => {
                g.set_u32(at(c.edi, 4), sse::lane0(c, 0))?;
                return leave(c, g);
            }
            _ => return Err(unowned(next)),
        };
    }
}
