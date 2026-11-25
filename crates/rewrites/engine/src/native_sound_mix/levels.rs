//! Build the level parameters on the stream frame before panner submission.

use crate::sim_objects::{Cx, Result, alu, frame, sse};
use crate::sim_physics::{byte, logic};
use runtime::{CpuAbi, Guest};
use sim_sound::environment::GainCurve;
use sim_sound::mix::levels::SpeakerCurve;

const FLOOR: u32 = 0x44a0b4;
const QUIET_MAGNITUDE: u32 = 0x45dbd8;

/// Prepare the panner descriptor at ESP + 0x1c and update the cached
/// positional levels in the stream. EDI points at the source parameters.
pub(super) fn prepare(cx: &mut Cx<'_>) -> Result<()> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let flags_at = c.edi.wrapping_add(0x30);
    let flags = g.u32(flags_at)?;
    // The panner uses bit 2 for the source's bit 1. Bits 3, 4 and 6
    // agree. The level paths below populate bits 0, 1 and 5.
    c.eax = ((flags & 2) << 1) | (flags & 0x58);
    frame::set_slot(c, g, 0x1c, c.eax)?;
    let mode_at = c.ebx.wrapping_add(3);
    let mode = g.u8(mode_at)? & !0x20;
    g.set_u8(mode_at, mode | (u8::from(c.edx as u8 != 0) << 5))?;
    let shifted = g.u32(flags_at)? >> 5;
    let four_speakers = (shifted & 1) as u8;
    c.ecx = byte(shifted, four_speakers);
    g.set_u8(c.esp.wrapping_add(0x0e), four_speakers)?;
    if four_speakers != 0 {
        speaker_levels(c, g)?;
        c.eax |= 0x20;
        logic(c, c.eax, 1 << 31);
        return frame::set_slot(c, g, 0x1c, c.eax);
    }

    let disabled = g.u8(flags_at)? & 1;
    c.edx = byte(c.edx, disabled);
    g.set_u8(c.esp.wrapping_add(0x0e), disabled)?;
    if disabled == 0 {
        let source = c.edi;
        c.edx = g.u32(source.wrapping_add(0x14))?;
        c.ecx = g.u32(source.wrapping_add(0x18))?;
        frame::set_slot(c, g, 0x24, c.edx)?;
        c.edx = g.u32(source.wrapping_add(0x1c))?;
        frame::set_slot(c, g, 0x28, c.edx)?;
        c.edx = g.u32(source.wrapping_add(0x20))?;
        frame::set_slot(c, g, 0x20, c.ecx)?;
        g.set_u32(c.ebx.wrapping_add(0x10), c.ecx)?;
        c.ecx = g.u32(source.wrapping_add(0x14))?;
        c.eax |= 1;
        frame::set_slot(c, g, 0x2c, c.edx)?;
        frame::set_slot(c, g, 0x1c, c.eax)?;
        g.set_u32(c.ebx.wrapping_add(0x18), c.ecx)?;
    }
    let submix = g.u16(c.edi.wrapping_add(0x24))?;
    alu::cmp16(c, u32::from(submix), u32::from(u16::MAX));
    if submix != u16::MAX {
        c.ecx = g.u32(c.edi.wrapping_add(0x28))?;
        frame::set_slot(c, g, 0x30, c.ecx)?;
        c.eax |= 2;
        logic(c, c.eax, 1 << 31);
        g.set_u32(c.ebx.wrapping_add(0x14), c.ecx)?;
        frame::set_slot(c, g, 0x1c, c.eax)?;
    }
    Ok(())
}

fn speaker_levels(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let mut channels = [0; 4];
    for (i, db) in channels.iter_mut().enumerate() {
        let offset = 4 * i as u32;
        *db = g.u32(c.edi.wrapping_add(0x38 + offset))?;
        frame::set_slot(c, g, 0x34 + offset, *db)?;
    }
    let curve = SpeakerCurve {
        gain: GainCurve { decibel_scale: g.u32(0x45dc1c)?, exponent_scale: g.u32(0x44a0bc)? },
        db_floor: g.u32(FLOOR)?,
        input_floor_level: g.u32(0x21fda4)?,
        output_floor_level: g.u32(0x21fe66)?,
        quiet_magnitude: g.u32(QUIET_MAGNITUDE)?,
        maximum_magnitude: g.u32(0x45dbc0)?,
        output_scale: g.u32(0x45dc08)?,
    };
    let mut stack = super::load_x87(c);
    let level = curve.combine(channels, &mut stack.0, &mut c.mxcsr);
    let (last, operand) = if level.evaluated_logarithm {
        (0x21fe43, FLOOR)
    } else {
        (0x21fdfd, QUIET_MAGNITUDE)
    };
    super::store_x87(c, g, stack, last, operand)?;
    sse::load(c, 0, level.decibels);
    sse::zero(c, 1);
    if level.loaded_maximum {
        sse::load(c, 2, curve.maximum_magnitude);
    }
    sse::load(c, 3, curve.db_floor);
    c.edx = channels.len() as u32;
    frame::set_slot(c, g, 0x10, level.decibels)?;
    frame::set_slot(c, g, 0x20, level.send)?;
    c.ecx = g.u32(c.edi.wrapping_add(0x14))?;
    frame::set_slot(c, g, 0x24, c.ecx)
}
