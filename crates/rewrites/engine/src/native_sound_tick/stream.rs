//! Service one channel's queued and current streaming chunks. All cache
//! operations may call back into the guest. In particular the current chunk
//! after a promotion and the stream phase after releasing it are fresh reads.

use super::*;
use crate::datum::layout::DATA;
use crate::points::{Loc, Point, Sig};
use crate::sim_objects::alu;
use crate::sound::{SoundInstance, SoundStream};
use sim_sound::tick::StreamPhase;

pub(super) static POINT: Point = Point::new(
    "SOUND_CHANNEL_SERVICE_STREAM",
    0x12a450,
    Sig::Fn { args: &[("channel", Loc::Eax)], ret: None, pop: 0 },
    "docs/engine/sim-rewrite.md",
);
pub(super) const BODY: Body = Body {
    point: &POINT,
    sites: &[0x12a4ce, 0x12a4da, 0x12a521, 0x12a554, 0x12a5b4],
    run: update,
};

const CACHE: u32 = 0x0051_ebd4;
const RELEASE: u32 = 0x0012_5ef0;
const NEXT_CHUNK: u32 = 0x0021_9290;
const ACQUIRE: u32 = 0x0021_8850;
const ELAPSED: u32 = 0x0021_f650;

fn signed(raw: u16) -> u32 {
    raw as i16 as i32 as u32
}

fn word(reg: u32, value: u16) -> u32 {
    (reg & 0xffff_0000) | u32::from(value)
}

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}

fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x12a450 => {
            c.ecx = g.u32(points::SOUND_CHANNELS.base)?;
            c.eax = signed(c.eax as u16).wrapping_mul(9);
            push(c, g, c.esi)?;
            c.esi = at(c.ecx, c.eax.wrapping_mul(4));
            let stream = g.u16(at(c.esi, 0xc))?;
            c.eax = word(c.eax, stream);
            alu::cmp16(c, c.eax, u32::MAX);
            if stream == u16::MAX {
                return finish_stream(c, g);
            }
            let skip = g.u8(at(c.esi, 9))?;
            c.ecx = byte(c.ecx, skip);
            logic(c, skip.into(), 0x80);
            if skip != 0 {
                return finish_stream(c, g);
            }
            c.ecx = g.u32(at(c.esi, 0x1c))?;
            logic(c, c.ecx, 1 << 31);
            if c.ecx == 0 {
                return finish_stream(c, g);
            }
            c.edx = signed(stream).wrapping_mul(SoundStream::SIZE);
            c.eax = g.u32(points::SOUND_DRIVER.base)?;
            let phase = StreamPhase::from_record(g.u8(at(at(c.edx, c.eax), SoundStream::ARRAY_OFFSET + 1))?);
            c.edx = byte(c.edx, phase.value() as u8);
            c.eax = g.u32(at(c.esi, 0x20))?;
            logic(c, c.eax, 1 << 31);
            push(c, g, c.ebp)?;
            c.ebp = word(c.ebp, phase.value() as i16 as u16);
            if c.eax != 0 && phase.accepts_queued() {
                let first = signed(g.u16(at(c.ecx, 0xc))?);
                c.ecx = signed(g.u16(at(c.esi, 0x18))?);
                c.eax = arithmetic(c, first, c.ecx, false);
                push(c, g, c.edi)?;
                c.edi = g.u32(CACHE)?;
                c.edx = c.eax.wrapping_mul(3);
                let records = g.u32(at(c.edi, DATA))?;
                c.eax = at(records, c.edx.wrapping_mul(4));
                return call(RELEASE, 0x12a4ce);
            }
            release_current(c, g)
        }
        0x12a4ce => {
            c.eax = word(c.eax, g.u16(at(c.esi, 0x18))?);
            c.ecx = g.u32(at(c.esi, 0x1c))?;
            call(NEXT_CHUNK, 0x12a4da)
        }
        0x12a4da => {
            if c.eax as u16 == u16::MAX {
                sse::zero(c, 0);
                g.set_u32(at(c.esi, 0x10), 0)?;
            }
            c.ecx = g.u32(at(c.esi, 0x20))?;
            let queued = g.u16(at(c.esi, 0x1a))?;
            c.edx = word(c.edx, queued);
            g.set_u32(at(c.esi, 0x1c), c.ecx)?;
            g.set_u16(at(c.esi, 0x18), queued)?;
            g.set_u32(at(c.esi, 0x20), 0)?;
            g.set_u16(at(c.esi, 0x1a), u16::MAX)?;
            let first = signed(g.u16(at(c.ecx, 0xc))?);
            c.ecx = signed(queued);
            c.eax = arithmetic(c, first, c.ecx, false);
            c.edx = c.eax.wrapping_mul(3);
            let records = g.u32(at(c.edi, DATA))?;
            push(c, g, 0)?;
            c.eax = at(records, c.edx.wrapping_mul(4));
            push(c, g, c.eax)?;
            push(c, g, u32::MAX)?;
            call(ACQUIRE, 0x12a521)
        }
        0x12a521 => {
            c.eax >>= 1;
            let ready = c.eax & 1 != 0;
            c.edi = pop(c, g)?;
            if !ready {
                c.ebp = 0;
            }
            release_current(c, g)
        }
        0x12a554 => {
            g.set_u32(at(c.esi, 0x1c), 0)?;
            g.set_u16(at(c.esi, 0x18), u16::MAX)?;
            update_time(c, g)
        }
        0x12a5b4 => {
            c.eax = g.u32(points::SOUND_GLOBALS.base)?;
            sse::ss(c, sse::Ss::Mul, 0, g.u32(at(c.eax, 0x1e0))?);
            let new_time = sse::ss(c, sse::Ss::Add, 0, g.u32(at(c.esi, 0x10))?);
            g.set_u32(at(c.esi, 0x10), new_time)?;
            finish_stream(c, g)
        }
        _ => unreachable!("unregistered stream service selector {pc:#x}"),
    }
}

