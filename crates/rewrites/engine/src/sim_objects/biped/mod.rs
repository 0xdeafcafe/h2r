//! The biped update (slice 7a of docs/engine/sim-rewrite-objects.md;
//! contracts in docs/engine/object-update.md, "Slice 7a bodies"): the
//! `bipd` descriptor's slot `+40` (`dd360`) and the part of its tree that
//! is ported. The rest of the tree stays guest: the seat updates `e3380`
//! and `e3700`, the movement and animation steps `e01d0`, `dc5c0`,
//! `e24f0`, `e0ef0`, `e3c90`, `e40d0`, `e4450`, and the shared callees
//! (`1e5af0`, `113e40`, `b58c0`, `30bf0`). The unit action callbacks
//! (`e6830`) and unit animation state (`111650`) run as bodies.
//!
//! Every body is a segment over the retail frame, the retail code block by
//! block, keyed by retail address: at each call and return the eight
//! integer registers, the arithmetic flags of the last instruction that
//! set them, the `xmm` registers, MXCSR and the x87 state are retail's.
//!
//! **`dd360 (object: stack 0) -> al`; `ret 4`.** Frame: `sub esp, 16c`,
//! then `ebp`, `esi`, `edi` pushed (`S0` = that `esp`), and `ebx` once the
//! biped has a physics component (`S` = `S0 - 4`, the `esp` at every site
//! after the first). With `S`: `[S+14]` the definition (then, on the seated
//! path, the tick count `fistp` stores), `[S+18]`, `[S+1c]`, `[S+20]` a
//! three-word record (two words read from the instructions at `dd3df` and
//! `dd3e7`, the second replaced on the path under a biped parent; its
//! address is passed to the movement and animation steps), `[S+24]` the
//! animation block (body `+ (i16) body[+12a]`; on the seated path a float
//! spill), `[S+28]` the header offset `(object & ffff) * c`, `[S+2c]` the
//! out buffer of `dc5c0` (`[S+7c]`, `[S+7e]` two words set to none
//! before `e01d0`), `[S+13]` `e6830`'s answer, the object at `[S+180]`.
//! Across the calls `ebp` holds the body, `edi` the object, `bl` the
//! result, `esi` the carrier's body (seated) or body `+150`, then body
//! `+3dc`.
//!
//! In order: the carrier `baf40` (fused; the first ancestor whose body
//! `+4` bit 26 is clear). Without a component (`+b4` none) it returns 0
//! at once. `e2d80` (fused) starts the result. Under a vehicle: `e3380`,
//! `e3700` outside network mode 4, and (outside mode 4, with body `+34b`
//! of the vehicle non-zero) the seat's time from definition `+1cc` (stride
//! `b0`, `+20`) through `1469f0` (fused) against `+34b`, which may ask
//! `e68c0` (fused) for action `1e`; the result is 1. Under a biped the
//! record's second word is picked by the parent's body `+10a` bit 2.
//! Unparented: body `+158` zeroed and `+150` normalised (`30bf0`), reset
//! from `[4687a8]` when its length is zero, unless body `+10a` bit 2 is
//! clear, `+3dc` is not 2 and definition `+264` bit 3 is set; the
//! animation kind (`dd300`, fused) into body `+34a`; body `+1b0` reset from
//! `[4687a4]` when its squared length is below a constant; the two
//! counters `+399`, `+39a` (up to `7f` while body `+348` bit 0 or 1 is
//! set, else cleared); `e01d0`, `dc5c0`, `1e5af0`, `e24f0`; by body
//! `+3dc` state 3 `e0ef0` and `e3c90`, state 1 the landing test `e4050`
//! (fused) then `e40d0`, or `e4450` with a word at `+34c`, or the fall
//! update `e45e0` (fused); then `e4c50` (fused). Then for every biped the
//! look update `de190` (fused); with body `+10a` bit 2 and `+c1` bit 0,
//! `113e40` and the wake for post `bba20` (or body `+348` bits 0 and 1
//! cleared); otherwise the tick into body `+bc`, `bba20`, and `b58c0` on
//! body `+d4`. Then `e6830` and `111650`. `al` is the OR of the answers.
//!
//! Instruction immediates the public source must not carry (string IDs)
//! are read from the loaded image, as are the `.rdata` constants.

