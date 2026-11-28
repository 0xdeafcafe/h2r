//! The unit animation state `111650` (slice 6c of
//! docs/engine/sim-rewrite-objects.md; contract in
//! docs/engine/object-update.md, "Slice 6c bodies"): it picks the unit's
//! two animation names, A (set against the block's `+70`) and B (against
//! `+7c`), starts them on the unit's animation block when they change,
//! and steps the three channels of the unit's limits block.
//!
//! The body is a segment over the retail frame, the retail code block by
//! block, keyed by retail address: at each call and at the return the
//! eight integer registers, the `xmm` registers and MXCSR are retail's
//! (the x87 state is never touched). Arithmetic flags are left undefined
//! at every transfer: no callee and no site reads them
//! (`scripts/sim-liveness.py 0x111650`).
//!
//! String IDs the public source must not carry are read from the `imm32`
//! of the instruction that holds them in the loaded image (`imm(g, insn +
//! k)`, `insn` the instruction's address and `k` the offset of its
//! immediate), as are the `push imm32` arguments of `1cb710`, `1c68c0`
//! and `ba350`.
//!
//! **`111650 (unit: stack 0, record: stack 1) -> al`; `ret 8`.** Frame `E`
//! = `esp` after `sub esp, 80`; `ebx`, `ebp`, `esi`, `edi` pushed, so
//! between argument pushes `esp` is `S` = `E - 10`; the code's offsets are
//! the instructions' own (from `S`, or from `S` less the pushes made so
//! far). Slots, from `E`: `[E+1]` the "changed" byte (0; set or cleared
//! by the switch paths, set when `111010` answers 0), `[E+2]` the result byte
//! (`113910`'s `al`, then 1 when a channel plays or `10ee20` answers 0),
//! `[E+3]` the "names given" byte (0; 1 after `e6b00` or `11b710`),
//! `[E+4]` name A, `[E+b]` the "restart" byte, `[E+c]` the body, `[E+10]`
//! name B, `[E+14]` the limits block (body `+ [body+33e]`), `[E+18..21]`
//! a vector (zeroed, then filled, for `e5840`), `[E+24]` the block's B
//! before (`[ebp+7c]`), `[E+28]` the definition (tag entry `+8` of body
//! word 0; later the second channel's `+8`), `[E+2c]` the flags `10f430`
//! takes (0, or 4 when the parent's seat asks), `[E+30]`, `[E+34]` the B
//! and A that `e6b00` writes, `[E+38]` the record's byte `+8` (0 unless
//! the record gives B), `[E+3c]` the block's A before (`[ebp+70]`),
//! `[E+40..48]` the vector `1c73a0` writes, `[E+4c..]` the frame `ba160`
//! writes, `[E+84]` the unit, `[E+88]` the record.
//!
//! Registers across calls: `ebp` the animation block (body `+
//! [body+12a]`) until `111fb7`, where it becomes the third channel's
//! `+8`; `edi` the unit index times 12 until `1119a8`, then `S+28`, the
//! unit (`1119d9`), the parent handle (`111bea`), the count of the
//! weapon-slot loop, and in the tail the channel (limits `+dc`, `+9c`,
//! `+bc`); `ebx` the body, the unit (`1118ad`), the seat index
//! (`111986`), B (`111af8`), the "kill" byte `bl` (`111b78`), the body
//! (`111be6`), the weapon-slot pointer, the unit, the second channel's
//! `+8`, the unit, and last the result byte; `esi` the limits block, the
//! animation block (`1cb710`), the unit (`e6a20`), B before (`1118a9`,
//! across `e6b00` and the switch), the unit (`111bbd`), A (`111c7c`), and
//! -1 from `111d3c`/`111d6f`.
//!
//! In order: `113910` (`eax` the unit). The record's two names (`[rec]`,
//! `[rec+4]`, each unless -1 or 0) replace the defaults; with B comes the
//! record's byte `+8`. With no parent (body `+14` -1), A becomes the
//! block's `+70` when that is the name the `11171c` instruction carries,
//! else a fixed name when body `+10a` bit 2 is set, else B the default
//! when the record word (body `+ [body+346]`) has a non-zero word `+36`;
//! otherwise, when A is still the default, `10eef0` (`eax` the unit, `dl`
//! the record's byte `+9`, one stack 0) names it, a fixed name wins when
//! body byte `+1f4` is non-zero, and the limits block's `+2c` wins when
//! not -1. Then, when the block plays (`[ebp]` and word `[ebp+6]` not
//! -1): `1cb710` (five stack arguments, `esi` the block), `e6a20` when
//! word `[ebp+14]` is non-zero, and unless `[ebp+6c]` bit 0, `[ebp+12]`
//! bit 0 clear or `[ebp+11] & 9`: `1c6440` (`ecx` the block), whose word
//! `+14` (signed) must exceed 1, and then `[E+2]` = 1 and `[ebp+11] & a`
//! must be set. Word `[ebp+12]` with bits 14 and 1 stops there (`[E+1]`
//! 0); with bit 14 alone and the limits block's byte `+1` bit 0 clear,
//! word `[ebp+6c] |= 1` and it stops too. Otherwise `e6b00` gives A and B
//! for B before, and a signed `cmp` ladder on B before runs one of: the
//! seat vector (`bfc90`, `1c7380`, `10eea0`, `10ee70`, `13e420`, `ba160`,
//! `1c73a0`, `142570`, `b8bd0`, or the vector at `[4687a4]`) and
//! `e5840`; the three channel marks (`or byte [ebp+51|11|31], 1`);
//! `e4300`; `11b710`; the kill test (`10f5f0`, `a7670`, then `d6bc0`, or
//! the limits byte `|= 4` and `[ebp+6c] |= 1`); `[E+1]` 0; or nothing.
//! Every path meets at `111bbd`: `111010`, and for a unit with a parent
//! `c7070` and the parent's seat flags. `111c69` decides whether the
//! names changed (`al`) or must restart (`[E+b]`): `1103a0` unless the
//! names were given, then `10f430` starts them (with `1ccba0` when B is
//! or returns to the idle name), and `10ee20`; or, once in 16 ticks per
//! unit (the `[55e70c]` counter), `1ce180` and `1060a0` for each held
//! weapon. Unless `[ebp+6c]` bit 0, the three channels each get
//! `1c68c0`, `e6a20`, `11b710`, `e6b00`, `110fc0`, `1c63f0`, `114330`
//! and `ba350` as their flags ask, and set `[E+2]` when they play. Last
//! `10dc70` and `10eaf0`; `al` is the OR of their answers and `[E+2]`.

