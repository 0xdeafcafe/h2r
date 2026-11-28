//! The creature update and post-physics update (slice 11 of
//! docs/engine/sim-rewrite-objects.md; contracts in
//! docs/engine/object-update.md, "Slice 11 bodies"): the `crea`
//! descriptor's slot `+40` (`118700`) and slot `+44` (`118b80`).
//!
//! Both are segment bodies. Their callees stay guest (the AI calls
//! `1e5bb0`, `1e6360`, `1e6120`, `1e55d0`, `1e5af0`, `1e68a0`, `1e5a60`,
//! `1ec500` in retail order), except the wake leaves `baf80` and `bba20`,
//! which [`super::drive`] fuses.
//!
//! **`118700 (object: stack 0) -> al`; `ret 4`.** Frame: `sub esp, 1f0`,
//! then `ebx`, `ebp`, `edi` pushed, and `esi` once the creature has a
//! physics component (body `+b4`); with all four pushed (`S` = that
//! `esp`): `[S+13]` the result byte, `[S+14]` the definition (then the
//! definition `+d4`, then its low byte replaced by `119bc0`'s `al`),
//! `[S+18]` the header offset `(object & ffff) * c` (later a float spill),
//! `[S+100]`, `[S+102]` two words set to none, the object at `[S+204]`.
//! Across the calls `ebx` holds the object until `119bc0`, `ebp` the body,
//! `edi` the animation block (body `+ [body+12a]`), then the out buffer
//! `S+b0`, then body `+17c`; `esi` body `+70` from `1e5bb0` to `1e5af0`,
//! then the object. Without a component it returns 0 at once.
//!
//! In order: `b9dd0`; unless body `+12c` bit 0, `118f90` and the copy of
//! the forward and up vectors (`+70`, `+7c`) to `+154`, `+160` (else the
//! result is 1); `119380`, OR into the result; `1197b0` when unparented;
//! `ba1d0`; the AI update `1e5bb0` (21 arguments); `1e6360` and `1e5af0`
//! or `1e6120` by definition `+d4` bit 4; `1e55d0`; with that bit, the
//! frame's vectors copied back to `+70`, `+7c` and `1e5af0` again; body
//! `+12c` bit 4 from the squared length of the frame vector at `[S+1e8]`
//! against a constant; `119bc0`, its `al` and the frame's velocity into
//! body `+88`; `1c4b00`, `119500`, `1195c0`; the timer at `+204` (counted
//! down, refilled from the tick rate through `fld`/`fistp`, or none);
//! `bba20` unless body `+10a` bit 2 and `+c1` bit 0 (storing the tick at
//! `+bc` first); the word timer at `+206` and `d6bc0` when it runs out;
//! body `+12c` bit 6 cleared. `al` is the result byte.
//!
//! **`118b80 (object: stack 0)`; `ret 4`.** Frame: `sub esp, 6c`, then
//! `ebp`, `esi`, `edi`, and `ebx` once the creature has a component (`S`
//! = that `esp`): `[S+10]` the definition, `[S+14]` the out buffer of
//! `b9dd0`, `[S+80]` the object argument, whose low byte becomes
//! `118b10`'s `al`. `edi` holds the object, then the definition `+d4`;
//! `ebp` the body; `ebx` the component record, then the flags word of
//! body `+c0`, then body `+17c`. Without a component it returns at once.
//!
//! In order: the root ancestor (`baf80`) and, unless its body `+c1` bit 0,
//! `1ec500` (body `+10a |= 20` when it answers); `b9dd0`; `118b10`;
//! `1d1c10`; unless both answered 0 and body `+c0` bit 6 is set, the AI
//! post update `1e68a0` (10 arguments) and `1e5a60`; the two byte counters
//! at `+130`, `+131` (one counts up to `fe`, the other is cleared) and
//! bit 0 of `+12d` and bit 7 of `+12c` from `1e5a60`'s answer; then bit 5
//! of `+12c` from the counters against a time threshold.

use super::frame::{set_slot, slot};
use super::sse::{self, Ss};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, status};
use runtime::{CpuAbi, Guest};
use x87::abi as fp;

const NONE: u32 = u32::MAX;
/// `118700`'s locals.
const FRAME: u32 = 0x1f0;
/// `118b80`'s locals.
const POST_FRAME: u32 = 0x6c;
/// `1.0f`, pushed as an argument.
const ONE: u32 = 0x3f80_0000;

