//! Track instance rings in the looping-sound update. Lookups stop on the
//! first matching tag and pitch range in mix order. A miss can start an
//! overlapping instance through the ordinary allocation path.

use crate::datum::layout::DATA;
use crate::points::{self, Point};
use crate::sim_objects::{Body, Cx, Next, Result, frame};
use crate::sim_physics::{ARITH, arithmetic, logic};
use crate::sound::{LoopingSound, LoopingTrack, SoundInstance};
use runtime::{Guest, GuestFault};
use sim_sound::loops::TrackRing;

const DOC: &str = "docs/engine/sim-rewrite.md";
const EMPTY: u32 = TrackRing::EMPTY;
pub static FIND_INSTANCE: Point = Point::new(
    "SOUND_TRACK_FIND_INSTANCE",
    0x21b870,
    crate::point_sig!(fn(owner: eax, track: ecx, tag: ebx, pitch: stack 0) -> eax ret 4),
    DOC,
);
pub static FIND_OR_START: Point = Point::new(
    "SOUND_TRACK_FIND_OR_START",
    0x21b910,
    crate::point_sig!(fn(tag: eax, track: esi, pitch: edi, owner: stack 0, mode: stack 1, rng: stack 2) -> eax ret 12),
    DOC,
);
pub static LINK_INSTANCE: Point = Point::new(
    "SOUND_TRACK_LINK_INSTANCE",
    0x21bc80,
    crate::point_sig!(fn(owner: eax, track: ecx, instance: esi) ret 0),
    DOC,
);

pub const BODIES: &[Body] = &[
    Body {
        point: &FIND_INSTANCE,
        sites: &[],
        run: find,
    },
    Body {
        point: &FIND_OR_START,
        sites: &[0x21b922, 0x21b93a],
        run: find_or_start,
    },
    Body {
        point: &LINK_INSTANCE,
        sites: &[],
        run: link,
    },
];

fn track(g: &Guest, owner: u32, number: u32) -> Result<(u32, u32)> {
    let pool = g.u32(points::LOOPING_SOUNDS.base)?;
    let base = g.u32(pool.wrapping_add(DATA))?;
    let record = base.wrapping_add((owner & 0xffff) * LoopingSound::SIZE);
    // The ABI accepts a signed 16-bit track. Keep the unchecked guest
    // addressing of the inner routine, including signed index arithmetic.
    let first = LoopingSound::at(record).track(0).unwrap().addr;
    let index = i32::from(number as i16) as u32;
    Ok((
        base,
        first.wrapping_add(index.wrapping_mul(LoopingTrack::SIZE)),
    ))
}

fn instance(records: u32, datum: u32) -> u32 {
    records.wrapping_add((datum & 0xffff) * SoundInstance::SIZE)
}

fn find(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let (base, track) = track(g, c.eax, c.ecx)?;
    c.ecx = base;
    c.edx = g.u32(track)?;
    frame::push(c, g, c.ebp)?;
    c.ebp = frame::slot(c, g, 8)?;
    frame::push(c, g, c.esi)?;
    c.esi = g.u32(points::SOUND_INSTANCES.base)?;
    frame::push(c, g, c.edi)?;
    let ring = TrackRing::new(c.edx);
    let mut current = ring.first();
    while let Some(datum) = current {
        let records = g.u32(c.esi.wrapping_add(DATA))?;
        c.ecx = instance(records, datum);
        if g.u32(SoundInstance::TAG.at(c.ecx))? == c.ebx {
            arithmetic(c, c.ebp, EMPTY, true);
            if c.ebp == EMPTY {
                break;
            }
            c.ecx = i32::from(g.u8(SoundInstance::PITCH_RANGE.at(c.ecx))? as i8) as u32;
            arithmetic(c, c.ecx, c.ebp, true);
            if c.ecx == c.ebp {
                break;
            }
        }
        c.ecx = records;
        let next = g.u32(SoundInstance::NEXT_ON_TRACK.at(instance(records, datum)))?;
        current = ring.following(next);
    }
    c.eax = current.unwrap_or(EMPTY);
    if current.is_none() {
        logic(c, EMPTY, 1 << 31);
    }
    c.edi = frame::pop(c, g)?;
    c.esi = frame::pop(c, g)?;
    c.ebp = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

fn find_or_start(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x21b910 => {
            frame::push(c, g, c.ebx)?;
            frame::push(c, g, c.ebp)?;
            c.ebp = frame::slot(c, g, 12)?;
            c.ebx = c.eax;
            frame::push(c, g, c.edi)?;
            c.ecx = c.esi;
            c.eax = c.ebp;
            return Ok(Next::Call {
                target: FIND_INSTANCE.base,
                site: 0x21b922,
            });
        }
        0x21b922 => {
            arithmetic(c, c.eax, EMPTY, true);
            if c.eax == EMPTY {
                c.eax = frame::slot(c, g, 0x14)?;
                c.ecx = frame::slot(c, g, 0x10)?;
                for word in [c.eax, c.ecx, c.esi, c.edi, c.ebx, c.ebp] {
                    frame::push(c, g, word)?;
                }
                return Ok(Next::Call {
                    target: 0x21b940,
                    site: 0x21b93a,
                });
            }
        }
        0x21b93a => {}
        _ => {
            return Err(GuestFault {
                addr: pc,
                len: 1,
                access: runtime::guest::Access::Execute,
                reason: runtime::guest::FaultReason::Unmapped,
            });
        }
    }
    c.ebp = frame::pop(c, g)?;
    c.ebx = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 12 })
}

fn link(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let (_, track) = track(g, c.eax, c.ecx)?;
    c.edx = track;
    frame::push(c, g, c.edi)?;
    c.edi = g.u32(points::SOUND_INSTANCES.base)?;
    let records = g.u32(c.edi.wrapping_add(DATA))?;
    let added = instance(records, c.esi);
    // ECX retains the guest pointer to the link subrecord on the empty path.
    c.ecx = added.wrapping_add(0x5c);
    c.eax = g.u32(track)?;
    arithmetic(c, c.eax, EMPTY, true);
    if c.eax != EMPTY {
        frame::push(c, g, c.ebx)?;
        let head = instance(records, c.eax);
        let successor = g.u32(SoundInstance::NEXT_ON_TRACK.at(head))?;
        g.set_u32(SoundInstance::NEXT_ON_TRACK.at(added), successor)?;
        c.edx = g.u32(track)?;
        g.set_u32(added.wrapping_add(0x70), c.edx)?;
        g.set_u32(SoundInstance::NEXT_ON_TRACK.at(head), c.esi)?;
        let successor = g.u32(SoundInstance::NEXT_ON_TRACK.at(added))?;
        c.ecx = g.u32(c.edi.wrapping_add(DATA))?;
        c.eax = (successor & 0xffff) * SoundInstance::SIZE;
        // The masked slot times the stride always fits the signed product.
        c.eflags &= !(1 | 0x800);
        c.eflags_defined = (c.eflags_defined & !ARITH) | 1 | 0x800;
        g.set_u32(c.ecx.wrapping_add(c.eax).wrapping_add(0x70), c.esi)?;
        c.ebx = frame::pop(c, g)?;
    }
    c.edi = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}
