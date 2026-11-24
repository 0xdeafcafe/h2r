//! Two consecutive passes over sound instances. The first evaluates live
//! sources against their listeners; the second admits due instances to
//! channels. A callback can delete or add a later instance.
use super::*;
use sim_sound::sources::InstanceScan;

const NONE: u32 = u32::MAX;
const GESTALT: u32 = 0x51ebd4;
const FIRST_SLOT: u32 = 0x16bc00;
const STOP: u32 = points::SOUND_FINISH_INSTANCE.base;
const SOUND_CLASS: u32 = 0x221810;
const SOURCE_POINT: Point = Point::new("SOUND_SOURCE_WALK", 0x128020, NO_ARGS, DOC);
const ASSIGN_POINT: Point = Point::new("SOUND_ASSIGN_CHANNELS", 0x1282d0, NO_ARGS, DOC);
pub(super) const SOURCE_WALK: Body = Body {
    point: &SOURCE_POINT,
    sites: &[
        0x128034, 0x128114, 0x12814b, 0x128155, 0x128162, 0x1281af, 0x1281df, 0x128204, 0x128253,
    ],
    run: source_walk,
};
pub(super) const CHANNEL_ASSIGNMENT: Body = Body {
    point: &ASSIGN_POINT,
    sites: &[
        0x1282e4, 0x128345, 0x128358, 0x128388, 0x12838f, 0x1283df, 0x12842b, 0x12845d, 0x12846e,
        0x128484, 0x12848d,
    ],
    run: assign,
};

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}
fn row(base: u32, index: u32, size: u32) -> u32 {
    at(base, index.wrapping_mul(size))
}
fn signed_byte(b: u8) -> u32 {
    b as i8 as i32 as u32
}

/// Registers at the two boundaries differ, but both walks pack a datum's
/// current generation with the slot returned by the same allocator.
fn first(c: &mut CpuAbi, g: &Guest, pool: u32) -> Result<u32> {
    let index = c.eax;
    let handle = if index != NONE {
        c.ecx = g.u32(at(pool, 0x24))?;
        c.edx = g.u32(at(pool, RECORDS))?;
        c.ecx = c.ecx.wrapping_mul(index);
        (u32::from(g.u16(at(c.edx, c.ecx))?) << 16) | index
    } else {
        NONE
    };
    arithmetic(c, handle, NONE, true);
    Ok(handle)
}

#[derive(Clone, Copy)]
enum Pass {
    Sources,
    Assign,
}

/// Read the current occupancy bitmap after the last callback, skipping
/// empty words. Preserve the iterator's observable scan registers for
/// the next guest call even when the last word is only partly occupied.
fn next(c: &mut CpuAbi, g: &Guest, pass: Pass) -> Result<bool> {
    let (current, local) = match pass {
        Pass::Sources => (c.esi, 0x18),
        Pass::Assign => (c.ebx, 0x14),
    };
    let start = if current == NONE {
        0
    } else {
        frame::slot(c, g, local)?.wrapping_add(1) as i32
    };
    let pool = g.u32(points::SOUND_INSTANCES.base)?;
    match pass {
        Pass::Sources => c.ebx = pool,
        Pass::Assign => c.ebp = pool,
    }
    let mut found = None;
    if start >= 0 {
        c.edi = g.u32(at(pool, 0x38))?;
        if start < c.edi as i32 {
            c.edx = g.u32(at(pool, 0x48))?;
            let scan = InstanceScan::read(start, c.edi as i32, |i| g.u32(row(c.edx, i, 4)))?;
            if let Some(last) = scan.last_examined {
                c.ecx = last >> 5;
                let mask = 1 << (last & 31);
                match pass {
                    Pass::Sources => c.ebp = mask,
                    Pass::Assign => c.ebx = mask,
                }
            }
            found = scan.next;
        }
    }
    c.esi = found.unwrap_or(NONE);
    c.eax = NONE;
    if let Some(index) = found {
        c.edx = g.u32(at(pool, 0x24))?.wrapping_mul(index);
        c.eax = g.u32(at(pool, RECORDS))?;
        c.eax = (u32::from(g.u16(at(c.eax, c.edx))?) << 16) | index;
    }
    arithmetic(c, c.eax, NONE, true);
    match pass {
        Pass::Sources => c.esi = c.eax,
        Pass::Assign => c.ebx = c.eax,
    }
    Ok(c.eax != NONE)
}

