//! Game sound update and the announcer's delayed impulse queue. The frame
//! retains a slot index across calls, so each continuation sees allocation,
//! deletion and changes to the next sound made by the preceding callback.
use crate::datum::layout;
use crate::points::{self, Loc, Point, Sig, announcer_queue as queue};
use crate::sim_objects::{Body, Cx, Next, Result};
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, status};
use runtime::{CpuAbi, Guest, GuestFault};
use sim_sound::scheduling::{self, HeadAction};

const DOC: &str = "docs/engine/sim-rewrite.md";
pub const GAME_SOUND_UPDATE: Point = Point::new(
    "GAME_SOUND_UPDATE",
    0x18c060,
    Sig::Fn {
        args: &[("step", Loc::Stack(0))],
        ret: None,
        pop: 4,
    },
    DOC,
);
pub const ANNOUNCER_COMPLETE: Point = Point::new(
    "ANNOUNCER_COMPLETE",
    0x23f120,
    Sig::Fn {
        args: &[("voice", Loc::Stack(1))],
        ret: None,
        pop: 12,
    },
    DOC,
);
pub const ANNOUNCER_ENQUEUE: Point = Point::new(
    "ANNOUNCER_ENQUEUE",
    0x23f140,
    Sig::Fn {
        args: &[("sound", Loc::Ecx), ("delay", Loc::Stack(0))],
        ret: None,
        pop: 4,
    },
    DOC,
);
const FRAME_SITES: &[u32] = &[
    0x18c084, 0x18c08d, 0x18c097, 0x18c09d, 0x18c0a2, 0x18c0d5, 0x18c12a, 0x18c18b, 0x18c1a6,
    0x18c1b3, 0x18c1c0, 0x18c1e5, 0x18c1f4, 0x18c207, 0x18c232,
];
pub const BODIES: &[Body] = &[
    Body {
        point: &GAME_SOUND_UPDATE,
        sites: FRAME_SITES,
        run: update,
    },
    Body {
        point: &points::ANNOUNCER_QUEUE_UPDATE,
        sites: &[0x23f044],
        run: announcer,
    },
    Body {
        point: &ANNOUNCER_COMPLETE,
        sites: &[],
        run: complete,
    },
    Body {
        point: &ANNOUNCER_ENQUEUE,
        sites: &[],
        run: enqueue,
    },
];

const GAME_SOUND_ARRAY: u32 = 0x004e_d28c;
const GAME_SOUND_STATE: u32 = 0x004e_d288;
const OVERRIDE: u32 = 0x0051_0c50;
const AUDIBLE_CLUSTERS: u32 = 0x0054_e8a0;
const SOURCE_STEP: u32 = 0x0018_a7b0;
const PLAY_IMPULSE: u32 = 0x0018_9710;
const UNITY_GAIN: u32 = 0x0045_dbc0;

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}
fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

