//! `c58f0 (unit: stack 0) -> al`; `ret 4`: the unit's state update, the
//! second call of the unit update `c6b30` (slice 6b of
//! docs/engine/sim-rewrite-objects.md; contracts in
//! docs/engine/object-update.md, "Slice 6b bodies"). Field names are ours
//! and likely (**L**).
//!
//! **Frame.** `sub esp, 28`, then `ebx`, `ebp`, `esi`, `edi` pushed (the
//! last three between loads, so the slot stores at `c5919`, `c593a` and
//! `c593e` are made from a part-pushed `esp`). With `E` the `esp` after the
//! four pushes (the `esp` at every call site): `[E+12]` the result byte,
//! `[E+13]` whether the weapon loop is on its first slot, `[E+14]` the
//! unit's offset in the object array (`(handle & ffff) * 12`), `[E+18]`
//! the weapon loop index, `[E+1c]` the float argument of `100430`,
//! `[E+20]` the flag word (a word store; the dword read back carries the
//! stale upper half), `[E+24]` the definition, then the held weapon's
//! handle, `[E+28..38]` the record `189210` takes (`edi`): none, a value,
//! a fraction, 0. The unit is at `[E+3c]`. Across the calls `ebx` holds
//! the unit (until `c5ae1`), `ebp` the body, `esi` the definition (then
//! the loop index), `edi` the first loop's index.
//!
//! **What it does.** Unless definition `+bc` bit 11 is set: the timers at
//! body `+1f5` (states `13` and `9` through `e68c0` when it runs out),
//! `+1fa` and `+1fb` count down; with body `+134` bit 1 set, `113da0` or
//! `113df0` (as body `+148` bit 27, or bit 14 when `+aa` is 0, selects)
//! answers whether `113d20` or `113d60` runs with `0.5` (read from the
//! image). Unless definition `+bc` bit 10 or body `+10a` bit 2 is set:
//! with body `+19` bit 3 states `13` and `9` are set; otherwise each held
//! weapon index (`+212`, two slots) stamps the game time into `+228`, and
//! a slot whose index differs from `+216` sets state `8` or `12`. Body
//! `+148` bit 28 sets states `0` and `a`. When `+23d` differs from `+23c`
//! and `110ab0` answers no, `cbec0` picks the new `+23c`. The idle test
//! (`c85c0`, fused) chooses `+240` (`+241`, or none); when it changes,
//! `189210` gets the held weapon's (or the globals') name for it with a
//! fraction. Then, unless definition `+bc` bit 11 is set, each held
//! weapon gets `100430` with a flag word built from body `+148` bits (the
//! first slot's from bits 16 to 20 and 30, the second's from 21 to 25 and
//! 31; both bit 29), `110ab0`'s answer, the record word's bits and `+240`,
//! and the float `+1c0` or `+1c4`. Last, `d1540`. `al` is whether a timer
//! or state changed.

use super::{NONE, ONE, byte, call, callee, imm, owned, pop, push, ret, sx8, sx16};
use guest_layout::points;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_objects::{Cx, Next, Result, unowned};
use runtime::CpuAbi;

/// `c58f0`'s return sites, in address order.
pub(in crate::sim_objects) const STATE_SITES: [u32; 18] = [
    0xc5968, 0xc5973, 0xc59dd, 0xc59ed, 0xc59f6, 0xc5a06, 0xc5a3f, 0xc5a4a, 0xc5a94, 0xc5aae, 0xc5ab9, 0xc5ad1,
    0xc5ae8, 0xc5afd, 0xc5c2b, 0xc5ca8, 0xc5e88, 0xc5ea0,
];

/// Replace the low word of `r` (a 16-bit register write).
fn w16(r: u32, v: u32) -> u32 {
    (r & 0xffff_0000) | (v & 0xffff)
}

