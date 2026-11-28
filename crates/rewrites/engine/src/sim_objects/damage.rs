//! The object damage update `d5de0` (docs/engine/object-update.md, "Slice 8
//! body"), and the tick's `Damage` and `Kill` events.
//!
//! `d5de0 (object: stack 0) -> al`, `ret 4`, runs once per awake object
//! from the pre-update `bc470`: the damage sections' timers, pending
//! damage (`dbfb0`) and a pending kill (`d66d0`, `d6800`), the shield's
//! recharge, drain and overshield, the body's regeneration, the section
//! damage decay, and the recent-damage accumulators (`d8b50`). It is a
//! segment body: every guest call it makes is a callout with retail's
//! frame, registers, flags, XMM and x87 state, so the lifted original can
//! take over at any site and the reverse. The body is the retail code
//! block by block, keyed by retail address; a block that branches names
//! the next block.
//!
//! The function applies no damage itself: damage reaches objects through
//! the damage report `d9640` and kills are credited at `15ce70`, whoever
//! calls them (`dbfb0` from here, projectiles, melee, explosions). So the
//! events are recorded at those two functions' entries
//! ([`damage_report`], [`player_killed`], from `sim_bridge::sim_objects`'
//! observers), which makes them the same events the existing hooks there
//! see.
//!
//! The Halo 1 health model hooks this point (`before` and `after`); the
//! installer then guards the body ([`super::Table::guard`]), so the chain
//! runs those hooks around it as around the original.

use super::{Cx, Next, Result, unowned};
use crate::light_constants::X87;
use guest_layout::memory::GuestMemory;
use guest_layout::points;
use crate::sim_physics::{ARITH, byte, logic, pop, push, status};
use ::sim_objects::events::{self, SimEvent};
use guest_layout::Datum;
use runtime::guest::{Access, FaultReason};
use runtime::{CpuAbi, Guest, GuestFault};

/// The entry.
pub(super) const ENTRY: u32 = 0xd5de0;

/// The return sites of its guest calls, in address order.
pub(super) const SITES: [u32; 26] = [
    0xd5e0d, 0xd5f27, 0xd5f57, 0xd5f78, 0xd5f8c, 0xd5fdd, 0xd600b, 0xd601a, 0xd6077, 0xd60ca, 0xd61c3,
    0xd6240, 0xd624c, 0xd62c4, 0xd62f6, 0xd631f, 0xd633e, 0xd638e, 0xd63a4, 0xd63ae, 0xd63d9, 0xd6450,
    0xd649b, 0xd64b5, 0xd65c1, 0xd663d,
];

/// Its callees, every one a callout (docs/engine/object-update.md, "Slice
/// 8 body").
pub(super) mod callee {
    /// `(object: ecx) -> eax`: the object's damage information, or 0.
    pub const INFO: u32 = 0xd5b60;
    pub const SECTION_CHECK: u32 = 0xd9f70;
    pub const SECTION_DAMAGE: u32 = 0xda110;
    pub const SECTION_EFFECTS: u32 = 0xdbc80;
    pub const SECTION_DESTROYED: u32 = 0xd6bc0;
    /// The actor's record for a unit with an actor.
    pub const ACTOR: u32 = 0x1e4a10;
    /// Builds the damage owner of a pending kill.
    pub const OWNER: u32 = 0xd66d0;
    /// The body-depleted transition.
    pub const DEPLETE: u32 = 0xd6800;
    /// Pending damage: reaches the damage report through `d7b80`.
    pub const PENDING: u32 = 0xdbfb0;
    /// A player's shield recharge rate.
    pub const PLAYER_RATE: u32 = 0x1588b0;
    pub const OVERSHIELD_OWNER: u32 = 0x14de90;
    pub const OVERSHIELD_DRAIN: u32 = 0x13a6e8;
    /// Marks the object's state for replication.
    pub const REPLICATE: u32 = 0xa7a30;
    /// A team's damage rate.
    pub const TEAM_RATE: u32 = 0x1e9720;
    pub const SHIELD_EFFECT: u32 = 0xba7f0;
    pub const RECHARGE_EFFECT: u32 = 0x176780;
    /// Whether the stun timers hold.
    pub const STUN_HOLD: u32 = 0x138880;
    /// Decays a recent-damage accumulator pair.
    pub const RECENT_DAMAGE: u32 = 0xd8b50;
}

/// The Black Eye skull's byte.
const BLACK_EYE: u32 = 0x004f_55e4;
/// Image constants, read where retail reads them.
const ONE: u32 = 0x0045_dbc0;
const THRESHOLD: u32 = 0x0045_dbd8;
const RATE_SCALE: u32 = 0x0045_dc0c;
/// The overshield's cap.
const MAX_OVERSHIELD: u32 = 0x0045_dbd4;
const DRAIN_SCALE: u32 = 0x0045_e320;
const BYTE_SCALE: u32 = 0x0045_dc38;

/// `fistp dword [esp+1Ch]` at `d6555`: the x87 state's last instruction.
const FISTP_AT: u32 = 0xd6555;

const NONE: u32 = u32::MAX;
const CF: u32 = 0x1;
const ZF: u32 = 0x40;
const AF: u32 = 0x10;
const PF: u32 = 0x4;
const SF: u32 = 0x80;
const OF: u32 = 0x800;
/// The direction flag.
const DF: u32 = 0x400;

fn fp_fault() -> GuestFault {
    GuestFault { addr: 0, len: 0, access: Access::Execute, reason: FaultReason::InvalidOperation }
}

fn s(c: &CpuAbi, off: u32) -> u32 {
    c.esp.wrapping_add(off)
}

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

fn mask(bits: u32) -> u32 {
    if bits == 32 { u32::MAX } else { (1 << bits) - 1 }
}

/// `test`/`and`/`or`/`xor` of `bits` width on `v`.
fn tst(c: &mut CpuAbi, v: u32, bits: u32) {
    logic(c, v & mask(bits), 1 << (bits - 1));
}

/// `add` or `sub`/`cmp` of `bits` width: the result and every arithmetic
/// flag.
fn arith(c: &mut CpuAbi, a: u32, b: u32, sub: bool, bits: u32) -> u32 {
    let (m, sign) = (mask(bits), 1u32 << (bits - 1));
    let (a, b) = (a & m, b & m);
    let wide = if sub { u64::from(a).wrapping_sub(u64::from(b)) } else { u64::from(a) + u64::from(b) };
    let v = wide as u32 & m;
    let carry = if sub { a < b } else { wide > u64::from(m) };
    let overflow = (if sub { a ^ b } else { !(a ^ b) }) & (a ^ v) & sign != 0;
    c.eflags = (c.eflags & !ARITH) | status(v, sign) | u32::from(carry) | ((a ^ b ^ v) & AF) | u32::from(overflow) << 11;
    c.eflags_defined |= ARITH;
    v
}

