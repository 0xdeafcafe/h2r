//! The object loop player's spatial descriptor. The source kind determines
//! where the position comes from: effect, particle, object marker or a
//! location already stored in the game sound record.

use super::*;

pub(super) const POINT: Point = Point::new("SOUND_OBJECT_DESCRIPTOR", FILL_SOURCE,
    Sig::Fn { args: &[("sound", Loc::Eax), ("output", Loc::Ecx)], ret: None, pop: 0 },
    "docs/engine/sim-rewrite.md").with_flags(&[Flag::Optional]);

pub(super) const SITES: &[u32] = &[0x18cc5a, 0x18cc6f, 0x18ccaa, 0x18cced, 0x18ccf7,
    0x18cd41, 0x18ce5b, 0x18ce7f, 0x18ceb2, 0x18cec5, 0x18ceef, 0x18cf01, 0x18cf2a,
    0x18cf48, 0x18cf5d, 0x18cf7b];

const DEFAULT_LOCATION: u32 = 0x468788;
const DEFAULT_ORIENTATION: u32 = 0x4687b0;
const DEFAULT_OWNER: u32 = 0x444ae0;
const EFFECT_POOL: u32 = 0x4ea93c;
const PARTICLE_POOL: u32 = 0x51ebfc;

fn read_owner(c: &mut CpuAbi, g: &Guest, kind: u8) -> Result<u32> {
    let current_kind = g.u8(at(c.esi, 3))?;
    cmp8(c, current_kind, kind);
    if current_kind == kind { return g.u32(at(c.esi, 16)); }
    logic(c, u32::MAX, 1 << 31);
    Ok(u32::MAX)
}

fn set_flag(c: &mut CpuAbi, g: &mut Guest, addr: u32, mask: u8) -> Result<()> {
    let value = g.u8(addr)? | mask;
    g.set_u8(addr, value)?;
    logic(c, value.into(), 0x80);
    Ok(())
}

