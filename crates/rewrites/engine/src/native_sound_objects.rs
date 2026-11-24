//! Update one game sound record. Object functions, spatial queries and the
//! loop player can change records or relocate pools. Live descriptors and
//! handles cross those callbacks in the original frame.

mod descriptor;

use crate::points::{Flag, Loc, Point, Sig};
use crate::sim_objects::{Body, Cx, Next, Result, frame, sse};
use crate::sim_physics::{ARITH, arithmetic, byte, logic, status};
use runtime::{CpuAbi, Guest};
use sim_sound::objects::{self, Binding};

pub const OBJECT_SOUND_STEP: Point = Point::new("OBJECT_SOUND_STEP", 0x18a7b0,
    Sig::Fn { args: &[("sound", Loc::Stack(0))], ret: None, pop: 4 },
    "docs/engine/sim-rewrite.md").with_flags(&[Flag::Optional, Flag::Hot]);

const POOL: u32 = 0x4ed28c;
const TAGS: u32 = 0x4e3b44;
const OVERRIDE: u32 = 0x510c50;
const HEADERS: u32 = 0x4e0300;
const FADE: u32 = 0x45dc7c;
const EVALUATE: u32 = 0xbab40;
const FILL_SOURCE: u32 = 0x18cbc0;
const CHECK_POSITION: u32 = 0x18d670;
const LOOP_PLAYER: u32 = 0x21a250;
const FIND_RECORD: u32 = 0x18d1c0;
const FREE_RECORD: u32 = 0x16ba40;

// The position is in ESI. The comparison radius arrives in XMM3.
const LISTENER_IN_RANGE: Point = Point::new("SOUND_OBJECT_LISTENER_IN_RANGE", CHECK_POSITION,
    Sig::Fn { args: &[("position", Loc::Esi)], ret: Some(Loc::Al), pop: 0 },
    "docs/engine/sim-rewrite.md").with_flags(&[Flag::Optional]);
const SCENARIO_FADE: Point = Point::new("SOUND_OBJECT_SCENARIO_FADE", 0x18d6e0,
    Sig::Fn { args: &[], ret: Some(Loc::Xmm0), pop: 0 },
    "docs/engine/sim-rewrite.md").with_flags(&[Flag::Optional]);

pub const BODIES: &[Body] = &[Body {
    point: &OBJECT_SOUND_STEP,
    sites: &[0x18a82b, 0x18a83f, 0x18a90f, 0x18a976, 0x18aa0f, 0x18aa40,
        0x18aa4d, 0x18aa7b, 0x18aab3, 0x18aac0, 0x18aac7, 0x18aae7, 0x18aaf5,
        0x18ab09, 0x18ab50],
    run: step,
}, Body { point: &LISTENER_IN_RANGE, sites: &[], run: listener_in_range },
   Body { point: &SCENARIO_FADE, sites: &[], run: scenario_fade },
   Body { point: &descriptor::POINT, sites: descriptor::SITES, run: descriptor::build }];

fn at(base: u32, off: u32) -> u32 { base.wrapping_add(off) }
fn call(target: u32, site: u32) -> Result<Next> { Ok(Next::Call { target, site }) }

fn cmp8(c: &mut CpuAbi, a: u8, b: u8) {
    let v = a.wrapping_sub(b);
    let of = (a ^ b) & (a ^ v) & 0x80 != 0;
    c.eflags = (c.eflags & !ARITH) | status(v.into(), 0x80)
        | u32::from(a < b) | u32::from((a ^ b ^ v) & 0x10) | (u32::from(of) << 11);
    c.eflags_defined |= ARITH;
}

fn record_data(g: &Guest) -> Result<u32> { g.u32(at(g.u32(POOL)?, 0x44)) }

