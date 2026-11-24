//! Guest channel visits and source selection in the sound frame. A guest
//! callback can free its source or change the channel count; each visit
//! reads the next channel from the current pool after the callback.

use crate::points::{self, Loc, Point, Sig};
use crate::sim_objects::{Body, Cx, Next, Result, alu, frame, sse};
use crate::sim_physics::{arithmetic, byte, logic};
use guest_layout::sound::{LoopingSound, SoundChannel, SoundInstance};
use runtime::{CpuAbi, Guest, GuestFault};
use sim_sound::sources::{FadeWindow, release_loop_ref, uses_specialized_channel};

mod completion;
mod instances;
mod listeners;

const DOC: &str = "docs/engine/sim-rewrite.md";
const NO_ARGS: Sig = Sig::Fn {
    args: &[],
    ret: None,
    pop: 0,
};
pub static CHANNEL_STREAMS: Point = Point::new("SOUND_CHANNEL_STREAMS", 0x1290d0, NO_ARGS, DOC);
pub static CHANNEL_EFFECTS: Point = Point::new(
    "SOUND_CHANNEL_EFFECTS",
    0x129380,
    Sig::Fn {
        args: &[("submixes", Loc::Ebx), ("streams", Loc::Edi)],
        ret: None,
        pop: 0,
    },
    DOC,
);
pub static CHANNEL_SOURCES: Point = Point::new("SOUND_CHANNEL_SOURCES", 0x1293f0, NO_ARGS, DOC);
pub static RELEASE_LOOP: Point = Point::new(
    "SOUND_LOOP_RELEASE",
    0x21b070,
    Sig::Fn {
        args: &[("loop", Loc::Edi)],
        ret: None,
        pop: 0,
    },
    DOC,
);

pub const BODIES: &[Body] = &[
    completion::FINISH,
    completion::FREE,
    instances::SOURCE_WALK,
    instances::CHANNEL_ASSIGNMENT,
    Body {
        point: &CHANNEL_STREAMS,
        sites: &[0x1290e8],
        run: streams,
    },
    Body {
        point: &CHANNEL_EFFECTS,
        sites: &[0x1293d7],
        run: effects,
    },
    Body {
        point: &CHANNEL_SOURCES,
        sites: &[
            0x129408, 0x129412, 0x129465, 0x1294be, 0x129528, 0x12956f, 0x129581, 0x1295b8,
        ],
        run: update,
    },
    Body {
        point: &RELEASE_LOOP,
        sites: &[0x21b0c7],
        run: release,
    },
    Body {
        point: &listeners::SELECT,
        sites: &[0x127d6f, 0x127da3],
        run: listeners::select,
    },
    Body {
        point: &listeners::DISTANCE,
        sites: &[],
        run: listeners::distance,
    },
];