pub(super) fn build(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let mut pc = pc;
    loop {
        pc = match pc {
            FILL_SOURCE => {
                c.esp = c.esp.wrapping_sub(0x80);
                for word in [c.ebx, c.ebp, c.esi, c.edi] { frame::push(c, g, word)?; }
                c.edi = c.ecx;
                c.edx = record_data(g)?;
                c.esi = at(c.edx, (c.eax & 0xffff) * 24);
                c.ecx = g.u32(TAGS)?;
                c.eax = (g.u32(at(c.esi, 12))? & 0xffff) << 4;
                c.ebp = g.u32(at(c.ecx, c.eax + 8))?;
                g.set_u16(c.edi, 0)?;
                let kind = g.u8(at(c.esi, 3))?;
                c.eax = g.u32(if kind == 0 { at(c.esi, 16) } else { DEFAULT_OWNER })?;
                g.set_u32(at(c.edi, 8), c.eax)?;
                g.set_u8(at(c.edi, 2), 0)?;
                c.ecx = byte(c.ecx, g.u8(at(c.esi, 3))?);
                c.eax = c.ecx as i8 as i32 as u32;
                match c.eax {
                    0 => {
                        c.eax = g.u16(at(c.esi, 0x16))? as i16 as i32 as u32;
                        if c.eax != u32::MAX {
                            g.set_u16(at(c.edi, 0x2c), c.eax as u16)?;
                            g.set_u32(at(c.edi, 0x28), u32::MAX)?;
                            let kind = g.u8(at(c.esi, 3))?;
                            c.eax = if kind == 0 {
                                g.u8(at(c.esi, 0x15))? as i8 as i32 as u32
                            } else { u32::MAX };
                            set_flag(c, g, at(c.edi, 1), 0x10)?;
                            g.set_u16(at(c.edi, 0x2e), c.eax as u16)?;
                        }
                        0x18cdab
                    }
                    2 => {
                        c.eax = byte(0, (g.u8(at(c.edi, 3))? & !14) | 1);
                        g.set_u8(at(c.edi, 3), c.eax as u8)?;
                        c.ecx = byte(c.ecx, g.u8(at(c.esi, 4))? >> 1);
                        if c.ecx & 1 != 0 {
                            set_flag(c, g, c.edi, 2)?;
                            sse::zero(c, 0);
                            g.set_u32(at(c.edi, 4), 0)?;
                            c.edx = 0;
                            logic(c, 0, 1 << 31);
                            c.eax = at(c.edi, 12);
                            for offset in (0..36).step_by(4) { g.set_u32(at(c.eax, offset), 0)?; }
                            0x18ccdf
                        } else {
                            c.esi = g.u32(at(c.esi, 16))? & 0xffff;
                            c.ecx = g.u32(PARTICLE_POOL)?;
                            c.edx = at(c.edi, 4);
                            frame::push(c, g, c.edx)?;
                            c.edx = g.u32(at(c.ecx, 0x44))?;
                            c.edi = at(c.edi, 12);
                            frame::push(c, g, c.edi)?;
                            c.eax = arithmetic(c, c.esi * 160, c.edx, false);
                            frame::push(c, g, c.eax)?;
                            return call(0x22a110, 0x18cd41);
                        }
                    }
                    3 => {
                        c.edx = byte(c.edx, (g.u8(at(c.edi, 3))? & !14) | 1);
                        g.set_u8(at(c.edi, 3), c.edx as u8)?;
                        c.eax = u32::from(g.u8(at(c.esi, 4))? >> 1);
                        logic(c, c.eax & 1, 0x80);
                        if c.eax & 1 != 0 { 0x18ccb5 } else {
                            c.eax = g.u32(at(c.esi, 16))?;
                            c.ecx = at(c.edi, 4);
                            frame::push(c, g, c.ecx)?;
                            c.edx = at(c.esp, 0x18);
                            c.ebx = at(c.edi, 12);
                            frame::push(c, g, c.edx)?;
                            return call(0x17b2d0, 0x18cc5a);
                        }
                    }
                    _ => { c.eax = c.eax.wrapping_sub(3); 0x18cdab }
                }
            }
            0x18cc5a => {
                let missing = u8::from(c.eax as u8 == 0);
                c.eax = byte(c.eax, missing);
                // NEG; SBB AL,AL; INC AL. INC retains the subtraction's
                // carry, and sets AF when it turns -1 into zero.
                let prior = if missing == 0 { u8::MAX } else { 0 };
                c.eflags = (c.eflags & !ARITH) | status(missing.into(), 0x80)
                    | u32::from(prior == u8::MAX) * 0x11;
                c.eflags_defined |= ARITH;
                g.set_u8(at(c.esp, 0x13), missing)?;
                if missing != 0 { 0x18ccb5 } else {
                    c.eax = at(c.esp, 0x14);
                    return call(0x188dd0, 0x18cc6f);
                }
            }
            0x18cc6f => {
                c.ecx = g.u32(EFFECT_POOL)?;
                g.set_u32(at(c.edi, 0x18), c.eax)?;
                c.eax = g.u32(at(c.esi, 16))? & 0xffff;
                c.edx = g.u32(at(c.ecx, 0x44))?;
                c.eax = at(c.edx, (c.eax * 400) + 0x30);
                // IMUL EAX,400 has no signed overflow for a 16-bit index.
                c.eflags &= !ARITH;
                c.eflags_defined = (c.eflags_defined & !ARITH) | 0x801;
                c.ecx = at(c.edi, 0x1c);
                for i in [0, 4] {
                    c.edx = g.u32(at(c.eax, i))?;
                    g.set_u32(at(c.ecx, i), c.edx)?;
                }
                c.eax = g.u32(at(c.eax, 8))?;
                g.set_u32(at(c.ecx, 8), c.eax)?;
                c.esi = at(c.edi, 0x28);
                c.edx = c.ebx;
                return call(0x11bed0, 0x18ccaa);
            }
            0x18ccb5 => {
                set_flag(c, g, c.edi, 2)?;
                sse::zero(c, 0);
                g.set_u32(at(c.edi, 4), 0)?;
                c.ecx = 0;
                logic(c, 0, 1 << 31);
                c.edx = at(c.edi, 12);
                for offset in (0..36).step_by(4) { g.set_u32(at(c.edx, offset), 0)?; }
                0x18ccdf
            }
            0x18ccdf => {
                c.edx = g.u32(DEFAULT_LOCATION)?;
                c.esi = at(c.edi, 0x28);
                return call(0x11bed0, 0x18cced);
            }
            0x18cced => {
                c.eax = g.u32(DEFAULT_ORIENTATION)?;
                return call(0x188dd0, 0x18ccf7);
            }
            0x18ccf7 => { g.set_u32(at(c.edi, 0x18), c.eax)?; 0x18cfba }
            0x18cdab => {
                if g.u8(at(c.esi, 4))? & 1 != 0 {
                    c.eax = g.u32(at(c.esi, 8))?;
                    g.set_u32(at(c.edi, 4), c.eax)?;
                    0x18cecd
                } else {
                    c.eax = read_owner(c, g, 1)?;
                    c.ecx = g.u32(HEADERS)?;
                    c.edx = g.u32(at(c.ecx, 0x44))?;
                    c.eax = (c.eax & 0xffff) * 3;
                    c.ecx = byte(c.ecx, g.u8(at(c.edx, c.eax * 4 + 3))?);
                    c.ebx = 1 << (c.ecx & 31);
                    if c.ebx & (1 << 10) == 0 { 0x18ce9b } else {
                        c.eax = g.u32(at(c.esi, 16))? & 0xffff;
                        sse::zero(c, 0);
                        c.ecx = c.eax * 3;
                        c.ebx = g.u32(at(c.edx, c.ecx * 4 + 8))?;
                        c.edx = byte(c.edx, g.u8(at(c.ebx, 0x12c))?);
                        g.set_u8(at(c.edi, 2), c.edx as u8)?;
                        c.eax = g.u32(at(c.ebx, 0x130))?;
                        g.set_u32(at(c.edi, 0x30), c.eax)?;
                        for (src, dst, flag, use_ecx) in [(0x134, 0x34, 4, true), (0x138, 0x38, 8, false)] {
                            sse::load(c, 1, g.u32(at(c.ebx, src))?);
                            sse::comiss(c, sse::lane0(c, 1), 0);
                            if !sse::below_or_equal(c) {
                                set_flag(c, g, c.edi, flag)?;
                                let v = g.u32(at(c.ebx, src))?;
                                if use_ecx { c.ecx = v; } else { c.edx = v; }
                                g.set_u32(at(c.edi, dst), v)?;
                            }
                        }
                        sse::load(c, 1, g.u32(at(c.ebx, 0x13c))?);
                        sse::comiss(c, sse::lane0(c, 1), 0);
                        if !sse::below_or_equal(c) {
                            set_flag(c, g, c.edi, 16)?;
                            sse::load(c, 1, g.u32(at(c.ebx, 0x13c))?);
                            return call(0x188790, 0x18ce5b);
                        }
                        0x18ce62
                    }
                }
            }
            0x18ce5b => {
                sse::zero(c, 0);
                g.set_u16(at(c.edi, 0x3c), c.eax as u16)?;
                0x18ce62
            }
            0x18ce62 => {
                sse::load(c, 1, g.u32(at(c.ebx, 0x140))?);
                sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0));
                if !sse::below_or_equal(c) {
                    set_flag(c, g, c.edi, 32)?;
                    sse::load(c, 1, g.u32(at(c.ebx, 0x140))?);
                    return call(0x188790, 0x18ce7f);
                }
                0x18ce86
            }
            0x18ce7f => {
                sse::zero(c, 0);
                g.set_u16(at(c.edi, 0x3e), c.eax as u16)?;
                0x18ce86
            }
            0x18ce86 => {
                sse::comiss(c, sse::lane0(c, 0), g.u32(at(c.ebx, 0x144))?);
                if !sse::below_or_equal(c) {
                    set_flag(c, g, c.edi, 64)?;
                    c.eax = g.u32(at(c.ebx, 0x144))?;
                    g.set_u32(at(c.edi, 0x40), c.eax)?;
                }
                0x18ce9b
            }
            0x18ce9b => {
                c.eax = read_owner(c, g, 1)?;
                let attached = g.u8(at(c.esi, 3))? == 1;
                c.edx = g.u32(at(c.esi, 20))?;
                c.ecx = at(c.edi, 4);
                for word in [c.ecx, c.edx, c.eax] { frame::push(c, g, word)?; }
                return call(EVALUATE, if attached { 0x18ceb2 } else { 0x18cec5 });
            }
            0x18ceb2 | 0x18cec5 => 0x18cecd,
            0x18cecd => {
                if g.u8(at(c.esi, 3))? != 1 {
                    let mode = g.u8(at(c.edi, 3))? & !15;
                    c.eax = byte(c.eax, mode);
                    g.set_u8(at(c.edi, 3), mode)?;
                    0x18cfac
                } else {
                    c.ecx = g.u16(at(c.esi, 6))?.into();
                    c.eax = 0;
                    crate::sim_objects::alu::cmp16(c, c.ecx, 0xffff);
                    if c.ecx != 0xffff {
                        c.eax = c.ecx;
                        c.ecx = g.u32(at(c.esi, 16))?;
                        return call(0xb8c40, 0x18ceef);
                    }
                    0x18ceef
                }
            }
            0x18ceef => {
                c.ecx = g.u32(at(c.esi, 16))?;
                for word in [1, c.eax, c.ecx] { frame::push(c, g, word)?; }
                c.ecx = byte(c.ecx, 0);
                logic(c, 0, 0x80);
                c.ebx = at(c.esp, 0x2c);
                return call(0xb8d30, 0x18cf01);
            }
            0x18cf01 => {
                c.edx = frame::slot(c, g, 0x80)?;
                c.eax = frame::slot(c, g, 0x84)?;
                c.ecx = at(c.edi, 12);
                g.set_u32(c.ecx, c.edx)?;
                c.edx = frame::slot(c, g, 0x88)?;
                g.set_u32(at(c.ecx, 4), c.eax)?;
                c.eax = at(c.esp, 0x5c);
                g.set_u32(at(c.ecx, 8), c.edx)?;
                return call(0x188dd0, 0x18cf2a);
            }
            0x18cf2a => {
                g.set_u32(at(c.edi, 0x18), c.eax)?;
                c.ebx = byte(c.ebx, 1);
                c.eax = read_owner(c, g, 1)?;
                frame::push(c, g, 0)?;
                c.ecx = at(c.edi, 0x1c);
                return call(0xba1d0, 0x18cf48);
            }
            0x18cf48 => {
                c.ecx = read_owner(c, g, c.ebx as u8)?;
                c.eax = at(c.edi, 0x28);
                return call(0xba300, 0x18cf5d);
            }
            0x18cf5d => {
                c.eax = read_owner(c, g, c.ebx as u8)?;
                frame::push(c, g, 0)?;
                c.ecx = at(c.esp, 0x17);
                frame::push(c, g, c.ecx)?;
                frame::push(c, g, c.eax)?;
                g.set_u8(at(c.esp, 0x1f), c.ebx as u8)?;
                return call(crate::points::SOUND_OWNER_USER.base, 0x18cf7b);
            }
            0x18cf7b => {
                c.edx = byte(c.edx, g.u8(at(c.edi, 3))?);
                c.ecx = byte(c.ecx, g.u8(at(c.esp, 0x13))?);
                let diff = (c.edx as u8 ^ c.ecx as u8) & 15;
                c.edx = byte(c.edx, diff);
                let mode = g.u8(at(c.edi, 3))? ^ diff;
                c.ebx = byte(c.ebx, mode);
                g.set_u8(at(c.edi, 3), mode)?;
                c.eax = byte(c.eax, mode);
                if g.u8(c.ebp)? & 8 != 0 && mode & 15 == 0 {
                    c.eax = byte(c.eax, (mode & !14) | 1);
                    set_flag(c, g, c.edi, 1)?;
                    g.set_u8(at(c.edi, 3), c.eax as u8)?;
                }
                0x18cfac
            }
            0x18cfac => {
                c.eax = byte(c.eax, g.u8(at(c.esi, 4))? >> 5);
                if c.eax & 1 != 0 { set_flag(c, g, at(c.edi, 1), 4)?; }
                0x18cfba
            }
            0x18ccaa | 0x18cd41 | 0x18cfba => {
                c.edi = frame::pop(c, g)?;
                c.esi = frame::pop(c, g)?;
                c.ebp = frame::pop(c, g)?;
                c.ebx = frame::pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x80, false);
                return Ok(Next::Ret { pop: 0 });
            }
            _ => unreachable!("unregistered object sound descriptor continuation"),
        }
    }
}
