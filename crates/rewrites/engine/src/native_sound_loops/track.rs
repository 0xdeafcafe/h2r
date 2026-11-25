//! Pitch-range scheduling for an active loop track. Unlayered tracks crossfade
//! between two instances. Layered tracks first measure every pitch range, then
//! stop, start and update those voices in range order. The staged range table
//! survives guest allocations and callbacks in the loop update's stack frame.

use crate::points;
use crate::sim_native::sub_flags;
use crate::sim_objects::frame::{push, set_slot, slot};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_objects::{Cx, Next, Result};
use crate::sim_physics::spatial::{finish_fpu, load_fpu};
use crate::sim_physics::{ARITH, arithmetic, logic};
use crate::sound::SoundInstance;
use runtime::{CpuAbi, Guest};
use sim_sound::loops::TrackRing;
use x87::abi as fp;

const EMPTY: u32 = TrackRing::EMPTY;
const GESTALT: u32 = 0x51ebd4;
const RNG_POOL: u32 = 0x51ebd8;
const GLOBAL_RNG: u32 = 0x51ebdc;
// Locations of immediate operands in the owner's image.
const RNG_MULTIPLIER: u32 = 0x21b359;
const RNG_ADDEND: u32 = 0x21b35e;
const SILENT_GAIN: u32 = 0x21b3ab;
const CROSSFADE_TIME: u32 = 0x21b654;

fn at(base: u32, delta: u32) -> u32 {
    base.wrapping_add(delta)
}
fn low_byte(reg: u32, value: u8) -> u32 {
    (reg & !0xff) | u32::from(value)
}
fn sx8(v: u8) -> u32 {
    i32::from(v as i8) as u32
}
fn sx16(v: u16) -> u32 {
    i32::from(v as i16) as u32
}
fn voice(rows: u32, handle: u32) -> u32 {
    at(rows, (handle & 0xffff) * SoundInstance::SIZE)
}
fn tag(g: &Guest, handle: u32, entries: u32) -> Result<u32> {
    g.u32(at(entries, (handle & 0xffff) * 16 + 8))
}
fn range_slot(c: &CpuAbi, offset: u32, i: u32) -> u32 {
    at(at(c.esp, offset), i.wrapping_mul(4))
}
fn transfer(target: u32, site: u32) -> Result<Option<Next>> {
    Ok(Some(Next::Call { target, site }))
}

// Advance the track's interpolation fraction by the sound class's rate.
fn advance_fraction(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.edx = g.u32(points::SOUND_INSTANCES.base)?;
    c.edx = g.u32(at(c.edx, 0x44))?;
    c.eax = g.u32(at(voice(c.edx, c.eax), 0xc))?;
    c.edx = g.u32(points::TAG_ENTRIES.base)?;
    c.eax = sx16(g.u16(at(tag(g, c.eax, c.edx)?, 6))?);
    c.edx = g.u32(GESTALT)?;
    c.eax = at(g.u32(at(c.edx, 4))?, c.eax.wrapping_mul(0x38));
    let rate_at = at(c.eax, 0xc);
    let rate = g.u32(rate_at)?;
    let epsilon_at = 0x45dbdc;
    let mut fpu = load_fpu(c);
    fp::ld_f32(&mut fpu.0, rate);
    fp::unary(&mut fpu.0, fp::unary_op::ABS);
    fp::ld_f32(&mut fpu.0, g.u32(epsilon_at)?);
    let flags = fp::cmp(&mut fpu.0, 0, fp::src_kind::ST, 1, 1, true) & 0x45;
    fp::st_st(&mut fpu.0, 0, true);
    finish_fpu(c, g, fpu, 0x21b239, epsilon_at)?;
    let fraction = at(c.ecx, 0x10);
    if flags & 0x41 == 0 {
        c.edx = slot(c, g, 0x14)?;
        c.eax = g.u32(at(c.edx, 0x10))?;
        g.set_u32(fraction, c.eax)?;
    } else {
        c.edx = g.u32(points::SOUND_GLOBALS.base)?;
        sse::load(c, 0, g.u32(at(c.edx, 0x1e0))?);
        let limit = sse::ss(c, Ss::Mul, 0, rate);
        c.eax = slot(c, g, 0x14)?;
        sse::load(c, 1, g.u32(at(c.eax, 0x10))?);
        sse::zero(c, 2);
        let delta = sse::ss(c, Ss::Sub, 1, g.u32(fraction)?);
        let negative = sse::ss(c, Ss::Sub, 2, limit);
        sse::comiss(c, negative, delta);
        if !sse::below_or_equal(c) {
            sse::movaps(c, 0, 2);
        } else {
            sse::comiss(c, delta, limit);
            if sse::below_or_equal(c) {
                sse::movaps(c, 0, 1);
            }
        }
        let next = sse::ss(c, Ss::Add, 0, g.u32(fraction)?);
        g.set_u32(fraction, next)?;
    }
    Ok(())
}