mod actions;
mod leaves;
mod look;
// Slice 7b: the biped post-physics update and its tree.
mod post;
mod state;

pub(super) use actions::{ACTIONS_SITES, actions_update};
pub(super) use leaves::{airborne_test, animation_kind, carrier, seconds_to_ticks};
pub(super) use look::{LOOK_SITES, look_update};
pub(super) use post::{
    CONTACT_SITES, CROUCH_HEIGHT_SITES, CROUCH_STEP_SITES, INPUT_SITES, JUMP_SITES, POST_SITES, PROBE_SITES,
    contact_update, crouch_forced, crouch_height, crouch_step, ground_flag, ground_probe, jump_update,
    landing_update, physics_input, post_update,
};
pub(super) use state::{
    ACTION_SITES, FALL_SITES, STATE_SITES, TIMER_SITES, action, animation_state, fall_update, timer_update,
};

use super::alu;
use super::frame::{set_slot, slot};
use super::sse::{self, Ss};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, status};
use runtime::{CpuAbi, Guest};
use x87::abi as fp;

const NONE: u32 = u32::MAX;
/// `dd360`'s locals.
const FRAME: u32 = 0x16c;

/// `.rdata` constants: f32 at `45dd24` (seconds), `45dbd8`, `45e02c`.
const K_45DD24: u32 = 0x0045_dd24;
const K_45DBD8: u32 = 0x0045_dbd8;
const K_45E02C: u32 = 0x0045_e02c;
/// Hold pointers to the default vectors copied into body `+150` and
/// `+1b0`.
const VECTOR_A8: u32 = 0x0046_87a8;
const VECTOR_A4: u32 = 0x0046_87a4;
/// The `imm32`s of `mov dword [esp+1c], imm` at `dd3df` and `mov dword
/// [esp+20], imm` at `dd3e7`.
const RECORD_A: u32 = 0x000d_d3e3;
const RECORD_B: u32 = 0x000d_d3eb;
/// The `imm32`s of `and eax, imm` at `dd51f` and `add eax, imm` at `dd524`.
const PARENT_AND: u32 = 0x000d_d520;
const PARENT_ADD: u32 = 0x000d_d525;

/// Callees that stay guest.
const SEAT_UPDATE: u32 = 0x000e_3380;
const SEAT_EXIT: u32 = 0x000e_3700;
const E01D0: u32 = 0x000e_01d0;
const DC5C0: u32 = 0x000d_c5c0;
const X1E5AF0: u32 = 0x001e_5af0;
const E24F0: u32 = 0x000e_24f0;
const E0EF0: u32 = 0x000e_0ef0;
const E3C90: u32 = 0x000e_3c90;
const E40D0: u32 = 0x000e_40d0;
const E4450: u32 = 0x000e_4450;
const NORMALIZE: u32 = 0x0003_0bf0;
const X113E40: u32 = 0x0011_3e40;
const B58C0: u32 = 0x000b_58c0;
const E6830: u32 = points::UNIT_ACTIONS_UPDATE.base;
const X111650: u32 = 0x0011_1650;
/// The action `e68c0` is asked for when the seat's time has run out.
const SEAT_ACTION: u32 = 0x1e;
/// Network mode 4 (`[game globals + c]`), where the seat updates are
/// skipped.
const MODE_4: u8 = 4;