fn release_current(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(at(c.esi, 0x1c))?;
    // BP received the sign-extended three-bit phase.
    let phase = StreamPhase::from_record(c.ebp as u8);
    if c.eax != 0 && phase.releases_current() {
        let first = signed(g.u16(at(c.eax, 0xc))?);
        c.ecx = signed(g.u16(at(c.esi, 0x18))?);
        c.eax = arithmetic(c, first, c.ecx, false);
        c.edx = c.eax.wrapping_mul(3);
        c.eax = g.u32(CACHE)?;
        c.ecx = g.u32(at(c.eax, DATA))?;
        c.eax = at(c.ecx, c.edx.wrapping_mul(4));
        return call(RELEASE, 0x12a554);
    }
    update_time(c, g)
}

fn update_time(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = signed(g.u16(at(c.esi, 0xc))?);
    c.ecx = g.u32(points::SOUND_DRIVER.base)?;
    c.edx = c.eax.wrapping_mul(SoundStream::SIZE);
    let flags = g.u8(at(at(c.edx, c.ecx), SoundStream::ARRAY_OFFSET + 1))?;
    let phase = StreamPhase::from_record(flags);
    let shifted = flags << 5;
    c.edx = byte(c.edx, shifted);
    logic(c, shifted.into(), 0x80);
    c.ebp = pop(c, g)?;
    if phase.advances_time() {
        let source = g.u32(c.esi)? & 0xffff;
        let pool = g.u32(points::SOUND_INSTANCES.base)?;
        c.edx = g.u32(at(pool, DATA))?;
        let tag = g.u32(at(at(c.edx, source * SoundInstance::SIZE), 0xc))? & 0xffff;
        c.edx = g.u32(points::TAG_ENTRIES.base)?;
        c.ecx = alu::shl(c, tag, 4, 32);
        c.ecx = g.u32(at(at(c.ecx, c.edx), 8))?;
        c.edx = g.u8(at(c.ecx, 3))? as i8 as i32 as u32;
        push(c, g, c.edx)?;
        return call(ELAPSED, 0x12a5b4);
    }
    finish_stream(c, g)
}

fn finish_stream(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}
