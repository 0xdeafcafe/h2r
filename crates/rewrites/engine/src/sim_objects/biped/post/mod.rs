//! The biped post-physics update (slice 7b of
//! docs/engine/sim-rewrite-objects.md; contracts in
//! docs/engine/object-update.md, "Slice 7b bodies"): the `bipd`
//! descriptor's slot `+44` (`dd990`) and the part of its tree that is
//! ported: the crouch step `dd7d0`, the crouch height `dd8e0` with its
//! forced test `e0dc0`, the ground probe `e30c0` with its surface test
//! `ddf60`, the jump update `e4680`, the physics input `dd0d0`, the
//! landing update `e4330` and the contact update `e2fa0`. The rest stays
//! guest: the AI hooks `1ec500` and `1e5a60`, the damage packet
//! (`d6660`, `d66d0`, `d7b80`), the jump `e4770`, the physics step
//! `1e68a0`, and the shared callees listed in the doc.
//!
//! Every body is a segment over the retail frame, the retail code block by
//! block, keyed by retail address: at each call and return the eight
//! integer registers, the arithmetic flags of the last instruction that
//! set them, the `xmm` registers, MXCSR and the x87 state are retail's.
//!
//! **`dd990 (object: stack 0)`; `ret 4`.** Frame: `sub esp, b8`, then
//! `ebp`, `esi`, `edi` pushed, and `ebx` once the biped has a physics
//! component (body `+b4`; `S` = that `esp`, the `esp` at every site). With
//! `S`: `[S+10]` the component record, `[S+14]` the crouch height's out
//! float (and the contact record pointer on the ground path), `[S+18]` the
//! crouch step (its low byte then the jump update's answer, the whole
//! dword pushed to `dd0d0`), `[S+1c..24]` the root velocity (then the
//! physics output `1e5a60` fills: `[S+20]` its flags byte, `[S+24]` a
//! contact index, `[S+28]` an object), `[S+2c]` the surface query buffer,
//! `[S+40]` the damage packet (then the physics input record), the object
//! at `[S+cc]`. Across the calls `ebp` holds the body and `esi` the object
//! (from `ddba5` on, none; the object is read back from `[S+cc]` into
//! `edi`).
//!
//! In order: the root `baf80` (fused), the crouch step `dd7d0` (fused)
//! into `[S+18]`, the ground probe `e30c0` (fused). Unless the root's body
//! `+c1` bit 0 is set: `1ec500`, and when it answers, body `+349 |= 80`
//! and outside network mode 4 a damage packet (`d6660` on the matg's
//! `+144` record `+28`, `d66d0`, `d7b80`). The crouch height `dd8e0`
//! (fused): when it answers, the root velocity (`ba1d0`) less the height
//! on its `z`, set back (`b7740`), `1c4b00`, and above a squared speed
//! `b9b90`, the wake `b7360` and `bba20`. Body `+2c4` plus the step,
//! clamped to `[0, [45dbc0]]`; `+349 &= f7`; the jump update `e4680`
//! (fused); `1d1c10` on the component; unless either answered, a set body
//! `+c0` bit 6 ends the update. Then `+349 &= ef`, the world position
//! (`b9dd0`) into body `+368`, the contact `+354..364` cleared unless
//! `+c1` bit 0; unparented with `+c0` bit 6, or in state 3 with `+404`
//! set: the physics input `dd0d0` (fused), `1e5a60`, the contact from the
//! output's index (the component's contact array `+88`, `48` bytes each,
//! by its kind: a surface, an object through `1efb40` and `1efd80`, or
//! the record's object), the landing update `e4330` (fused) in state 1
//! with output bit 1, the contact's two vectors into body `+380` and
//! `+374` (`1201a0`) and the position into `+368`, the contact update
//! `e2fa0` (fused) with the output's object; and body `+348` bit 0 from
//! the output's bit 0, bit 7 cleared.
//!
//! Instruction immediates the public source must not carry (string IDs,
//! collision flags, a float) are read from the loaded image, as are the
//! `.rdata` constants and the jump table of `dd0d0`.

mod input;
mod leaves;
mod probe;
mod steps;

pub(in crate::sim_objects) use input::{INPUT_SITES, physics_input};
pub(in crate::sim_objects) use leaves::{crouch_forced, ground_flag, landing_update};
pub(in crate::sim_objects) use probe::{PROBE_SITES, ground_probe};
pub(in crate::sim_objects) use steps::{
    CONTACT_SITES, CROUCH_HEIGHT_SITES, CROUCH_STEP_SITES, JUMP_SITES, contact_update, crouch_height, crouch_step,
    jump_update,
};