use super::frame::{set_slot, slot};
use super::functions::{OBJECTS, Ss, TAGS, call, imm, lo, load, ret, ss};
use super::{Cx, Next, Result, unowned};
use guest_layout::points;
use crate::sim_physics::{byte, pop, push};
use runtime::{CpuAbi, Guest};

const NONE: u32 = u32::MAX;
/// The pointer to the zero vector.
const VECTOR_A4: u32 = 0x0046_87a4;
/// The tick counter the weapon refresh is spread over.
const COUNTER: u32 = 0x0055_e70c;

/// The callees, every one a guest callout.
mod callee {
    pub const X113910: u32 = 0x0011_3910;
    pub const X10EEF0: u32 = 0x0010_eef0;
    pub const X1CB710: u32 = 0x001c_b710;
    pub const XE6A20: u32 = 0x000e_6a20;
    pub const X1C6440: u32 = 0x001c_6440;
    pub const XE6B00: u32 = 0x000e_6b00;
    pub const XBFC90: u32 = 0x000b_fc90;
    pub const X1C7380: u32 = 0x001c_7380;
    pub const X10EEA0: u32 = 0x0010_eea0;
    pub const X10EE70: u32 = 0x0010_ee70;
    pub const X13E420: u32 = 0x0013_e420;
    pub const XBA160: u32 = 0x000b_a160;
    pub const X1C73A0: u32 = 0x001c_73a0;
    pub const X142570: u32 = 0x0014_2570;
    pub const XB8BD0: u32 = 0x000b_8bd0;
    pub const XE4300: u32 = 0x000e_4300;
    pub const XE5840: u32 = 0x000e_5840;
    pub const X11B710: u32 = 0x0011_b710;
    pub const X10F5F0: u32 = 0x0010_f5f0;
    pub const XA7670: u32 = 0x000a_7670;
    pub const XD6BC0: u32 = 0x000d_6bc0;
    pub const X111010: u32 = 0x0011_1010;
    pub const XC7070: u32 = 0x000c_7070;
    pub const X1103A0: u32 = 0x0011_03a0;
    pub const X10F430: u32 = 0x0010_f430;
    pub const X1CCBA0: u32 = 0x001c_cba0;
    pub const X10EE20: u32 = 0x0010_ee20;
    pub const X1CE180: u32 = 0x001c_e180;
    pub const X1060A0: u32 = 0x0010_60a0;
    pub const X1C68C0: u32 = 0x001c_68c0;
    pub const X1C63F0: u32 = 0x001c_63f0;
    pub const X114330: u32 = 0x0011_4330;
    pub const X110FC0: u32 = 0x0011_0fc0;
    pub const XBA350: u32 = 0x000b_a350;
    pub const X10DC70: u32 = 0x0010_dc70;
    pub const X10EAF0: u32 = 0x0010_eaf0;
}
use callee::*;

/// `111650`'s return sites, in address order.
pub(super) const ANIM_SITES: [u32; 47] = [
    0x1116d7, 0x11178d, 0x111803, 0x11182f, 0x111854, 0x1118d1, 0x11195d, 0x1119b3, 0x1119be, 0x1119d3, 0x1119d9,
    0x1119ef, 0x1119fd, 0x111a0b, 0x111a13, 0x111acd, 0x111b20, 0x111b57, 0x111b85, 0x111ba2, 0x111bb0, 0x111bcb,
    0x111bf9, 0x111cda, 0x111d10, 0x111d3c, 0x111d64, 0x111daa, 0x111dd2, 0x111e1c, 0x111e36, 0x111e44, 0x111ea4,
    0x111ecb, 0x111efd, 0x111f09, 0x111f19, 0x111f24, 0x111f42, 0x111fb0, 0x111fd3, 0x111fea, 0x111ff6, 0x112001,
    0x11201a, 0x112045, 0x112052,
];

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}

fn sx16(v: u16) -> u32 {
    v as i16 as i32 as u32
}

/// `mov byte ptr [esp+off], v`.
fn set8(c: &CpuAbi, g: &mut Guest, off: u32, v: u8) -> Result<()> {
    g.set_u8(at(c.esp, off), v)
}

/// The byte at `[esp+off]`.
fn get8(c: &CpuAbi, g: &Guest, off: u32) -> Result<u8> {
    g.u8(at(c.esp, off))
}

/// `or`/`and byte ptr [addr], k`.
fn rmw8(g: &mut Guest, addr: u32, f: impl FnOnce(u8) -> u8) -> Result<()> {
    let v = f(g.u8(addr)?);
    g.set_u8(addr, v)
}

/// The "channel plays" test at `111e44`, `111f42` and `11201a`: dword
/// `[x]` and word `[x+6]` not `none`, byte `[x+12]` bit 0 set and byte
/// `[x+11] & 9` clear. Reads in retail's order, stopping where it stops.
fn plays(g: &Guest, x: u32, none: u32) -> Result<bool> {
    Ok(g.u32(x)? != none
        && u32::from(g.u16(at(x, 6))?) != none & 0xffff
        && g.u8(at(x, 0x12))? & 1 != 0
        && g.u8(at(x, 0x11))? & 9 == 0)
}

/// A string ID's `imm32` as the signed operand of `cmp`.
fn k(g: &Guest, insn_imm: u32) -> Result<i32> {
    Ok(imm(g, insn_imm)? as i32)
}

/// Whether `pc` is `entry` or one of `sites`.
fn owned(pc: u32, entry: u32, sites: &[u32]) -> Result<()> {
    if pc == entry || sites.contains(&pc) { Ok(()) } else { Err(unowned(pc)) }
}

