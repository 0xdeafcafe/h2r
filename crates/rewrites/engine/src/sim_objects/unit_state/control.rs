//! `c49b0 (unit: stack 0) -> al`; `ret 4`: the unit's control update.
//!
//! **Frame.** `E` = `esp` after `sub esp, 1c`; `ebx`, `ebp`, `esi`, then
//! `edi` pushed, so at every call site `esp` is `E - 10` (the four calls
//! of `10f720` push their argument `1` first, which the callee pops).
//! With `P = E - 10`: `[P+13]` the result byte, `[P+14]` the definition,
//! `[P+18]` the tick rate scaled by `[45dc20]` (f32), `[P+1c]` that rate
//! as an integer (`fistp`), later the second linked object's body,
//! `[P+20..28]` the parent-relative facing, `[P+30]` the unit argument.
//! Across the calls `ebx` holds the unit handle (the first linked
//! object's body around `10f720`), `edi` the unit's body, `ebp` the
//! definition (the linked object's name answer between `10f5f0` and the
//! name tests, then reloaded from `[P+14]`), `esi` the linked object's
//! handle.
//!
//! **What it does** (field names ours, **L**). `d0d20` reads the
//! unit's control. When body `+134` bit 21 is set, the facing `d0080`
//! writes at `+150` becomes the desired aim (`+15c`) and look (`+180`),
//! and `+1b0` is set from `[4687a8]`; when bit 1 is clear, the facing is
//! the parent's seat facing (the parent's node at `+18`, `34` bytes a
//! node from its `+116` table, applied to `+70`), or `+70` with no parent,
//! copied to `+180`, `+15c` and `+150`, with `+1b0` from `[4687a4]`;
//! either way `+148` is cleared. Unless definition `+bc` bit 11 is set:
//! the timed bits (`+1ec` counts down, `+1f0` ORed into `+148`, bit 16
//! toggled each `[45dc20]` × ticks-per-second ticks, `+1c0` 1 or 0);
//! then, unless `+134` bit 22, the two linked objects (`+248`, `+24c`):
//! by their name (`10f5f0`, `c7070`, `10f720`) the first's `+148` low
//! half, `+150` and `+1b0` are copied in, the second's `+15c` into
//! `+15c` and `+180`, its `+148` high half, bit 5 and `+1c0`; a non-zero
//! high half of `+148` stamps `+244` with the game tick. Last, when
//! definition `+bc` bit 28 is set, the floats `+334` and `+338` follow
//! definition `+1d4..+1e0` and the tick length, `cc380` choosing
//! `[45dbdc]`. `al` is 1 when anything changed.

use super::{
    NONE, ONE, SMALL, VECTOR_A4, VECTOR_A8, byte, call, callee, fistp, imm, owned, pop, ret, sx8, sx16, x87,
};
use guest_layout::points;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_objects::{Cx, Next, Result, unowned};
use runtime::{CpuAbi, Guest, GuestFault};

/// `c49b0`'s return sites, in address order.
pub(in crate::sim_objects) const CONTROL_SITES: [u32; 13] = [
    0xc49f8, 0xc4a15, 0xc4cba, 0xc4cc3, 0xc4cd0, 0xc4d0c, 0xc4d98, 0xc4dc8, 0xc4dd8, 0xc4de5, 0xc4e14, 0xc4f24,
    0xc5024,
];

/// The tick-rate scale `c4bfa` multiplies by.
const RATE_SCALE: u32 = 0x0045_dc20;

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}

/// `[esp + off]`'s address.
fn sp(c: &CpuAbi, off: u32) -> u32 {
    c.esp.wrapping_add(off)
}

/// `movss [addr], xmm[r]`.
fn st(c: &CpuAbi, g: &mut Guest, addr: u32, r: usize) -> Result<()> {
    g.set_u32(addr, sse::lane0(c, r))
}

