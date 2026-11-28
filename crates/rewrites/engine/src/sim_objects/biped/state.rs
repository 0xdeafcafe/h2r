//! The biped update's state segments: the timer update `e2d80`, the
//! action request `e68c0`, the fall update `e45e0` and the animation state
//! update `e4c50`. Each is the retail code block by block, keyed by
//! retail address; the guest calls they make stay callouts, except the
//! root velocity `ba1d0` (a slice 4 body, fused).

use super::{
    NONE, and32, arithmetic, at, below_or_equal, call, cmp8, dec8, flags8, pop, push, set_slot, slot, store_ss, sx8,
    sx16, ucomiss_jp,
};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_objects::alu;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{byte, logic};

/// `.rdata` constants: f32 at `45dc68` (a time in seconds), `45dbc0`,
/// `45dc6c`, `44ac38`.
const K_45DC68: u32 = 0x0045_dc68;
const K_45DBC0: u32 = 0x0045_dbc0;
const K_45DC6C: u32 = 0x0045_dc6c;
const K_44AC38: u32 = 0x0044_ac38;

/// The `imm32`s of the four `push imm32` at `e2e02`..`e2e11` (string IDs),
/// in push order.
const TIMER_PUSHES: [u32; 4] = [0x000e_2e03, 0x000e_2e08, 0x000e_2e0d, 0x000e_2e12];
/// The `imm32`s of `cmp ecx, imm32` at `e4c98` and `e4ca0` (string IDs).
const STATE_NAME_A: u32 = 0x000e_4c9a;
const STATE_NAME_B: u32 = 0x000e_4ca2;
/// The `imm32` of `push imm32` at `e465d` (a float).
const FALL_PUSH: u32 = 0x000e_465e;

/// Callees that stay guest.
const E5930: u32 = 0x000e_5930;
const X10F430: u32 = 0x0010_f430;
const BFA40: u32 = 0x000b_fa40;
const X1FAF30: u32 = 0x001f_af30;
const E6900: u32 = 0x000e_6900;
const E4D90: u32 = 0x000e_4d90;

/// Every return address of `e2d80`'s calls.
pub(in crate::sim_objects) const TIMER_SITES: [u32; 4] = [0xe2dfe, 0xe2e21, 0xe2e50, 0xe2e6b];
/// The return address of `e68c0`'s call.
pub(in crate::sim_objects) const ACTION_SITES: [u32; 1] = [0xe68f6];
/// Every return address of `e45e0`'s calls.
pub(in crate::sim_objects) const FALL_SITES: [u32; 2] = [0xe4626, 0xe466f];
/// The return address of `e4c50`'s call.
pub(in crate::sim_objects) const STATE_SITES: [u32; 1] = [0xe4d7a];