fn source_walk(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    loop {
        match pc {
            0x128020 => {
                c.esp = arithmetic(c, c.esp, 12, true);
                frame::push(c, g, c.esi)?;
                frame::push(c, g, c.edi)?;
                c.edi = g.u32(points::SOUND_INSTANCES.base)?;
                c.edx = 0;
                logic(c, 0, 1 << 31);
                c.ecx = c.edi;
                return call(FIRST_SLOT, 0x128034);
            }
            0x128034 => {
                c.esi = first(c, g, c.edi)?;
                if c.esi == NONE {
                    pc = 0x1282c6;
                } else {
                    frame::push(c, g, c.ebx)?;
                    frame::push(c, g, c.ebp)?;
                    pc = 0x128061;
                }
            }
            0x128061 => {
                c.ebp = g.u32(at(c.edi, RECORDS))?;
                c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                c.ebx = c.esi & 0xffff;
                frame::set_slot(c, g, 0x18, c.ebx)?;
                c.ebx *= SoundInstance::SIZE;
                c.eax = g.u32(at(at(c.ebp, c.ebx), 12))?;
                c.ebp = at(c.ebp, c.ebx);
                c.eax = g.u32(at(row(c.ecx, c.eax & 0xffff, 16), 8))?;
                frame::set_slot(c, g, 0x14, c.eax)?;
                let marker = g.u8(at(c.eax, 12))?;
                c.eax = if marker == 0xff {
                    0
                } else {
                    c.edx = g.u32(GESTALT)?;
                    c.ecx = g.u32(at(c.edx, 0x4c))?;
                    row(c.ecx, signed_byte(marker), 0x1c)
                };
                let channel = g.u16(at(c.ebp, 0xac))?;
                c.edi = lo16(c.edi, channel);
                if channel != u16::MAX {
                    c.edx = g.u32(points::SOUND_CHANNELS.base)?;
                    c.ecx = sw(channel).wrapping_mul(9);
                    if g.u8(at(row(c.edx, c.ecx, 4), 9))? != 0 {
                        pc = 0x128257;
                        continue;
                    }
                }
                c.ecx = u32::from(g.u16(at(c.ebp, 4))?);
                c.edx = c.ecx >> 9;
                if c.edx & 1 != 0 {
                    c.ecx = byte(c.ecx, (c.ecx as u8) >> 6);
                    if c.ecx & 1 != 0 {
                        c.edx = g.u32(points::SOUND_CHANNELS.base)?;
                        c.ecx = sw(channel).wrapping_mul(9);
                        c.ecx = sw(g.u16(at(row(c.edx, c.ecx, 4), 12))?);
                        c.edx = g.u32(points::SOUND_DRIVER.base)?;
                        c.ecx = c.ecx.wrapping_mul(0x34);
                        let state = g.u8(at(at(c.ecx, c.edx), 13))? & 7;
                        logic(c, state.into(), 0x80);
                        if state == 0 {
                            frame::push(c, g, 2)?;
                            c.eax = c.esi;
                            return call(STOP, 0x128114);
                        }
                    }
                }
                logic(c, c.eax, 1 << 31);
                if c.eax != 0 {
                    let mark = g.u8(at(c.ebp, 0xa0))?;
                    c.ecx = byte(c.ecx, mark);
                    cmp_byte(c, mark, 0xff);
                    if mark != 0xff {
                        c.edx = g.u32(at(c.eax, 0x10))?;
                        c.ecx = signed_byte(mark);
                        arithmetic(c, c.ecx, c.edx, true);
                        if (c.ecx as i32) < c.edx as i32 {
                            c.ecx = g.u32(at(c.ebp, 0x8c))?;
                            let when = g.u32(at(c.eax, 0x14))?;
                            arithmetic(c, c.ecx, when, true);
                            if c.ecx == when {
                                let limit = g.u32(at(c.eax, 0x18))?;
                                arithmetic(c, c.ecx, limit, true);
                                if (c.ecx as i32) < limit as i32 {
                                    frame::push(c, g, 15)?;
                                    c.eax = c.esi;
                                    return call(STOP, 0x12814b);
                                }
                            }
                        }
                    }
                }
                return call(0x127fc0, 0x128155);
            }
            0x128155 => {
                let available = c.eax as u8;
                logic(c, available.into(), 0x80);
                if available == 0 {
                    frame::push(c, g, 3)?;
                    c.eax = c.esi;
                    return call(STOP, 0x128162);
                }
                c.edx = frame::slot(c, g, 0x14)?;
                if g.u8(c.edx)? & 8 != 0 {
                    let flags = g.u8(at(c.ebp, 0x18))? | 1;
                    g.set_u8(at(c.ebp, 0x18), flags)?;
                }
                let flags = g.u8(at(c.ebp, 0x18))?;
                c.eax = byte(c.eax, flags >> 3);
                c.edi = at(c.ebp, 0x18);
                logic(c, c.eax & 1, 0x80);
                if c.eax & 1 != 0 {
                    sse::load(c, 0, g.u32(at(c.edi, 0x38))?);
                } else {
                    c.ecx = (g.u32(at(c.ebp, 12))? & 0xffff) << 4;
                    c.edx = g.u32(points::TAG_ENTRIES.base)?;
                    c.eax = g.u32(at(at(c.edx, c.ecx), 8))?;
                    let use_class = g.u8(at(c.eax, 1))? & 8;
                    logic(c, use_class.into(), 0x80);
                    if use_class != 0 {
                        c.eax = lo16(c.eax, signed_byte(g.u8(at(c.eax, 2))?) as u16);
                        frame::push(c, g, c.eax)?;
                        return call(SOUND_CLASS, 0x1281af);
                    }
                    let index = g.u16(at(c.eax, 6))?;
                    c.edx = g.u32(GESTALT)?;
                    c.ecx = imul(c, sw(index), 0x38);
                    c.eax = g.u32(at(c.edx, 4))?;
                    sse::load(c, 0, g.u32(at(at(c.ecx, c.eax), 4))?);
                }
                pc = 0x1281cc;
            }
            0x1281af => {
                sse::load(c, 0, g.u32(at(c.eax, 0x1c))?);
                pc = 0x1281cc;
            }
            0x1281cc => {
                frame::set_slot(c, g, 0x10, sse::lane0(c, 0))?;
                c.ecx = frame::slot(c, g, 0x10)?;
                frame::push(c, g, 0)?;
                frame::push(c, g, c.ecx)?;
                frame::push(c, g, c.edi)?;
                return call(listeners::SELECT.base, 0x1281df);
            }
            0x1281df => {
                if c.eax == NONE {
                    c.edx = byte(c.edx, g.u8(at(c.ebp, 4))? >> 2);
                    if c.edx & 1 == 0 {
                        if c.esi != NONE {
                            c.eax = g.u32(points::SOUND_INSTANCES.base)?;
                            c.edi = g.u32(at(c.eax, RECORDS))?;
                            c.eax = c.esi;
                            c.edi = arithmetic(c, c.edi, c.ebx, false);
                            return call(0x12a810, 0x128204);
                        }
                        let flags = g.u8(at(c.ebp, 4))? | 4;
                        g.set_u8(at(c.ebp, 4), flags)?;
                    }
                } else {
                    c.ecx = byte(c.ecx, g.u8(at(c.ebp, 4))? >> 2);
                    g.set_u8(at(c.ebp, 0xa1), c.eax as u8)?;
                    if c.ecx & 1 != 0 {
                        frame::push(c, g, g.u32(0x128243)?)?;
                        c.edi = NONE;
                        c.eax = c.esi;
                        c.ebx = 0;
                        logic(c, 0, 1 << 31);
                        return call(0x126df0, 0x128253);
                    }
                }
                pc = 0x128257;
            }
            0x128204 => {
                g.set_u32(at(c.edi, 0xb0), c.eax)?;
                g.set_u16(at(c.edi, 0xae), 0)?;
                g.set_u32(at(c.edi, 0xb4), g.u32(0x128219)?)?;
                g.set_u32(at(c.edi, 0xb8), NONE)?;
                let ramp = g.u8(at(c.edi, 4))? | 0x80;
                g.set_u8(at(c.edi, 4), ramp)?;
                let flags = g.u8(at(c.ebp, 4))? | 4;
                g.set_u8(at(c.ebp, 4), flags)?;
                pc = 0x128257;
            }
            0x128253 => {
                let flags = g.u8(at(c.ebp, 4))? & !4;
                g.set_u8(at(c.ebp, 4), flags)?;
                pc = 0x128257;
            }
            0x128114 | 0x12814b | 0x128162 | 0x128257 => {
                if next(c, g, Pass::Sources)? {
                    c.edi = g.u32(points::SOUND_INSTANCES.base)?;
                    pc = 0x128061;
                } else {
                    c.ebp = frame::pop(c, g)?;
                    c.ebx = frame::pop(c, g)?;
                    pc = 0x1282c6;
                }
            }
            0x1282c6 => {
                c.edi = frame::pop(c, g)?;
                c.esi = frame::pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 12, false);
                return Ok(Next::Ret { pop: 0 });
            }
            _ => return Err(invalid_site(pc)),
        }
    }
}