/// `inc` or `dec` of `bits` width: carry kept.
fn incdec(c: &mut CpuAbi, a: u32, dec: bool, bits: u32) -> u32 {
    let (m, sign) = (mask(bits), 1u32 << (bits - 1));
    let a = a & m;
    let v = if dec { a.wrapping_sub(1) } else { a.wrapping_add(1) } & m;
    let of = if dec { a == sign } else { v == sign };
    let flags = status(v, sign) | ((a ^ 1 ^ v) & AF) | u32::from(of) << 11;
    c.eflags = (c.eflags & !(ARITH & !CF)) | flags;
    c.eflags_defined |= ARITH & !CF;
    v
}

/// A shift's flags: carry the last bit out, overflow defined only for a
/// count of one, auxiliary carry undefined.
fn shift_flags(c: &mut CpuAbi, v: u32, cf: u32, of: Option<bool>, bits: u32) {
    let mut flags = status(v, 1 << (bits - 1)) | cf;
    let mut defined = CF | PF | ZF | SF;
    if let Some(o) = of {
        flags |= u32::from(o) << 11;
        defined |= OF;
    }
    c.eflags = (c.eflags & !ARITH) | flags;
    c.eflags_defined = (c.eflags_defined & !ARITH) | defined;
}

/// `shr` of `bits` width by a constant `n` (1 to `bits - 1`).
fn shr(c: &mut CpuAbi, a: u32, n: u32, bits: u32) -> u32 {
    let a = a & mask(bits);
    let v = a >> n;
    shift_flags(c, v, (a >> (n - 1)) & 1, (n == 1).then_some(a >> (bits - 1) & 1 != 0), bits);
    v
}

/// `shl r32` by `n` (1 to 31).
fn shl(c: &mut CpuAbi, a: u32, n: u32) -> u32 {
    let v = a << n;
    let cf = (a >> (32 - n)) & 1;
    shift_flags(c, v, cf, (n == 1).then_some((v >> 31) ^ cf != 0), 32);
    v
}

/// `shl r32, cl`: a zero count changes nothing.
fn shl_cl(c: &mut CpuAbi, a: u32) -> u32 {
    match c.ecx & 31 {
        0 => a,
        n => shl(c, a, n),
    }
}

/// `imul r32, r32, imm32`: carry and overflow when the product does not
/// fit; the other flags undefined.
fn imul(c: &mut CpuAbi, a: u32, b: i32) -> u32 {
    let wide = i64::from(a as i32) * i64::from(b);
    let v = wide as i32;
    let over = i64::from(v) != wide;
    c.eflags = (c.eflags & !ARITH) | if over { CF | OF } else { 0 };
    c.eflags_defined = (c.eflags_defined | CF | OF) & !(SF | ZF | AF | PF);
    v as u32
}

/// `and byte [addr], imm`.
fn and8(c: &mut CpuAbi, g: &mut Guest, addr: u32, imm: u8) -> Result<()> {
    let v = g.u8(addr)? & imm;
    g.set_u8(addr, v)?;
    tst(c, v.into(), 8);
    Ok(())
}

/// `or byte [addr], imm`.
fn or8(c: &mut CpuAbi, g: &mut Guest, addr: u32, imm: u8) -> Result<()> {
    let v = g.u8(addr)? | imm;
    g.set_u8(addr, v)?;
    tst(c, v.into(), 8);
    Ok(())
}

/// `rep stosd`.
fn stosd(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let step = if c.eflags & DF != 0 { 4u32.wrapping_neg() } else { 4 };
    while c.ecx != 0 {
        g.set_u32(c.edi, c.eax)?;
        c.edi = c.edi.wrapping_add(step);
        c.ecx -= 1;
    }
    Ok(())
}

fn sx16(v: u16) -> u32 {
    v as i16 as i32 as u32
}

fn low16(word: u32, v: u16) -> u32 {
    (word & !0xffff) | u32::from(v)
}

/// XMM lane 0.
fn low(c: &CpuAbi, r: usize) -> u32 {
    u32::from_le_bytes([c.xmm[r][0], c.xmm[r][1], c.xmm[r][2], c.xmm[r][3]])
}

/// `movss xmm, m32`: the upper lanes cleared.
fn load(c: &mut CpuAbi, r: usize, v: u32) {
    c.xmm[r] = u128::from(v).to_le_bytes();
}

type Op = fn(&mut u32, u128, u32) -> std::result::Result<u128, simd::Xm>;

/// A scalar SSE operation into `xmm[r]` under the guest's MXCSR.
fn sse(c: &mut CpuAbi, r: usize, b: u32, op: Op) -> Result<()> {
    let a = u128::from_le_bytes(c.xmm[r]);
    c.xmm[r] = op(&mut c.mxcsr, a, b).map_err(|_| fp_fault())?.to_le_bytes();
    Ok(())
}

/// `cvtsi2ss xmm[r], i`.
fn cvtsi2ss(c: &mut CpuAbi, r: usize, i: u32) -> Result<()> {
    let a = u128::from_le_bytes(c.xmm[r]);
    c.xmm[r] = simd::cvtsi2ss(&mut c.mxcsr, a, i).map_err(|_| fp_fault())?.to_le_bytes();
    Ok(())
}

/// `comiss a, b`: the flags it sets.
fn comiss(c: &mut CpuAbi, a: u32, b: u32) -> Result<u32> {
    let f = simd::comiss(&mut c.mxcsr, a, b).map_err(|_| fp_fault())?;
    c.eflags = (c.eflags & !ARITH) | f;
    c.eflags_defined |= ARITH;
    Ok(f)
}

/// `jbe` after a compare.
fn jbe(f: u32) -> bool {
    f & (CF | ZF) != 0
}

/// `fld dword [src]; fistp dword [dst]` under the guest's x87 control
/// word, leaving the x87 state as the two instructions do.
fn fld_fistp(c: &mut CpuAbi, g: &mut Guest, src: u32, dst: u32) -> Result<()> {
    let bits = g.u32(src)?;
    let x = &c.x87;
    let mut x = X87(x87::abi::X87Abi {
        st: x.st,
        control: x.control,
        status: x.status,
        tag: x.tag,
        opcode: x.opcode,
        instruction_pointer: x.instruction_pointer,
        data_pointer: x.data_pointer,
        instruction_selector: x.instruction_selector,
        data_selector: x.data_selector,
    });
    x.fld(bits).map_err(|_| fp_fault())?;
    let (code, store) = x87::abi::st_i32(&x.0, 0);
    X87::ok(code).map_err(|_| fp_fault())?;
    x87::abi::store_commit(&mut x.0, store.status, true);
    g.set_u32(dst, store.value as u32)?;
    // FOP: the low three opcode bits and the ModR/M byte, as mapped.
    let [op, modrm] = g.read::<2>(FISTP_AT)?;
    c.x87.st = x.0.st;
    c.x87.status = x.0.status;
    c.x87.tag = x.0.tag;
    c.x87.instruction_pointer = FISTP_AT;
    c.x87.instruction_selector = 8;
    c.x87.opcode = (u16::from(op & 7) << 8) | u16::from(modrm);
    c.x87.data_pointer = dst;
    c.x87.data_selector = 16;
    Ok(())
}

