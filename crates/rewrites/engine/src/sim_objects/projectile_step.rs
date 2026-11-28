//! The projectile step `f8eb0` (slice 11 of docs/engine/sim-rewrite-objects.md;
//! contract in docs/engine/object-update.md, "Slice 11 bodies"), called by
//! the projectile update `f8de0` once a tick. Read at its address with
//! `h2 disasm`; the arms below are retail's basic blocks, keyed by
//! address, one statement per instruction in retail order, so the lifted
//! original can take over at any of the 39 return sites and the reverse.
//!
//! **`f8eb0 (object: stack 0, velocity: stack 1)`, `ret 8`.** Frame: `sub
//! esp, 118`, then `push ebx, ebp, esi, edi`. With `S` that `esp` (every
//! callee pops its own arguments, so `S` holds at every site): the object
//! at `[S+12c]` and the velocity argument at `[S+130]` (passed on to the
//! hit `faa60`); the working velocity at `[S+10..18]`; the "moved" byte at
//! `[S+1e]` and the "flyby heard" byte at `[S+1f]`; the time left in the
//! tick at `[S+20]` (the tick length from `[510c54]+4` at entry); the up
//! and forward bases at `[S+24..2c]`, `[S+30..38]`; the definition at
//! `[S+3c]`; the attachment handle, then the turn rate, at `[S+40]`; the
//! sub-step count (a word, at most 10) at `[S+44]`; the half step at
//! `[S+48..50]` and the new position at `[S+54..5c]`; the header offset
//! (index × `c`) at `[S+7c]`; the speed at `[S+80]` and `[S+ac]`; the time
//! left at the sub-step's start at `[S+a8]`; the target's point (`f87f0`
//! fills it) at `[S+c0..c8]`; the collision record `fc030` fills at
//! `[S+cc]` (its fraction at `[S+d0]`).
//!
//! Registers across calls: `ebp` the body throughout; `edi` the object
//! until the sub-step loop, then none (`or edi, -1`), the definition's
//! `+bc` flags, the flyby cursor's buffer or 0 as each call needs; `ebx`
//! the definition (reloaded from `[S+3c]`), the object around the
//! collision and hit calls; `esi` body `+88` inside a sub-step, the
//! object from `f9ef3`, the attachment handle at `17bbd0`.
//!
//! What it does, in retail order: the attachment (`fd410`, `17c880`,
//! `16ba40`, `fd460`) unless flags bit 1; the age and arming timers
//! (body `+158..+164`, `+1aa`); then, while time is left, one sub-step:
//! the speed (x87 `fsqrt` of the velocity's squared length), homing toward
//! the target `+144` (`badc0`, `1e9700`, `f87f0`, the turn axis through
//! `30bf0` and an x87 `fsin`/`fcos` rotation, `b75a0`), drag and the
//! acceleration (`24590` returns the length in `st(0)`, `faa30` raises the
//! state at `+130`), gravity (`[51e9c4]`), the arming distance, the
//! collision `fc030` and on a hit `1ca9f0`, `faa60`, `1ca690` (at most 10
//! hits a tick; the tenth calls `faa30(object, 1)`), the flyby test per
//! local player (`1897c0`), the orientation from the velocity (`30bf0`,
//! `11d000`) or from the spin at `+174..+184`, and the new transform and
//! velocity (`b75a0`, `b7740`, `1c4b00`; `b9b90`, `b7360`, `bba20` when it
//! moves); the attachment's update (`17bbd0`). After the loop, by the
//! state at `+130`: detonation (`fc330`) or deletion (`OBJECT_DELETE`
//! `b8540`, a callout in retail order), or in network mode 4 the client's
//! attachment refresh (`b9d20`, `bef30`, `b8b70`).
//!
//! Floating point: scalar SSE through [`sse`] on the guest's `xmm`
//! registers and MXCSR; the x87 sequences (`f909a..f911d`, `f9299..f92fb`,
//! `f9675..f9689`, `f9a18..f9a24`) through `x87::abi` on the guest's x87
//! state, each instruction leaving FIP, FOP and FDP as it does, so every
//! callout sees retail's `xmm0..7`, x87 stack, tags, status and
//! last-instruction pointers. `st(0)` is live from `24590`'s return into
//! the `fadd` after it; `xmm1` is live across `1e9700` and `xmm0` holds
//! `faa30`'s argument.
//!
//! Flags: every instruction that sets them is reproduced, so each
//! transfer carries the flags of the last one before it (or the callee's,
//! where none follows the call).

use super::sse::{self, Ss::{Add, Div, Mul, Sub}};
use super::{Cx, Next, Result, alu, unowned};
use guest_layout::points;
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, status};
use runtime::{CpuAbi, Guest};
use x87::abi::{self as fp, arith_op, src_kind};

const ENTRY: u32 = points::PROJECTILE_STEP.base;
const NONE: u32 = u32::MAX;
const SIGN32: u32 = 1 << 31;

/// Binary32 constants in the image, read where retail reads them (names
/// ours, L): one, zero, the small threshold the moved test uses, the half
/// that averages two velocities, and the limit a hit's surface value is
/// compared with.
const ONE: u32 = 0x0045_dbc0;
const ZERO: u32 = 0x0045_dbd8;
const EPSILON: u32 = 0x0045_dbdc;
const HALF: u32 = 0x0045_dbbc;
const SURFACE_LIMIT: u32 = 0x0044_ae90;
/// The physics constants (gravity at `+0`; L) and the owner `16ba40` is
/// passed in `edx` (L).
const GRAVITY_OWNER: u32 = 0x0051_e9c4;
const EFFECT_OWNER: u32 = 0x004e_a944;

// The callees (names ours, L), each a callout with its `ret`: see
// docs/engine/object-update.md, "Slice 11 bodies".
const ATTACHMENT: u32 = 0x000f_d410;
const ATTACHMENT_STOP: u32 = 0x0017_c880;
const ATTACHMENT_FREE: u32 = 0x0016_ba40;
const ATTACHMENT_SET: u32 = 0x000f_d460;
const ATTACHMENT_UPDATE: u32 = 0x0017_bbd0;
const OBJECT_LOOKUP: u32 = 0x000b_adc0;
const ENGINE_SCALE: u32 = 0x001e_9700;
const TARGET_POINT: u32 = 0x000f_87f0;
const NORMALIZE: u32 = 0x0003_0bf0;
const OBJECT_SET_TRANSFORM: u32 = 0x000b_75a0;
const RAISE_STATE: u32 = 0x000f_aa30;
const COLLIDE: u32 = 0x000f_c030;
const HIT_SURFACE: u32 = 0x001c_a9f0;
const HIT: u32 = 0x000f_aa60;
const HIT_EFFECT: u32 = 0x001c_a690;
const FLYBY: u32 = 0x0018_97c0;
const PERPENDICULAR: u32 = 0x0011_d000;
const OBJECT_SET_VELOCITY: u32 = 0x000b_7740;
const COMPONENT_SYNC: u32 = 0x001c_4b00;
const OBJECT_MOVED: u32 = 0x000b_9b90;
const NEEDS_REFRESH: u32 = 0x000b_9d20;
const ATTACHMENT_REFRESH: u32 = 0x000b_ef30;
const REFRESH_DONE: u32 = 0x000b_8b70;
const DETONATE: u32 = 0x000f_c330;

/// The retail return addresses of `f8eb0`'s 39 calls, in address order.
pub(super) const STEP_SITES: [u32; 39] = [
    0xf8f29, 0xf8f37, 0xf8f46, 0xf8f4f, 0xf9130, 0xf918d, 0xf91ab, 0xf928c, 0xf9417, 0xf94b9,
    0xf94d7, 0xf966a, 0xf96c5, 0xf97b9, 0xf97cf, 0xf984d, 0xf98fc, 0xf9932, 0xf9952, 0xf997a,
    0xf9a18, 0xf9a6a, 0xf9adf, 0xf9b85, 0xf9b9f, 0xf9ba4, 0xf9f16, 0xf9f25, 0xf9f33, 0xf9f73,
    0xf9f7a, 0xf9f81, 0xf9f9f, 0xf9fc0, 0xfa02d, 0xfa03f, 0xfa048, 0xfa0e2, 0xfa0ed,
];

fn zf(c: &CpuAbi) -> bool {
    c.eflags & 0x40 != 0
}

fn cf(c: &CpuAbi) -> bool {
    c.eflags & 0x01 != 0
}

fn pf(c: &CpuAbi) -> bool {
    c.eflags & 0x04 != 0
}

fn sf(c: &CpuAbi) -> bool {
    c.eflags & 0x80 != 0
}

fn of(c: &CpuAbi) -> bool {
    c.eflags & 0x800 != 0
}

/// `inc` on a `bits`-wide operand: CF kept, the rest from the result.
fn inc(c: &mut CpuAbi, v: u32, bits: u32) -> u32 {
    let mask = if bits >= 32 { u32::MAX } else { (1 << bits) - 1 };
    let sign = 1u32 << (bits - 1);
    let v = v & mask;
    let r = v.wrapping_add(1) & mask;
    let af = u32::from(v & 15 == 15) << 4;
    let of = u32::from(r == sign) << 11;
    let keep = ARITH & !1;
    c.eflags = (c.eflags & !keep) | status(r, sign) | af | of;
    c.eflags_defined |= keep;
    r
}

/// `cmp a8, b8`.
fn cmp8(c: &mut CpuAbi, a: u32, b: u32) {
    let (a, b) = (a & 0xff, b & 0xff);
    let v = a.wrapping_sub(b) & 0xff;
    let cf = u32::from(a < b);
    let of = u32::from((a ^ b) & (a ^ v) & 0x80 != 0) << 11;
    c.eflags = (c.eflags & !ARITH) | status(v, 0x80) | cf | ((a ^ b ^ v) & 0x10) | of;
    c.eflags_defined |= ARITH;
}

/// Run one x87 instruction on the guest's x87 state. The bodies run only
/// where every x87 exception is masked (`super::supported`), so the
/// helper's status is always `OK` and is not looked at.
fn fpu<R>(c: &mut CpuAbi, f: impl FnOnce(&mut fp::X87Abi) -> R) -> R {
    let s = &c.x87;
    let mut x = fp::X87Abi {
        st: s.st,
        control: s.control,
        status: s.status,
        tag: s.tag,
        opcode: s.opcode,
        instruction_pointer: s.instruction_pointer,
        data_pointer: s.data_pointer,
        instruction_selector: s.instruction_selector,
        data_selector: s.data_selector,
    };
    let r = f(&mut x);
    c.x87.st = x.st;
    c.x87.control = x.control;
    c.x87.status = x.status;
    c.x87.tag = x.tag;
    r
}

/// The last-instruction pointers as the x87 instruction at `pc` leaves
/// them: FIP, FCS, FOP from its mapped bytes, and for a memory operand FDP
/// and FDS (the IR's `Mark`).
fn mark(c: &mut CpuAbi, g: &Guest, pc: u32, data: Option<u32>) -> Result<()> {
    c.x87.instruction_pointer = pc;
    c.x87.instruction_selector = 0x8;
    c.x87.opcode = g.u16(pc)?.swap_bytes() & 0x7ff;
    if let Some(a) = data {
        c.x87.data_pointer = a;
        c.x87.data_selector = 0x10;
    }
    Ok(())
}

/// `fst`/`fstp dword ptr [addr]` at `pc`.
pub(crate) fn store_f32(c: &mut CpuAbi, g: &mut Guest, pc: u32, addr: u32, pop: bool) -> Result<()> {
    let (_, s) = fpu(c, |x| fp::st_f32(x, 0));
    g.set_u32(addr, s.value)?;
    fpu(c, |x| fp::store_commit(x, s.status, pop));
    mark(c, g, pc, Some(addr))
}

