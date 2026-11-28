//! The item update `10b4d0` (slice 11 of docs/engine/sim-rewrite-objects.md;
//! contract in docs/engine/object-update.md, "Slice 11 bodies"): the `item`
//! descriptor's slot `+40`, which weapons, equipment and garbage inherit
//! (a weapon runs it before its own `fdfb0`).
//!
//! **`10b4d0 (object: stack 0) -> al`; `ret 4`.** Frame: `sub esp, ac`,
//! then `ebx` (the object, all through), `ebp` (the body), `esi` (the
//! definition, tag entry `+8`), and `edi` once body `+04` bit 8 is set.
//! With `S` the `esp` after `edi` is pushed: `[S+13]` the result byte,
//! `[S+14..+20]` a vector, `[S+20]` the definition again, `[S+24]` body
//! `+140`, `[S+28..+b8]` the garbage-collection scratch, the object at
//! `[S+c0]`. Without `edi` pushed, the same slots are 4 lower.
//!
//! In order, when the object is in the world (body `+04` bit 8):
//!
//! 1. With body `+c1` bit 0 (woken by an early mover) and not `+bc` bit 2:
//!    - body `+12c` bit 5 with `+130` equal to the word at `4686c4`: when
//!      `+136` bit 3 is set, `184000(eax: +137, ecx: +132)`; on 0, clear
//!      `+12c` bit 5 and apply the scaled gravity vector (below);
//!    - else body `+12c` bit 6: `badc0(edi: +13c, -1)`, `ret 4`; when it
//!      answers, the parent's node matrix (`b8bd0`), a transform
//!      (`142570`) and `10d710(object, 0)` (`edi` = body `+158`, `ret
//!      8`); else clear `+12c` bit 6 and apply the gravity vector;
//!    - gravity: game time `+4` × `[51e9c4]` scales the vector at
//!      `[4687bc]` into `[S+14..+20]`, passed to `10cf80(object, 0)`
//!      (`eax` = `&S[14]`, `ret 8`).
//!
//!    Each of these sets the result.
//! 2. Body `+12c` bit 4: velocity `+94..+9c` scaled by the constant at
//!    `45dc84` and `10d5f0(eax: object)`; result 1.
//! 3. Unless body `+c1` bit 0: wake for post (`bba20`, a slice 1 body);
//!    result 1.
//! 4. The garbage timer, body `+12e` (word): when positive it counts down;
//!    unless the network mode (`[[4e6948]+c]`) is 4, at zero the item
//!    deletes itself: `bc190`, then, when definition `+110` is set, an
//!    effect from the object's transform (`b9dd0`, `d6660`, `ba300`,
//!    `d6c80`), then `176780(definition +128, 0, 0, 0)` with `edx` the
//!    object, `ebx` = `&S[14]`, `xmm0` zero, `ret 10`, and `OBJECT_DELETE`
//!    `b8540(object)`. A positive timer sets the result.
//!
//! Then for every object: with body `+c8` (in the world) or `+14` (not)
//! not none and definition `+104` positive, `16eb20(&definition[104])`;
//! and always `10b850(object)` (`ret 4`, it reads the early mover's
//! record, docs/engine/object-update.md "Who reads it, and when").
//!
//! Exit: `al` the result byte over `10b850`'s `eax`; `ecx`, `edx` and the
//! `xmm` registers as the last callee left them; flags from `add esp, ac`.

use super::alu::{cmp16, dec32, less_or_equal, rmw8, shr, test16, test8};
use super::sse::{self, Ss};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use runtime::{CpuAbi, Guest};

const NONE: u32 = u32::MAX;
const FRAME: u32 = 0xac;

/// The word body `+130` is compared with (O: meaning).
const SPECIAL_WORD: u32 = 0x0046_86c4;
/// The gravity-like vector the item falls by (O: meaning).
const FALL_VECTOR: u32 = 0x0046_87bc;
/// Holds a pointer to the `f32` the fall is scaled by.
const FALL_SCALE: u32 = 0x0051_e9c4;
/// The velocity damping factor (bit 4 of `+12c`).
const DAMPING: u32 = 0x0045_dc84;
/// `1.0f`, stored into the effect scratch.
const ONE: u32 = 0x0045_dbc0;