fn select_pitch(c: &mut CpuAbi, g: &mut Guest) -> Result<Option<Next>> {
    // Tag-derived bounds interpolate with the instance's pitch offset.
    c.edx = g.u32(points::SOUND_INSTANCES.base)?;
    c.ebp = g.u32(at(c.edx, 0x44))?;
    c.ecx = g.u32(points::TAG_ENTRIES.base)?;
    c.esi = voice(c.ebp, c.eax);
    let sound = g.u32(SoundInstance::TAG.at(c.esi))?;
    c.ebx = slot(c, g, 0x10)?;
    set_slot(c, g, 0x3c, sound)?;
    c.edi = tag(g, sound, c.ecx)?;
    let scale = sx8(g.u8(at(c.edi, 0xb))?);
    let gestalt = g.u32(GESTALT)?;
    let bounds = at(
        g.u32(at(gestalt, 0xc))?,
        scale.wrapping_mul(20).wrapping_add(8),
    );
    c.edx = sx16(g.u16(bounds)?);
    c.eax = sx16(g.u16(at(bounds, 2))?);
    c.ecx = slot(c, g, 0x14)?;
    let lo = sse::cvtsi2ss(c, 0, c.edx);
    sse::cvtsi2ss(c, 1, c.eax);
    c.eax = g.u32(at(c.ecx, 0x54))?;
    c.ecx = g.u32(GLOBAL_RNG)?;
    sse::ss(c, Ss::Sub, 1, lo);
    sse::ss(c, Ss::Mul, 1, g.u32(at(c.ebx, 0x10))?);
    sse::ss(c, Ss::Add, 1, g.u32(at(c.esi, 0x90))?);
    let pitch = sse::ss(c, Ss::Add, 1, lo);
    set_slot(c, g, 0x40, pitch)?;

    if c.eax != EMPTY {
        c.edx = g.u32(RNG_POOL)?;
        let rows = g.u32(at(c.edx, 0x44))?;
        c.eax = at(rows, (c.eax & 0xffff) * 0x1c);
        c.ecx = c.eax;
    }
    let owner = g.u32(at(c.ecx, 4))?;
    sub_flags(c, owner, EMPTY, 32);
    if owner == EMPTY {
        let seed = g.u32(at(c.ecx, 8))?.wrapping_mul(g.u32(RNG_MULTIPLIER)?);
        c.eax = arithmetic(c, seed, g.u32(RNG_ADDEND)?, false);
        g.set_u32(at(c.ecx, 8), c.eax)?;
    }
    c.ebp = (c.ebp & !0xffff) | (sx8(g.u8(SoundInstance::PITCH_RANGE.at(c.esi))?) & 0xffff);
    c.ecx = g.u32(at(c.ecx, 8))?;
    set_slot(c, g, 0x20, c.ecx)?;
    c.ecx = c.edi;
    c.eax = c.ebp;
    transfer(0x218f50, 0x21b37d)
}

