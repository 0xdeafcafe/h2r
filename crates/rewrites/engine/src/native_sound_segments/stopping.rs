//! Fade the current instance and select the loop track's out segment.
//! A fade call may stop the head. The subsequent edit resolves it again.
use super::*;
use crate::sim_objects::{alu, sse};
use sim_sound::segments::stopping::OutSegment;

pub(super) const SITES: &[u32] = &[0x21aa36, 0x21aac6, 0x21aae8];

fn fade(c: &mut CpuAbi, g: &mut Guest, head: u32, duration: u32, site: u32) -> Result<TrackStep> {
    frame::push(c, g, head)?;
    frame::push(c, g, duration)?;
    Ok(TrackStep::Transfer(Next::Call { target: 0x21bd00, site }))
}

pub(super) fn run(cx: &mut Cx<'_>, pc: u32) -> Result<TrackStep> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x21aa36 | 0x21aae8 => return Ok(TrackStep::Done),
        0x21aac6 => return after_fade(c, g),
        0x21aa02 => {}
        _ => unreachable!("stop branch entry or resume"),
    }

    // Subsequent instructions at the track loop's tail set flags before
    // leaving the body. The mode-2 early exit only reads the mode.
    if g.u8(at(c.ecx, 0x52))? == 2 {
        return Ok(TrackStep::Done);
    }
    let requested_fade = frame::slot(c, g, 0x6c)?;
    sse::load(c, 0, requested_fade);
    sse::ucomiss(c, requested_fade, g.u32(0x45dbd8)?);
    sse::lahf(c);
    logic(c, (c.eax >> 8) & 0x44, 0x80);
    // Unordered takes the explicit-duration path as well.
    if c.eflags & 4 != 0 {
        c.ebp = g.u32(c.ebp)?;
        arithmetic(c, c.ebp, NONE, true);
        if c.ebp == NONE {
            return Ok(TrackStep::Done);
        }
        c.edx = frame::slot(c, g, 0x6c)?;
        return fade(c, g, c.ebp, c.edx, 0x21aa36);
    }

    let current = g.u8(at(c.ebp, 0xc))?;
    c.edi = OutSegment::alternate_selector(current);
    c.ecx = frame::slot(c, g, 0x54)?;
    c.eax = g.u32(at(c.ecx, 4))?;
    let alt_tag = if c.edi != 0 {
        let tag = g.u32(at(c.ecx, 0x38))?;
        (tag != NONE).then_some(tag)
    } else {
        None
    };
    let out = OutSegment::select(c.edi, c.eax, alt_tag);
    let offset = match out { OutSegment::Alternate => 0x38, OutSegment::Normal => 0x28 };
    c.esi = g.u32(at(c.ecx, offset))?;
    c.ebx = c.eax >> out.fade_flag_shift();
    c.ebx = byte(c.ebx, c.ebx as u8 & 1);
    g.set_u8(at(c.esp, 0x60), u8::from(out == OutSegment::Alternate))?;
    c.edx = g.u32(c.ebp)?;
    if c.edx == NONE {
        return after_fade(c, g);
    }
    if c.ebx as u8 == 0 {
        if c.esi != NONE {
            return defer_out(c, g);
        }
        c.ecx = frame::slot(c, g, 0x18)?;
        if alu::test8(c, g, c.ecx, 2)? {
            return Ok(TrackStep::Done);
        }
        c.ecx = frame::slot(c, g, 0x54)?;
    }
    logic(c, c.edi, 1 << 31);
    let alt_fade = if c.edi == 0 {
        false
    } else {
        logic(c, c.eax & 0x10, 0x80);
        c.eax & 0x10 != 0
    };
    let duration = g.u32(at(c.ecx, if alt_fade { 0x54 } else { 0x10 }))?;
    sse::load(c, 0, duration);
    frame::set_slot(c, g, 0x28, duration)?;
    let head = c.edx;
    c.edx = duration;
    fade(c, g, head, duration, 0x21aac6)
}

fn after_fade(c: &mut CpuAbi, g: &mut Guest) -> Result<TrackStep> {
    arithmetic(c, c.esi, NONE, true);
    if c.esi == NONE { return Ok(TrackStep::Done); }
    logic(c, c.ebx & 0xff, 0x80);
    if c.ebx as u8 == 0 { return defer_out(c, g); }

    c.eax = frame::slot(c, g, 0x5c)?;
    c.ecx = frame::slot(c, g, 0x50)?;
    c.edx = frame::slot(c, g, 0x14)?;
    for arg in [c.eax, 0, c.ecx, NONE, c.esi, c.edx] {
        frame::push(c, g, arg)?;
    }
    Ok(TrackStep::Transfer(Next::Call { target: 0x21b940, site: 0x21aae8 }))
}

fn defer_out(c: &mut CpuAbi, g: &mut Guest) -> Result<TrackStep> {
    c.eax = g.u32(c.ebp)?;
    arithmetic(c, c.eax, NONE, true);
    if c.eax != NONE {
        c.ecx = g.u32(points::SOUND_INSTANCES.base)?;
        c.edx = g.u32(at(c.ecx, 0x44))?;
        c.eax = at(c.edx, (c.eax & 0xffff) * crate::sound::SoundInstance::SIZE);
        let channel = g.u16(at(c.eax, 0xac))?;
        alu::cmp16(c, u32::from(channel), 0xffff);
        if channel != u16::MAX {
            c.ecx = byte(c.ecx, g.u8(at(c.esp, 0x60))?);
            logic(c, c.ecx & 0xff, 0x80);
            c.edx = byte(c.edx, u8::from(c.ecx as u8 != 0)).wrapping_mul(4).wrapping_add(2);
            g.set_u8(at(c.ebp, 0xd), c.edx as u8)?;
            g.set_u8(at(c.eax, 2), 3)?;
        }
    }
    Ok(TrackStep::Done)
}
