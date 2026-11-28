//! The biped post-update's step segments: the crouch step `dd7d0`, the
//! crouch height `dd8e0`, the jump update `e4680` and the contact update
//! `e2fa0`. Each is the retail code block by block, keyed by retail
//! address; the guest calls they make stay callouts, except the landing
//! test `e4050` (slice 7a), the forced-crouch test `e0dc0` and the crouch
//! step (bodies, fused).

use super::{NONE, above, and32, at, below_or_equal, call, cmp8, cmp32, flags8, inc8, neg8, store_ss, sx8, sx16};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_objects::alu;
use crate::sim_objects::biped::{fld_fistp, less, ucomiss_jp};
use crate::sim_objects::frame::slot;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, byte, logic, pop, push};

/// `.rdata` constants: f32 at `45dbc0` (the crouch fraction's upper
/// clamp), `45dbd8` (zero, the crouch height's test), `44ae90` and
/// `45dbbc` (times in seconds), `45dd74` (a grounded time) and `45dc68`
/// (the contact's time).
const K_45DBC0: u32 = 0x0045_dbc0;
const K_45DBD8: u32 = 0x0045_dbd8;
const K_44AE90: u32 = 0x0044_ae90;
const K_45DD74: u32 = 0x0045_dd74;
const K_45DBBC: u32 = 0x0045_dbbc;
const K_45DC68: u32 = 0x0045_dc68;

/// Callees that stay guest.
const X25AAD0: u32 = 0x0025_aad0;
const E4770: u32 = 0x000e_4770;
const BADC0: u32 = 0x000b_adc0;
const X1C9C00: u32 = 0x001c_9c00;
const X1FB6F0: u32 = 0x001f_b6f0;

/// The return address of `dd7d0`'s call.
pub(in crate::sim_objects) const CROUCH_STEP_SITES: [u32; 1] = [0xdd83f];
/// Every return address of `dd8e0`'s calls.
pub(in crate::sim_objects) const CROUCH_HEIGHT_SITES: [u32; 3] = [0xdd915, 0xdd92d, 0xdd938];
/// Every return address of `e4680`'s calls.
pub(in crate::sim_objects) const JUMP_SITES: [u32; 2] = [0xe471c, 0xe4730];
/// Every return address of `e2fa0`'s calls.
pub(in crate::sim_objects) const CONTACT_SITES: [u32; 3] = [0xe3024, 0xe3033, 0xe3042];