fn invalid_site(pc: u32) -> GuestFault {
    GuestFault {
        addr: pc,
        len: 1,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

fn sw(value: u16) -> u32 {
    i32::from(value as i16) as u32
}

fn lo16(register: u32, value: u16) -> u32 {
    (register & 0xffff_0000) | u32::from(value)
}

const CHANNEL_COUNT: u32 = 0x208;
const RECORDS: u32 = 0x44;
const GAIN_FLOOR: u32 = 0x44a0b4;
const LOOP_COMPANIONS: u32 = 0x51ebd8;
const DATUM_DELETE: u32 = 0x16ba40;

fn streams(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let more = match pc {
        0x1290d0 => {
            c.eax = g.u32(points::SOUND_GLOBALS.base)?;
            frame::push(c, g, c.esi)?;
            c.esi = 0;
            let count = g.u16(c.eax + CHANNEL_COUNT)?;
            alu::cmp16(c, u32::from(count), 0);
            (count as i16) > 0
        }
        0x1290e8 => {
            c.ecx = g.u32(points::SOUND_GLOBALS.base)?;
            c.esi = c.esi.wrapping_add(1);
            let count = g.u16(c.ecx + CHANNEL_COUNT)?;
            alu::cmp16(c, c.esi, u32::from(count));
            (c.esi as i16) < count as i16
        }
        _ => return Err(invalid_site(pc)),
    };
    if more {
        c.eax = c.esi;
        return call(0x12a450, 0x1290e8);
    }
    c.esi = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}

fn effects(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let mut visit = match pc {
        0x129380 => {
            c.eax = g.u32(points::SOUND_GLOBALS.base)?;
            frame::push(c, g, c.esi)?;
            c.esi = 0;
            let count = g.u16(c.eax + CHANNEL_COUNT)?;
            alu::cmp16(c, u32::from(count), 0);
            (count as i16) > 0
        }
        0x1293d7 => next_effect(c, g)?,
        _ => return Err(invalid_site(pc)),
    };
    while visit {
        c.edx = g.u32(points::SOUND_CHANNELS.base)?;
        let index = sw(c.esi as u16);
        c.ecx = index.wrapping_mul(9);
        c.eax = c.edx.wrapping_add(index.wrapping_mul(SoundChannel::SIZE));
        if g.u32(c.eax)? != u32::MAX {
            c.ecx = byte(c.ecx, g.u8(c.eax + 9)?);
            if c.ecx as u8 == 0 {
                let submix = g.u16(c.eax + 0xe)?;
                c.ecx = lo16(c.ecx, submix);
                alu::cmp16(c, c.ecx, u16::MAX.into());
                let argument = if submix != u16::MAX {
                    c.eax = sw(submix);
                    c.ecx = g.u32(c.ebx.wrapping_add(c.eax.wrapping_mul(4)))?;
                    Some(c.ecx)
                } else {
                    let stream = g.u16(c.eax + 0xc)?;
                    c.eax = lo16(c.eax, stream);
                    alu::cmp16(c, c.eax, u16::MAX.into());
                    if stream != u16::MAX {
                        c.edx = sw(stream);
                        c.eax = g.u32(c.edi.wrapping_add(c.edx.wrapping_mul(4)))?;
                        Some(c.eax)
                    } else {
                        None
                    }
                };
                if let Some(data) = argument {
                    frame::push(c, g, data)?;
                    c.eax = c.esi;
                    return call(0x129790, 0x1293d7);
                }
            }
        }
        visit = next_effect(c, g)?;
    }
    c.esi = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}

fn next_effect(c: &mut CpuAbi, g: &Guest) -> Result<bool> {
    c.ecx = g.u32(points::SOUND_GLOBALS.base)?;
    c.edx = sw(g.u16(c.ecx + CHANNEL_COUNT)?);
    c.esi = c.esi.wrapping_add(1);
    arithmetic(c, c.esi, c.edx, true);
    Ok((c.esi as i32) < c.edx as i32)
}

fn release(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == RELEASE_LOOP.base {
        c.eax = (c.edi & 0xffff) * LoopingSound::SIZE;
        frame::push(c, g, c.ebp)?;
        c.ebp = g.u32(points::LOOPING_SOUNDS.base)?;
        c.ecx = g.u32(c.ebp + RECORDS)?;
        frame::push(c, g, c.esi)?;
        c.esi = g.u32(c.ecx.wrapping_add(c.eax) + 0x54)?;
        arithmetic(c, c.esi, u32::MAX, true);
        if c.esi != u32::MAX {
            c.edx = g.u32(LOOP_COMPANIONS)?;
            frame::push(c, g, c.ebx)?;
            c.ebx = g.u32(c.edx + RECORDS)?;
            c.ecx = c.ebx.wrapping_add((c.esi & 0xffff) * 0x1c);
            let old = g.u8(c.ecx + 2)?;
            c.eax = byte(c.eax, old);
            let remaining = release_loop_ref(old);
            g.set_u8(c.ecx + 2, remaining)?;
            logic(c, u32::from(remaining & 0x7f), 0x80);
            c.ebx = frame::pop(c, g)?;
            if remaining & 0x7f == 0 {
                c.eax = c.esi;
                return call(DATUM_DELETE, 0x21b0c7);
            }
        }
    } else if pc != 0x21b0c7 {
        return Err(invalid_site(pc));
    }
    c.esi = frame::pop(c, g)?;
    c.edx = c.ebp;
    c.eax = c.edi;
    c.ebp = frame::pop(c, g)?;
    Ok(Next::Tail {
        target: DATUM_DELETE,
    })
}

/// Update each occupied channel. A retiring source may be reused by the
/// callback, so the channel owner and instance pool are fetched again at
/// the same boundaries as the guest.
fn update(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    loop {
        match pc {
            0x1293f0 => {
                c.esp = arithmetic(c, c.esp, 0x44, true);
                frame::push(c, g, c.ebp)?;
                frame::push(c, g, c.esi)?;
                c.esi = g.u32(points::SOUND_GLOBALS.base)?;
                sse::load(c, 0, g.u32(c.esi + 0x1e4)?);
                return call(0x2197f0, 0x129408);
            }
            0x129408 => {
                frame::push(c, g, c.esi)?;
                frame::set_slot(c, g, 0x14, c.eax)?;
                return call(0x21f360, 0x129412);
            }
            0x129412 => {
                c.ebp = 0;
                let count = g.u16(c.esi + CHANNEL_COUNT)?;
                alu::cmp16(c, u32::from(count), 0);
                if count as i16 <= 0 {
                    pc = 0x1295ce;
                    continue;
                }
                frame::push(c, g, c.edi)?;
                pc = 0x129422;
            }
            0x129422 => {
                c.ecx = g.u32(points::SOUND_CHANNELS.base)?;
                c.eax = sw(c.ebp as u16).wrapping_mul(9);
                c.edi = c.ecx.wrapping_add(c.eax.wrapping_mul(4));
                c.eax = g.u32(c.edi)?;
                if c.eax != u32::MAX {
                    c.ecx = byte(c.ecx, g.u8(c.edi + 9)?);
                    if c.ecx as u8 == 0 {
                        c.edx = g.u32(points::SOUND_INSTANCES.base)?;
                        c.ecx = g.u32(c.edx + RECORDS)?;
                        let offset = (c.eax & 0xffff) * SoundInstance::SIZE;
                        c.esi = arithmetic(c, offset, c.ecx, false);
                        return call(0x12a810, 0x129465);
                    }
                }
                pc = 0x1295b8;
            }
            0x129465 => {
                sse::load(c, 0, g.u32(c.esi + 0x20)?);
                let floor = g.u32(GAIN_FLOOR)?;
                sse::load(c, 1, floor);
                frame::set_slot(c, g, 0x10, c.eax)?;
                let total = sse::ss(c, sse::Ss::Add, 0, c.eax);
                let quiet = sse::comiss(c, floor, total) & 1 == 0;
                frame::set_slot(c, g, 0x0c, c.eax)?;
                if quiet {
                    c.eax = g.u32(c.edi)?;
                    c.edx = g.u32(points::SOUND_INSTANCES.base)?;
                    c.ecx = ((c.eax & 0xffff) * SoundInstance::SIZE)
                        .wrapping_add(g.u32(c.edx + RECORDS)?);
                    c.edx = byte(c.edx, g.u8(c.ecx + 4)? >> 7);
                    if c.edx as u8 != 0 {
                        c.edx = g.u32(c.ecx + 0xb4)?;
                        let end = g.u32(c.ecx + 0xb8)?;
                        arithmetic(c, c.edx, end, true);
                        if (FadeWindow {
                            start: c.edx as i32,
                            end: end as i32,
                        })
                        .finished_fade_out(quiet)
                        {
                            frame::push(c, g, 4)?;
                            return call(points::SOUND_FINISH_INSTANCE.base, 0x1294be);
                        }
                    }
                }
                c.eax = g.u32(c.edi)?;
                c.ecx = g.u32(points::SOUND_INSTANCES.base)?;
                c.edx = g.u32(c.ecx + RECORDS)?;
                c.eax = ((c.eax & 0xffff) * SoundInstance::SIZE).wrapping_add(c.edx);
                let flags = g.u8(c.eax + 4)?;
                c.edx = byte(c.edx, flags >> 7);
                if c.edx as u8 != 0 {
                    c.ecx = g.u32(c.eax + 0xb8)?;
                    let fade = FadeWindow {
                        start: g.u32(c.eax + 0xb4)? as i32,
                        end: c.ecx as i32,
                    };
                    if fade.start < fade.end {
                        c.edx = g.u32(points::SOUND_GLOBALS.base)?;
                        if fade.finished_fade_in(g.u32(c.edx + 0x84)? as i32) {
                            g.set_u8(c.eax + 4, flags & 0x7f)?;
                        }
                    }
                }
                let stream = g.u16(c.edi + 0xc)?;
                alu::cmp16(c, stream.into(), u16::MAX.into());
                if stream == u16::MAX {
                    pc = 0x1295b8;
                } else {
                    c.eax = c.esp.wrapping_add(0x18);
                    frame::push(c, g, c.eax)?;
                    c.ecx = c.esp.wrapping_add(0x10);
                    frame::push(c, g, c.ecx)?;
                    c.eax = c.ebp;
                    return call(0x129aa0, 0x129528);
                }
            }
            0x1294be => {
                g.set_u32(c.edi, u32::MAX)?;
                pc = 0x1295b8;
            }
            0x129528 => {
                c.edx = g.u32(c.edi)?;
                c.eax = g.u32(points::SOUND_INSTANCES.base)?;
                c.ecx = g.u32(c.eax + RECORDS)?;
                sse::load(c, 0, frame::slot(c, g, 0x14)?);
                let gain = sse::ss(c, sse::Ss::Add, 0, frame::slot(c, g, 0xc)?);
                c.edx = (c.edx & 0xffff) * SoundInstance::SIZE;
                let kind = g.u8(c.ecx.wrapping_add(c.edx) + 2)?;
                c.ecx = byte(c.ecx, kind);
                c.eax = 1u32.wrapping_shl(kind.into());
                frame::set_slot(c, g, 0xc, gain)?;
                logic(c, c.eax & 0x1e, 0x80);
                let params = c.esp.wrapping_add(0x18);
                let (callee, site) = if uses_specialized_channel(kind) {
                    c.edx = gain;
                    c.ecx = params;
                    (0x21c400, 0x12956f)
                } else {
                    c.ecx = gain;
                    c.eax = params;
                    (0x128ca0, 0x129581)
                };
                frame::push(c, g, params)?;
                frame::push(c, g, gain)?;
                frame::push(c, g, c.ebp)?;
                return call(callee, site);
            }
            0x12956f | 0x129581 => {
                c.edx = byte(c.edx, g.u8(c.esi + 4)? >> 5);
                if c.edx & 1 == 0 {
                    c.eax = g.u32(c.esi + 0x14)?;
                    if c.eax != 0 {
                        c.eax = g.u32(c.eax + 8)?;
                        logic(c, c.eax, 1 << 31);
                        if c.eax != 0 {
                            c.ecx = g.u32(c.edi + 0x10)?;
                            c.edx = g.u32(c.edi + 0x1c)?;
                            frame::push(c, g, c.ecx)?;
                            c.ecx = g.u8(c.esi + 0x9c)? as i8 as i32 as u32;
                            frame::push(c, g, c.edx)?;
                            c.edx = g.u32(c.esi + 0xc)?;
                            frame::push(c, g, c.ecx)?;
                            frame::push(c, g, c.edx)?;
                            c.edx = g.u32(c.esi + 0x10)?;
                            c.ecx = c.esi.wrapping_add(0x5c);
                            frame::push(c, g, c.ecx)?;
                            frame::push(c, g, c.edx)?;
                            return call(c.eax, 0x1295b8);
                        }
                    }
                }
                pc = 0x1295b8;
            }
            0x1295b8 => {
                c.eax = g.u32(points::SOUND_GLOBALS.base)?;
                c.ecx = sw(g.u16(c.eax + CHANNEL_COUNT)?);
                c.ebp = c.ebp.wrapping_add(1);
                arithmetic(c, c.ebp, c.ecx, true);
                if (c.ebp as i32) < c.ecx as i32 {
                    pc = 0x129422;
                } else {
                    c.edi = frame::pop(c, g)?;
                    pc = 0x1295ce;
                }
            }
            0x1295ce => {
                c.esi = frame::pop(c, g)?;
                c.ebp = frame::pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x44, false);
                return Ok(Next::Ret { pop: 0 });
            }
            _ => return Err(invalid_site(pc)),
        }
    }
}