// Build the guest frame's range-indexed view of the instances on the
// track. A later match in the ring wins for the same pitch range.
fn collect_ranges(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.ebx = g.u32(c.ebx)?;
    c.ebp = slot(c, g, 0x3c)?;
    c.esi = 0;
    c.edx = EMPTY;
    for i in 0..9 {
        g.set_u32(range_slot(c, 0x50, i), EMPTY)?;
    }
    c.eax = g.u32(SILENT_GAIN)?;
    for i in 0..9 {
        g.set_u32(range_slot(c, 0x74, i), c.eax)?;
    }
    c.edx = g.u32(points::SOUND_INSTANCES.base)?;
    set_slot(c, g, 0x30, 0)?;
    c.ecx = c.ebx;
    loop {
        if (c.esi as i32) > 0 && c.ecx != EMPTY {
            let rows = g.u32(at(c.edx, 0x44))?;
            c.eax = at(voice(rows, c.ecx), 0x5c);
            c.ecx = g.u32(at(c.eax, 0x10))?;
            c.ecx = TrackRing::new(c.ebx).following(c.ecx).unwrap_or(EMPTY);
        }
        c.esi = c.esi.wrapping_add(1);
        if c.ecx == EMPTY {
            return Ok(());
        }
        c.eax = voice(g.u32(at(c.edx, 0x44))?, c.ecx);
        if g.u32(at(c.eax, 0xc))? == c.ebp && g.u8(at(c.eax, 2))? == 2 {
            c.eax = sx8(g.u8(SoundInstance::PITCH_RANGE.at(c.eax))?);
            g.set_u32(range_slot(c, 0x50, c.eax), c.ecx)?;
        }
    }
}

fn measure(c: &mut CpuAbi, g: &mut Guest) -> Result<Option<Next>> {
    let first = sx16(g.u16(at(c.edi, 8))?);
    c.edx = g.u32(at(c.ebp, 0x24))?;
    c.eax = arithmetic(c, first, c.esi, false);
    c.ecx = c.eax.wrapping_mul(3);
    c.edx = at(c.edx, c.ecx.wrapping_mul(4));
    c.eax = sx16(g.u16(at(c.edx, 2))?);
    c.ecx = g.u32(at(c.ebp, 0x1c))?;
    c.eax = c.eax.wrapping_mul(5);
    c.ebx = at(c.ecx, c.eax.wrapping_mul(2).wrapping_add(2));
    c.eax = slot(c, g, 0x40)?;
    push(c, g, c.eax)?;
    c.ecx = c.edi;
    transfer(0x219f80, 0x21b49a)
}

fn accepted(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.ecx = sx16(g.u16(c.ebx)?);
    let pitch = slot(c, g, 0x40)?;
    sse::load(c, 1, pitch);
    let lo = sse::cvtsi2ss(c, 0, c.ecx);
    sse::comiss(c, lo, pitch);
    g.set_u32(range_slot(c, 0x74, c.esi), c.eax)?;
    if sse::below_or_equal(c) {
        c.edx = sx16(g.u16(at(c.ebx, 2))?);
        let hi = sse::cvtsi2ss(c, 0, c.edx);
        sse::comiss(c, pitch, hi);
        if sse::below_or_equal(c) {
            sse::movaps(c, 0, 1);
        }
    }
    sse::ucomiss(c, sse::lane0(c, 0), pitch);
    sse::lahf(c);
    let equality = (c.eax >> 8) & 0x44;
    if equality.count_ones() % 2 != 0 {
        c.ecx = c.esi;
        c.eax = 1u32.wrapping_shl(c.ecx);
        set_slot(c, g, 0x30, slot(c, g, 0x30)? | c.eax)?;
    }
    c.eax = slot(c, g, 0x38)?;
    c.esi = c.esi.wrapping_add(1);
    sub_flags(c, c.esi, c.eax, 32);
    Ok(())
}

#[derive(Clone, Copy)]
enum Stage {
    Apply,
    Start,
    Update,
    Advance,
}

