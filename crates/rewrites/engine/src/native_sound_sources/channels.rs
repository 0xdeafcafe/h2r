//! Channel choice for a sound instance: the census of occupied channels
//! (128500), the class ranking (128b90), the pick that stops an
//! incompatible loop owner or the oldest owner (128700, 128920) and the
//! per-class ceiling (128a60). Each body keeps the retail frame, so a
//! callback or the distance leaf can change the channel pool between
//! visits. Decisions come from `sim_sound`; the rest is retail's
//! registers, stack slots and flags at every transfer.
use super::instances::{cmp_byte, imul};
use super::*;
use sim_sound::scheduling::{CappedSelection, inherit_source};
use sim_sound::sources::{PriorityTest, VoicePriority};

const CENSUS_POINT: Point = Point::new(
    "SOUND_CHANNEL_CENSUS",
    0x128500,
    Sig::Fn {
        args: &[("results", Loc::Esi), ("instance", Loc::Stack(0))],
        ret: None,
        pop: 4,
    },
    DOC,
);
const COMPARE_POINT: Point = Point::new(
    "SOUND_COMPARE_CHANNEL_PRIORITY",
    0x128b90,
    Sig::Fn {
        args: &[
            ("owner", Loc::Eax),
            ("instance", Loc::Ecx),
            ("distance", Loc::Stack(0)),
        ],
        ret: Some(Loc::Al),
        pop: 4,
    },
    DOC,
);
const SELECT_POINT: Point = Point::new(
    "SOUND_CHOOSE_CHANNEL",
    0x128700,
    Sig::Fn {
        args: &[("instance", Loc::Stack(0)), ("reason", Loc::Stack(1))],
        ret: Some(Loc::Eax),
        pop: 8,
    },
    DOC,
);
const PRESSURE_POINT: Point = Point::new(
    "SOUND_CHANNEL_PRESSURE",
    0x128920,
    Sig::Fn {
        args: &[("instance", Loc::Stack(0)), ("reason", Loc::Stack(1))],
        ret: Some(Loc::Eax),
        pop: 8,
    },
    DOC,
);
const CAPPED_POINT: Point = Point::new(
    "SOUND_CHOOSE_LIMITED_CHANNEL",
    0x128a60,
    Sig::Fn {
        args: &[
            ("instance", Loc::Eax),
            ("count", Loc::Ecx),
            ("channels", Loc::Stack(0)),
        ],
        ret: Some(Loc::Eax),
        pop: 4,
    },
    DOC,
);
pub(super) const CENSUS: Body = Body {
    point: &CENSUS_POINT,
    sites: &[0x12865b],
    run: census,
};
pub(super) const COMPARE: Body = Body {
    point: &COMPARE_POINT,
    sites: &[0x128c81],
    run: compare,
};
pub(super) const SELECT: Body = Body {
    point: &SELECT_POINT,
    sites: &[
        0x128765, 0x128801, 0x128868, 0x128880, 0x128889, 0x1288a0, 0x1288d9, 0x128900, 0x128911,
    ],
    run: select,
};
pub(super) const PRESSURE: Body = Body {
    point: &PRESSURE_POINT,
    sites: &[0x128970, 0x1289c9, 0x1289e1, 0x128a1a],
    run: pressure,
};
pub(super) const CAPPED: Body = Body {
    point: &CAPPED_POINT,
    sites: &[],
    run: capped,
};

const INSTANCES: u32 = points::SOUND_INSTANCES.base;
const CHANNELS: u32 = points::SOUND_CHANNELS.base;
const SOUND_GLOBALS: u32 = points::SOUND_GLOBALS.base;
const TAGS: u32 = points::TAG_ENTRIES.base;
const GLOBALS_TAG: u32 = points::GLOBALS_TAG.base;
const SOUND_CLASS: u32 = 0x221810;
const NONE: u32 = u32::MAX;
const SIGN: u32 = 1 << 31;

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}
fn sext8(value: u8) -> u32 {
    value as i8 as i32 as u32
}
fn sext16(value: u32) -> u32 {
    value as u16 as i16 as i32 as u32
}
/// `and r, 0ffffh`.
fn low_word(c: &mut CpuAbi, value: u32) -> u32 {
    let result = value & 0xffff;
    logic(c, result, SIGN);
    result
}
/// `and r,0ffffh; shl r,4`: the tag-entry offset of a handle.
fn tag_data_offset(c: &mut CpuAbi, handle: u32) -> u32 {
    let masked = low_word(c, handle);
    alu::shl(c, masked, 4, 32)
}
/// The record offset of a datum's slot (`imul r, 0bch`).
fn scale(c: &mut CpuAbi, index: u32) -> u32 {
    imul(c, index, SoundInstance::SIZE as i32)
}
/// `inc r16/m16` or `inc r32`: carry is kept.
fn increment(c: &mut CpuAbi, value: u32, bits: u32) -> u32 {
    let mask = if bits == 32 { u32::MAX } else { (1 << bits) - 1 };
    let sign = 1 << (bits - 1);
    let result = value.wrapping_add(1) & mask;
    let keep = crate::sim_physics::ARITH & !1;
    let adjust = (value ^ 1 ^ result) & 0x10;
    let overflow = u32::from(value & mask == sign - 1) << 11;
    c.eflags = (c.eflags & !keep) | crate::sim_physics::status(result, sign) | adjust | overflow;
    c.eflags_defined |= keep;
    result
}
fn push_all(c: &mut CpuAbi, g: &mut Guest, words: &[u32]) -> Result<()> {
    for &word in words {
        frame::push(c, g, word)?;
    }
    Ok(())
}