fn step(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let mut pc = pc;
    loop {
        pc = match pc {
            0x18a7b0 => {
                c.esp = c.esp.wrapping_sub(0xac);
                c.edx = g.u32(TAGS)?;
                for word in [c.ebx, c.ebp, c.esi, c.edi] { frame::push(c, g, word)?; }
                c.edi = frame::slot(c, g, 0xc0)?;
                c.esi = (c.edi & 0xffff) * 24;
                c.ebp = at(record_data(g)?, c.esi);
                let tag = g.u32(at(c.ebp, 12))?;
                c.ecx = (tag & 0xffff) << 4;
                sse::load(c, 0, g.u32(at(c.ebp, 8))?);
                c.eax = g.u32(at(c.edx, c.ecx + 8))?;
                frame::set_slot(c, g, 0x14, c.eax)?;
                frame::set_slot(c, g, 0x24, sse::lane0(c, 0))?;
                let flags = g.u16(at(c.ebp, 4))?;
                c.eax = (c.eax & !0xffff) | u32::from(flags);
                c.ebx = byte(c.ebx, ((flags >> 8) & 1) as u8);
                c.ecx = byte(c.ecx, flags as u8);
                g.set_u8(at(c.esp, 0x1b), c.ebx as u8)?;
                if flags & 1 == 0 {
                    let kind = g.u8(at(c.ebp, 3))?;
                    c.edx = g.u32(at(c.ebp, 0x14))?;
                    c.ecx = at(c.esp, 0x24);
                    cmp8(c, kind, 1);
                    c.eax = if kind == 1 { g.u32(at(c.ebp, 0x10))? } else {
                        logic(c, u32::MAX, 1 << 31);
                        u32::MAX
                    };
                    for word in [c.ecx, c.edx, c.eax] { frame::push(c, g, word)?; }
                    return call(EVALUATE, if kind == 1 { 0x18a82b } else { 0x18a83f });
                }
                c.eax = byte(c.eax, u8::from(flags & 2 == 0));
                0x18a847
            }
            0x18a82b | 0x18a83f => 0x18a847,
            0x18a847 => {
                g.set_u8(at(c.esp, 0x13), c.eax as u8)?;
                c.eax = at(record_data(g)?, c.esi);
                let kind = g.u8(at(c.eax, 3))?;
                c.ecx = byte(c.ecx, kind);
                frame::set_slot(c, g, 0x20, c.edi)?;
                if kind == 0 {
                    c.ecx = g.u8(at(c.eax, 0x14))? as i8 as i32 as u32;
                    if c.ecx != u32::MAX {
                        let binding = Binding { record: c.edi, tag: g.u32(at(c.eax, 12))?,
                            kind, key: c.ecx as i8 };
                        c.eax = binding.playback_key();
                        frame::set_slot(c, g, 0x20, c.eax)?;
                    }
                }
                c.ecx = frame::slot(c, g, 0x14)?;
                c.edx = g.u32(c.ecx)?;
                frame::set_slot(c, g, 0x1c, u32::MAX)?;
                let request_group = c.edx & 0x30 != 0;
                if request_group {
                    let kind = g.u8(at(c.ebp, 3))?;
                    let owner = if kind == 1 { g.u32(at(c.ebp, 0x10))? } else { u32::MAX };
                    let binding = Binding { record: c.edi, tag: 0, kind, key: -1 };
                    if let Some(key) = binding.group(c.edx, || owner) {
                        c.eax = key;
                        frame::set_slot(c, g, 0x1c, key)?;
                    }
                }
                c.eax = byte(c.eax, g.u8(at(c.esp, 0x13))?);
                if c.eax as u8 == 0 {
                    c.eax = byte(c.eax, g.u8(at(c.ebp, 2))?);
                    if c.eax as u8 == 3 { pc = 0x18aa26; continue; }
                    if c.ebx as u8 == 0 {
                        g.set_u8(at(c.ebp, 2), 3)?;
                        pc = 0x18aa26;
                        continue;
                    }
                }
                let kind = g.u8(at(c.ebp, 3))?;
                c.ebx = objects::playback_flags(c.edx, kind);
                c.ecx = at(c.esp, 0x34);
                c.eax = c.edi;
                logic(c, c.ebx, 1 << 31);
                g.set_u16(c.ecx, 0)?;
                return call(FILL_SOURCE, 0x18a90f);
            }
            0x18a90f => {
                c.ecx = byte(c.ecx, g.u8(at(c.esp, 0x34))?);
                c.edx = byte(c.edx, (c.ecx as u8) >> 3);
                let gain_at = if c.edx & 1 != 0 { at(c.esp, 0x6c) } else {
                    c.eax = frame::slot(c, g, 0x14)?;
                    at(c.eax, 16)
                };
                sse::load(c, 3, g.u32(gain_at)?);
                let flags = g.u16(at(c.ebp, 4))?;
                c.eax = flags.into();
                c.edx = byte(c.edx, (flags as u8) >> 7);
                frame::set_slot(c, g, 0x14, c.eax)?;
                c.edi = objects::track_flags(flags, g.u8(at(c.ebp, 3))?);
                if c.ebx & 2 != 0 {
                    c.ecx = byte(c.ecx, (c.ecx as u8) >> 1);
                    if c.ecx & 1 == 0 {
                        logic(c, 0, 0x80);
                        c.esi = at(c.esp, 0x40);
                        return call(CHECK_POSITION, 0x18a976);
                    }
                }
                0x18a987
            }
            0x18a976 => {
                if c.eax as u8 == 0 {
                    let f = at(c.ebp, 5);
                    g.set_u8(f, g.u8(f)? & !1)?;
                    0x18aa1f
                } else {
                    c.eax = frame::slot(c, g, 0x14)?;
                    0x18a987
                }
            }
            0x18a987 => {
                c.ecx = byte(c.ecx, g.u8(at(c.esp, 0x13))?);
                if c.ecx as u8 == 0 { pc = 0x18aafa; continue; }
                c.eax = g.u32(OVERRIDE)?;
                let mut force = false;
                if c.eax != 0 {
                    c.ecx = byte(c.ecx, g.u8(at(c.eax, 5))?);
                    if c.ecx as u8 != 0 && g.u8(at(c.ebp, 3))? == 1 {
                        c.ecx = g.u32(HEADERS)?;
                        c.eax = (g.u32(at(c.ebp, 16))? & 0xffff) * 3;
                        c.edx = g.u32(at(c.ecx, 0x44))?;
                        frame::set_slot(c, g, 0x14, 1)?;
                        c.ecx = g.u8(at(c.edx, c.eax * 4 + 3))?.into();
                        c.eax = 1 << (c.ecx & 31);
                        logic(c, c.eax & 2, 0x80);
                        force = c.eax & 2 != 0;
                    }
                }
                let mode = if force { true } else {
                    c.ecx = byte(c.ecx, g.u8(at(c.ebp, 2))?);
                    c.edx = 1 << (c.ecx & 31);
                    logic(c, c.edx & 3, 0x80);
                    c.edx & 3 != 0
                };
                c.eax = u32::from(mode);
                if !mode { logic(c, 0, 1 << 31); }
                g.set_u8(at(c.ebp, 2), mode.into())?;
                c.esi = frame::slot(c, g, 0x1c)?;
                c.ecx = frame::slot(c, g, 0x20)?;
                c.edx = g.u32(at(c.ebp, 12))?;
                let descriptor = at(c.esp, 0x34);
                let args = [0, c.edi, c.eax, descriptor, c.esi, c.ecx, c.ebx, c.edx];
                c.eax = descriptor;
                for word in args { frame::push(c, g, word)?; }
                return call(LOOP_PLAYER, 0x18aa0f);
            }
            0x18aa0f => {
                if c.eax as u8 != 0 {
                    let f = at(c.ebp, 4);
                    g.set_u8(f, g.u8(f)? | 2)?;
                    g.set_u8(at(c.ebp, 2), 3)?;
                    0x18aa1b
                } else if c.esi == u32::MAX { 0x18aa1b } else {
                    let kind = g.u8(at(c.ebp, 3))?;
                    cmp8(c, kind, 1);
                    c.ecx = 17;
                    c.esi = at(c.esp, 0x34);
                    c.edi = at(c.esp, 0x78);
                    let step = if c.eflags & 0x400 == 0 { 4 } else { 4u32.wrapping_neg() };
                    while c.ecx != 0 {
                        let word = g.u32(c.esi)?;
                        g.set_u32(c.edi, word)?;
                        c.esi = at(c.esi, step);
                        c.edi = at(c.edi, step);
                        c.ecx -= 1;
                    }
                    if kind == 1 {
                        c.ebx = g.u32(at(c.ebp, 16))?;
                        c.esi = at(c.esp, 0x84);
                        c.eax = c.ebx;
                        return call(0xb9dd0, 0x18aab3);
                    }
                    0x18aae7
                }
            }
            0x18aab3 => {
                c.edi = 0;
                logic(c, 0, 1 << 31);
                c.esi = at(c.esp, 0x28);
                c.eax = c.ebx;
                return call(0xb9fc0, 0x18aac0);
            }
            0x18aac0 => {
                c.eax = c.esi;
                return call(0x188dd0, 0x18aac7);
            }
            0x18aac7 => {
                frame::set_slot(c, g, 0x4c, c.eax)?;
                let kind = g.u8(at(c.ebp, 3))?;
                cmp8(c, kind, 1);
                c.eax = if kind == 1 { g.u32(at(c.ebp, 16))? } else {
                    logic(c, u32::MAX, 1 << 31);
                    u32::MAX
                };
                frame::push(c, g, 0)?;
                c.ecx = at(c.esp, 0x98);
                return call(0xba1d0, 0x18aae7);
            }
            0x18aae7 => {
                c.eax = at(c.esp, 0x78);
                frame::push(c, g, c.eax)?;
                c.eax = frame::slot(c, g, 0x20)?;
                return call(0x219c30, 0x18aaf5);
            }
            0x18aaf5 => 0x18aa1b,
            0x18aafa => {
                c.ecx = byte(c.ecx, (c.eax as u8) >> 2);
                logic(c, c.ecx & 1, 0x80);
                if c.ecx & 1 != 0 { return call(0x18d6e0, 0x18ab09); }
                sse::load(c, 0, g.u32(FADE)?);
                c.eax = byte(c.eax, (c.eax as u8) >> 3);
                if c.eax & 1 == 0 { sse::zero(c, 0); }
                0x18ab09
            }
            0x18ab09 => {
                c.eax = byte(c.eax, g.u8(at(c.esp, 0x1b))?);
                logic(c, c.eax & 0xff, 0x80);
                frame::set_slot(c, g, 0x14, sse::lane0(c, 0))?;
                if c.eax as u8 == 0 {
                    g.set_u8(at(c.ebp, 2), 3)?;
                    0x18aa1b
                } else {
                    c.ecx = frame::slot(c, g, 0x1c)?;
                    c.edx = frame::slot(c, g, 0x20)?;
                    c.eax = g.u32(at(c.ebp, 12))?;
                    let descriptor = at(c.esp, 0x34);
                    for word in [sse::lane0(c, 0), c.edi, 2, descriptor, c.ecx, c.edx, c.ebx, c.eax] {
                        frame::push(c, g, word)?;
                    }
                    return call(LOOP_PLAYER, 0x18ab50);
                }
            }
            0x18ab50 => {
                g.set_u8(at(c.ebp, 2), if c.eax as u8 != 0 { 3 } else { 2 })?;
                0x18aa1b
            }
            0x18aa1b => {
                let f = at(c.ebp, 5);
                g.set_u8(f, g.u8(f)? | 1)?;
                0x18aa1f
            }
            0x18aa1f => { c.edi = frame::slot(c, g, 0xc0)?; 0x18aa26 }
            0x18aa26 => {
                let stage = g.u8(at(c.ebp, 2))?;
                if stage == 3 {
                    c.eax = byte(c.eax, g.u8(at(c.ebp, 4))?);
                    if objects::should_release(stage, c.eax as u8) {
                        logic(c, c.eax & 1, 0x80);
                        c.ecx = g.u32(at(c.ebp, 12))?;
                        frame::push(c, g, c.ecx)?;
                        return call(FIND_RECORD, 0x18aa40);
                    }
                }
                0x18aa7b
            }
            0x18aa40 => {
                arithmetic(c, c.eax, c.edi, true);
                if c.eax == c.edi {
                    c.ebp = g.u32(at(c.ebp, 12))?;
                    frame::push(c, g, c.ebp)?;
                    return call(FIND_RECORD, 0x18aa4d);
                }
                0x18aa6e
            }
            0x18aa4d => {
                arithmetic(c, c.eax, u32::MAX, true);
                if c.eax != u32::MAX {
                    c.edx = (c.eax & 0xffff) * 3;
                    c.ecx = record_data(g)?;
                    c.eax = at(c.ecx, c.edx * 8);
                    let tag = g.u32(at(c.eax, 12))?;
                    arithmetic(c, tag, c.ebp, true);
                    if tag == c.ebp {
                        let f = at(c.eax, 4);
                        let flags = g.u8(f)? & !0x20;
                        g.set_u8(f, flags)?;
                        logic(c, flags.into(), 0x80);
                    }
                }
                0x18aa6e
            }
            0x18aa6e => {
                c.edx = g.u32(POOL)?;
                c.eax = c.edi;
                return call(FREE_RECORD, 0x18aa7b);
            }
            0x18aa7b => {
                c.edi = frame::pop(c, g)?;
                c.esi = frame::pop(c, g)?;
                c.ebp = frame::pop(c, g)?;
                c.ebx = frame::pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0xac, false);
                return Ok(Next::Ret { pop: 4 });
            }
            _ => unreachable!("unregistered object sound continuation"),
        };
    }
}