pub(super) fn cmp_byte(c: &mut CpuAbi, a: u8, b: u8) {
    let v = a.wrapping_sub(b);
    let carry = u32::from(a < b);
    let overflow = u32::from((a ^ b) & (a ^ v) & 0x80 != 0) << 11;
    c.eflags = (c.eflags & !crate::sim_physics::ARITH)
        | crate::sim_physics::status(v.into(), 0x80)
        | u32::from((a ^ b ^ v) & 0x10)
        | carry
        | overflow;
    c.eflags_defined |= crate::sim_physics::ARITH;
}

pub(super) fn imul(c: &mut CpuAbi, a: u32, b: i32) -> u32 {
    let product = i64::from(a as i32) * i64::from(b);
    let narrowed = product as i32;
    let overflow = if i64::from(narrowed) == product {
        0
    } else {
        0x801
    };
    c.eflags = (c.eflags & !crate::sim_physics::ARITH) | overflow;
    c.eflags_defined = (c.eflags_defined & !crate::sim_physics::ARITH) | 0x801;
    narrowed as u32
}

fn assign(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    loop {
        match pc {
            0x1282d0 => {
                c.esp = arithmetic(c, c.esp, 8, true);
                frame::push(c, g, c.ebx)?;
                frame::push(c, g, c.ebp)?;
                c.ebp = g.u32(points::SOUND_INSTANCES.base)?;
                c.edx = 0;
                logic(c, 0, 1 << 31);
                c.ecx = c.ebp;
                return call(FIRST_SLOT, 0x1282e4);
            }
            0x1282e4 => {
                c.ebx = first(c, g, c.ebp)?;
                if c.ebx == NONE {
                    pc = 0x1284f8;
                } else {
                    frame::push(c, g, c.esi)?;
                    frame::push(c, g, c.edi)?;
                    pc = 0x128310;
                }
            }
            0x128310 => {
                c.ecx = g.u32(at(c.ebp, RECORDS))?;
                let index = c.ebx & 0xffff;
                frame::set_slot(c, g, 0x14, index)?;
                c.esi = row(c.ecx, index, SoundInstance::SIZE);
                c.edx = g.u32(at(c.esi, 0x8c))?;
                c.eax = g.u32(points::SOUND_GLOBALS.base)?;
                let clock = g.u32(at(c.eax, 0x84))?;
                arithmetic(c, c.edx, clock, true);
                if (c.edx as i32) > clock as i32 {
                    pc = 0x12848d;
                } else {
                    frame::push(c, g, c.esi)?;
                    return call(0x125e60, 0x128345);
                }
            }
            0x128345 => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax as u8 != 0 {
                    c.ecx = at(c.esp, 0x10);
                    frame::push(c, g, c.ecx)?;
                    frame::push(c, g, c.ebx)?;
                    return call(points::SOUND_CHOOSE_CHANNEL.base, 0x128358);
                }
                if g.u16(at(c.esi, 0xac))? != u16::MAX {
                    pc = 0x12848d;
                    continue;
                }
                c.eax = g.u32(at(c.esi, 12))?;
                c.edx = byte(c.edx, g.u8(at(c.esi, 4))?);
                c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                c.eax = (c.eax & 0xffff) << 4;
                c.edi = g.u32(at(at(c.eax, c.ecx), 8))?;
                c.edx = byte(c.edx, (c.edx as u8) >> 4);
                logic(c, c.edx & 1, 0x80);
                if c.edx & 1 != 0 {
                    c.eax = 1;
                    pc = 0x12848d;
                } else {
                    c.eax = lo16(c.eax, signed_byte(g.u8(at(c.edi, 2))?) as u16);
                    frame::push(c, g, c.eax)?;
                    return call(SOUND_CLASS, 0x12842b);
                }
            }
            0x128358 => {
                c.edi = c.eax;
                alu::cmp16(c, c.edi, 0xffff);
                if c.edi as u16 == u16::MAX {
                    frame::push(c, g, 7)?;
                    c.eax = c.ebx;
                    return call(STOP, 0x12848d);
                }
                c.edx = sw(c.edi as u16).wrapping_mul(9);
                c.eax = g.u32(points::SOUND_CHANNELS.base)?;
                c.ebp = row(c.eax, c.edx, 4);
                c.eax = g.u32(c.ebp)?;
                arithmetic(c, c.eax, c.ebx, true);
                if c.eax == c.ebx {
                    pc = 0x1283ab;
                } else {
                    arithmetic(c, c.eax, NONE, true);
                    if c.eax != NONE {
                        c.ecx = frame::slot(c, g, 0x10)?;
                        frame::push(c, g, c.ecx)?;
                        return call(STOP, 0x128388);
                    }
                    pc = 0x12838f;
                }
            }
            0x128388 => {
                c.eax = c.edi;
                return call(0x12a5d0, 0x12838f);
            }
            0x12838f => {
                c.edx = g.u32(points::SOUND_GLOBALS.base)?;
                g.set_u32(c.ebp, c.ebx)?;
                c.eax = g.u32(at(c.edx, 0x84))?;
                g.set_u32(at(c.esi, 0x8c), c.eax)?;
                g.set_u16(at(c.esi, 0xac), c.edi as u16)?;
                pc = 0x1283ab;
            }
            0x1283ab => {
                c.ecx = byte(c.ecx, g.u8(at(c.esi, 4))? >> 5);
                if c.ecx & 1 == 0 {
                    c.eax = g.u32(at(c.esi, 0x14))?;
                    if c.eax != 0 {
                        c.eax = g.u32(at(c.eax, 4))?;
                        logic(c, c.eax, 1 << 31);
                        if c.eax != 0 {
                            c.ecx = g.u32(at(c.esi, 12))?;
                            c.edx = at(c.esi, 0x5c);
                            frame::push(c, g, c.edx)?;
                            c.edx = g.u32(at(c.esi, 0x10))?;
                            frame::push(c, g, c.ebx)?;
                            frame::push(c, g, c.ecx)?;
                            frame::push(c, g, c.edx)?;
                            return call(c.eax, 0x1283df);
                        }
                    }
                }
                pc = 0x12848d;
            }
            0x12842b => {
                c.eax = sw(g.u16(at(c.eax, 0xe))?);
                if c.eax != 0 {
                    pc = 0x12848d;
                    continue;
                }
                c.ecx = sw(g.u16(at(c.edi, 8))?);
                c.eax = signed_byte(g.u8(at(c.esi, 0x9c))?);
                c.eax = arithmetic(c, c.eax, c.ecx, false);
                c.edx = c.eax.wrapping_mul(3);
                c.eax = g.u32(GESTALT)?;
                c.ecx = g.u32(at(c.eax, 0x24))?;
                c.ebp = row(c.ecx, c.edx, 4);
                c.edx = lo16(c.edx, signed_byte(g.u8(at(c.edi, 2))?) as u16);
                frame::push(c, g, c.edx)?;
                return call(SOUND_CLASS, 0x12845d);
            }
            0x12845d => {
                c.eax = byte(c.eax, g.u8(at(c.eax, 0xa))? >> 5);
                logic(c, c.eax & 1, 0x80);
                if c.eax & 1 == 0 {
                    frame::push(c, g, c.edi)?;
                    frame::push(c, g, c.ebp)?;
                    return call(0x2193d0, 0x12846e);
                }
                pc = 0x128484;
            }
            0x12846e => {
                alu::cmp16(c, c.eax, 0xffff);
                if c.eax as u16 == u16::MAX {
                    c.ecx = lo16(c.ecx, signed_byte(g.u8(at(c.esi, 0x9d))?) as u16);
                    frame::push(c, g, c.ecx)?;
                    frame::push(c, g, c.edi)?;
                    frame::push(c, g, c.ebp)?;
                    return call(0x219310, 0x128484);
                }
                pc = 0x128484;
            }
            0x128484 => {
                frame::push(c, g, 14)?;
                c.eax = c.ebx;
                return call(STOP, 0x12848d);
            }
            0x1283df | 0x12848d => {
                if next(c, g, Pass::Assign)? {
                    pc = 0x128310;
                } else {
                    c.edi = frame::pop(c, g)?;
                    c.esi = frame::pop(c, g)?;
                    pc = 0x1284f8;
                }
            }
            0x1284f8 => {
                c.ebp = frame::pop(c, g)?;
                c.ebx = frame::pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 8, false);
                return Ok(Next::Ret { pop: 0 });
            }
            _ => return Err(invalid_site(pc)),
        }
    }
}
