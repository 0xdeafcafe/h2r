//! Per-frame loop iteration. A callback can delete a loop, queue a new
//! one, change its track list, or replace its detail sound. Its caller
//! keeps live cursors on the guest frame and reloads the counts after it.

use super::{EMPTY, Point, tag_definition, track};
use crate::datum::layout;
use crate::points;
use crate::sim_objects::frame::{pop, push, set_slot, slot};
use crate::sim_objects::{Cx, Next, Result};
use crate::sim_physics::{arithmetic, byte, logic, spatial};
use crate::sound::{LoopingSound, LoopingTrack, LoopingTrackTag};
use runtime::{CpuAbi, Guest};

pub(super) static UPDATE: Point = Point::new(
    "SOUND_LOOPS_UPDATE",
    0x21b0e0,
    crate::point_sig!(fn(advance: stack 0) ret 4),
    super::DOC,
);

pub(super) const SITES: &[u32] = &[
    0x21b0f7, 0x21b175, 0x21b37d, 0x21b49a, 0x21b520, 0x21b54a, 0x21b605, 0x21b623, 0x21b664,
    0x21b735, 0x21b751, 0x21b7b6,
];

fn datum(g: &Guest, pool: u32, index: u32) -> Result<u32> {
    let stride = g.u32(pool.wrapping_add(layout::ELEMENT_SIZE))?;
    let records = g.u32(pool.wrapping_add(layout::DATA))?;
    let salt = g.u16(records.wrapping_add(index.wrapping_mul(stride)))?;
    Ok((u32::from(salt) << 16) | index)
}

fn finish(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x128, false);
    Ok(Next::Ret { pop: 4 })
}