/// `111650 (unit: stack 0, record: stack 1) -> al`.
pub(super) fn anim(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::UNIT_ANIMATION_STATE.base;
    owned(pc, entry, &ANIM_SITES)?;
    let mut at_pc = pc;
    loop {
        at_pc = match at_pc {
            0x111650 => {
                c.esp = c.esp.wrapping_sub(0x80);
                c.ecx = slot(c, g, 0x84)?;
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                push(c, g, c.esi)?;
                c.eax = c.ecx & 0xffff;
                push(c, g, c.edi)?;
                c.edi = c.eax.wrapping_mul(3);
                c.eax = g.u32(OBJECTS)?;
                c.edx = g.u32(at(c.eax, 0x44))?;
                c.edi <<= 2;
                c.ebx = g.u32(at(at(c.edi, c.edx), 8))?;
                c.eax = g.u32(c.ebx)?;
                c.edx = g.u32(TAGS)?;
                c.esi = sx16(g.u16(at(c.ebx, 0x33e))?);
                c.ebp = sx16(g.u16(at(c.ebx, 0x12a))?);
                c.eax = (c.eax & 0xffff) << 4;
                c.eax = g.u32(at(at(c.eax, c.edx), 8))?;
                set_slot(c, g, 0x38, c.eax)?;
                c.eax = imm(g, 0x1116a0 + 1)?;
                set_slot(c, g, 0x14, c.eax)?;
                set_slot(c, g, 0x20, c.eax)?;
                c.esi = c.esi.wrapping_add(c.ebx);
                c.eax = c.ecx;
                set_slot(c, g, 0x1c, c.ebx)?;
                set_slot(c, g, 0x24, c.esi)?;
                c.ebp = c.ebp.wrapping_add(c.ebx);
                set8(c, g, 0x11, 0)?;
                set8(c, g, 0x13, 0)?;
                set8(c, g, 0x48, 0)?;
                set_slot(c, g, 0x3c, 0)?;
                return call(c, X113910, 0x1116d7);
            }
            0x1116d7 => {
                c.ecx = slot(c, g, 0x98)?;
                set8(c, g, 0x12, c.eax as u8)?;
                c.eax = g.u32(c.ecx)?;
                if c.eax != NONE && c.eax != 0 {
                    set_slot(c, g, 0x14, c.eax)?;
                }
                // 1116f1
                c.ecx = slot(c, g, 0x98)?;
                c.eax = g.u32(at(c.ecx, 4))?;
                if c.eax != NONE && c.eax != 0 {
                    c.edx = byte(c.edx, g.u8(at(c.ecx, 8))?);
                    set_slot(c, g, 0x20, c.eax)?;
                    set8(c, g, 0x48, c.edx as u8)?;
                }
                // 11170f
                if g.u32(at(c.ebx, 0x14))? != NONE {
                    0x1117af
                } else {
                    c.ecx = g.u32(at(c.ebp, 0x70))?;
                    c.eax = imm(g, 0x11171c + 1)?;
                    if c.ecx == c.eax {
                        set_slot(c, g, 0x14, c.eax)?;
                        0x1117af
                    } else {
                        // 11172e
                        let al = g.u8(at(c.ebx, 0x10a))? >> 2;
                        c.eax = byte(c.eax, al);
                        if al & 1 != 0 {
                            let name = imm(g, 0x11173b + 4)?;
                            set_slot(c, g, 0x14, name)?;
                            0x1117af
                        } else {
                            0x111745
                        }
                    }
                }
            }
            0x111745 => {
                c.ecx = g.u32(OBJECTS)?;
                c.edx = g.u32(at(c.ecx, 0x44))?;
                c.eax = g.u32(at(at(c.edi, c.edx), 8))?;
                c.ecx = sx16(g.u16(at(c.eax, 0x346))?);
                if g.u16(at(at(c.ecx, c.eax), 0x36))? != 0 {
                    let name = imm(g, 0x111761 + 4)?;
                    set_slot(c, g, 0x20, name)?;
                    0x1117af
                } else if slot(c, g, 0x14)? != imm(g, 0x11176b + 4)? {
                    0x111791
                } else {
                    c.edx = slot(c, g, 0x98)?;
                    c.edx = byte(c.edx, g.u8(at(c.edx, 9))?);
                    c.eax = slot(c, g, 0x94)?;
                    push(c, g, 0)?;
                    return call(c, X10EEF0, 0x11178d);
                }
            }
            0x11178d => {
                set_slot(c, g, 0x14, c.eax)?;
                0x111791
            }
            0x111791 => {
                let al = g.u8(at(c.ebx, 0x1f4))?;
                c.eax = byte(c.eax, al);
                if al != 0 {
                    let name = imm(g, 0x11179b + 4)?;
                    set_slot(c, g, 0x14, name)?;
                }
                // 1117a3
                c.esi = g.u32(at(c.esi, 0x2c))?;
                if c.esi != NONE {
                    set_slot(c, g, 0x14, c.esi)?;
                }
                0x1117af
            }
            0x1117af => {
                if g.u32(c.ebp)? == NONE {
                    0x111c7a
                } else if g.u16(at(c.ebp, 6))? == 0xffff {
                    0x111c69
                } else {
                    c.eax = g.u32(OBJECTS)?;
                    c.ecx = g.u32(at(c.eax, 0x44))?;
                    c.eax = g.u32(at(at(c.ecx, c.edi), 8))?;
                    c.edx = sx16(g.u16(at(c.eax, 0x112))?);
                    c.ecx = sx16(g.u16(at(c.eax, 0x10e))?);
                    c.edx = c.edx.wrapping_add(c.eax);
                    push(c, g, c.edx)?;
                    c.edx = sx16(g.u16(at(c.eax, 0x110))?);
                    c.ecx = c.ecx.wrapping_add(c.eax);
                    c.eax = slot(c, g, 0x98)?;
                    push(c, g, c.ecx)?;
                    c.edx >>= 5;
                    push(c, g, c.edx)?;
                    push(c, g, c.eax)?;
                    let k = imm(g, 0x1117f7 + 1)?;
                    push(c, g, k)?;
                    c.esi = c.ebp;
                    return call(c, X1CB710, 0x111803);
                }
            }
            0x111803 => {
                c.ecx = g.u32(at(c.ebp, 0x70))?;
                c.eax = g.u32(at(c.ebp, 0x7c))?;
                set_slot(c, g, 0x4c, c.ecx)?;
                let cx = g.u16(at(c.ebp, 0x14))?;
                c.ecx = (c.ecx & !0xffff) | u32::from(cx);
                set_slot(c, g, 0x34, c.eax)?;
                if cx == 0 {
                    0x11182f
                } else {
                    c.edx = u32::from(g.u16(at(c.ebp, 0x16))?);
                    c.esi = slot(c, g, 0x94)?;
                    c.ecx = u32::from(cx);
                    push(c, g, c.edx)?;
                    push(c, g, c.ecx)?;
                    return call(c, XE6A20, 0x11182f);
                }
            }
            0x11182f => {
                if g.u8(at(c.ebp, 0x6c))? & 1 != 0 || g.u8(at(c.ebp, 0x12))? & 1 == 0 || g.u8(at(c.ebp, 0x11))? & 9 != 0
                {
                    0x111c69
                } else {
                    c.ecx = c.ebp;
                    return call(c, X1C6440, 0x111854);
                }
            }
            0x111854 => {
                c.edx = 1;
                if (g.u16(at(c.eax, 0x14))? as i16) <= 1 {
                    0x111c69
                } else {
                    let t = g.u8(at(c.ebp, 0x11))? & 0xa;
                    set8(c, g, 0x12, c.edx as u8)?;
                    if t == 0 {
                        0x111c69
                    } else {
                        c.eax = u32::from(g.u16(at(c.ebp, 0x12))?);
                        c.ecx = c.eax & 0x4000;
                        if c.ecx != 0 && c.eax & 2 != 0 {
                            set8(c, g, 0x11, 0)?;
                            0x111c69
                        } else if c.ecx != 0 {
                            // 111891
                            c.eax = slot(c, g, 0x24)?;
                            if g.u8(at(c.eax, 1))? & 1 != 0 {
                                0x1118a9
                            } else {
                                let w = g.u16(at(c.ebp, 0x6c))? | c.edx as u16;
                                g.set_u16(at(c.ebp, 0x6c), w)?;
                                set8(c, g, 0x11, 0)?;
                                0x111c69
                            }
                        } else {
                            0x1118a9
                        }
                    }
                }
            }
            0x1118a9 => {
                c.esi = slot(c, g, 0x34)?;
                c.ebx = slot(c, g, 0x94)?;
                c.ecx = at(c.esp, 0x40);
                push(c, g, c.ecx)?;
                c.edx = at(c.esp, 0x48);
                c.eax = imm(g, 0x1118bd + 1)?;
                push(c, g, c.edx)?;
                push(c, g, c.esi)?;
                set_slot(c, g, 0x50, c.eax)?;
                set_slot(c, g, 0x4c, c.eax)?;
                return call(c, XE6B00, 0x1118d1);
            }
            0x1118d1 => {
                c.eax = slot(c, g, 0x44)?;
                c.ecx = slot(c, g, 0x40)?;
                if c.eax != imm(g, 0x1118d5 + 1)? || c.ecx != imm(g, 0x1118e0 + 2)? {
                    // 1118e8
                    set_slot(c, g, 0x14, c.eax)?;
                    set_slot(c, g, 0x20, c.ecx)?;
                    set8(c, g, 0x13, 1)?;
                }
                0x1118f5
            }
            0x1118f5 => {
                // The signed `cmp` ladder over the state name before.
                let s = c.esi as i32;
                if s > k(g, 0x1118f5 + 2)? {
                    0x111b2e
                } else if s == k(g, 0x1118f5 + 2)? {
                    0x11193f
                } else if s > k(g, 0x111903 + 2)? {
                    0x111aa3
                } else if s >= k(g, 0x11190f + 2)? {
                    0x111aaf
                } else if s < k(g, 0x11191b + 2)? {
                    0x111bbd
                } else if s <= k(g, 0x111927 + 2)? {
                    0x111a4e
                } else if s != k(g, 0x111933 + 2)? {
                    0x111bbd
                } else {
                    0x11193f
                }
            }
            0x11193f => {
                c.ecx = slot(c, g, 0x1c)?;
                let al = g.u8(at(c.ecx, 0xaa))?;
                c.eax = byte(c.eax, al);
                if al != 0 {
                    0x111bbd
                } else {
                    c.ecx = slot(c, g, 0x94)?;
                    return call(c, XBFC90, 0x11195d);
                }
            }
            0x11195d => {
                if c.eax as u8 != 0 {
                    0x111bbd
                } else {
                    c.edx = g.u32(OBJECTS)?;
                    c.eax = g.u32(at(c.edx, 0x44))?;
                    c.ecx = g.u32(at(at(c.eax, c.edi), 8))?;
                    c.edx = g.u32(c.ecx)?;
                    c.eax = g.u32(TAGS)?;
                    c.edx = (c.edx & 0xffff) << 4;
                    c.ecx = g.u32(at(at(c.edx, c.eax), 8))?;
                    c.ebx = sx16(g.u16(at(c.ecx, 0x254))?);
                    if c.ebx == NONE {
                        0x111bbd
                    } else {
                        c.eax = 0;
                        set_slot(c, g, 0x28, 0)?;
                        set_slot(c, g, 0x2c, 0)?;
                        set8(c, g, 0x30, 0)?;
                        set8(c, g, 0x31, 0)?;
                        c.edi = at(c.esp, 0x28);
                        c.eax = c.ebp;
                        return call(c, X1C7380, 0x1119b3);
                    }
                }
            }
            0x1119b3 => {
                c.ecx = c.ebx;
                c.edx = at(c.esp, 0x28);
                return call(c, X10EEA0, 0x1119be);
            }
            0x1119be => {
                if c.eax as u8 == 0 {
                    0x111ad7
                } else {
                    c.edx = c.ebx.wrapping_add(1);
                    push(c, g, c.edx)?;
                    c.edx = at(c.esp, 0x2c);
                    return call(c, X10EE70, 0x1119d3);
                }
            }
            0x1119d3 => {
                push(c, g, c.eax)?;
                return call(c, X13E420, 0x1119d9);
            }
            0x1119d9 => {
                c.edi = slot(c, g, 0x94)?;
                c.esi = c.eax;
                c.eax = at(c.esp, 0x5c);
                push(c, g, c.eax)?;
                c.eax = c.edi;
                c.esi = c.esi.wrapping_sub(1);
                return call(c, XBA160, 0x1119ef);
            }
            0x1119ef => {
                push(c, g, c.esi)?;
                push(c, g, 0)?;
                c.esi = at(c.esp, 0x58);
                c.eax = c.ebp;
                return call(c, X1C73A0, 0x1119fd);
            }
            0x1119fd => {
                c.ecx = at(c.esp, 0x5c);
                push(c, g, c.ecx)?;
                c.edx = c.esi;
                c.eax = c.esi;
                return call(c, X142570, 0x111a0b);
            }
            0x111a0b => {
                push(c, g, c.ebx)?;
                c.eax = c.edi;
                return call(c, XB8BD0, 0x111a13);
            }
            0x111a13 => {
                // The seat position less the vector at `S+50`, into `S+28`.
                for (i, (from, minus, to)) in
                    [(0x28, 0x50, 0x28), (4, 0x54, 0x2c), (8, 0x58, 0x30)].into_iter().enumerate()
                {
                    load(c, 0, g.u32(at(c.eax, from))?);
                    let b = slot(c, g, minus)?;
                    ss(c, Ss::Sub, 0, b)?;
                    if i == 0 {
                        c.eax = c.eax.wrapping_add(0x28);
                    }
                    g.set_u32(at(c.esp, to), lo(c, 0))?;
                }
                0x111af8
            }
            0x111a4e => {
                c.ecx = g.u32(at(c.ebp, 0x40))?;
                c.eax = NONE;
                if c.ecx != c.eax && g.u16(at(c.ebp, 0x46))? != 0xffff && g.u8(at(c.ebp, 0x52))? & 1 != 0 {
                    rmw8(g, at(c.ebp, 0x51), |v| v | 1)?;
                }
                // 111a68
                if g.u32(c.ebp)? != c.eax && g.u16(at(c.ebp, 6))? != 0xffff && g.u8(at(c.ebp, 0x12))? & 1 != 0 {
                    rmw8(g, at(c.ebp, 0x11), |v| v | 1)?;
                }
                // 111a7d
                if g.u32(at(c.ebp, 0x20))? != c.eax
                    && g.u16(at(c.ebp, 0x26))? != 0xffff
                    && g.u8(at(c.ebp, 0x32))? & 1 != 0
                {
                    rmw8(g, at(c.ebp, 0x31), |v| v | 1)?;
                }
                0x111bbd
            }
            0x111aa3 => {
                if c.esi != imm(g, 0x111aa3 + 2)? {
                    0x111bbd
                } else {
                    0x111aaf
                }
            }
            0x111aaf => {
                c.eax = slot(c, g, 0x1c)?;
                let cl = g.u8(at(c.eax, 0xaa))?;
                c.ecx = byte(c.ecx, cl);
                if cl != 0 {
                    0x111bbd
                } else {
                    c.eax = slot(c, g, 0x94)?;
                    return call(c, XE4300, 0x111acd);
                }
            }
            0x111acd => {
                set8(c, g, 0x11, 1)?;
                0x111bbd
            }
            0x111ad7 => {
                c.edx = g.u32(VECTOR_A4)?;
                c.eax = g.u32(c.edx)?;
                c.ecx = g.u32(at(c.edx, 4))?;
                c.edx = g.u32(at(c.edx, 8))?;
                c.edi = slot(c, g, 0x94)?;
                set_slot(c, g, 0x28, c.eax)?;
                set_slot(c, g, 0x2c, c.ecx)?;
                set_slot(c, g, 0x30, c.edx)?;
                0x111af8
            }
            0x111af8 => {
                c.ebx = slot(c, g, 0x20)?;
                if c.ebx == slot(c, g, 0x34)? {
                    c.eax = slot(c, g, 0x4c)?;
                    if slot(c, g, 0x14)? == c.eax {
                        c.eax = byte(c.eax, 0);
                    } else {
                        c.eax = 1;
                    }
                } else {
                    c.eax = 1;
                }
                // 111b14
                push(c, g, c.eax)?;
                c.ecx = at(c.esp, 0x2c);
                c.esi = c.edi;
                return call(c, XE5840, 0x111b20);
            }
            0x111b20 => {
                let changed = c.ebx != slot(c, g, 0x34)?;
                set8(c, g, 0x11, u8::from(changed))?;
                0x111bbd
            }
            0x111b2e => {
                if c.esi == imm(g, 0x111b2e + 2)? {
                    0x111b65
                } else if c.esi == imm(g, 0x111b36 + 2)? {
                    0x111b5e
                } else if c.esi != imm(g, 0x111b3e + 2)? {
                    0x111bbd
                } else {
                    c.ecx = slot(c, g, 0x14)?;
                    c.eax = slot(c, g, 0x94)?;
                    push(c, g, c.ecx)?;
                    return call(c, X11B710, 0x111b57);
                }
            }
            0x111b57 => {
                set8(c, g, 0x13, 1)?;
                0x111bbd
            }
            0x111b5e => {
                set8(c, g, 0x11, 0)?;
                0x111bbd
            }
            0x111b65 => {
                c.edx = slot(c, g, 0x38)?;
                c.eax = g.u32(at(c.edx, 0xbc))?;
                c.esi = slot(c, g, 0x94)?;
                c.eax >>= 1;
                c.ebx = byte(c.ebx, 0);
                if c.eax & 1 == 0 {
                    0x111b9b
                } else {
                    c.eax = c.esi;
                    return call(c, X10F5F0, 0x111b85);
                }
            }
            0x111b85 => {
                if c.eax != imm(g, 0x111b85 + 1)? {
                    0x111b99
                } else {
                    c.ecx = slot(c, g, 0x1c)?;
                    if g.u8(at(c.ecx, 0xc1))? & 1 == 0 { 0x111b9b } else { 0x111b99 }
                }
            }
            0x111b99 => {
                c.ebx = byte(c.ebx, 1);
                0x111b9b
            }
            0x111b9b => {
                c.eax = c.esi;
                return call(c, XA7670, 0x111ba2);
            }
            0x111ba2 => {
                if c.eax as u8 != 0 || c.ebx as u8 == 0 {
                    0x111bb2
                } else {
                    push(c, g, c.esi)?;
                    return call(c, XD6BC0, 0x111bb0);
                }
            }
            0x111bb0 => 0x111bbd,
            0x111bb2 => {
                c.eax = slot(c, g, 0x24)?;
                rmw8(g, c.eax, |v| v | 4)?;
                rmw8(g, at(c.ebp, 0x6c), |v| v | 1)?;
                0x111bbd
            }
            0x111bbd => {
                c.esi = slot(c, g, 0x94)?;
                c.ecx = c.esi;
                return call(c, X111010, 0x111bcb);
            }
            0x111bcb => {
                if c.eax as u8 == 0 {
                    let name = slot(c, g, 0x20)?;
                    set8(c, g, 0x11, 1)?;
                    if name == imm(g, 0x111bcf + 4)? {
                        let idle = imm(g, 0x111bde + 4)?;
                        set_slot(c, g, 0x20, idle)?;
                    }
                }
                0x111be6
            }
            0x111be6 => {
                c.ebx = slot(c, g, 0x1c)?;
                c.edi = g.u32(at(c.ebx, 0x14))?;
                if c.edi == NONE {
                    0x111c65
                } else {
                    c.ecx = c.esi;
                    return call(c, XC7070, 0x111bf9);
                }
            }
            0x111bf9 => {
                if c.eax as u8 == 0 {
                    0x111c65
                } else {
                    c.eax = g.u32(OBJECTS)?;
                    c.ecx = g.u32(at(c.eax, 0x44))?;
                    c.edi &= 0xffff;
                    c.edx = c.edi.wrapping_mul(3);
                    c.edx = g.u32(at(at(c.ecx, c.edx.wrapping_mul(4)), 8))?;
                    c.eax = g.u32(c.edx)?;
                    c.ecx = g.u32(TAGS)?;
                    c.eax = (c.eax & 0xffff) << 4;
                    c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
                    c.eax = sx16(g.u16(at(c.ebx, 0x1fc))?);
                    c.edi = g.u32(at(c.edx, 0x1cc))?;
                    c.eax = c.eax.wrapping_mul(0xb0);
                    c.ecx = g.u32(at(c.eax, c.edi))?;
                    c.eax = c.eax.wrapping_add(c.edi);
                    c.ecx >>= 11;
                    if c.ecx & 1 == 0 || g.u16(at(c.eax, 0x3e))? == 0xffff {
                        0x111c65
                    } else {
                        set_slot(c, g, 0x3c, 4)?;
                        let state = imm(g, 0x111c55 + 4)?;
                        set_slot(c, g, 0x14, state)?;
                        let overlay = imm(g, 0x111c5d + 4)?;
                        set_slot(c, g, 0x20, overlay)?;
                        0x111c65
                    }
                }
            }
            0x111c65 => {
                c.ebx = slot(c, g, 0x1c)?;
                0x111c69
            }
            0x111c69 => {
                if g.u32(c.ebp)? == NONE || g.u16(at(c.ebp, 6))? == 0xffff {
                    0x111c7a
                } else {
                    c.ecx = byte(c.ecx, 1);
                    0x111c7c
                }
            }
            0x111c7a => {
                c.ecx = byte(c.ecx, 0);
                0x111c7c
            }
            0x111c7c => {
                c.esi = slot(c, g, 0x14)?;
                c.edx = g.u32(at(c.ebp, 0x70))?;
                c.eax = byte(c.eax, 0);
                set8(c, g, 0x1b, 0)?;
                if c.esi != c.edx && c.esi != imm(g, 0x111c8d + 2)? {
                    c.eax = byte(c.eax, 1);
                }
                // 111c97
                c.edi = slot(c, g, 0x20)?;
                if c.edi != g.u32(at(c.ebp, 0x7c))? && c.edi != imm(g, 0x111ca0 + 2)? {
                    c.eax = byte(c.eax, 1);
                }
                // 111caa
                let dl = get8(c, g, 0x11)?;
                c.edx = byte(c.edx, dl);
                if dl != 0 || c.ecx as u8 == 0 {
                    set8(c, g, 0x1b, 1)?;
                }
                // 111cbb
                if c.eax as u8 == 0 {
                    0x111cde
                } else {
                    let al = get8(c, g, 0x13)?;
                    c.eax = byte(c.eax, al);
                    if al != 0 {
                        0x111cea
                    } else {
                        c.edx = slot(c, g, 0x48)?;
                        c.eax = slot(c, g, 0x94)?;
                        push(c, g, c.edx)?;
                        push(c, g, c.edi)?;
                        push(c, g, c.eax)?;
                        return call(c, X1103A0, 0x111cda);
                    }
                }
            }
            0x111cda => {
                if c.eax as u8 != 0 {
                    0x111cea
                } else {
                    0x111cde
                }
            }
            0x111cde => {
                let al = get8(c, g, 0x1b)?;
                c.eax = byte(c.eax, al);
                if al == 0 { 0x111d6f } else { 0x111cea }
            }
            0x111cea => {
                c.eax = slot(c, g, 0x3c)?;
                c.ebx = slot(c, g, 0x94)?;
                push(c, g, 0)?;
                push(c, g, 0)?;
                push(c, g, c.edi)?;
                let k = imm(g, 0x111cfa + 1)?;
                push(c, g, k)?;
                let k = imm(g, 0x111cff + 1)?;
                push(c, g, k)?;
                push(c, g, c.esi)?;
                c.eax |= 0x401;
                push(c, g, c.ebx)?;
                return call(c, X10F430, 0x111d10);
            }
            0x111d10 => {
                if c.eax as u8 != 0 {
                    0x111d3c
                } else if c.edi == imm(g, 0x111d14 + 2)? {
                    if g.u32(at(c.ebp, 0x7c))? == imm(g, 0x111d1c + 3)? { 0x111d2f } else { 0x111d3c }
                } else if c.edi != imm(g, 0x111d27 + 2)? {
                    0x111d3c
                } else {
                    0x111d2f
                }
            }
            0x111d2f => {
                if g.u8(at(c.ebp, 0x6e))? & 1 != 0 {
                    0x111d3c
                } else {
                    c.eax = c.ebp;
                    return call(c, X1CCBA0, 0x111d3c);
                }
            }
            0x111d3c => {
                c.eax = g.u32(at(c.ebp, 0x68))?;
                c.esi = NONE;
                if c.eax == c.esi || g.u32(c.ebp)? == c.esi || u32::from(g.u16(at(c.ebp, 6))?) == c.esi & 0xffff {
                    0x111ddf
                } else {
                    c.ecx = c.ebp;
                    return call(c, X10EE20, 0x111d64);
                }
            }
            0x111d64 => {
                if c.eax as u8 == 0 {
                    set8(c, g, 0x12, 1)?;
                }
                0x111ddf
            }
            0x111d6f => {
                c.eax = g.u32(COUNTER)?;
                c.edx = slot(c, g, 0x94)?;
                c.eax = c.eax.wrapping_add(1);
                g.set_u32(COUNTER, c.eax)?;
                c.eax ^= c.edx;
                c.esi = NONE;
                if c.eax & 0xf != 0 {
                    0x111dd8
                } else if g.u32(at(c.ebp, 0x68))? == c.esi || g.u8(at(c.ebp, 0x6c))? & 1 != 0 {
                    0x111db1
                } else {
                    c.ecx = g.u32(at(c.ebp, 0x70))?;
                    c.edx = g.u32(at(c.ebp, 0x78))?;
                    push(c, g, 1)?;
                    push(c, g, 1)?;
                    push(c, g, c.ecx)?;
                    c.ecx = g.u32(at(c.ebp, 0x74))?;
                    c.edi = c.ebp;
                    return call(c, X1CE180, 0x111daa);
                }
            }
            0x111daa => {
                c.ebx = slot(c, g, 0x1c)?;
                c.esi = NONE;
                0x111db1
            }
            0x111db1 => {
                c.ebx = c.ebx.wrapping_add(0x218);
                c.edi = 4;
                0x111dc0
            }
            0x111dc0 => {
                c.eax = g.u32(c.ebx)?;
                if c.eax == c.esi {
                    0x111dd2
                } else {
                    c.ecx = slot(c, g, 0x94)?;
                    return call(c, X1060A0, 0x111dd2);
                }
            }
            0x111dd2 => {
                c.ebx = c.ebx.wrapping_add(4);
                c.edi = c.edi.wrapping_sub(1);
                if c.edi != 0 { 0x111dc0 } else { 0x111dd8 }
            }
            0x111dd8 => {
                c.ebx = slot(c, g, 0x94)?;
                0x111ddf
            }
            0x111ddf => {
                if g.u8(at(c.ebp, 0x6c))? & 1 != 0 {
                    0x112037
                } else {
                    c.edi = slot(c, g, 0x24)?;
                    c.eax = g.u32(at(c.edi, 0xdc))?;
                    c.edi = c.edi.wrapping_add(0xdc);
                    if c.eax == c.esi || u32::from(g.u16(at(c.edi, 6))?) == c.esi & 0xffff {
                        0x111e5f
                    } else if g.u32(at(c.ebp, 0x68))? == c.esi || g.u8(at(c.ebp, 0x6c))? & 1 != 0 {
                        0x111e1c
                    } else {
                        push(c, g, c.ebx)?;
                        let k = imm(g, 0x111e0f + 1)?;
                        push(c, g, k)?;
                        push(c, g, c.ebp)?;
                        c.eax = c.edi;
                        return call(c, X1C68C0, 0x111e1c);
                    }
                }
            }
            0x111e1c => {
                if g.u8(at(c.edi, 0x11))? & 0xa == 0 {
                    0x111e44
                } else {
                    c.edx = slot(c, g, 0x24)?;
                    let al = g.u8(c.edx)? >> 7;
                    c.eax = byte(c.eax, al);
                    if al & 1 == 0 {
                        0x111e44
                    } else {
                        c.eax = c.edi;
                        return call(c, X1C63F0, 0x111e36);
                    }
                }
            }
            0x111e36 => {
                c.eax = slot(c, g, 0x24)?;
                rmw8(g, c.eax, |v| v & 0x7f)?;
                c.eax = c.ebx;
                return call(c, X114330, 0x111e44);
            }
            0x111e44 => {
                if plays(g, c.edi, c.esi)? {
                    set8(c, g, 0x12, 1)?;
                }
                0x111e5f
            }
            0x111e5f => {
                if g.u8(at(c.ebp, 0x6c))? & 1 != 0 {
                    0x112037
                } else {
                    c.edi = slot(c, g, 0x24)?;
                    c.eax = g.u32(at(c.edi, 0x9c))?;
                    c.edi = c.edi.wrapping_add(0x9c);
                    if c.eax == c.esi || u32::from(g.u16(at(c.edi, 6))?) == c.esi & 0xffff {
                        0x111f5f
                    } else if g.u32(at(c.ebp, 0x68))? == c.esi || g.u8(at(c.ebp, 0x6c))? & 1 != 0 {
                        0x111ea4
                    } else {
                        push(c, g, c.ebx)?;
                        let k = imm(g, 0x111e97 + 1)?;
                        push(c, g, k)?;
                        push(c, g, c.ebp)?;
                        c.eax = c.edi;
                        return call(c, X1C68C0, 0x111ea4);
                    }
                }
            }
            0x111ea4 => {
                let ax = g.u16(at(c.edi, 0x14))?;
                c.eax = (c.eax & !0xffff) | u32::from(ax);
                c.ebx = g.u32(at(c.edi, 8))?;
                set_slot(c, g, 0x38, c.ebx)?;
                if ax == 0 {
                    0x111ecb
                } else {
                    c.ecx = u32::from(g.u16(at(c.edi, 0x16))?);
                    c.esi = slot(c, g, 0x94)?;
                    c.edx = u32::from(ax);
                    push(c, g, c.ecx)?;
                    push(c, g, c.edx)?;
                    c.eax = c.ebx;
                    return call(c, XE6A20, 0x111ecb);
                }
            }
            0x111ecb => {
                if g.u8(at(c.edi, 0x11))? & 0xe == 0 {
                    0x111f42
                } else {
                    c.ecx = u32::from(g.u16(at(c.edi, 0x12))?);
                    c.eax = c.ecx;
                    if c.eax & 0x4000 != 0 && c.eax & 2 != 0 {
                        0x111f42
                    } else {
                        // 111ee3
                        c.esi = slot(c, g, 0x94)?;
                        c.ecx >>= 14;
                        if c.ecx & 1 == 0 {
                            0x111efd
                        } else {
                            c.eax = g.u32(at(c.ebp, 0x70))?;
                            push(c, g, c.eax)?;
                            c.eax = c.esi;
                            return call(c, X11B710, 0x111efd);
                        }
                    }
                }
            }
            0x111efd => {
                push(c, g, 0)?;
                push(c, g, 0)?;
                push(c, g, c.ebx)?;
                c.ebx = c.esi;
                return call(c, XE6B00, 0x111f09);
            }
            0x111f09 => {
                c.ecx = slot(c, g, 0x38)?;
                if g.u32(at(c.edi, 8))? != c.ecx {
                    0x111f42
                } else {
                    c.eax = c.esi;
                    return call(c, X110FC0, 0x111f19);
                }
            }
            0x111f19 => {
                if c.eax as u8 != 0 {
                    0x111f42
                } else {
                    c.eax = c.edi;
                    return call(c, X1C63F0, 0x111f24);
                }
            }
            0x111f24 => {
                c.edx = slot(c, g, 0x1c)?;
                if g.u8(at(c.edx, 0xaa))? == 1 {
                    0x111f42
                } else {
                    c.esi = slot(c, g, 0x94)?;
                    let k = imm(g, 0x111f38 + 1)?;
                    push(c, g, k)?;
                    return call(c, XBA350, 0x111f42);
                }
            }
            0x111f42 => {
                if plays(g, c.edi, NONE)? {
                    set8(c, g, 0x12, 1)?;
                }
                0x111f5f
            }
            0x111f5f => {
                if g.u8(at(c.ebp, 0x6c))? & 1 != 0 {
                    0x112037
                } else {
                    c.edi = slot(c, g, 0x24)?;
                    c.ecx = g.u32(at(c.edi, 0xbc))?;
                    c.edi = c.edi.wrapping_add(0xbc);
                    c.eax = NONE;
                    if c.ecx == c.eax || g.u16(at(c.edi, 6))? == 0xffff {
                        0x112037
                    } else if g.u32(at(c.ebp, 0x68))? == c.eax || g.u8(at(c.ebp, 0x6c))? & 1 != 0 {
                        0x111fb0
                    } else {
                        c.eax = slot(c, g, 0x94)?;
                        push(c, g, c.eax)?;
                        let k = imm(g, 0x111fa3 + 1)?;
                        push(c, g, k)?;
                        push(c, g, c.ebp)?;
                        c.eax = c.edi;
                        return call(c, X1C68C0, 0x111fb0);
                    }
                }
            }
            0x111fb0 => {
                let ax = g.u16(at(c.edi, 0x14))?;
                c.eax = (c.eax & !0xffff) | u32::from(ax);
                c.ebp = g.u32(at(c.edi, 8))?;
                if ax == 0 {
                    0x111fd3
                } else {
                    c.ecx = u32::from(g.u16(at(c.edi, 0x16))?);
                    c.esi = slot(c, g, 0x94)?;
                    c.edx = u32::from(ax);
                    push(c, g, c.ecx)?;
                    push(c, g, c.edx)?;
                    c.eax = c.ebp;
                    return call(c, XE6A20, 0x111fd3);
                }
            }
            0x111fd3 => {
                if g.u8(at(c.edi, 0x11))? & 0xe == 0 {
                    0x11201a
                } else {
                    c.ebx = slot(c, g, 0x94)?;
                    push(c, g, 0)?;
                    push(c, g, 0)?;
                    push(c, g, c.ebp)?;
                    return call(c, XE6B00, 0x111fea);
                }
            }
            0x111fea => {
                if g.u32(at(c.edi, 8))? != c.ebp {
                    0x11201a
                } else {
                    c.eax = c.ebx;
                    return call(c, X110FC0, 0x111ff6);
                }
            }
            0x111ff6 => {
                if c.eax as u8 != 0 {
                    0x11201a
                } else {
                    c.eax = c.edi;
                    return call(c, X1C63F0, 0x112001);
                }
            }
            0x112001 => {
                c.eax = slot(c, g, 0x1c)?;
                if g.u8(at(c.eax, 0xaa))? == 1 {
                    0x11201a
                } else {
                    let k = imm(g, 0x11200e + 1)?;
                    push(c, g, k)?;
                    c.esi = c.ebx;
                    return call(c, XBA350, 0x11201a);
                }
            }
            0x11201a => {
                if plays(g, c.edi, NONE)? {
                    set8(c, g, 0x12, 1)?;
                }
                0x112037
            }
            0x112037 => {
                c.esi = slot(c, g, 0x94)?;
                c.eax = c.esi;
                return call(c, X10DC70, 0x112045);
            }
            0x112045 => {
                let bl = get8(c, g, 0x12)? | c.eax as u8;
                c.ebx = byte(c.ebx, bl);
                c.eax = c.esi;
                return call(c, X10EAF0, 0x112052);
            }
            0x112052 => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.eax = byte(c.eax, (c.eax | c.ebx) as u8);
                c.ebx = pop(c, g)?;
                c.esp = c.esp.wrapping_add(0x80);
                return ret(c, 8);
            }
            _ => return Err(unowned(at_pc)),
        };
    }
}
