//! Select the next normal or alternate track segment and submit the
//! selected primary and overlay voices in their guest order.
use super::*;
use crate::sim_objects::sse::{self, Ss};
use sim_sound::segments::playing::{self, Segments};

const NORMAL_NEXT: u32 = 0x0044_a150;
const ALTERNATE_NEXT: u32 = 0x0044_a16c;
const REFERENCE_OFFSETS: u32 = 0x0044_a188;
const GESTALT: u32 = 0x0051_ebd4;
const PLAY_SEGMENT: u32 = 0x21b940;
const FADE: u32 = 0x126df0;

pub(super) const SITES: &[u32] = &[
    0x21a826, 0x21a8da, 0x21a8fc, 0x21a98c, 0x21a99d, 0x21a9bc, 0x21a9f4,
];

fn call(target: u32, site: u32) -> Result<TrackStep> {
    Ok(TrackStep::Transfer(Next::Call { target, site }))
}

fn index(base: u32, i: u32) -> u32 {
    at(base, i.wrapping_mul(4))
}

pub(super) fn run(cx: &mut Cx<'_>, pc: u32) -> Result<TrackStep> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x21a708 => choose(c, g),
        0x21a826 => after_overlay(c, g),
        0x21a8da => {
            c.esi = c.eax;
            arithmetic(c, c.esi, NONE, true);
            if c.esi == NONE {
                return Ok(TrackStep::Done);
            }
            crossfade(c, g)
        }
        0x21a8fc => {
            let alternate = g.u8(at(c.esp, 0x60))?;
            let kind = if alternate != 0 { 4 } else { 1 };
            c.ebx = byte(c.ebx, alternate);
            c.eax = byte(c.eax, kind);
            c.ecx = byte(c.ecx, kind);
            g.set_u32(c.ebp, c.esi)?;
            g.set_u8(at(c.ebp, 0xc), kind)?;
            g.set_u8(at(c.ebp, 0xd), kind)?;
            Ok(TrackStep::Done)
        }
        0x21a98c => {
            c.ecx = frame::slot(c, g, 0x50)?;
            c.edi = c.eax as i16 as i32 as u32;
            c.eax = frame::slot(c, g, 0x14)?;
            frame::push(c, g, c.edi)?;
            call(0x21b870, 0x21a99d)
        }
        0x21a99d => {
            c.esi = c.eax;
            arithmetic(c, c.esi, NONE, true);
            if c.esi != NONE {
                return start(c, g);
            }
            c.ecx = frame::slot(c, g, 0x5c)?;
            c.edx = frame::slot(c, g, 0x50)?;
            c.eax = frame::slot(c, g, 0x14)?;
            for arg in [c.ecx, 2, c.edx, c.edi, c.ebx, c.eax] {
                frame::push(c, g, arg)?;
            }
            call(PLAY_SEGMENT, 0x21a9bc)
        }
        0x21a9bc => {
            c.esi = c.eax;
            arithmetic(c, c.esi, NONE, true);
            if c.esi == NONE {
                Ok(TrackStep::Done)
            } else {
                start(c, g)
            }
        }
        0x21a9f4 => install(c, g),
        _ => unreachable!("playing track return site"),
    }
}

fn choose(c: &mut CpuAbi, g: &mut Guest) -> Result<TrackStep> {
    let stopping = g.u8(at(c.ecx, 0x50))? & 8;
    logic(c, u32::from(stopping), 0x80);
    if stopping != 0 {
        return Ok(TrackStep::Stopping);
    }
    let flags = g.u32(at(c.edx, 4))?;
    let options = Segments::from_track_flags(flags);
    c.edi = options.bits();
    c.ebx = c.esi;
    frame::set_slot(c, g, 0x1c, c.ebx)?;
    frame::set_slot(c, g, 0x24, c.esi)?;
    let current = g.u8(at(c.ebp, 0xc))?;
    let queued = g.u8(at(c.ebp, 0xd))?;
    let target_alt = g.u8(at(c.esp, 0x60))?;
    if current != queued {
        let off = g.u32(index(REFERENCE_OFFSETS, queued as i8 as i32 as u32))?;
        if g.u32(at(at(c.edx, off), 4))? != c.esi
            && target_alt == u8::from(Segments::alternate(current))
        {
            return Ok(TrackStep::Done);
        }
    }
    let table = if target_alt != 0 {
        ALTERNATE_NEXT
    } else {
        NORMAL_NEXT
    };
    let choice = playing::select(
        current as i8 as i32 as u32,
        c.esi,
        options,
        |kind| g.u32(index(table, kind)),
        |kind| {
            let offset = g.u32(index(REFERENCE_OFFSETS, kind))?;
            let addr = at(c.edx, offset);
            if offset == NONE || addr == 0 {
                return Ok(None);
            }
            let tag = g.u32(at(addr, 4))?;
            Ok((tag != NONE).then_some(tag))
        },
    )?;
    c.esi = choice.last;
    c.ebx = choice.primary;
    c.ecx = choice.segment;
    g.set_u8(at(c.ebp, 0xd), c.ecx as u8)?;
    frame::set_slot(c, g, 0x24, choice.overlay)?;
    c.eax = choice.overlay;
    arithmetic(c, c.eax, NONE, true);
    if c.eax == NONE {
        return after_overlay(c, g);
    }
    c.ecx = frame::slot(c, g, 0x5c)?;
    c.edx = frame::slot(c, g, 0x50)?;
    let instance = frame::slot(c, g, 0x14)?;
    for arg in [c.ecx, 0, c.edx, NONE, c.eax, instance] {
        frame::push(c, g, arg)?;
    }
    c.eax = instance;
    call(PLAY_SEGMENT, 0x21a826)
}