/// `or cx, k` when `set`, else `and cx, !k`.
fn bit16(c: &mut CpuAbi, set: bool, k: u32) {
    let cx = if set { c.ecx | k } else { c.ecx & !k };
    c.ecx = w16(c.ecx, cx);
}

/// `c58f0 (unit: stack 0) -> al`.
pub(in crate::sim_objects) fn state(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::UNIT_UPDATE_STATE.base;
    owned(pc, entry, &STATE_SITES)?;
    // Every site is at `E`.
    let at_e = |c: &CpuAbi, o: u32| c.esp.wrapping_add(o);
    let mut at = pc;
    loop {
        at = match at {
            0xc58f0 => {
                c.esp = c.esp.wrapping_sub(0x28);
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(c.ecx.wrapping_add(0x44))?;
                c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                push(c, g, c.ebx)?;
                c.ebx = g.u32(c.esp.wrapping_add(0x30))?;
                c.eax = (c.ebx & 0xffff).wrapping_mul(3) << 2;
                push(c, g, c.ebp)?;
                c.ebp = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(8))?;
                g.set_u32(c.esp.wrapping_add(0xc), c.eax)?;
                c.eax = (g.u32(c.ebp)? & 0xffff) << 4;
                push(c, g, c.esi)?;
                c.esi = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(8))?;
                c.edx = g.u32(c.esi.wrapping_add(0xbc))? >> 11;
                push(c, g, c.edi)?;
                g.set_u32(c.esp.wrapping_add(0x24), c.esi)?;
                g.set_u8(c.esp.wrapping_add(0x12), 0)?;
                if c.edx & 1 != 0 { 0xc5a0b } else { 0xc5949 }
            }
            0xc5949 => {
                let al = g.u8(c.ebp.wrapping_add(0x1f5))?;
                c.eax = byte(c.eax, al);
                if (al as i8) <= 0 {
                    0xc5978
                } else {
                    let al = al.wrapping_sub(1);
                    c.eax = byte(c.eax, al);
                    g.set_u8(c.ebp.wrapping_add(0x1f5), al)?;
                    if al != 0 {
                        0xc5973
                    } else {
                        push(c, g, c.ebx)?;
                        c.ecx = 0x13;
                        return call(c, callee::STATE_SET, 0xc5968);
                    }
                }
            }
            0xc5968 => {
                push(c, g, c.ebx)?;
                c.ecx = 9;
                return call(c, callee::STATE_SET, 0xc5973);
            }
            0xc5973 => {
                g.set_u8(at_e(c, 0x12), 1)?;
                0xc5978
            }
            0xc5978 => {
                for field in [0x1fa, 0x1fb] {
                    let al = g.u8(c.ebp.wrapping_add(field))?;
                    c.eax = byte(c.eax, al);
                    if al != 0 {
                        let al = al.wrapping_sub(1);
                        c.eax = byte(c.eax, al);
                        g.set_u8(c.ebp.wrapping_add(field), al)?;
                        g.set_u8(at_e(c, 0x12), 1)?;
                    }
                }
                // c59a6
                c.eax = g.u32(c.ebp.wrapping_add(0x134))? >> 1;
                if c.eax & 1 == 0 {
                    0xc5a0b
                } else {
                    c.eax = g.u32(c.ebp.wrapping_add(0x148))?;
                    if c.eax & 0x0800_0000 != 0 {
                        0xc59ef
                    } else {
                        let cl = g.u8(c.ebp.wrapping_add(0xaa))?;
                        c.ecx = byte(c.ecx, cl);
                        c.edx = 1u32 << (cl & 31);
                        if c.edx & 1 != 0 && c.eax & 0x4000 != 0 {
                            0xc59ef
                        } else {
                            c.eax = c.ebx;
                            return call(c, callee::F113DA0, 0xc59dd);
                        }
                    }
                }
            }
            0xc59dd => {
                if c.eax as u8 != 0 {
                    0xc5a0b
                } else {
                    let half = imm(g, 0xc59e2)?;
                    push(c, g, half)?;
                    c.eax = c.ebx;
                    return call(c, callee::F113D20, 0xc59ed);
                }
            }
            0xc59ed => 0xc5a06,
            0xc59ef => {
                c.eax = c.ebx;
                return call(c, callee::F113DF0, 0xc59f6);
            }
            0xc59f6 => {
                if c.eax as u8 != 0 {
                    0xc5a0b
                } else {
                    let half = imm(g, 0xc59fb)?;
                    push(c, g, half)?;
                    c.eax = c.ebx;
                    return call(c, callee::F113D60, 0xc5a06);
                }
            }
            0xc5a06 => {
                g.set_u8(at_e(c, 0x12), 1)?;
                0xc5a0b
            }
            0xc5a0b => {
                c.eax = g.u32(c.esi.wrapping_add(0xbc))? >> 10;
                if c.eax & 1 != 0 {
                    0xc5c2f
                } else {
                    let cl = g.u8(c.ebp.wrapping_add(0x10a))? >> 2;
                    c.ecx = byte(c.ecx, cl);
                    if cl & 1 != 0 {
                        0xc5c2f
                    } else if g.u8(c.ebp.wrapping_add(0x19))? & 8 != 0 {
                        push(c, g, c.ebx)?;
                        c.ecx = 0x13;
                        return call(c, callee::STATE_SET, 0xc5a3f);
                    } else {
                        c.edi = 0;
                        0xc5a50
                    }
                }
            }
            0xc5a3f => {
                push(c, g, c.ebx)?;
                c.ecx = 9;
                return call(c, callee::STATE_SET, 0xc5a4a);
            }
            0xc5a4a => 0xc5a9a,
            0xc5a50 => {
                let slot = c.edi.wrapping_add(c.ebp);
                c.eax = sx8(g.u8(slot.wrapping_add(0x212))?);
                c.ecx = sx8(g.u8(slot.wrapping_add(0x216))?);
                if c.eax != NONE {
                    c.edx = g.u32(points::GAME_TIME.base)?;
                    c.edx = g.u32(c.edx.wrapping_add(8))?;
                    g.set_u32(c.ebp.wrapping_add(c.eax.wrapping_mul(4)).wrapping_add(0x228), c.edx)?;
                }
                // c5a75
                if c.ecx == c.eax {
                    0xc5a94
                } else if c.edi == 0 || c.edi == 1 {
                    c.ecx = if c.edi == 0 { 8 } else { 0x12 };
                    push(c, g, c.ebx)?;
                    return call(c, callee::STATE_SET, 0xc5a94);
                } else {
                    0xc5a94
                }
            }
            0xc5a94 => {
                c.edi = c.edi.wrapping_add(1);
                if (c.edi as i32) < 2 { 0xc5a50 } else { 0xc5a9a }
            }
            0xc5a9a => {
                if g.u32(c.ebp.wrapping_add(0x148))? & 0x1000_0000 == 0 {
                    0xc5ab9
                } else {
                    push(c, g, c.ebx)?;
                    c.ecx = 0;
                    return call(c, callee::STATE_SET, 0xc5aae);
                }
            }
            0xc5aae => {
                push(c, g, c.ebx)?;
                c.ecx = 0xa;
                return call(c, callee::STATE_SET, 0xc5ab9);
            }
            0xc5ab9 => {
                let al = g.u8(c.ebp.wrapping_add(0x23d))?;
                c.eax = byte(c.eax, al);
                if al == g.u8(c.ebp.wrapping_add(0x23c))? {
                    0xc5af4
                } else {
                    c.ecx = g.u32(at_e(c, 0x3c))?;
                    push(c, g, c.ecx)?;
                    return call(c, callee::F110AB0, 0xc5ad1);
                }
            }
            0xc5ad1 => {
                if c.eax as u8 != 0 {
                    0xc5af4
                } else {
                    c.ecx = w16(c.ecx, sx8(g.u8(c.ebp.wrapping_add(0x23d))?));
                    c.eax = g.u32(at_e(c, 0x3c))?;
                    c.ebx = 0;
                    return call(c, callee::CBEC0, 0xc5ae8);
                }
            }
            0xc5ae8 => {
                if c.eax & 0xffff != 0xffff {
                    g.set_u8(c.ebp.wrapping_add(0x23c), c.eax as u8)?;
                }
                0xc5af4
            }
            0xc5af4 => {
                c.eax = g.u32(at_e(c, 0x3c))?;
                return call(c, points::UNIT_IDLE_TEST.base, 0xc5afd);
            }
            0xc5afd => {
                if c.eax as u8 != 0 {
                    c.ebx = w16(c.ebx, sx8(g.u8(c.ebp.wrapping_add(0x241))?));
                } else {
                    c.ebx = NONE;
                }
                // c5b0e
                c.edx = w16(c.edx, sx8(g.u8(c.ebp.wrapping_add(0x240))?));
                if c.edx & 0xffff == c.ebx & 0xffff {
                    0xc5c2f
                } else {
                    let bl = c.ebx as u8;
                    g.set_u8(c.ebp.wrapping_add(0x240), bl)?;
                    if bl == 0xff {
                        sse::zero(c, 0);
                        g.set_u32(c.ebp.wrapping_add(0x26c), sse::lane0(c, 0))?;
                    }
                    // c5b35
                    c.edi = g.u32(points::OBJECTS.base)?;
                    c.eax = g.u32(c.edi.wrapping_add(0x44))?;
                    c.ecx = g.u32(at_e(c, 0x14))?;
                    c.ecx = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(8))?;
                    c.edx = w16(c.edx, sx8(g.u8(c.ecx.wrapping_add(0x212))?));
                    c.eax = NONE;
                    if c.edx & 0xffff != 0xffff {
                        c.edx = sx16(c.edx as u16);
                        c.eax = g.u32(c.ecx.wrapping_add(c.edx.wrapping_mul(4)).wrapping_add(0x218))?;
                    }
                    // c5b61
                    sse::load(c, 0, g.u32(ONE)?);
                    if c.eax == NONE { 0xc5bc0 } else { 0xc5b6e }
                }
            }
            0xc5b6e => {
                c.ecx = g.u32(c.edi.wrapping_add(0x44))?;
                c.eax = (c.eax & 0xffff).wrapping_mul(3);
                c.edx = g.u32(c.ecx.wrapping_add(c.eax.wrapping_mul(4)).wrapping_add(8))?;
                c.eax = g.u32(c.edx)?;
                c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                c.eax = (c.eax & 0xffff) << 4;
                c.eax = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(8))?;
                if (g.u16(c.eax.wrapping_add(0x1fe))? as i16) <= 0 {
                    0xc5bc0
                } else if c.ebx as u8 == 0xff {
                    c.ecx = g.u32(c.eax.wrapping_add(0x278))?;
                    0xc5bfa
                } else {
                    c.ecx = g.u32(c.eax.wrapping_add(0x270))?;
                    let ax = g.u16(c.eax.wrapping_add(0x1fe))?;
                    c.eax = w16(c.eax, u32::from(ax));
                    if (ax as i16) <= 1 {
                        0xc5bfa
                    } else {
                        c.eax = sx16(ax);
                        0xc5bea
                    }
                }
            }
            0xc5bc0 => {
                c.ecx = g.u32(points::GLOBALS_TAG.base)?;
                c.eax = g.u32(c.ecx.wrapping_add(0x134))?;
                if c.ebx as u8 == 0xff {
                    c.ecx = g.u32(c.eax.wrapping_add(0xd0))?;
                    0xc5bfa
                } else {
                    c.ecx = g.u32(c.eax.wrapping_add(0xc8))?;
                    c.eax = g.u32(c.eax.wrapping_add(0xb8))?;
                    if (c.eax as i32) <= 1 { 0xc5bfa } else { 0xc5bea }
                }
            }
            0xc5bea => {
                c.edx = sx8(c.ebx as u8);
                c.eax = c.eax.wrapping_sub(1);
                sse::cvtsi2ss(c, 0, c.edx);
                sse::cvtsi2ss(c, 1, c.eax);
                sse::ss_reg(c, Ss::Div, 0, 1);
                0xc5bfa
            }
            0xc5bfa => {
                if c.ecx == NONE {
                    0xc5c2f
                } else {
                    c.esi = g.u32(at_e(c, 0x3c))?;
                    c.eax = imm(g, 0xc5c04)?;
                    c.edi = at_e(c, 0x28);
                    g.set_u32(at_e(c, 0x28), NONE)?;
                    g.set_u32(at_e(c, 0x2c), c.ecx)?;
                    g.set_u32(at_e(c, 0x30), sse::lane0(c, 0))?;
                    g.set_u32(at_e(c, 0x34), 0)?;
                    return call(c, callee::F189210, 0xc5c2b);
                }
            }
            0xc5c2b => {
                c.esi = g.u32(at_e(c, 0x24))?;
                0xc5c2f
            }
            0xc5c2f => {
                c.eax = g.u32(c.esi.wrapping_add(0xbc))? >> 11;
                if c.eax & 1 != 0 {
                    0xc5ea0
                } else {
                    c.esi = 0;
                    g.set_u32(at_e(c, 0x18), 0)?;
                    0xc5c50
                }
            }
            0xc5c50 => {
                let al = g.u8(c.esi.wrapping_add(c.ebp).wrapping_add(0x212))?;
                c.eax = byte(c.eax, al);
                let bl = u8::from(c.esi == 0);
                c.ebx = byte(c.ebx, bl);
                g.set_u8(at_e(c, 0x13), bl)?;
                if al == 0xff {
                    0xc5e88
                } else {
                    c.edi = g.u32(points::OBJECTS.base)?;
                    c.ecx = g.u32(c.edi.wrapping_add(0x44))?;
                    c.edx = g.u32(at_e(c, 0x14))?;
                    c.eax = g.u32(c.ecx.wrapping_add(c.edx).wrapping_add(8))?;
                    c.ecx = w16(c.ecx, sx8(g.u8(c.eax.wrapping_add(c.esi).wrapping_add(0x212))?));
                    g.set_u32(at_e(c, 0x24), NONE)?;
                    if c.ecx & 0xffff != 0xffff {
                        c.ecx = sx16(c.ecx as u16);
                        c.edx = g.u32(c.eax.wrapping_add(c.ecx.wrapping_mul(4)).wrapping_add(0x218))?;
                        g.set_u32(at_e(c, 0x24), c.edx)?;
                    }
                    // c5c9e
                    c.eax = g.u32(at_e(c, 0x3c))?;
                    push(c, g, c.eax)?;
                    return call(c, callee::F110AB0, 0xc5ca8);
                }
            }
            0xc5ca8 => {
                c.ecx = g.u32(c.ebp.wrapping_add(0x1ec))?;
                if (c.ecx as i32) > 0 {
                    c.eax = byte(c.eax, 0);
                }
                // c5cb4
                c.ecx = g.u32(c.edi.wrapping_add(0x44))?;
                c.edx = g.u32(at_e(c, 0x14))?;
                c.edx = g.u32(c.ecx.wrapping_add(c.edx).wrapping_add(8))?;
                let mut set = false;
                for shift in [0x16, 0x1a, 0x1b] {
                    c.ecx = sx16(g.u16(c.edx.wrapping_add(0x346))?);
                    c.ecx = g.u32(c.ecx.wrapping_add(c.edx).wrapping_add(4))? >> shift;
                    if c.ecx & 1 != 0 {
                        set = true;
                        break;
                    }
                }
                if !set {
                    // c5cf8
                    c.edi = sx16(g.u16(c.edx.wrapping_add(0x346))?);
                    // mov cl, bl; neg cl; sbb ecx, ecx; and ecx, -10; add ecx, 12h
                    c.ecx = if c.ebx as u8 != 0 { 8 } else { 0x12 };
                    c.edi = c.edi.wrapping_add(c.edx);
                    c.esi = c.ecx;
                    c.ecx &= 0x1f;
                    c.edx = 1;
                    c.esi = ((c.esi as i32) >> 5) as u32;
                    c.edx <<= c.ecx & 31;
                    c.ecx = g.u32(c.edi.wrapping_add(c.esi.wrapping_mul(4)).wrapping_add(4))?;
                    c.esi = g.u32(at_e(c, 0x18))?;
                    set = c.edx & c.ecx != 0;
                }
                if set {
                    c.eax = byte(c.eax, 1);
                }
                // c5d2a: the flag word.
                c.edx = g.u32(c.ebp.wrapping_add(0x148))?;
                let first = c.ebx as u8 != 0;
                c.ecx = c.edx;
                c.ebx = c.edx;
                let base = if first { 0x10 } else { 0x15 };
                c.ecx >>= base;
                c.ecx = byte(c.ecx, (c.ecx & 1) as u8);
                c.ecx = w16(c.ecx, c.ecx & 0xff);
                c.ebx >>= base + 1;
                c.ecx = w16(c.ecx, c.ecx << 1);
                bit16(c, c.ebx & 1 != 0, 4);
                c.ebx = c.edx >> (base + 2);
                bit16(c, c.ebx & 1 != 0, 8);
                c.ebx = c.edx >> (base + 3);
                bit16(c, c.ebx & 1 != 0, 0x10);
                c.ebx = c.edx >> (base + 4);
                bit16(c, c.ebx & 1 != 0, 1);
                c.ebx = c.edx >> if first { 0x1e } else { 0x1f };
                // c5e07
                bit16(c, c.ebx & 1 != 0, 0x80);
                c.edx >>= 0x1d;
                bit16(c, c.edx & 1 != 0, 0x100);
                bit16(c, c.eax as u8 != 0, 0x20);
                bit16(c, g.u8(c.ebp.wrapping_add(0x240))? != 0xff, 0x40);
                // c5e4f
                let al = g.u8(at_e(c, 0x13))?;
                c.eax = byte(c.eax, al);
                g.set_u16(at_e(c, 0x20), c.ecx as u16)?;
                let from = if al != 0 { 0x1c0 } else { 0x1c4 };
                sse::load(c, 0, g.u32(c.ebp.wrapping_add(from))?);
                c.eax = g.u32(at_e(c, 0x20))?;
                c.ecx = g.u32(at_e(c, 0x24))?;
                g.set_u32(at_e(c, 0x1c), sse::lane0(c, 0))?;
                c.edx = g.u32(at_e(c, 0x1c))?;
                push(c, g, c.edx)?;
                push(c, g, c.eax)?;
                push(c, g, c.ecx)?;
                return call(c, callee::F100430, 0xc5e88);
            }
            0xc5e88 => {
                c.esi = c.esi.wrapping_add(1);
                g.set_u32(at_e(c, 0x18), c.esi)?;
                if (c.esi as i32) < 2 {
                    0xc5c50
                } else {
                    c.edx = g.u32(at_e(c, 0x3c))?;
                    push(c, g, c.edx)?;
                    return call(c, callee::D1540, 0xc5ea0);
                }
            }
            0xc5ea0 => {
                let al = g.u8(at_e(c, 0x12))?;
                c.eax = byte(c.eax, al);
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = c.esp.wrapping_add(0x28);
                return ret(c, 4);
            }
            _ => return Err(unowned(at)),
        };
    }
}