/// Collect the channels that hold the same sound class, source and owner.
/// The results block counts the tag-level matches at +0 with their channel
/// indices from +2, and the per-object matches at +0x26 with their indices
/// from +0x28. Byte +0x24 says whether an equal-source owner was found, and
/// the two limits are copied to +0x22 and +0x48.
fn census(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let mut state = pc;
    loop {
        state = match state {
            0x128500 => {
                c.esp = arithmetic(c, c.esp, 8, true);
                c.eax = g.u32(INSTANCES)?;
                frame::push(c, g, c.ebx)?;
                c.ebx = g.u32(at(c.eax, RECORDS))?;
                c.eax = g.u32(TAGS)?;
                push_all(c, g, &[c.ebp, c.edi])?;
                c.edi = frame::slot(c, g, 0x18)?;
                c.edi = low_word(c, c.edi);
                c.edi = scale(c, c.edi);
                c.ecx = g.u32(at(at(c.edi, c.ebx), 12))?;
                c.edi = arithmetic(c, c.edi, c.ebx, false);
                c.ecx = tag_data_offset(c, c.ecx);
                c.ebx = g.u32(at(at(c.ecx, c.eax), 8))?;
                c.ecx = g.u32(GLOBALS_TAG)?;
                c.edx = 0;
                logic(c, 0, SIGN);
                g.set_u8(at(c.esi, 0x24), 0)?;
                g.set_u16(c.esi, 0)?;
                g.set_u16(at(c.esi, 0x26), 0)?;
                let palette = g.u32(at(c.ecx, 0xc0))?;
                arithmetic(c, palette, c.edx, true);
                frame::set_slot(c, g, 0x10, c.ebx)?;
                if palette != 0 {
                    c.edx = g.u32(at(c.ecx, 0xc4))?;
                }
                c.edx = g.u32(at(c.edx, 4))?;
                c.ebp = sext8(g.u8(at(c.ebx, 2))?);
                c.edx = tag_data_offset(c, c.edx);
                c.ebp = imul(c, c.ebp, 0x5c);
                c.edx = g.u32(at(at(c.edx, c.eax), 8))?;
                c.edx = g.u32(at(c.edx, 4))?;
                let limit = g.u16(at(c.edx, c.ebp))?;
                c.edx = lo16(c.edx, limit);
                g.set_u16(at(c.esi, 0x22), limit)?;
                c.edx = g.u32(at(c.ecx, 0xc0))?;
                logic(c, c.edx, SIGN);
                if c.edx == 0 {
                    c.ecx = 0;
                    logic(c, 0, SIGN);
                } else {
                    c.ecx = g.u32(at(c.ecx, 0xc4))?;
                }
                c.ecx = g.u32(at(c.ecx, 4))?;
                c.ecx = tag_data_offset(c, c.ecx);
                c.edx = g.u32(at(at(c.ecx, c.eax), 8))?;
                c.eax = sext8(g.u8(at(c.ebx, 2))?);
                c.ecx = g.u32(at(c.edx, 4))?;
                c.eax = imul(c, c.eax, 0x5c);
                let object_limit = g.u16(at(at(c.ecx, c.eax), 2))?;
                c.edx = lo16(c.edx, object_limit);
                c.eax = g.u32(SOUND_GLOBALS)?;
                c.ebx = 0;
                logic(c, 0, SIGN);
                g.set_u16(at(c.esi, 0x48), object_limit)?;
                let count = g.u16(at(c.eax, 0x208))?;
                alu::cmp16(c, count.into(), c.ebx);
                frame::set_slot(c, g, 0xc, c.ebx)?;
                if (count as i16) <= 0 { 0x12869f } else { 0x1285d0 }
            }
            0x1285d0 => {
                c.edx = g.u32(CHANNELS)?;
                c.eax = sext16(c.ebx);
                c.ecx = c.eax.wrapping_mul(9);
                let row = at(c.edx, c.ecx.wrapping_mul(4));
                c.eax = g.u32(row)?;
                c.ecx = row;
                if c.eax == NONE || c.eax == frame::slot(c, g, 0x18)? {
                    0x128688
                } else {
                    c.edx = g.u32(INSTANCES)?;
                    c.ebp = g.u32(at(c.edx, RECORDS))?;
                    c.eax = (c.eax & 0xffff).wrapping_mul(SoundInstance::SIZE);
                    c.eax = at(c.eax, c.ebp);
                    c.ebp = c.eax;
                    c.eax = frame::slot(c, g, 0x10)?;
                    c.edx = byte(c.edx, g.u8(at(c.eax, 4))?);
                    if c.edx as u8 != g.u8(at(c.ecx, 8))? {
                        0x128688
                    } else {
                        c.eax = g.u32(at(c.edi, 12))?;
                        if c.eax != g.u32(at(c.ebp, 12))? {
                            0x128688
                        } else {
                            let count = g.u16(c.esi)?;
                            c.ecx = sext16(count.into());
                            g.set_u16(
                                at(c.esi, c.ecx.wrapping_mul(2).wrapping_add(2)),
                                c.ebx as u16,
                            )?;
                            let next = increment(c, count.into(), 16) as u16;
                            g.set_u16(c.esi, next)?;
                            c.eax = g.u32(at(c.edi, 0x10))?;
                            if c.eax == NONE {
                                0x128688
                            } else {
                                c.edx = g.u32(at(c.ebp, 0x10))?;
                                if c.edx == NONE {
                                    0x128688
                                } else {
                                    c.ecx = g.u32(at(c.edi, 0x14))?;
                                    if c.ecx != g.u32(at(c.ebp, 0x14))? {
                                        0x128688
                                    } else if c.eax == c.edx {
                                        0x128663
                                    } else {
                                        c.ecx = g.u32(at(c.ecx, 0x1c))?;
                                        logic(c, c.ecx, SIGN);
                                        if c.ecx == 0 {
                                            0x128688
                                        } else {
                                            c.ebx = at(c.ebp, 0x5c);
                                            let first = at(c.edi, 0x5c);
                                            push_all(c, g, &[c.ebx, c.edx, first, c.eax])?;
                                            c.edx = first;
                                            return call(c.ecx, 0x12865b);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            0x12865b => {
                logic(c, c.eax & 0xff, 0x80);
                c.ebx = frame::slot(c, g, 0xc)?;
                if c.eax as u8 == 0 { 0x128688 } else { 0x128663 }
            }
            0x128663 => {
                let count = g.u16(at(c.esi, 0x26))?;
                c.eax = sext16(count.into());
                c.ecx = g.u32(SOUND_GLOBALS)?;
                g.set_u16(
                    at(c.esi, c.eax.wrapping_mul(2).wrapping_add(0x28)),
                    c.ebx as u16,
                )?;
                let next = increment(c, count.into(), 16) as u16;
                g.set_u16(at(c.esi, 0x26), next)?;
                c.edx = g.u32(at(c.ecx, 0x84))?;
                let started = g.u32(at(c.ebp, 0x8c))?;
                arithmetic(c, c.edx, started, true);
                if c.edx == started {
                    g.set_u8(at(c.esi, 0x24), 1)?;
                }
                0x128688
            }
            0x128688 => {
                c.eax = g.u32(SOUND_GLOBALS)?;
                c.ebx = increment(c, c.ebx, 32);
                let count = g.u16(at(c.eax, 0x208))?;
                alu::cmp16(c, c.ebx, count.into());
                frame::set_slot(c, g, 0xc, c.ebx)?;
                if (c.ebx as i16) < count as i16 { 0x1285d0 } else { 0x12869f }
            }
            0x12869f => {
                c.edi = frame::pop(c, g)?;
                c.ebp = frame::pop(c, g)?;
                c.ebx = frame::pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 8, false);
                return Ok(Next::Ret { pop: 4 });
            }
            _ => return Err(invalid_site(pc)),
        };
    }
}

/// Does the instance in `ecx` outrank the owner in `eax`? Sound class
/// decides; the same tag compares layers; a remaining tie compares the
/// distances the caller measured. The answer is in `al`.
fn compare(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let prefer = match pc {
        0x128b90 => {
            frame::push(c, g, c.ecx)?;
            c.edx = g.u32(INSTANCES)?;
            c.edx = g.u32(at(c.edx, RECORDS))?;
            c.eax = (c.eax & 0xffff).wrapping_mul(SoundInstance::SIZE);
            c.ecx = (c.ecx & 0xffff).wrapping_mul(SoundInstance::SIZE);
            c.ecx = at(c.ecx, c.edx);
            c.eax = at(c.eax, c.edx);
            c.edx = g.u32(at(c.eax, 12))?;
            frame::push(c, g, c.ebx)?;
            c.ebx = g.u32(at(c.ecx, 12))?;
            frame::push(c, g, c.ebp)?;
            c.ebp = g.u32(GLOBALS_TAG)?;
            c.edx &= 0xffff;
            frame::push(c, g, c.esi)?;
            c.esi = g.u32(TAGS)?;
            frame::push(c, g, c.edi)?;
            c.edx = g.u32(at(at(c.edx << 4, c.esi), 8))?;
            c.edi = (c.ebx & 0xffff) << 4;
            frame::set_slot(c, g, 0x10, c.edx)?;
            let palette = g.u32(at(c.ebp, 0xc0))?;
            c.edx = palette;
            c.edi = g.u32(at(at(c.edi, c.esi), 8))?;
            c.ebp = if palette != 0 { g.u32(at(c.ebp, 0xc4))? } else { 0 };
            c.edi = sext8(g.u8(at(c.edi, 2))?).wrapping_mul(0x5c);
            c.ebp = g.u32(at(c.ebp, 4))?;
            c.ebp = (c.ebp & 0xffff) << 4;
            c.ebp = g.u32(at(at(c.esi, c.ebp), 8))?;
            c.ebp = g.u32(at(c.ebp, 4))?;
            c.edi = sw(g.u16(at(at(c.edi, c.ebp), 12))?);
            c.edx = if palette != 0 {
                let globals = g.u32(GLOBALS_TAG)?;
                g.u32(at(globals, 0xc4))?
            } else {
                0
            };
            c.edx = g.u32(at(c.edx, 4))?;
            c.edx = (c.edx & 0xffff) << 4;
            c.edx = g.u32(at(at(c.edx, c.esi), 8))?;
            c.esi = frame::slot(c, g, 0x10)?;
            c.esi = sext8(g.u8(at(c.esi, 2))?);
            c.edx = g.u32(at(c.edx, 4))?;
            c.esi = c.esi.wrapping_mul(0x5c);
            c.edx = sw(g.u16(at(at(c.edx, c.esi), 12))?);
            arithmetic(c, c.edi, c.edx, true);
            let challenger = VoicePriority {
                class: c.edi as i16,
                tag: c.ebx,
            };
            let candidate = VoicePriority {
                class: c.edx as i16,
                tag: g.u32(at(c.eax, 12))?,
            };
            match challenger.compare(candidate) {
                PriorityTest::Prefer(prefer) => prefer,
                decision => {
                    arithmetic(c, c.ebx, candidate.tag, true);
                    let layer = if decision == PriorityTest::CompareLayer {
                        let (ours, theirs) = (g.u8(at(c.ecx, 0xa0))?, g.u8(at(c.eax, 0xa0))?);
                        c.ecx = byte(c.ecx, ours);
                        c.edx = byte(c.edx, theirs);
                        cmp_byte(c, ours, theirs);
                        Some((ours as i8).cmp(&(theirs as i8)))
                    } else {
                        None
                    };
                    match layer {
                        Some(std::cmp::Ordering::Greater) => true,
                        Some(std::cmp::Ordering::Less) => false,
                        _ => {
                            c.edx = sext8(g.u8(at(c.eax, 0xa1))?);
                            c.ecx = at(c.eax, 0x18);
                            return call(listeners::DISTANCE.base, 0x128c81);
                        }
                    }
                }
            }
        }
        0x128c81 => {
            let flags = sse::comiss(c, sse::lane0(c, 0), frame::slot(c, g, 0x18)?);
            flags & 0x41 == 0
        }
        _ => return Err(invalid_site(pc)),
    };
    c.edi = frame::pop(c, g)?;
    c.esi = frame::pop(c, g)?;
    c.ebp = frame::pop(c, g)?;
    if prefer {
        c.eax = 1;
    } else {
        c.eax = 0;
        logic(c, 0, SIGN);
    }
    c.ebx = frame::pop(c, g)?;
    c.ecx = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

/// `mov cl,[eax+8]; shr cl,1; test cl,1`: the class row's loop bit.
fn loop_flag(c: &mut CpuAbi, g: &Guest) -> Result<bool> {
    let flags = g.u8(at(c.eax, 8))?;
    let shifted = alu::shr(c, flags.into(), 1, 8) as u8;
    c.ecx = byte(c.ecx, shifted);
    logic(c, (shifted & 1).into(), 0x80);
    Ok(shifted & 1 != 0)
}

/// The epilogue shared by the exits: restore the four saved registers and
/// release the frame.
fn leave(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = frame::pop(c, g)?;
    c.esi = frame::pop(c, g)?;
    c.ebp = frame::pop(c, g)?;
    c.ebx = frame::pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x60, false);
    Ok(Next::Ret { pop: 8 })
}

/// Choose the channel for a new instance. An instance that already has a
/// channel keeps it. A loop sound first looks for an incompatible owner of
/// the same loop to take over, then the census limits, then pressure.
fn select(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let mut state = pc;
    loop {
        state = match state {
            0x128700 => {
                c.eax = g.u32(INSTANCES)?;
                c.edx = g.u32(at(c.eax, RECORDS))?;
                c.esp = arithmetic(c, c.esp, 0x60, true);
                frame::push(c, g, c.ebx)?;
                c.ebx = frame::slot(c, g, 0x6c)?;
                frame::push(c, g, c.ebp)?;
                c.ebp = frame::slot(c, g, 0x6c)?;
                push_all(c, g, &[c.esi, c.edi])?;
                c.edi = low_word(c, c.ebp);
                c.edi = scale(c, c.edi);
                c.edi = arithmetic(c, c.edi, c.edx, false);
                g.set_u32(c.ebx, 0)?;
                c.eax = lo16(c.eax, g.u16(at(c.edi, 0xac))?);
                c.esi = NONE;
                logic(c, NONE, SIGN);
                alu::cmp16(c, c.eax, c.esi);
                if c.eax as u16 != 0xffff {
                    0x128911
                } else {
                    c.ecx = g.u32(at(c.edi, 12))?;
                    c.edx = g.u32(TAGS)?;
                    c.ecx = tag_data_offset(c, c.ecx);
                    c.eax = g.u32(at(at(c.ecx, c.edx), 8))?;
                    frame::set_slot(c, g, 0x18, c.eax)?;
                    c.eax = lo16(c.eax, sext8(g.u8(at(c.eax, 2))?) as u16);
                    frame::push(c, g, c.eax)?;
                    return call(SOUND_CLASS, 0x128765);
                }
            }
            0x128765 => {
                if !loop_flag(c, g)? {
                    0x128896
                } else {
                    let owner = g.u32(at(c.edi, 0x10))?;
                    arithmetic(c, owner, c.esi, true);
                    if owner == c.esi {
                        0x128896
                    } else {
                        c.edx = g.u32(SOUND_GLOBALS)?;
                        c.eax = 0;
                        logic(c, 0, SIGN);
                        c.eax = lo16(c.eax, g.u16(at(c.edx, 0x208))?);
                        c.ebx = 0;
                        logic(c, 0, SIGN);
                        alu::test16(c, c.eax);
                        frame::set_slot(c, g, 0x10, c.esi)?;
                        frame::set_slot(c, g, 0x14, c.esi)?;
                        frame::set_slot(c, g, 0x20, c.eax)?;
                        if (c.eax as i16) <= 0 { 0x12885d } else { 0x1287a2 }
                    }
                }
            }
            0x1287a2 => {
                c.ecx = g.u32(CHANNELS)?;
                c.eax = sext16(c.ebx).wrapping_mul(9);
                let row = at(c.ecx, c.eax.wrapping_mul(4));
                c.ebp = g.u32(row)?;
                arithmetic(c, c.ebp, NONE, true);
                c.eax = row;
                frame::set_slot(c, g, 0x1c, c.eax)?;
                if c.ebp == NONE {
                    0x128823
                } else {
                    c.edx = g.u32(INSTANCES)?;
                    c.eax = g.u32(at(c.edx, RECORDS))?;
                    c.esi = (c.ebp & 0xffff).wrapping_mul(SoundInstance::SIZE);
                    c.ecx = g.u32(at(at(c.esi, c.eax), 0x10))?;
                    c.esi = at(c.esi, c.eax);
                    c.eax = g.u32(at(c.edi, 0x10))?;
                    arithmetic(c, c.ecx, c.eax, true);
                    if c.ecx != c.eax {
                        0x128823
                    } else {
                        c.eax = g.u32(at(c.esi, 12))?;
                        c.ecx = g.u32(TAGS)?;
                        c.eax = tag_data_offset(c, c.eax);
                        c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
                        c.eax = lo16(c.eax, sext8(g.u8(at(c.edx, 2))?) as u16);
                        frame::push(c, g, c.eax)?;
                        return call(SOUND_CLASS, 0x128801);
                    }
                }
            }
            0x128801 => {
                if !loop_flag(c, g)? {
                    0x128823
                } else {
                    c.edx = frame::slot(c, g, 0x18)?;
                    c.eax = byte(c.eax, g.u8(at(c.edx, 4))?);
                    c.ecx = frame::slot(c, g, 0x1c)?;
                    let kind = g.u8(at(c.ecx, 8))?;
                    cmp_byte(c, c.eax as u8, kind);
                    if c.eax as u8 == kind {
                        0x128831
                    } else {
                        frame::set_slot(c, g, 0x14, c.ebx)?;
                        frame::set_slot(c, g, 0x10, c.ebp)?;
                        0x128823
                    }
                }
            }
            0x128823 => {
                c.ebx = increment(c, c.ebx, 32);
                let end = g.u16(at(c.esp, 0x20))?;
                alu::cmp16(c, c.ebx, end.into());
                if (c.ebx as i16) < end as i16 { 0x1287a2 } else { 0x128859 }
            }
            0x128831 => {
                let (old, request) = (g.u8(at(c.esi, 0x1b))?, g.u8(at(c.edi, 0x1b))?);
                c.edx = byte(c.edx, (old ^ request) & 0xf);
                let merged = inherit_source(request, old);
                logic(c, merged.into(), 0x80);
                c.eax = frame::slot(c, g, 0x78)?;
                c.ecx = byte(c.ecx, merged);
                alu::cmp16(c, c.ebx, 0xffff);
                g.set_u8(at(c.edi, 0x1b), merged)?;
                g.set_u32(c.eax, 13)?;
                if c.ebx as u16 != 0xffff { 0x128870 } else { 0x128859 }
            }
            0x128859 => {
                c.ebp = frame::slot(c, g, 0x74)?;
                0x12885d
            }
            0x12885d => {
                c.ecx = frame::slot(c, g, 0x78)?;
                push_all(c, g, &[c.ecx, c.ebp])?;
                return call(PRESSURE_POINT.base, 0x128868);
            }
            0x128868 => {
                c.ebx = c.eax;
                alu::cmp16(c, c.ebx, 0xffff);
                if c.ebx as u16 == 0xffff { 0x128889 } else { 0x128870 }
            }
            0x128870 => {
                c.eax = frame::slot(c, g, 0x10)?;
                arithmetic(c, c.eax, NONE, true);
                if c.eax == NONE {
                    0x128889
                } else {
                    frame::push(c, g, 13)?;
                    return call(0x127320, 0x128880);
                }
            }
            0x128880 => {
                c.eax = frame::slot(c, g, 0x14)?;
                return call(0x12a5d0, 0x128889);
            }
            0x128889 => {
                c.edi = frame::pop(c, g)?;
                c.esi = frame::pop(c, g)?;
                c.ebp = frame::pop(c, g)?;
                c.eax = lo16(c.eax, c.ebx as u16);
                c.ebx = frame::pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x60, false);
                return Ok(Next::Ret { pop: 8 });
            }
            0x128896 => {
                frame::push(c, g, c.ebp)?;
                c.esi = at(c.esp, 0x28);
                return call(CENSUS_POINT.base, 0x1288a0);
            }
            0x1288a0 => {
                c.eax = byte(c.eax, g.u8(at(c.esp, 0x48))?);
                logic(c, c.eax & 0xff, 0x80);
                if c.eax as u8 != 0 {
                    c.edi = frame::pop(c, g)?;
                    c.esi = frame::pop(c, g)?;
                    c.ebp = frame::pop(c, g)?;
                    g.set_u32(c.ebx, 5)?;
                    c.eax = lo16(c.eax, 0xffff);
                    c.ebx = frame::pop(c, g)?;
                    c.esp = arithmetic(c, c.esp, 0x60, false);
                    return Ok(Next::Ret { pop: 8 });
                }
                c.ecx = g.u32(at(c.esp, 0x4a))?;
                let ceiling = g.u16(at(c.esp, 0x6c))?;
                alu::cmp16(c, c.ecx, ceiling.into());
                if (c.ecx as i16) < ceiling as i16 {
                    0x1288e3
                } else {
                    c.edx = at(c.esp, 0x4c);
                    frame::push(c, g, c.edx)?;
                    c.eax = c.ebp;
                    g.set_u32(c.ebx, 5)?;
                    return call(CAPPED_POINT.base, 0x1288d9);
                }
            }
            0x1288e3 => {
                c.ecx = g.u32(at(c.esp, 0x24))?;
                let ceiling = g.u16(at(c.esp, 0x46))?;
                alu::cmp16(c, c.ecx, ceiling.into());
                if (c.ecx as i16) < ceiling as i16 {
                    0x12890a
                } else {
                    c.eax = at(c.esp, 0x26);
                    frame::push(c, g, c.eax)?;
                    c.eax = c.ebp;
                    g.set_u32(c.ebx, 6)?;
                    return call(CAPPED_POINT.base, 0x128900);
                }
            }
            0x12890a => {
                push_all(c, g, &[c.ebx, c.ebp])?;
                return call(PRESSURE_POINT.base, 0x128911);
            }
            0x1288d9 | 0x128900 | 0x128911 => return leave(c, g),
            _ => return Err(invalid_site(pc)),
        };
    }
}

fn leave_pressure(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.ebp = frame::pop(c, g)?;
    c.ebx = frame::pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 12, false);
    Ok(Next::Ret { pop: 8 })
}

/// Look for a channel to take. The first channel with no owner ends the
/// scan. Otherwise the best candidate is the owner of the same source that
/// the instance outranks, nearest first. `reason` receives 9 when one is
/// found; the result is the channel index or -1.
fn pressure(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let mut state = pc;
    loop {
        state = match state {
            0x128920 => {
                c.esp = arithmetic(c, c.esp, 12, true);
                c.eax = frame::slot(c, g, 0x10)?;
                c.ecx = g.u32(INSTANCES)?;
                c.eax = low_word(c, c.eax);
                c.eax = scale(c, c.eax);
                push_all(c, g, &[c.ebx, c.ebp, c.edi])?;
                c.edi = g.u32(at(c.ecx, RECORDS))?;
                c.edx = g.u32(at(at(c.eax, c.edi), 12))?;
                c.ecx = g.u32(TAGS)?;
                c.eax = arithmetic(c, c.eax, c.edi, false);
                c.edx = tag_data_offset(c, c.edx);
                c.edx = g.u32(at(at(c.edx, c.ecx), 8))?;
                frame::set_slot(c, g, 0xc, c.edx)?;
                c.edx = sext8(g.u8(at(c.eax, 0xa1))?);
                c.ecx = at(c.eax, 0x18);
                c.ebp = NONE;
                c.ebx = NONE;
                logic(c, NONE, SIGN);
                return call(listeners::DISTANCE.base, 0x128970);
            }
            0x128970 => {
                c.eax = frame::slot(c, g, 0x20)?;
                c.ecx = g.u32(SOUND_GLOBALS)?;
                c.edi = 0;
                logic(c, 0, SIGN);
                g.set_u32(c.eax, 0)?;
                let count = g.u16(at(c.ecx, 0x208))?;
                alu::cmp16(c, count.into(), c.edi);
                frame::set_slot(c, g, 0x10, sse::lane0(c, 0))?;
                if (count as i16) <= 0 {
                    0x128a4d
                } else {
                    frame::push(c, g, c.esi)?;
                    0x128996
                }
            }
            0x128996 => {
                c.ecx = frame::slot(c, g, 0x10)?;
                c.eax = sext16(c.edi);
                c.edx = c.eax.wrapping_mul(9);
                c.eax = g.u32(CHANNELS)?;
                c.esi = at(c.eax, c.edx.wrapping_mul(4));
                c.edx = byte(c.edx, g.u8(at(c.ecx, 4))?);
                let kind = g.u8(at(c.esi, 8))?;
                cmp_byte(c, c.edx as u8, kind);
                if c.edx as u8 != kind {
                    0x128a20
                } else {
                    c.eax = g.u32(c.esi)?;
                    arithmetic(c, c.eax, NONE, true);
                    if c.eax == NONE {
                        0x128a40
                    } else {
                        c.ecx = frame::slot(c, g, 0x14)?;
                        frame::push(c, g, c.ecx)?;
                        c.ecx = frame::slot(c, g, 0x24)?;
                        return call(COMPARE_POINT.base, 0x1289c9);
                    }
                }
            }
            0x1289c9 => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax as u8 == 0 {
                    0x128a20
                } else {
                    alu::cmp16(c, c.ebp, 0xffff);
                    if c.ebp as u16 == 0xffff {
                        0x1289e5
                    } else {
                        c.edx = frame::slot(c, g, 0x18)?;
                        c.eax = g.u32(c.esi)?;
                        frame::push(c, g, c.edx)?;
                        c.ecx = c.ebx;
                        return call(COMPARE_POINT.base, 0x1289e1);
                    }
                }
            }
            0x1289e1 => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax as u8 == 0 { 0x128a20 } else { 0x1289e5 }
            }
            0x1289e5 => {
                c.eax = g.u32(c.esi)?;
                c.ecx = g.u32(INSTANCES)?;
                c.edx = g.u32(at(c.ecx, RECORDS))?;
                c.eax = low_word(c, c.eax);
                c.eax = scale(c, c.eax);
                c.eax = arithmetic(c, c.eax, c.edx, false);
                c.edx = frame::slot(c, g, 0x24)?;
                g.set_u32(c.edx, 9)?;
                c.edx = sext8(g.u8(at(c.eax, 0xa1))?);
                c.ebx = g.u32(c.esi)?;
                c.ecx = at(c.eax, 0x18);
                c.ebp = c.edi;
                return call(listeners::DISTANCE.base, 0x128a1a);
            }
            0x128a1a => {
                frame::set_slot(c, g, 0x18, sse::lane0(c, 0))?;
                0x128a20
            }
            0x128a20 => {
                c.eax = g.u32(SOUND_GLOBALS)?;
                c.edi = increment(c, c.edi, 32);
                let count = g.u16(at(c.eax, 0x208))?;
                alu::cmp16(c, c.edi, count.into());
                if (c.edi as i16) < count as i16 {
                    0x128996
                } else {
                    c.esi = frame::pop(c, g)?;
                    c.edi = frame::pop(c, g)?;
                    c.eax = lo16(c.eax, c.ebp as u16);
                    return leave_pressure(c, g);
                }
            }
            0x128a40 => {
                c.esi = frame::pop(c, g)?;
                c.eax = lo16(c.eax, c.edi as u16);
                c.edi = frame::pop(c, g)?;
                return leave_pressure(c, g);
            }
            0x128a4d => {
                c.edi = frame::pop(c, g)?;
                c.eax = lo16(c.eax, c.ebp as u16);
                return leave_pressure(c, g);
            }
            _ => return Err(invalid_site(pc)),
        };
    }
}

/// Pick the best owner in the supplied list to evict: older than its
/// class's minimum age and older than the best so far, or outranked by
/// layer. Returns -1 in `eax` for an empty list.
fn capped(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != 0x128a60 {
        return Err(invalid_site(pc));
    }
    c.esp = arithmetic(c, c.esp, 0x18, true);
    c.edx = g.u32(INSTANCES)?;
    c.eax = low_word(c, c.eax);
    c.eax = scale(c, c.eax);
    frame::push(c, g, c.ebp)?;
    c.ebp = g.u32(at(c.edx, RECORDS))?;
    c.edx = g.u32(TAGS)?;
    c.eax = arithmetic(c, c.eax, c.ebp, false);
    frame::set_slot(c, g, 0x18, c.eax)?;
    c.eax = g.u32(at(c.eax, 12))?;
    c.eax = tag_data_offset(c, c.eax);
    c.edx = g.u32(at(at(c.eax, c.edx), 8))?;
    c.eax = NONE;
    logic(c, NONE, SIGN);
    alu::test16(c, c.ecx);
    frame::set_slot(c, g, 4, 0)?;
    frame::set_slot(c, g, 8, c.eax)?;
    if (c.ecx as i16) > 0 {
        c.eax = g.u32(SOUND_GLOBALS)?;
        c.eax = g.u32(at(c.eax, 0x84))?;
        frame::set_slot(c, g, 0xc, c.eax)?;
        c.eax = g.u32(GLOBALS_TAG)?;
        c.eax = g.u32(at(c.eax, 0xc0))?;
        frame::set_slot(c, g, 0x10, c.eax)?;
        c.eax = sext8(g.u8(at(c.edx, 2))?);
        c.eax = imul(c, c.eax, 0x5c);
        push_all(c, g, &[c.ebx, c.esi])?;
        c.ecx &= 0xffff;
        frame::push(c, g, c.edi)?;
        c.edi = frame::slot(c, g, 0x2c)?;
        frame::set_slot(c, g, 0x20, c.eax)?;
        frame::set_slot(c, g, 0x2c, c.ecx)?;
        loop {
            c.esi = lo16(c.esi, g.u16(c.edi)?);
            c.eax = g.u32(CHANNELS)?;
            c.ecx = sext16(c.esi);
            c.edx = c.ecx.wrapping_mul(9);
            c.ecx = g.u32(at(c.eax, c.edx.wrapping_mul(4)))?;
            c.edx = frame::slot(c, g, 0x18)?;
            c.eax = frame::slot(c, g, 0x1c)?;
            c.ecx = low_word(c, c.ecx);
            c.ecx = scale(c, c.ecx);
            c.ebx = g.u32(at(at(c.ecx, c.ebp), 0x8c))?;
            c.ecx = arithmetic(c, c.ecx, c.ebp, false);
            c.edx = arithmetic(c, c.edx, c.ebx, true);
            logic(c, c.eax, SIGN);
            c.eax = if c.eax == 0 {
                logic(c, 0, SIGN);
                0
            } else {
                let globals = g.u32(GLOBALS_TAG)?;
                g.u32(at(globals, 0xc4))?
            };
            c.eax = g.u32(at(c.eax, 4))?;
            c.ebx = g.u32(TAGS)?;
            c.eax = tag_data_offset(c, c.eax);
            c.eax = g.u32(at(at(c.eax, c.ebx), 8))?;
            c.eax = g.u32(at(c.eax, 4))?;
            c.ebx = frame::slot(c, g, 0x20)?;
            let minimum = g.u32(at(at(c.eax, c.ebx), 4))?;
            arithmetic(c, c.edx, minimum, true);
            let best = frame::slot(c, g, 0x10)?;
            let age = c.edx as i32;
            let evict_by_age = if age < minimum as i32 {
                false
            } else {
                arithmetic(c, c.edx, best, true);
                CappedSelection { channel: None, age: best as i32 }.old_enough(age, minimum as i32)
            };
            let evict = evict_by_age || {
                c.eax = frame::slot(c, g, 0x24)?;
                let (ours, theirs) = (g.u8(at(c.eax, 0xa0))?, g.u8(at(c.ecx, 0xa0))?);
                c.eax = byte(c.eax, ours);
                cmp_byte(c, ours, theirs);
                ours as i8 > theirs as i8
            };
            if evict {
                frame::set_slot(c, g, 0x14, c.esi)?;
                frame::set_slot(c, g, 0x10, c.edx)?;
            }
            c.eax = frame::slot(c, g, 0x2c)?;
            c.edi = arithmetic(c, c.edi, 2, false);
            c.eax = alu::dec32(c, c.eax);
            frame::set_slot(c, g, 0x2c, c.eax)?;
            if c.eax == 0 {
                break;
            }
        }
        c.eax = lo16(c.eax, frame::slot(c, g, 0x14)? as u16);
        c.edi = frame::pop(c, g)?;
        c.esi = frame::pop(c, g)?;
        c.ebx = frame::pop(c, g)?;
    }
    c.ebp = frame::pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x18, false);
    Ok(Next::Ret { pop: 4 })
}