fn matching_sequence(c: &CpuAbi, g: &Guest) -> Result<bool> {
    let current = g.u8(at(c.ebp, 0xc))?;
    Ok(g.u8(at(c.esp, 0x60))? == u8::from(Segments::alternate(current)))
}

fn after_overlay(c: &mut CpuAbi, g: &mut Guest) -> Result<TrackStep> {
    if c.ebx == NONE && matching_sequence(c, g)? {
        return Ok(TrackStep::Done);
    }
    let mode = frame::slot(c, g, 0x64)?;
    let head = g.u32(c.ebp)?;
    let force_start = mode == 0 && g.u8(at(frame::slot(c, g, 0x54)?, 4))? & 1 != 0;
    if head == NONE || force_start {
        if c.ebx == NONE {
            return Ok(TrackStep::Done);
        }
        return pitch(c, g);
    }

    let instances = g.u32(points::SOUND_INSTANCES.base)?;
    let rows = g.u32(at(instances, layout::DATA))?;
    let row = at(rows, (head & 0xffff) * crate::sound::SoundInstance::SIZE);
    if g.u32(at(row, 0xc))? == c.ebx || matching_sequence(c, g)? {
        return Ok(TrackStep::Done);
    }
    c.ecx = frame::slot(c, g, 0x54)?;
    if g.u8(at(c.ecx, 4))? & 4 == 0 {
        return Ok(TrackStep::Done);
    }
    if c.ebx == NONE {
        c.esi = NONE;
        logic(c, NONE, 1 << 31);
        return crossfade(c, g);
    }

    c.edx = frame::slot(c, g, 0x5c)?;
    c.eax = frame::slot(c, g, 0x14)?;
    c.esi = frame::slot(c, g, 0x50)?;
    for arg in [c.edx, 2, c.eax] {
        frame::push(c, g, arg)?;
    }
    c.edi = NONE;
    logic(c, NONE, 1 << 31);
    c.eax = c.ebx;
    call(0x21b910, 0x21a8da)
}

fn crossfade(c: &mut CpuAbi, g: &mut Guest) -> Result<TrackStep> {
    c.ecx = frame::slot(c, g, 0x54)?;
    c.edx = g.u32(at(c.ecx, 0x50))?;
    c.edi = g.u32(c.ebp)?;
    frame::push(c, g, c.edx)?;
    c.eax = c.esi;
    c.ebx = 1;
    call(FADE, 0x21a8fc)
}

fn pitch(c: &mut CpuAbi, g: &Guest) -> Result<TrackStep> {
    let tags = g.u32(points::TAG_ENTRIES.base)?;
    c.ecx = g.u32(at(tags, (c.ebx & 0xffff) * 16 + 8))?;
    let range = g.u8(at(c.ecx, 0xb))? as i8 as i32 as u32;
    let gestalt = g.u32(GESTALT)?;
    let ranges = g.u32(at(gestalt, 0xc))?;
    c.edx = g.u32(at(gestalt, 4))?;
    let descriptor = at(at(ranges, range.wrapping_mul(20)), 8);
    c.esi = g.u16(descriptor)? as i16 as i32 as u32;
    let high = g.u16(at(descriptor, 2))? as i16 as i32 as u32;
    sse::cvtsi2ss(c, 1, high);
    let playback = g.u16(at(c.ecx, 6))? as i16 as i32 as u32;
    let offset = g.u16(at(at(c.edx, playback.wrapping_mul(0x38)), 0x18))? as i16 as i32 as u32;
    sse::cvtsi2ss(c, 0, c.esi);
    sse::ss_reg(c, Ss::Sub, 1, 0);
    sse::ss(c, Ss::Mul, 1, g.u32(at(c.ebp, 0x10))?);
    sse::cvtsi2ss(c, 2, offset);
    sse::ss_reg(c, Ss::Add, 1, 2);
    sse::ss_reg(c, Ss::Add, 1, 0);
    c.eax = NONE;
    logic(c, NONE, 1 << 31);
    call(0x218f50, 0x21a98c)
}

fn start(c: &mut CpuAbi, g: &mut Guest) -> Result<TrackStep> {
    c.eax = frame::slot(c, g, 0x64)?;
    c.ebx = 1;
    if c.eax == 0 {
        c.ecx = frame::slot(c, g, 0x54)?;
        if g.u8(at(c.ecx, 4))? & 1 == 0 {
            return install(c, g);
        }
        c.edx = c.ecx;
        c.eax = g.u32(at(c.edx, 0xc))?;
        frame::push(c, g, c.eax)?;
    } else {
        // The non-start request's default fade time comes from the image.
        frame::push(c, g, g.u32(0x21a9e6)?)?;
    }
    c.eax = c.esi;
    c.edi = NONE;
    logic(c, NONE, 1 << 31);
    call(FADE, 0x21a9f4)
}

fn install(c: &mut CpuAbi, g: &mut Guest) -> Result<TrackStep> {
    c.ecx = byte(c.ecx, g.u8(at(c.ebp, 0xd))?);
    g.set_u32(c.ebp, c.esi)?;
    g.set_u8(at(c.ebp, 0xc), c.ecx as u8)?;
    Ok(TrackStep::Done)
}