/// `e2d80 (object: stack 0) -> al`; `ret 4`. `ebx`, `ebp`, `esi`, `edi`
/// pushed; across the calls `ebp` the object, `esi` the header offset
/// (then the body), `edi` the body, `bl` 0. With the counter at body
/// `+39d` exactly 1, state `+3dc` 3, definition `+1f0` bit 10 and body
/// `+10a` bit 2 clear: `e5930`, then `10f430` (`eax` 2, the object and six
/// words, four of them string IDs read from the image); then, with a node
/// word at `+112`, `bfa40` when body `+4` bit 29 is set and that bit
/// cleared. State 2 calls `1faf30` on body `+3ec`. Last, a non-zero
/// counter is decremented and `al` is 1; otherwise `al` is `bl`, 0.
pub(in crate::sim_objects) fn timer_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_TIMER_UPDATE.base;
    if pc != entry && !TIMER_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                c.ebp = slot(c, g, 0xc)?;
                c.eax = and32(c, c.ebp & 0xffff);
                push(c, g, c.esi)?;
                c.esi = c.eax.wrapping_mul(3);
                c.eax = g.u32(points::OBJECTS.base)?;
                c.ecx = g.u32(at(c.eax, DATA))?;
                c.esi = alu::shl(c, c.esi, 2, 32);
                push(c, g, c.edi)?;
                c.edi = g.u32(at(at(c.ecx, c.esi), 8))?;
                let al = g.u8(at(c.edi, 0x39d))?;
                c.eax = byte(c.eax, al);
                c.ebx = byte(c.ebx, flags8(c, 0));
                if flags8(c, al) == 0 {
                    0xe2e6b
                } else {
                    let state = g.u8(at(c.edi, 0x3dc))?;
                    cmp8(c, state, 3);
                    if state != 3 {
                        0xe2e6b
                    } else {
                        c.edx = g.u32(c.edi)?;
                        c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                        c.edx = and32(c, c.edx & 0xffff);
                        c.edx = alu::shl(c, c.edx, 4, 32);
                        c.edx = g.u32(at(at(c.edx, c.ecx), 8))?;
                        c.ecx = g.u32(at(c.edx, 0x1f0))?;
                        c.ecx = alu::shr(c, c.ecx, 0xa, 32);
                        if flags8(c, c.ecx as u8 & 1) == 0 {
                            0xe2e6b
                        } else {
                            let dl = alu::shr(c, g.u8(at(c.edi, 0x10a))?.into(), 2, 8) as u8;
                            c.edx = byte(c.edx, dl);
                            if flags8(c, dl & 1) != 0 {
                                0xe2e6b
                            } else {
                                cmp8(c, al, 1);
                                if al != 1 {
                                    0xe2e6b
                                } else {
                                    c.eax = c.ebp;
                                    return call(E5930, 0xe2dfe);
                                }
                            }
                        }
                    }
                }
            }
            0xe2dfe => {
                push(c, g, 0)?;
                push(c, g, 0)?;
                for imm in TIMER_PUSHES {
                    push(c, g, g.u32(imm)?)?;
                }
                push(c, g, c.ebp)?;
                c.eax = 2;
                return call(X10F430, 0xe2e21);
            }
            0xe2e21 => {
                arithmetic(c, c.ebp, NONE, true);
                if c.ebp == NONE {
                    0xe2e57
                } else {
                    c.eax = g.u32(points::OBJECTS.base)?;
                    c.ecx = g.u32(at(c.eax, DATA))?;
                    c.esi = g.u32(at(at(c.ecx, c.esi), 8))?;
                    let w = g.u16(at(c.esi, 0x112))?;
                    alu::cmp16(c, w.into(), 0xffff);
                    if w == 0xffff {
                        0xe2e57
                    } else {
                        c.edx = g.u32(at(c.esi, 4))?;
                        c.edx = alu::shr(c, c.edx, 0x1d, 32);
                        if flags8(c, c.edx as u8 & 1) == 0 {
                            0xe2e50
                        } else {
                            push(c, g, 0)?;
                            c.eax = c.ebp;
                            return call(BFA40, 0xe2e50);
                        }
                    }
                }
            }
            0xe2e50 => {
                let v = g.u32(at(c.esi, 4))? & 0xdfff_ffff;
                g.set_u32(at(c.esi, 4), v)?;
                logic(c, v, 1 << 31);
                0xe2e57
            }
            0xe2e57 => {
                let state = g.u8(at(c.edi, 0x3dc))?;
                cmp8(c, state, 2);
                if state != 2 {
                    0xe2e6b
                } else {
                    c.eax = at(c.edi, 0x3ec);
                    return call(X1FAF30, 0xe2e6b);
                }
            }
            0xe2e6b => {
                let al = g.u8(at(c.edi, 0x39d))?;
                c.eax = byte(c.eax, al);
                let counted = flags8(c, al) != 0;
                if counted {
                    let al = dec8(c, al);
                    c.eax = byte(c.eax, al);
                    g.set_u8(at(c.edi, 0x39d), al)?;
                }
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.eax = byte(c.eax, if counted { 1 } else { c.ebx as u8 });
                c.ebx = pop(c, g)?;
                return Ok(Next::Ret { pop: 4 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}

/// `e68c0 (action: ecx, object: stack 0)`; `ret 4`. An eight-dword request
/// on the stack (`sub esp, 20`, zeroed), its first word the action, handed
/// to `e6900` with `eax` the object and `edi` the request (`edi` pushed
/// and restored).
pub(in crate::sim_objects) fn action(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        p if p == points::BIPED_ACTION.base => {
            c.esp = arithmetic(c, c.esp, 0x20, true);
            c.eax = and32(c, 0);
            for i in 0..7 {
                set_slot(c, g, 4 * i, c.eax)?;
            }
            push(c, g, c.edi)?;
            set_slot(c, g, 0x20, c.eax)?;
            c.eax = slot(c, g, 0x28)?;
            c.edi = at(c.esp, 4);
            set_slot(c, g, 4, c.ecx)?;
            call(E6900, 0xe68f6)
        }
        0xe68f6 => {
            c.edi = pop(c, g)?;
            c.esp = arithmetic(c, c.esp, 0x20, false);
            Ok(Next::Ret { pop: 4 })
        }
        _ => Err(unowned(pc)),
    }
}

/// `e45e0 (object: esi)`; `ret`. When the counter at body `+39a` times the
/// tick length exceeds `[45dc68]`: the root velocity (`ba1d0`, fused) into
/// a 12-byte frame, and when its squared length exceeds `[45dbc0]`,
/// `e4d90` (`edx` 2; the object, 2, and a float read from the image).
pub(in crate::sim_objects) fn fall_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        p if p == points::BIPED_FALL_UPDATE.base => {
            c.ecx = g.u32(points::OBJECTS.base)?;
            c.edx = g.u32(at(c.ecx, DATA))?;
            c.eax = and32(c, c.esi & 0xffff).wrapping_mul(3);
            c.eax = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
            c.ecx = sx8(g.u8(at(c.eax, 0x39a))?);
            c.edx = g.u32(points::GAME_TIME.base)?;
            sse::cvtsi2ss(c, 0, c.ecx);
            sse::ss(c, Ss::Mul, 0, g.u32(at(c.edx, 4))?);
            c.esp = arithmetic(c, c.esp, 0xc, true);
            let x0 = sse::lane0(c, 0);
            if below_or_equal(c, x0, g.u32(K_45DC68)?) {
                return fall_ret(c);
            }
            push(c, g, 0)?;
            c.ecx = at(c.esp, 4);
            c.eax = c.esi;
            call(points::OBJECT_ROOT_VELOCITY.base, 0xe4626)
        }
        0xe4626 => {
            sse::load(c, 0, slot(c, g, 8)?);
            sse::movaps(c, 1, 0);
            sse::ss_reg(c, Ss::Mul, 1, 0);
            sse::load(c, 0, slot(c, g, 4)?);
            sse::movaps(c, 2, 0);
            sse::ss_reg(c, Ss::Mul, 2, 0);
            sse::load(c, 0, slot(c, g, 0)?);
            sse::ss_reg(c, Ss::Add, 1, 2);
            sse::movaps(c, 2, 0);
            sse::ss_reg(c, Ss::Mul, 2, 0);
            sse::ss_reg(c, Ss::Add, 1, 2);
            let x1 = sse::lane0(c, 1);
            if below_or_equal(c, x1, g.u32(K_45DBC0)?) {
                return fall_ret(c);
            }
            push(c, g, g.u32(FALL_PUSH)?)?;
            push(c, g, 2)?;
            push(c, g, c.esi)?;
            c.edx = 2;
            call(E4D90, 0xe466f)
        }
        0xe466f => fall_ret(c),
        _ => Err(unowned(pc)),
    }
}