/// Holds a pointer to an `f32` the AI update takes (O: meaning).
const STEP: u32 = 0x0051_e9c4;
/// The float the AI update gets in `xmm0`, and `118b80`'s threshold when
/// definition `+d4` bit 3 is clear (L: seconds).
const THRESHOLD_CLEAR: u32 = 0x0045_dbc0;
/// `118b80`'s threshold when definition `+d4` bit 3 is set.
const THRESHOLD_SET: u32 = 0x0045_dbd4;
/// What both thresholds are multiplied by.
const THRESHOLD_SCALE: u32 = 0x0044_07b8;
/// The squared speed above which body `+12c` bit 4 is set (L: zero).
const MOVING: u32 = 0x0045_dbd8;
/// What the tick rate (game time `+2`) is multiplied by to refill the
/// timer at body `+204`.
const TIMER_SCALE: u32 = 0x0045_dc5c;

/// Every return address of `118700`'s calls.
pub(super) const CREATURE_SITES: [u32; 19] = [
    0x118783, 0x11879e, 0x1187db, 0x1187f1, 0x1187fe, 0x11889f, 0x1188bc, 0x1188cd, 0x1188fa, 0x118912,
    0x11895e, 0x1189c0, 0x118a07, 0x118a17, 0x118a20, 0x118a47, 0x118a60, 0x118ac7, 0x118aed,
];

/// Every return address of `118b80`'s calls.
pub(super) const CREATURE_POST_SITES: [u32; 7] = [0x118be6, 0x118c00, 0x118c16, 0x118c1d, 0x118c2b, 0x118cc7, 0x118cef];

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

/// `dec r/m8`: SF, ZF, PF, AF and OF from the decrement; CF kept, as
/// defined as it was.
fn dec8(c: &mut CpuAbi, old: u8) -> u8 {
    let new = old.wrapping_sub(1);
    let af = u32::from(old & 15 == 0) << 4;
    let of = u32::from(old == 0x80) << 11;
    let keep = ARITH & !1;
    c.eflags = (c.eflags & !keep) | status(new.into(), 0x80) | af | of;
    c.eflags_defined |= keep;
    new
}

/// `shl r32, n` with `n > 1`: SF, ZF and PF from the result, CF the last
/// bit shifted out; OF and AF undefined.
fn shifted(c: &mut CpuAbi, result: u32, carry: bool) {
    c.eflags = (c.eflags & !ARITH) | status(result, 1 << 31) | u32::from(carry);
    c.eflags_defined = (c.eflags_defined | ARITH) & !0x810;
}

/// A byte store with the flags of the `or`/`and` that made it.
fn store_flags(c: &mut CpuAbi, g: &mut Guest, addr: u32, f: impl FnOnce(u8) -> u8) -> Result<()> {
    let v = f(g.u8(addr)?);
    g.set_u8(addr, v)?;
    logic(c, v.into(), 0x80);
    Ok(())
}

/// `fld dword [src]; fistp dword [dst]` at `fistp_pc`, under the guest's
/// x87 control word. Returns the stored integer.
fn fld_fistp(c: &mut CpuAbi, g: &mut Guest, src: u32, dst: u32, fistp_pc: u32) -> Result<u32> {
    let mut x = fp::X87Abi {
        st: c.x87.st,
        control: c.x87.control,
        status: c.x87.status,
        tag: c.x87.tag,
        opcode: c.x87.opcode,
        instruction_pointer: c.x87.instruction_pointer,
        data_pointer: c.x87.data_pointer,
        instruction_selector: c.x87.instruction_selector,
        data_selector: c.x87.data_selector,
    };
    // `supported` masks every x87 exception with none pending, so neither
    // step can ask for delivery.
    let code = fp::ld_f32(&mut x, g.u32(src)?);
    if code & fp::status::MASK != fp::status::OK {
        return Err(unowned(fistp_pc));
    }
    let (code, store) = fp::st_i32(&x, 0);
    if code & fp::status::MASK != fp::status::OK {
        return Err(unowned(fistp_pc));
    }
    g.set_u32(dst, store.value as u32)?;
    fp::store_commit(&mut x, store.status, true);
    c.x87.st = x.st;
    c.x87.status = x.status;
    c.x87.tag = x.tag;
    c.x87.instruction_pointer = fistp_pc;
    c.x87.opcode = g.u16(fistp_pc)?.swap_bytes() & 0x7ff;
    c.x87.data_pointer = dst;
    c.x87.instruction_selector = 8;
    c.x87.data_selector = 16;
    Ok(store.value as u32)
}

/// `118afd`/`118afe`: restore, drop the frame, `ret 4`.
fn update_ret(c: &mut CpuAbi, g: &Guest, esi: bool) -> Result<Next> {
    if esi {
        c.esi = pop(c, g)?;
    }
    c.edi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    Ok(Next::Ret { pop: 4 })
}