use super::{NONE, and32, at, below_or_equal, call, cmp8, flags8, store_ss, sx8, sx16};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_objects::alu;
use crate::sim_objects::frame::{set_slot, slot};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, status};
use runtime::{CpuAbi, Guest};

/// `dd990`'s locals.
const FRAME: u32 = 0xb8;

/// `.rdata` constants: f32 at `45dbdc` (a squared speed) and `45dbc0`
/// (the crouch fraction's upper clamp).
const K_45DBDC: u32 = 0x0045_dbdc;
const K_45DBC0: u32 = 0x0045_dbc0;
/// Holds the structure (BSP) whose `+144`, `+13c` the surface contact
/// reads.
const STRUCTURE: u32 = 0x004e_0348;
/// Holds the object-per-index table a kind-4 contact reads.
const CONTACT_OBJECTS: u32 = 0x0051_e9cc;
/// Holds the argument `1201a0` takes in `ecx`.
const X4687B0: u32 = 0x0046_87b0;

/// Callees that stay guest.
const X1EC500: u32 = 0x001e_c500;
const D66D0: u32 = 0x000d_66d0;
const D7B80: u32 = 0x000d_7b80;
const X1C4B00: u32 = 0x001c_4b00;
const B9B90: u32 = 0x000b_9b90;
const X1D1C10: u32 = 0x001d_1c10;
const X1E5A60: u32 = 0x001e_5a60;
const X1EFB40: u32 = 0x001e_fb40;
const X1EFD80: u32 = 0x001e_fd80;
const X1CFDF0: u32 = 0x001c_fdf0;
const X1201A0: u32 = 0x0012_01a0;
/// Network mode 4 (`[game globals + c]`), where no damage packet is sent.
const MODE_4: u8 = 4;

/// Every return address of `dd990`'s calls.
pub(in crate::sim_objects) const POST_SITES: [u32; 25] = [
    0xdd9e7, 0xdd9fa, 0xdda07, 0xdda17, 0xdda57, 0xdda74, 0xdda87, 0xdda93, 0xddaa8, 0xddac9, 0xddad7, 0xddb17,
    0xddb1e, 0xddb25, 0xddb61, 0xddb70, 0xddba5, 0xddc07, 0xddc1a, 0xddd89, 0xddd96, 0xdde66, 0xdde82, 0xddeda,
    0xddf16,
];

/// `inc r8`: CF kept.
fn inc8(c: &mut CpuAbi, v: u8) -> u8 {
    let r = v.wrapping_add(1);
    let af = u32::from(v & 15 == 15) << 4;
    let of = u32::from(v == 0x7f) << 11;
    let keep = ARITH & !1;
    c.eflags = (c.eflags & !keep) | status(r.into(), 0x80) | af | of;
    c.eflags_defined |= keep;
    r
}

/// `inc r32`: CF kept.
fn inc32(c: &mut CpuAbi, v: u32) -> u32 {
    let r = v.wrapping_add(1);
    let af = u32::from(v & 15 == 15) << 4;
    let of = u32::from(v == 0x7fff_ffff) << 11;
    let keep = ARITH & !1;
    c.eflags = (c.eflags & !keep) | status(r, 1 << 31) | af | of;
    c.eflags_defined |= keep;
    r
}

/// `neg r8`: `0 - v`, CF set unless `v` is 0.
fn neg8(c: &mut CpuAbi, v: u8) -> u8 {
    let r = 0u8.wrapping_sub(v);
    let cf = u32::from(v != 0);
    let af = u32::from(v & 15 != 0) << 4;
    let of = u32::from(v == 0x80) << 11;
    c.eflags = (c.eflags & !ARITH) | status(r.into(), 0x80) | cf | af | of;
    c.eflags_defined |= ARITH;
    r
}

/// `cmp r32, r/m32` (or `imm`): the flags only.
fn cmp32(c: &mut CpuAbi, a: u32, b: u32) {
    arithmetic(c, a, b, true);
}

/// `ja` after a `comiss a, b`: neither CF nor ZF.
fn above(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    !below_or_equal(c, a, b)
}

