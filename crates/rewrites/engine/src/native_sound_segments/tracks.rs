//! The loop's tracks are visited in tag order. A callback may change the
//! tag or runtime record before the next visit.
use super::*;
use crate::sound::{LoopingTrackTag, SoundInstance};
use sse::Ss;

pub(super) fn run(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    loop {
        if pc == 0x21a708 || playing::SITES.contains(&pc) {
            match playing::run(cx, pc)? {
                TrackStep::Transfer(next) => return Ok(next),
                TrackStep::Done => pc = 0x21ab29,
                TrackStep::Stopping => pc = 0x21aa02,
            }
        }
        if pc == 0x21aa02 || stopping::SITES.contains(&pc) {
            match stopping::run(cx, pc)? {
                TrackStep::Transfer(next) => return Ok(next),
                TrackStep::Done => pc = 0x21ab29,
                TrackStep::Stopping => unreachable!(),
            }
        }
        let (c, g) = (&mut *cx.cpu, &mut *cx.g);
        match pc {
            0x21a515 => {
                c.eax = g.u32(points::SOUND_GLOBALS.base)?;
                c.ebx = frame::slot(c, g, 0x64)?;
                c.ecx = byte(c.ecx, g.u8(at(c.eax, 0x7f))?);
                g.set_u8(at(c.ebp, 0x53), c.ecx as u8)?;
                if RequestMode(c.ebx).stopping() || g.u8(at(c.ebp, 0x50))? & 8 != 0 {
                    let voices = g.u8(at(c.ebp, 0x51))?;
                    c.eax = byte(c.eax, voices);
                    logic(c, voices.into(), 0x80);
                    if voices == 0 {
                        c.edi = frame::slot(c, g, 0x14)?;
                        return Ok(Next::Call { target: 0x21b070, site: 0x21a53f });
                    }
                }
                c.edx = frame::slot(c, g, 0x18)?;
                c.eax = g.u32(at(c.edx, 0x1c))?;
                g.set_u8(at(c.esp, 0x11), 0)?;
                frame::set_slot(c, g, 0x50, 0)?;
                pc = if c.eax as i32 > 0 { 0x21a570 } else { 0x21ab45 };
            }
            0x21a53f => return finish_request(c, g),
            0x21a570 => {
                let index = frame::slot(c, g, 0x50)?;
                c.eax = frame::slot(c, g, 0x18)?;
                c.edi = g.u32(at(c.eax, 0x20))?;
                c.edx = at(c.edi, index.wrapping_mul(LoopingTrackTag::SIZE));
                c.ecx = frame::slot(c, g, 0x20)?;
                c.ebp = at(c.ecx, at(0x88, index.wrapping_mul(0x14)));
                c.esi = at(g.u32(points::RANDOM_SEEDS.base)?, 4);
                frame::set_slot(c, g, 0x54, c.edx)?;
                frame::set_slot(c, g, 0x5c, c.esi)?;
                if g.u8(c.eax)? & 0x30 != 0 {
                    c.eax = at(c.ebp, 8);
                    frame::set_slot(c, g, 0x5c, c.eax)?;
                }
                let mut alternate = g.u8(at(c.esp, 0x13))?;
                c.eax = g.u32(at(c.edx, 0x44))?;
                c.esi = NONE;
                if c.eax == NONE && g.u32(at(c.edx, 0x30))? == NONE
                    && g.u32(at(c.edx, 0x38))? == NONE && g.u32(at(c.edx, 0x4c))? == NONE
                    && g.u8(at(c.edx, 4))? & 0x14 == 0
                {
                    alternate = 0;
                }
                g.set_u8(at(c.esp, 0x60), alternate)?;
                let created = g.u8(at(c.esp, 0x58))?;
                c.eax = byte(c.eax, created);
                if created != 0 {
                    c.eax = g.u32(at(c.ecx, 0x10))?;
                    g.set_u32(at(c.ebp, 0x10), c.eax)?;
                    g.set_u8(at(c.ebp, 0xe), 0)?;
                }
                if created != 0 || g.u32(c.ebp)? == NONE {
                    c.eax = if alternate != 0 { 3 } else { 0 };
                    g.set_u32(c.ebp, NONE)?;
                    g.set_u8(at(c.ebp, 0xc), c.eax as u8)?;
                    g.set_u8(at(c.ebp, 0xd), c.eax as u8)?;
                }
                if c.ebx == 0 {
                    c.edi = g.u32(at(c.edx, 0x18))?;
                    arithmetic(c, c.edi, NONE, true);
                    if c.edi != NONE {
                        c.edx = frame::slot(c, g, 0x54)?;
                        c.ecx = frame::slot(c, g, 0x50)?;
                        c.ebx = g.u32(at(c.edx, 0x28))?;
                        c.eax = frame::slot(c, g, 0x14)?;
                        frame::push(c, g, NONE)?;
                        return Ok(Next::Call { target: 0x21b870, site: 0x21a632 });
                    }
                }
                pc = if c.ebx == 2 { 0x21aa02 } else { 0x21a708 };
            }
            0x21a632 => {
                c.edx = frame::slot(c, g, 0x50)?;
                c.esi = c.eax;
                c.eax = frame::slot(c, g, 0x54)?;
                c.ecx = byte(c.ecx, g.u8(at(c.eax, 4))?);
                c.eax = frame::slot(c, g, 0x14)?;
                c.ebx = 1;
                let detached = c.ecx & 1;
                logic(c, detached, 0x80);
                c.ecx = frame::slot(c, g, 0x5c)?;
                for arg in [c.ecx, 1 - detached, c.edx, NONE, c.edi, c.eax] {
                    frame::push(c, g, arg)?;
                }
                let site = if detached == 0 { 0x21a65c } else { 0x21a670 };
                return Ok(Next::Call { target: 0x21b940, site });
            }
            0x21a65c => {
                g.set_u32(c.ebp, c.eax)?;
                pc = 0x21a6e8;
            }
            0x21a670 => {
                if c.esi == NONE {
                    arithmetic(c, c.esi, NONE, true);
                    pc = 0x21a6d7;
                } else {
                    c.ecx = frame::slot(c, g, 0x54)?;
                    let fade = g.u32(at(c.ecx, 0xc))?;
                    sse::load(c, 0, fade);
                    c.edx = g.u32(points::SOUND_INSTANCES.base)?;
                    c.ecx = g.u32(at(c.edx, layout::DATA))?;
                    c.eax = c.esi;
                    frame::set_slot(c, g, 0x2c, fade)?;
                    c.edi = arithmetic(c, (c.esi & 0xffff) * SoundInstance::SIZE, c.ecx, false);
                    return Ok(Next::Call { target: 0x12a810, site: 0x21a6a4 });
                }
            }
            0x21a6a4 => {
                sse::load(c, 0, frame::slot(c, g, 0x2c)?);
                let duration = sse::ss(c, Ss::Mul, 0, g.u32(0x45dccc)?);
                g.set_u32(at(c.edi, 0xb0), c.eax)?;
                c.eax = sse::cvttss2si(c, duration);
                g.set_u16(at(c.edi, 0xae), c.ebx as u16)?;
                g.set_u32(at(c.edi, 0xb4), c.eax)?;
                g.set_u32(at(c.edi, 0xb8), NONE)?;
                let flags = g.u8(at(c.edi, 4))? | 0x80;
                g.set_u8(at(c.edi, 4), flags)?;
                logic(c, flags.into(), 0x80);
                pc = 0x21a6d7;
            }
            0x21a6d7 => {
                c.ecx = frame::slot(c, g, 0x54)?;
                c.edx = g.u32(at(c.ecx, 0xc))?;
                frame::push(c, g, c.edx)?;
                frame::push(c, g, c.ebx)?;
                c.ebx = g.u32(c.ebp)?;
                return Ok(Next::Call { target: 0x21be20, site: 0x21a6e8 });
            }
            0x21a6e8 => {
                c.edx = frame::slot(c, g, 0x54)?;
                c.ecx = frame::slot(c, g, 0x20)?;
                g.set_u8(at(c.ebp, 0xc), 0)?;
                c.eax = byte(c.eax, 0);
                g.set_u8(at(c.ebp, 0xd), c.eax as u8)?;
                c.esi = NONE;
                pc = 0x21a708;
            }
            0x21ab29 => {
                c.eax = frame::slot(c, g, 0x50)?.wrapping_add(1);
                c.ecx = frame::slot(c, g, 0x18)?;
                c.edx = g.u32(at(c.ecx, 0x1c))?;
                c.ebx = frame::slot(c, g, 0x64)?;
                frame::set_slot(c, g, 0x50, c.eax)?;
                pc = if (c.eax as i32) < c.edx as i32 { 0x21a570 } else { 0x21ab45 };
            }
            0x21ab45 => {
                c.esi = frame::slot(c, g, 0x20)?;
                let voices = g.u8(at(c.esi, 0x51))?;
                c.eax = byte(c.eax, voices);
                if voices == 0 {
                    c.edx = byte(c.edx, g.u8(at(c.esi, 0xc))? >> 3);
                    c.eax = at(c.esi, 0xc);
                    logic(c, c.edx & 1, 0x80);
                    let radius = if c.edx & 1 != 0 {
                        g.u32(at(c.eax, 0x38))?
                    } else {
                        c.ecx = g.u32(at(c.esi, 4))?;
                        c.edx = g.u32(points::TAG_ENTRIES.base)?;
                        c.ecx = g.u32(at(c.edx, (c.ecx & 0xffff) * 16 + 8))?;
                        g.u32(at(c.ecx, 0x10))?
                    };
                    sse::load(c, 0, radius);
                    frame::set_slot(c, g, 0x64, radius)?;
                    c.edx = radius;
                    for arg in [0, c.edx, c.eax] {
                        frame::push(c, g, arg)?;
                    }
                    return Ok(Next::Call { target: 0x127d00, site: 0x21ab93 });
                }
                pc = 0x21ab9c;
            }
            0x21ab93 => {
                arithmetic(c, c.eax, NONE, true);
                if c.eax == NONE {
                    c.edi = frame::slot(c, g, 0x14)?;
                    return Ok(Next::Call { target: 0x21b070, site: 0x21a53f });
                }
                pc = 0x21ab9c;
            }
            0x21ab9c => {
                let alternate = g.u8(at(c.esp, 0x13))?;
                let flags = g.u8(at(c.esi, 0x50))?;
                let options = frame::slot(c, g, 0x68)?;
                // The lower three bits carry the current request's options.
                let low = u8::from(alternate != 0) | (options as u8 & 6);
                let updated = flags & !7 | low;
                let before_bit_two = flags & !3 | low & 3;
                c.ecx = byte(options >> 1, before_bit_two);
                c.eax = byte(options >> 2, updated);
                g.set_u8(at(c.esi, 0x50), updated)?;
                g.set_u8(at(c.esi, 0x52), c.ebx as u8)?;
                return finish_request(c, g);
            }
            _ => unreachable!("loop track update at {pc:#x}"),
        }
    }
}