/// `118700 (object: stack 0) -> al`; `ret 4`.
pub(super) fn creature_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::CREATURE_UPDATE.base;
    if pc != entry && !CREATURE_SITES.contains(&pc) {
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
                c.ebx = slot(c, g, 0x1f8)?;
                c.eax = (c.ebx & 0xffff) * 12;
                push(c, g, c.ebp)?;
                c.ebp = g.u32(at(at(c.edx, c.eax), 8))?;
                set_slot(c, g, 0x10, c.eax)?;
                c.eax = (g.u32(c.ebp)? & 0xffff) << 4;
                c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
                push(c, g, c.edi)?;
                c.edi = g.u16(at(c.ebp, 0x12a))? as i16 as i32 as u32;
                c.ecx = NONE;
                set_slot(c, g, 0x10, c.edx)?;
                g.set_u16(at(c.esp, 0xfc), 0xffff)?;
                g.set_u16(at(c.esp, 0xfe), 0xffff)?;
                c.edx = g.u32(at(c.ebp, 0xb4))?;
                c.eax = byte(c.eax, 0);
                c.edi = c.edi.wrapping_add(c.ebp);
                arithmetic(c, c.edx, c.ecx, true);
                g.set_u8(at(c.esp, 0xf), 0)?;
                if c.edx == NONE {
                    return update_ret(c, g, false);
                }
                push(c, g, c.esi)?;
                c.esi = at(c.esp, 0x28);
                c.eax = c.ebx;
                return call(0xb9dd0, 0x118783);
            }
            0x118783 => {
                let f = g.u8(at(c.ebp, 0x12c))? & 1;
                logic(c, f.into(), 0x80);
                if f != 0 {
                    g.set_u8(at(c.esp, 0x13), 1)?;
                    0x1187d0
                } else {
                    c.edx = at(c.ebp, 0x13c);
                    return call(0x118f90, 0x11879e);
                }
            }
            0x11879e => {
                c.eax = at(c.ebp, 0x70);
                c.edx = g.u32(c.eax)?;
                c.ecx = at(c.ebp, 0x154);
                g.set_u32(c.ecx, c.edx)?;
                c.edx = g.u32(at(c.eax, 4))?;
                c.eax = g.u32(at(c.eax, 8))?;
                g.set_u32(at(c.ecx, 4), c.edx)?;
                g.set_u32(at(c.ecx, 8), c.eax)?;
                c.ecx = at(c.ebp, 0x7c);
                c.eax = g.u32(c.ecx)?;
                c.edx = at(c.ebp, 0x160);
                g.set_u32(c.edx, c.eax)?;
                c.eax = g.u32(at(c.ecx, 4))?;
                c.ecx = g.u32(at(c.ecx, 8))?;
                g.set_u32(at(c.edx, 4), c.eax)?;
                g.set_u32(at(c.edx, 8), c.ecx)?;
                0x1187d0
            }
            0x1187d0 => {
                c.edx = at(c.esp, 0x1c);
                push(c, g, c.edx)?;
                push(c, g, c.ebx)?;
                return call(0x119380, 0x1187db);
            }
            0x1187db => {
                let cl = g.u8(at(c.esp, 0x13))? | c.eax as u8;
                c.ecx = byte(c.ecx, cl);
                let parent = g.u32(at(c.ebp, 0x14))?;
                arithmetic(c, parent, NONE, true);
                g.set_u8(at(c.esp, 0x13), cl)?;
                if parent != NONE {
                    0x1187f1
                } else {
                    push(c, g, c.ebx)?;
                    return call(0x1197b0, 0x1187f1);
                }
            }
            0x1187f1 => {
                push(c, g, 0)?;
                c.ecx = at(c.esp, 0x38);
                c.eax = c.ebx;
                return call(0xba1d0, 0x1187fe);
            }
            0x1187fe => {
                c.edx = slot(c, g, 0x14)?;
                push(c, g, 0)?;
                c.eax = (c.eax & 0xffff_0000) | u32::from(g.u16(at(c.ebp, 0x12c))?);
                c.edx = arithmetic(c, c.edx, 0xd4, false);
                set_slot(c, g, 0x18, c.edx)?;
                c.ecx = (c.ecx & 0xffff_0000) | u32::from(g.u16(at(c.ebp, 0xc0))?);
                c.edx = at(c.esp, 0x38);
                push(c, g, c.edx)?;
                c.esi = at(c.ebp, 0x70);
                push(c, g, c.esi)?;
                push(c, g, c.esi)?;
                c.edx = at(c.ebp, 0x7c);
                push(c, g, c.edx)?;
                push(c, g, c.esi)?;
                c.edx = at(c.esp, 0x40);
                push(c, g, c.edx)?;
                c.edx = at(c.ebp, 0x148);
                push(c, g, c.edx)?;
                c.edx = g.u32(STEP)?;
                c.edx = g.u32(c.edx)?;
                push(c, g, ONE)?;
                push(c, g, c.edx)?;
                // `xor edx, edx; mov dl, ah; shr al, 5`.
                c.edx = (c.eax >> 8) & 0xff;
                c.eax = byte(c.eax, (c.eax as u8) >> 5);
                push(c, g, 0)?;
                sse::load(c, 0, g.u32(THRESHOLD_CLEAR)?);
                c.ecx = byte(c.ecx, (c.ecx as u8) >> 6);
                c.edx &= 0xffff_ff01;
                push(c, g, c.edx)?;
                c.eax &= 0xffff_ff01;
                push(c, g, c.eax)?;
                c.edx = g.u32(at(c.ebp, 0xb4))?;
                // `xor eax, eax; mov al, ch`.
                c.eax = (c.ecx >> 8) & 0xff;
                c.ecx &= 0xffff_ff01;
                c.eax &= 0xffff_ff01;
                logic(c, c.eax, 1 << 31);
                push(c, g, c.eax)?;
                push(c, g, 0)?;
                push(c, g, 1)?;
                push(c, g, 0)?;
                push(c, g, c.ecx)?;
                c.ecx = slot(c, g, 0x5c)?;
                push(c, g, 0x0f)?;
                push(c, g, c.ecx)?;
                c.eax = c.edi;
                push(c, g, c.ebx)?;
                c.ecx = at(c.ebp, 0x17c);
                c.edi = at(c.esp, 0x104);
                return call(0x1e5bb0, 0x11889f);
            }
            0x11889f => {
                c.edx = slot(c, g, 0x14)?;
                c.eax = g.u32(c.edx)? >> 4;
                logic(c, c.eax & 1, 0x80);
                push(c, g, 0)?;
                if c.eax & 1 != 0 {
                    sse::load(c, 0, g.u32(at(c.ebp, 0x178))?);
                    push(c, g, c.ebx)?;
                    return call(0x1e6360, 0x1188bc);
                }
                sse::zero(c, 0);
                c.ecx = u32::from(g.u8(at(c.ebp, 0x12c))? >> 6);
                c.edx = at(c.ebp, 0x17c);
                c.eax = at(c.esp, 0xb4);
                c.ecx &= 0xffff_ff01;
                logic(c, c.ecx, 1 << 31);
                push(c, g, c.ecx)?;
                push(c, g, 0)?;
                push(c, g, 0)?;
                return call(0x1e6120, 0x1188fa);
            }
            0x1188bc => {
                push(c, g, c.esi)?;
                c.eax = c.edi;
                c.ecx = at(c.ebp, 0x7c);
                c.edx = at(c.ebp, 0x17c);
                return call(0x1e5af0, 0x1188cd);
            }
            0x1188cd => 0x1188fa,
            0x1188fa => {
                c.edx = at(c.esp, 0xb0);
                push(c, g, c.edx)?;
                c.edi = at(c.ebp, 0x17c);
                push(c, g, c.edi)?;
                c.eax = at(c.esp, 0x48);
                return call(0x1e55d0, 0x118912);
            }
            0x118912 => {
                c.eax = slot(c, g, 0x14)?;
                c.ecx = g.u32(c.eax)? >> 4;
                logic(c, c.ecx & 1, 0x80);
                if c.ecx & 1 == 0 {
                    0x11895e
                } else {
                    c.eax = slot(c, g, 0x58)?;
                    c.ecx = slot(c, g, 0x5c)?;
                    c.edx = c.esi;
                    g.set_u32(c.edx, c.eax)?;
                    c.eax = slot(c, g, 0x60)?;
                    g.set_u32(at(c.edx, 4), c.ecx)?;
                    g.set_u32(at(c.edx, 8), c.eax)?;
                    c.eax = slot(c, g, 0x64)?;
                    c.ecx = at(c.ebp, 0x7c);
                    c.edx = c.ecx;
                    g.set_u32(c.edx, c.eax)?;
                    c.eax = slot(c, g, 0x68)?;
                    g.set_u32(at(c.edx, 4), c.eax)?;
                    c.eax = slot(c, g, 0x6c)?;
                    g.set_u32(at(c.edx, 8), c.eax)?;
                    push(c, g, c.esi)?;
                    c.eax = at(c.esp, 0xb4);
                    c.edx = c.edi;
                    return call(0x1e5af0, 0x11895e);
                }
            }
            0x11895e => {
                sse::load(c, 0, slot(c, g, 0x1f0)?);
                sse::movaps(c, 1, 0);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                sse::load(c, 0, slot(c, g, 0x1ec)?);
                sse::movaps(c, 2, 0);
                sse::ss_reg(c, Ss::Mul, 2, 0);
                sse::load(c, 0, slot(c, g, 0x1e8)?);
                sse::ss_reg(c, Ss::Add, 1, 2);
                sse::movaps(c, 2, 0);
                sse::ss_reg(c, Ss::Mul, 2, 0);
                sse::ss_reg(c, Ss::Add, 1, 2);
                let speed = sse::lane0(c, 1);
                sse::comiss(c, speed, g.u32(MOVING)?);
                let moving = !sse::below_or_equal(c);
                c.eax = byte(c.eax, u8::from(moving));
                logic(c, u32::from(moving), 0x80);
                store_flags(c, g, at(c.ebp, 0x12c), |f| if moving { f | 0x10 } else { f & 0xef })?;
                c.eax = c.ebx;
                return call(0x119bc0, 0x1189c0);
            }
            0x1189c0 => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                c.ecx = slot(c, g, 0x4c)?;
                c.esi = slot(c, g, 0x204)?;
                g.set_u8(at(c.esp, 0x14), c.eax as u8)?;
                c.eax = slot(c, g, 0x18)?;
                c.eax = g.u32(at(at(c.edx, c.eax), 8))?;
                c.edx = slot(c, g, 0x50)?;
                c.eax = arithmetic(c, c.eax, 0x88, false);
                g.set_u32(c.eax, c.ecx)?;
                c.ecx = slot(c, g, 0x54)?;
                g.set_u32(at(c.eax, 4), c.edx)?;
                c.edx = slot(c, g, 0x14)?;
                push(c, g, c.edx)?;
                g.set_u32(at(c.eax, 8), c.ecx)?;
                push(c, g, 0)?;
                c.ebx = at(c.esp, 0x54);
                c.eax = c.esi;
                return call(0x1c4b00, 0x118a07);
            }
            0x118a07 => {
                c.eax = slot(c, g, 0x44)?;
                push(c, g, c.eax)?;
                c.edx = at(c.esp, 0x20);
                c.eax = c.esi;
                return call(0x119500, 0x118a17);
            }
            0x118a17 => {
                c.ebx = c.edx;
                c.edx = c.esi;
                return call(0x1195c0, 0x118a20);
            }
            0x118a20 => {
                let cl = g.u8(at(c.ebp, 0x10a))? >> 2;
                c.ecx = byte(c.ecx, cl);
                logic(c, u32::from(cl & 1), 0x80);
                if cl & 1 != 0 {
                    0x118a49
                } else {
                    let f = g.u8(at(c.ebp, 0x144))? & 8;
                    logic(c, f.into(), 0x80);
                    if f == 0 {
                        0x118a49
                    } else {
                        c.eax = g.u32(at(c.ebp, 0x14))?;
                        c.ebx = NONE;
                        arithmetic(c, c.eax, c.ebx, true);
                        if c.eax != c.ebx {
                            0x118a51
                        } else {
                            push(c, g, c.esi)?;
                            return call(0x119c10, 0x118a47);
                        }
                    }
                }
            }
            0x118a47 => 0x118a4c,
            0x118a49 => {
                c.ebx = NONE;
                logic(c, NONE, 1 << 31);
                0x118a4c
            }
            0x118a4c => {
                let parent = g.u32(at(c.ebp, 0x14))?;
                arithmetic(c, parent, c.ebx, true);
                if parent == c.ebx { 0x118a95 } else { 0x118a51 }
            }
            0x118a51 => {
                let a = at(c.ebp, 0x204);
                let old = g.u8(a)?;
                let new = dec8(c, old);
                g.set_u8(a, new)?;
                if new != 0 {
                    0x118a8e
                } else {
                    c.ebx = c.esi;
                    return call(0x11a140, 0x118a60);
                }
            }
            0x118a60 => {
                c.edx = g.u32(points::GAME_TIME.base)?;
                c.eax = g.u16(at(c.edx, 2))? as i16 as i32 as u32;
                sse::cvtsi2ss(c, 0, c.eax);
                sse::ss(c, Ss::Mul, 0, g.u32(TIMER_SCALE)?);
                set_slot(c, g, 0x18, sse::lane0(c, 0))?;
                let (src, dst) = (at(c.esp, 0x18), at(c.esp, 0x14));
                fld_fistp(c, g, src, dst, 0x118a80)?;
                let cl = g.u8(dst)?;
                c.ecx = byte(c.ecx, cl);
                g.set_u8(at(c.ebp, 0x204), cl)?;
                0x118a8e
            }
            0x118a8e => {
                g.set_u8(at(c.esp, 0x13), 1)?;
                0x118a9b
            }
            0x118a95 => {
                g.set_u8(at(c.ebp, 0x204), c.ebx as u8)?;
                0x118a9b
            }
            0x118a9b => {
                let dl = g.u8(at(c.ebp, 0x10a))? >> 2;
                c.edx = byte(c.edx, dl);
                logic(c, u32::from(dl & 1), 0x80);
                if dl & 1 == 0 {
                    0x118ab2
                } else {
                    let f = g.u8(at(c.ebp, 0xc1))? & 1;
                    logic(c, f.into(), 0x80);
                    if f != 0 { 0x118acc } else { 0x118ab2 }
                }
            }
            0x118ab2 => {
                c.eax = g.u32(points::GAME_TIME.base)?;
                c.ecx = g.u32(at(c.eax, 8))?;
                c.eax = c.esi;
                g.set_u32(at(c.ebp, 0xbc), c.ecx)?;
                return call(points::OBJECT_WAKE_FOR_POST.base, 0x118ac7);
            }
            0x118ac7 => {
                g.set_u8(at(c.esp, 0x13), 1)?;
                0x118acc
            }
            0x118acc => {
                c.eax = u32::from(g.u16(at(c.ebp, 0x206))?);
                logic(c, c.eax, 0x8000);
                if c.eax == 0 {
                    0x118af2
                } else {
                    // `dec eax`; its flags are replaced by the `test`.
                    c.eax = c.eax.wrapping_sub(1);
                    let ax = c.eax as u16;
                    logic(c, ax.into(), 0x8000);
                    g.set_u16(at(c.ebp, 0x206), ax)?;
                    if ax as i16 > 0 {
                        0x118aed
                    } else {
                        push(c, g, c.esi)?;
                        return call(0xd6bc0, 0x118aed);
                    }
                }
            }
            0x118aed => {
                g.set_u8(at(c.esp, 0x13), 1)?;
                0x118af2
            }
            0x118af2 => {
                store_flags(c, g, at(c.ebp, 0x12c), |f| f & 0xbf)?;
                c.eax = byte(c.eax, g.u8(at(c.esp, 0x13))?);
                return update_ret(c, g, true);
            }
            _ => return Err(unowned(pc)),
        };
    }
}