fn apply(c: &mut CpuAbi, g: &mut Guest, mut stage: Stage) -> Result<Option<Next>> {
    loop {
        let voice_at = range_slot(c, 0x50, c.esi);
        match stage {
            Stage::Apply => {
                c.eax = slot(c, g, 0x30)?;
                c.ecx = c.esi;
                c.edi = 1u32.wrapping_shl(c.ecx) & c.eax;
                logic(c, c.edi, 1 << 31);
                if c.edi == 0 {
                    c.eax = g.u32(voice_at)?;
                    sub_flags(c, c.eax, c.ebx, 32);
                    if c.eax != c.ebx {
                        push(c, g, 8)?;
                        return transfer(0x127320, 0x21b520);
                    }
                }
                stage = Stage::Start;
            }
            Stage::Start => {
                logic(c, c.edi, 1 << 31);
                if c.edi != 0 {
                    let existing = g.u32(voice_at)?;
                    sub_flags(c, existing, c.ebx, 32);
                    if existing == c.ebx {
                        c.edx = slot(c, g, 0x24)?;
                        c.eax = slot(c, g, 0x3c)?;
                        c.ecx = at(c.esp, 0x20);
                        push(c, g, c.ecx)?;
                        c.ecx = slot(c, g, 0x20)?;
                        for arg in [2, c.edx, c.esi, c.eax, c.ecx] {
                            push(c, g, arg)?;
                        }
                        return transfer(0x21b940, 0x21b54a);
                    }
                }
                stage = Stage::Update;
            }
            Stage::Update => {
                c.ecx = g.u32(voice_at)?;
                sub_flags(c, c.ecx, c.ebx, 32);
                if c.ecx != c.ebx {
                    c.edx = g.u32(points::SOUND_INSTANCES.base)?;
                    c.edx = g.u32(at(c.edx, 0x44))?;
                    c.edi = g.u32(range_slot(c, 0x74, c.esi))?;
                    sse::load(c, 0, g.u32(at(c.ebp, 8))?);
                    c.eax = voice(c.edx, c.ecx);
                    c.ecx = (c.ecx & 0xffff) * SoundInstance::SIZE;
                    set_slot(c, g, 0x28, c.edi)?;
                    let gain = sse::ss(c, Ss::Add, 0, slot(c, g, 0x28)?);
                    set_slot(c, g, 0x4c, gain)?;
                    c.edi = slot(c, g, 0x4c)?;
                    g.set_u32(at(c.eax, 0x94), c.edi)?;
                    g.set_u8(at(c.eax, 0x63), 0xff)?;
                }
                stage = Stage::Advance;
            }
            Stage::Advance => {
                c.eax = slot(c, g, 0x38)?;
                c.esi = c.esi.wrapping_add(1);
                sub_flags(c, c.esi, c.eax, 32);
                if (c.esi as i32) >= c.eax as i32 {
                    return Ok(None);
                }
                stage = Stage::Apply;
            }
        }
    }
}

fn begin_apply(c: &mut CpuAbi, g: &mut Guest) -> Result<Option<Next>> {
    c.eax = sx8(g.u8(at(c.edi, 0xa))?);
    c.esi = 0;
    logic(c, c.eax, 1 << 31);
    set_slot(c, g, 0x38, c.eax)?;
    if (c.eax as i32) <= 0 {
        return Ok(None);
    }
    c.ebp = slot(c, g, 0x28)?;
    c.ebx = EMPTY;
    apply(c, g, Stage::Apply)
}

fn crossfade(c: &mut CpuAbi, g: &mut Guest) -> Result<Option<Next>> {
    c.eax = g.u32(points::SOUND_INSTANCES.base)?;
    c.ecx = g.u32(at(c.eax, 0x44))?;
    c.eax = g.u32(at(c.esi, 0x90))?;
    let offset = (c.ebp & 0xffff) * SoundInstance::SIZE;
    // The signed multiply of the zero-extended datum index cannot overflow.
    c.edx = offset;
    c.eflags &= !(1 | (1 << 11));
    c.eflags_defined = (c.eflags_defined & !ARITH) | 1 | (1 << 11);
    g.set_u32(at(at(c.edx, c.ecx), 0x90), c.eax)?;
    c.ecx = slot(c, g, 0x10)?;
    c.edi = g.u32(c.ecx)?;
    push(c, g, g.u32(CROSSFADE_TIME)?)?;
    c.eax = c.ebp;
    c.ebx = 1;
    transfer(0x126df0, 0x21b664)
}

