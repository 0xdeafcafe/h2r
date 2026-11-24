//! Route the stream to the panner or the two buffers of a dynamic submix.
//! A setter may run a stream callback, so every resume uses the live frame.

use super::Step;
use crate::sim_objects::{Cx, Next, Result, frame, sse};
use crate::sim_physics::{byte, logic};
use runtime::{CpuAbi, Guest};

const SOUND_DRIVER: u32 = 0x51ebe4;
const VOLUME_FLOOR: u32 = 0x44a0b4;
const GAIN_SCALE: u32 = 0x45e008;
const BUFFER_PROPERTIES: u32 = 0x37b7e3;
const BUFFER_VOLUME: u32 = 0x37b66f;
const BUFFER_STATUS: u32 = 0x37b75b;
const BUFFER_PLAY: u32 = 0x37b6df;
const BUFFER_STOP: u32 = 0x37b71b;
const STREAM_BINS: u32 = 0x37c70a;
const PITCH: u32 = 0x22013b;

fn call(target: u32, site: u32) -> Result<Step> {
    Ok(Step::Call(Next::Call { target, site }))
}

/// Finish an AND on a byte register, retaining the other three bytes.
fn bit(c: &mut CpuAbi, g: &mut Guest, register: u32) -> Result<u32> {
    let low = register as u8 & 1;
    logic(c, low.into(), 0x80);
    g.set_u8(c.esp.wrapping_add(0x0e), low)?;
    Ok(byte(register, low))
}