const GROUND_TEST: u32 = 0x0018_4000;
const OBJECT_LOOKUP: u32 = 0x000b_adc0;
const NODE_MATRICES: u32 = 0x000b_8bd0;
const TRANSFORM: u32 = 0x0014_2570;
const ATTACHED_MOVE: u32 = 0x0010_d710;
const FALL: u32 = 0x0010_cf80;
const DAMPED: u32 = 0x0010_d5f0;
const BEFORE_DELETE: u32 = 0x000b_c190;
const OBJECT_ORIGIN: u32 = 0x000b_9dd0;
const EFFECT_SETUP: u32 = 0x000d_6660;
const OBJECT_LOCATION: u32 = 0x000b_a300;
const EFFECT_NEW: u32 = 0x000d_6c80;
const DELETE_EFFECT: u32 = 0x0017_6780;
const DEFINITION_CALL: u32 = 0x0016_eb20;
const MOVER_FOLLOW: u32 = 0x0010_b850;

/// Every return address of `10b4d0`'s calls, address order.
pub(super) const ITEM_SITES: [u32; 17] = [
    0x10b584, 0x10b5ed, 0x10b60b, 0x10b619, 0x10b629, 0x10b683, 0x10b6e1, 0x10b6f6, 0x10b739, 0x10b75d,
    0x10b767, 0x10b7a3, 0x10b7d3, 0x10b7f2, 0x10b7ff, 0x10b836, 0x10b83c,
];

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

/// The fall vector into `[esp+14..+20]` (`10b593..10b5ce` and
/// `10b62b..10b661`): `xmm0` the scale, `xmm1` the last component,
/// not yet multiplied.
fn fall_vector(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.eax = g.u32(points::GAME_TIME.base)?;
    sse::load(c, 0, g.u32(at(c.eax, 4))?);
    c.eax = g.u32(FALL_VECTOR)?;
    c.ecx = g.u32(FALL_SCALE)?;
    sse::ss(c, Ss::Mul, 0, g.u32(c.ecx)?);
    for (i, slot) in [(0u32, 0x14u32), (4, 0x18)] {
        sse::load(c, 1, g.u32(at(c.eax, i))?);
        let v = sse::ss_reg(c, Ss::Mul, 1, 0);
        g.set_u32(at(c.esp, slot), v)?;
    }
    sse::load(c, 1, g.u32(at(c.eax, 8))?);
    Ok(())
}