fn invalid(pc: u32) -> GuestFault {
    GuestFault {
        addr: pc,
        len: 1,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

fn compare_small(c: &mut CpuAbi, lhs: u32, rhs: u32, sign: u32) {
    let mask = sign * 2 - 1;
    let diff = lhs.wrapping_sub(rhs) & mask;
    let overflow = (lhs ^ rhs) & (lhs ^ diff) & sign != 0;
    c.eflags = (c.eflags & !ARITH)
        | status(diff, sign)
        | u32::from(lhs < rhs)
        | ((lhs ^ rhs ^ diff) & 0x10)
        | (u32::from(overflow) << 11);
    c.eflags_defined |= ARITH;
}

/// INC/DEC leave the carry bit and its definition alone.
fn count(c: &mut CpuAbi, value: u32, down: bool) -> u32 {
    let carry = (c.eflags & 1, c.eflags_defined & 1);
    let result = arithmetic(c, value, 1, down);
    c.eflags = (c.eflags & !1) | carry.0;
    c.eflags_defined = (c.eflags_defined & !1) | carry.1;
    result
}

/// Scan one occupied bitmap word at a time. The final mask and word index
/// are also the callee-saved EDI and scratch ECX visible to the mixer.
fn next_source(c: &mut CpuAbi, g: &Guest) -> Result<bool> {
    let previous = g.u32(at(c.esp, 0x24))?;
    c.eax = count(c, previous, false);
    if (c.eax as i32) < 0 {
        return Ok(false);
    }
    c.esi = g.u32(at(c.ebp, layout::HIGH_WATER))?;
    arithmetic(c, c.eax, c.esi, true);
    if (c.eax as i32) >= c.esi as i32 {
        return Ok(false);
    }
    c.edx = g.u32(at(c.ebp, layout::BITMAP))?;
    loop {
        c.ecx = c.eax / 32;
        let bits = g.u32(at(c.edx, c.ecx * 4))?;
        if let Some(slot) = scheduling::next_in_word(bits, c.eax, c.esi) {
            c.eax = slot;
            c.edi = 1 << (slot % 32);
            return Ok(true);
        }
        let next = ((c.eax & !31) + 32).min(c.esi);
        c.edi = 1 << ((next - 1) % 32);
        c.eax = next;
        arithmetic(c, c.eax, c.esi, true);
        if next == c.esi {
            return Ok(false);
        }
    }
}

fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let mut state = pc;
    loop {
        state = match state {
            0x18c060 => {
                c.eax = g.u32(points::SOUND_GLOBALS.base)?;
                c.esp = c.esp.wrapping_sub(24);
                for field in [0x78, 0x79, 0x7a] {
                    let enabled = g.u8(at(c.eax, field))?;
                    c.ecx = byte(c.ecx, enabled);
                    logic(c, enabled.into(), 0x80);
                    if enabled == 0 {
                        break;
                    }
                }
                if c.ecx as u8 != 0 {
                    push(c, g, 0)?;
                    return call(points::SOUND_LISTENERS_UPDATE.base, 0x18c084);
                }
                0x18c084
            }
            0x18c084 => {
                for value in [c.ebx, c.ebp, c.esi, c.edi] {
                    push(c, g, value)?;
                }
                return call(0x18cb10, 0x18c08d);
            }
            0x18c08d => {
                c.esi = g.u32(at(c.esp, 0x2c))?;
                push(c, g, c.esi)?;
                return call(0x18bb80, 0x18c097);
            }
            0x18c097 => {
                push(c, g, c.esi)?;
                return call(0x225b60, 0x18c09d);
            }
            0x18c09d => return call(0x18bf90, 0x18c0a2),
            0x18c0a2 => {
                c.eax = g.u32(OVERRIDE)?;
                c.ecx = u32::MAX;
                if c.eax != 0 {
                    c.edx = byte(c.edx, g.u8(at(c.eax, 5))?);
                }
                if c.eax != 0 && c.edx as u8 != 0 {
                    c.ecx = 0;
                    logic(c, 0, 1 << 31);
                } else {
                    c.eax = g.u32(GAME_SOUND_STATE)?;
                    c.edx = g.u32(at(c.eax, 0x24))?;
                    c.eax = g.u32(points::GAME_TIME.base)?;
                    let tick = g.u32(at(c.eax, 8))?;
                    arithmetic(c, c.edx, tick, true);
                    if c.edx as i32 >= tick as i32 {
                        c.ecx = 1;
                    }
                }
                return call(0x1272c0, 0x18c0d5);
            }
            0x18c0d5 => {
                c.ebx = g.u32(GAME_SOUND_ARRAY)?;
                c.ebp = c.ebx;
                g.set_u32(at(c.esp, 0x1c), c.ebp)?;
                g.set_u32(at(c.esp, 0x24), u32::MAX)?;
                0x18c0f0
            }
            0x18c0eb => {
                c.ebp = g.u32(at(c.esp, 0x1c))?;
                0x18c0f0
            }
            0x18c0f0 => {
                if !next_source(c, g)? {
                    c.eax = g.u32(points::SOUND_GLOBALS.base)?;
                    g.set_u8(at(c.eax, 0x7e), 1)?;
                    return call(0x21d390, 0x18c12a);
                }
                c.esi = g.u32(at(c.ebp, layout::ELEMENT_SIZE))?;
                c.edi = g.u32(at(c.ebp, layout::DATA))?;
                c.esi = at(c.edi, c.esi.wrapping_mul(c.eax));
                c.edi = (u32::from(g.u16(c.esi)?) << 16) | c.eax;
                g.set_u32(at(c.esp, 0x24), c.eax)?;
                if g.u8(at(c.esi, 3))? == 4 {
                    c.eax = g.u32(at(c.esi, 0x10))?;
                    if c.eax != u32::MAX {
                        c.edx = g.u32(points::GAME_TIME.base)?;
                        if scheduling::due(c.eax as i32, g.u32(at(c.edx, 8))? as i32) {
                            c.ecx = g.u32(at(c.ebx, layout::DATA))?;
                            c.eax = at(c.ecx, 24 * (c.edi & 0xffff) + 4);
                            g.set_u8(c.eax, g.u8(c.eax)? | 2)?;
                        }
                    }
                }
                let kind = g.u8(at(c.esi, 3))?;
                compare_small(c, kind.into(), 1, 0x80);
                if kind != 1 {
                    push(c, g, c.edi)?;
                    return call(SOURCE_STEP, 0x18c18b);
                }
                let attached = g.u8(at(c.esi, 4))? & 1;
                logic(c, attached.into(), 0x80);
                if attached != 0 {
                    c.eax = g.u32(at(c.esi, 0x10))?;
                    push(c, g, u32::MAX)?;
                    return call(0xbadc0, 0x18c1a6);
                }
                0x18c1ea
            }
            0x18c18b => {
                c.ebx = g.u32(GAME_SOUND_ARRAY)?;
                0x18c0f0
            }
            0x18c1a6 => {
                logic(c, c.eax, 1 << 31);
                if c.eax == 0 {
                    c.edx = g.u32(at(c.esi, 0xc))?;
                    push(c, g, c.edx)?;
                    return call(0x18d1c0, 0x18c1b3);
                }
                0x18c1ea
            }
            0x18c1b3 => {
                arithmetic(c, c.eax, c.edi, true);
                if c.eax == c.edi {
                    c.esi = g.u32(at(c.esi, 0xc))?;
                    push(c, g, c.esi)?;
                    return call(0x18d1c0, 0x18c1c0);
                }
                0x18c1dc
            }
            0x18c1c0 => {
                arithmetic(c, c.eax, u32::MAX, true);
                if c.eax != u32::MAX {
                    c.ecx = g.u32(at(c.ebx, layout::DATA))?;
                    c.eax = at(c.ecx, 24 * (c.eax & 0xffff));
                    let tag = g.u32(at(c.eax, 0xc))?;
                    arithmetic(c, tag, c.esi, true);
                    if tag == c.esi {
                        let flags = g.u8(at(c.eax, 4))? & !0x20;
                        g.set_u8(at(c.eax, 4), flags)?;
                        logic(c, flags.into(), 0x80);
                    }
                }
                0x18c1dc
            }
            0x18c1dc => {
                c.eax = c.edi;
                c.edx = c.ebx;
                return call(0x16ba40, 0x18c1e5);
            }
            0x18c1e5 => 0x18c0f0,
            0x18c1ea => {
                c.ebp = g.u32(at(c.esi, 0x10))?;
                c.eax = c.ebp;
                return call(0xb9d20, 0x18c1f4);
            }
            0x18c1f4 => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax as u8 != 0 {
                    c.eax = at(c.esp, 0x10);
                    c.ecx = c.ebp;
                    return call(0xba300, 0x18c207);
                }
                0x18c0eb
            }
            0x18c207 => {
                c.eax = g.u32(at(c.esp, 0x14))?;
                let cluster = c.eax as i16;
                if cluster != -1 {
                    c.ecx = cluster as u32 & 31;
                    c.edx = 1 << c.ecx;
                    c.eax = (i32::from(cluster) >> 5) as u32;
                    let set = g.u32(at(AUDIBLE_CLUSTERS, c.eax.wrapping_mul(4)))? & c.edx;
                    logic(c, set, 1 << 31);
                    if set != 0 {
                        push(c, g, c.edi)?;
                        return call(SOURCE_STEP, 0x18c232);
                    }
                }
                let flags = g.u8(at(c.esi, 5))? & !1;
                g.set_u8(at(c.esi, 5), flags)?;
                logic(c, flags.into(), 0x80);
                0x18c0eb
            }
            0x18c232 => {
                c.ebx = g.u32(GAME_SOUND_ARRAY)?;
                0x18c0eb
            }
            0x18c12a => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 24, false);
                return Ok(Next::Ret { pop: 4 });
            }
            _ => return Err(invalid(pc)),
        }
    }
}