/// Find the first mapped listener inside the squared source radius. The last
/// comparison leaves its XMM registers live for the resumed source update.
fn listener_in_range(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    use sse::Ss;
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.ecx = at(g.u32(crate::points::SOUND_GLOBALS.base)?, 0xc0);
    c.eax = byte(c.eax, 0);
    c.edx = 0;
    sse::ss_reg(c, Ss::Mul, 3, 3);
    frame::push(c, g, c.ebx)?;
    loop {
        let active = g.u8(c.ecx.wrapping_sub(0x32))?;
        c.ebx = byte(c.ebx, active);
        if active != 0 {
            for axis in 0..3 {
                sse::load(c, axis, g.u32(at(c.ecx.wrapping_sub(8), axis as u32 * 4))?);
                sse::ss(c, Ss::Sub, axis, g.u32(at(c.esi, axis as u32 * 4))?);
            }
            sse::movaps(c, 4, 0);
            sse::ss_reg(c, Ss::Mul, 4, 0);
            for axis in [1, 2] {
                sse::movaps(c, 0, axis);
                sse::ss_reg(c, Ss::Mul, 0, axis);
                sse::ss_reg(c, Ss::Add, 4, 0);
            }
            sse::comiss(c, sse::lane0(c, 3), sse::lane0(c, 4));
            if !sse::below(c) {
                c.eax = byte(c.eax, 1);
                break;
            }
        }
        c.edx += 1;
        c.ecx = at(c.ecx, 0x48);
        arithmetic(c, c.edx, 4, true);
        if c.edx == 4 { break; }
    }
    c.ebx = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}

fn scenario_fade(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.eax = g.u32(0x4e034c)?;
    c.ecx = g.u32(at(c.eax, 0xc0))?;
    sse::load(c, 0, g.u32(0x45dc10)?);
    let entry = if c.ecx == 0 { 0 } else { g.u32(at(c.eax, 0xc4))? };
    c.eax = g.u32(at(entry, 0x14))?;
    arithmetic(c, c.eax, u32::MAX, true);
    if c.eax != u32::MAX {
        c.ecx = g.u32(TAGS)?;
        c.eax = (c.eax & 0xffff) << 4;
        // The last integer instruction affecting flags is SHL EAX,4.
        crate::sim_objects::alu::shl(c, c.eax >> 4, 4, 32);
        c.edx = g.u32(at(c.ecx, c.eax + 8))?;
        sse::load(c, 0, g.u32(at(c.edx, 0x54))?);
    }
    Ok(Next::Ret { pop: 0 })
}