/// `e466f`: `add esp, c; ret`.
fn fall_ret(c: &mut runtime::CpuAbi) -> Result<Next> {
    c.esp = arithmetic(c, c.esp, 0xc, false);
    Ok(Next::Ret { pop: 0 })
}

/// `e4c50 (object: edi)`; `ret`. `ecx` and `esi` pushed (the saved `ecx`
/// slot is the float argument's spill, so `ecx` can return as that
/// float); `esi` the body, `dl` whether to report, `xmm1` 0. By the
/// animation kind at body `+34a`: 0 counts `+39b` up while positive and,
/// past `[45dc68]` seconds, clears it and reports; 1 sets it to 1; any
/// other clears it. Then, when the animation block's name (body
/// `+ [+12a] + 7c`) is one of two string IDs read from the image and the
/// block is unset or its scaled `+1c` is zero, and the biped reports, the
/// weight `[45dbc0]` or `+39f` scaled by `[44ac38]` and clamped to 0..that
/// goes to `e4d90` (`edx` 2; the object, 3, the weight) and `+39f` is
/// cleared.
pub(in crate::sim_objects) fn animation_state(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_ANIMATION_STATE_UPDATE.base;
    if pc != entry && !STATE_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                push(c, g, c.ecx)?;
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                sse::zero(c, 1);
                c.eax = and32(c, c.edi & 0xffff).wrapping_mul(3);
                push(c, g, c.esi)?;
                c.esi = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
                c.eax = sx8(g.u8(at(c.esi, 0x34a))?);
                c.edx = byte(c.edx, flags8(c, 0));
                c.eax = arithmetic(c, c.eax, 0, true);
                if c.eax == 0 {
                    0xe4d1a
                } else {
                    c.eax = alu::dec32(c, c.eax);
                    if c.eax == 0 {
                        0xe4d0e
                    } else {
                        g.set_u8(at(c.esi, 0x39b), c.edx as u8)?;
                        0xe4c8b
                    }
                }
            }
            0xe4c8b => {
                c.eax = sx16(g.u16(at(c.esi, 0x12a))?);
                c.ecx = g.u32(at(at(c.eax, c.esi), 0x7c))?;
                c.eax = arithmetic(c, c.eax, c.esi, false);
                let a = g.u32(STATE_NAME_A)?;
                arithmetic(c, c.ecx, a, true);
                let named = c.ecx == a || {
                    let b = g.u32(STATE_NAME_B)?;
                    arithmetic(c, c.ecx, b, true);
                    c.ecx == b
                };
                if !named {
                    0xe4d81
                } else {
                    let tag = g.u32(c.eax)?;
                    arithmetic(c, tag, NONE, true);
                    if tag == NONE {
                        0xe4cd2
                    } else {
                        let w = g.u16(at(c.eax, 6))?;
                        alu::cmp16(c, w.into(), 0xffff);
                        if w == 0xffff {
                            0xe4cd2
                        } else {
                            sse::load(c, 0, g.u32(at(c.eax, 0x1c))?);
                            sse::ss(c, Ss::Mul, 0, g.u32(K_45DC6C)?);
                            let (x0, x1) = (sse::lane0(c, 0), sse::lane0(c, 1));
                            if ucomiss_jp(c, x0, x1) { 0xe4d81 } else { 0xe4cd2 }
                        }
                    }
                }
            }
            0xe4cd2 => {
                let dl = flags8(c, c.edx as u8);
                sse::load(c, 2, g.u32(K_45DBC0)?);
                store_ss(c, g, at(c.esp, 4), 2)?;
                if dl == 0 {
                    0xe4d68
                } else {
                    c.edx = sx8(g.u8(at(c.esi, 0x39f))?);
                    sse::cvtsi2ss(c, 0, c.edx);
                    sse::ss(c, Ss::Mul, 0, g.u32(K_44AC38)?);
                    let (x1, x0) = (sse::lane0(c, 1), sse::lane0(c, 0));
                    let not_above = below_or_equal(c, x1, x0);
                    store_ss(c, g, at(c.esp, 4), 0)?;
                    if not_above {
                        let (x0, x2) = (sse::lane0(c, 0), sse::lane0(c, 2));
                        if !below_or_equal(c, x0, x2) {
                            store_ss(c, g, at(c.esp, 4), 2)?;
                        }
                    } else {
                        store_ss(c, g, at(c.esp, 4), 1)?;
                    }
                    0xe4d68
                }
            }
            0xe4d0e => {
                g.set_u8(at(c.esi, 0x39b), 1)?;
                0xe4c8b
            }
            0xe4d1a => {
                let al = g.u8(at(c.esi, 0x39b))?;
                c.eax = byte(c.eax, al);
                flags8(c, al);
                if alu::less_or_equal(c) {
                    0xe4c8b
                } else {
                    c.ecx = g.u32(points::GAME_TIME.base)?;
                    // `inc al`: its flags are overwritten by the `comiss`.
                    let al = al.wrapping_add(1);
                    c.eax = byte(c.eax, al);
                    g.set_u8(at(c.esi, 0x39b), al)?;
                    c.eax = sx8(al);
                    sse::cvtsi2ss(c, 0, c.eax);
                    sse::ss(c, Ss::Mul, 0, g.u32(at(c.ecx, 4))?);
                    let x0 = sse::lane0(c, 0);
                    if below_or_equal(c, x0, g.u32(K_45DC68)?) {
                        0xe4c8b
                    } else {
                        g.set_u8(at(c.esi, 0x39b), 0)?;
                        c.edx = byte(c.edx, 1);
                        0xe4cd2
                    }
                }
            }
            0xe4d68 => {
                c.eax = slot(c, g, 4)?;
                push(c, g, c.eax)?;
                push(c, g, 3)?;
                push(c, g, c.edi)?;
                c.edx = 2;
                return call(E4D90, 0xe4d7a);
            }
            0xe4d7a => {
                g.set_u8(at(c.esi, 0x39f), 0)?;
                0xe4d81
            }
            0xe4d81 => {
                c.esi = pop(c, g)?;
                c.ecx = pop(c, g)?;
                return Ok(Next::Ret { pop: 0 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}