fn announcer(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::ANNOUNCER_QUEUE_UPDATE.base {
        c.edx = g.u32(queue::COUNT)?;
        logic(c, c.edx, 1 << 31);
        if c.edx == 0 {
            return Ok(Next::Ret { pop: 0 });
        }
        let delay = g.u16(queue::HEAD_DELAY)?;
        c.eax = (c.eax & !0xffff) | u32::from(delay);
        logic(c, delay.into(), 0x8000);
        let playing = if (delay as i16) < 0 {
            let value = g.u8(queue::HEAD_PLAYING)?;
            c.eax = byte(c.eax, value);
            logic(c, value.into(), 0x80);
            value != 0
        } else {
            false
        };
        match scheduling::head_action(delay as i16, playing) {
            HeadAction::Wait => return Ok(Next::Ret { pop: 0 }),
            HeadAction::CountDown(next) => {
                let next = next as u16;
                c.eax = (c.eax & !0xffff) | u32::from(next);
                g.set_u16(queue::HEAD_DELAY, next)?;
                compare_small(c, next.into(), 0xffff, 0x8000);
                return Ok(Next::Ret { pop: 0 });
            }
            HeadAction::Start => {
                g.set_u16(queue::HEAD_DELAY, u16::MAX)?;
                c.ecx = g.u32(queue::QUEUE)?;
                c.eax = u32::MAX;
                arithmetic(c, c.ecx, c.eax, true);
                if c.ecx != u32::MAX {
                    c.xmm[0] = [0; 16];
                    c.xmm[0][..4].copy_from_slice(&g.u32(UNITY_GAIN)?.to_le_bytes());
                    push(c, g, c.ecx)?;
                    return call(PLAY_IMPULSE, 0x23f044);
                }
            }
            HeadAction::Remove => {
                c.ecx = 1;
                arithmetic(c, c.edx, 1, true);
                if c.edx as i32 > 1 {
                    push(c, g, c.esi)?;
                    c.eax = queue::QUEUE + 12;
                    push(c, g, c.edi)?;
                    loop {
                        let dest = c.eax.wrapping_sub(12);
                        let tag = g.u32(c.eax)?;
                        g.set_u32(dest, tag)?;
                        let voice = g.u32(at(c.eax, 4))?;
                        let timing = g.u32(at(c.eax, 8))?;
                        g.set_u32(at(dest, 4), voice)?;
                        g.set_u32(at(dest, 8), timing)?;
                        c.edx = g.u32(queue::COUNT)?;
                        c.ecx = c.ecx.wrapping_add(1);
                        c.eax = at(c.eax, 12);
                        arithmetic(c, c.ecx, c.edx, true);
                        if (c.ecx as i32) >= c.edx as i32 {
                            break;
                        }
                    }
                    c.edi = pop(c, g)?;
                    c.esi = pop(c, g)?;
                }
                c.edx = count(c, c.edx, true);
                g.set_u32(queue::COUNT, c.edx)?;
                return Ok(Next::Ret { pop: 0 });
            }
        }
    } else if pc != 0x23f044 {
        return Err(invalid(pc));
    }
    arithmetic(c, c.eax, u32::MAX, true);
    g.set_u32(queue::QUEUE + 4, c.eax)?;
    if c.eax != u32::MAX {
        g.set_u8(queue::HEAD_PLAYING, 1)?;
    }
    Ok(Next::Ret { pop: 0 })
}

fn complete(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.eax = g.u32(queue::QUEUE + 4)?;
    let ended = g.u32(at(c.esp, 8))?;
    arithmetic(c, c.eax, ended, true);
    if c.eax == ended {
        g.set_u8(queue::HEAD_PLAYING, 0)?;
    }
    Ok(Next::Ret { pop: 12 })
}

fn enqueue(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.eax = g.u32(queue::COUNT)?;
    arithmetic(c, c.eax, 5, true);
    if (c.eax as i32) < 5 {
        let offset = c.eax.wrapping_mul(12);
        g.set_u32(at(queue::QUEUE, offset), c.ecx)?;
        c.eax = g.u32(queue::COUNT)?;
        c.edx = c.eax.wrapping_mul(3);
        let delay = g.u16(at(c.esp, 4))?;
        c.eax = (c.eax & !0xffff) | u32::from(delay);
        g.set_u16(at(queue::HEAD_DELAY, c.edx.wrapping_mul(4)), delay)?;
        let length = count(c, g.u32(queue::COUNT)?, false);
        g.set_u32(queue::COUNT, length)?;
    }
    Ok(Next::Ret { pop: 4 })
}
