//! The biped's physics input `dd0d0`: the record the physics step
//! `1e68a0` takes, and the state's share of its answer.

use super::{NONE, and32, at, below_or_equal, call, flags8, sx8, sx16};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_objects::alu;
use crate::sim_objects::biped::less;
use crate::sim_objects::frame::{set_slot, slot};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use runtime::{CpuAbi, Guest};

/// `dd0d0`'s locals.
const FRAME: u32 = 0x44;
/// `.rdata` constant: f32 at `45dd74` (a grounded time in seconds).
const K_45DD74: u32 = 0x0045_dd74;
/// The jump table `jmp [ecx*4 + dd2e4]` reads, by the state less 1.
const STATE_TABLE: u32 = 0x000d_d2e4;

/// Callees that stay guest.
const BA160: u32 = 0x000b_a160;
const X1E68A0: u32 = 0x001e_68a0;
const X1D0870: u32 = 0x001d_0870;

/// Every return address of `dd0d0`'s calls.
pub(in crate::sim_objects) const INPUT_SITES: [u32; 4] = [0xdd121, 0xdd217, 0xdd23b, 0xdd2bd];

/// `pop ebp`, `pop ebx`, drop the frame, `ret c`.
fn input_ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    Ok(Next::Ret { pop: 12 })
}

/// A byte's bit `n` into `r`'s low byte (`shr r8, n; and r8, 1`), stored
/// at `[esp+58]` (the object argument's slot, reused).
fn bit_to_arg(c: &mut CpuAbi, g: &mut Guest, v: u8, n: u32) -> Result<u8> {
    let b = alu::shr(c, v.into(), n, 8) as u8;
    let b = flags8(c, b & 1);
    g.set_u8(at(c.esp, 0x58), b)?;
    Ok(b)
}