/// `dd7d0 (object: eax) -> xmm0`; `ret`. `push ecx` makes a one-dword
/// local (`[esp+4]` after `push esi`): the definition's rate `+250`, which
/// an actor (body `+12c`, its record `+858` set) may scale through
/// `25aad0` (the local's address on the stack). The rate times the tick
/// length is the step: toward 1 (at most `[45dbc0]` less body `+2c4`) when
/// body `+134` bit 23 is set, else toward 0 (at least `-[+2c4]`); in state
/// 6 with `+3f8` set and `+348` bit 13, at least `[45dbc0] - [+2c4]`.
/// `esi` is pushed and popped back; `ecx`'s push is the local, so `pop
/// ecx` returns the rate (as scaled by `25aad0`), not the caller's `ecx`.
pub(in crate::sim_objects) fn crouch_step(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_CROUCH_STEP.base;
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                push(c, g, c.ecx)?;
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
                push(c, g, c.esi)?;
                c.esi = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
                c.eax = g.u32(c.esi)?;
                c.eax = and32(c, c.eax & 0xffff);
                c.eax = alu::shl(c, c.eax, 4, 32);
                c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
                c.eax = g.u32(at(c.esi, 0x12c))?;
                cmp32(c, c.eax, NONE);
                sse::load(c, 0, g.u32(at(c.edx, 0x250))?);
                store_ss(c, g, at(c.esp, 4), 0)?;
                if c.eax == NONE {
                    0xdd83f
                } else {
                    c.edx = g.u32(points::ACTORS.base)?;
                    c.edx = g.u32(at(c.edx, DATA))?;
                    c.ecx = c.eax;
                    c.ecx = and32(c, c.ecx & 0xffff);
                    // `imul ecx, 888`: its flags are overwritten by the
                    // `cmp`.
                    c.ecx = c.ecx.wrapping_mul(0x888);
                    let w = g.u32(at(c.ecx.wrapping_add(c.edx), 0x858))?;
                    cmp32(c, w, NONE);
                    if w == NONE {
                        0xdd83f
                    } else {
                        c.ecx = at(c.esp, 4);
                        push(c, g, c.ecx)?;
                        return call(X25AAD0, 0xdd83f);
                    }
                }
            }
            0xdd83f => {
                c.eax = g.u32(at(c.esi, 0x134))?;
                c.edx = g.u32(points::GAME_TIME.base)?;
                sse::load(c, 0, g.u32(at(c.edx, 4))?);
                sse::ss(c, Ss::Mul, 0, slot(c, g, 4)?);
                sse::load(c, 2, g.u32(K_45DBC0)?);
                c.eax = alu::shr(c, c.eax, 0x17, 32);
                let fraction = g.u32(at(c.esi, 0x2c4))?;
                if flags8(c, c.eax as u8 & 1) != 0 {
                    sse::movaps(c, 1, 2);
                    sse::ss(c, Ss::Sub, 1, fraction);
                    let (x0, x1) = (sse::lane0(c, 0), sse::lane0(c, 1));
                    if above(c, x0, x1) {
                        sse::movaps(c, 0, 1);
                    }
                } else {
                    // `dd87a`.
                    let x0 = sse::lane0(c, 0);
                    if above(c, x0, fraction) {
                        sse::load(c, 1, fraction);
                    } else {
                        sse::movaps(c, 1, 0);
                    }
                    sse::zero(c, 0);
                    let x1 = sse::lane0(c, 1);
                    sse::ss(c, Ss::Sub, 0, x1);
                }
                0xdd897
            }
            0xdd897 => {
                let state = g.u8(at(c.esi, 0x3dc))?;
                cmp8(c, state, 6);
                if state == 6 {
                    let al = g.u8(at(c.esi, 0x3f8))?;
                    c.eax = byte(c.eax, al);
                    if flags8(c, al) != 0 {
                        c.ecx = and32(c, 0);
                        c.ecx = u32::from(g.u16(at(c.esi, 0x348))?);
                        c.ecx = alu::shr(c, c.ecx, 0xd, 32);
                        if flags8(c, c.ecx as u8 & 1) != 0 {
                            sse::movaps(c, 0, 2);
                            sse::ss(c, Ss::Sub, 0, g.u32(at(c.esi, 0x2c4))?);
                            let (x0, x2) = (sse::lane0(c, 0), sse::lane0(c, 2));
                            if !above(c, x2, x0) {
                                sse::movaps(c, 0, 2);
                            }
                        }
                    }
                }
                c.esi = pop(c, g)?;
                c.ecx = pop(c, g)?;
                return Ok(Next::Ret { pop: 0 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}

/// `dd8e0 (object: edi, out: stack 0) -> al`; `ret 4`. `ebx`, `esi`
/// pushed; `esi` the definition. The crouch step (`dd7d0`, fused) into
/// `xmm1`; unless definition `+264` bit 1, when the biped is landing
/// (`e4050`) or forced to crouch (`e0dc0`) and the step is not zero:
/// `out` = (definition `+268` − `+26c`) × the tick rate × the step, and
/// `al` 1. Otherwise `al` 0.
pub(in crate::sim_objects) fn crouch_height(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_CROUCH_HEIGHT.base;
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                c.eax = c.edi;
                c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
                c.eax = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
                c.ecx = g.u32(c.eax)?;
                c.edx = g.u32(points::TAG_ENTRIES.base)?;
                c.ecx = and32(c, c.ecx & 0xffff);
                push(c, g, c.ebx)?;
                c.ecx = alu::shl(c, c.ecx, 4, 32);
                push(c, g, c.esi)?;
                c.esi = g.u32(at(at(c.ecx, c.edx), 8))?;
                c.eax = c.edi;
                return call(points::BIPED_CROUCH_STEP.base, 0xdd915);
            }
            0xdd915 => {
                c.eax = g.u32(at(c.esi, 0x264))?;
                c.eax = alu::shr(c, c.eax, 1, 32);
                c.ebx = byte(c.ebx, 0);
                logic(c, 0, 0x80);
                let skip = flags8(c, c.eax as u8 & 1) != 0;
                sse::movaps(c, 1, 0);
                if skip {
                    0xdd97e
                } else {
                    c.eax = c.edi;
                    return call(points::BIPED_AIRBORNE_TEST.base, 0xdd92d);
                }
            }
            0xdd92d => {
                if flags8(c, c.eax as u8) != 0 {
                    0xdd93c
                } else {
                    c.eax = c.edi;
                    return call(points::BIPED_CROUCH_FORCED.base, 0xdd938);
                }
            }
            0xdd938 => {
                if flags8(c, c.eax as u8) == 0 {
                    0xdd97e
                } else {
                    0xdd93c
                }
            }
            0xdd93c => {
                let x1 = sse::lane0(c, 1);
                if !ucomiss_jp(c, x1, g.u32(K_45DBD8)?) {
                    0xdd97e
                } else {
                    c.ecx = g.u32(points::GAME_TIME.base)?;
                    c.edx = sx16(g.u16(at(c.ecx, 2))?);
                    sse::load(c, 0, g.u32(at(c.esi, 0x268))?);
                    sse::ss(c, Ss::Sub, 0, g.u32(at(c.esi, 0x26c))?);
                    c.eax = slot(c, g, 0xc)?;
                    sse::cvtsi2ss(c, 2, c.edx);
                    sse::ss_reg(c, Ss::Mul, 0, 2);
                    sse::ss_reg(c, Ss::Mul, 0, 1);
                    c.esi = pop(c, g)?;
                    store_ss(c, g, c.eax, 0)?;
                    c.eax = byte(c.eax, 1);
                    c.ebx = pop(c, g)?;
                    return Ok(Next::Ret { pop: 4 });
                }
            }
            0xdd97e => {
                c.esi = pop(c, g)?;
                c.eax = byte(c.eax, c.ebx as u8);
                c.ebx = pop(c, g)?;
                return Ok(Next::Ret { pop: 4 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}

/// `e4680 (object: edi) -> al`; `ret`. Frame `sub esp, 8` (`[esp+8]`,
/// `[esp+c]` after `ebx`, `esi` are pushed: the `fistp` out and its
/// float). The counter at body `+39c` counts up to `7f`. With body `+148`
/// bit 1: the ticks of `[44ae90]` seconds; with no target (`+13c`) or the
/// counter `+39e` at least that, the counter `+39c` past `[45dd74]`
/// seconds, not landing (`e4050`) and `+34c` not 1, `e4770`; when it
/// answers, `+39e` = the ticks and `al` 1. `+39e` counts up to `7f`
/// (without bit 1 it is cleared instead).
pub(in crate::sim_objects) fn jump_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_JUMP_UPDATE.base;
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                c.esp = arithmetic(c, c.esp, 8, true);
                c.eax = c.edi;
                c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
                push(c, g, c.ebx)?;
                push(c, g, c.esi)?;
                c.esi = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
                c.eax = g.u32(at(c.esi, 0x148))?;
                let cl = g.u8(at(c.esi, 0x39c))?;
                c.ecx = byte(c.ecx, cl);
                c.eax = alu::shr(c, c.eax, 1, 32);
                let al = flags8(c, c.eax as u8 & 1);
                c.eax = byte(c.eax, al);
                c.ebx = byte(c.ebx, 0);
                logic(c, 0, 0x80);
                cmp8(c, cl, 0x7f);
                if less(c) {
                    let cl = inc8(c, cl);
                    c.ecx = byte(c.ecx, cl);
                    g.set_u8(at(c.esi, 0x39c), cl)?;
                }
                cmp8(c, al, c.ebx as u8);
                if al == c.ebx as u8 {
                    g.set_u8(at(c.esi, 0x39e), c.ebx as u8)?;
                    0xe4760
                } else {
                    c.eax = g.u32(points::GAME_TIME.base)?;
                    c.ecx = sx16(g.u16(at(c.eax, 2))?);
                    sse::cvtsi2ss(c, 0, c.ecx);
                    sse::ss(c, Ss::Mul, 0, g.u32(K_44AE90)?);
                    store_ss(c, g, at(c.esp, 0xc), 0)?;
                    fld_fistp(c, g, at(c.esp, 0xc), at(c.esp, 8), 0xe46e2)?;
                    let target = g.u32(at(c.esi, 0x13c))?;
                    cmp32(c, target, NONE);
                    let mut next = 0xe46fc;
                    if target != NONE {
                        c.edx = sx8(g.u8(at(c.esi, 0x39e))?);
                        cmp32(c, c.edx, slot(c, g, 8)?);
                        if !less(c) {
                            next = 0xe4740;
                        }
                    }
                    if next == 0xe46fc {
                        c.ecx = sx8(g.u8(at(c.esi, 0x39c))?);
                        sse::cvtsi2ss(c, 0, c.ecx);
                        sse::ss(c, Ss::Mul, 0, g.u32(at(c.eax, 4))?);
                        let x0 = sse::lane0(c, 0);
                        if below_or_equal(c, x0, g.u32(K_45DD74)?) {
                            0xe4740
                        } else {
                            c.eax = c.edi;
                            return call(points::BIPED_AIRBORNE_TEST.base, 0xe471c);
                        }
                    } else {
                        next
                    }
                }
            }
            0xe471c => {
                if flags8(c, c.eax as u8) != 0 {
                    0xe4740
                } else {
                    let w = g.u16(at(c.esi, 0x34c))?;
                    alu::cmp16(c, w.into(), 1);
                    if w == 1 {
                        0xe4740
                    } else {
                        push(c, g, c.edi)?;
                        return call(E4770, 0xe4730);
                    }
                }
            }
            0xe4730 => {
                if flags8(c, c.eax as u8) != 0 {
                    let dl = g.u8(at(c.esp, 8))?;
                    c.edx = byte(c.edx, dl);
                    g.set_u8(at(c.esi, 0x39e), dl)?;
                    c.ebx = byte(c.ebx, 1);
                }
                0xe4740
            }
            0xe4740 => {
                let al = g.u8(at(c.esi, 0x39e))?;
                c.eax = byte(c.eax, al);
                cmp8(c, al, 0x7f);
                if less(c) {
                    let al = inc8(c, al);
                    c.eax = byte(c.eax, al);
                    g.set_u8(at(c.esi, 0x39e), al)?;
                }
                0xe4760
            }
            0xe4760 => {
                c.esi = pop(c, g)?;
                c.eax = byte(c.eax, c.ebx as u8);
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 8, false);
                return Ok(Next::Ret { pop: 0 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}

/// `e2fa0 (object: eax, contact: ebx, unused: stack 0)`; `ret 4`. Frame
/// `sub esp, 8`, then `esi`, `edi` pushed (`[esp+8]`, `[esp+c]` the
/// `fistp` pair); `esi` the body, `edi` the object. The counter at body
/// `+398`: while negative it counts up, or with a contact is set to minus
/// the ticks of `[45dbbc]` seconds. Otherwise, with a contact that
/// `badc0` accepts: `1c9c00`; and when the biped has a target (`+13c`) or
/// `1fb6f0` answers, a new contact is stored at `+394` with the counter
/// cleared, and the same contact counts the counter up until it passes
/// `[45dc68]` seconds, when it is set to minus the ticks again.
pub(in crate::sim_objects) fn contact_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_CONTACT_UPDATE.base;
    // `e30b5`: `pop edi`, `pop esi`, drop the frame, `ret 4`. The other
    // returns store between the two pops, as retail orders them.
    fn done(c: &mut runtime::CpuAbi, g: &mut runtime::Guest) -> Result<Next> {
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
        c.esp = arithmetic(c, c.esp, 8, false);
        Ok(Next::Ret { pop: 4 })
    }
    // The ticks of `[45dbbc]` seconds into `[esp+dst]` through `[esp+src]`,
    // negated into `dl`.
    fn minus_ticks(c: &mut runtime::CpuAbi, g: &mut runtime::Guest, src: u32, dst: u32, fistp: u32) -> Result<u8> {
        c.ecx = sx16(g.u16(at(c.eax, 2))?);
        sse::cvtsi2ss(c, 0, c.ecx);
        sse::ss(c, Ss::Mul, 0, g.u32(K_45DBBC)?);
        store_ss(c, g, at(c.esp, src), 0)?;
        fld_fistp(c, g, at(c.esp, src), at(c.esp, dst), fistp)?;
        let dl = neg8(c, g.u8(at(c.esp, dst))?);
        c.edx = byte(c.edx, dl);
        Ok(dl)
    }
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                c.esp = arithmetic(c, c.esp, 8, true);
                push(c, g, c.esi)?;
                push(c, g, c.edi)?;
                c.edi = c.eax;
                c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
                c.esi = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
                let al = g.u8(at(c.esi, 0x398))?;
                c.eax = byte(c.eax, al);
                flags8(c, al);
                if (al as i8) >= 0 {
                    0xe3012
                } else {
                    cmp32(c, c.ebx, NONE);
                    if c.ebx == NONE {
                        let al = inc8(c, al);
                        c.eax = byte(c.eax, al);
                        c.edi = pop(c, g)?;
                        g.set_u8(at(c.esi, 0x398), al)?;
                        c.esi = pop(c, g)?;
                        c.esp = arithmetic(c, c.esp, 8, false);
                        return Ok(Next::Ret { pop: 4 });
                    }
                    c.eax = g.u32(points::GAME_TIME.base)?;
                    let dl = minus_ticks(c, g, 8, 0xc, 0xe2ffa)?;
                    c.edi = pop(c, g)?;
                    g.set_u8(at(c.esi, 0x398), dl)?;
                    c.esi = pop(c, g)?;
                    c.esp = arithmetic(c, c.esp, 8, false);
                    return Ok(Next::Ret { pop: 4 });
                }
            }
            0xe3012 => {
                cmp32(c, c.ebx, NONE);
                if c.ebx == NONE {
                    0xe30b5
                } else {
                    push(c, g, NONE)?;
                    c.eax = c.ebx;
                    return call(BADC0, 0xe3024);
                }
            }
            0xe3024 => {
                logic(c, c.eax, 1 << 31);
                if c.eax == 0 {
                    0xe30b5
                } else {
                    c.eax = c.ebx;
                    return call(X1C9C00, 0xe3033);
                }
            }
            0xe3033 => {
                let target = g.u32(at(c.esi, 0x13c))?;
                cmp32(c, target, NONE);
                if target != NONE {
                    0xe3046
                } else {
                    push(c, g, c.edi)?;
                    return call(X1FB6F0, 0xe3042);
                }
            }
            0xe3042 => {
                if flags8(c, c.eax as u8) == 0 { 0xe30b5 } else { 0xe3046 }
            }
            0xe3046 => {
                let contact = g.u32(at(c.esi, 0x394))?;
                cmp32(c, contact, c.ebx);
                if contact != c.ebx {
                    c.edi = pop(c, g)?;
                    g.set_u32(at(c.esi, 0x394), c.ebx)?;
                    g.set_u8(at(c.esi, 0x398), 0)?;
                    c.esi = pop(c, g)?;
                    c.esp = arithmetic(c, c.esp, 8, false);
                    return Ok(Next::Ret { pop: 4 });
                }
                let al = inc8(c, g.u8(at(c.esi, 0x398))?);
                c.eax = byte(c.eax, al);
                g.set_u8(at(c.esi, 0x398), al)?;
                c.eax = sx8(al);
                sse::cvtsi2ss(c, 0, c.eax);
                c.eax = g.u32(points::GAME_TIME.base)?;
                sse::ss(c, Ss::Mul, 0, g.u32(at(c.eax, 4))?);
                let x0 = sse::lane0(c, 0);
                if !below_or_equal(c, x0, g.u32(K_45DC68)?) {
                    let dl = minus_ticks(c, g, 0xc, 8, 0xe30a5)?;
                    g.set_u8(at(c.esi, 0x398), dl)?;
                }
                0xe30b5
            }
            0xe30b5 => return done(c, g),
            _ => return Err(unowned(pc)),
        };
    }
}