/// `movss xmm[r], [addr]`.
fn ld(c: &mut CpuAbi, g: &Guest, r: usize, addr: u32) -> Result<()> {
    sse::load(c, r, g.u32(addr)?);
    Ok(())
}

/// `op xmm[d], [addr]`.
fn opm(c: &mut CpuAbi, g: &Guest, op: Ss, d: usize, addr: u32) -> Result<()> {
    let b = g.u32(addr)?;
    sse::ss(c, op, d, b);
    Ok(())
}

/// The guest's divide error at the `idiv` (`c4c19`).
fn divide_error() -> GuestFault {
    GuestFault {
        addr: 0xc4c19,
        len: 0,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::InvalidOperation,
    }
}

/// `pop edi; pop esi; pop ebp; [mov al, 1;] pop ebx; add esp, 1c; ret 4`.
fn leave(c: &mut CpuAbi, g: &Guest, one: bool) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    if one {
        c.eax = byte(c.eax, 1);
    }
    c.ebx = pop(c, g)?;
    c.esp = c.esp.wrapping_add(0x1c);
    ret(c, 4)
}

/// `c49b0 (unit: stack 0) -> al`.
pub(in crate::sim_objects) fn control(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    owned(pc, points::UNIT_UPDATE_CONTROL.base, &CONTROL_SITES)?;
    let time = points::GAME_TIME.base;
    let mut at_ = pc;
    loop {
        at_ = match at_ {
            0xc49b0 => {
                c.esp = c.esp.wrapping_sub(0x1c);
                super::push(c, g, c.ebx)?;
                c.ebx = g.u32(sp(c, 0x24))?;
                super::push(c, g, c.ebp)?;
                super::push(c, g, c.esi)?;
                c.eax = c.ebx & 0xffff;
                c.esi = c.eax.wrapping_mul(3);
                c.eax = g.u32(points::OBJECTS.base)?;
                c.ecx = g.u32(at(c.eax, 0x44))?;
                c.eax = g.u32(points::TAG_ENTRIES.base)?;
                super::push(c, g, c.edi)?;
                c.esi <<= 2;
                c.edi = g.u32(at(at(c.ecx, c.esi), 8))?;
                c.edx = (g.u32(c.edi)? & 0xffff) << 4;
                c.ebp = g.u32(at(at(c.edx, c.eax), 8))?;
                c.eax = c.ebx;
                g.set_u32(sp(c, 0x14), c.ebp)?;
                g.set_u8(sp(c, 0x13), 0)?;
                return call(c, callee::D0D20, 0xc49f8);
            }
            0xc49f8 => {
                c.eax = g.u32(at(c.edi, 0x134))?;
                c.ecx = c.eax >> 0x15;
                if c.ecx & 1 == 0 {
                    0xc4a69
                } else {
                    c.esi = at(c.edi, 0x150);
                    c.eax = c.ebx;
                    return call(c, callee::D0080, 0xc4a15);
                }
            }
            0xc4a15 => {
                c.eax = c.esi;
                c.ecx = g.u32(c.eax)?;
                c.edx = at(c.edi, 0x15c);
                g.set_u32(c.edx, c.ecx)?;
                c.ecx = g.u32(at(c.eax, 4))?;
                c.eax = g.u32(at(c.eax, 8))?;
                g.set_u32(at(c.edx, 4), c.ecx)?;
                g.set_u32(at(c.edx, 8), c.eax)?;
                c.edx = g.u32(c.esi)?;
                c.eax = g.u32(at(c.esi, 4))?;
                c.ecx = at(c.edi, 0x180);
                g.set_u32(c.ecx, c.edx)?;
                c.edx = g.u32(at(c.esi, 8))?;
                g.set_u32(at(c.ecx, 4), c.eax)?;
                g.set_u32(at(c.ecx, 8), c.edx)?;
                c.ecx = g.u32(VECTOR_A8)?;
                c.edx = g.u32(c.ecx)?;
                c.eax = at(c.edi, 0x1b0);
                g.set_u32(c.eax, c.edx)?;
                c.edx = g.u32(at(c.ecx, 4))?;
                g.set_u32(at(c.eax, 4), c.edx)?;
                c.ecx = g.u32(at(c.ecx, 8))?;
                g.set_u32(at(c.eax, 8), c.ecx)?;
                g.set_u8(sp(c, 0x13), 1)?;
                0xc4b9c
            }
            0xc4a69 => {
                c.eax >>= 1;
                if c.eax & 1 != 0 {
                    g.set_u8(sp(c, 0x13), 1)?;
                    0xc4ba6
                } else {
                    c.edx = g.u32(points::OBJECTS.base)?;
                    c.edx = g.u32(at(c.edx, 0x44))?;
                    c.ecx = g.u32(at(at(c.edx, c.esi), 8))?;
                    c.eax = g.u32(at(c.ecx, 0x14))?;
                    if c.eax == NONE {
                        c.ecx = at(c.ecx, 0x70);
                        c.edx = c.ecx;
                        c.ecx = g.u32(c.edx)?;
                        c.eax = g.u32(at(c.edx, 4))?;
                        c.edx = g.u32(at(c.edx, 8))?;
                        0xc4b56
                    } else {
                        0xc4aa0
                    }
                }
            }
            0xc4aa0 => {
                c.esi = sx8(g.u8(at(c.ecx, 0x18))?);
                ld(c, g, 1, at(c.ecx, 0x74))?;
                c.esi = c.esi.wrapping_mul(0x34);
                ld(c, g, 2, at(c.ecx, 0x78))?;
                ld(c, g, 0, at(c.ecx, 0x70))?;
                c.eax = (c.eax & 0xffff).wrapping_mul(3);
                c.edx = g.u32(at(at(c.edx, c.eax.wrapping_mul(4)), 8))?;
                c.eax = sx16(g.u16(at(c.edx, 0x116))?);
                c.esi = c.esi.wrapping_add(c.edx);
                ld(c, g, 3, at(at(c.eax, c.esi), 0x1c))?;
                ld(c, g, 4, at(at(c.eax, c.esi), 0x10))?;
                c.eax = c.eax.wrapping_add(c.esi);
                sse::ss_reg(c, Ss::Mul, 3, 2);
                sse::ss_reg(c, Ss::Mul, 4, 1);
                sse::ss_reg(c, Ss::Add, 3, 4);
                ld(c, g, 4, at(c.eax, 4))?;
                sse::ss_reg(c, Ss::Mul, 4, 0);
                sse::ss_reg(c, Ss::Add, 3, 4);
                ld(c, g, 4, at(c.eax, 0x14))?;
                st(c, g, sp(c, 0x20), 3)?;
                ld(c, g, 3, at(c.eax, 0x20))?;
                c.ecx = g.u32(sp(c, 0x20))?;
                sse::ss_reg(c, Ss::Mul, 3, 2);
                sse::ss_reg(c, Ss::Mul, 4, 1);
                sse::ss_reg(c, Ss::Add, 3, 4);
                ld(c, g, 4, at(c.eax, 8))?;
                sse::ss_reg(c, Ss::Mul, 4, 0);
                sse::ss_reg(c, Ss::Add, 3, 4);
                st(c, g, sp(c, 0x24), 3)?;
                ld(c, g, 3, at(c.eax, 0x24))?;
                sse::ss_reg(c, Ss::Mul, 3, 2);
                ld(c, g, 2, at(c.eax, 0x18))?;
                sse::ss_reg(c, Ss::Mul, 2, 1);
                ld(c, g, 1, at(c.eax, 0xc))?;
                c.eax = g.u32(sp(c, 0x24))?;
                sse::ss_reg(c, Ss::Add, 3, 2);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                sse::ss_reg(c, Ss::Add, 3, 1);
                st(c, g, sp(c, 0x28), 3)?;
                c.edx = g.u32(sp(c, 0x28))?;
                0xc4b56
            }
            0xc4b56 => {
                for off in [0x180, 0x15c, 0x150] {
                    c.esi = at(c.edi, off);
                    g.set_u32(c.esi, c.ecx)?;
                    g.set_u32(at(c.esi, 4), c.eax)?;
                    g.set_u32(at(c.esi, 8), c.edx)?;
                }
                c.edx = g.u32(VECTOR_A4)?;
                c.eax = g.u32(c.edx)?;
                c.ecx = at(c.edi, 0x1b0);
                g.set_u32(c.ecx, c.eax)?;
                c.eax = g.u32(at(c.edx, 4))?;
                g.set_u32(at(c.ecx, 4), c.eax)?;
                c.edx = g.u32(at(c.edx, 8))?;
                g.set_u32(at(c.ecx, 8), c.edx)?;
                0xc4b9c
            }
            0xc4b9c => {
                g.set_u32(at(c.edi, 0x148), 0)?;
                0xc4ba6
            }
            0xc4ba6 => {
                c.eax = g.u32(at(c.ebp, 0xbc))? >> 0xb;
                if c.eax & 1 != 0 {
                    0xc4f14
                } else {
                    c.ebx = g.u32(at(c.edi, 0x134))?;
                    c.ecx = g.u32(at(c.edi, 0x1ec))?;
                    c.ebx >>= 0x16;
                    c.ebx = byte(c.ebx, (c.ebx as u8) & 1);
                    if (c.ecx as i32) <= 0 {
                        0xc4c74
                    } else {
                        c.eax = g.u32(at(c.edi, 0x1f0))?;
                        c.esi = g.u32(at(c.edi, 0x148))? | c.eax;
                        g.set_u32(at(c.edi, 0x148), c.esi)?;
                        if c.eax & 0x1_0000 == 0 {
                            sse::zero(c, 0);
                            0xc4c54
                        } else {
                            0xc4bec
                        }
                    }
                }
            }
            0xc4bec => {
                c.ecx = g.u32(time)?;
                c.edx = sx16(g.u16(at(c.ecx, 2))?);
                sse::cvtsi2ss(c, 0, c.edx);
                opm(c, g, Ss::Mul, 0, RATE_SCALE)?;
                st(c, g, sp(c, 0x18), 0)?;
                let bits = g.u32(sp(c, 0x18))?;
                let slot = sp(c, 0x1c);
                let mut v = 0;
                x87(c, g, 0xc4c0c, slot, |x| {
                    x.fld(bits)?;
                    v = fistp(x)?;
                    Ok(())
                })?;
                g.set_u32(slot, v as u32)?;
                c.ecx = g.u32(at(c.edi, 0x1ec))?;
                c.eax = c.ecx;
                c.edx = ((c.eax as i32) >> 31) as u32;
                let divisor = g.u32(slot)? as i32;
                let dividend = c.eax as i32;
                if divisor == 0 || (dividend == i32::MIN && divisor == -1) {
                    return Err(divide_error());
                }
                c.eax = (dividend / divisor) as u32;
                c.edx = (dividend % divisor) as u32;
                c.eax = g.u32(at(c.edi, 0x148))?;
                sse::load(c, 0, g.u32(ONE)?);
                if c.edx == 0 {
                    c.eax |= 0x1_0000;
                } else {
                    c.eax &= !0x1_0000;
                }
                g.set_u32(at(c.edi, 0x148), c.eax)?;
                0xc4c54
            }
            0xc4c54 => {
                c.ecx = c.ecx.wrapping_sub(1);
                st(c, g, at(c.edi, 0x1c0), 0)?;
                g.set_u32(at(c.edi, 0x1ec), c.ecx)?;
                if c.ecx == 0 {
                    g.set_u32(at(c.edi, 0x1f0), 0)?;
                }
                g.set_u8(sp(c, 0x13), 1)?;
                0xc4c74
            }
            0xc4c74 => {
                if c.ebx as u8 != 0 {
                    0xc4f10
                } else {
                    c.esi = g.u32(at(c.edi, 0x248))?;
                    if c.esi == NONE {
                        0xc4d71
                    } else {
                        let al = g.u8(at(c.edi, 0x10a))? >> 2;
                        c.eax = byte(c.eax, al);
                        if al & 1 != 0 {
                            0xc4d71
                        } else {
                            c.edx = g.u32(points::OBJECTS.base)?;
                            c.eax = c.esi & 0xffff;
                            c.ecx = c.eax.wrapping_mul(3);
                            c.eax = g.u32(at(c.edx, 0x44))?;
                            c.ebx = g.u32(at(at(c.eax, c.ecx.wrapping_mul(4)), 8))?;
                            c.eax = c.esi;
                            return call(c, callee::F10F5F0, 0xc4cba);
                        }
                    }
                }
            }
            0xc4cba => {
                c.ecx = c.esi;
                c.ebp = c.eax;
                return call(c, callee::C7070, 0xc4cc3);
            }
            0xc4cc3 => {
                if c.eax as u8 != 0 {
                    super::push(c, g, 1)?;
                    c.eax = c.esi;
                    return call(c, callee::F10F720, 0xc4cd0);
                }
                // c4cf3: the name tests.
                if c.ebp == imm(g, 0xc4cf5)? || c.ebp == imm(g, 0xc4cfd)? {
                    super::push(c, g, 1)?;
                    c.eax = c.esi;
                    return call(c, callee::F10F720, 0xc4d0c);
                }
                0xc4d16
            }
            0xc4cd0 => {
                let mut eq1 = true;
                if g.u32(at(c.ebx, 0x13c))? != NONE {
                    c.ecx = g.u32(points::PLAYER_GLOBALS.base)?;
                    let dl = g.u8(at(c.ecx, 6))?;
                    c.edx = byte(c.edx, dl);
                    eq1 = dl != 0;
                }
                // `setne al` against 2, or (c4cee) `sete al` against 1.
                let take = if eq1 { c.eax == 1 } else { c.eax != 2 };
                c.eax = byte(c.eax, take as u8);
                if take { 0xc4d16 } else { 0xc4d68 }
            }
            0xc4d0c => {
                let take = c.eax == 2;
                c.eax = byte(c.eax, take as u8);
                if take { 0xc4d16 } else { 0xc4d68 }
            }
            0xc4d16 => {
                c.edx = g.u32(at(c.ebx, 0x148))?;
                c.eax = g.u32(at(c.edi, 0x148))?;
                c.edx &= 0xffff;
                c.eax |= c.edx;
                g.set_u32(at(c.edi, 0x148), c.eax)?;
                c.eax = at(c.ebx, 0x150);
                c.edx = g.u32(c.eax)?;
                c.ecx = at(c.edi, 0x150);
                g.set_u32(c.ecx, c.edx)?;
                c.edx = g.u32(at(c.eax, 4))?;
                g.set_u32(at(c.ecx, 4), c.edx)?;
                c.eax = g.u32(at(c.eax, 8))?;
                g.set_u32(at(c.ecx, 8), c.eax)?;
                c.ebx = at(c.ebx, 0x1b0);
                c.edx = g.u32(c.ebx)?;
                c.ecx = at(c.edi, 0x1b0);
                g.set_u32(c.ecx, c.edx)?;
                c.eax = g.u32(at(c.ebx, 4))?;
                g.set_u32(at(c.ecx, 4), c.eax)?;
                c.edx = g.u32(at(c.ebx, 8))?;
                g.set_u32(at(c.ecx, 8), c.edx)?;
                0xc4d68
            }
            0xc4d68 => {
                c.ebp = g.u32(sp(c, 0x14))?;
                g.set_u8(sp(c, 0x13), 1)?;
                0xc4d71
            }
            0xc4d71 => {
                if g.u32(at(c.edi, 0x24c))? == NONE {
                    0xc4ef0
                } else {
                    let al = g.u8(at(c.edi, 0x10a))? >> 2;
                    c.eax = byte(c.eax, al);
                    if al & 1 != 0 {
                        0xc4ef0
                    } else {
                        c.eax = g.u32(sp(c, 0x30))?;
                        return call(c, callee::F1143D0, 0xc4d98);
                    }
                }
            }
            0xc4d98 => {
                if c.eax as u8 == 0 {
                    0xc4ef0
                } else {
                    c.esi = g.u32(at(c.edi, 0x24c))?;
                    c.edx = g.u32(points::OBJECTS.base)?;
                    c.eax = c.esi & 0xffff;
                    c.ecx = c.eax.wrapping_mul(3);
                    c.eax = g.u32(at(c.edx, 0x44))?;
                    c.ecx = g.u32(at(at(c.eax, c.ecx.wrapping_mul(4)), 8))?;
                    c.eax = c.esi;
                    g.set_u32(sp(c, 0x1c), c.ecx)?;
                    return call(c, callee::F10F5F0, 0xc4dc8);
                }
            }
            0xc4dc8 => {
                c.ecx = c.esi;
                c.ebp = c.eax;
                g.set_u8(sp(c, 0x13), 0)?;
                c.ebx = byte(c.ebx, 0);
                return call(c, callee::C7070, 0xc4dd8);
            }
            0xc4dd8 => {
                if c.eax as u8 != 0 {
                    super::push(c, g, 1)?;
                    c.eax = c.esi;
                    return call(c, callee::F10F720, 0xc4de5);
                }
                // c4dfb: the name tests.
                if c.ebp == imm(g, 0xc4dfd)? || c.ebp == imm(g, 0xc4e05)? {
                    super::push(c, g, 1)?;
                    c.eax = c.esi;
                    return call(c, callee::F10F720, 0xc4e14);
                }
                0xc4e23
            }
            0xc4de5 => {
                if c.eax == 2 || c.eax == 0 {
                    g.set_u8(sp(c, 0x13), 1)?;
                } else {
                    g.set_u8(sp(c, 0x13), c.ebx as u8)?;
                }
                0xc4e23
            }
            0xc4e14 => {
                let bl = u8::from(c.eax == 1 || c.eax == 0);
                c.ebx = byte(c.ebx, bl);
                0xc4e23
            }
            0xc4e23 => {
                c.ecx = g.u32(sp(c, 0x1c))?;
                let bl = c.ebx as u8;
                let r13 = g.u8(sp(c, 0x13))?;
                let mut c4e52 = true;
                if g.u32(at(c.ecx, 0x13c))? != NONE {
                    c.edx = g.u32(points::PLAYER_GLOBALS.base)?;
                    let al = g.u8(at(c.edx, 6))?;
                    c.eax = byte(c.eax, al);
                    if al == 0 {
                        // c4e3d: `sete al`, then `dl` 1 only when neither
                        // `bl` nor the result byte is set.
                        c4e52 = false;
                        c.eax = byte(c.eax, u8::from(bl == 0));
                        c.edx = byte(c.edx, u8::from(bl == 0 && r13 == 0));
                    }
                }
                if c4e52 {
                    if bl == 0 && r13 == 0 {
                        // `mov al, [esp+13]`, then `mov al, 1`; c4e70.
                        c.eax = byte(c.eax, 1);
                        c.edx = byte(c.edx, 1);
                    } else {
                        // c4e62.
                        c.eax = byte(c.eax, 0);
                        if bl == 0 {
                            c.edx = byte(c.edx, r13);
                        }
                        at_ = 0xc4ee7;
                        continue;
                    }
                }
                0xc4e72
            }
            0xc4e72 => {
                if c.eax as u8 != 0 {
                    c.eax = at(c.ecx, 0x15c);
                    c.ebx = c.eax;
                    c.ebp = g.u32(c.ebx)?;
                    c.esi = at(c.edi, 0x15c);
                    g.set_u32(c.esi, c.ebp)?;
                    c.ebp = g.u32(at(c.ebx, 4))?;
                    g.set_u32(at(c.esi, 4), c.ebp)?;
                    c.ebx = g.u32(at(c.ebx, 8))?;
                    g.set_u32(at(c.esi, 8), c.ebx)?;
                    c.ebx = g.u32(c.eax)?;
                    c.esi = at(c.edi, 0x180);
                    g.set_u32(c.esi, c.ebx)?;
                    c.ebx = g.u32(at(c.eax, 4))?;
                    g.set_u32(at(c.esi, 4), c.ebx)?;
                    c.eax = g.u32(at(c.eax, 8))?;
                    g.set_u32(at(c.esi, 8), c.eax)?;
                }
                if c.edx as u8 != 0 {
                    c.edx = g.u32(at(c.ecx, 0x148))?;
                    c.ebp = g.u32(at(c.edi, 0x148))?;
                    c.edx &= 0xffff_0000;
                    c.ebp |= c.edx;
                    g.set_u32(at(c.edi, 0x148), c.ebp)?;
                    c.edx = g.u32(at(c.ecx, 0x148))?;
                    c.eax = c.ebp;
                    c.edx &= 0x20;
                    c.edx |= c.eax;
                    g.set_u32(at(c.edi, 0x148), c.edx)?;
                    c.eax = g.u32(at(c.ecx, 0x1c0))?;
                    g.set_u32(at(c.edi, 0x1c0), c.eax)?;
                }
                0xc4ee7
            }
            0xc4ee7 => {
                c.ebp = g.u32(sp(c, 0x14))?;
                g.set_u8(sp(c, 0x13), 1)?;
                0xc4ef0
            }
            0xc4ef0 => {
                if g.u32(at(c.edi, 0x148))? & 0xffff_0000 != 0 {
                    c.ecx = g.u32(time)?;
                    c.edx = g.u32(at(c.ecx, 8))?;
                    g.set_u32(at(c.edi, 0x244), c.edx)?;
                    g.set_u8(sp(c, 0x13), 1)?;
                }
                0xc4f10
            }
            0xc4f10 => {
                c.ebx = g.u32(sp(c, 0x30))?;
                0xc4f14
            }
            0xc4f14 => {
                if g.u8(at(c.edi, 0xaa))? == 1 {
                    c.eax = c.ebx;
                    return call(c, callee::F42F0, 0xc4f24);
                }
                0xc4f28
            }
            0xc4f24 => {
                if c.eax as u8 != 0 {
                    c.ecx = byte(c.ecx, 1);
                    0xc4f39
                } else {
                    0xc4f28
                }
            }
            0xc4f28 => {
                c.eax = g.u32(at(c.edi, 0x148))?;
                c.ecx = byte(c.ecx, u8::from(c.eax & 0x800 != 0));
                0xc4f39
            }
            0xc4f39 => {
                c.eax = g.u32(at(c.ebp, 0xbc))? >> 0x1c;
                if c.eax & 1 == 0 {
                    0xc508f
                } else {
                    let cl = c.ecx as u8;
                    sse::zero(c, 1);
                    if cl == 0 {
                        ld(c, g, 0, at(c.edi, 0x334))?;
                        let (a, b) = (sse::lane0(c, 0), sse::lane0(c, 1));
                        sse::comiss(c, a, b);
                        if sse::below_or_equal(c) {
                            at_ = 0xc508f;
                            continue;
                        }
                    }
                    ld(c, g, 0, at(c.edi, 0x338))?;
                    let (a, b) = (sse::lane0(c, 0), sse::lane0(c, 1));
                    let f = sse::ucomiss(c, a, b);
                    sse::lahf(c);
                    // `test ah, 44h; jp`: taken unless ordered and equal.
                    if f & 0x44 != 0x40 { 0xc5053 } else { 0xc4f77 }
                }
            }
            0xc4f77 => {
                let cl = c.ecx as u8;
                ld(c, g, 2, at(c.ebp, 0x1d8))?;
                opm(c, g, Ss::Add, 2, at(c.ebp, 0x1d4))?;
                if cl != 0 {
                    c.ecx = g.u32(time)?;
                    ld(c, g, 0, at(c.ecx, 4))?;
                    opm(c, g, Ss::Add, 0, at(c.edi, 0x334))?;
                    let (a, b) = (sse::lane0(c, 2), sse::lane0(c, 0));
                    sse::comiss(c, a, b);
                    let above = !sse::below_or_equal(c);
                    st(c, g, at(c.edi, 0x334), 0)?;
                    if above {
                        at_ = 0xc5083;
                        continue;
                    }
                }
                // c4faf
                let a = sse::lane0(c, 2);
                let b = g.u32(at(c.edi, 0x334))?;
                sse::comiss(c, a, b);
                if sse::below_or_equal(c) {
                    ld(c, g, 1, at(c.ebp, 0x1dc))?;
                    ld(c, g, 0, at(c.ebp, 0x1e0))?;
                } else {
                    ld(c, g, 0, at(c.ebp, 0x1d4))?;
                    let a = sse::lane0(c, 0);
                    let b = g.u32(at(c.edi, 0x334))?;
                    sse::comiss(c, a, b);
                    if sse::below_or_equal(c) {
                        ld(c, g, 1, at(c.ebp, 0x1dc))?;
                    } else {
                        ld(c, g, 1, at(c.edi, 0x334))?;
                        sse::ss_reg(c, Ss::Div, 1, 0);
                        opm(c, g, Ss::Mul, 1, at(c.ebp, 0x1dc))?;
                    }
                    // c4fe7
                    ld(c, g, 0, at(c.edi, 0x334))?;
                    ld(c, g, 3, at(c.ebp, 0x1dc))?;
                    sse::ss_reg(c, Ss::Add, 0, 1);
                    opm(c, g, Ss::Mul, 0, at(c.ebp, 0x1e0))?;
                    sse::ss_reg(c, Ss::Add, 3, 2);
                    sse::ss_reg(c, Ss::Div, 0, 3);
                }
                // c501d
                c.eax = c.ebx;
                return call(c, callee::CC380, 0xc5024);
            }
            0xc5024 => {
                if c.eax as u8 != 0 {
                    ld(c, g, 0, SMALL)?;
                }
                sse::movaps(c, 2, 0);
                sse::ss_reg(c, Ss::Add, 2, 1);
                st(c, g, at(c.edi, 0x334), 2)?;
                st(c, g, at(c.edi, 0x338), 0)?;
                return leave(c, g, true);
            }
            0xc5053 => {
                ld(c, g, 0, at(c.edi, 0x334))?;
                c.edx = g.u32(time)?;
                opm(c, g, Ss::Sub, 0, at(c.edx, 4))?;
                let (a, b) = (sse::lane0(c, 1), sse::lane0(c, 0));
                sse::comiss(c, a, b);
                let below = sse::below(c);
                st(c, g, at(c.edi, 0x334), 0)?;
                if !below {
                    st(c, g, at(c.edi, 0x334), 1)?;
                    st(c, g, at(c.edi, 0x338), 1)?;
                }
                0xc5083
            }
            0xc5083 => return leave(c, g, true),
            0xc508f => {
                let al = g.u8(sp(c, 0x13))?;
                c.eax = byte(c.eax, al);
                return leave(c, g, false);
            }
            _ => return Err(unowned(at_)),
        };
    }
}