/// Every return address of `dd360`'s calls.
pub(super) const UPDATE_SITES: [u32; 26] = [
    0xdd3af, 0xdd3f4, 0xdd42a, 0xdd43d, 0xdd4e6, 0xdd4fc, 0xdd572, 0xdd5a1, 0xdd676, 0xdd683, 0xdd69b, 0xdd6ab,
    0xdd6be, 0xdd6d1, 0xdd6dc, 0xdd6eb, 0xdd702, 0xdd717, 0xdd71e, 0xdd725, 0xdd743, 0xdd74e, 0xdd771, 0xdd797,
    0xdd79d, 0xdd7ac,
];

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}

/// `call target` returning to `site`, the flags as the last instruction
/// before it left them.
fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

/// `and r32, k` / `test r32, r32`: the flags of the result.
fn and32(c: &mut CpuAbi, v: u32) -> u32 {
    logic(c, v, 1 << 31);
    v
}

/// `and r8, k` / `test r8, r8` / `or r8, r8`: the flags of a byte.
fn flags8(c: &mut CpuAbi, v: u8) -> u8 {
    logic(c, v.into(), 0x80);
    v
}

/// `cmp r8, r8` / `cmp byte ptr [m], imm`.
fn cmp8(c: &mut CpuAbi, a: u8, b: u8) {
    let v = a.wrapping_sub(b);
    let (a, b, v) = (u32::from(a), u32::from(b), u32::from(v));
    let cf = u32::from(a < b);
    let of = u32::from((a ^ b) & (a ^ v) & 0x80 != 0) << 11;
    c.eflags = (c.eflags & !ARITH) | status(v, 0x80) | cf | ((a ^ b ^ v) & 0x10) | of;
    c.eflags_defined |= ARITH;
}

/// `dec r8`: CF kept.
fn dec8(c: &mut CpuAbi, v: u8) -> u8 {
    let r = v.wrapping_sub(1);
    let af = u32::from(v & 15 == 0) << 4;
    let of = u32::from(v == 0x80) << 11;
    let keep = ARITH & !1;
    c.eflags = (c.eflags & !keep) | status(r.into(), 0x80) | af | of;
    c.eflags_defined |= keep;
    r
}

const ZF: u32 = 0x40;
const SF: u32 = 0x80;
const OF: u32 = 0x800;

/// `jl`: SF not equal to OF.
fn less(c: &CpuAbi) -> bool {
    (c.eflags & SF != 0) != (c.eflags & OF != 0)
}

/// `jg`: ZF clear and SF equal to OF.
fn greater(c: &CpuAbi) -> bool {
    c.eflags & ZF == 0 && !less(c)
}

/// `movsx r32, r/m16`.
fn sx16(v: u16) -> u32 {
    v as i16 as i32 as u32
}

/// `movsx r32, r/m8`.
fn sx8(v: u8) -> u32 {
    v as i8 as i32 as u32
}

/// `comiss a, b; jbe` taken (CF or ZF; unordered too).
fn below_or_equal(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    sse::comiss(c, a, b) & 0x41 != 0
}

/// `ucomiss a, b; lahf; test ah, 44h; jp` taken: unordered, or not equal.
/// `lahf` writes `ah`.
fn ucomiss_jp(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    sse::ucomiss(c, a, b);
    sse::lahf(c);
    logic(c, (c.eax >> 8) & 0x44, 0x80);
    sse::parity(c)
}

/// `movss dword [addr], xmm[r]`.
fn store_ss(c: &CpuAbi, g: &mut Guest, addr: u32, r: usize) -> Result<()> {
    g.set_u32(addr, sse::lane0(c, r))
}

/// `fld dword [src]; fistp dword [dst]`, the `fistp` at `fistp_pc`, under
/// the guest's x87 control word: the stack, status and tags as the two
/// instructions leave them, and the last-instruction pointers the `fistp`
/// leaves. Returns the stored integer.
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

/// The body of the object whose header offset `(handle & ffff) * c` is
/// `offset`: `[[OBJECTS] + 44 + offset + 8]`.
fn body_at(g: &Guest, data: u32, offset: u32) -> Result<u32> {
    g.u32(at(at(data, offset), 8))
}