pub(super) fn dispatch(cx: &mut Cx<'_>, pc: u32) -> Result<Step> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let sp = c.esp;
    match pc {
        0x21fef5 => {
            let slot = g.u16(c.edi.wrapping_add(0x24))? as i16;
            c.ecx = (c.ecx & 0xffff_0000) | u32::from(slot as u16);
            c.ebp = g.u32(SOUND_DRIVER)?;
            let dynamic_slots = g.u16(c.ebp.wrapping_add(8))? as i16;
            if slot >= dynamic_slots {
                c.edx = bit(c, g, c.eax >> 5)?;
                if c.edx as u8 == 0 {
                    c.edx = g.u32(SOUND_DRIVER)?;
                    let offset = (i32::from(slot) as u32).wrapping_mul(0x3c);
                    c.esi = c.edx.wrapping_add(0x1a18).wrapping_add(offset);
                    c.eax = sp.wrapping_add(0xd4);
                    c.ebp = 0;
                    logic(c, 0, 1 << 31);
                    c.ecx = sp.wrapping_add(0x1d4);
                    frame::set_slot(c, g, 0x114, 0)?;
                    frame::set_slot(c, g, 0x118, c.eax)?;
                    c.eax = g.u32(c.esi.wrapping_add(0x34))?;
                    frame::push(c, g, c.ecx)?;
                    frame::push(c, g, c.eax)?;
                    return call(BUFFER_PROPERTIES, 0x21ff56);
                }
            }
            begin_direct(c, g)
        }
        0x21ff56 => {
            c.eax = g.u32(c.esi.wrapping_add(0x38))?;
            c.edx = sp.wrapping_add(0x178);
            frame::push(c, g, c.edx)?;
            frame::push(c, g, c.eax)?;
            call(BUFFER_PROPERTIES, 0x21ff67)
        }
        0x21ff67 => {
            let flags = frame::slot(c, g, 0x1c)?;
            c.eax = bit(c, g, flags >> 1)?;
            if c.eax as u8 != 0 {
                c.eax = frame::slot(c, g, 0x30)?;
                frame::push(c, g, c.eax)?;
                frame::push(c, g, c.ebp)?;
                frame::push(c, g, g.u32(VOLUME_FLOOR)?)?;
                c.ecx = byte(c.ecx, 1);
                call(0x12aff0, 0x21ff87)
            } else {
                c.eax = g.u32(c.esi.wrapping_add(0x34))?;
                // DSBVOLUME_MIN, in hundredths of a dB.
                frame::push(c, g, -10_000i32 as u32)?;
                frame::push(c, g, c.eax)?;
                call(BUFFER_VOLUME, 0x21ffb0)
            }
        }
        0x21ff87 => {
            sse::load(c, 1, g.u32(GAIN_SCALE)?);
            c.ecx = g.u32(c.esi.wrapping_add(0x34))?;
            sse::ss_reg(c, sse::Ss::Mul, 0, 1);
            let scaled = sse::ss_reg(c, sse::Ss::Sub, 0, 1);
            c.eax = sse::cvttss2si(c, scaled);
            frame::push(c, g, c.eax)?;
            frame::push(c, g, c.ecx)?;
            call(BUFFER_VOLUME, 0x21ffb0)
        }
        0x21ffb0 => {
            let flags = g.u8(sp.wrapping_add(0x1c))?;
            c.ecx = bit(c, g, byte(c.ecx, flags))?;
            if c.ecx as u8 == 0 {
                c.eax = g.u32(c.esi.wrapping_add(0x38))?;
                c.edx = sp.wrapping_add(0x10);
                frame::push(c, g, c.edx)?;
                frame::push(c, g, c.eax)?;
                return call(BUFFER_STATUS, 0x220040);
            }
            c.eax = g.u32(SOUND_DRIVER)?;
            c.ecx = g.u32(c.eax.wrapping_add(0x2ab4))?;
            c.eax = g.u8(c.eax.wrapping_add(1))?.into();
            frame::set_slot(c, g, 0x84, c.ebp)?;
            c.ebp = sp.wrapping_add(0x44);
            frame::push(c, g, c.ebp)?;
            frame::push(c, g, c.eax)?;
            c.edx = sp.wrapping_add(0x44);
            frame::push(c, g, 0)?;
            c.eax = sp.wrapping_add(0x1c);
            g.set_u32(sp.wrapping_add(0x88), c.edx)?;
            c.edx = g.u32(c.ecx)?;
            frame::push(c, g, c.eax)?;
            call(g.u32(c.edx.wrapping_add(0xc))?, 0x21fff0)
        }
        0x21fff0 => {
            c.ecx = g.u32(c.esi.wrapping_add(0x38))?;
            frame::push(c, g, c.ecx)?;
            c.edx = sp.wrapping_add(0x44);
            frame::push(c, g, c.edx)?;
            c.eax = sp.wrapping_add(0x178);
            call(0x2216f0, 0x220005)
        }
        0x220005 => {
            c.eax = g.u32(c.esi.wrapping_add(0x38))?;
            c.ecx = sp.wrapping_add(0x10);
            frame::push(c, g, c.ecx)?;
            frame::push(c, g, c.eax)?;
            call(BUFFER_STATUS, 0x220013)
        }
        0x220013 | 0x220040 => {
            let playing = g.u8(sp.wrapping_add(0x10))? & 1;
            logic(c, playing.into(), 0x80);
            let play = pc == 0x220013;
            if play == (playing != 0) {
                return Ok(Step::To(PITCH));
            }
            c.esi = g.u32(c.esi.wrapping_add(0x38))?;
            let arg = if play { 0 } else { c.ebp };
            for value in [arg, arg, arg, c.esi] {
                frame::push(c, g, value)?;
            }
            if play {
                call(BUFFER_PLAY, 0x22002d)
            } else {
                call(BUFFER_STOP, 0x220057)
            }
        }
        0x22002d | 0x220057 => Ok(Step::To(PITCH)),
        0x220099 => {
            c.eax = frame::slot(c, g, 0x1c)?;
            c.ebp = g.u32(SOUND_DRIVER)?;
            panner_submix(c, g)
        }
        0x2200c8 => {
            c.eax = frame::slot(c, g, 0x1c)?;
            c.ebp = g.u32(SOUND_DRIVER)?;
            panner_levels(c, g)
        }
        0x2200f9 => {
            c.ebp = g.u32(SOUND_DRIVER)?;
            panner_finish(c, g)
        }
        0x220117 => {
            c.eax = g.u32(c.ebx.wrapping_add(0x24))?;
            c.edx = sp.wrapping_add(0x84);
            frame::push(c, g, c.edx)?;
            c.ecx = sp.wrapping_add(0x8c);
            frame::push(c, g, c.eax)?;
            g.set_u32(sp.wrapping_add(0x10), 0)?;
            g.set_u32(sp.wrapping_add(0x14), c.ecx)?;
            call(STREAM_BINS, PITCH)
        }
        _ => unreachable!("unknown route resume {pc:#x}"),
    }
}