/// `118e01`/`118e02`: restore, drop the frame, `ret 4`.
fn post_ret(c: &mut CpuAbi, g: &Guest, ebx: bool) -> Result<Next> {
    if ebx {
        c.ebx = pop(c, g)?;
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, POST_FRAME, false);
    Ok(Next::Ret { pop: 4 })
}

/// `118d8d`/`118dd8`: `counter * [time+4] > threshold * scale`, with the
/// threshold in `xmm0`; leaves the compare in EFLAGS.
fn past(c: &mut CpuAbi, g: &Guest, counter: u32) -> Result<bool> {
    c.edx = u32::from(g.u8(at(c.ebp, counter))?);
    c.eax = g.u32(points::GAME_TIME.base)?;
    sse::ss(c, Ss::Mul, 0, g.u32(THRESHOLD_SCALE)?);
    sse::cvtsi2ss(c, 1, c.edx);
    sse::ss(c, Ss::Mul, 1, g.u32(at(c.eax, 4))?);
    let (a, b) = (sse::lane0(c, 1), sse::lane0(c, 0));
    sse::comiss(c, a, b);
    Ok(!sse::below_or_equal(c))
}

/// `118b80 (object: stack 0)`; `ret 4`.
pub(super) fn creature_post_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::CREATURE_POST_UPDATE.base;
    if pc != entry && !CREATURE_POST_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                c.eax = g.u32(points::OBJECTS.base)?;
                c.esp = arithmetic(c, c.esp, POST_FRAME, true);
                push(c, g, c.ebp)?;
                push(c, g, c.esi)?;
                c.esi = g.u32(at(c.eax, DATA))?;
                push(c, g, c.edi)?;
                c.edi = slot(c, g, 0x7c)?;
                c.eax = c.edi & 0xffff;
                c.ecx = c.eax * 3;
                c.ebp = g.u32(at(c.esi, c.ecx * 4 + 8))?;
                c.eax = g.u32(at(c.ebp, 0xb4))?;
                arithmetic(c, c.eax, NONE, true);
                if c.eax == NONE {
                    return post_ret(c, g, false);
                }
                c.edx = g.u32(points::PHYSICS_COMPONENTS.base)?;
                c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                c.eax &= 0xffff;
                push(c, g, c.ebx)?;
                c.ebx = (c.eax * 5) << 5;
                c.eax = g.u32(at(c.edx, DATA))?;
                c.ebx = arithmetic(c, c.ebx, c.eax, false);
                let tag = g.u32(c.ebp)? & 0xffff;
                c.eax = tag << 4;
                shifted(c, c.eax, tag & 1 << 28 != 0);
                c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
                c.ecx = c.edi;
                set_slot(c, g, 0x10, c.edx)?;
                return call(points::OBJECT_ROOT.base, 0x118be6);
            }
            0x118be6 => {
                c.eax = (c.eax & 0xffff) * 3;
                c.ecx = g.u32(at(c.esi, c.eax * 4 + 8))?;
                let f = g.u8(at(c.ecx, 0xc1))? & 1;
                logic(c, f.into(), 0x80);
                if f != 0 {
                    0x118c0b
                } else {
                    return call(0x1ec500, 0x118c00);
                }
            }
            0x118c00 => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax & 0xff != 0 {
                    store_flags(c, g, at(c.ebp, 0x10a), |f| f | 0x20)?;
                }
                0x118c0b
            }
            0x118c0b => {
                c.esi = at(c.esp, 0x14);
                c.eax = c.edi;
                return call(0xb9dd0, 0x118c16);
            }
            0x118c16 => {
                c.eax = c.edi;
                return call(0x118b10, 0x118c1d);
            }
            0x118c1d => {
                c.esi = c.ebx;
                g.set_u8(at(c.esp, 0x80), c.eax as u8)?;
                return call(0x1d1c10, 0x118c2b);
            }
            0x118c2b => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax & 0xff != 0 {
                    0x118c4c
                } else {
                    let al = g.u8(at(c.esp, 0x80))?;
                    c.eax = byte(c.eax, al);
                    logic(c, al.into(), 0x80);
                    if al != 0 {
                        0x118c4c
                    } else {
                        let dl = g.u8(at(c.ebp, 0xc0))? >> 6;
                        c.edx = byte(c.edx, dl);
                        logic(c, u32::from(dl & 1), 0x80);
                        if dl & 1 != 0 {
                            return post_ret(c, g, true);
                        }
                        0x118c4c
                    }
                }
            }
            0x118c4c => {
                c.ebx = (c.ebx & 0xffff_0000) | u32::from(g.u16(at(c.ebp, 0xc0))?);
                let al = (c.ebx as u8) >> 2;
                c.eax = byte(c.eax, al);
                logic(c, u32::from(al & 1), 0x80);
                if al & 1 != 0 {
                    0x118c6a
                } else {
                    let cl = (c.ebx as u8) >> 1;
                    c.ecx = byte(c.ecx, cl);
                    logic(c, u32::from(cl & 1), 0x80);
                    c.eax = NONE;
                    if cl & 1 == 0 { 0x118c70 } else { 0x118c6a }
                }
            }
            0x118c6a => {
                c.eax = g.u32(at(c.ebp, 0xb8))?;
                0x118c70
            }
            0x118c70 => {
                c.edi = slot(c, g, 0x10)?;
                c.edx = at(c.ebp, 0x148);
                push(c, g, c.edx)?;
                c.ecx = at(c.ebp, 0x70);
                push(c, g, c.ecx)?;
                c.ecx = slot(c, g, 0x88)?;
                c.edx = at(c.esp, 0x1c);
                push(c, g, c.edx)?;
                // `xor edx, edx; mov dl, [ebp+12c]`.
                c.edx = u32::from(g.u8(at(c.ebp, 0x12c))?);
                push(c, g, c.ecx)?;
                c.edx = byte(c.edx, (c.edx as u8) >> 5);
                push(c, g, 0)?;
                push(c, g, 0)?;
                c.edi = arithmetic(c, c.edi, 0xd4, false);
                c.esi = at(c.ebp, 0x17c);
                // `mov bl, bh; and bl, 1`.
                c.ebx = byte(c.ebx, ((c.ebx >> 8) as u8) & 1);
                c.edx &= 0xffff_ff01;
                logic(c, c.edx, 1 << 31);
                push(c, g, c.edx)?;
                push(c, g, c.eax)?;
                c.eax = g.u32(at(c.ebp, 0xb4))?;
                push(c, g, c.eax)?;
                push(c, g, c.edi)?;
                c.ecx = c.esi;
                c.eax = at(c.esp, 0x4c);
                return call(0x1e68a0, 0x118cc7);
            }
            0x118cc7 => {
                c.ecx = g.u32(c.edi)? >> 4;
                c.eax = byte(c.eax, 1);
                let t = (c.ecx as u8) & 1;
                logic(c, t.into(), 0x80);
                g.set_u8(at(c.esp, 0x38), 1)?;
                if t == 0 {
                    c.edx = g.u32(at(c.ebp, 0x1a8))?;
                    set_slot(c, g, 0x68, c.edx)?;
                }
                c.eax = at(c.esp, 0x24);
                c.ecx = at(c.esp, 0x14);
                c.ebx = c.esi;
                return call(0x1e5a60, 0x118cef);
            }
            0x118cef => {
                let cl = g.u8(at(c.esp, 0x18))?;
                c.ecx = byte(c.ecx, cl);
                logic(c, u32::from(cl & 1), 0x80);
                // One counter counts the ticks in this state, up to `fe`;
                // the other is cleared.
                let (up, clear) = if cl & 1 != 0 { (0x130, 0x131) } else { (0x131, 0x130) };
                c.eax = u32::from(g.u8(at(c.ebp, up))?) + 1;
                arithmetic(c, c.eax, 0xfe, true);
                if cl & 1 == 0 {
                    g.set_u8(at(c.ebp, clear), 0)?;
                }
                if c.eax as i32 > 0xfe {
                    c.eax = 0xfe;
                }
                g.set_u8(at(c.ebp, up), c.eax as u8)?;
                if cl & 1 != 0 {
                    g.set_u8(at(c.ebp, clear), 0)?;
                }
                // `118d3c`.
                let al = cl & 1;
                c.eax = byte(c.eax, al);
                logic(c, al.into(), 0x80);
                store_flags(c, g, at(c.ebp, 0x12d), |f| if al != 0 { f | 1 } else { f & 0xfe })?;
                logic(c, al.into(), 0x80);
                store_flags(c, g, at(c.ebp, 0x12c), |f| if al != 0 { f | 0x80 } else { f & 0x7f })?;
                // `118d66`.
                let al = g.u8(at(c.ebp, 0x12c))? >> 5;
                c.ecx = g.u32(c.edi)?;
                sse::load(c, 0, g.u32(THRESHOLD_SET)?);
                c.eax = byte(c.eax, al);
                c.ecx >>= 3;
                logic(c, u32::from(al & 1), 0x80);
                let set = c.ecx & 1;
                if al & 1 != 0 {
                    logic(c, set, 0x80);
                    if set == 0 {
                        sse::load(c, 0, g.u32(THRESHOLD_CLEAR)?);
                    }
                    if past(c, g, 0x131)? {
                        store_flags(c, g, at(c.ebp, 0x12c), |f| f & 0xdf)?;
                    }
                    return post_ret(c, g, true);
                }
                logic(c, set, 0x80);
                if set == 0 {
                    sse::load(c, 0, g.u32(THRESHOLD_CLEAR)?);
                }
                let al = g.u8(at(c.esp, 0x80))?;
                c.eax = byte(c.eax, al);
                logic(c, al.into(), 0x80);
                if al != 0 || past(c, g, 0x130)? {
                    store_flags(c, g, at(c.ebp, 0x12c), |f| f | 0x20)?;
                }
                return post_ret(c, g, true);
            }
            _ => return Err(unowned(pc)),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    type Run = fn(&mut Cx<'_>, u32) -> Result<Next>;

    /// Without a physics component both return at once, with the frame
    /// dropped and the saved registers back.
    #[test]
    fn without_a_component_both_return_at_once() {
        const ARRAY: u32 = 0x60_0000;
        const HEADERS: u32 = 0x60_1000;
        const BODY: u32 = 0x61_0000;
        let mut g = Guest::new();
        g.map_zeroed(0, 0x80_0000, Permissions::RW).unwrap();
        g.set_u32(points::OBJECTS.base, ARRAY).unwrap();
        g.set_u32(ARRAY + DATA, HEADERS).unwrap();
        g.set_u32(HEADERS + 12 * 3 + 8, BODY).unwrap();
        g.set_u32(BODY + 0xb4, NONE).unwrap();
        let runs: [(u32, Run); 2] = [
            (points::CREATURE_UPDATE.base, creature_update),
            (points::CREATURE_POST_UPDATE.base, creature_post_update),
        ];
        for (entry, run) in runs {
            let mut c = CpuAbi { esp: 0x7f_0000, eax: 0x1234_5678, ebx: 11, ebp: 22, esi: 33, edi: 44, ..Default::default() };
            g.set_u32(c.esp + 4, 0x8200_0003).unwrap();
            let next = run(&mut Cx { cpu: &mut c, g: &mut g }, entry).unwrap();
            assert_eq!(next, Next::Ret { pop: 4 });
            assert_eq!(c.esp, 0x7f_0000);
            assert_eq!((c.ebx, c.ebp, c.esi, c.edi), (11, 22, 33, 44));
            if entry == points::CREATURE_UPDATE.base {
                assert_eq!(c.eax & 0xff, 0, "the update answers 0");
            } else {
                assert_eq!(c.eax, NONE);
            }
        }
    }

    #[test]
    fn an_unlisted_pc_is_not_owned() {
        let mut g = Guest::new();
        let mut c = CpuAbi::default();
        assert!(creature_update(&mut Cx { cpu: &mut c, g: &mut g }, 0x118704).is_err());
        assert!(creature_post_update(&mut Cx { cpu: &mut c, g: &mut g }, 0x118b84).is_err());
    }
}