/// One of the two counters at body `+399`, `+39a`: `cmp al, 7f; jge`, else
/// `inc al` stored. Their flags are overwritten before any transfer.
fn count_up(c: &mut CpuAbi, g: &mut Guest, addr: u32) -> Result<()> {
    let al = g.u8(addr)?;
    c.eax = byte(c.eax, al);
    cmp8(c, al, 0x7f);
    if (al as i8) < 0x7f {
        let al = al.wrapping_add(1);
        c.eax = byte(c.eax, al);
        g.set_u8(addr, al)?;
    }
    Ok(())
}

/// `dd7b5`: `pop edi, esi, ebp`, drop the frame, `ret 4`.
fn update_ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    Ok(Next::Ret { pop: 4 })
}

/// `dd360 (object: stack 0) -> al`; `ret 4`.
pub(super) fn biped_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_UPDATE.base;
    if pc != entry && !UPDATE_SITES.contains(&pc) {
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
                push(c, g, c.ebp)?;
                push(c, g, c.esi)?;
                push(c, g, c.edi)?;
                c.edi = slot(c, g, 0x17c)?;
                c.eax = and32(c, c.edi & 0xffff);
                c.eax = c.eax.wrapping_mul(3);
                c.eax = alu::shl(c, c.eax, 2, 32);
                c.ebp = g.u32(at(at(c.edx, c.eax), 8))?;
                set_slot(c, g, 0x24, c.eax)?;
                c.eax = and32(c, g.u32(c.ebp)? & 0xffff);
                c.eax = alu::shl(c, c.eax, 4, 32);
                c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
                c.eax = g.u32(at(c.ebp, 0x14))?;
                set_slot(c, g, 0x10, c.edx)?;
                return call(points::OBJECT_CARRIER.base, 0xdd3af);
            }
            0xdd3af => {
                c.ecx = g.u32(at(c.ebp, 0xb4))?;
                c.esi = c.eax;
                c.eax = sx16(g.u16(at(c.ebp, 0x12a))?);
                c.eax = arithmetic(c, c.eax, c.ebp, false);
                set_slot(c, g, 0x20, c.eax)?;
                c.eax = byte(c.eax, flags8(c, 0));
                arithmetic(c, c.ecx, NONE, true);
                if c.ecx == NONE {
                    return update_ret(c, g);
                }
                c.eax = and32(c, 0);
                set_slot(c, g, 0x14, c.eax)?;
                push(c, g, c.ebx)?;
                set_slot(c, g, 0x1c, c.eax)?;
                push(c, g, c.edi)?;
                set_slot(c, g, 0x24, c.eax)?;
                set_slot(c, g, 0x1c, g.u32(RECORD_A)?)?;
                set_slot(c, g, 0x20, g.u32(RECORD_B)?)?;
                return call(points::BIPED_TIMER_UPDATE.base, 0xdd3f4);
            }
            0xdd3f4 => {
                arithmetic(c, c.esi, NONE, true);
                c.ebx = byte(c.ebx, c.eax as u8);
                if c.esi == NONE {
                    0xdd532
                } else {
                    c.edx = g.u32(points::OBJECTS.base)?;
                    c.eax = g.u32(at(c.edx, DATA))?;
                    c.esi = and32(c, c.esi & 0xffff);
                    c.ecx = c.esi.wrapping_mul(3);
                    c.eax = c.eax.wrapping_add(c.ecx.wrapping_mul(4));
                    c.ecx = u32::from(g.u8(at(c.eax, 3))?);
                    arithmetic(c, c.ecx, 1, true);
                    if c.ecx != 1 {
                        0xdd503
                    } else {
                        c.esi = g.u32(at(c.eax, 8))?;
                        push(c, g, c.edi)?;
                        return call(SEAT_UPDATE, 0xdd42a);
                    }
                }
            }
            0xdd42a => {
                c.ecx = g.u32(points::GAME_GLOBALS.base)?;
                let mode = g.u8(at(c.ecx, 0xc))?;
                cmp8(c, mode, MODE_4);
                if mode == MODE_4 {
                    0xdd43d
                } else {
                    c.eax = c.edi;
                    return call(SEAT_EXIT, 0xdd43d);
                }
            }
            0xdd43d => {
                c.edx = g.u32(points::GAME_GLOBALS.base)?;
                let mode = g.u8(at(c.edx, 0xc))?;
                cmp8(c, mode, MODE_4);
                if mode == MODE_4 {
                    0xdd4fc
                } else {
                    let al = g.u8(at(c.esi, 0x34b))?;
                    c.eax = byte(c.eax, al);
                    if flags8(c, al) == 0 {
                        0xdd4fc
                    } else {
                        c.eax = g.u32(points::GAME_TIME.base)?;
                        c.ecx = sx16(g.u16(at(c.eax, 2))?);
                        sse::cvtsi2ss(c, 0, c.ecx);
                        sse::ss(c, Ss::Mul, 0, g.u32(K_45DD24)?);
                        store_ss(c, g, at(c.esp, 0x24), 0)?;
                        fld_fistp(c, g, at(c.esp, 0x24), at(c.esp, 0x14), 0xdd47a)?;
                        let dx = g.u16(at(c.ebp, 0x1fc))?;
                        c.edx = (c.edx & 0xffff_0000) | u32::from(dx);
                        alu::cmp16(c, dx.into(), 0xffff);
                        c.eax = slot(c, g, 0x14)?;
                        if dx == 0xffff {
                            0xdd4e6
                        } else {
                            c.ecx = g.u32(at(c.ebp, 0x14))?;
                            c.ebx = g.u32(points::OBJECTS.base)?;
                            c.ebx = g.u32(at(c.ebx, DATA))?;
                            c.ecx = and32(c, c.ecx & 0xffff);
                            c.ecx = c.ecx.wrapping_mul(3);
                            c.ecx = g.u32(at(c.ebx.wrapping_add(c.ecx.wrapping_mul(4)), 8))?;
                            c.ecx = g.u32(c.ecx)?;
                            c.ebx = g.u32(points::TAG_ENTRIES.base)?;
                            c.ecx = and32(c, c.ecx & 0xffff);
                            c.ecx = alu::shl(c, c.ecx, 4, 32);
                            c.ebx = g.u32(at(at(c.ecx, c.ebx), 8))?;
                            c.ecx = sx16(dx);
                            c.edx = g.u32(at(c.ebx, 0x1cc))?;
                            // `imul ecx, ecx, b0`: its flags are overwritten
                            // by the `add`.
                            c.ecx = c.ecx.wrapping_mul(0xb0);
                            sse::load(c, 0, g.u32(at(at(c.ecx, c.edx), 0x20))?);
                            c.ecx = arithmetic(c, c.ecx, c.edx, false);
                            let x0 = sse::lane0(c, 0);
                            if below_or_equal(c, x0, g.u32(K_45DBD8)?) {
                                0xdd4e6
                            } else {
                                c.edx = g.u32(at(c.ecx, 0x20))?;
                                push(c, g, c.edx)?;
                                return call(points::GAME_SECONDS_TO_TICKS.base, 0xdd4e6);
                            }
                        }
                    }
                }
            }
            0xdd4e6 => {
                c.ecx = u32::from(g.u8(at(c.esi, 0x34b))?);
                arithmetic(c, c.ecx, c.eax, true);
                if alu::less_or_equal(c) {
                    0xdd4fc
                } else {
                    push(c, g, c.edi)?;
                    c.ecx = SEAT_ACTION;
                    return call(points::BIPED_ACTION.base, 0xdd4fc);
                }
            }
            0xdd4fc => {
                c.ebx = byte(c.ebx, 1);
                0xdd71e
            }
            0xdd503 => {
                // Under a parent that is not a vehicle: a biped picks the
                // record's second word by its body `+10a` bit 2.
                if and32(c, c.ecx) != 0 {
                    0xdd71e
                } else {
                    c.edx = g.u32(at(c.eax, 8))?;
                    let al = g.u8(at(c.edx, 0x10a))?;
                    let al = alu::shr(c, al.into(), 2, 8) as u8;
                    let al = flags8(c, al & 1);
                    // `neg al; sbb eax, eax`: CF is `al != 0`. Both sets
                    // of flags are overwritten by the `and` and the `add`.
                    c.ebx = byte(c.ebx, 1);
                    c.eax = if al != 0 { u32::MAX } else { 0 };
                    c.eax = and32(c, c.eax & g.u32(PARENT_AND)?);
                    c.eax = arithmetic(c, c.eax, g.u32(PARENT_ADD)?, false);
                    set_slot(c, g, 0x1c, c.eax)?;
                    0xdd71e
                }
            }
            0xdd532 => {
                let cl = g.u8(at(c.ebp, 0x10a))?;
                let cl = alu::shr(c, cl.into(), 2, 8) as u8;
                c.ecx = byte(c.ecx, cl);
                if flags8(c, cl & 1) != 0 {
                    0xdd55a
                } else {
                    let state = g.u8(at(c.ebp, 0x3dc))?;
                    cmp8(c, state, 2);
                    if state == 2 {
                        0xdd595
                    } else {
                        c.edx = slot(c, g, 0x14)?;
                        c.eax = g.u32(at(c.edx, 0x264))?;
                        c.eax = alu::shr(c, c.eax, 3, 32);
                        if flags8(c, c.eax as u8 & 1) != 0 { 0xdd595 } else { 0xdd55a }
                    }
                }
            }
            0xdd55a => {
                sse::zero(c, 0);
                c.esi = at(c.ebp, 0x150);
                c.eax = c.esi;
                store_ss(c, g, at(c.ebp, 0x158), 0)?;
                return call(NORMALIZE, 0xdd572);
            }
            0xdd572 => {
                let x0 = sse::lane0(c, 0);
                if ucomiss_jp(c, x0, g.u32(K_45DBD8)?) {
                    0xdd595
                } else {
                    c.ecx = g.u32(VECTOR_A8)?;
                    c.edx = g.u32(c.ecx)?;
                    g.set_u32(c.esi, c.edx)?;
                    c.eax = g.u32(at(c.ecx, 4))?;
                    g.set_u32(at(c.esi, 4), c.eax)?;
                    c.ecx = g.u32(at(c.ecx, 8))?;
                    g.set_u32(at(c.esi, 8), c.ecx)?;
                    0xdd595
                }
            }
            0xdd595 => {
                c.edx = slot(c, g, 0x24)?;
                c.ecx = g.u32(at(c.edx, 0x7c))?;
                return call(points::BIPED_ANIMATION_KIND.base, 0xdd5a1);
            }
            0xdd5a1 => {
                g.set_u8(at(c.ebp, 0x34a), c.eax as u8)?;
                sse::load(c, 2, g.u32(at(c.ebp, 0x1b0))?);
                sse::load(c, 1, g.u32(at(c.ebp, 0x1b4))?);
                sse::load(c, 0, g.u32(at(c.ebp, 0x1b8))?);
                sse::movaps(c, 3, 2);
                sse::ss_reg(c, Ss::Mul, 3, 2);
                sse::movaps(c, 2, 1);
                sse::ss_reg(c, Ss::Mul, 2, 1);
                sse::movaps(c, 1, 0);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                sse::load(c, 0, g.u32(K_45E02C)?);
                sse::ss_reg(c, Ss::Add, 3, 2);
                sse::ss_reg(c, Ss::Add, 3, 1);
                let (x0, x3) = (sse::lane0(c, 0), sse::lane0(c, 3));
                if !below_or_equal(c, x0, x3) {
                    c.eax = g.u32(VECTOR_A4)?;
                    c.ecx = g.u32(c.eax)?;
                    g.set_u32(at(c.ebp, 0x1b0), c.ecx)?;
                    c.edx = g.u32(at(c.eax, 4))?;
                    g.set_u32(at(c.ebp, 0x1b4), c.edx)?;
                    c.eax = g.u32(at(c.eax, 8))?;
                    g.set_u32(at(c.ebp, 0x1b8), c.eax)?;
                }
                if alu::test8(c, g, at(c.ebp, 0x348), 1)? {
                    count_up(c, g, at(c.ebp, 0x399))?;
                    c.ebx = byte(c.ebx, 1);
                } else {
                    g.set_u8(at(c.ebp, 0x399), 0)?;
                }
                let cl = alu::shr(c, g.u8(at(c.ebp, 0x348))?.into(), 1, 8) as u8;
                c.ecx = byte(c.ecx, cl);
                if flags8(c, cl & 1) != 0 {
                    count_up(c, g, at(c.ebp, 0x39a))?;
                    c.ebx = byte(c.ebx, 1);
                } else {
                    g.set_u8(at(c.ebp, 0x39a), 0)?;
                }
                c.edx = at(c.esp, 0x18);
                c.eax = and32(c, u32::MAX);
                push(c, g, c.edx)?;
                push(c, g, c.edi)?;
                g.set_u16(at(c.esp, 0x84), 0xffff)?;
                g.set_u16(at(c.esp, 0x86), 0xffff)?;
                return call(E01D0, 0xdd676);
            }
            0xdd676 => {
                let bl = flags8(c, c.ebx as u8 | c.eax as u8);
                c.ebx = byte(c.ebx, bl);
                c.eax = at(c.esp, 0x2c);
                push(c, g, c.eax)?;
                push(c, g, c.edi)?;
                return call(DC5C0, 0xdd683);
            }
            0xdd683 => {
                c.edx = at(c.ebp, 0x70);
                c.esi = at(c.ebp, 0x3dc);
                push(c, g, c.edx)?;
                c.ecx = at(c.ebp, 0x7c);
                c.eax = at(c.esp, 0x30);
                c.edx = c.esi;
                return call(X1E5AF0, 0xdd69b);
            }
            0xdd69b => {
                c.eax = at(c.esp, 0x2c);
                push(c, g, c.eax)?;
                c.ecx = at(c.esp, 0x1c);
                push(c, g, c.ecx)?;
                push(c, g, c.edi)?;
                return call(E24F0, 0xdd6ab);
            }
            0xdd6ab => {
                let bl = flags8(c, c.ebx as u8 | c.eax as u8);
                c.ebx = byte(c.ebx, bl);
                c.eax = u32::from(g.u8(c.esi)?);
                c.eax = alu::dec32(c, c.eax);
                if c.eax == 0 {
                    0xdd6d5
                } else {
                    c.eax = arithmetic(c, c.eax, 2, true);
                    if c.eax != 0 {
                        0xdd719
                    } else {
                        push(c, g, c.edi)?;
                        return call(E0EF0, 0xdd6be);
                    }
                }
            }
            0xdd6be => {
                let bl = flags8(c, c.ebx as u8 | c.eax as u8);
                c.ebx = byte(c.ebx, bl);
                c.edx = at(c.esp, 0x2c);
                push(c, g, c.edx)?;
                c.eax = at(c.esp, 0x1c);
                push(c, g, c.eax)?;
                c.eax = c.edi;
                return call(E3C90, 0xdd6d1);
            }
            0xdd6d1 => {
                let bl = flags8(c, c.ebx as u8 | c.eax as u8);
                c.ebx = byte(c.ebx, bl);
                0xdd719
            }
            0xdd6d5 => {
                c.eax = c.edi;
                return call(points::BIPED_AIRBORNE_TEST.base, 0xdd6dc);
            }
            0xdd6dc => {
                if flags8(c, c.eax as u8) == 0 {
                    0xdd6ed
                } else {
                    c.ecx = at(c.esp, 0x18);
                    push(c, g, c.ecx)?;
                    push(c, g, c.edi)?;
                    return call(E40D0, 0xdd6eb);
                }
            }
            0xdd6eb | 0xdd702 => 0xdd717,
            0xdd6ed => {
                let w = g.u16(at(c.ebp, 0x34c))?;
                alu::cmp16(c, w.into(), 0xffff);
                if w == 0xffff {
                    0xdd704
                } else {
                    c.edx = at(c.esp, 0x18);
                    push(c, g, c.edx)?;
                    push(c, g, c.edi)?;
                    return call(E4450, 0xdd702);
                }
            }
            0xdd704 => {
                let al = alu::shr(c, g.u8(at(c.ebp, 0x348))?.into(), 1, 8) as u8;
                c.eax = byte(c.eax, al);
                if flags8(c, al & 1) == 0 {
                    0xdd717
                } else {
                    c.esi = c.edi;
                    return call(points::BIPED_FALL_UPDATE.base, 0xdd717);
                }
            }
            0xdd717 => {
                c.ebx = byte(c.ebx, 1);
                0xdd719
            }
            0xdd719 => return call(points::BIPED_ANIMATION_STATE_UPDATE.base, 0xdd71e),
            0xdd71e => {
                c.eax = c.edi;
                return call(points::BIPED_LOOK_UPDATE.base, 0xdd725);
            }
            0xdd725 => {
                let cl = alu::shr(c, g.u8(at(c.ebp, 0x10a))?.into(), 2, 8) as u8;
                c.ecx = byte(c.ecx, cl);
                if flags8(c, cl & 1) != 0 && alu::test8(c, g, at(c.ebp, 0xc1), 1)? {
                    c.eax = c.edi;
                    return call(X113E40, 0xdd743);
                }
                0xdd75b
            }
            0xdd743 => {
                if flags8(c, c.eax as u8) == 0 {
                    alu::rmw8(c, g, at(c.ebp, 0x348), |v| v & 0xfc)?;
                    0xdd797
                } else {
                    c.eax = c.edi;
                    return call(points::OBJECT_WAKE_FOR_POST.base, 0xdd74e);
                }
            }
            0xdd74e => {
                c.ebx = byte(c.ebx, 1);
                0xdd797
            }
            0xdd75b => {
                c.edx = g.u32(points::GAME_TIME.base)?;
                c.eax = g.u32(at(c.edx, 8))?;
                g.set_u32(at(c.ebp, 0xbc), c.eax)?;
                c.eax = c.edi;
                return call(points::OBJECT_WAKE_FOR_POST.base, 0xdd771);
            }
            0xdd771 => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                c.eax = slot(c, g, 0x28)?;
                c.eax = body_at(g, c.edx, c.eax)?;
                c.eax = g.u32(at(c.eax, 0xd4))?;
                arithmetic(c, c.eax, NONE, true);
                c.ebx = byte(c.ebx, 1);
                if c.eax == NONE {
                    0xdd797
                } else {
                    push(c, g, 4)?;
                    push(c, g, c.eax)?;
                    return call(B58C0, 0xdd797);
                }
            }
            0xdd797 => {
                push(c, g, c.edi)?;
                return call(E6830, 0xdd79d);
            }
            0xdd79d => {
                c.ecx = at(c.esp, 0x18);
                push(c, g, c.ecx)?;
                push(c, g, c.edi)?;
                g.set_u8(at(c.esp, 0x1b), c.eax as u8)?;
                return call(X111650, 0xdd7ac);
            }
            0xdd7ac => {
                let al = flags8(c, c.eax as u8 | g.u8(at(c.esp, 0x13))?);
                c.eax = byte(c.eax, al);
                let bl = flags8(c, c.ebx as u8 | al);
                c.ebx = byte(c.ebx, bl);
                c.eax = byte(c.eax, bl);
                c.ebx = pop(c, g)?;
                return update_ret(c, g);
            }
            _ => return Err(unowned(pc)),
        };
    }
}

#[cfg(test)]
mod tests;