pub(super) fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Option<Next>> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x21b1c0 => {
            c.eax = slot(c, g, 0x2c)?;
            c.eax = g.u32(at(c.eax, 0x20))?;
            c.ecx = slot(c, g, 0x18)?;
            c.edx = slot(c, g, 0x14)?;
            c.eax = arithmetic(c, c.eax, c.ecx, false);
            c.ecx = slot(c, g, 0x10)?;
            set_slot(c, g, 0x28, c.eax)?;
            g.set_u8(at(c.ecx, 0xe), 0)?;
            let state = g.u8(at(c.edx, 0x52))?;
            sub_flags(c, u32::from(state), 2, 8);
            if state == 2 {
                return Ok(None);
            }
            c.eax = g.u32(c.ecx)?;
            if c.eax != EMPTY {
                advance_fraction(c, g)?;
            }
            let active = g.u8(at(c.esp, 0x13c))?;
            c.eax = low_byte(c.eax, active);
            logic(c, u32::from(active), 0x80);
            if active == 0 {
                return Ok(None);
            }
            c.ecx = slot(c, g, 0x10)?;
            c.eax = g.u32(c.ecx)?;
            sub_flags(c, c.eax, EMPTY, 32);
            if c.eax == EMPTY {
                return Ok(None);
            }
            select_pitch(c, g)
        }
        0x21b37d => {
            let layered = g.u8(at(c.edi, 1))? & 0x20;
            logic(c, u32::from(layered), 0x80);
            set_slot(c, g, 0x34, c.eax)?;
            if layered == 0 {
                sub_flags(c, c.eax, c.ebp, 16);
                if c.eax as u16 == c.ebp as u16 {
                    return Ok(None);
                }
                c.edi = sx16(slot(c, g, 0x34)? as u16);
                c.ebx = g.u32(SoundInstance::TAG.at(c.esi))?;
                c.eax = g.u32(SoundInstance::LOOP_OWNER.at(c.esi))?;
                c.ecx = slot(c, g, 0x24)?;
                push(c, g, c.edi)?;
                return transfer(0x21b870, 0x21b605);
            }
            collect_ranges(c, g)?;
            c.eax = sx8(g.u8(at(c.edi, 0xa))?);
            c.esi = 0;
            logic(c, c.eax, 1 << 31);
            set_slot(c, g, 0x38, c.eax)?;
            if (c.eax as i32) <= 0 {
                return begin_apply(c, g);
            }
            c.ebp = g.u32(GESTALT)?;
            measure(c, g)
        }
        0x21b49a => {
            accepted(c, g)?;
            if (c.esi as i32) < c.eax as i32 {
                measure(c, g)
            } else {
                begin_apply(c, g)
            }
        }
        0x21b520 => {
            g.set_u32(range_slot(c, 0x50, c.esi), c.ebx)?;
            apply(c, g, Stage::Start)
        }
        0x21b54a => {
            sub_flags(c, c.eax, c.ebx, 32);
            if c.eax != c.ebx {
                c.edx = sx16(slot(c, g, 0x34)? as u16);
                if c.edx == c.esi {
                    c.ecx = slot(c, g, 0x10)?;
                    g.set_u32(c.ecx, c.eax)?;
                }
                c.ecx = g.u32(points::SOUND_INSTANCES.base)?;
                c.ecx = g.u32(at(c.ecx, 0x44))?;
                c.edx = (c.eax & 0xffff) * SoundInstance::SIZE;
                g.set_u32(at(at(c.edx, c.ecx), 0x94), g.u32(0x21b57b)?)?;
                g.set_u32(range_slot(c, 0x50, c.esi), c.eax)?;
            }
            apply(c, g, Stage::Update)
        }
        0x21b605 => {
            c.ebp = c.eax;
            sub_flags(c, c.ebp, EMPTY, 32);
            if c.ebp != EMPTY {
                return crossfade(c, g);
            }
            c.ecx = slot(c, g, 0x24)?;
            c.eax = at(c.esp, 0x20);
            push(c, g, c.eax)?;
            c.eax = g.u32(SoundInstance::LOOP_OWNER.at(c.esi))?;
            for value in [2, c.ecx, c.edi, c.ebx, c.eax] {
                push(c, g, value)?;
            }
            transfer(0x21b940, 0x21b623)
        }
        0x21b623 => {
            c.ebp = c.eax;
            sub_flags(c, c.ebp, EMPTY, 32);
            if c.ebp == EMPTY {
                Ok(None)
            } else {
                crossfade(c, g)
            }
        }
        0x21b664 => {
            c.edx = slot(c, g, 0x10)?;
            g.set_u32(c.edx, c.ebp)?;
            Ok(None)
        }
        _ => unreachable!("active track entry or one of its return sites"),
    }
}