/// `10b4d0 (object: stack 0) -> al`; `ret 4`.
pub(super) fn item_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::ITEM_UPDATE.base;
    if pc != entry && !ITEM_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                c.esp = arithmetic(c, c.esp, FRAME, true);
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                push(c, g, c.ebx)?;
                c.ebx = g.u32(at(c.esp, 0xb4))?;
                c.eax = (c.ebx & 0xffff) * 3;
                push(c, g, c.ebp)?;
                c.ebp = g.u32(at(c.edx, c.eax * 4 + 8))?;
                c.eax = g.u32(c.ebp)?;
                c.edx = g.u32(at(c.ebp, 4))?;
                c.eax &= 0xffff;
                c.eax <<= 4;
                push(c, g, c.esi)?;
                c.esi = g.u32(at(at(c.eax, c.ecx), 8))?;
                c.edx = shr(c, c.edx, 8, 32);
                logic(c, c.edx & 1, 0x80);
                g.set_u32(at(c.esp, 0x1c), c.esi)?;
                g.set_u8(at(c.esp, 0xf), 0)?;
                if c.edx & 1 == 0 {
                    // `10b818`: not in the world.
                    c.eax = g.u32(at(c.ebp, 0x14))?;
                    0x10b81b
                } else {
                    let woken = test8(c, g, at(c.ebp, 0xc1), 1)?;
                    push(c, g, c.edi)?;
                    if !woken { 0x10b6ef } else { 0x10b532 }
                }
            }
            0x10b532 => {
                c.eax = g.u32(at(c.esi, 0xbc))?;
                c.eax = shr(c, c.eax, 2, 32);
                logic(c, c.eax & 1, 0x80);
                if c.eax & 1 != 0 {
                    0x10b688
                } else {
                    c.eax = (c.eax & 0xffff_0000) | u32::from(g.u16(at(c.ebp, 0x12c))?);
                    let cl = shr(c, c.eax & 0xff, 5, 8);
                    c.ecx = byte(c.ecx, cl as u8);
                    logic(c, cl & 1, 0x80);
                    if cl & 1 == 0 {
                        0x10b5d3
                    } else {
                        c.edx = (c.edx & 0xffff_0000) | u32::from(g.u16(at(c.ebp, 0x130))?);
                        let word = u32::from(g.u16(SPECIAL_WORD)?);
                        cmp16(c, c.edx, word);
                        if c.edx & 0xffff != word {
                            0x10b5d3
                        } else if !test8(c, g, at(c.ebp, 0x136), 8)? {
                            0x10b688
                        } else {
                            c.eax = u32::from(g.u8(at(c.ebp, 0x137))?);
                            c.ecx = g.u16(at(c.ebp, 0x132))? as i16 as i32 as u32;
                            return call(GROUND_TEST, 0x10b584);
                        }
                    }
                }
            }
            0x10b584 => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax as u8 != 0 {
                    0x10b683
                } else {
                    rmw8(c, g, at(c.ebp, 0x12c), |f| f & 0xdf)?;
                    fall_vector(c, g)?;
                    0x10b66d
                }
            }
            0x10b5d3 => {
                let al = shr(c, c.eax & 0xff, 6, 8);
                c.eax = byte(c.eax, al as u8);
                logic(c, al & 1, 0x80);
                if al & 1 == 0 {
                    0x10b688
                } else {
                    c.edi = g.u32(at(c.ebp, 0x13c))?;
                    push(c, g, NONE)?;
                    c.eax = c.edi;
                    return call(OBJECT_LOOKUP, 0x10b5ed);
                }
            }
            0x10b5ed => {
                logic(c, c.eax, 1 << 31);
                if c.eax == 0 {
                    // `10b62b`.
                    fall_vector(c, g)?;
                    rmw8(c, g, at(c.ebp, 0x12c), |f| f & 0xbf)?;
                    0x10b66d
                } else {
                    c.edx = (c.edx & 0xffff_0000) | u32::from(g.u8(at(c.ebp, 0x13b))?);
                    c.eax = at(c.ebp, 0x140);
                    g.set_u32(at(c.esp, 0x24), c.eax)?;
                    c.eax = c.edi;
                    push(c, g, c.edx)?;
                    return call(NODE_MATRICES, 0x10b60b);
                }
            }
            0x10b60b => {
                push(c, g, c.eax)?;
                c.eax = g.u32(at(c.esp, 0x28))?;
                c.edx = at(c.esp, 0x2c);
                return call(TRANSFORM, 0x10b619);
            }
            0x10b619 => {
                push(c, g, 0)?;
                c.edi = at(c.ebp, 0x158);
                push(c, g, c.ebx)?;
                c.eax = c.edx;
                return call(ATTACHED_MOVE, 0x10b629);
            }
            0x10b629 | 0x10b683 => {
                g.set_u8(at(c.esp, 0x13), 1)?;
                0x10b688
            }
            0x10b66d => {
                push(c, g, 0)?;
                let z = sse::ss_reg(c, Ss::Mul, 1, 0);
                c.eax = at(c.esp, 0x18);
                push(c, g, c.ebx)?;
                g.set_u32(at(c.esp, 0x24), z)?;
                return call(FALL, 0x10b683);
            }
            0x10b688 => {
                let dl = shr(c, u32::from(g.u8(at(c.ebp, 0x12c))?), 4, 8);
                c.edx = byte(c.edx, dl as u8);
                logic(c, dl & 1, 0x80);
                if dl & 1 == 0 {
                    0x10b6e6
                } else {
                    sse::load(c, 1, g.u32(at(c.ebp, 0x94))?);
                    sse::load(c, 0, g.u32(DAMPING)?);
                    for off in [0x94u32, 0x98, 0x9c] {
                        if off != 0x94 {
                            sse::load(c, 1, g.u32(at(c.ebp, off))?);
                        }
                        let v = sse::ss_reg(c, Ss::Mul, 1, 0);
                        if off == 0x9c {
                            c.eax = c.ebx;
                        }
                        g.set_u32(at(c.ebp, off), v)?;
                    }
                    return call(DAMPED, 0x10b6e1);
                }
            }
            0x10b6e1 => {
                g.set_u8(at(c.esp, 0x13), 1)?;
                0x10b6e6
            }
            0x10b6e6 => {
                if test8(c, g, at(c.ebp, 0xc1), 1)? { 0x10b6fb } else { 0x10b6ef }
            }
            0x10b6ef => {
                c.eax = c.ebx;
                return call(points::OBJECT_WAKE_FOR_POST.base, 0x10b6f6);
            }
            0x10b6f6 => {
                g.set_u8(at(c.esp, 0x13), 1)?;
                0x10b6fb
            }
            0x10b6fb => {
                c.eax = 0;
                logic(c, 0, 1 << 31);
                c.eax = u32::from(g.u16(at(c.ebp, 0x12e))?);
                test16(c, c.eax);
                if less_or_equal(c) {
                    0x10b80f
                } else {
                    c.ecx = g.u32(points::GAME_GLOBALS.base)?;
                    c.eax = dec32(c, c.eax);
                    g.set_u16(at(c.ebp, 0x12e), c.eax as u16)?;
                    // `cmp byte [ecx+c], 4`: its flags reach no transfer.
                    let mode = g.u8(at(c.ecx, 0xc))?;
                    if mode == 4 {
                        0x10b80a
                    } else {
                        test16(c, c.eax);
                        if c.eax & 0xffff != 0 {
                            0x10b80a
                        } else {
                            c.ecx = at(c.esp, 0x14);
                            c.eax = c.ebx;
                            return call(BEFORE_DELETE, 0x10b739);
                        }
                    }
                }
            }
            0x10b739 => {
                c.edi = g.u32(at(c.esi, 0x110))?;
                arithmetic(c, c.edi, NONE, true);
                if c.edi == NONE {
                    0x10b7d7
                } else {
                    c.esi = at(c.esp, 0x28);
                    c.eax = c.ebx;
                    g.set_u16(at(c.esp, 0xb0), 0xffff)?;
                    return call(OBJECT_ORIGIN, 0x10b75d);
                }
            }
            0x10b75d => {
                push(c, g, c.edi)?;
                c.edx = at(c.esp, 0x38);
                return call(EFFECT_SETUP, 0x10b767);
            }
            0x10b767 => {
                c.eax = g.u32(at(c.esp, 0x18))?;
                c.ecx = g.u32(at(c.esp, 0x1c))?;
                c.edx = g.u32(at(c.esp, 0x14))?;
                sse::load(c, 0, g.u32(ONE)?);
                g.set_u32(at(c.esp, 0x40), c.eax)?;
                g.set_u32(at(c.esp, 0x44), c.ecx)?;
                c.eax = at(c.esp, 0x50);
                c.ecx = c.ebx;
                g.set_u8(at(c.esp, 0xb8), 4)?;
                g.set_u32(at(c.esp, 0x3c), c.edx)?;
                g.set_u32(at(c.esp, 0x88), sse::lane0(c, 0))?;
                return call(OBJECT_LOCATION, 0x10b7a3);
            }
            0x10b7a3 => {
                c.edx = g.u32(at(c.esp, 0x30))?;
                c.eax = g.u32(at(c.esp, 0x28))?;
                c.ecx = g.u32(at(c.esp, 0x2c))?;
                g.set_u32(at(c.esp, 0x6c), c.edx)?;
                g.set_u32(at(c.esp, 0x60), c.edx)?;
                push(c, g, NONE)?;
                c.edx = at(c.esp, 0x38);
                push(c, g, c.edx)?;
                g.set_u32(at(c.esp, 0x6c), c.eax)?;
                g.set_u32(at(c.esp, 0x70), c.ecx)?;
                g.set_u32(at(c.esp, 0x60), c.eax)?;
                g.set_u32(at(c.esp, 0x64), c.ecx)?;
                return call(EFFECT_NEW, 0x10b7d3);
            }
            0x10b7d3 => {
                c.esi = g.u32(at(c.esp, 0x20))?;
                0x10b7d7
            }
            0x10b7d7 => {
                c.eax = g.u32(at(c.esi, 0x128))?;
                sse::zero(c, 0);
                push(c, g, 0)?;
                push(c, g, 0)?;
                push(c, g, 0)?;
                c.edx = c.ebx;
                push(c, g, c.eax)?;
                c.ebx = at(c.esp, 0x24);
                return call(DELETE_EFFECT, 0x10b7f2);
            }
            0x10b7f2 => {
                c.ecx = g.u32(at(c.esp, 0xc0))?;
                push(c, g, c.ecx)?;
                return call(points::OBJECT_DELETE.base, 0x10b7ff);
            }
            0x10b7ff => {
                c.esi = g.u32(at(c.esp, 0x20))?;
                c.ebx = g.u32(at(c.esp, 0xc0))?;
                0x10b80a
            }
            0x10b80a => {
                g.set_u8(at(c.esp, 0x13), 1)?;
                0x10b80f
            }
            0x10b80f => {
                c.eax = g.u32(at(c.ebp, 0xc8))?;
                c.edi = pop(c, g)?;
                0x10b81b
            }
            0x10b81b => {
                arithmetic(c, c.eax, NONE, true);
                if c.eax == NONE {
                    0x10b836
                } else {
                    c.ecx = g.u32(at(c.esi, 0x104))?;
                    logic(c, c.ecx, 1 << 31);
                    c.eax = at(c.esi, 0x104);
                    if (c.ecx as i32) <= 0 {
                        0x10b836
                    } else {
                        push(c, g, c.eax)?;
                        return call(DEFINITION_CALL, 0x10b836);
                    }
                }
            }
            0x10b836 => {
                push(c, g, c.ebx)?;
                return call(MOVER_FOLLOW, 0x10b83c);
            }
            0x10b83c => {
                c.eax = byte(c.eax, g.u8(at(c.esp, 0xf))?);
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, FRAME, false);
                return Ok(Next::Ret { pop: 4 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}