pub(super) fn update(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    loop {
        let (c, g) = (&mut *cx.cpu, &mut *cx.g);
        pc = match pc {
            0x21b0e0 => {
                c.esp = arithmetic(c, c.esp, 0x128, true);
                push(c, g, c.esi)?;
                c.esi = g.u32(points::LOOPING_SOUNDS.base)?;
                push(c, g, c.edi)?;
                c.edx = 0;
                logic(c, 0, 1 << 31);
                c.ecx = c.esi;
                return Ok(Next::Call {
                    target: 0x16bc00,
                    site: 0x21b0f7,
                });
            }
            0x21b0f7 => {
                c.ecx = if c.eax == EMPTY {
                    EMPTY
                } else {
                    c.edx = g.u32(c.esi.wrapping_add(layout::DATA))?;
                    datum(g, c.esi, c.eax)?
                };
                c.edi = c.ecx;
                set_slot(c, g, 0x14, c.edi)?;
                if c.edi == EMPTY {
                    return finish(c, g);
                }
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                0x21b130
            }
            0x21b130 => {
                c.edx = g.u32(c.esi.wrapping_add(layout::DATA))?;
                c.ecx = byte(c.ecx, g.u8(c.esp.wrapping_add(0x13c))?);
                let index = c.edi & 0xffff;
                set_slot(c, g, 0x44, index)?;
                c.eax = c.edx.wrapping_add(index * LoopingSound::SIZE);
                set_slot(c, g, 0x14, c.eax)?;
                if c.ecx as u8 != 0 {
                    c.edx = g.u32(points::SOUND_GLOBALS.base)?;
                    c.ecx = byte(c.ecx, g.u8(c.eax.wrapping_add(0x53))?);
                    if c.ecx as u8 != g.u8(c.edx.wrapping_add(0x7f))? {
                        let flag = g.u8(c.eax.wrapping_add(0x50))? & 2;
                        logic(c, u32::from(flag), 0x80);
                        if flag != 0 {
                            c.ecx = byte(c.ecx, g.u8(c.eax.wrapping_add(0x51))?);
                            logic(c, u32::from(c.ecx as u8), 0x80);
                        }
                        if flag == 0 || c.ecx as u8 == 0 {
                            return Ok(Next::Call {
                                target: 0x21b070,
                                site: 0x21b175,
                            });
                        }
                    }
                }
                let owner = c.eax;
                let tag = g.u32(LoopingSound::TAG.at(owner))?;
                c.eax = tag_definition(g, g.u32(points::TAG_ENTRIES.base)?, tag)?;
                c.ecx = g.u32(c.eax.wrapping_add(0x1c))?;
                logic(c, c.ecx, 1 << 31);
                set_slot(c, g, 0x2c, c.eax)?;
                set_slot(c, g, 0x24, 0)?;
                if (c.ecx as i32) <= 0 {
                    0x21b698
                } else {
                    c.edx = arithmetic(c, owner, 0x88, false);
                    set_slot(c, g, 0x18, 0)?;
                    set_slot(c, g, 0x10, c.edx)?;
                    0x21b1c0
                }
            }
            0x21b1c0 | 0x21b37d | 0x21b49a | 0x21b520 | 0x21b54a | 0x21b605 | 0x21b623
            | 0x21b664 => {
                if let Some(call) = track::update(cx, pc)? {
                    return Ok(call);
                }
                0x21b66a
            }
            0x21b66a => {
                c.eax = slot(c, g, 0x24)?.wrapping_add(1);
                c.edi = slot(c, g, 0x18)?.wrapping_add(LoopingTrackTag::SIZE);
                c.esi = slot(c, g, 0x10)?.wrapping_add(LoopingTrack::SIZE);
                c.ecx = slot(c, g, 0x2c)?;
                c.edx = g.u32(c.ecx.wrapping_add(0x1c))?;
                arithmetic(c, c.eax, c.edx, true);
                set_slot(c, g, 0x24, c.eax)?;
                set_slot(c, g, 0x18, c.edi)?;
                set_slot(c, g, 0x10, c.esi)?;
                if (c.eax as i32) < c.edx as i32 {
                    0x21b1c0
                } else {
                    0x21b698
                }
            }
            0x21b698 => {
                c.edi = slot(c, g, 0x14)?;
                if g.u8(c.edi.wrapping_add(0x52))? == 2 || g.u8(c.edi.wrapping_add(0x50))? & 4 != 0
                {
                    0x21b7e5
                } else {
                    c.ecx = slot(c, g, 0x2c)?;
                    c.eax = g.u32(c.ecx.wrapping_add(0x24))?;
                    c.edx = 0;
                    set_slot(c, g, 0x20, 0)?;
                    if (c.eax as i32) <= 0 {
                        0x21b7e5
                    } else {
                        set_slot(c, g, 0x18, 0)?;
                        c.ebp = c.edi.wrapping_add(0x58);
                        0x21b6d0
                    }
                }
            }
            0x21b6d0 => {
                c.esi = g.u32(c.ecx.wrapping_add(0x28))?;
                c.ecx = slot(c, g, 0x18)?;
                c.esi = c.esi.wrapping_add(c.ecx);
                c.eax = g.u32(c.esi.wrapping_add(8))?;
                if c.eax != EMPTY {
                    c.ecx = g.u32(points::SOUND_GLOBALS.base)?;
                    c.eax = g.u32(c.ebp)?;
                    if (c.eax as i32) < g.u32(c.ecx.wrapping_add(0x84))? as i32 {
                        c.eax = g.u32(c.esi.wrapping_add(0x18))?;
                        c.ecx = byte(c.ecx, 1);
                        let flags = c.eax as u8;
                        let alternate = || g.u8(c.edi.wrapping_add(0x50)).map(|v| v & 1);
                        if flags & 1 == 0 || alternate()? == 0 {
                            let gate = if flags & 2 != 0 { alternate()? } else { 0 };
                            if flags & 2 == 0 || gate != 0 {
                                logic(c, u32::from(gate), 0x80);
                                c.eax = slot(c, g, 0x1c)?;
                                g.set_u16(c.esp.wrapping_add(0xa4), c.edx as u16)?;
                                c.edx = c.esp.wrapping_add(0x98);
                                for arg in [c.edx, c.eax, c.esi] {
                                    push(c, g, arg)?;
                                }
                                return Ok(Next::Call {
                                    target: 0x21cc60,
                                    site: 0x21b735,
                                });
                            }
                        }
                    }
                }
                0x21b7bf
            }
            0x21b735 => {
                logic(c, u32::from(c.eax as u8), 0x80);
                if c.eax as u8 != 0 {
                    c.ebx = g.u32(c.esi.wrapping_add(8))?;
                    c.ecx = c.esp.wrapping_add(0x98);
                    push(c, g, c.ecx)?;
                    c.edi = c.esp.wrapping_add(0x4c);
                    return Ok(Next::Call {
                        target: 0x125f70,
                        site: 0x21b751,
                    });
                }
                0x21b7bf
            }
            0x21b751 => {
                if c.eax != EMPTY || slot(c, g, 0x48)? == 1 {
                    c.ecx = g.u32(points::RANDOM_SEEDS.base)?;
                    let seed = c.ecx.wrapping_add(4);
                    let stepped = g
                        .u32(seed)?
                        .wrapping_mul(g.u32(0x21b76e)?)
                        .wrapping_add(g.u32(0x21b773)?);
                    g.set_u32(seed, stepped)?;
                    c.eax = stepped >> 16;
                    logic(c, c.eax, 1 << 31);
                    set_slot(c, g, 0x28, c.eax)?;
                    let interval = [
                        g.u32(c.esi.wrapping_add(12))?,
                        g.u32(c.esi.wrapping_add(16))?,
                    ];
                    c.edx = g.u32(points::SOUND_GLOBALS.base)?;
                    let time = c.edx.wrapping_add(0x84);
                    let mut fpu = spatial::load_fpu(c);
                    sim_sound::loops::detail_deadline(
                        &mut fpu.0,
                        interval,
                        c.eax as u16,
                        g.u32(0x45dc50)?,
                        g.u32(0x45dccc)?,
                        g.u32(time)? as i32,
                    );
                    spatial::finish_fpu(c, g, fpu, 0x21b7ab, time)?;
                    return Ok(Next::Call {
                        target: 0x322b0c,
                        site: 0x21b7b6,
                    });
                }
                c.edi = slot(c, g, 0x14)?;
                0x21b7bf
            }
            0x21b7b6 => {
                let mut fpu = spatial::load_fpu(c);
                x87::abi::st_st(&mut fpu.0, 0, true);
                let operand = c.x87.data_pointer;
                let selector = c.x87.data_selector;
                spatial::finish_fpu(c, g, fpu, 0x21b7b6, operand)?;
                c.x87.data_selector = selector;
                g.set_u32(c.ebp, c.eax)?;
                c.edi = slot(c, g, 0x14)?;
                0x21b7bf
            }
            0x21b7bf => {
                c.eax = slot(c, g, 0x20)?.wrapping_add(1);
                c.esi = slot(c, g, 0x18)?.wrapping_add(0x34);
                c.ecx = slot(c, g, 0x2c)?;
                c.edx = g.u32(c.ecx.wrapping_add(0x24))?;
                c.ebp = c.ebp.wrapping_add(4);
                set_slot(c, g, 0x20, c.eax)?;
                set_slot(c, g, 0x18, c.esi)?;
                if (c.eax as i32) < c.edx as i32 {
                    c.edx = 0;
                    0x21b6d0
                } else {
                    0x21b7e5
                }
            }
            0x21b175 | 0x21b7e5 => {
                // The loop just released may have been the last occupied row.
                // Re-read the header, high water and bitmap after each callback.
                c.eax = if slot(c, g, 0x1c)? == EMPTY {
                    0
                } else {
                    slot(c, g, 0x44)?.wrapping_add(1)
                };
                c.esi = EMPTY;
                c.ebx = g.u32(points::LOOPING_SOUNDS.base)?;
                if (c.eax as i32) >= 0 {
                    c.edi = g.u32(c.ebx.wrapping_add(layout::HIGH_WATER))?;
                    if (c.eax as i32) < c.edi as i32 {
                        c.edx = g.u32(c.ebx.wrapping_add(layout::BITMAP))?;
                        while (c.eax as i32) < c.edi as i32 {
                            c.ecx = c.eax >> 5;
                            let word = g.u32(c.edx.wrapping_add(c.ecx * 4))?;
                            if let Some(slot) =
                                sim_sound::scheduling::next_in_word(word, c.eax, c.edi)
                            {
                                c.esi = slot;
                                c.ebp = 1 << (slot & 31);
                                break;
                            }
                            c.eax = ((c.eax & !31) + 32).min(c.edi);
                            c.ebp = 1 << ((c.eax - 1) & 31);
                        }
                    }
                }
                c.eax = EMPTY;
                if c.esi != EMPTY {
                    c.edx = g
                        .u32(c.ebx.wrapping_add(layout::ELEMENT_SIZE))?
                        .wrapping_mul(c.esi);
                    c.eax = datum(g, c.ebx, c.esi)?;
                }
                set_slot(c, g, 0x1c, c.eax)?;
                if c.eax == EMPTY {
                    c.ebp = pop(c, g)?;
                    c.ebx = pop(c, g)?;
                    return finish(c, g);
                }
                c.edi = c.eax;
                c.esi = g.u32(points::LOOPING_SOUNDS.base)?;
                0x21b130
            }
            _ => unreachable!("invalid loop walk continuation"),
        }
    }
}