/// `d664f`: no damage information; `al` is 0 (`dl`).
fn leave_early(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.eax = byte(c.eax, c.edx as u8);
    c.ebx = pop(c, g)?;
    c.esp = arith(c, c.esp, 0x70, false, 32);
    Ok(Next::Ret { pop: 4 })
}

/// `d5de0 (object: stack 0) -> al`.
///
/// Frame, with `esp` after the four pushes (`S`): `S+13` the "changed"
/// byte (the result), `S+14` the actor's record (0 without), `S+18` the
/// damage information, `S+1c` and `S+20` the sections cursor and the
/// header offset, then float temporaries; `S+84` the object. `ebp` is the
/// body throughout, read once at entry as retail does.
pub(super) fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != ENTRY && !SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let objects = points::OBJECTS.base;
    let game_globals = points::GAME_GLOBALS.base;
    let game_time = points::GAME_TIME.base;
    let mut at = pc;
    loop {
        at = match at {
            ENTRY => {
                c.esp = arith(c, c.esp, 0x70, true, 32);
                c.ecx = g.u32(s(c, 0x74))?;
                c.edx = g.u32(objects)?;
                c.edx = g.u32(c.edx.wrapping_add(0x44))?;
                c.eax = c.ecx & 0xffff;
                tst(c, c.eax, 32);
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                c.eax = c.eax.wrapping_mul(3);
                c.eax = shl(c, c.eax, 2);
                c.ebp = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(8))?;
                push(c, g, c.esi)?;
                g.set_u32(s(c, 0x1c), c.eax)?;
                return call(callee::INFO, 0xd5e0d);
            }
            0xd5e0d => {
                c.ecx = byte(c.ecx, g.u8(c.ebp.wrapping_add(0xaa))?);
                c.ebx = shl_cl(c, 1);
                c.edx = byte(c.edx, 0);
                tst(c, 0, 8);
                g.set_u32(s(c, 0x14), c.eax)?;
                g.set_u8(s(c, 0xf), c.edx as u8)?;
                tst(c, c.ebx & 3, 8);
                if c.ebx & 3 != 0 {
                    // The Black Eye skull holds a player's shield in the
                    // campaign.
                    c.ecx = g.u32(game_globals)?;
                    let mode = g.u32(c.ecx.wrapping_add(8))?;
                    arith(c, mode, 1, true, 32);
                    if mode == 1 {
                        c.ecx = byte(c.ecx, g.u8(BLACK_EYE)?);
                        tst(c, c.ecx, 8);
                        if c.ecx & 0xff != 0 {
                            let player = g.u32(c.ebp.wrapping_add(0x13c))?;
                            arith(c, player, NONE, true, 32);
                            if player != NONE {
                                g.set_u16(c.ebp.wrapping_add(0x104), 0xa)?;
                            }
                        }
                    }
                }
                tst(c, c.eax, 32);
                if c.eax == 0 {
                    return leave_early(c, g);
                }
                c.ecx = g.u32(c.eax.wrapping_add(0xbc))?;
                tst(c, c.ecx, 32);
                push(c, g, c.edi)?;
                g.set_u32(s(c, 0x14), 0)?;
                if c.ecx as i32 <= 0 {
                    0xd5fa4
                } else {
                    c.eax = sx16(g.u16(c.ebp.wrapping_add(0x120))?);
                    c.ecx = sx16(g.u16(c.ebp.wrapping_add(0x122))?);
                    c.eax = shr(c, c.eax, 3, 32);
                    c.ecx = arith(c, c.ecx, c.ebp, false, 32);
                    c.ebx = 0;
                    tst(c, 0, 32);
                    tst(c, c.eax, 32);
                    g.set_u32(s(c, 0x24), c.eax)?;
                    if c.eax as i32 <= 0 {
                        0xd5fa4
                    } else {
                        c.ecx = arith(c, c.ecx, 4, false, 32);
                        g.set_u32(s(c, 0x1c), c.ecx)?;
                        0xd5ea0
                    }
                }
            }
            // The sections loop: `ebx` the index, `[S+1c]` the record.
            0xd5ea0 => {
                c.edx = g.u32(s(c, 0x1c))?;
                c.ecx = 0;
                tst(c, 0, 32);
                c.ecx = low16(c.ecx, g.u16(c.edx)?);
                c.eax = 0;
                tst(c, 0, 32);
                c.eax = low16(c.eax, c.ecx as u16);
                let ax = shr(c, c.eax, 3, 16);
                c.eax = low16(c.eax, ax as u16);
                tst(c, ax, 16);
                if ax == 0 {
                    0xd5f8c
                } else {
                    c.ecx &= 7;
                    tst(c, c.ecx, 32);
                    c.eax = c.eax.wrapping_mul(8).wrapping_sub(8);
                    c.eax |= c.ecx;
                    tst(c, c.eax, 32);
                    let held = c.eax & 0xfff8;
                    tst(c, held, 32);
                    c.ecx = c.edx;
                    g.set_u8(s(c, 0x13), 1)?;
                    g.set_u16(c.ecx, c.eax as u16)?;
                    if held != 0 {
                        0xd5f8c
                    } else {
                        c.edx = g.u32(objects)?;
                        c.ecx = g.u32(c.edx.wrapping_add(0x44))?;
                        c.edx = g.u32(s(c, 0x20))?;
                        c.ecx = g.u32(c.ecx.wrapping_add(c.edx).wrapping_add(8))?;
                        c.edx = g.u32(c.ecx.wrapping_add(0xc8))?;
                        g.set_u32(s(c, 0x2c), c.edx)?;
                        c.edx = g.u32(c.ecx.wrapping_add(0xc4))?;
                        g.set_u32(s(c, 0x28), c.edx)?;
                        c.ecx = low16(c.ecx, g.u16(c.ecx.wrapping_add(0xc2))?);
                        c.edx = g.u32(s(c, 0x84))?;
                        c.esi = c.eax;
                        g.set_u16(s(c, 0x30), c.ecx as u16)?;
                        c.esi &= 7;
                        tst(c, c.esi, 32);
                        push(c, g, c.edx)?;
                        c.edx = g.u32(s(c, 0x1c))?;
                        c.ecx = c.esi;
                        c.eax = c.ebx;
                        return call(callee::SECTION_CHECK, 0xd5f27);
                    }
                }
            }
            0xd5f27 => {
                tst(c, c.eax, 8);
                if c.eax & 0xff == 0 {
                    0xd5f8c
                } else {
                    c.edx = g.u32(s(c, 0x84))?;
                    c.eax = 0;
                    tst(c, 0, 32);
                    c.ecx = 0xd;
                    c.edi = s(c, 0x4c);
                    stosd(c, g)?;
                    c.eax = s(c, 0x4c);
                    push(c, g, c.eax)?;
                    c.eax = g.u32(s(c, 0x1c))?;
                    push(c, g, c.ebx)?;
                    c.ecx = s(c, 0x30);
                    push(c, g, c.ecx)?;
                    push(c, g, c.edx)?;
                    push(c, g, c.eax)?;
                    c.eax = c.esi;
                    return call(callee::SECTION_DAMAGE, 0xd5f57);
                }
            }
            0xd5f57 => {
                c.eax = g.u32(s(c, 0x78))?;
                tst(c, c.eax, 16);
                c.ecx = g.u32(s(c, 0x7a))?;
                let effects = c.eax & 0xffff != 0 || {
                    tst(c, c.ecx, 16);
                    c.ecx & 0xffff != 0
                };
                if effects {
                    push(c, g, c.ecx)?;
                    c.ecx = g.u32(s(c, 0x88))?;
                    push(c, g, c.eax)?;
                    push(c, g, c.ecx)?;
                    return call(callee::SECTION_EFFECTS, 0xd5f78);
                }
                0xd5f78
            }
            0xd5f78 => {
                let destroyed = g.u8(s(c, 0x7c))? & 4;
                tst(c, destroyed.into(), 8);
                if destroyed == 0 {
                    0xd5f8c
                } else {
                    c.edx = g.u32(s(c, 0x84))?;
                    push(c, g, c.edx)?;
                    return call(callee::SECTION_DESTROYED, 0xd5f8c);
                }
            }
            0xd5f8c => {
                c.ecx = g.u32(s(c, 0x1c))?;
                c.eax = g.u32(s(c, 0x24))?;
                c.ebx = incdec(c, c.ebx, false, 32);
                c.ecx = arith(c, c.ecx, 8, false, 32);
                arith(c, c.ebx, c.eax, true, 32);
                g.set_u32(s(c, 0x1c), c.ecx)?;
                if (c.ebx as i32) < (c.eax as i32) { 0xd5ea0 } else { 0xd5fa4 }
            }
            0xd5fa4 => {
                c.ecx = byte(c.ecx, g.u8(c.ebp.wrapping_add(0xaa))?);
                c.eax = shl_cl(c, 1);
                tst(c, c.eax & 3, 8);
                let mut actor = c.eax & 3 != 0;
                if actor {
                    c.eax = g.u32(c.ebp.wrapping_add(0x12c))?;
                    arith(c, c.eax, NONE, true, 32);
                    actor = c.eax != NONE;
                }
                if actor {
                    c.ecx = g.u32(points::ACTORS.base)?;
                    c.edx = g.u32(c.ecx.wrapping_add(0x44))?;
                    c.eax &= 0xffff;
                    tst(c, c.eax, 32);
                    c.eax = imul(c, c.eax, 0x888);
                    c.ecx = g.u32(c.eax.wrapping_add(c.edx).wrapping_add(0x54))?;
                    return call(callee::ACTOR, 0xd5fdd);
                }
                c.ebx = g.u32(s(c, 0x14))?;
                0xd5fe9
            }
            0xd5fdd => {
                c.ebx = c.eax;
                g.set_u32(s(c, 0x14), c.ebx)?;
                0xd5fe9
            }
            // A pending kill (damage flags bit 13).
            0xd5fe9 => {
                c.edi = g.u32(s(c, 0x84))?;
                c.eax = 0;
                tst(c, 0, 32);
                c.eax = u32::from(g.u16(c.ebp.wrapping_add(0x10a))?);
                c.eax = shr(c, c.eax, 0xd, 32);
                tst(c, c.eax & 1, 8);
                if c.eax & 1 == 0 {
                    0xd6021
                } else {
                    c.ecx = s(c, 0x34);
                    push(c, g, c.ecx)?;
                    push(c, g, c.edi)?;
                    return call(callee::OWNER, 0xd600b);
                }
            }
            0xd600b => {
                push(c, g, 0)?;
                push(c, g, 1)?;
                c.edx = s(c, 0x3c);
                push(c, g, c.edx)?;
                push(c, g, c.edi)?;
                return call(callee::DEPLETE, 0xd601a);
            }
            0xd601a => {
                and8(c, g, c.ebp.wrapping_add(0x10b), 0xdf)?;
                0xd6021
            }
            // Pending damage (bits 5, 6 and 10), outside a distributed
            // client.
            0xd6021 => {
                c.eax = low16(c.eax, g.u16(c.ebp.wrapping_add(0x10a))?);
                c.ecx = byte(c.ecx, c.eax as u8);
                let cl = shr(c, c.ecx, 5, 8);
                c.ecx = byte(c.ecx, cl as u8);
                tst(c, cl & 1, 8);
                let mut pending = cl & 1 != 0;
                if !pending {
                    c.edx = byte(c.edx, c.eax as u8);
                    let dl = shr(c, c.edx, 6, 8);
                    c.edx = byte(c.edx, dl as u8);
                    tst(c, dl & 1, 8);
                    pending = dl & 1 != 0;
                }
                if !pending {
                    c.ecx = c.eax;
                    c.ecx = shr(c, c.ecx, 0xa, 32);
                    tst(c, c.ecx & 1, 8);
                    pending = c.ecx & 1 != 0;
                }
                if !pending {
                    0xd6089
                } else {
                    c.edx = g.u32(game_globals)?;
                    let mode = g.u8(c.edx.wrapping_add(0xc))?;
                    arith(c, mode.into(), 4, true, 8);
                    if mode == 4 {
                        0xd607b
                    } else {
                        c.edi = g.u32(s(c, 0x84))?;
                        c.ecx = c.eax;
                        let al = shr(c, c.eax, 6, 8);
                        c.eax = byte(c.eax, al as u8);
                        c.ecx = shr(c, c.ecx, 0xa, 32);
                        push(c, g, 0)?;
                        c.ecx &= 0xffff_ff01;
                        tst(c, c.ecx, 32);
                        push(c, g, c.ecx)?;
                        c.esi = 0;
                        tst(c, 0, 32);
                        c.eax &= 0xffff_ff01;
                        tst(c, c.eax, 32);
                        push(c, g, c.eax)?;
                        return call(callee::PENDING, 0xd6077);
                    }
                }
            }
            0xd6077 => {
                c.ebx = g.u32(s(c, 0x14))?;
                0xd607b
            }
            0xd607b => {
                and8(c, g, c.ebp.wrapping_add(0x10a), 0x9f)?;
                and8(c, g, c.ebp.wrapping_add(0x10b), 0xfb)?;
                0xd6089
            }
            // The shield's ceiling: 1, or a player's rate in a synchronous
            // client.
            0xd6089 => {
                c.edx = g.u32(game_globals)?;
                c.esi = g.u32(c.edx.wrapping_add(8))?;
                arith(c, c.esi, 2, true, 32);
                load(c, 3, g.u32(ONE)?);
                c.xmm[2] = c.xmm[3];
                g.set_u32(s(c, 0x1c), low(c, 2))?;
                if c.esi == 2 {
                    c.ecx = byte(c.ecx, g.u8(c.ebp.wrapping_add(0xaa))?);
                    c.eax = shl_cl(c, 1);
                    tst(c, c.eax & 3, 8);
                    if c.eax & 3 != 0 {
                        c.ecx = g.u32(c.ebp.wrapping_add(0x13c))?;
                        arith(c, c.ecx, NONE, true, 32);
                        if c.ecx != NONE {
                            push(c, g, c.esi)?;
                            return call(callee::PLAYER_RATE, 0xd60ca);
                        }
                    }
                }
                0xd60d3
            }
            0xd60ca => {
                g.set_u32(s(c, 0x1c), low(c, 0))?;
                c.xmm[2] = c.xmm[0];
                0xd60d3
            }
            // The overshield: charging (bit 4) or draining.
            0xd60d3 => {
                c.eax = low16(c.eax, g.u16(c.ebp.wrapping_add(0x10a))?);
                c.ecx = byte(c.ecx, c.eax as u8);
                let cl = shr(c, c.ecx, 2, 8);
                c.ecx = byte(c.ecx, cl as u8);
                tst(c, cl & 1, 8);
                if cl & 1 != 0 {
                    0xd6151
                } else {
                    load(c, 0, g.u32(c.ebp.wrapping_add(0xe8))?);
                    let f = comiss(c, low(c, 0), g.u32(THRESHOLD)?)?;
                    if jbe(f) {
                        0xd6151
                    } else {
                        let al = shr(c, c.eax, 4, 8);
                        c.eax = byte(c.eax, al as u8);
                        tst(c, al & 1, 8);
                        if al & 1 == 0 {
                            0xd61a2
                        } else {
                            c.edx = g.u32(game_time)?;
                            c.eax = sx16(g.u16(c.edx.wrapping_add(2))?);
                            load(c, 0, g.u32(RATE_SCALE)?);
                            cvtsi2ss(c, 1, c.eax)?;
                            sse(c, 1, low(c, 0), simd::mulss)?;
                            sse(c, 0, low(c, 1), simd::divss)?;
                            sse(c, 0, g.u32(c.ebp.wrapping_add(0xf0))?, simd::addss)?;
                            load(c, 1, g.u32(MAX_OVERSHIELD)?);
                            let f = comiss(c, low(c, 0), low(c, 1))?;
                            g.set_u32(c.ebp.wrapping_add(0xf0), low(c, 0))?;
                            if f & CF != 0 {
                                or8(c, g, c.ebp.wrapping_add(0x10b), 2)?;
                                g.set_u8(s(c, 0x13), 1)?;
                                0xd6151
                            } else {
                                g.set_u32(c.ebp.wrapping_add(0xf0), low(c, 1))?;
                                0xd6143
                            }
                        }
                    }
                }
            }
            0xd6143 => {
                and8(c, g, c.ebp.wrapping_add(0x10a), 0xef)?;
                0xd614a
            }
            0xd614a => {
                and8(c, g, c.ebp.wrapping_add(0x10b), 0xfd)?;
                0xd6151
            }
            // The body: regeneration while not depleted.
            0xd6151 => {
                load(c, 0, g.u32(c.ebp.wrapping_add(0xe4))?);
                c.xmm[1] = [0; 16];
                let f = comiss(c, low(c, 0), 0)?;
                if jbe(f) {
                    0xd64b8
                } else {
                    c.edx = byte(c.edx, g.u8(c.ebp.wrapping_add(0x10a))?);
                    let dl = shr(c, c.edx, 2, 8);
                    c.edx = byte(c.edx, dl as u8);
                    tst(c, dl & 1, 8);
                    if dl & 1 != 0 {
                        0xd64b8
                    } else {
                        tst(c, c.ebx, 32);
                        if c.ebx == 0 {
                            0xd63de
                        } else {
                            load(c, 2, g.u32(c.ebx.wrapping_add(0x14))?);
                            load(c, 0, g.u32(c.ebx.wrapping_add(0x68))?);
                            g.set_u32(s(c, 0x1c), low(c, 2))?;
                            0xd63f5
                        }
                    }
                }
            }
            0xd61a2 => {
                load(c, 0, g.u32(c.ebp.wrapping_add(0xf0))?);
                let f = comiss(c, low(c, 0), low(c, 2))?;
                let mut drain = !jbe(f);
                if drain {
                    arith(c, c.esi, 2, true, 32);
                    drain = c.esi == 2;
                }
                if drain {
                    c.eax = c.edi;
                    return call(callee::OVERSHIELD_OWNER, 0xd61c3);
                }
                0xd6256
            }
            0xd61c3 => {
                c.ecx = g.u32(game_time)?;
                c.edx = sx16(g.u16(c.ecx.wrapping_add(2))?);
                load(c, 0, g.u32(RATE_SCALE)?);
                cvtsi2ss(c, 1, c.edx)?;
                sse(c, 1, g.u32(DRAIN_SCALE)?, simd::mulss)?;
                sse(c, 0, low(c, 1), simd::divss)?;
                load(c, 1, g.u32(c.ebp.wrapping_add(0xf0))?);
                and8(c, g, c.ebp.wrapping_add(0x10b), 0xfd)?;
                sse(c, 1, low(c, 3), simd::subss)?;
                let f = comiss(c, low(c, 0), low(c, 1))?;
                g.set_u32(s(c, 0x20), low(c, 0))?;
                g.set_u32(s(c, 0x24), low(c, 1))?;
                let drained = if jbe(f) {
                    arith(c, c.eax, NONE, true, 32);
                    load(c, 1, g.u32(c.ebp.wrapping_add(0xf0))?);
                    sse(c, 1, low(c, 0), simd::subss)?;
                    g.set_u32(c.ebp.wrapping_add(0xf0), low(c, 1))?;
                    0x20
                } else {
                    arith(c, c.eax, NONE, true, 32);
                    g.set_u32(c.ebp.wrapping_add(0xf0), low(c, 2))?;
                    0x24
                };
                if c.eax == NONE {
                    0xd6240
                } else {
                    let amount = g.u32(s(c, drained))?;
                    if drained == 0x24 {
                        c.ecx = amount;
                    } else {
                        c.edx = amount;
                    }
                    push(c, g, amount)?;
                    return call(callee::OVERSHIELD_DRAIN, 0xd6240);
                }
            }
            0xd6240 => {
                push(c, g, 0x80)?;
                c.eax = c.edi;
                return call(callee::REPLICATE, 0xd624c);
            }
            0xd624c => {
                g.set_u8(s(c, 0x13), 1)?;
                0xd6151
            }
            // The shield: below its ceiling, recharge once the stun timer
            // runs out.
            0xd6256 => {
                let f = comiss(c, low(c, 2), g.u32(c.ebp.wrapping_add(0xf0))?)?;
                if jbe(f) {
                    0xd614a
                } else {
                    c.esi = low16(c.esi, g.u16(c.ebp.wrapping_add(0x104))?);
                    tst(c, c.esi, 16);
                    if c.esi & 0xffff != 0 {
                        0xd63a9
                    } else {
                        tst(c, c.ebx, 32);
                        if c.ebx != 0 {
                            load(c, 0, g.u32(c.ebx.wrapping_add(0x6c))?);
                        } else {
                            c.eax = g.u32(s(c, 0x18))?;
                            load(c, 0, g.u32(c.eax.wrapping_add(0xd0))?);
                        }
                        c.ecx = g.u32(game_time)?;
                        load(c, 1, g.u32(c.ecx.wrapping_add(4))?);
                        c.ecx = byte(c.ecx, g.u8(c.ebp.wrapping_add(0xaa))?);
                        c.edx = shl_cl(c, 1);
                        sse(c, 1, low(c, 0), simd::mulss)?;
                        g.set_u32(s(c, 0x20), low(c, 1))?;
                        tst(c, c.edx & 3, 8);
                        if c.edx & 3 == 0 {
                            0xd62d6
                        } else {
                            c.ecx = 0;
                            tst(c, 0, 32);
                            c.ecx = u32::from(g.u16(c.ebp.wrapping_add(0x138))?);
                            c.eax = 3;
                            return call(callee::TEAM_RATE, 0xd62c4);
                        }
                    }
                }
            }
            0xd62c4 => {
                sse(c, 0, g.u32(s(c, 0x20))?, simd::mulss)?;
                load(c, 2, g.u32(s(c, 0x1c))?);
                g.set_u32(s(c, 0x20), low(c, 0))?;
                0xd62d6
            }
            0xd62d6 => {
                c.eax = byte(c.eax, g.u8(c.ebp.wrapping_add(0x10a))?);
                let al = shr(c, c.eax, 3, 8);
                c.eax = byte(c.eax, al as u8);
                tst(c, al & 1, 8);
                if al & 1 == 0 {
                    0xd62fc
                } else {
                    and8(c, g, c.ebp.wrapping_add(0x10a), 0xf7)?;
                    push(c, g, 0)?;
                    push(c, g, 2)?;
                    push(c, g, NONE)?;
                    push(c, g, c.edi)?;
                    return call(callee::SHIELD_EFFECT, 0xd62f6);
                }
            }
            0xd62f6 => {
                load(c, 2, g.u32(s(c, 0x1c))?);
                0xd62fc
            }
            0xd62fc => {
                c.ecx = 0;
                tst(c, 0, 32);
                c.ecx = u32::from(g.u16(c.ebp.wrapping_add(0x10a))?);
                c.ecx = shr(c, c.ecx, 9, 32);
                tst(c, c.ecx & 1, 8);
                if c.ecx & 1 != 0 {
                    0xd634a
                } else {
                    c.esi = g.u32(s(c, 0x84))?;
                    c.edx = s(c, 0x40);
                    push(c, g, c.edx)?;
                    push(c, g, c.esi)?;
                    return call(callee::OWNER, 0xd631f);
                }
            }
            0xd631f => {
                c.eax = g.u32(s(c, 0x18))?;
                c.ecx = g.u32(c.eax.wrapping_add(0xb8))?;
                c.xmm[0] = [0; 16];
                push(c, g, 0)?;
                push(c, g, 0)?;
                push(c, g, 0)?;
                push(c, g, c.ecx)?;
                c.edx = c.esi;
                c.ebx = s(c, 0x50);
                return call(callee::RECHARGE_EFFECT, 0xd633e);
            }
            0xd633e => {
                load(c, 2, g.u32(s(c, 0x1c))?);
                c.ebx = g.u32(s(c, 0x14))?;
                c.edi = c.esi;
                0xd634a
            }
            0xd634a => {
                or8(c, g, c.ebp.wrapping_add(0x10b), 2)?;
                load(c, 0, g.u32(c.ebp.wrapping_add(0xf0))?);
                sse(c, 0, g.u32(s(c, 0x20))?, simd::addss)?;
                let f = comiss(c, low(c, 0), low(c, 2))?;
                g.set_u32(c.ebp.wrapping_add(0xf0), low(c, 0))?;
                if jbe(f) {
                    push(c, g, 0x80)?;
                    c.eax = c.edi;
                    g.set_u8(s(c, 0x17), 1)?;
                    return call(callee::REPLICATE, 0xd63a4);
                }
                g.set_u32(c.ebp.wrapping_add(0xf0), low(c, 2))?;
                and8(c, g, c.ebp.wrapping_add(0x10b), 0xfd)?;
                and8(c, g, c.ebp.wrapping_add(0x10a), 0xfd)?;
                push(c, g, 0x80)?;
                c.eax = c.edi;
                return call(callee::REPLICATE, 0xd638e);
            }
            0xd638e | 0xd63a4 | 0xd63d9 => 0xd6151,
            // The shield's stun timer runs down.
            0xd63a9 => return call(callee::STUN_HOLD, 0xd63ae),
            0xd63ae => {
                tst(c, c.eax, 8);
                if c.eax & 0xff == 0 {
                    let si = c.esi & 0xffff;
                    arith(c, si, 0x7fff, true, 16);
                    if si != 0x7fff {
                        c.esi = incdec(c, c.esi, true, 32);
                        g.set_u16(c.ebp.wrapping_add(0x104), c.esi as u16)?;
                        g.set_u8(s(c, 0x13), 1)?;
                    }
                }
                and8(c, g, c.ebp.wrapping_add(0x10b), 0xfd)?;
                push(c, g, 0x80)?;
                c.eax = c.edi;
                return call(callee::REPLICATE, 0xd63d9);
            }
            0xd63de => {
                c.eax = g.u32(s(c, 0x18))?;
                load(c, 2, g.u32(c.eax.wrapping_add(0x38))?);
                load(c, 0, g.u32(c.eax.wrapping_add(0xd4))?);
                g.set_u32(s(c, 0x1c), low(c, 2))?;
                0xd63f5
            }
            // The body's regeneration (`d63de`..`d64b5`): the Halo 1
            // health model's `after` undoes what it raises.
            0xd63f5 => {
                let f = comiss(c, low(c, 2), g.u32(c.ebp.wrapping_add(0xec))?)?;
                let mut regenerate = !jbe(f);
                if regenerate {
                    let f = comiss(c, low(c, 0), low(c, 1))?;
                    regenerate = !jbe(f);
                }
                if !regenerate {
                    0xd64b8
                } else {
                    c.esi = low16(c.esi, g.u16(c.ebp.wrapping_add(0x106))?);
                    tst(c, c.esi, 16);
                    if c.esi & 0xffff != 0 {
                        0xd6496
                    } else {
                        c.ecx = byte(c.ecx, g.u8(c.ebp.wrapping_add(0xaa))?);
                        c.eax = g.u32(game_time)?;
                        load(c, 1, g.u32(c.eax.wrapping_add(4))?);
                        c.edx = shl_cl(c, 1);
                        sse(c, 1, low(c, 0), simd::mulss)?;
                        g.set_u32(s(c, 0x20), low(c, 1))?;
                        tst(c, c.edx & 3, 8);
                        if c.edx & 3 == 0 {
                            0xd6465
                        } else {
                            c.ecx = 0;
                            tst(c, 0, 32);
                            c.ecx = u32::from(g.u16(c.ebp.wrapping_add(0x138))?);
                            c.eax = 3;
                            return call(callee::TEAM_RATE, 0xd6450);
                        }
                    }
                }
            }
            0xd6450 => {
                sse(c, 0, g.u32(s(c, 0x20))?, simd::mulss)?;
                load(c, 2, g.u32(s(c, 0x1c))?);
                g.set_u32(s(c, 0x20), low(c, 0))?;
                c.xmm[1] = c.xmm[0];
                0xd6465
            }
            0xd6465 => {
                or8(c, g, c.ebp.wrapping_add(0x10b), 1)?;
                load(c, 0, g.u32(c.ebp.wrapping_add(0xec))?);
                sse(c, 0, low(c, 1), simd::addss)?;
                let f = comiss(c, low(c, 0), low(c, 2))?;
                g.set_u32(c.ebp.wrapping_add(0xec), low(c, 0))?;
                if jbe(f) {
                    0xd64a7
                } else {
                    g.set_u32(c.ebp.wrapping_add(0xec), low(c, 2))?;
                    and8(c, g, c.ebp.wrapping_add(0x10b), 0xfe)?;
                    0xd64ac
                }
            }
            0xd6496 => return call(callee::STUN_HOLD, 0xd649b),
            0xd649b => {
                tst(c, c.eax, 8);
                if c.eax & 0xff != 0 {
                    0xd64ac
                } else {
                    c.esi = incdec(c, c.esi, true, 32);
                    g.set_u16(c.ebp.wrapping_add(0x106), c.esi as u16)?;
                    0xd64a7
                }
            }
            0xd64a7 => {
                g.set_u8(s(c, 0x13), 1)?;
                0xd64ac
            }
            0xd64ac => {
                push(c, g, 0x40)?;
                c.eax = c.edi;
                return call(callee::REPLICATE, 0xd64b5);
            }
            0xd64b5 => {
                c.xmm[1] = [0; 16];
                0xd64b8
            }
            // Section damage decays (alive and not depleted).
            0xd64b8 => {
                load(c, 0, g.u32(c.ebp.wrapping_add(0xe4))?);
                let f = comiss(c, low(c, 0), low(c, 1))?;
                if jbe(f) {
                    0xd6592
                } else {
                    c.eax = byte(c.eax, g.u8(c.ebp.wrapping_add(0x10a))?);
                    let al = shr(c, c.eax, 2, 8);
                    c.eax = byte(c.eax, al as u8);
                    tst(c, al & 1, 8);
                    if al & 1 != 0 {
                        0xd6592
                    } else {
                        c.edi = sx16(g.u16(c.ebp.wrapping_add(0x120))?);
                        c.eax = sx16(g.u16(c.ebp.wrapping_add(0x122))?);
                        c.edi = shr(c, c.edi, 3, 32);
                        c.esi = 0;
                        tst(c, 0, 32);
                        c.eax = arith(c, c.eax, c.ebp, false, 32);
                        arith(c, c.edi, c.esi, true, 32);
                        if c.edi as i32 <= 0 {
                            0xd6592
                        } else {
                            load(c, 0, g.u32(BYTE_SCALE)?);
                            g.set_u32(s(c, 0x20), c.esi)?;
                            c.eax = arith(c, c.eax, 2, false, 32);
                            0xd6506
                        }
                    }
                }
            }
            // `esi` the section, `eax` its record's `+2`, `[S+20]` the
            // offset of its definition.
            0xd6506 => {
                let level = g.u8(c.eax)?;
                arith(c, level.into(), 0, true, 8);
                if level == 0 {
                    0xd657b
                } else {
                    c.ecx = low16(c.ecx, g.u16(c.eax.wrapping_add(4))?);
                    tst(c, c.ecx, 16);
                    let mut count_down = c.ecx & 0xffff != 0;
                    if !count_down {
                        c.edx = g.u32(s(c, 0x18))?;
                        let count = g.u32(c.edx.wrapping_add(0xbc))?;
                        arith(c, count, c.esi, true, 32);
                        count_down = count as i32 <= c.esi as i32;
                    }
                    if count_down {
                        c.ecx = incdec(c, c.ecx, true, 32);
                        g.set_u16(c.eax.wrapping_add(4), c.ecx as u16)?;
                        g.set_u8(s(c, 0x13), 1)?;
                        0xd657b
                    } else {
                        c.ecx = g.u32(c.edx.wrapping_add(0xc0))?;
                        c.edx = g.u32(s(c, 0x20))?;
                        load(c, 2, g.u32(c.ecx.wrapping_add(c.edx).wrapping_add(0x2c))?);
                        c.ecx = arith(c, c.ecx, c.edx, false, 32);
                        let f = comiss(c, low(c, 2), low(c, 1))?;
                        if jbe(f) {
                            0xd657b
                        } else {
                            c.edx = g.u32(game_time)?;
                            load(c, 2, g.u32(c.edx.wrapping_add(4))?);
                            sse(c, 2, g.u32(c.ecx.wrapping_add(0x2c))?, simd::mulss)?;
                            sse(c, 2, low(c, 0), simd::mulss)?;
                            g.set_u32(s(c, 0x24), low(c, 2))?;
                            fld_fistp(c, g, s(c, 0x24), s(c, 0x1c))?;
                            c.ecx = byte(c.ecx, g.u8(c.eax)?);
                            c.edx = g.u32(s(c, 0x1c))?;
                            c.ebx = c.ecx & 0xff;
                            arith(c, c.ebx, c.edx, true, 32);
                            if c.ebx as i32 <= c.edx as i32 {
                                g.set_u8(c.eax, 0)?;
                                0xd657b
                            } else {
                                let cl = arith(c, c.ecx, c.edx, true, 8);
                                c.ecx = byte(c.ecx, cl as u8);
                                g.set_u8(c.eax, cl as u8)?;
                                g.set_u8(s(c, 0x13), 1)?;
                                0xd657b
                            }
                        }
                    }
                }
            }
            0xd657b => {
                c.ebx = g.u32(s(c, 0x20))?;
                c.esi = incdec(c, c.esi, false, 32);
                c.ebx = arith(c, c.ebx, 0x38, false, 32);
                c.eax = arith(c, c.eax, 8, false, 32);
                arith(c, c.esi, c.edi, true, 32);
                g.set_u32(s(c, 0x20), c.ebx)?;
                if (c.esi as i32) < (c.edi as i32) { 0xd6506 } else { 0xd6592 }
            }
            // The recent-damage accumulators, body then (unless bit 11)
            // shield.
            0xd6592 => {
                c.eax = byte(c.eax, g.u8(c.ebp.wrapping_add(0x10a))?);
                c.edx = g.u32(s(c, 0x14))?;
                let al = shr(c, c.eax, 2, 8) & 1;
                tst(c, al, 8);
                c.eax = byte(c.eax, al as u8);
                g.set_u8(s(c, 0x24), al as u8)?;
                c.ecx = g.u32(s(c, 0x24))?;
                c.esi = c.ebp.wrapping_add(0x109);
                c.ebx = c.ebp.wrapping_add(0x100);
                c.edi = c.ebp.wrapping_add(0xf8);
                push(c, g, c.ecx)?;
                return call(callee::RECENT_DAMAGE, 0xd65c1);
            }
            0xd65c1 => {
                let bl = g.u8(s(c, 0x13))? | c.eax as u8;
                tst(c, bl.into(), 8);
                c.ebx = byte(c.ebx, bl);
                c.eax = 0;
                tst(c, 0, 32);
                c.eax = u32::from(g.u16(c.ebp.wrapping_add(0x10a))?);
                g.set_u8(s(c, 0x13), bl)?;
                c.edx = c.eax;
                c.edx = shr(c, c.edx, 0xb, 32);
                tst(c, c.edx & 1, 8);
                if c.edx & 1 == 0 {
                    c.edx = g.u32(s(c, 0x14))?;
                    let al = shr(c, c.eax, 3, 8) & 1;
                    tst(c, al, 8);
                    c.eax = byte(c.eax, al as u8);
                    g.set_u8(s(c, 0x24), al as u8)?;
                    c.eax = g.u32(s(c, 0x24))?;
                    c.esi = c.ebp.wrapping_add(0x108);
                    c.ebx = c.ebp.wrapping_add(0xfc);
                    c.edi = c.ebp.wrapping_add(0xf4);
                    push(c, g, c.eax)?;
                    return call(callee::RECENT_DAMAGE, 0xd663d);
                }
                c.eax = byte(c.eax, g.u8(c.ebp.wrapping_add(0x108))?);
                let al = c.eax as u8;
                tst(c, al.into(), 8);
                if al as i8 > 0 {
                    load(c, 0, g.u32(c.ebp.wrapping_add(0xfc))?);
                    sse(c, 0, g.u32(c.ebp.wrapping_add(0xf4))?, simd::addss)?;
                    let al = incdec(c, al.into(), true, 8);
                    c.eax = byte(c.eax, al as u8);
                    g.set_u8(c.ebp.wrapping_add(0x108), al as u8)?;
                    g.set_u32(c.ebp.wrapping_add(0xf4), low(c, 0))?;
                }
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.eax = byte(c.eax, 1);
                c.ebx = pop(c, g)?;
                c.esp = arith(c, c.esp, 0x70, false, 32);
                return Ok(Next::Ret { pop: 4 });
            }
            0xd663d => {
                let changed = g.u8(s(c, 0x13))?;
                c.ecx = byte(c.ecx, changed);
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                let cl = changed | c.eax as u8;
                tst(c, cl.into(), 8);
                c.ecx = byte(c.ecx, cl);
                c.ebp = pop(c, g)?;
                c.eax = byte(c.eax, cl);
                c.ebx = pop(c, g)?;
                c.esp = arith(c, c.esp, 0x70, false, 32);
                return Ok(Next::Ret { pop: 4 });
            }
            other => return Err(unowned(other)),
        };
    }
}