fn begin_direct(c: &mut CpuAbi, g: &mut Guest) -> Result<Step> {
    let sp = c.esp;
    c.ecx = sp.wrapping_add(0x44);
    c.edx = bit(c, g, byte(c.edx, c.eax as u8))?;
    frame::set_slot(c, g, 0x84, 0)?;
    frame::set_slot(c, g, 0x88, c.ecx)?;
    if c.edx as u8 == 0 {
        return panner_submix(c, g);
    }
    c.ecx = g.u32(c.ebp.wrapping_add(0x2ab4))?;
    c.eax = g.u32(c.ecx)?;
    c.edx = sp.wrapping_add(0x44);
    frame::push(c, g, c.edx)?;
    logic(c, 0, 1 << 31);
    c.edx = g.u8(c.ebp.wrapping_add(1))?.into();
    frame::push(c, g, c.esi)?;
    frame::push(c, g, c.edx)?;
    c.edx = sp.wrapping_add(0x1c);
    frame::push(c, g, c.edx)?;
    call(g.u32(c.eax.wrapping_add(0xc))?, 0x220099)
}

fn panner_submix(c: &mut CpuAbi, g: &mut Guest) -> Result<Step> {
    c.ecx = bit(c, g, c.eax >> 1)?;
    if c.ecx as u8 == 0 {
        return panner_levels(c, g);
    }
    c.ecx = g.u32(c.ebp.wrapping_add(0x2ab4))?;
    c.edx = g.u32(c.ecx)?;
    c.eax = c.esp.wrapping_add(0x44);
    frame::push(c, g, c.eax)?;
    c.eax = frame::slot(c, g, 0x34)?;
    for arg in [c.eax, c.esi, 0x1f] {
        frame::push(c, g, arg)?;
    }
    call(g.u32(c.edx.wrapping_add(0x10))?, 0x2200c8)
}

fn panner_levels(c: &mut CpuAbi, g: &mut Guest) -> Result<Step> {
    c.eax = bit(c, g, c.eax >> 5)?;
    if c.eax as u8 == 0 {
        return panner_finish(c, g);
    }
    let sp = c.esp;
    c.ecx = g.u32(c.ebp.wrapping_add(0x2ab4))?;
    c.edx = g.u32(c.ecx)?;
    c.eax = sp.wrapping_add(0x44);
    frame::push(c, g, c.eax)?;
    logic(c, 0, 1 << 31);
    c.eax = g.u8(c.ebp.wrapping_add(1))?.into();
    frame::push(c, g, c.esi)?;
    frame::push(c, g, c.eax)?;
    c.eax = sp.wrapping_add(0x1c);
    frame::push(c, g, c.eax)?;
    call(g.u32(c.edx.wrapping_add(0x18))?, 0x2200f9)
}

fn panner_finish(c: &mut CpuAbi, g: &mut Guest) -> Result<Step> {
    c.ecx = g.u32(c.ebp.wrapping_add(0x2ab4))?;
    c.edx = g.u32(c.ecx)?;
    c.eax = c.esp.wrapping_add(0x44);
    frame::push(c, g, c.eax)?;
    logic(c, 0, 1 << 31);
    c.eax = g.u8(c.ebp.wrapping_add(1))?.into();
    for arg in [c.esi, c.eax, c.edi] {
        frame::push(c, g, arg)?;
    }
    call(g.u32(c.edx.wrapping_add(0x14))?, 0x220117)
}
