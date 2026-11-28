//! The device update `106790` (docs/engine/object-update.md, "Slice 11:
//! small types"): descriptor `devi` slot `+40`, which machines, controls
//! and light fixtures inherit.
//!
//! `(object: stack 0) -> al`; `ret 4`. `al` is 1 when the device changed
//! this tick (the byte at `[esp+13]` below), or 1 on the path that ends in
//! the machine wake (`bd020`, `115b10`).
//!
//! What it does, in retail order. Body `+12c` (device flags, L) loses bit
//! 8 at entry. Then the power half: with flag bit 3 set, the power
//! (`+140`) is moved by `107cc0` toward its target (flag bit 5 latched
//! from its result) and `+144` becomes the change per second; without it,
//! the power follows the power group (`+13c`, the group array at
//! `[4e0328]`, `0xc` per group, value at `+4`): the change rate (`+144`)
//! is limited by the definition's rates (`+c8..+d4`, mixed by the
//! position `+134`), the delay counter `+148` counts ticks against the
//! definition's delay `+10c`, `11eed0` steps the value, and the callouts
//! `107980` (definition `+114`, `+e0`, `+e8`, `+f0`, `+f8`), `107520`
//! and the animation calls `1c6620`, `1c6920` follow its changes. Then
//! the position half with flag bits 4, 6 and the position group
//! (`+130`), in the same shape over `+134`, `+138` and `+1ac`. Last,
//! with flag bit 7 set and either flag bit 3 or a power group, a machine
//! is woken through `bd020` and `115b10`, and bit 7 is cleared when the
//! object's sleep timer `+b3` is zero. Field names are L; the control
//! flow and every access are C.
//!
//! Frame (`esp` = S after `push edi`, `0x30` below the return address):
//! `[S]` `edi`, `[S+4]` `esi`, `[S+8]` `ebp`, `[S+c]` `ebx` saved;
//! `[S+13]` the result byte; `[S+14]` the definition; `[S+18]` the
//! animation block or 0; `[S+1c]` the header offset (`slot × c`);
//! `[S+20..S+2c]` scratch (the value `107cc0` writes, the old change
//! rate, the two limited rates, the old power or position); `[S+30]`
//! the return address; `[S+34]` the object.
//!
//! Across calls: `ebp` the body, `edi` the power or position field,
//! `ebx` the header offset or the change-rate field or the group record,
//! `esi` the group record or the field `11eed0` steps.

use super::sse::{self, Ss};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout;
use guest_layout::points;
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, status};
use runtime::{CpuAbi, Guest};
use x87::abi as fp;

const NONE: u32 = u32::MAX;

/// The device group datum array (L): `0xc` per group, the value at `+4`.
const DEVICE_GROUPS: u32 = 0x004e_0328;
/// binary32 constants the update reads: one (L) and zero.
const ONE: u32 = 0x0045_dbc0;
const ZERO: u32 = 0x0045_dbd8;
/// A function pointer the animation call `1c6920` takes (O).
const CALLBACK: u32 = 0x000b_f600;

/// Moves a value toward its target; `(object, animation, out: stack;
/// eax, ecx field pointers) -> al`; `ret 0c`.
const STEP_TOWARD: u32 = 0x0010_7cc0;
/// `(object: stack 0)`; `ret 4`.
const NOTIFY: u32 = 0x0010_7520;
/// `(object: edx, value: eax, definition: ecx)`; `ret`.
const TRIGGER: u32 = 0x0010_7980;
/// `(record: esi, value: stack 0)`; `ret 4`.
const ANIMATION_SET: u32 = 0x001c_6620;
/// Four stack arguments; `ret 10`.
const ANIMATION_PLAY: u32 = 0x001c_6920;
/// Five stack arguments, `dl`, `esi`, `edi`; `ret 14`.
const LIMITED_STEP: u32 = 0x0011_eed0;
/// `(record: eax) -> al`; `ret`.
const ANIMATION_VALID: u32 = 0x000b_6760;
/// `(object: stack 0)`; `ret 4`.
const MACHINE_WAKE: u32 = 0x000b_d020;
/// `(object: eax, stack 0)`; `ret 4`.
const MACHINE_WAKE_PATH: u32 = 0x0011_5b10;