/// `dd0d0 (object: stack 0, record: stack 1, flag: stack 2)`; `ret c`.
/// Frame: `sub esp, 44`, then `ebx`, `ebp`, `esi`, `edi` pushed (`S` =
/// that `esp`); `ebp` the object (then the record), `esi` the body, `ebx`
/// the body `+ (i16) [+33e]` (then a flag byte), `edi` the header offset
/// (then body `+3dc`). With `S`: `[S+10]` the definition, `[S+14..1c]` a
/// vector (the velocity `ba160` writes at `[S+20]`'s `+4` when definition
/// `+bc` bit 11 is set, else body `+168`; then `1d0870`'s out), `[S+48]`
/// a buffer; the object's slot `[S+58]` takes three flag bytes in turn,
/// then the early mover (body `+b8`, when `+c0` bit 1 or 2) or none.
///
/// `e4050` (fused) runs with six of `1e68a0`'s arguments already pushed:
/// body `+1b0`, `S+14`, `S+48`, the flag, whether the counter `+39c` is
/// below `[45dd74]` seconds, and whether the word at `ebx` has bits 2 and
/// 4. `1e68a0` takes four more (definition `+264`, body `+b4`, the slot,
/// `e4050`'s answer) with `eax` the record, `ecx` body `+3dc`, `bl` body
/// `+c1` bit 0. Then, by the state (the image's jump table): 1 stores body
/// `+408` at record `+44`; 3 the bit `+34b != 0` at `+48`; 4 and 5 the
/// component's rigid-body index (or none) through `1d0870` into `+4c`;
/// every state sets record `+14` to 1.
pub(in crate::sim_objects) fn physics_input(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_PHYSICS_INPUT.base;
    match pc {
        p if p == entry => {
            c.esp = arithmetic(c, c.esp, FRAME, true);
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = slot(c, g, 0x50)?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.eax = c.ebp;
            c.eax = and32(c, c.eax & 0xffff);
            c.edi = c.eax.wrapping_mul(3);
            c.eax = g.u32(points::OBJECTS.base)?;
            c.ecx = g.u32(at(c.eax, DATA))?;
            c.eax = g.u32(points::TAG_ENTRIES.base)?;
            c.edi = alu::shl(c, c.edi, 2, 32);
            c.esi = g.u32(at(c.ecx.wrapping_add(c.edi), 8))?;
            c.edx = g.u32(c.esi)?;
            c.ebx = sx16(g.u16(at(c.esi, 0x33e))?);
            c.edx = and32(c, c.edx & 0xffff);
            c.edx = alu::shl(c, c.edx, 4, 32);
            c.ecx = g.u32(at(at(c.edx, c.eax), 8))?;
            c.edx = at(c.esp, 0x20);
            push(c, g, c.edx)?;
            c.eax = c.ebp;
            set_slot(c, g, 0x14, c.ecx)?;
            c.ebx = arithmetic(c, c.ebx, c.esi, false);
            call(BA160, 0xdd121)
        }
        0xdd121 => {
            c.eax = slot(c, g, 0x10)?;
            c.ecx = g.u32(at(c.eax, 0xbc))?;
            c.ecx = alu::shr(c, c.ecx, 0xb, 32);
            let cl = flags8(c, c.ecx as u8 & 1);
            c.ecx = byte(c.ecx, cl);
            g.set_u8(at(c.esp, 0x58), cl)?;
            if cl != 0 {
                c.edx = slot(c, g, 0x24)?;
                c.eax = slot(c, g, 0x28)?;
                c.ecx = slot(c, g, 0x2c)?;
            } else {
                c.edx = g.u32(points::OBJECTS.base)?;
                c.eax = g.u32(at(c.edx, DATA))?;
                c.ecx = g.u32(at(c.eax.wrapping_add(c.edi), 8))?;
                c.ecx = arithmetic(c, c.ecx, 0x168, false);
                c.edx = g.u32(c.ecx)?;
                c.eax = g.u32(at(c.ecx, 4))?;
                c.ecx = g.u32(at(c.ecx, 8))?;
            }
            // `dd160`.
            sse::load(c, 1, g.u32(K_45DD74)?);
            set_slot(c, g, 0x14, c.edx)?;
            c.edx = sx8(g.u8(at(c.esi, 0x39c))?);
            set_slot(c, g, 0x18, c.eax)?;
            c.eax = g.u32(points::GAME_TIME.base)?;
            sse::cvtsi2ss(c, 0, c.edx);
            sse::ss(c, Ss::Mul, 0, g.u32(at(c.eax, 4))?);
            let (x0, x1) = (sse::lane0(c, 0), sse::lane0(c, 1));
            let short = !below_or_equal(c, x1, x0);
            set_slot(c, g, 0x1c, c.ecx)?;
            if short {
                c.edx = byte(c.edx, 1);
            } else {
                c.edx = byte(c.edx, 0);
                logic(c, 0, 0x80);
            }
            // `dd194`: the word at `ebx`, bits 2 and 4.
            let bx = g.u16(c.ebx)?;
            c.ebx = (c.ebx & 0xffff_0000) | u32::from(bx);
            let cl = bit_to_arg(c, g, bx as u8, 2)?;
            c.ecx = byte(c.ecx, cl);
            let mut both = false;
            if cl != 0 {
                let bl = bit_to_arg(c, g, bx as u8, 4)?;
                c.ebx = byte(c.ebx, bl);
                both = bl != 0;
            }
            if both {
                c.ecx = byte(c.ecx, 1);
            } else {
                c.ecx = byte(c.ecx, 0);
                logic(c, 0, 0x80);
            }
            // `dd1b7`: body `+c0` bits 2 and 1 pick the early mover.
            let ax = g.u16(at(c.esi, 0xc0))?;
            c.eax = (c.eax & 0xffff_0000) | u32::from(ax);
            let mut bl = bit_to_arg(c, g, ax as u8, 2)?;
            if bl == 0 {
                bl = bit_to_arg(c, g, ax as u8, 1)?;
            }
            c.ebx = byte(c.ebx, bl);
            if bl != 0 {
                c.edi = g.u32(at(c.esi, 0xb8))?;
                set_slot(c, g, 0x58, c.edi)?;
            } else {
                set_slot(c, g, 0x58, NONE)?;
            }
            // `dd1ed`.
            c.ebx = byte(c.ebx, (ax >> 8) as u8);
            c.eax = at(c.esi, 0x1b0);
            push(c, g, c.eax)?;
            c.eax = at(c.esp, 0x18);
            push(c, g, c.eax)?;
            c.eax = at(c.esp, 0x50);
            push(c, g, c.eax)?;
            c.eax = slot(c, g, 0x6c)?;
            push(c, g, c.eax)?;
            push(c, g, c.edx)?;
            push(c, g, c.ecx)?;
            c.eax = c.ebp;
            let bl = flags8(c, c.ebx as u8 & 1);
            c.ebx = byte(c.ebx, bl);
            c.edi = at(c.esi, 0x3dc);
            call(points::BIPED_AIRBORNE_TEST.base, 0xdd217)
        }
        0xdd217 => {
            c.ecx = slot(c, g, 0x70)?;
            c.edx = g.u32(at(c.esi, 0xb4))?;
            c.ebp = slot(c, g, 0x74)?;
            push(c, g, c.eax)?;
            c.eax = slot(c, g, 0x2c)?;
            push(c, g, c.ecx)?;
            c.eax = arithmetic(c, c.eax, 0x264, false);
            push(c, g, c.edx)?;
            push(c, g, c.eax)?;
            c.ecx = c.edi;
            c.eax = c.ebp;
            call(X1E68A0, 0xdd23b)
        }
        0xdd23b => {
            c.ecx = u32::from(g.u8(c.edi)?);
            c.ecx = alu::dec32(c, c.ecx);
            let target = g.u32(STATE_TABLE.wrapping_add(c.ecx.wrapping_mul(4)))?;
            match target {
                0xdd246 => {
                    c.edi = pop(c, g)?;
                    g.set_u8(at(c.ebp, 0x14), 1)?;
                    c.edx = g.u32(at(c.esi, 0x408))?;
                    c.esi = pop(c, g)?;
                    g.set_u32(at(c.ebp, 0x44), c.edx)?;
                    input_ret(c, g)
                }
                0xdd25d | 0xdd26b => {
                    if target == 0xdd25d {
                        let al = flags8(c, g.u8(at(c.esi, 0x34b))?);
                        c.eax = byte(c.eax, u8::from(al != 0));
                        g.set_u8(at(c.ebp, 0x48), c.eax as u8)?;
                    }
                    c.edi = pop(c, g)?;
                    c.esi = pop(c, g)?;
                    g.set_u8(at(c.ebp, 0x14), 1)?;
                    input_ret(c, g)
                }
                0xdd279 => {
                    // The component's rigid-body index, when it is
                    // below the count `+74`; else none.
                    c.esi = g.u32(at(c.esi, 0xb4))?;
                    c.edx = g.u32(points::PHYSICS_COMPONENTS.base)?;
                    c.esi = and32(c, c.esi & 0xffff);
                    c.ecx = c.esi.wrapping_mul(5);
                    c.esi = g.u32(at(c.edx, DATA))?;
                    c.ecx = alu::shl(c, c.ecx, 5, 32);
                    let al = g.u8(at(c.ecx.wrapping_add(c.esi), 0x18))?;
                    c.eax = byte(c.eax, al);
                    c.ecx = arithmetic(c, c.ecx, c.esi, false);
                    flags8(c, al);
                    let mut valid = false;
                    if (al as i8) >= 0 {
                        c.esi = g.u32(at(c.ecx, 0x74))?;
                        c.edx = sx8(al);
                        arithmetic(c, c.edx, c.esi, true);
                        valid = less(c);
                    }
                    if valid {
                        c.eax = (c.eax & 0xffff_0000) | (sx8(al) & 0xffff);
                    } else {
                        c.eax = NONE;
                        logic(c, NONE, 1 << 31);
                    }
                    c.eax = sx16(c.eax as u16);
                    c.edi = at(c.esp, 0x14);
                    call(X1D0870, 0xdd2bd)
                }
                _ => Err(unowned(target)),
            }
        }
        0xdd2bd => {
            c.ecx = slot(c, g, 0x14)?;
            c.edx = slot(c, g, 0x18)?;
            c.eax = at(c.ebp, 0x4c);
            c.edi = pop(c, g)?;
            g.set_u32(c.eax, c.ecx)?;
            c.ecx = slot(c, g, 0x18)?;
            c.esi = pop(c, g)?;
            g.set_u8(at(c.ebp, 0x14), 1)?;
            g.set_u32(at(c.eax, 4), c.edx)?;
            c.ebp = pop(c, g)?;
            g.set_u32(at(c.eax, 8), c.ecx)?;
            c.ebx = pop(c, g)?;
            c.esp = arithmetic(c, c.esp, FRAME, false);
            Ok(Next::Ret { pop: 12 })
        }
        _ => Err(unowned(pc)),
    }
}