/// `ddf46` (and `ddf45` with `pop ebx` first): body `+348 &= 7f`, `pop
/// edi, esi, ebp`, drop the frame, `ret 4`.
fn post_ret(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    alu::rmw8(c, g, at(c.ebp, 0x348), |v| v & 0x7f)?;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    Ok(Next::Ret { pop: 4 })
}

/// Store `v` in the five contact words at body `+354..364`, in retail's
/// order for the kind (the order matters only for the stores' values).
fn contact(g: &mut Guest, body: u32, words: [(u32, u32); 5]) -> Result<()> {
    for (off, v) in words {
        g.set_u32(at(body, off), v)?;
    }
    Ok(())
}

/// `dd990 (object: stack 0)`; `ret 4`.
pub(in crate::sim_objects) fn post_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_POST_UPDATE.base;
    if pc != entry && !POST_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                c.eax = g.u32(points::OBJECTS.base)?;
                c.esp = arithmetic(c, c.esp, FRAME, true);
                push(c, g, c.ebp)?;
                push(c, g, c.esi)?;
                c.esi = slot(c, g, 0xc4)?;
                push(c, g, c.edi)?;
                c.edi = g.u32(at(c.eax, DATA))?;
                c.eax = and32(c, c.esi & 0xffff);
                c.ecx = c.eax.wrapping_mul(3);
                c.ebp = g.u32(at(c.edi.wrapping_add(c.ecx.wrapping_mul(4)), 8))?;
                c.eax = g.u32(at(c.ebp, 0xb4))?;
                cmp32(c, c.eax, NONE);
                if c.eax == NONE {
                    return post_ret(c, g);
                }
                c.edx = g.u32(points::PHYSICS_COMPONENTS.base)?;
                c.eax = and32(c, c.eax & 0xffff);
                c.eax = c.eax.wrapping_mul(5);
                push(c, g, c.ebx)?;
                c.ebx = g.u32(at(c.edx, DATA))?;
                c.eax = alu::shl(c, c.eax, 5, 32);
                c.eax = arithmetic(c, c.eax, c.ebx, false);
                c.ecx = c.esi;
                set_slot(c, g, 0x10, c.eax)?;
                return call(points::OBJECT_ROOT.base, 0xdd9e7);
            }
            0xdd9e7 => {
                // `edi` still holds the datum array's headers.
                c.eax = and32(c, c.eax & 0xffff);
                c.eax = c.eax.wrapping_mul(3);
                c.edi = g.u32(at(c.edi.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
                c.eax = c.esi;
                return call(points::BIPED_CROUCH_STEP.base, 0xdd9fa);
            }
            0xdd9fa => {
                c.ebx = c.esi;
                store_ss(c, g, at(c.esp, 0x18), 0)?;
                return call(points::BIPED_GROUND_PROBE.base, 0xdda07);
            }
            0xdda07 => {
                if alu::test8(c, g, at(c.edi, 0xc1), 1)? {
                    0xdda87
                } else {
                    c.edi = c.esi;
                    return call(X1EC500, 0xdda17);
                }
            }
            0xdda17 => {
                if flags8(c, c.eax as u8) == 0 {
                    0xdda87
                } else {
                    alu::rmw8(c, g, at(c.ebp, 0x349), |v| v | 0x80)?;
                    c.ecx = g.u32(points::GAME_GLOBALS.base)?;
                    let al = g.u8(at(c.ecx, 0xc))?;
                    c.eax = byte(c.eax, al);
                    c.ebx = u32::from(MODE_4);
                    cmp8(c, al, MODE_4);
                    if al == MODE_4 {
                        0xdda87
                    } else {
                        c.edx = g.u32(points::GLOBALS_TAG.base)?;
                        c.eax = g.u32(at(c.edx, 0x144))?;
                        g.set_u16(at(c.esp, 0xbc), 0xffff)?;
                        c.ecx = g.u32(at(c.eax, 0x28))?;
                        push(c, g, c.ecx)?;
                        c.edx = at(c.esp, 0x44);
                        return call(points::DAMAGE_DATA_INIT.base, 0xdda57);
                    }
                }
            }
            0xdda57 => {
                c.eax = slot(c, g, 0x44)?;
                c.edx = at(c.esp, 0x48);
                push(c, g, c.edx)?;
                c.eax |= c.ebx;
                logic(c, c.eax, 1 << 31);
                push(c, g, c.esi)?;
                g.set_u8(at(c.esp, 0xcc), 1)?;
                set_slot(c, g, 0x4c, c.eax)?;
                return call(D66D0, 0xdda74);
            }
            0xdda74 => {
                push(c, g, 0)?;
                push(c, g, NONE)?;
                push(c, g, NONE)?;
                push(c, g, NONE)?;
                push(c, g, c.esi)?;
                c.eax = at(c.esp, 0x54);
                push(c, g, c.eax)?;
                return call(D7B80, 0xdda87);
            }
            0xdda87 => {
                c.ecx = at(c.esp, 0x14);
                push(c, g, c.ecx)?;
                c.edi = c.esi;
                return call(points::BIPED_CROUCH_HEIGHT.base, 0xdda93);
            }
            0xdda93 => {
                if flags8(c, c.eax as u8) == 0 {
                    0xddb25
                } else {
                    push(c, g, 0)?;
                    c.ecx = at(c.esp, 0x20);
                    c.eax = c.esi;
                    return call(points::OBJECT_ROOT_VELOCITY.base, 0xddaa8);
                }
            }
            0xddaa8 => {
                sse::load(c, 0, slot(c, g, 0x24)?);
                sse::ss(c, Ss::Sub, 0, slot(c, g, 0x14)?);
                push(c, g, 0)?;
                c.edi = and32(c, 0);
                c.ecx = at(c.esp, 0x20);
                c.eax = c.esi;
                store_ss(c, g, at(c.esp, 0x28), 0)?;
                return call(points::OBJECT_SET_VELOCITY.base, 0xddac9);
            }
            0xddac9 => {
                push(c, g, 1)?;
                push(c, g, c.edi)?;
                c.ebx = at(c.esp, 0x24);
                c.eax = c.esi;
                return call(X1C4B00, 0xddad7);
            }
            0xddad7 => {
                sse::load(c, 0, slot(c, g, 0x1c)?);
                sse::movaps(c, 1, 0);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                sse::load(c, 0, slot(c, g, 0x24)?);
                sse::movaps(c, 2, 0);
                sse::ss_reg(c, Ss::Mul, 2, 0);
                sse::load(c, 0, slot(c, g, 0x20)?);
                sse::ss_reg(c, Ss::Add, 1, 2);
                sse::movaps(c, 2, 0);
                sse::ss_reg(c, Ss::Mul, 2, 0);
                sse::ss_reg(c, Ss::Add, 1, 2);
                let x1 = sse::lane0(c, 1);
                if below_or_equal(c, x1, g.u32(K_45DBDC)?) {
                    0xddb25
                } else {
                    push(c, g, c.esi)?;
                    c.edx = byte(c.edx, 0);
                    logic(c, 0, 0x80);
                    return call(B9B90, 0xddb17);
                }
            }
            0xddb17 => {
                c.eax = c.esi;
                return call(points::OBJECT_WAKE.base, 0xddb1e);
            }
            0xddb1e => {
                c.eax = c.esi;
                return call(points::OBJECT_WAKE_FOR_POST.base, 0xddb25);
            }
            0xddb25 => {
                // The crouch fraction plus the step, clamped to
                // `[0, [45dbc0]]`.
                sse::load(c, 0, slot(c, g, 0x18)?);
                sse::ss(c, Ss::Add, 0, g.u32(at(c.ebp, 0x2c4))?);
                sse::zero(c, 1);
                let x0 = sse::lane0(c, 0);
                if above(c, 0, x0) {
                    sse::movaps(c, 0, 1);
                } else {
                    sse::load(c, 1, g.u32(K_45DBC0)?);
                    let x1 = sse::lane0(c, 1);
                    if above(c, x0, x1) {
                        sse::movaps(c, 0, 1);
                    }
                }
                store_ss(c, g, at(c.ebp, 0x2c4), 0)?;
                alu::rmw8(c, g, at(c.ebp, 0x349), |v| v & 0xf7)?;
                c.edi = c.esi;
                return call(points::BIPED_JUMP_UPDATE.base, 0xddb61);
            }
            0xddb61 => {
                c.esi = slot(c, g, 0x10)?;
                c.ebx = byte(c.ebx, c.eax as u8);
                g.set_u8(at(c.esp, 0x18), c.ebx as u8)?;
                return call(X1D1C10, 0xddb70);
            }
            0xddb70 => {
                if flags8(c, c.eax as u8) != 0 || flags8(c, c.ebx as u8) != 0 {
                    0xddb8a
                } else {
                    let dl = alu::shr(c, g.u8(at(c.ebp, 0xc0))?.into(), 6, 8) as u8;
                    c.edx = byte(c.edx, dl);
                    if flags8(c, dl & 1) != 0 { 0xddf45 } else { 0xddb8a }
                }
            }
            0xddb8a => {
                c.edi = slot(c, g, 0xcc)?;
                alu::rmw8(c, g, at(c.ebp, 0x349), |v| v & 0xef)?;
                c.esi = at(c.ebp, 0x368);
                c.eax = c.edi;
                return call(points::OBJECT_WORLD_POSITION.base, 0xddba5);
            }
            0xddba5 => {
                let ax = g.u16(at(c.ebp, 0xc0))?;
                c.eax = (c.eax & 0xffff_0000) | u32::from(ax);
                c.esi = NONE;
                logic(c, NONE, 1 << 31);
                if flags8(c, (ax >> 8) as u8 & 1) == 0 {
                    for off in [0x354, 0x358, 0x35c, 0x360, 0x364] {
                        g.set_u32(at(c.ebp, off), c.esi)?;
                    }
                }
                // `ddbd2`: unparented with `+c0` bit 6, or in state 3 with
                // `+404` set.
                let parent = g.u32(at(c.ebp, 0x14))?;
                cmp32(c, parent, c.esi);
                let mut input = false;
                if parent == c.esi {
                    let al = alu::shr(c, c.eax & 0xff, 6, 8) as u8;
                    c.eax = byte(c.eax, al);
                    input = flags8(c, al & 1) != 0;
                }
                if input {
                    0xddbf7
                } else {
                    let state = g.u8(at(c.ebp, 0x3dc))?;
                    cmp8(c, state, 3);
                    if state != 3 {
                        0xddf45
                    } else {
                        let w = g.u32(at(c.ebp, 0x404))?;
                        cmp32(c, w, c.esi);
                        if w == c.esi { 0xddf45 } else { 0xddbf7 }
                    }
                }
            }
            0xddbf7 => {
                c.eax = slot(c, g, 0x18)?;
                push(c, g, c.eax)?;
                c.ecx = at(c.esp, 0x44);
                push(c, g, c.ecx)?;
                push(c, g, c.edi)?;
                return call(points::BIPED_PHYSICS_INPUT.base, 0xddc07);
            }
            0xddc07 => {
                c.ebx = at(c.ebp, 0x3dc);
                c.eax = at(c.esp, 0x40);
                c.ecx = at(c.esp, 0x1c);
                return call(X1E5A60, 0xddc1a);
            }
            0xddc1a => {
                let set = alu::test8(c, g, at(c.ebp, 0xc1), 1)?;
                c.edi = slot(c, g, 0x24)?;
                if set {
                    0xdde43
                } else {
                    cmp32(c, c.edi, c.esi);
                    if c.edi == c.esi {
                        0xdde43
                    } else {
                        // The index against the component's contact count:
                        // `ddc49` compares the count (or 0 for a negative
                        // index) with the index.
                        logic(c, c.edi, 1 << 31);
                        let next = if (c.edi as i32) < 0 {
                            c.eax = and32(c, 0);
                            0xddc49
                        } else {
                            c.edx = slot(c, g, 0x10)?;
                            c.eax = g.u32(at(c.edx, 0x8c))?;
                            cmp32(c, c.edi, c.eax);
                            if alu::less_or_equal(c) { 0xddc51 } else { 0xddc49 }
                        };
                        if next == 0xddc49 {
                            cmp32(c, c.eax, c.edi);
                            if c.eax != c.edi { 0xdde43 } else { 0xddc51 }
                        } else {
                            next
                        }
                    }
                }
            }
            0xddc51 => {
                c.ecx = slot(c, g, 0x10)?;
                c.edx = g.u32(at(c.ecx, 0x88))?;
                c.eax = c.edi.wrapping_mul(9);
                let record = c.edx.wrapping_add(c.eax.wrapping_mul(8));
                c.ecx = g.u32(at(record, 4))?;
                c.esi = record;
                c.ebx = NONE;
                logic(c, NONE, 1 << 31);
                cmp32(c, c.ecx, c.ebx);
                set_slot(c, g, 0x14, c.esi)?;
                if c.ecx == NONE {
                    0xdddf7
                } else {
                    // The contact's kind (bits 29..31) and its two fields.
                    c.eax = c.ecx;
                    c.edx = c.ecx;
                    c.eax = alu::shr(c, c.eax, 0x10, 32);
                    c.ecx = alu::shr(c, c.ecx, 0x1d, 32);
                    c.edx = and32(c, c.edx & 0xffff);
                    c.eax = and32(c, c.eax & 0x1fff);
                    c.ecx = alu::dec32(c, c.ecx);
                    if c.ecx == 0 {
                        0xdddcf
                    } else {
                        c.ecx = arithmetic(c, c.ecx, 3, true);
                        if c.ecx == 0 {
                            0xddcf0
                        } else {
                            c.ecx = alu::dec32(c, c.ecx);
                            if c.ecx != 0 { 0xdddf7 } else { 0xddc9c }
                        }
                    }
                }
            }
            0xddc9c => {
                // Kind 5: a surface of the structure.
                c.ecx = g.u32(STRUCTURE)?;
                c.ebx = g.u32(at(c.ecx, 0x144))?;
                c.ecx = g.u32(at(c.ecx, 0x13c))?;
                c.esi = c.edx;
                // `imul esi, 58` and `imul esi, c8`: their flags are
                // overwritten by the `test`.
                c.esi = c.esi.wrapping_mul(0x58);
                c.esi = sx16(g.u16(at(c.ebx.wrapping_add(c.esi), 0x34))?);
                c.esi = c.esi.wrapping_mul(0xc8);
                c.ecx = g.u32(at(c.esi.wrapping_add(c.ecx), 0xb4))?;
                c.ecx = g.u32(at(c.ecx, 0xc))?;
                c.ecx = g.u32(at(c.ecx, 0x2c))?;
                let flagged = alu::test8(c, g, at(c.ecx.wrapping_add(c.eax.wrapping_mul(8)), 4), 1)?;
                g.set_u32(at(c.ebp, 0x35c), c.edx)?;
                if flagged {
                    0xdde43
                } else {
                    g.set_u32(at(c.ebp, 0x354), c.eax)?;
                    c.eax = NONE;
                    logic(c, NONE, 1 << 31);
                    g.set_u32(at(c.ebp, 0x360), c.eax)?;
                    0xdde37
                }
            }
            0xddcf0 => {
                // Kind 4: an object through the per-index table.
                c.ecx = g.u32(CONTACT_OBJECTS)?;
                c.ebx = g.u32(c.ecx.wrapping_add(c.edx.wrapping_mul(4)))?;
                cmp32(c, c.ebx, NONE);
                if c.ebx == NONE {
                    0xdddf7
                } else {
                    c.ecx = c.ebx;
                    c.ecx = and32(c, c.ecx & 0xffff);
                    c.edx = c.ecx.wrapping_mul(3);
                    c.ecx = g.u32(points::OBJECTS.base)?;
                    c.ecx = g.u32(at(c.ecx, DATA))?;
                    c.esi = g.u32(at(c.ecx.wrapping_add(c.edx.wrapping_mul(4)), 8))?;
                    c.edx = g.u32(at(c.esi, 4))?;
                    c.edx = alu::shr(c, c.edx, 0x16, 32);
                    if flags8(c, c.edx as u8 & 1) == 0 {
                        0xdddf3
                    } else {
                        c.edx = g.u32(c.esi)?;
                        c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                        c.edx = and32(c, c.edx & 0xffff);
                        c.edx = alu::shl(c, c.edx, 4, 32);
                        c.edx = g.u32(at(at(c.edx, c.ecx), 8))?;
                        c.edx = g.u32(at(c.edx, 0x38))?;
                        c.edx = and32(c, c.edx & 0xffff);
                        c.edx = alu::shl(c, c.edx, 4, 32);
                        c.edx = g.u32(at(at(c.edx, c.ecx), 8))?;
                        c.esi = g.u32(at(c.edx, 0x74))?;
                        c.ecx = c.eax;
                        c.ecx = and32(c, c.ecx & 0x1f);
                        c.ecx = alu::shl(c, c.ecx, 4, 32);
                        c.edx = g.u32(at(c.ecx.wrapping_add(c.esi), 0xc))?;
                        // `sar eax, 5`: its flags are overwritten by the
                        // `add`.
                        c.eax = ((c.eax as i32) >> 5) as u32;
                        c.ecx = arithmetic(c, c.ecx, c.esi, false);
                        c.ecx = sx8(g.u8(at(c.ecx, 4))?);
                        c.eax = and32(c, c.eax & 0xff);
                        c.eax = sx8(g.u8(at(c.edx.wrapping_add(c.eax.wrapping_mul(8)), 5))?);
                        c.eax = and32(c, c.eax & 0xff);
                        c.ecx = alu::shl(c, c.ecx, 8, 32);
                        c.eax |= c.ecx;
                        logic(c, c.eax, 1 << 31);
                        c.eax = alu::shl(c, c.eax, 0x10, 32);
                        push(c, g, c.ebx)?;
                        c.esi = at(c.esp, 0x30);
                        c.edi = c.eax;
                        return call(X1EFB40, 0xddd89);
                    }
                }
            }
            0xddd89 => {
                if flags8(c, c.eax as u8) == 0 {
                    0xdddef
                } else {
                    push(c, g, c.edi)?;
                    c.edx = c.esi;
                    push(c, g, c.edx)?;
                    return call(X1EFD80, 0xddd96);
                }
            }
            0xddd96 => {
                c.ecx = slot(c, g, 0x14)?;
                c.edx = g.u32(at(c.ecx, 0xc))?;
                c.eax = g.u32(at(c.eax, 0x2c))?;
                if alu::test8(c, g, at(c.eax.wrapping_add(c.edx.wrapping_mul(8)), 4), 1)? {
                    0xdddef
                } else {
                    c.ecx = slot(c, g, 0x14)?;
                    c.eax = NONE;
                    logic(c, NONE, 1 << 31);
                    g.set_u32(at(c.ebp, 0x35c), c.eax)?;
                    g.set_u32(at(c.ebp, 0x360), c.ebx)?;
                    c.edx = g.u32(at(c.ecx, 0xc))?;
                    g.set_u32(at(c.ebp, 0x364), c.edi)?;
                    c.edi = slot(c, g, 0x24)?;
                    g.set_u32(at(c.ebp, 0x354), c.edx)?;
                    0xdde3d
                }
            }
            0xdddcf => {
                // Kind 1: the surface index alone.
                contact(g, c.ebp, [
                    (0x35c, c.ebx),
                    (0x354, c.edx),
                    (0x360, c.ebx),
                    (0x364, c.ebx),
                    (0x358, c.ebx),
                ])?;
                0xdde43
            }
            0xdddef => {
                c.edi = slot(c, g, 0x24)?;
                0xdddf3
            }
            0xdddf3 => {
                c.esi = slot(c, g, 0x14)?;
                0xdddf7
            }
            0xdddf7 => {
                // The record's own object, when it is in the world (body
                // `+4` bit 22).
                c.eax = g.u32(at(c.esi, 0x18))?;
                cmp32(c, c.eax, NONE);
                if c.eax == NONE {
                    0xdde43
                } else {
                    c.ecx = g.u32(points::OBJECTS.base)?;
                    c.edx = g.u32(at(c.ecx, DATA))?;
                    c.eax = and32(c, c.eax & 0xffff);
                    c.eax = c.eax.wrapping_mul(3);
                    c.eax = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
                    c.ecx = g.u32(at(c.eax, 4))?;
                    c.ecx = alu::shr(c, c.ecx, 0x16, 32);
                    if flags8(c, c.ecx as u8 & 1) == 0 {
                        0xdde43
                    } else {
                        c.eax = NONE;
                        logic(c, NONE, 1 << 31);
                        g.set_u32(at(c.ebp, 0x35c), c.eax)?;
                        c.edx = g.u32(at(c.esi, 0x18))?;
                        g.set_u32(at(c.ebp, 0x360), c.edx)?;
                        g.set_u32(at(c.ebp, 0x354), c.eax)?;
                        0xdde37
                    }
                }
            }
            0xdde37 => {
                g.set_u32(at(c.ebp, 0x364), c.eax)?;
                0xdde3d
            }
            0xdde3d => {
                g.set_u32(at(c.ebp, 0x358), c.eax)?;
                0xdde43
            }
            0xdde43 => {
                let state = g.u8(at(c.ebp, 0x3dc))?;
                cmp8(c, state, 1);
                if state != 1 || !alu::test8(c, g, at(c.esp, 0x20), 2)? {
                    c.esi = slot(c, g, 0x10)?;
                    0xdde88
                } else {
                    c.esi = slot(c, g, 0x10)?;
                    sse::load(c, 2, g.u32(at(c.ebp, 0x45c))?);
                    c.edx = c.esi;
                    return call(X1CFDF0, 0xdde66);
                }
            }
            0xdde66 => {
                if flags8(c, c.eax as u8) != 0 {
                    sse::load(c, 0, g.u32(at(c.esi, 0x14))?);
                    let x2 = sse::lane0(c, 2);
                    sse::ss(c, Ss::Add, 0, x2);
                    sse::movaps(c, 2, 0);
                }
                c.eax = slot(c, g, 0xcc)?;
                return call(points::BIPED_LANDING_UPDATE.base, 0xdde82);
            }
            0xdde82 => 0xdde88,
            0xdde88 => {
                cmp32(c, c.edi, NONE);
                if c.edi == NONE {
                    0xddefb
                } else {
                    // The contact record's two vectors (`+1c`, `+28`) into
                    // body `+380` and `+374`.
                    c.ecx = g.u32(at(c.esi, 0x88))?;
                    c.eax = c.edi.wrapping_mul(9);
                    c.ecx = c.ecx.wrapping_add(c.eax.wrapping_mul(8));
                    c.edx = at(c.ecx, 0x1c);
                    c.edi = g.u32(c.edx)?;
                    c.esi = at(c.ebp, 0x380);
                    c.eax = c.esi;
                    g.set_u32(c.eax, c.edi)?;
                    c.edi = g.u32(at(c.edx, 4))?;
                    g.set_u32(at(c.eax, 4), c.edi)?;
                    c.edx = g.u32(at(c.edx, 8))?;
                    g.set_u32(at(c.eax, 8), c.edx)?;
                    c.ecx = arithmetic(c, c.ecx, 0x28, false);
                    c.edi = g.u32(c.ecx)?;
                    c.eax = at(c.ebp, 0x374);
                    c.edx = c.eax;
                    g.set_u32(c.edx, c.edi)?;
                    c.edi = g.u32(at(c.ecx, 4))?;
                    g.set_u32(at(c.edx, 4), c.edi)?;
                    c.ecx = g.u32(at(c.ecx, 8))?;
                    g.set_u32(at(c.edx, 8), c.ecx)?;
                    c.ecx = g.u32(X4687B0)?;
                    return call(X1201A0, 0xddeda);
                }
            }
            0xddeda => {
                alu::rmw8(c, g, at(c.ebp, 0x349), |v| v | 0x10)?;
                c.edx = g.u32(c.esi)?;
                c.eax = g.u32(at(c.esi, 4))?;
                c.ecx = g.u32(at(c.esi, 8))?;
                g.set_u32(at(c.ebp, 0x368), c.edx)?;
                g.set_u32(at(c.ebp, 0x36c), c.eax)?;
                g.set_u32(at(c.ebp, 0x370), c.ecx)?;
                0xddefb
            }
            0xddefb => {
                c.edx = slot(c, g, 0x10)?;
                c.eax = g.u32(at(c.edx, 0x70))?;
                c.ebx = slot(c, g, 0x28)?;
                c.eax = arithmetic(c, c.eax, 0xc, false);
                push(c, g, c.eax)?;
                c.eax = slot(c, g, 0xd0)?;
                return call(points::BIPED_CONTACT_UPDATE.base, 0xddf16);
            }
            0xddf16 => {
                let cl = g.u8(at(c.esp, 0x20))?;
                c.ecx = byte(c.ecx, cl);
                c.eax = 1;
                if flags8(c, cl & 1) == 0 {
                    alu::rmw8(c, g, at(c.ebp, 0x348), |v| v & 0xfe)?;
                    0xddf45
                } else {
                    let w = g.u16(at(c.ebp, 0x348))? | 1;
                    g.set_u16(at(c.ebp, 0x348), w)?;
                    logic(c, w.into(), 0x8000);
                    alu::rmw8(c, g, at(c.ebp, 0x348), |v| v & 0x7f)?;
                    c.ebx = pop(c, g)?;
                    c.edi = pop(c, g)?;
                    c.esi = pop(c, g)?;
                    c.ebp = pop(c, g)?;
                    c.esp = arithmetic(c, c.esp, FRAME, false);
                    return Ok(Next::Ret { pop: 4 });
                }
            }
            0xddf45 => {
                c.ebx = pop(c, g)?;
                return post_ret(c, g);
            }
            _ => return Err(unowned(pc)),
        };
    }
}

#[cfg(test)]
mod tests;