/// The `fstp st(0)` that ends the one x87 sequence.
const FSTP: u32 = 0x0010_6a64;

/// Every retail return site, in address order.
pub(super) const DEVICE_SITES: [u32; 19] = [
    0x106843, 0x106887, 0x10689f, 0x106935, 0x106a41, 0x106aac, 0x106b1a, 0x106b3d, 0x106b6c, 0x106ba7,
    0x106bc9, 0x106c45, 0x106c51, 0x106c94, 0x106cad, 0x106cc2, 0x106cd9, 0x106d1c, 0x106d27,
];

/// `106790 (object: stack 0) -> al`; `ret 4`.
pub(super) fn device_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    if pc != points::DEVICE_UPDATE.base && !DEVICE_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    run(cx.cpu, cx.g, pc)
}

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}

/// `[esp + off]`.
fn slot(c: &CpuAbi, g: &Guest, off: u32) -> Result<u32> {
    g.u32(at(c.esp, off))
}

/// `cmp r/m16, imm16`.
fn cmp16(c: &mut CpuAbi, a: u32, b: u32) {
    let (a, b) = (a & 0xffff, b & 0xffff);
    let v = a.wrapping_sub(b) & 0xffff;
    let carry = u32::from(a < b);
    let overflow = (((a ^ b) & (a ^ v)) >> 15) & 1;
    c.eflags = (c.eflags & !ARITH) | status(v, 0x8000) | carry | ((a ^ b ^ v) & 0x10) | (overflow << 11);
    c.eflags_defined |= ARITH;
}

/// `lahf; test ah, 44h`: true for `jp` (unordered, or not equal).
fn lahf_jp(c: &mut CpuAbi) -> bool {
    sse::lahf(c);
    logic(c, (c.eax >> 8) & 0x44, 0x80);
    sse::parity(c)
}

/// `106a50..106a64`: `fld [esp+20]; fabs; fld [esp+24]; fxch; fcomip
/// st, st(1); fstp st(0)`. EFLAGS as `fcomip` leaves them.
fn magnitude_compare(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    let (a, b) = (slot(c, g, 0x20)?, slot(c, g, 0x24)?);
    let mut x = fp::X87Abi {
        st: c.x87.st,
        control: c.x87.control,
        status: c.x87.status,
        tag: c.x87.tag,
        ..Default::default()
    };
    fp::ld_f32(&mut x, a);
    fp::unary(&mut x, fp::unary_op::ABS);
    fp::ld_f32(&mut x, b);
    fp::xch(&mut x, 1);
    let f = fp::cmp(&mut x, 0, fp::src_kind::ST, 1, 1, true);
    fp::st_st(&mut x, 0, true);
    c.x87.st = x.st;
    c.x87.status = x.status;
    c.x87.tag = x.tag;
    c.x87.instruction_pointer = FSTP;
    c.x87.opcode = g.u16(FSTP)?.swap_bytes() & 0x7ff;
    c.x87.data_pointer = at(c.esp, 0x24);
    c.x87.instruction_selector = 8;
    c.x87.data_selector = 16;
    c.eflags = (c.eflags & !ARITH) | (f & 0x45);
    c.eflags_defined |= ARITH;
    Ok(())
}

/// Store lane 0 of `xmm r` at `addr`.
fn store_ss(c: &CpuAbi, g: &mut Guest, addr: u32, r: usize) -> Result<()> {
    g.set_u32(addr, sse::lane0(c, r))
}

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

