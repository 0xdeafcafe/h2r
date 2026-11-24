//! A positioned loop accumulates its response across the four listener
//! channels. The next frame clears those accumulators at the first request
//! with a different frame stamp.
use super::*;
use sse::Ss;

pub(super) fn run(cx: &mut Cx<'_>, mut pc: u32) -> Result<Option<Next>> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    loop {
        match pc {
            AFTER_SOURCE => {
                c.eax = byte(c.eax, g.u8(at(c.esp, 0x12))?);
                if c.eax as u8 == 0 {
                    return Ok(None);
                }
                c.edx = g.u32(points::SOUND_GLOBALS.base)?;
                c.ecx = byte(c.ecx, g.u8(at(c.ebp, 0x53))?);
                if c.ecx as u8 != g.u8(at(c.edx, 0x7f))? {
                    c.eax = 0;
                    g.write(at(c.ebp, 0xc4), [0; 16])?;
                }
                c.ecx = byte(c.ecx, g.u8(c.ebx)? >> 3);
                logic(c, c.ecx & 1, 0x80);
                let radius = if c.ecx & 1 != 0 {
                    g.u32(at(c.ebp, 0x44))?
                } else {
                    c.edx = frame::slot(c, g, 0x18)?;
                    g.u32(at(c.edx, 0x10))?
                };
                sse::load(c, 0, radius);
                frame::set_slot(c, g, 0x60, radius)?;
                c.edi = radius;
                c.eax = at(c.esp, 0x54);
                frame::push(c, g, c.eax)?;
                frame::push(c, g, c.edi)?;
                frame::push(c, g, c.ebx)?;
                return Ok(Some(Next::Call { target: 0x127d00, site: 0x21a3eb }));
            }
            0x21a3eb => {
                c.esi = c.eax;
                if c.esi == NONE {
                    break;
                }
                c.ecx = byte(c.ecx, g.u8(c.ebx)? >> 2);
                let distance = if c.ecx & 1 != 0 {
                    g.u32(at(c.ebp, 0x40))?
                } else {
                    c.edx = frame::slot(c, g, 0x18)?;
                    g.u32(at(c.edx, 0xc))?
                };
                sse::load(c, 0, g.u32(0x45dc70)?);
                sse::load(c, 1, distance);
                let lower = sse::lane0(c, 0);
                sse::comiss(c, distance, lower);
                c.ecx = if sse::below_or_equal(c) { lower } else { distance };
                frame::set_slot(c, g, 0x60, c.ecx)?;
                c.eax = frame::slot(c, g, 0x54)?;
                frame::push(c, g, c.eax)?;
                frame::push(c, g, c.edi)?;
                frame::push(c, g, c.ecx)?;
                return Ok(Some(Next::Call { target: 0x192d70, site: 0x21a43f }));
            }
            0x21a43f => {
                c.eax = at(c.esp, 0x30);
                c.ecx = c.ebx;
                frame::set_slot(c, g, 0x50, sse::lane0(c, 0))?;
                return Ok(Some(Next::Call { target: 0x12abe0, site: 0x21a450 }));
            }
            0x21a450 => {
                let kind = g.u8(at(c.ebp, 0xf))? & 0xf;
                c.edx = byte(c.edx, kind);
                if kind == 1 {
                    arithmetic(c, 1, 1, true);
                    c.ecx = g.u32(points::SOUND_GLOBALS.base)?;
                    c.eax = at(c.esi.wrapping_mul(9), 18);
                    c.edx = at(c.esp, 0x30);
                    c.ecx = at(c.ecx, c.eax.wrapping_mul(8));
                    c.esi = c.edx;
                    return Ok(Some(Next::Call { target: 0x142700, site: 0x21a473 }));
                }
                pc = 0x21a473;
            }
            0x21a473 => {
                let distance = frame::slot(c, g, 0x54)?;
                sse::load(c, 0, distance);
                sse::comiss(c, distance, frame::slot(c, g, 0x60)?);
                if !sse::below_or_equal(c) {
                    c.eax = at(c.esp, 0x30);
                    return Ok(Some(Next::Call { target: 0x30bf0, site: 0x21a489 }));
                }
                pc = 0x21a489;
            }
            0x21a489 => {
                frame::push(c, g, 1)?;
                c.edx = at(c.esp, 0x40);
                frame::push(c, g, c.edx)?;
                c.ebx = at(c.esp, 0x38);
                return Ok(Some(Next::Call { target: 0x192ce0, site: 0x21a499 }));
            }
            0x21a499 => {
                let unity = g.u32(0x45dbc0)?;
                sse::load(c, 2, unity);
                sse::zero(c, 1);
                for channel in 0..4 {
                    sse::load(c, 0, frame::slot(c, g, 0x3c + 4 * channel)?);
                    sse::ss(c, Ss::Mul, 0, frame::slot(c, g, 0x50)?);
                    sse::movaps(c, 3, 0);
                    sse::ss_reg(c, Ss::Mul, 3, 0);
                    sse::ss_reg(c, Ss::Add, 3, 1);
                    sse::movaps(c, 1, 3);
                }
                let energy = sse::lane0(c, 1);
                sse::comiss(c, energy, unity);
                if !sse::below_or_equal(c) {
                    sse::ss_reg(c, Ss::Div, 2, 1);
                }
                for channel in 0..4 {
                    let acc = at(c.ebp, 0xc4 + 4 * channel);
                    sse::load(c, 0, frame::slot(c, g, 0x3c + 4 * channel)?);
                    sse::ss_reg(c, Ss::Mul, 0, 2);
                    sse::ss(c, Ss::Mul, 0, frame::slot(c, g, 0x50)?);
                    let gain = sse::ss(c, Ss::Add, 0, g.u32(acc)?);
                    g.set_u32(acc, gain)?;
                }
                c.eax = 4;
                c.ecx = at(c.ebp, 0xd4);
                break;
            }
            _ => unreachable!("loop listener response at {pc:#x}"),
        }
    }
    g.set_u8(at(c.ebp, 2), g.u8(at(c.ebp, 2))? & !2)?;
    Ok(None)
}