/// `f8eb0 (object: stack 0, velocity: stack 1)`; `ret 8`.
pub(super) fn projectile_step(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != ENTRY && !STEP_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let (mut next, mut blocks) = (pc, 0);
    loop {
        super::progress(&mut blocks, ENTRY, next);
        next = match next {
            ENTRY => {
                let v = c.esp;
                c.esp = arithmetic(c, v, 0x118, true);
                let v = c.ebx;
                push(c, g, v)?;
                let v = c.ebp;
                push(c, g, v)?;
                let v = c.esi;
                push(c, g, v)?;
                let v = c.edi;
                push(c, g, v)?;
                c.edi = g.u32(c.esp.wrapping_add(0x12c))?;
                c.eax = c.edi;
                let v = c.eax & 0xffff;
                c.eax = v;
                logic(c, v, SIGN32);
                c.esi = c.eax.wrapping_add(c.eax.wrapping_mul(2));
                c.eax = g.u32(points::OBJECTS.base)?;
                c.ecx = g.u32(c.eax.wrapping_add(0x44))?;
                c.eax = g.u32(points::TAG_ENTRIES.base)?;
                let (v, n) = (c.esi, 0x2);
                let v = alu::shl(c, v, n, 32);
                c.esi = v;
                c.ebp = g.u32(c.esi.wrapping_add(c.ecx).wrapping_add(0x8))?;
                c.edx = g.u32(c.ebp)?;
                c.ecx = g.u32(points::GAME_TIME.base)?;
                let b = g.u32(c.ecx.wrapping_add(0x4))?;
                sse::load(c, 1, b);
                let v = c.edx & 0xffff;
                c.edx = v;
                logic(c, v, SIGN32);
                let (v, n) = (c.edx, 0x4);
                let v = alu::shl(c, v, n, 32);
                c.edx = v;
                c.ebx = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(0x8))?;
                c.edx = g.u32(c.ebp.wrapping_add(0x12c))?;
                let (v, n) = (c.edx, 0x1);
                let v = alu::shr(c, v, n, 32);
                c.edx = v;
                let v = (c.edx & 0xff) & 0x1;
                logic(c, v, 0x80);
                g.set_u32(c.esp.wrapping_add(0x7c), c.esi)?;
                g.set_u32(c.esp.wrapping_add(0x3c), c.ebx)?;
                g.set_u32(c.esp.wrapping_add(0x20), sse::lane0(c, 1))?;
                g.set_u8(c.esp.wrapping_add(0x1f), 0)?;
                g.set_u32(c.esp.wrapping_add(0x44), 0x0)?;
                if !zf(c) { 0xf8f59 } else { 0xf8f22 }
            }
            0xf8f22 => {
                c.eax = c.edi;
                return Ok(Next::Call { target: ATTACHMENT, site: 0xf8f29 });
            }
            0xf8f29 => {
                let (a, b) = (c.eax, NONE);
                arithmetic(c, a, b, true);
                g.set_u32(c.esp.wrapping_add(0x40), c.eax)?;
                if zf(c) { 0xf8f46 } else { 0xf8f32 }
            }
            0xf8f32 => {
                return Ok(Next::Call { target: ATTACHMENT_STOP, site: 0xf8f37 });
            }
            0xf8f37 => {
                c.eax = g.u32(c.esp.wrapping_add(0x40))?;
                c.edx = g.u32(EFFECT_OWNER)?;
                return Ok(Next::Call { target: ATTACHMENT_FREE, site: 0xf8f46 });
            }
            0xf8f46 => {
                push(c, g, NONE)?;
                c.eax = c.edi;
                return Ok(Next::Call { target: ATTACHMENT_SET, site: 0xf8f4f });
            }
            0xf8f4f => {
                g.set_u32(c.ebp.wrapping_add(0x14c), NONE)?;
                0xf8f59
            }
            0xf8f59 => {
                let b = g.u32(c.ebp.wrapping_add(0x160))?;
                sse::load(c, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x164))?;
                sse::ss(c, Add, 0, b);
                let v = u32::from(g.u16(c.ebp.wrapping_add(0x1aa))?);
                let v = inc(c, v, 16);
                g.set_u16(c.ebp.wrapping_add(0x1aa), v as u16)?;
                g.set_u32(c.ebp.wrapping_add(0x160), sse::lane0(c, 0))?;
                c.eax = u32::from(g.u16(c.ebx.wrapping_add(0xc0))?) as u16 as i16 as i32 as u32;
                let v = c.eax;
                c.eax = alu::dec32(c, v);
                if zf(c) { 0xf8fa2 } else { 0xf8f82 }
            }
            0xf8f82 => {
                let v = c.eax;
                c.eax = alu::dec32(c, v);
                if zf(c) { 0xf8f97 } else { 0xf8f85 }
            }
            0xf8f85 => {
                let v = c.eax;
                c.eax = alu::dec32(c, v);
                if zf(c) { 0xf8f8c } else { 0xf8f88 }
            }
            0xf8f88 => {
                c.eax = byte(c.eax, 1);
                0xf8fad
            }
            0xf8f8c => {
                c.eax = g.u32(c.ebp.wrapping_add(0x12c))?;
                let (v, n) = (c.eax, 0xb);
                let v = alu::shr(c, v, n, 32);
                c.eax = v;
                0xf8fab
            }
            0xf8f97 => {
                c.eax = g.u32(c.ebp.wrapping_add(0x12c))?;
                let (v, n) = (c.eax, 0x4);
                let v = alu::shr(c, v, n, 32);
                c.eax = v;
                0xf8fab
            }
            0xf8fa2 => {
                c.eax = g.u32(c.ebp.wrapping_add(0x12c))?;
                let (v, n) = (c.eax, 0x2);
                let v = alu::shr(c, v, n, 32);
                c.eax = v;
                0xf8fab
            }
            0xf8fab => {
                let v = c.eax & 1;
                c.eax = byte(c.eax, v as u8);
                logic(c, v, 0x80);
                0xf8fad
            }
            0xf8fad => {
                c.edi = g.u32(c.ebp.wrapping_add(0x12c))?;
                c.ecx = c.edi;
                let (v, n) = (c.ecx, 0x5);
                let v = alu::shr(c, v, n, 32);
                c.ecx = v;
                let v = c.ecx & 1;
                c.ecx = byte(c.ecx, v as u8);
                logic(c, v, 0x80);
                if !zf(c) { 0xf8fcb } else { 0xf8fbd }
            }
            0xf8fbd => {
                c.edx = c.edi;
                let (v, n) = (c.edx, 0x3);
                let v = alu::shr(c, v, n, 32);
                c.edx = v;
                let v = (c.edx & 0xff) & 0x1;
                logic(c, v, 0x80);
                if !zf(c) { 0xf8fcb } else { 0xf8fc7 }
            }
            0xf8fc7 => {
                let v = c.eax & 0xff;
                logic(c, v, 0x80);
                if zf(c) { 0xf9018 } else { 0xf8fcb }
            }
            0xf8fcb => {
                let v = c.ecx & 0xff;
                logic(c, v, 0x80);
                if !zf(c) { 0xf8fd6 } else { 0xf8fcf }
            }
            0xf8fcf => {
                let v = g.u32(c.ebp.wrapping_add(0x12c))? | 0x20;
                g.set_u32(c.ebp.wrapping_add(0x12c), v)?;
                logic(c, v, SIGN32);
                0xf8fd6
            }
            0xf8fd6 => {
                let b = g.u32(c.ebp.wrapping_add(0x15c))?;
                sse::load(c, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x158))?;
                sse::ss(c, Add, 0, b);
                let b = g.u32(ONE)?;
                sse::comiss(c, sse::lane0(c, 0), b);
                g.set_u32(c.ebp.wrapping_add(0x158), sse::lane0(c, 0))?;
                if cf(c) { 0xf9018 } else { 0xf8ff7 }
            }
            0xf8ff7 => {
                c.eax = g.u32(points::OBJECTS.base)?;
                c.ecx = g.u32(c.eax.wrapping_add(0x44))?;
                c.esi = g.u32(c.esi.wrapping_add(c.ecx).wrapping_add(0x8))?;
                c.eax = 0x1;
                let (a, b) = (u32::from(g.u16(c.esi.wrapping_add(0x130))?), c.eax & 0xffff);
                alu::cmp16(c, a, b);
                if sf(c) == of(c) { 0xf9018 } else { 0xf9011 }
            }
            0xf9011 => {
                g.set_u16(c.esi.wrapping_add(0x130), c.eax as u16)?;
                0xf9018
            }
            0xf9018 => {
                sse::zero(c, 3);
                let b = sse::lane0(c, 3);
                sse::comiss(c, sse::lane0(c, 1), b);
                if cf(c) || zf(c) { 0xf9fd2 } else { 0xf9024 }
            }
            0xf9024 => {
                c.eax = (c.eax & !0xffff) | u32::from(g.u16(c.ebp.wrapping_add(0x130))?);
                let v = c.eax & 0xffff;
                logic(c, v, 0x8000);
                if zf(c) { 0xf9064 } else { 0xf9030 }
            }
            0xf9030 => {
                let (a, b) = (c.eax & 0xffff, 0x1);
                alu::cmp16(c, a, b);
                if !zf(c) { 0xf9fd2 } else { 0xf903a }
            }
            0xf903a => {
                let b = g.u32(c.ebp.wrapping_add(0x164))?;
                sse::load(c, 0, b);
                let b = sse::lane0(c, 3);
                sse::ucomiss(c, sse::lane0(c, 0), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                if !pf(c) { 0xf9fd2 } else { 0xf904f }
            }
            0xf904f => {
                let b = g.u32(ONE)?;
                sse::load(c, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x160))?;
                sse::comiss(c, sse::lane0(c, 0), b);
                if cf(c) || zf(c) { 0xf9fd2 } else { 0xf9064 }
            }
            0xf9064 => {
                c.eax = g.u32(c.ebp.wrapping_add(0x12c))?;
                c.edx = c.eax;
                let (v, n) = (c.edx, 0x3);
                let v = alu::shr(c, v, n, 32);
                c.edx = v;
                let v = (c.edx & 0xff) & 0x1;
                logic(c, v, 0x80);
                if !zf(c) { 0xf9fd2 } else { 0xf9078 }
            }
            0xf9078 => {
                let v = u32::from(g.u8(c.ebp.wrapping_add(0xc1))?) & 0x1;
                logic(c, v, 0x80);
                if !zf(c) { 0xf9fd2 } else { 0xf9085 }
            }
            0xf9085 => {
                let (v, n) = (c.eax, 0xe);
                let v = alu::shr(c, v, n, 32);
                c.eax = v;
                let v = (c.eax & 0xff) & 0x1;
                logic(c, v, 0x80);
                if !zf(c) { 0xf9fd2 } else { 0xf9090 }
            }
            0xf9090 => {
                let (a, b) = (g.u32(c.ebp.wrapping_add(0x14))?, NONE);
                arithmetic(c, a, b, true);
                if !zf(c) { 0xf9fd2 } else { 0xf909a }
            }
            0xf909a => {
                let (a, v) = (c.ebp.wrapping_add(0x90), g.u32(c.ebp.wrapping_add(0x90))?);
                fpu(c, |x| fp::ld_f32(x, v));
                mark(c, g, 0xf909a, Some(a))?;
                c.esi = c.ebp.wrapping_add(0x88);
                let (a, v) = (c.esi.wrapping_add(0x4), g.u32(c.esi.wrapping_add(0x4))?);
                fpu(c, |x| fp::ld_f32(x, v));
                mark(c, g, 0xf90a6, Some(a))?;
                let b = g.u32(c.esp.wrapping_add(0x20))?;
                sse::load(c, 0, b);
                let (a, v) = (c.esi, g.u32(c.esi)?);
                fpu(c, |x| fp::ld_f32(x, v));
                mark(c, g, 0xf90af, Some(a))?;
                c.eax = c.esi;
                fpu(c, |x| fp::ld_st(x, 0));
                mark(c, g, 0xf90b3, None)?;
                c.ecx = g.u32(c.eax)?;
                fpu(c, |x| fp::arith(x, arith_op::MUL, 0, src_kind::ST, 1));
                mark(c, g, 0xf90b7, None)?;
                c.edx = g.u32(c.eax.wrapping_add(0x4))?;
                fpu(c, |x| fp::ld_st(x, 2));
                mark(c, g, 0xf90bc, None)?;
                c.eax = g.u32(c.eax.wrapping_add(0x8))?;
                fpu(c, |x| fp::arith(x, arith_op::MUL, 0, src_kind::ST, 3));
                mark(c, g, 0xf90c1, None)?;
                g.set_u32(c.esp.wrapping_add(0x10), c.ecx)?;
                c.ecx = c.esi;
                g.set_u32(c.esp.wrapping_add(0x14), c.edx)?;
                fpu(c, |x| fp::arith(x, arith_op::ADD | fp::ARITH_POP, 1, src_kind::ST, 0));
                mark(c, g, 0xf90cd, None)?;
                c.edx = g.u32(c.ecx)?;
                fpu(c, |x| fp::ld_st(x, 3));
                mark(c, g, 0xf90d1, None)?;
                g.set_u32(c.esp.wrapping_add(0x18), c.eax)?;
                fpu(c, |x| fp::arith(x, arith_op::MUL, 0, src_kind::ST, 4));
                mark(c, g, 0xf90d7, None)?;
                c.eax = g.u32(c.ecx.wrapping_add(0x4))?;
                c.ecx = g.u32(c.ecx.wrapping_add(0x8))?;
                let v = c.edi | NONE;
                c.edi = v;
                logic(c, v, SIGN32);
                fpu(c, |x| fp::arith(x, arith_op::ADD | fp::ARITH_POP, 1, src_kind::ST, 0));
                mark(c, g, 0xf90e2, None)?;
                g.set_u32(c.esp.wrapping_add(0x4c), c.eax)?;
                c.eax = g.u32(c.ebp.wrapping_add(0x144))?;
                let (a, b) = (c.eax, c.edi);
                arithmetic(c, a, b, true);
                fpu(c, |x| fp::unary(x, fp::unary_op::SQRT));
                mark(c, g, 0xf90f0, None)?;
                g.set_u32(c.esp.wrapping_add(0xa8), sse::lane0(c, 0))?;
                g.set_u16(c.esp.wrapping_add(0xf0), c.edi as u16)?;
                g.set_u8(c.esp.wrapping_add(0x1e), 0)?;
                g.set_u32(c.esp.wrapping_add(0x48), c.edx)?;
                g.set_u32(c.esp.wrapping_add(0x50), c.ecx)?;
                fpu(c, |x| fp::st_st(x, 3, true));
                mark(c, g, 0xf9110, None)?;
                fpu(c, |x| fp::st_st(x, 0, true));
                mark(c, g, 0xf9112, None)?;
                fpu(c, |x| fp::st_st(x, 0, true));
                mark(c, g, 0xf9114, None)?;
                let a = c.esp.wrapping_add(0xac);
                store_f32(c, g, 0xf9116, a, false)?;
                let a = c.esp.wrapping_add(0x80);
                store_f32(c, g, 0xf911d, a, true)?;
                if zf(c) { 0xf94e1 } else { 0xf912a }
            }
            0xf912a => {
                let v = c.edi;
                push(c, g, v)?;
                return Ok(Next::Call { target: OBJECT_LOOKUP, site: 0xf9130 });
            }
            0xf9130 => {
                let v = c.eax;
                logic(c, v, SIGN32);
                if zf(c) { 0xf94e1 } else { 0xf9138 }
            }
            0xf9138 => {
                let b = g.u32(c.ebp.wrapping_add(0x18c))?;
                sse::load(c, 0, b);
                let b = sse::lane0(c, 3);
                sse::comiss(c, sse::lane0(c, 0), b);
                if cf(c) || zf(c) { 0xf94e1 } else { 0xf9149 }
            }
            0xf9149 => {
                c.eax = g.u32(c.ebp.wrapping_add(0x144))?;
                let v = c.eax & 0xffff;
                c.eax = v;
                logic(c, v, SIGN32);
                c.edx = c.eax.wrapping_add(c.eax.wrapping_mul(2));
                c.eax = g.u32(points::OBJECTS.base)?;
                c.ecx = g.u32(c.eax.wrapping_add(0x44))?;
                c.eax = g.u32(c.ecx.wrapping_add(c.edx.wrapping_mul(4)).wrapping_add(0x8))?;
                c.ecx = byte(c.ecx, g.u8(c.eax.wrapping_add(0xaa))?);
                c.edx = 0x1;
                let (v, n) = (c.edx, c.ecx & 0xff);
                let v = alu::shl(c, v, n, 32);
                c.edx = v;
                sse::movaps(c, 1, 0);
                g.set_u32(c.esp.wrapping_add(0x40), sse::lane0(c, 1))?;
                let v = (c.edx & 0xff) & 0x3;
                logic(c, v, 0x80);
                if zf(c) { 0xf9197 } else { 0xf917e }
            }
            0xf917e => {
                let (a, b) = (g.u32(c.eax.wrapping_add(0x13c))?, c.edi);
                arithmetic(c, a, b, true);
                if zf(c) { 0xf9197 } else { 0xf9186 }
            }
            0xf9186 => {
                push(c, g, 0x13)?;
                return Ok(Next::Call { target: ENGINE_SCALE, site: 0xf918d });
            }
            0xf918d => {
                sse::ss_reg(c, Mul, 0, 1);
                g.set_u32(c.esp.wrapping_add(0x40), sse::lane0(c, 0))?;
                0xf9197
            }
            0xf9197 => {
                c.eax = g.u32(c.esp.wrapping_add(0x12c))?;
                let v = c.eax;
                push(c, g, v)?;
                c.ebx = c.esp.wrapping_add(0xc4);
                return Ok(Next::Call { target: TARGET_POINT, site: 0xf91ab });
            }
            0xf91ab => {
                let b = g.u32(c.esp.wrapping_add(0xc4))?;
                sse::load(c, 1, b);
                let b = g.u32(c.ebp.wrapping_add(0x68))?;
                sse::ss(c, Sub, 1, b);
                let b = g.u32(c.esp.wrapping_add(0xc8))?;
                sse::load(c, 2, b);
                let b = g.u32(c.ebp.wrapping_add(0x6c))?;
                sse::ss(c, Sub, 2, b);
                let b = g.u32(c.esi.wrapping_add(0x4))?;
                sse::load(c, 3, b);
                let b = g.u32(c.esi.wrapping_add(0x8))?;
                sse::load(c, 4, b);
                c.ebx = g.u32(c.esp.wrapping_add(0x3c))?;
                let b = g.u32(c.esp.wrapping_add(0xc0))?;
                sse::load(c, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x64))?;
                sse::ss(c, Sub, 0, b);
                c.edi = g.u32(c.ebx.wrapping_add(0xbc))?;
                sse::ss_reg(c, Mul, 3, 2);
                sse::ss_reg(c, Mul, 4, 1);
                sse::ss_reg(c, Sub, 3, 4);
                g.set_u32(c.esp.wrapping_add(0x70), sse::lane0(c, 3))?;
                let b = g.u32(c.esi.wrapping_add(0x8))?;
                sse::load(c, 3, b);
                sse::ss_reg(c, Mul, 3, 0);
                sse::movaps(c, 4, 2);
                let b = g.u32(c.esi)?;
                sse::ss(c, Mul, 4, b);
                sse::ss_reg(c, Sub, 3, 4);
                let b = g.u32(c.esi.wrapping_add(0x4))?;
                sse::load(c, 4, b);
                g.set_u32(c.esp.wrapping_add(0x74), sse::lane0(c, 3))?;
                c.ecx = c.edi;
                sse::movaps(c, 3, 1);
                let b = g.u32(c.esi)?;
                sse::ss(c, Mul, 3, b);
                let (v, n) = (c.ecx, 0x8);
                let v = alu::shr(c, v, n, 32);
                c.ecx = v;
                let v = (c.ecx & 0xff) & 0x1;
                logic(c, v, 0x80);
                sse::ss_reg(c, Mul, 4, 0);
                sse::ss_reg(c, Sub, 3, 4);
                g.set_u32(c.esp.wrapping_add(0x90), sse::lane0(c, 0))?;
                g.set_u32(c.esp.wrapping_add(0x94), sse::lane0(c, 1))?;
                g.set_u32(c.esp.wrapping_add(0x98), sse::lane0(c, 2))?;
                g.set_u32(c.esp.wrapping_add(0x78), sse::lane0(c, 3))?;
                if !zf(c) { 0xf9283 } else { 0xf9254 }
            }
            0xf9254 => {
                let b = g.u32(c.esi.wrapping_add(0x8))?;
                sse::load(c, 3, b);
                sse::ss_reg(c, Mul, 3, 2);
                let b = g.u32(c.esi.wrapping_add(0x4))?;
                sse::load(c, 2, b);
                sse::ss_reg(c, Mul, 2, 1);
                let b = g.u32(c.esi)?;
                sse::load(c, 1, b);
                sse::ss_reg(c, Add, 3, 2);
                sse::ss_reg(c, Mul, 1, 0);
                sse::ss_reg(c, Add, 3, 1);
                let b = g.u32(ZERO)?;
                sse::comiss(c, sse::lane0(c, 3), b);
                if cf(c) || zf(c) { 0xf94d7 } else { 0xf9283 }
            }
            0xf9283 => {
                c.eax = c.esp.wrapping_add(0x70);
                return Ok(Next::Call { target: NORMALIZE, site: 0xf928c });
            }
            0xf928c => {
                let b = g.u32(ZERO)?;
                sse::comiss(c, sse::lane0(c, 0), b);
                if cf(c) || zf(c) { 0xf94d7 } else { 0xf9299 }
            }
            0xf9299 => {
                let (a, v) = (c.esp.wrapping_add(0x40), g.u32(c.esp.wrapping_add(0x40))?);
                fpu(c, |x| fp::ld_f32(x, v));
                mark(c, g, 0xf9299, Some(a))?;
                let b = g.u32(c.esp.wrapping_add(0x70))?;
                sse::load(c, 5, b);
                let (a, v) = (c.esp.wrapping_add(0x20), g.u32(c.esp.wrapping_add(0x20))?);
                fpu(c, |x| fp::arith(x, arith_op::MUL, 0, src_kind::F32, u64::from(v)));
                mark(c, g, 0xf92a3, Some(a))?;
                let b = g.u32(c.esp.wrapping_add(0x10))?;
                sse::load(c, 6, b);
                let b = g.u32(c.esp.wrapping_add(0x78))?;
                sse::load(c, 4, b);
                let b = g.u32(c.esp.wrapping_add(0x18))?;
                sse::load(c, 3, b);
                fpu(c, |x| fp::ld_st(x, 0));
                mark(c, g, 0xf92b9, None)?;
                let b = g.u32(c.esp.wrapping_add(0x74))?;
                sse::load(c, 7, b);
                fpu(c, |x| fp::unary(x, fp::unary_op::SIN));
                mark(c, g, 0xf92c1, None)?;
                let b = g.u32(ONE)?;
                sse::load(c, 2, b);
                sse::movaps(c, 0, 5);
                sse::ss_reg(c, Mul, 0, 6);
                sse::movaps(c, 1, 4);
                sse::ss_reg(c, Mul, 1, 3);
                sse::ss_reg(c, Add, 0, 1);
                sse::movaps(c, 1, 7);
                let b = g.u32(c.esp.wrapping_add(0x14))?;
                sse::ss(c, Mul, 1, b);
                sse::ss_reg(c, Add, 0, 1);
                c.edx = c.edi;
                let (v, n) = (c.edx, 0x6);
                let v = alu::shr(c, v, n, 32);
                c.edx = v;
                let v = (c.edx & 0xff) & 0x1;
                logic(c, v, 0x80);
                let a = c.esp.wrapping_add(0xb0);
                store_f32(c, g, 0xf92f2, a, true)?;
                fpu(c, |x| fp::unary(x, fp::unary_op::COS));
                mark(c, g, 0xf92f9, None)?;
                let a = c.esp.wrapping_add(0x60);
                store_f32(c, g, 0xf92fb, a, true)?;
                let b = g.u32(c.esp.wrapping_add(0x60))?;
                sse::load(c, 1, b);
                sse::ss_reg(c, Sub, 2, 1);
                sse::ss_reg(c, Mul, 0, 2);
                sse::movaps(c, 2, 5);
                sse::ss_reg(c, Mul, 2, 3);
                sse::movaps(c, 3, 4);
                sse::ss_reg(c, Mul, 3, 6);
                sse::ss_reg(c, Sub, 2, 3);
                sse::movaps(c, 3, 7);
                sse::ss_reg(c, Mul, 3, 6);
                sse::movaps(c, 6, 5);
                let b = g.u32(c.esp.wrapping_add(0x14))?;
                sse::ss(c, Mul, 6, b);
                sse::ss_reg(c, Sub, 3, 6);
                sse::movaps(c, 6, 1);
                let b = g.u32(c.esp.wrapping_add(0x10))?;
                sse::ss(c, Mul, 6, b);
                sse::movaps(c, 7, 0);
                sse::ss_reg(c, Mul, 7, 5);
                sse::ss_reg(c, Add, 6, 7);
                let b = g.u32(c.esp.wrapping_add(0xb0))?;
                sse::load(c, 7, b);
                g.set_u32(c.esp.wrapping_add(0x60), sse::lane0(c, 6))?;
                let b = g.u32(c.esp.wrapping_add(0x74))?;
                sse::load(c, 6, b);
                let b = g.u32(c.esp.wrapping_add(0x18))?;
                sse::ss(c, Mul, 6, b);
                sse::movaps(c, 5, 4);
                let b = g.u32(c.esp.wrapping_add(0x14))?;
                sse::ss(c, Mul, 5, b);
                sse::ss_reg(c, Sub, 5, 6);
                let b = g.u32(c.esp.wrapping_add(0x60))?;
                sse::load(c, 6, b);
                sse::ss_reg(c, Mul, 5, 7);
                sse::ss_reg(c, Sub, 6, 5);
                sse::movaps(c, 5, 1);
                let b = g.u32(c.esp.wrapping_add(0x14))?;
                sse::ss(c, Mul, 5, b);
                let b = g.u32(c.esp.wrapping_add(0x18))?;
                sse::ss(c, Mul, 1, b);
                g.set_u32(c.esp.wrapping_add(0x10), sse::lane0(c, 6))?;
                let b = g.u32(c.esp.wrapping_add(0x74))?;
                sse::load(c, 6, b);
                sse::ss_reg(c, Mul, 6, 0);
                sse::ss_reg(c, Mul, 4, 0);
                sse::ss_reg(c, Add, 5, 6);
                sse::ss_reg(c, Mul, 2, 7);
                sse::ss_reg(c, Add, 1, 4);
                sse::ss_reg(c, Mul, 3, 7);
                sse::ss_reg(c, Sub, 5, 2);
                sse::ss_reg(c, Sub, 1, 3);
                g.set_u32(c.esp.wrapping_add(0x14), sse::lane0(c, 5))?;
                g.set_u32(c.esp.wrapping_add(0x18), sse::lane0(c, 1))?;
                if zf(c) { 0xf94dc } else { 0xf93ca }
            }
            0xf93ca => {
                c.eax = g.u32(c.esp.wrapping_add(0x90))?;
                c.ecx = g.u32(c.esp.wrapping_add(0x94))?;
                c.edx = g.u32(c.esp.wrapping_add(0x98))?;
                g.set_u32(c.esp.wrapping_add(0x84), c.eax)?;
                c.eax = c.ebp.wrapping_add(0x7c);
                g.set_u32(c.esp.wrapping_add(0x88), c.ecx)?;
                c.ecx = g.u32(c.eax)?;
                g.set_u32(c.esp.wrapping_add(0x8c), c.edx)?;
                c.edx = g.u32(c.eax.wrapping_add(0x4))?;
                c.eax = g.u32(c.eax.wrapping_add(0x8))?;
                g.set_u32(c.esp.wrapping_add(0x6c), c.eax)?;
                c.eax = c.esp.wrapping_add(0x84);
                g.set_u32(c.esp.wrapping_add(0x64), c.ecx)?;
                g.set_u32(c.esp.wrapping_add(0x68), c.edx)?;
                return Ok(Next::Call { target: NORMALIZE, site: 0xf9417 });
            }
            0xf9417 => {
                let b = g.u32(c.esp.wrapping_add(0x8c))?;
                sse::load(c, 2, b);
                let b = g.u32(c.esp.wrapping_add(0x6c))?;
                sse::load(c, 5, b);
                let b = g.u32(c.esp.wrapping_add(0x88))?;
                sse::load(c, 1, b);
                let b = g.u32(c.esp.wrapping_add(0x68))?;
                sse::load(c, 4, b);
                let b = g.u32(c.esp.wrapping_add(0x64))?;
                sse::load(c, 7, b);
                let b = g.u32(c.esp.wrapping_add(0x84))?;
                sse::load(c, 6, b);
                sse::movaps(c, 3, 5);
                sse::ss_reg(c, Mul, 3, 1);
                sse::movaps(c, 0, 2);
                sse::ss_reg(c, Mul, 0, 4);
                sse::ss_reg(c, Sub, 3, 0);
                sse::ss_reg(c, Mul, 5, 6);
                sse::movaps(c, 0, 2);
                sse::ss_reg(c, Mul, 0, 7);
                sse::ss_reg(c, Sub, 0, 5);
                sse::movaps(c, 5, 1);
                sse::ss_reg(c, Mul, 5, 7);
                sse::ss_reg(c, Mul, 4, 6);
                sse::ss_reg(c, Sub, 4, 5);
                sse::movaps(c, 7, 4);
                sse::movaps(c, 5, 0);
                sse::ss_reg(c, Mul, 7, 1);
                sse::ss_reg(c, Mul, 5, 2);
                sse::ss_reg(c, Mul, 4, 6);
                sse::ss_reg(c, Mul, 2, 3);
                sse::ss_reg(c, Mul, 1, 3);
                sse::ss_reg(c, Mul, 0, 6);
                sse::ss_reg(c, Sub, 5, 7);
                sse::ss_reg(c, Sub, 4, 2);
                sse::ss_reg(c, Sub, 1, 0);
                c.eax = c.esp.wrapping_add(0x64);
                g.set_u32(c.esp.wrapping_add(0x64), sse::lane0(c, 5))?;
                g.set_u32(c.esp.wrapping_add(0x68), sse::lane0(c, 4))?;
                g.set_u32(c.esp.wrapping_add(0x6c), sse::lane0(c, 1))?;
                return Ok(Next::Call { target: NORMALIZE, site: 0xf94b9 });
            }
            0xf94b9 => {
                c.ecx = g.u32(c.esp.wrapping_add(0x12c))?;
                push(c, g, 0)?;
                push(c, g, 0)?;
                let v = c.ecx;
                push(c, g, v)?;
                c.edx = c.esp.wrapping_add(0x70);
                c.ecx = c.esp.wrapping_add(0x90);
                c.eax = 0;
                logic(c, 0, SIGN32);
                return Ok(Next::Call { target: OBJECT_SET_TRANSFORM, site: 0xf94d7 });
            }
            0xf94d7 => {
                sse::zero(c, 3);
                0xf94e5
            }
            0xf94dc => {
                sse::zero(c, 3);
                0xf94eb
            }
            0xf94e1 => {
                c.ebx = g.u32(c.esp.wrapping_add(0x3c))?;
                0xf94e5
            }
            0xf94e5 => {
                let b = g.u32(c.esp.wrapping_add(0x14))?;
                sse::load(c, 5, b);
                0xf94eb
            }
            0xf94eb => {
                c.edx = g.u32(c.ebp.wrapping_add(0x12c))?;
                let (v, n) = (c.edx, 0x8);
                let v = alu::shr(c, v, n, 32);
                c.edx = v;
                let v = (c.edx & 0xff) & 0x1;
                logic(c, v, 0x80);
                if !zf(c) { 0xf96c8 } else { 0xf94fd }
            }
            0xf94fd => {
                let b = g.u32(c.ebx.wrapping_add(0x17c))?;
                sse::load(c, 0, b);
                let b = sse::lane0(c, 3);
                sse::ucomiss(c, sse::lane0(c, 0), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                if !pf(c) { 0xf96e6 } else { 0xf9512 }
            }
            0xf9512 => {
                let b = g.u32(c.ebx.wrapping_add(0x180))?;
                sse::load(c, 0, b);
                let b = sse::lane0(c, 3);
                sse::ucomiss(c, sse::lane0(c, 0), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                if !pf(c) { 0xf96e6 } else { 0xf9527 }
            }
            0xf9527 => {
                let b = g.u32(c.ebx.wrapping_add(0x18c))?;
                sse::load(c, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x16c))?;
                sse::ss(c, Mul, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x194))?;
                sse::load(c, 1, b);
                c.eax = c.ebp.wrapping_add(0x70);
                c.ecx = g.u32(c.eax)?;
                c.edx = g.u32(c.eax.wrapping_add(0x4))?;
                c.eax = g.u32(c.eax.wrapping_add(0x8))?;
                sse::ss_reg(c, Sub, 1, 0);
                let b = g.u32(c.ebx.wrapping_add(0x194))?;
                sse::ss(c, Mul, 1, b);
                let b = sse::lane0(c, 1);
                sse::comiss(c, sse::lane0(c, 3), b);
                g.set_u32(c.esp.wrapping_add(0x9c), c.ecx)?;
                g.set_u32(c.esp.wrapping_add(0xa0), c.edx)?;
                g.set_u32(c.esp.wrapping_add(0xa4), c.eax)?;
                if cf(c) || zf(c) { 0xf9575 } else { 0xf9570 }
            }
            0xf9570 => {
                sse::movaps(c, 1, 3);
                0xf9585
            }
            0xf9575 => {
                let b = g.u32(ONE)?;
                sse::load(c, 0, b);
                let b = sse::lane0(c, 0);
                sse::comiss(c, sse::lane0(c, 1), b);
                if cf(c) || zf(c) { 0xf9585 } else { 0xf9582 }
            }
            0xf9582 => {
                sse::movaps(c, 1, 0);
                0xf9585
            }
            0xf9585 => {
                let b = g.u32(c.ebx.wrapping_add(0x180))?;
                sse::load(c, 0, b);
                let b = g.u32(c.ebx.wrapping_add(0x17c))?;
                sse::ss(c, Sub, 0, b);
                let b = g.u32(c.esp.wrapping_add(0x9c))?;
                sse::load(c, 3, b);
                let b = g.u32(c.esp.wrapping_add(0xa0))?;
                sse::load(c, 2, b);
                let b = g.u32(c.esp.wrapping_add(0xa4))?;
                sse::load(c, 4, b);
                sse::ss_reg(c, Mul, 0, 1);
                let b = g.u32(c.ebx.wrapping_add(0x17c))?;
                sse::ss(c, Add, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x16c))?;
                sse::ss(c, Mul, 0, b);
                sse::movaps(c, 1, 0);
                let b = g.u32(c.ebp.wrapping_add(0x190))?;
                sse::ss(c, Sub, 1, b);
                g.set_u32(c.ebp.wrapping_add(0x190), sse::lane0(c, 0))?;
                let b = g.u32(c.esp.wrapping_add(0x20))?;
                sse::ss(c, Mul, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x194))?;
                sse::ss(c, Add, 0, b);
                g.set_u32(c.ebp.wrapping_add(0x194), sse::lane0(c, 0))?;
                let b = g.u32(c.esi)?;
                sse::load(c, 0, b);
                sse::ss_reg(c, Mul, 3, 1);
                let b = g.u32(c.esp.wrapping_add(0x10))?;
                sse::ss(c, Add, 3, b);
                sse::ss_reg(c, Add, 0, 3);
                sse::ss_reg(c, Mul, 2, 1);
                sse::ss_reg(c, Mul, 4, 1);
                let b = g.u32(HALF)?;
                sse::load(c, 1, b);
                let b = g.u32(c.esp.wrapping_add(0x18))?;
                sse::ss(c, Add, 4, b);
                sse::ss_reg(c, Mul, 0, 1);
                g.set_u32(c.esp.wrapping_add(0x48), sse::lane0(c, 0))?;
                let b = g.u32(c.ebp.wrapping_add(0x8c))?;
                sse::load(c, 0, b);
                sse::ss_reg(c, Add, 2, 5);
                sse::ss_reg(c, Add, 0, 2);
                sse::ss_reg(c, Mul, 0, 1);
                g.set_u32(c.esp.wrapping_add(0x4c), sse::lane0(c, 0))?;
                let b = g.u32(c.ebp.wrapping_add(0x90))?;
                sse::load(c, 0, b);
                sse::ss_reg(c, Add, 0, 4);
                sse::ss_reg(c, Mul, 0, 1);
                c.eax = c.esp.wrapping_add(0x10);
                g.set_u32(c.esp.wrapping_add(0x10), sse::lane0(c, 3))?;
                g.set_u32(c.esp.wrapping_add(0x14), sse::lane0(c, 2))?;
                g.set_u32(c.esp.wrapping_add(0x18), sse::lane0(c, 4))?;
                g.set_u32(c.esp.wrapping_add(0x50), sse::lane0(c, 0))?;
                return Ok(Next::Call { target: points::VECTOR_LENGTH_3D.base, site: 0xf966a });
            }
            0xf966a => {
                let b = g.u32(c.ebx.wrapping_add(0xe0))?;
                sse::load(c, 1, b);
                sse::zero(c, 0);
                let (a, v) = (c.esp.wrapping_add(0xac), g.u32(c.esp.wrapping_add(0xac))?);
                fpu(c, |x| fp::arith(x, arith_op::ADD, 0, src_kind::F32, u64::from(v)));
                mark(c, g, 0xf9675, Some(a))?;
                let b = sse::lane0(c, 0);
                sse::ucomiss(c, sse::lane0(c, 1), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                let (a, v) = (HALF, g.u32(HALF)?);
                fpu(c, |x| fp::arith(x, arith_op::MUL, 0, src_kind::F32, u64::from(v)));
                mark(c, g, 0xf9683, Some(a))?;
                let a = c.esp.wrapping_add(0x80);
                store_f32(c, g, 0xf9689, a, true)?;
                if pf(c) { 0xf96c5 } else { 0xf9692 }
            }
            0xf9692 => {
                let b = g.u32(c.ebx.wrapping_add(0xd8))?;
                sse::load(c, 1, b);
                let b = sse::lane0(c, 0);
                sse::ucomiss(c, sse::lane0(c, 1), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                if pf(c) { 0xf96c5 } else { 0xf96a3 }
            }
            0xf96a3 => {
                let b = g.u32(c.ebp.wrapping_add(0x168))?;
                sse::load(c, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x170))?;
                sse::comiss(c, sse::lane0(c, 0), b);
                if cf(c) { 0xf96c5 } else { 0xf96b4 }
            }
            0xf96b4 => {
                c.eax = g.u32(c.esp.wrapping_add(0x12c))?;
                c.ecx = 0x2;
                return Ok(Next::Call { target: RAISE_STATE, site: 0xf96c5 });
            }
            0xf96c5 => {
                sse::zero(c, 3);
                0xf96c8
            }
            0xf96c8 => {
                c.eax = g.u32(c.ebp.wrapping_add(0x4))?;
                let (v, n) = (c.eax, 0x3);
                let v = alu::shr(c, v, n, 32);
                c.eax = v;
                let v = (c.eax & 0xff) & 0x1;
                logic(c, v, 0x80);
                if zf(c) { 0xf9714 } else { 0xf96d2 }
            }
            0xf96d2 => {
                c.ecx = g.u32(GRAVITY_OWNER)?;
                let b = g.u32(c.ebx.wrapping_add(0x170))?;
                sse::load(c, 0, b);
                let b = g.u32(c.ecx)?;
                sse::ss(c, Mul, 0, b);
                0xf9726
            }
            0xf96e6 => {
                c.ecx = c.esi;
                c.edx = g.u32(c.ecx)?;
                c.eax = g.u32(c.ecx.wrapping_add(0x4))?;
                c.ecx = g.u32(c.ecx.wrapping_add(0x8))?;
                g.set_u32(c.esp.wrapping_add(0x10), c.edx)?;
                c.edx = c.esi;
                g.set_u32(c.esp.wrapping_add(0x14), c.eax)?;
                c.eax = g.u32(c.edx)?;
                g.set_u32(c.esp.wrapping_add(0x18), c.ecx)?;
                c.ecx = g.u32(c.edx.wrapping_add(0x4))?;
                c.edx = g.u32(c.edx.wrapping_add(0x8))?;
                g.set_u32(c.esp.wrapping_add(0x48), c.eax)?;
                g.set_u32(c.esp.wrapping_add(0x4c), c.ecx)?;
                g.set_u32(c.esp.wrapping_add(0x50), c.edx)?;
                0xf96c8
            }
            0xf9714 => {
                c.edx = g.u32(GRAVITY_OWNER)?;
                let b = g.u32(c.ebx.wrapping_add(0x164))?;
                sse::load(c, 0, b);
                let b = g.u32(c.edx)?;
                sse::ss(c, Mul, 0, b);
                0xf9726
            }
            0xf9726 => {
                let b = g.u32(c.esp.wrapping_add(0x20))?;
                sse::load(c, 2, b);
                let b = g.u32(c.esp.wrapping_add(0x18))?;
                sse::load(c, 1, b);
                sse::ss_reg(c, Mul, 0, 2);
                sse::ss_reg(c, Sub, 1, 0);
                let b = g.u32(HALF)?;
                sse::ss(c, Mul, 0, b);
                g.set_u32(c.esp.wrapping_add(0x18), sse::lane0(c, 1))?;
                let b = g.u32(c.esp.wrapping_add(0x50))?;
                sse::load(c, 1, b);
                sse::ss_reg(c, Sub, 1, 0);
                let b = g.u32(c.ebx.wrapping_add(0xe0))?;
                sse::load(c, 0, b);
                let b = sse::lane0(c, 3);
                sse::ucomiss(c, sse::lane0(c, 0), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                sse::movaps(c, 4, 1);
                if !pf(c) { 0xf97d1 } else { 0xf9766 }
            }
            0xf9766 => {
                let b = g.u32(c.esp.wrapping_add(0x80))?;
                sse::load(c, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x168))?;
                sse::load(c, 5, b);
                sse::movaps(c, 1, 0);
                sse::ss_reg(c, Mul, 1, 2);
                sse::ss_reg(c, Add, 5, 1);
                let b = g.u32(c.ebx.wrapping_add(0xe0))?;
                sse::comiss(c, sse::lane0(c, 5), b);
                if cf(c) || zf(c) { 0xf97d1 } else { 0xf978b }
            }
            0xf978b => {
                let b = sse::lane0(c, 3);
                sse::ucomiss(c, sse::lane0(c, 0), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                if !pf(c) { 0xf97bb } else { 0xf9794 }
            }
            0xf9794 => {
                let b = g.u32(c.ebx.wrapping_add(0xe0))?;
                sse::load(c, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x168))?;
                sse::ss(c, Sub, 0, b);
                c.eax = g.u32(c.esp.wrapping_add(0x12c))?;
                c.ecx = 0x1;
                sse::ss_reg(c, Div, 0, 1);
                return Ok(Next::Call { target: RAISE_STATE, site: 0xf97b9 });
            }
            0xf97b9 => {
                0xf97d9
            }
            0xf97bb => {
                c.eax = g.u32(c.esp.wrapping_add(0x12c))?;
                c.ecx = 0x1;
                sse::movaps(c, 0, 3);
                return Ok(Next::Call { target: RAISE_STATE, site: 0xf97cf });
            }
            0xf97cf => {
                0xf97d9
            }
            0xf97d1 => {
                let b = g.u32(ONE)?;
                sse::load(c, 0, b);
                0xf97d9
            }
            0xf97d9 => {
                let (a, b) = (u32::from(g.u16(c.esp.wrapping_add(0x44))?), 0xa);
                alu::cmp16(c, a, b);
                sse::ss_reg(c, Mul, 0, 2);
                sse::movaps(c, 2, 0);
                let b = g.u32(c.esp.wrapping_add(0x48))?;
                sse::ss(c, Mul, 2, b);
                let b = g.u32(c.ebp.wrapping_add(0x64))?;
                sse::ss(c, Add, 2, b);
                sse::movaps(c, 1, 0);
                let b = g.u32(c.esp.wrapping_add(0x4c))?;
                sse::ss(c, Mul, 1, b);
                let b = g.u32(c.ebp.wrapping_add(0x68))?;
                sse::ss(c, Add, 1, b);
                sse::ss_reg(c, Mul, 0, 4);
                let b = g.u32(c.ebp.wrapping_add(0x6c))?;
                sse::ss(c, Add, 0, b);
                g.set_u32(c.esp.wrapping_add(0x54), sse::lane0(c, 2))?;
                g.set_u32(c.esp.wrapping_add(0x58), sse::lane0(c, 1))?;
                g.set_u32(c.esp.wrapping_add(0x5c), sse::lane0(c, 0))?;
                if zf(c) { 0xf9969 } else { 0xf9820 }
            }
            0xf9820 => {
                let (a, b) = (u32::from(g.u16(c.ebp.wrapping_add(0x130))?), 0x2);
                alu::cmp16(c, a, b);
                if zf(c) { 0xf9991 } else { 0xf982e }
            }
            0xf982e => {
                c.ebx = g.u32(c.esp.wrapping_add(0x12c))?;
                c.eax = c.esp.wrapping_add(0xcc);
                let v = c.eax;
                push(c, g, v)?;
                c.edi = c.esp.wrapping_add(0x58);
                c.ecx = c.ebx;
                g.set_u8(c.esp.wrapping_add(0x22), 1)?;
                return Ok(Next::Call { target: COLLIDE, site: 0xf984d });
            }
            0xf984d => {
                let v = c.eax & 0xff;
                logic(c, v, 0x80);
                if zf(c) { 0xf997c } else { 0xf9855 }
            }
            0xf9855 => {
                let b = g.u32(c.esp.wrapping_add(0xd0))?;
                sse::load(c, 3, b);
                let b = g.u32(ONE)?;
                sse::load(c, 0, b);
                let b = g.u32(c.esp.wrapping_add(0x14))?;
                sse::load(c, 1, b);
                let b = g.u32(c.esi.wrapping_add(0x4))?;
                sse::ss(c, Sub, 1, b);
                let b = g.u32(c.esp.wrapping_add(0x18))?;
                sse::load(c, 2, b);
                let b = g.u32(c.esi.wrapping_add(0x8))?;
                sse::ss(c, Sub, 2, b);
                c.eax = g.u32(c.ebp.wrapping_add(0x12c))?;
                sse::ss_reg(c, Sub, 0, 3);
                let b = g.u32(c.esp.wrapping_add(0x20))?;
                sse::ss(c, Mul, 0, b);
                g.set_u32(c.esp.wrapping_add(0x20), sse::lane0(c, 0))?;
                let b = g.u32(c.esp.wrapping_add(0x10))?;
                sse::load(c, 0, b);
                let b = g.u32(c.esi)?;
                sse::ss(c, Sub, 0, b);
                sse::ss_reg(c, Mul, 0, 3);
                let b = g.u32(c.esi)?;
                sse::ss(c, Add, 0, b);
                g.set_u32(c.esp.wrapping_add(0x10), sse::lane0(c, 0))?;
                let b = g.u32(c.esp.wrapping_add(0xfc))?;
                sse::load(c, 0, b);
                let v = c.eax | 0x800;
                c.eax = v;
                logic(c, v, SIGN32);
                let b = g.u32(SURFACE_LIMIT)?;
                sse::comiss(c, sse::lane0(c, 0), b);
                sse::ss_reg(c, Mul, 1, 3);
                let b = g.u32(c.esi.wrapping_add(0x4))?;
                sse::ss(c, Add, 1, b);
                sse::ss_reg(c, Mul, 2, 3);
                let b = g.u32(c.esi.wrapping_add(0x8))?;
                sse::ss(c, Add, 2, b);
                g.set_u32(c.esp.wrapping_add(0x14), sse::lane0(c, 1))?;
                g.set_u32(c.esp.wrapping_add(0x18), sse::lane0(c, 2))?;
                g.set_u32(c.ebp.wrapping_add(0x12c), c.eax)?;
                if cf(c) || zf(c) { 0xf98ee } else { 0xf98e5 }
            }
            0xf98e5 => {
                let v = c.eax | 0x4;
                c.eax = v;
                logic(c, v, SIGN32);
                g.set_u32(c.ebp.wrapping_add(0x12c), c.eax)?;
                0xf98ee
            }
            0xf98ee => {
                c.ecx = g.u32(c.esp.wrapping_add(0x10c))?;
                let v = c.ecx;
                push(c, g, v)?;
                let v = c.ebx;
                push(c, g, v)?;
                return Ok(Next::Call { target: HIT_SURFACE, site: 0xf98fc });
            }
            0xf98fc => {
                c.eax = g.u32(c.esp.wrapping_add(0x44))?;
                let v = c.eax;
                let v = inc(c, v, 32);
                c.eax = v;
                c.edx = (c.eax & 0xffff) as u16 as i16 as i32 as u32;
                let v = c.edx;
                push(c, g, v)?;
                g.set_u32(c.esp.wrapping_add(0x48), c.eax)?;
                c.eax = g.u32(c.esp.wrapping_add(0x134))?;
                let v = c.eax;
                push(c, g, v)?;
                c.ecx = c.esp.wrapping_add(0x5c);
                let v = c.ecx;
                push(c, g, v)?;
                c.edx = c.esp.wrapping_add(0xd8);
                let v = c.edx;
                push(c, g, v)?;
                let v = c.ebx;
                push(c, g, v)?;
                c.eax = c.esp.wrapping_add(0x24);
                g.set_u32(c.ebp.wrapping_add(0x140), NONE)?;
                return Ok(Next::Call { target: HIT, site: 0xf9932 });
            }
            0xf9932 => {
                c.eax = g.u32(c.esp.wrapping_add(0x3c))?;
                c.ecx = 0;
                logic(c, 0, SIGN32);
                c.ecx = (c.ecx & !0xffff) | u32::from(g.u16(c.eax.wrapping_add(0xc2))?);
                push(c, g, 1)?;
                c.edx = c.esp.wrapping_add(0xd8);
                let v = c.ecx;
                push(c, g, v)?;
                push(c, g, 2)?;
                let v = c.edx;
                push(c, g, v)?;
                let v = c.ebx;
                push(c, g, v)?;
                return Ok(Next::Call { target: HIT_EFFECT, site: 0xf9952 });
            }
            0xf9952 => {
                let b = g.u32(c.esp.wrapping_add(0x58))?;
                sse::load(c, 1, b);
                let b = g.u32(c.esp.wrapping_add(0x54))?;
                sse::load(c, 2, b);
                let b = g.u32(c.esp.wrapping_add(0x5c))?;
                sse::load(c, 0, b);
                sse::zero(c, 3);
                0xf9997
            }
            0xf9969 => {
                c.eax = g.u32(c.esp.wrapping_add(0x12c))?;
                c.ecx = 0x1;
                return Ok(Next::Call { target: RAISE_STATE, site: 0xf997a });
            }
            0xf997a => {
                0xf9991
            }
            0xf997c => {
                let b = g.u32(c.esp.wrapping_add(0x58))?;
                sse::load(c, 1, b);
                let b = g.u32(c.esp.wrapping_add(0x54))?;
                sse::load(c, 2, b);
                let b = g.u32(c.esp.wrapping_add(0x5c))?;
                sse::load(c, 0, b);
                sse::zero(c, 3);
                0xf9991
            }
            0xf9991 => {
                g.set_u32(c.esp.wrapping_add(0x20), sse::lane0(c, 3))?;
                0xf9997
            }
            0xf9997 => {
                c.eax = g.u32(c.ebp.wrapping_add(0x12c))?;
                let (v, n) = (c.eax, 0x3);
                let v = alu::shr(c, v, n, 32);
                c.eax = v;
                let v = (c.eax & 0xff) & 0x1;
                logic(c, v, 0x80);
                if !zf(c) { 0xf9fc3 } else { 0xf99a8 }
            }
            0xf99a8 => {
                c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x1e))?);
                let v = c.eax & 0xff;
                logic(c, v, 0x80);
                if zf(c) { 0xf9fc3 } else { 0xf99b4 }
            }
            0xf99b4 => {
                let b = g.u32(c.ebp.wrapping_add(0x64))?;
                sse::ss(c, Sub, 2, b);
                let b = g.u32(c.ebp.wrapping_add(0x68))?;
                sse::ss(c, Sub, 1, b);
                let b = g.u32(c.ebp.wrapping_add(0x6c))?;
                sse::ss(c, Sub, 0, b);
                c.eax = c.ebp.wrapping_add(0x70);
                c.ecx = g.u32(c.eax)?;
                c.edx = g.u32(c.eax.wrapping_add(0x4))?;
                c.eax = g.u32(c.eax.wrapping_add(0x8))?;
                g.set_u32(c.esp.wrapping_add(0x30), c.ecx)?;
                c.ecx = c.ebp.wrapping_add(0x7c);
                g.set_u32(c.esp.wrapping_add(0x34), c.edx)?;
                c.edx = g.u32(c.ecx)?;
                g.set_u32(c.esp.wrapping_add(0x38), c.eax)?;
                c.eax = g.u32(c.ecx.wrapping_add(0x4))?;
                c.ecx = g.u32(c.ecx.wrapping_add(0x8))?;
                g.set_u32(c.esp.wrapping_add(0x28), c.eax)?;
                c.eax = c.esp.wrapping_add(0xb4);
                g.set_u32(c.esp.wrapping_add(0x24), c.edx)?;
                g.set_u32(c.esp.wrapping_add(0x2c), c.ecx)?;
                g.set_u32(c.esp.wrapping_add(0xb4), sse::lane0(c, 2))?;
                g.set_u32(c.esp.wrapping_add(0xb8), sse::lane0(c, 1))?;
                g.set_u32(c.esp.wrapping_add(0xbc), sse::lane0(c, 0))?;
                return Ok(Next::Call { target: points::VECTOR_LENGTH_3D.base, site: 0xf9a18 });
            }
            0xf9a18 => {
                let (a, v) = (c.ebp.wrapping_add(0x168), g.u32(c.ebp.wrapping_add(0x168))?);
                fpu(c, |x| fp::arith(x, arith_op::ADD, 0, src_kind::F32, u64::from(v)));
                mark(c, g, 0xf9a18, Some(a))?;
                c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x1f))?);
                let v = c.eax & 0xff;
                logic(c, v, 0x80);
                let a = c.ebp.wrapping_add(0x168);
                store_f32(c, g, 0xf9a24, a, true)?;
                if !zf(c) { 0xf9a79 } else { 0xf9a2c }
            }
            0xf9a2c => {
                c.edx = g.u32(c.esp.wrapping_add(0x3c))?;
                let (a, b) = (g.u32(c.edx.wrapping_add(0x13c))?, NONE);
                arithmetic(c, a, b, true);
                if zf(c) { 0xf9a79 } else { 0xf9a39 }
            }
            0xf9a39 => {
                c.esi = 0;
                logic(c, 0, SIGN32);
                0xf9a40
            }
            // 0xf9a3d: not reachable (alignment padding).
            0xf9a40 => {
                c.ecx = g.u32(c.esp.wrapping_add(0x3c))?;
                c.edx = g.u32(c.ecx.wrapping_add(0x13c))?;
                c.eax = c.esp.wrapping_add(0xe8);
                let v = c.eax;
                push(c, g, v)?;
                c.eax = g.u32(c.ebp.wrapping_add(0xc4))?;
                let v = c.edx;
                push(c, g, v)?;
                let v = c.eax;
                push(c, g, v)?;
                let v = c.esi;
                push(c, g, v)?;
                c.edi = c.esp.wrapping_add(0xc4);
                c.ebx = c.ebp.wrapping_add(0x64);
                return Ok(Next::Call { target: FLYBY, site: 0xf9a6a });
            }
            0xf9a6a => {
                let v = c.eax & 0xff;
                logic(c, v, 0x80);
                if zf(c) { 0xf9a73 } else { 0xf9a6e }
            }
            0xf9a6e => {
                g.set_u8(c.esp.wrapping_add(0x1f), 1)?;
                0xf9a73
            }
            0xf9a73 => {
                let v = c.esi;
                let v = inc(c, v, 32);
                c.esi = v;
                let (a, b) = (c.esi, 0x4);
                arithmetic(c, a, b, true);
                if sf(c) != of(c) { 0xf9a40 } else { 0xf9a79 }
            }
            0xf9a79 => {
                c.ecx = g.u32(c.esp.wrapping_add(0x3c))?;
                let v = u32::from(g.u8(c.ecx.wrapping_add(0xbc))?) & 0x1;
                logic(c, v, 0x80);
                if zf(c) { 0xf9cc6 } else { 0xf9a8a }
            }
            0xf9a8a => {
                let b = g.u32(c.esp.wrapping_add(0x10))?;
                sse::load(c, 1, b);
                sse::zero(c, 0);
                let b = sse::lane0(c, 0);
                sse::ucomiss(c, sse::lane0(c, 1), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                if pf(c) { 0xf9abe } else { 0xf9a9c }
            }
            0xf9a9c => {
                let b = g.u32(c.esp.wrapping_add(0x14))?;
                sse::load(c, 1, b);
                let b = sse::lane0(c, 0);
                sse::ucomiss(c, sse::lane0(c, 1), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                if pf(c) { 0xf9abe } else { 0xf9aab }
            }
            0xf9aab => {
                let b = g.u32(c.esp.wrapping_add(0x18))?;
                sse::load(c, 1, b);
                let b = sse::lane0(c, 0);
                sse::ucomiss(c, sse::lane0(c, 1), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                if !pf(c) { 0xf9cc6 } else { 0xf9abe }
            }
            0xf9abe => {
                c.eax = g.u32(c.esp.wrapping_add(0x14))?;
                c.edx = g.u32(c.esp.wrapping_add(0x10))?;
                c.ecx = g.u32(c.esp.wrapping_add(0x18))?;
                g.set_u32(c.esp.wrapping_add(0x34), c.eax)?;
                c.eax = c.esp.wrapping_add(0x30);
                g.set_u32(c.esp.wrapping_add(0x30), c.edx)?;
                g.set_u32(c.esp.wrapping_add(0x38), c.ecx)?;
                return Ok(Next::Call { target: NORMALIZE, site: 0xf9adf });
            }
            0xf9adf => {
                let b = g.u32(ZERO)?;
                sse::comiss(c, sse::lane0(c, 0), b);
                if cf(c) || zf(c) { 0xf9ba6 } else { 0xf9aec }
            }
            0xf9aec => {
                let b = g.u32(c.esp.wrapping_add(0x38))?;
                sse::load(c, 6, b);
                let b = g.u32(c.esp.wrapping_add(0x2c))?;
                sse::load(c, 2, b);
                let b = g.u32(c.esp.wrapping_add(0x34))?;
                sse::load(c, 5, b);
                let b = g.u32(c.esp.wrapping_add(0x28))?;
                sse::load(c, 3, b);
                let b = g.u32(c.esp.wrapping_add(0x30))?;
                sse::load(c, 4, b);
                let b = g.u32(c.esp.wrapping_add(0x24))?;
                sse::load(c, 7, b);
                sse::movaps(c, 0, 2);
                sse::ss_reg(c, Mul, 0, 5);
                sse::movaps(c, 1, 6);
                sse::ss_reg(c, Mul, 1, 3);
                sse::ss_reg(c, Sub, 1, 0);
                sse::movaps(c, 0, 6);
                sse::ss_reg(c, Mul, 0, 7);
                sse::ss_reg(c, Mul, 2, 4);
                sse::ss_reg(c, Sub, 2, 0);
                sse::ss_reg(c, Mul, 3, 4);
                sse::movaps(c, 0, 5);
                sse::ss_reg(c, Mul, 0, 7);
                sse::ss_reg(c, Sub, 0, 3);
                sse::movaps(c, 7, 2);
                sse::movaps(c, 3, 0);
                sse::ss_reg(c, Mul, 7, 6);
                sse::ss_reg(c, Mul, 3, 5);
                sse::ss_reg(c, Mul, 6, 1);
                sse::ss_reg(c, Mul, 0, 4);
                sse::ss_reg(c, Mul, 2, 4);
                sse::ss_reg(c, Mul, 5, 1);
                sse::ss_reg(c, Sub, 3, 7);
                sse::ss_reg(c, Sub, 6, 0);
                sse::ss_reg(c, Sub, 2, 5);
                c.eax = c.esp.wrapping_add(0x24);
                g.set_u32(c.esp.wrapping_add(0x24), sse::lane0(c, 3))?;
                g.set_u32(c.esp.wrapping_add(0x28), sse::lane0(c, 6))?;
                g.set_u32(c.esp.wrapping_add(0x2c), sse::lane0(c, 2))?;
                return Ok(Next::Call { target: NORMALIZE, site: 0xf9b85 });
            }
            0xf9b85 => {
                let b = g.u32(ZERO)?;
                sse::ucomiss(c, sse::lane0(c, 0), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                if pf(c) { 0xf9bbb } else { 0xf9b92 }
            }
            0xf9b92 => {
                c.eax = c.esp.wrapping_add(0x24);
                c.ecx = c.esp.wrapping_add(0x30);
                return Ok(Next::Call { target: PERPENDICULAR, site: 0xf9b9f });
            }
            0xf9b9f => {
                return Ok(Next::Call { target: NORMALIZE, site: 0xf9ba4 });
            }
            0xf9ba4 => {
                0xf9bbb
            }
            0xf9ba6 => {
                c.edx = g.u32(c.ebp.wrapping_add(0x70))?;
                c.eax = g.u32(c.ebp.wrapping_add(0x74))?;
                c.ecx = g.u32(c.ebp.wrapping_add(0x78))?;
                g.set_u32(c.esp.wrapping_add(0x30), c.edx)?;
                g.set_u32(c.esp.wrapping_add(0x34), c.eax)?;
                g.set_u32(c.esp.wrapping_add(0x38), c.ecx)?;
                0xf9bbb
            }
            0xf9bbb => {
                let b = g.u32(c.esp.wrapping_add(0x30))?;
                sse::load(c, 6, b);
                let b = g.u32(c.esp.wrapping_add(0x24))?;
                sse::load(c, 5, b);
                let b = g.u32(c.esp.wrapping_add(0x38))?;
                sse::load(c, 2, b);
                let b = g.u32(c.esp.wrapping_add(0x2c))?;
                sse::load(c, 3, b);
                let b = g.u32(c.esp.wrapping_add(0x28))?;
                sse::load(c, 7, b);
                let b = g.u32(c.ebp.wrapping_add(0x184))?;
                sse::load(c, 4, b);
                sse::movaps(c, 0, 6);
                sse::ss_reg(c, Mul, 0, 5);
                sse::movaps(c, 1, 2);
                sse::ss_reg(c, Mul, 1, 3);
                sse::ss_reg(c, Add, 0, 1);
                let b = g.u32(c.esp.wrapping_add(0x34))?;
                sse::load(c, 1, b);
                sse::ss_reg(c, Mul, 7, 1);
                sse::ss_reg(c, Add, 0, 7);
                let b = g.u32(ONE)?;
                sse::load(c, 7, b);
                sse::ss_reg(c, Sub, 7, 4);
                sse::ss_reg(c, Mul, 0, 7);
                let b = g.u32(c.esp.wrapping_add(0x28))?;
                sse::load(c, 7, b);
                sse::ss_reg(c, Mul, 7, 6);
                sse::ss_reg(c, Mul, 3, 6);
                sse::movaps(c, 4, 2);
                sse::ss_reg(c, Mul, 4, 5);
                sse::ss_reg(c, Sub, 3, 4);
                sse::movaps(c, 4, 1);
                sse::ss_reg(c, Mul, 4, 5);
                sse::ss_reg(c, Sub, 4, 7);
                sse::movaps(c, 7, 0);
                sse::ss_reg(c, Mul, 7, 6);
                let b = g.u32(c.ebp.wrapping_add(0x184))?;
                sse::load(c, 6, b);
                sse::ss_reg(c, Mul, 5, 6);
                sse::ss_reg(c, Add, 7, 5);
                let b = g.u32(c.esp.wrapping_add(0x2c))?;
                sse::load(c, 5, b);
                sse::movaps(c, 6, 2);
                let b = g.u32(c.esp.wrapping_add(0x28))?;
                sse::ss(c, Mul, 6, b);
                sse::ss_reg(c, Mul, 5, 1);
                sse::ss_reg(c, Sub, 6, 5);
                let b = g.u32(c.ebp.wrapping_add(0x180))?;
                sse::load(c, 5, b);
                sse::ss_reg(c, Mul, 6, 5);
                sse::ss_reg(c, Sub, 7, 6);
                let b = g.u32(c.ebp.wrapping_add(0x184))?;
                sse::load(c, 6, b);
                sse::ss_reg(c, Mul, 1, 0);
                sse::ss_reg(c, Mul, 2, 0);
                let b = g.u32(c.esp.wrapping_add(0x2c))?;
                sse::load(c, 0, b);
                g.set_u32(c.esp.wrapping_add(0x24), sse::lane0(c, 7))?;
                let b = g.u32(c.esp.wrapping_add(0x28))?;
                sse::load(c, 7, b);
                sse::ss_reg(c, Mul, 7, 6);
                sse::ss_reg(c, Mul, 0, 6);
                sse::ss_reg(c, Add, 1, 7);
                sse::ss_reg(c, Mul, 3, 5);
                sse::ss_reg(c, Add, 2, 0);
                sse::ss_reg(c, Mul, 4, 5);
                sse::ss_reg(c, Sub, 1, 3);
                sse::ss_reg(c, Sub, 2, 4);
                g.set_u32(c.esp.wrapping_add(0x28), sse::lane0(c, 1))?;
                g.set_u32(c.esp.wrapping_add(0x2c), sse::lane0(c, 2))?;
                0xf9ef3
            }
            0xf9cc6 => {
                let v = u32::from(g.u8(c.ebp.wrapping_add(0x12c))?) & 0x1;
                logic(c, v, 0x80);
                if zf(c) { 0xf9ef3 } else { 0xf9cd3 }
            }
            0xf9cd3 => {
                let b = g.u32(c.esp.wrapping_add(0x38))?;
                sse::load(c, 1, b);
                let b = g.u32(c.esp.wrapping_add(0x34))?;
                sse::load(c, 2, b);
                let b = g.u32(c.esp.wrapping_add(0x30))?;
                sse::load(c, 5, b);
                let b = g.u32(c.ebp.wrapping_add(0x184))?;
                sse::load(c, 6, b);
                let b = g.u32(c.ebp.wrapping_add(0x17c))?;
                sse::load(c, 4, b);
                sse::movaps(c, 0, 1);
                let b = g.u32(c.ebp.wrapping_add(0x17c))?;
                sse::ss(c, Mul, 0, b);
                sse::movaps(c, 3, 2);
                let b = g.u32(c.ebp.wrapping_add(0x178))?;
                sse::ss(c, Mul, 3, b);
                sse::ss_reg(c, Add, 0, 3);
                let b = g.u32(c.ebp.wrapping_add(0x174))?;
                sse::load(c, 3, b);
                sse::ss_reg(c, Mul, 3, 5);
                sse::ss_reg(c, Add, 0, 3);
                let b = g.u32(ONE)?;
                sse::load(c, 3, b);
                sse::ss_reg(c, Sub, 3, 6);
                sse::ss_reg(c, Mul, 0, 3);
                sse::ss_reg(c, Mul, 4, 5);
                sse::movaps(c, 3, 1);
                let b = g.u32(c.ebp.wrapping_add(0x174))?;
                sse::ss(c, Mul, 3, b);
                sse::ss_reg(c, Sub, 3, 4);
                let b = g.u32(c.ebp.wrapping_add(0x178))?;
                sse::load(c, 4, b);
                sse::ss_reg(c, Mul, 4, 5);
                sse::ss_reg(c, Mul, 6, 5);
                sse::movaps(c, 7, 2);
                let b = g.u32(c.ebp.wrapping_add(0x174))?;
                sse::ss(c, Mul, 7, b);
                sse::ss_reg(c, Sub, 4, 7);
                let b = g.u32(c.ebp.wrapping_add(0x174))?;
                sse::load(c, 7, b);
                sse::ss_reg(c, Mul, 7, 0);
                sse::ss_reg(c, Add, 7, 6);
                sse::movaps(c, 6, 2);
                let b = g.u32(c.ebp.wrapping_add(0x17c))?;
                sse::ss(c, Mul, 6, b);
                sse::movaps(c, 5, 1);
                let b = g.u32(c.ebp.wrapping_add(0x178))?;
                sse::ss(c, Mul, 5, b);
                sse::ss_reg(c, Sub, 6, 5);
                let b = g.u32(c.ebp.wrapping_add(0x180))?;
                sse::load(c, 5, b);
                sse::ss_reg(c, Mul, 3, 5);
                sse::ss_reg(c, Mul, 6, 5);
                sse::ss_reg(c, Sub, 7, 6);
                let b = g.u32(c.ebp.wrapping_add(0x184))?;
                sse::load(c, 6, b);
                g.set_u32(c.esp.wrapping_add(0x30), sse::lane0(c, 7))?;
                sse::movaps(c, 7, 0);
                let b = g.u32(c.ebp.wrapping_add(0x178))?;
                sse::ss(c, Mul, 7, b);
                let b = g.u32(c.ebp.wrapping_add(0x17c))?;
                sse::ss(c, Mul, 0, b);
                sse::ss_reg(c, Mul, 4, 5);
                let b = g.u32(c.esp.wrapping_add(0x24))?;
                sse::load(c, 5, b);
                sse::ss_reg(c, Mul, 2, 6);
                sse::ss_reg(c, Add, 2, 7);
                sse::ss_reg(c, Sub, 2, 3);
                sse::ss_reg(c, Mul, 1, 6);
                sse::ss_reg(c, Add, 1, 0);
                sse::ss_reg(c, Sub, 1, 4);
                g.set_u32(c.esp.wrapping_add(0x34), sse::lane0(c, 2))?;
                let b = g.u32(c.esp.wrapping_add(0x28))?;
                sse::load(c, 2, b);
                g.set_u32(c.esp.wrapping_add(0x38), sse::lane0(c, 1))?;
                let b = g.u32(c.esp.wrapping_add(0x2c))?;
                sse::load(c, 1, b);
                sse::movaps(c, 0, 1);
                let b = g.u32(c.ebp.wrapping_add(0x17c))?;
                sse::ss(c, Mul, 0, b);
                sse::movaps(c, 3, 2);
                let b = g.u32(c.ebp.wrapping_add(0x178))?;
                sse::ss(c, Mul, 3, b);
                sse::ss_reg(c, Add, 0, 3);
                let b = g.u32(c.ebp.wrapping_add(0x174))?;
                sse::load(c, 3, b);
                sse::ss_reg(c, Mul, 3, 5);
                sse::ss_reg(c, Add, 0, 3);
                let b = g.u32(ONE)?;
                sse::load(c, 3, b);
                sse::ss_reg(c, Sub, 3, 6);
                sse::ss_reg(c, Mul, 0, 3);
                sse::movaps(c, 3, 1);
                let b = g.u32(c.ebp.wrapping_add(0x174))?;
                sse::ss(c, Mul, 3, b);
                sse::movaps(c, 4, 5);
                let b = g.u32(c.ebp.wrapping_add(0x17c))?;
                sse::ss(c, Mul, 4, b);
                sse::ss_reg(c, Sub, 3, 4);
                sse::movaps(c, 4, 5);
                let b = g.u32(c.ebp.wrapping_add(0x178))?;
                sse::ss(c, Mul, 4, b);
                sse::movaps(c, 7, 2);
                let b = g.u32(c.ebp.wrapping_add(0x174))?;
                sse::ss(c, Mul, 7, b);
                sse::ss_reg(c, Sub, 4, 7);
                let b = g.u32(c.ebp.wrapping_add(0x174))?;
                sse::load(c, 7, b);
                sse::ss_reg(c, Mul, 6, 5);
                sse::ss_reg(c, Mul, 7, 0);
                sse::ss_reg(c, Add, 7, 6);
                sse::movaps(c, 6, 2);
                let b = g.u32(c.ebp.wrapping_add(0x17c))?;
                sse::ss(c, Mul, 6, b);
                sse::movaps(c, 5, 1);
                let b = g.u32(c.ebp.wrapping_add(0x178))?;
                sse::ss(c, Mul, 5, b);
                sse::ss_reg(c, Sub, 6, 5);
                let b = g.u32(c.ebp.wrapping_add(0x180))?;
                sse::load(c, 5, b);
                sse::ss_reg(c, Mul, 6, 5);
                sse::ss_reg(c, Sub, 7, 6);
                let b = g.u32(c.ebp.wrapping_add(0x184))?;
                sse::load(c, 6, b);
                g.set_u32(c.esp.wrapping_add(0x24), sse::lane0(c, 7))?;
                sse::movaps(c, 7, 0);
                let b = g.u32(c.ebp.wrapping_add(0x178))?;
                sse::ss(c, Mul, 7, b);
                let b = g.u32(c.ebp.wrapping_add(0x17c))?;
                sse::ss(c, Mul, 0, b);
                sse::ss_reg(c, Mul, 2, 6);
                sse::ss_reg(c, Mul, 1, 6);
                sse::ss_reg(c, Add, 2, 7);
                sse::ss_reg(c, Mul, 3, 5);
                sse::ss_reg(c, Add, 1, 0);
                sse::ss_reg(c, Mul, 4, 5);
                sse::ss_reg(c, Sub, 2, 3);
                sse::ss_reg(c, Sub, 1, 4);
                g.set_u32(c.esp.wrapping_add(0x28), sse::lane0(c, 2))?;
                g.set_u32(c.esp.wrapping_add(0x2c), sse::lane0(c, 1))?;
                0xf9ef3
            }
            0xf9ef3 => {
                c.esi = g.u32(c.esp.wrapping_add(0x12c))?;
                push(c, g, 0)?;
                c.edx = c.esp.wrapping_add(0xec);
                let v = c.edx;
                push(c, g, v)?;
                let v = c.esi;
                push(c, g, v)?;
                c.edx = c.esp.wrapping_add(0x30);
                c.ecx = c.esp.wrapping_add(0x3c);
                c.eax = c.esp.wrapping_add(0x60);
                return Ok(Next::Call { target: OBJECT_SET_TRANSFORM, site: 0xf9f16 });
            }
            0xf9f16 => {
                push(c, g, 0)?;
                c.edi = 0;
                logic(c, 0, SIGN32);
                c.ecx = c.esp.wrapping_add(0x14);
                c.eax = c.esi;
                return Ok(Next::Call { target: OBJECT_SET_VELOCITY, site: 0xf9f25 });
            }
            0xf9f25 => {
                push(c, g, 1)?;
                let v = c.edi;
                push(c, g, v)?;
                c.ebx = c.esp.wrapping_add(0x18);
                c.eax = c.esi;
                return Ok(Next::Call { target: COMPONENT_SYNC, site: 0xf9f33 });
            }
            0xf9f33 => {
                let b = g.u32(c.esp.wrapping_add(0x18))?;
                sse::load(c, 0, b);
                sse::movaps(c, 1, 0);
                sse::ss_reg(c, Mul, 1, 0);
                let b = g.u32(c.esp.wrapping_add(0x14))?;
                sse::load(c, 0, b);
                sse::movaps(c, 2, 0);
                sse::ss_reg(c, Mul, 2, 0);
                let b = g.u32(c.esp.wrapping_add(0x10))?;
                sse::load(c, 0, b);
                sse::ss_reg(c, Add, 1, 2);
                sse::movaps(c, 2, 0);
                sse::ss_reg(c, Mul, 2, 0);
                sse::ss_reg(c, Add, 1, 2);
                let b = g.u32(EPSILON)?;
                sse::comiss(c, sse::lane0(c, 1), b);
                if cf(c) || zf(c) { 0xf9f81 } else { 0xf9f6b }
            }
            0xf9f6b => {
                let v = c.esi;
                push(c, g, v)?;
                c.edx = byte(c.edx, 0);
                logic(c, 0, 0x80);
                return Ok(Next::Call { target: OBJECT_MOVED, site: 0xf9f73 });
            }
            0xf9f73 => {
                c.eax = c.esi;
                return Ok(Next::Call { target: points::OBJECT_WAKE.base, site: 0xf9f7a });
            }
            0xf9f7a => {
                c.eax = c.esi;
                return Ok(Next::Call { target: points::OBJECT_WAKE_FOR_POST.base, site: 0xf9f81 });
            }
            0xf9f81 => {
                let b = g.u32(c.esp.wrapping_add(0x20))?;
                sse::load(c, 1, b);
                let b = g.u32(ZERO)?;
                sse::comiss(c, sse::lane0(c, 1), b);
                if cf(c) || zf(c) { 0xf9fc0 } else { 0xf9f90 }
            }
            0xf9f90 => {
                let (a, b) = (u32::from(g.u16(c.esp.wrapping_add(0x44))?), 0x0);
                alu::cmp16(c, a, b);
                if zf(c) { 0xf9fc0 } else { 0xf9f98 }
            }
            0xf9f98 => {
                c.eax = c.esi;
                return Ok(Next::Call { target: ATTACHMENT, site: 0xf9f9f });
            }
            0xf9f9f => {
                let (a, b) = (c.eax, NONE);
                arithmetic(c, a, b, true);
                if zf(c) { 0xf9fc0 } else { 0xf9fa4 }
            }
            0xf9fa4 => {
                let b = g.u32(c.esp.wrapping_add(0xa8))?;
                sse::load(c, 0, b);
                let v = c.ecx;
                push(c, g, v)?;
                sse::ss_reg(c, Sub, 0, 1);
                g.set_u32(c.esp, sse::lane0(c, 0))?;
                push(c, g, 0)?;
                c.esi = c.eax;
                return Ok(Next::Call { target: ATTACHMENT_UPDATE, site: 0xf9fc0 });
            }
            0xf9fc0 => {
                sse::zero(c, 3);
                0xf9fc3
            }
            0xf9fc3 => {
                let b = g.u32(c.esp.wrapping_add(0x20))?;
                sse::load(c, 0, b);
                let b = sse::lane0(c, 3);
                sse::comiss(c, sse::lane0(c, 0), b);
                if !(cf(c) || zf(c)) { 0xf9024 } else { 0xf9fd2 }
            }
            0xf9fd2 => {
                c.eax = u32::from(g.u16(c.ebp.wrapping_add(0x130))?) as u16 as i16 as i32 as u32;
                let v = c.eax;
                c.eax = alu::dec32(c, v);
                if zf(c) { 0xfa055 } else { 0xf9fdc }
            }
            0xf9fdc => {
                let v = c.eax;
                c.eax = alu::dec32(c, v);
                if !zf(c) { 0xfa0ed } else { 0xf9fe3 }
            }
            0xf9fe3 => {
                c.esi = g.u32(c.esp.wrapping_add(0x12c))?;
                0xf9fea
            }
            0xf9fea => {
                c.eax = g.u32(points::GAME_GLOBALS.base)?;
                let (a, b) = (u32::from(g.u8(c.eax.wrapping_add(0xc))?), 0x4);
                cmp8(c, a, b);
                if !zf(c) { 0xfa0e7 } else { 0xf9ff9 }
            }
            0xf9ff9 => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(c.ecx.wrapping_add(0x44))?;
                c.eax = g.u32(c.esp.wrapping_add(0x7c))?;
                c.edi = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(0x8))?;
                let (a, b) = (g.u32(c.edi.wrapping_add(0xd4))?, NONE);
                arithmetic(c, a, b, true);
                c.eax = byte(c.eax, u8::from(!zf(c)));
                let v = c.eax & 0xff;
                logic(c, v, 0x80);
                if zf(c) { 0xfa0e7 } else { 0xfa01c }
            }
            0xfa01c => {
                let v = u32::from(g.u8(c.edi.wrapping_add(0x4))?) & 0x1;
                logic(c, v, 0x80);
                if !zf(c) { 0xfa0ed } else { 0xfa026 }
            }
            0xfa026 => {
                c.eax = c.esi;
                return Ok(Next::Call { target: NEEDS_REFRESH, site: 0xfa02d });
            }
            0xfa02d => {
                let v = c.eax & 0xff;
                logic(c, v, 0x80);
                if zf(c) { 0xfa03f } else { 0xfa031 }
            }
            0xfa031 => {
                push(c, g, 0)?;
                push(c, g, 0)?;
                push(c, g, 0)?;
                push(c, g, 1)?;
                let v = c.esi;
                push(c, g, v)?;
                return Ok(Next::Call { target: ATTACHMENT_REFRESH, site: 0xfa03f });
            }
            0xfa03f => {
                let v = g.u32(c.edi.wrapping_add(0x4))? | 0x1;
                g.set_u32(c.edi.wrapping_add(0x4), v)?;
                logic(c, v, SIGN32);
                return Ok(Next::Call { target: REFRESH_DONE, site: 0xfa048 });
            }
            0xfa048 => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                let v = c.esp;
                c.esp = arithmetic(c, v, 0x118, false);
                return Ok(Next::Ret { pop: 8 });
            }
            0xfa055 => {
                let b = g.u32(c.ebp.wrapping_add(0x164))?;
                sse::load(c, 0, b);
                let b = sse::lane0(c, 3);
                sse::ucomiss(c, sse::lane0(c, 0), b);
                sse::lahf(c);
                let v = ((c.eax >> 8) & 0xff) & 0x44;
                logic(c, v, 0x80);
                if !pf(c) { 0xfa077 } else { 0xfa066 }
            }
            0xfa066 => {
                let b = g.u32(ONE)?;
                sse::load(c, 0, b);
                let b = g.u32(c.ebp.wrapping_add(0x160))?;
                sse::comiss(c, sse::lane0(c, 0), b);
                if !(cf(c) || zf(c)) { 0xfa0ed } else { 0xfa077 }
            }
            0xfa077 => {
                c.eax = g.u32(points::GAME_GLOBALS.base)?;
                let (a, b) = (u32::from(g.u8(c.eax.wrapping_add(0xc))?), 0x4);
                cmp8(c, a, b);
                if !zf(c) { 0xfa0c6 } else { 0xfa082 }
            }
            0xfa082 => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(c.ecx.wrapping_add(0x44))?;
                c.eax = g.u32(c.esp.wrapping_add(0x7c))?;
                c.eax = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(0x8))?;
                let (a, b) = (g.u32(c.eax.wrapping_add(0xd4))?, NONE);
                arithmetic(c, a, b, true);
                c.ecx = byte(c.ecx, u8::from(!zf(c)));
                let v = c.ecx & 0xff;
                logic(c, v, 0x80);
                if zf(c) { 0xfa0c6 } else { 0xfa0a1 }
            }
            0xfa0a1 => {
                c.esi = g.u32(c.esp.wrapping_add(0x12c))?;
                c.ecx = 0x2;
                let (a, b) = (u32::from(g.u16(c.eax.wrapping_add(0x130))?), c.ecx & 0xffff);
                alu::cmp16(c, a, b);
                if sf(c) == of(c) { 0xf9fea } else { 0xfa0ba }
            }
            0xfa0ba => {
                g.set_u16(c.eax.wrapping_add(0x130), c.ecx as u16)?;
                0xf9fea
            }
            0xfa0c6 => {
                let (a, b) = (u32::from(g.u16(c.esp.wrapping_add(0x44))?), 0x0);
                alu::cmp16(c, a, b);
                c.ecx = g.u32(c.esp.wrapping_add(0x20))?;
                c.esi = g.u32(c.esp.wrapping_add(0x12c))?;
                c.edx = byte(c.edx, u8::from(zf(c)));
                let v = c.ecx;
                push(c, g, v)?;
                let v = c.edx;
                push(c, g, v)?;
                let v = c.esi;
                push(c, g, v)?;
                return Ok(Next::Call { target: DETONATE, site: 0xfa0e2 });
            }
            0xfa0e2 => {
                0xf9fea
            }
            0xfa0e7 => {
                let v = c.esi;
                push(c, g, v)?;
                return Ok(Next::Call { target: points::OBJECT_DELETE.base, site: 0xfa0ed });
            }
            0xfa0ed => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                let v = c.esp;
                c.esp = arithmetic(c, v, 0x118, false);
                return Ok(Next::Ret { pop: 8 });
            }
            _ => return Err(unowned(next)),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sites_are_sorted_distinct_and_inside_the_function() {
        assert!(STEP_SITES.iter().all(|&s| s > ENTRY && s < ENTRY + 0x1247));
        assert!(STEP_SITES.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn inc_keeps_carry_and_sets_overflow_at_the_sign() {
        let mut c = CpuAbi { eflags: 1, ..Default::default() };
        assert_eq!(inc(&mut c, 0x7fff, 16), 0x8000);
        assert_eq!(c.eflags & 0x881, 0x881);
        assert_eq!(inc(&mut c, 0xffff_ffff, 32), 0);
        assert_eq!(c.eflags & 0x41, 0x41);
    }

    #[test]
    fn cmp8_sees_the_low_byte() {
        let mut c = CpuAbi::default();
        cmp8(&mut c, 0x1204, 4);
        assert!(zf(&c));
        cmp8(&mut c, 3, 4);
        assert!(cf(&c) && sf(&c));
    }
}