/// Run from `pc` (the entry, a site, or a label inside) to the next
/// transfer. Labels are retail addresses.
fn run(c: &mut CpuAbi, g: &mut Guest, mut pc: u32) -> Result<Next> {
    loop {
        pc = match pc {
            0x106790 => {
                c.esp = arithmetic(c, c.esp, 0x20, true);
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                push(c, g, c.esi)?;
                c.esi = slot(c, g, 0x30)?;
                c.eax = c.esi & 0xffff;
                c.ebx = c.eax * 3;
                c.eax = g.u32(points::OBJECTS.base)?;
                c.ecx = g.u32(at(c.eax, layout::DATA))?;
                c.eax = g.u32(points::TAG_ENTRIES.base)?;
                c.ebx <<= 2;
                c.ebp = g.u32(at(c.ecx, c.ebx + 8))?;
                c.edx = (g.u32(c.ebp)? & 0xffff) << 4;
                let animation = g.u16(at(c.ebp, 0x12a))?;
                c.edx = g.u32(at(c.edx, c.eax.wrapping_add(8)))?;
                g.set_u32(at(c.esp, 0x18), c.ebx)?;
                g.set_u32(at(c.esp, 0x10), c.edx)?;
                g.set_u8(at(c.esp, 0x0f), 0)?;
                g.set_u32(at(c.esp, 0x14), 0)?;
                if animation != 0xffff {
                    c.eax = (animation as i16 as i32 as u32).wrapping_add(c.ebp);
                    g.set_u32(at(c.esp, 0x14), c.eax)?;
                }
                c.ecx = g.u32(at(c.ebp, 0x12c))? & 0xffff_feff;
                logic(c, c.ecx & 8, 0x80);
                push(c, g, c.edi)?;
                g.set_u32(at(c.ebp, 0x12c), c.ecx)?;
                if c.ecx & 8 == 0 {
                    0x1068a4
                } else {
                    logic(c, c.ecx & 0x20, 0x80);
                    if c.ecx & 0x20 != 0 { 0x106b75 } else { 0x106819 }
                }
            }
            0x106819 => {
                c.edx = slot(c, g, 0x18)?;
                sse::load(c, 0, g.u32(at(c.ebp, 0x140))?);
                c.ecx = at(c.esp, 0x20);
                push(c, g, c.ecx)?;
                push(c, g, c.edx)?;
                c.eax = at(c.ebp, 0x18c);
                c.ecx = at(c.ebp, 0x14c);
                push(c, g, c.esi)?;
                store_ss(c, g, at(c.esp, 0x30), 0)?;
                return call(STEP_TOWARD, 0x106843);
            }
            0x106843 => {
                sse::load(c, 0, slot(c, g, 0x20)?);
                c.ecx = g.u32(points::GAME_TIME.base)?;
                store_ss(c, g, at(c.ebp, 0x140), 0)?;
                let old = slot(c, g, 0x24)?;
                sse::ss(c, Ss::Sub, 0, old);
                let dt = g.u32(at(c.ecx, 4))?;
                sse::ss(c, Ss::Div, 0, dt);
                let changed = c.eax as u8;
                logic(c, changed.into(), 0x80);
                c.eax = g.u32(at(c.ebp, 0x12c))?;
                store_ss(c, g, at(c.ebp, 0x144), 0)?;
                c.edx = slot(c, g, 0x34)?;
                c.eax = if changed != 0 { c.eax | 0x20 } else { c.eax & !0x20 };
                logic(c, c.eax, 1 << 31);
                push(c, g, c.edx)?;
                g.set_u32(at(c.ebp, 0x12c), c.eax)?;
                return call(NOTIFY, if changed != 0 { 0x106887 } else { 0x10689f });
            }
            0x106887 | 0x10689f => 0x106b70,
            0x1068a4 => {
                c.eax = g.u32(at(c.ebp, 0x13c))?;
                arithmetic(c, c.eax, NONE, true);
                if c.eax == NONE {
                    0x106b75
                } else {
                    c.esi = g.u32(DEVICE_GROUPS)?;
                    c.esi = g.u32(at(c.esi, layout::DATA))?;
                    c.eax = (c.eax & 0xffff) * 3;
                    sse::load(c, 0, g.u32(at(c.esi, c.eax * 4 + 4))?);
                    let power = g.u32(at(c.ebp, 0x140))?;
                    sse::ucomiss(c, sse::lane0(c, 0), power);
                    c.esi = at(c.esi, c.eax * 4);
                    c.edi = at(c.ebp, 0x140);
                    let jp = lahf_jp(c);
                    g.set_u32(at(c.esp, 0x24), c.esi)?;
                    if jp {
                        0x10690b
                    } else {
                        sse::load(c, 0, g.u32(at(c.ebp, 0x144))?);
                        let zero = g.u32(ZERO)?;
                        sse::ucomiss(c, sse::lane0(c, 0), zero);
                        if lahf_jp(c) {
                            0x10690b
                        } else {
                            logic(c, c.ecx & 0xff, 0x80);
                            if c.ecx & 0x80 != 0 {
                                0x10690b
                            } else {
                                g.set_u16(at(c.ebp, 0x148), 0)?;
                                0x106b75
                            }
                        }
                    }
                }
            }
            0x10690b => {
                c.eax = slot(c, g, 0x18)?;
                logic(c, c.eax, 1 << 31);
                if c.eax == 0 {
                    0x10693d
                } else {
                    c.ecx = g.u32(at(c.ebp, 0x18c))?;
                    arithmetic(c, c.ecx, NONE, true);
                    c.eax = at(c.ebp, 0x18c);
                    if c.ecx == NONE {
                        0x10693d
                    } else {
                        let w = g.u16(at(c.eax, 6))?;
                        cmp16(c, w.into(), 0xffff);
                        if w == 0xffff {
                            0x10693d
                        } else {
                            c.ecx = g.u32(c.edi)?;
                            push(c, g, c.ecx)?;
                            c.esi = c.eax;
                            return call(ANIMATION_SET, 0x106935);
                        }
                    }
                }
            }
            0x106935 => {
                c.esi = slot(c, g, 0x24)?;
                c.edx = slot(c, g, 0x14)?;
                0x10693d
            }
            0x10693d => {
                let position = at(c.ebp, 0x134);
                sse::load(c, 1, g.u32(ONE)?);
                sse::ss(c, Ss::Sub, 1, g.u32(position)?);
                sse::load(c, 0, g.u32(at(c.edx, 0xcc))?);
                sse::load(c, 2, g.u32(at(c.edx, 0xd4))?);
                sse::ss(c, Ss::Mul, 0, g.u32(position)?);
                sse::load(c, 5, g.u32(at(c.edx, 0xc8))?);
                sse::ss(c, Ss::Mul, 5, g.u32(position)?);
                sse::ss_reg(c, Ss::Mul, 2, 1);
                sse::ss_reg(c, Ss::Add, 0, 2);
                sse::load(c, 2, g.u32(at(c.edx, 0xd0))?);
                sse::ss_reg(c, Ss::Mul, 2, 1);
                sse::zero(c, 1);
                sse::ss_reg(c, Ss::Add, 5, 2);
                sse::load(c, 2, g.u32(at(c.ebp, 0x144))?);
                sse::comiss(c, sse::lane0(c, 2), 0);
                c.ebx = at(c.ebp, 0x144);
                store_ss(c, g, at(c.esp, 0x28), 0)?;
                store_ss(c, g, at(c.esp, 0x24), 5)?;
                g.set_u8(at(c.esp, 0x13), 1)?;
                if sse::below_or_equal(c) {
                    g.set_u8(at(c.esp, 0x13), 0)?;
                }
                sse::comiss(c, sse::lane0(c, 0), 0);
                let both = !sse::below_or_equal(c) && {
                    sse::comiss(c, sse::lane0(c, 5), 0);
                    !sse::below_or_equal(c)
                };
                c.eax = g.u32(at(c.ebp, 0x12c))?;
                c.eax = if both { c.eax | 0x100 } else { c.eax & !0x100 };
                logic(c, c.eax, 1 << 31);
                0x1069db
            }
            0x1069db => {
                let delay = g.u16(at(c.ebp, 0x148))?;
                c.ecx = (c.ecx & 0xffff_0000) | u32::from(delay);
                c.edx = delay as i16 as i32 as u32;
                g.set_u32(at(c.ebp, 0x12c), c.eax)?;
                c.eax = slot(c, g, 0x14)?;
                sse::cvtsi2ss(c, 0, c.edx);
                c.edx = g.u32(points::GAME_TIME.base)?;
                sse::ss(c, Ss::Mul, 0, g.u32(at(c.edx, 4))?);
                let limit = g.u32(at(c.eax, 0x10c))?;
                sse::comiss(c, sse::lane0(c, 0), limit);
                if !sse::below(c) {
                    0x106a46
                } else {
                    sse::load(c, 0, g.u32(c.edi)?);
                    sse::ucomiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
                    if lahf_jp(c) {
                        0x106a46
                    } else {
                        let group = g.u32(at(c.esi, 4))?;
                        sse::comiss(c, sse::lane0(c, 0), group);
                        if !sse::below_or_equal(c) {
                            0x106a46
                        } else {
                            c.eax = c.ecx.wrapping_add(1);
                            cmp16(c, c.eax, 1);
                            g.set_u16(at(c.ebp, 0x148), c.eax as u16)?;
                            if c.eax & 0xffff != 1 {
                                0x106b6c
                            } else {
                                c.ecx = slot(c, g, 0x14)?;
                                c.eax = g.u32(at(c.ecx, 0x114))?;
                                c.edx = slot(c, g, 0x34)?;
                                return call(TRIGGER, 0x106a41);
                            }
                        }
                    }
                }
            }
            0x106a41 => 0x106b6c,
            0x106a46 => {
                c.eax = g.u32(c.ebx)?;
                sse::load(c, 0, g.u32(c.edi)?);
                g.set_u32(at(c.esp, 0x20), c.eax)?;
                store_ss(c, g, at(c.esp, 0x2c), 0)?;
                magnitude_compare(c, g)?;
                if !sse::below_or_equal(c) {
                    let up = g.u8(at(c.esp, 0x13))?;
                    c.eax = byte(c.eax, up);
                    logic(c, up.into(), 0x80);
                    if up != 0 {
                        sse::movaps(c, 0, 5);
                    } else {
                        sse::movaps(c, 0, 1);
                        sse::ss_reg(c, Ss::Sub, 0, 5);
                    }
                    store_ss(c, g, c.ebx, 0)?;
                }
                0x106a80
            }
            0x106a80 => {
                c.ecx = g.u32(at(c.edx, 4))?;
                c.eax = slot(c, g, 0x28)?;
                c.edx = slot(c, g, 0x14)?;
                c.edx = byte(c.edx, g.u8(at(c.edx, 0xbc))?);
                push(c, g, 0x3f80_0000)?;
                push(c, g, 0)?;
                push(c, g, c.eax)?;
                c.eax = g.u32(at(c.esi, 4))?;
                push(c, g, c.ecx)?;
                c.edx = byte(c.edx, c.edx as u8 & 1);
                logic(c, c.edx & 0xff, 0x80);
                push(c, g, c.eax)?;
                c.esi = c.ebx;
                g.set_u32(at(c.esp, 0x38), c.ecx)?;
                return call(LIMITED_STEP, 0x106aac);
            }
            0x106aac => {
                let moved = c.eax as u8;
                logic(c, moved.into(), 0x80);
                if moved == 0 {
                    0x106ad0
                } else {
                    let up = g.u8(at(c.esp, 0x13))?;
                    c.eax = byte(c.eax, up);
                    logic(c, up.into(), 0x80);
                    if up != 0 {
                        c.ecx = slot(c, g, 0x14)?;
                        c.eax = g.u32(at(c.ecx, 0xf0))?;
                    } else {
                        c.edx = slot(c, g, 0x14)?;
                        c.eax = g.u32(at(c.edx, 0xf8))?;
                    }
                    0x106b11
                }
            }
            0x106ad0 => {
                sse::load(c, 0, g.u32(c.ebx)?);
                sse::zero(c, 1);
                sse::ucomiss(c, sse::lane0(c, 0), 0);
                if !lahf_jp(c) {
                    0x106b1a
                } else {
                    sse::movaps(c, 2, 0);
                    sse::load(c, 0, slot(c, g, 0x20)?);
                    sse::ss_reg(c, Ss::Mul, 2, 0);
                    sse::comiss(c, 0, sse::lane0(c, 2));
                    if sse::below(c) {
                        0x106b1a
                    } else {
                        sse::load(c, 1, g.u32(c.ebx)?);
                        sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0));
                        if sse::below_or_equal(c) {
                            c.ecx = slot(c, g, 0x14)?;
                            c.eax = g.u32(at(c.ecx, 0xe8))?;
                        } else {
                            c.eax = slot(c, g, 0x14)?;
                            c.eax = g.u32(at(c.eax, 0xe0))?;
                        }
                        0x106b11
                    }
                }
            }
            0x106b11 => {
                c.edx = slot(c, g, 0x34)?;
                return call(TRIGGER, 0x106b1a);
            }
            0x106b1a => {
                sse::load(c, 0, slot(c, g, 0x2c)?);
                let now = g.u32(c.edi)?;
                sse::ucomiss(c, sse::lane0(c, 0), now);
                if lahf_jp(c) {
                    0x106b33
                } else {
                    let flags = g.u8(at(c.ebp, 0x12c))?;
                    c.eax = byte(c.eax, flags);
                    logic(c, flags.into(), 0x80);
                    if flags & 0x80 == 0 { 0x106b6c } else { 0x106b33 }
                }
            }
            0x106b33 => {
                c.ebx = slot(c, g, 0x34)?;
                push(c, g, c.ebx)?;
                return call(NOTIFY, 0x106b3d);
            }
            0x106b3d => {
                c.eax = slot(c, g, 0x18)?;
                logic(c, c.eax, 1 << 31);
                if c.eax == 0 {
                    0x106b6c
                } else {
                    c.ecx = g.u32(at(c.ebp, 0x18c))?;
                    arithmetic(c, c.ecx, NONE, true);
                    c.esi = at(c.ebp, 0x18c);
                    if c.ecx == NONE {
                        0x106b6c
                    } else {
                        let w = g.u16(at(c.esi, 6))?;
                        cmp16(c, w.into(), 0xffff);
                        if w == 0xffff {
                            0x106b6c
                        } else {
                            c.edx = g.u32(c.edi)?;
                            push(c, g, c.ebx)?;
                            push(c, g, CALLBACK)?;
                            push(c, g, c.eax)?;
                            push(c, g, c.edx)?;
                            return call(ANIMATION_PLAY, 0x106b6c);
                        }
                    }
                }
            }
            0x106b6c => {
                c.ebx = slot(c, g, 0x1c)?;
                0x106b70
            }
            0x106b70 => {
                g.set_u8(at(c.esp, 0x13), 1)?;
                0x106b75
            }
            0x106b75 => {
                c.eax = g.u32(at(c.ebp, 0x12c))?;
                logic(c, c.eax & 0x10, 0x80);
                if c.eax & 0x10 == 0 {
                    0x106bd3
                } else {
                    logic(c, c.eax & 0x40, 0x80);
                    if c.eax & 0x40 != 0 {
                        0x106ce2
                    } else {
                        c.ecx = slot(c, g, 0x18)?;
                        c.edx = slot(c, g, 0x34)?;
                        sse::zero(c, 0);
                        push(c, g, 0)?;
                        push(c, g, c.ecx)?;
                        c.eax = at(c.ebp, 0x1ac);
                        c.ecx = at(c.ebp, 0x16c);
                        push(c, g, c.edx)?;
                        return call(STEP_TOWARD, 0x106ba7);
                    }
                }
            }
            0x106ba7 => {
                let moved = c.eax as u8;
                logic(c, moved.into(), 0x80);
                c.eax = g.u32(at(c.ebp, 0x12c))?;
                c.eax = if moved != 0 { c.eax | 0x40 } else { c.eax & !0x40 };
                logic(c, c.eax, 1 << 31);
                g.set_u32(at(c.ebp, 0x12c), c.eax)?;
                c.eax = slot(c, g, 0x34)?;
                push(c, g, c.eax)?;
                return call(NOTIFY, 0x106bc9);
            }
            0x106bc9 => {
                g.set_u8(at(c.esp, 0x13), 1)?;
                0x106ce2
            }
            0x106bd3 => {
                logic(c, c.eax & 8, 0x80);
                if c.eax & 8 != 0 {
                    0x106ce2
                } else {
                    c.eax = g.u32(at(c.ebp, 0x130))?;
                    arithmetic(c, c.eax, NONE, true);
                    if c.eax == NONE {
                        0x106ce2
                    } else {
                        c.edx = g.u32(DEVICE_GROUPS)?;
                        c.eax &= 0xffff;
                        c.ecx = c.eax * 3;
                        c.eax = g.u32(at(c.edx, layout::DATA))?;
                        sse::load(c, 0, g.u32(at(c.eax, c.ecx * 4 + 4))?);
                        let position = g.u32(at(c.ebp, 0x134))?;
                        sse::ucomiss(c, sse::lane0(c, 0), position);
                        c.ebx = at(c.eax, c.ecx * 4);
                        c.edi = at(c.ebp, 0x134);
                        if lahf_jp(c) {
                            0x106c30
                        } else {
                            sse::load(c, 0, g.u32(at(c.ebp, 0x138))?);
                            let zero = g.u32(ZERO)?;
                            sse::ucomiss(c, sse::lane0(c, 0), zero);
                            if lahf_jp(c) { 0x106c30 } else { 0x106cde }
                        }
                    }
                }
            }
            0x106c30 => {
                c.eax = slot(c, g, 0x18)?;
                logic(c, c.eax, 1 << 31);
                if c.eax == 0 {
                    0x106c51
                } else {
                    c.esi = at(c.ebp, 0x1ac);
                    c.eax = c.esi;
                    return call(ANIMATION_VALID, 0x106c45);
                }
            }
            0x106c45 => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax & 0xff == 0 {
                    0x106c51
                } else {
                    c.ecx = g.u32(c.edi)?;
                    push(c, g, c.ecx)?;
                    return call(ANIMATION_SET, 0x106c51);
                }
            }
            0x106c51 => {
                c.edx = slot(c, g, 0x14)?;
                c.eax = g.u32(at(c.edx, 0xc4))?;
                c.ecx = g.u32(points::GAME_TIME.base)?;
                c.edx = g.u32(at(c.ecx, 4))?;
                c.ecx = slot(c, g, 0x14)?;
                sse::load(c, 0, g.u32(c.edi)?);
                sse::load(c, 5, g.u32(at(c.ecx, 0xc0))?);
                push(c, g, 0x3f80_0000)?;
                push(c, g, 0)?;
                push(c, g, c.eax)?;
                c.eax = g.u32(at(c.ebx, 4))?;
                push(c, g, c.edx)?;
                push(c, g, c.eax)?;
                c.esi = at(c.ebp, 0x138);
                c.edx = byte(c.edx, 0);
                logic(c, 0, 0x80);
                store_ss(c, g, at(c.esp, 0x40), 0)?;
                return call(LIMITED_STEP, 0x106c94);
            }
            0x106c94 => {
                sse::load(c, 0, slot(c, g, 0x2c)?);
                let now = g.u32(c.edi)?;
                sse::ucomiss(c, sse::lane0(c, 0), now);
                if !lahf_jp(c) {
                    0x106cd9
                } else {
                    c.edx = slot(c, g, 0x34)?;
                    push(c, g, c.edx)?;
                    return call(NOTIFY, 0x106cad);
                }
            }
            0x106cad => {
                c.ebx = slot(c, g, 0x18)?;
                logic(c, c.ebx, 1 << 31);
                if c.ebx == 0 {
                    0x106cd9
                } else {
                    c.esi = at(c.ebp, 0x1ac);
                    c.eax = c.esi;
                    return call(ANIMATION_VALID, 0x106cc2);
                }
            }
            0x106cc2 => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax & 0xff == 0 {
                    0x106cd9
                } else {
                    c.eax = slot(c, g, 0x34)?;
                    c.ecx = g.u32(c.edi)?;
                    push(c, g, c.eax)?;
                    push(c, g, CALLBACK)?;
                    push(c, g, c.ebx)?;
                    push(c, g, c.ecx)?;
                    return call(ANIMATION_PLAY, 0x106cd9);
                }
            }
            0x106cd9 => {
                g.set_u8(at(c.esp, 0x13), 1)?;
                0x106cde
            }
            0x106cde => {
                c.ebx = slot(c, g, 0x1c)?;
                0x106ce2
            }
            0x106ce2 => {
                c.eax = g.u32(at(c.ebp, 0x12c))?;
                logic(c, c.eax & 8, 0x80);
                c.edi = pop(c, g)?;
                if c.eax & 8 != 0 {
                    0x106cf6
                } else {
                    let group = g.u32(at(c.ebp, 0x13c))?;
                    arithmetic(c, group, NONE, true);
                    if group == NONE { 0x106d5a } else { 0x106cf6 }
                }
            }
            0x106cf6 => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax & 0x80 == 0 {
                    0x106d5a
                } else {
                    c.edx = g.u32(points::OBJECTS.base)?;
                    c.eax = g.u32(at(c.edx, layout::DATA))?;
                    c.ecx = byte(c.ecx, g.u8(at(c.eax, c.ebx + 3))?);
                    c.edx = 1u32 << (c.ecx & 31);
                    logic(c, c.edx & 0xff, 0x80);
                    if c.edx & 0x80 == 0 {
                        0x106d27
                    } else {
                        c.eax = slot(c, g, 0x30)?;
                        push(c, g, c.eax)?;
                        return call(MACHINE_WAKE, 0x106d1c);
                    }
                }
            }
            0x106d1c => {
                c.eax = slot(c, g, 0x30)?;
                push(c, g, 1)?;
                return call(MACHINE_WAKE_PATH, 0x106d27);
            }
            0x106d27 => {
                let object = slot(c, g, 0x30)?;
                arithmetic(c, object, NONE, true);
                let asleep = if object == NONE {
                    true
                } else {
                    c.ecx = g.u32(points::OBJECTS.base)?;
                    c.edx = g.u32(at(c.ecx, layout::DATA))?;
                    c.eax = g.u32(at(c.edx, c.ebx + 8))?;
                    c.ecx = byte(c.ecx, g.u8(at(c.eax, 0xb3))?);
                    logic(c, c.ecx & 0xff, 0x80);
                    c.ecx & 0xff == 0
                };
                if asleep {
                    let v = g.u32(at(c.ebp, 0x12c))? & 0xffff_ff7f;
                    g.set_u32(at(c.ebp, 0x12c), v)?;
                    logic(c, v, 1 << 31);
                }
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.eax = byte(c.eax, 1);
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x20, false);
                return Ok(Next::Ret { pop: 4 });
            }
            0x106d5a => {
                let result = g.u8(at(c.esp, 0x0f))?;
                c.eax = byte(c.eax, result);
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x20, false);
                return Ok(Next::Ret { pop: 4 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}