/// The guest's memory as `guest_layout` reads it.
fn mem(g: &Guest) -> &dyn GuestMemory {
    g
}

/// A tick starts (`GAME_TICK`'s entry): the last tick's events go.
pub fn tick_starts() {
    events::clear();
}

/// The damage report `d9640` is about to apply the report at `report`
/// (`edi`) to `victim` (its stack argument): record the damage and any
/// actor kill.
/// A report the guest could not read records nothing (the guest function
/// faults on it too).
pub fn damage_report(g: &Guest, report: u32, victim: u32) {
    if let Ok(d) = ::sim_objects::damage::damage(mem(g), report, Datum(victim)) {
        events::push(SimEvent::Damage(d));
        if d.depleting()
            && let Some(kill) = ::sim_objects::damage::actor_killed(mem(g), report, d.victim)
        {
            events::push(kill);
        }
    }
}

/// `PLAYER_KILLED` (`15ce70`) is about to credit a kill; `esp` is at its
/// entry: attacker player `[esp+4]`, victim player `[esp+8]`, the packed
/// damage code in `[esp+10]`'s low byte. Record the `Kill` event.
pub fn player_killed(g: &Guest, esp: u32) {
    let arg = |i: u32| g.u32(esp.wrapping_add(4 * (i + 1)));
    let (Ok(attacker), Ok(victim), Ok(code)) = (arg(0), arg(1), arg(3)) else {
        return;
    };
    let kill = ::sim_objects::damage::kill(mem(g), Datum(attacker), Datum(victim), code as u8);
    events::push(SimEvent::Kill(kill));
}

#[cfg(test)]
mod tests;
