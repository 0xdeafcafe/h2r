//! The per-listener pass. The portal graph supplies the sides of each
//! audible door. Across each door, take the largest opening that reaches
//! a cluster. Then deliver that array to matching sources in list order.
use super::{BSP, GLOBALS, listener, low16, signed16};
use crate::sim_objects::{Cx, Next, Result, frame, sse};
use crate::sim_physics::{arithmetic, logic};
use runtime::{CpuAbi, Guest};
use sim_sound::environment::{PortalSides, portal_max};

const AFTER_AUDIBILITY: u32 = 0x221afe;
const AFTER_OPENING: u32 = 0x221b46;
const AFTER_INIT: u32 = 0x221b5f;
const AFTER_SIDES: u32 = 0x221b70;
const AFTER_SOURCE: u32 = 0x221d89;

pub(super) const SITES: &[u32] = &[
    AFTER_AUDIBILITY,
    AFTER_OPENING,
    AFTER_INIT,
    AFTER_SIDES,
    AFTER_SOURCE,
];

const FRAME_SIZE: u32 = 0x8ac;
const WEIGHTS: u32 = 0xbc;
const SIDES: u32 = 0x3c;

fn finish(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.ebp = frame::pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME_SIZE, false);
    Ok(Next::Ret { pop: 8 })
}

fn sources(c: &mut CpuAbi, g: &mut Guest, first: bool) -> Result<Next> {
    if first {
        c.edi = frame::slot(c, g, 0x8c4)?;
        c.eax = g.u32(c.edi)?;
        if c.eax as i32 > 0 {
            c.edi = c.edi.wrapping_add(4);
            c.esi = c.eax;
        } else {
            return finish_list(c, g);
        }
    }
    loop {
        c.eax = frame::slot(c, g, 0x8c0)?;
        if g.u32(c.edi)? == c.eax {
            arithmetic(c, c.eax, c.eax, true);
            frame::push(c, g, c.ebp)?;
            c.ecx = c.esp.wrapping_add(0xc0);
            frame::push(c, g, c.ecx)?;
            return Ok(Next::Call {
                target: super::SOURCE_ENVIRONMENT.base,
                site: AFTER_SOURCE,
            });
        }
        if !next_source(c) {
            return finish_list(c, g);
        }
    }
}

fn next_source(c: &mut CpuAbi) -> bool {
    c.edi = c.edi.wrapping_add(0x48);
    c.esi = c.esi.wrapping_sub(1);
    c.esi != 0
}

fn finish_list(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = frame::pop(c, g)?;
    c.esi = frame::pop(c, g)?;
    c.ebx = frame::pop(c, g)?;
    finish(c, g)
}

fn first(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.esp = c.esp.wrapping_sub(FRAME_SIZE);
    let index = frame::slot(c, g, 0x8b0)?;
    c.eax = index.wrapping_mul(9);
    c.ecx = g.u32(GLOBALS)?;
    frame::push(c, g, c.ebp)?;
    c.ebp = listener(c.ecx, index);
    frame::set_slot(c, g, 0x10, c.ebp)?;
    if c.ebp == 0 {
        return finish(c, g);
    }
    let cluster = g.u16(c.ebp.wrapping_add(4))?;
    c.edx = low16(c.edx, cluster);
    if cluster == u16::MAX {
        return finish(c, g);
    }

    frame::push(c, g, c.ebx)?;
    c.ebx = g.u32(BSP)?;
    frame::push(c, g, c.esi)?;
    frame::push(c, g, c.edi)?;
    c.eax = 0;
    c.ecx = 0;
    c.edi = c.esp.wrapping_add(WEIGHTS);
    if c.eflags & (1 << 10) == 0 {
        g.write(c.edi, [0; 512 * 4])?;
        c.edi = c.edi.wrapping_add(512 * 4);
    } else {
        for _ in 0..512 {
            g.set_u32(c.edi, 0)?;
            c.edi = c.edi.wrapping_sub(4);
        }
    }
    c.eax = g.u32(c.ebx.wrapping_add(0x224))?;
    frame::set_slot(c, g, 0x24, c.ebx)?;
    if c.eax as i32 <= 0 {
        return sources(c, g, true);
    }

    c.esi = g.u32(c.ebx.wrapping_add(0x228))?;
    g.write(c.esp.wrapping_add(0x2c), [0; 16])?;
    logic(c, 0, 1 << 31);
    frame::push(c, g, signed16(c.edx))?;
    c.ecx = c.esp.wrapping_add(0x30);
    c.eax = c.ebx;
    frame::set_slot(c, g, 0x2c, c.esi)?;
    Ok(Next::Call {
        target: 0x249d10,
        site: AFTER_AUDIBILITY,
    })
}

fn mask_word(g: &Guest, base: u32, index: i32) -> Result<u32> {
    g.u32(base.wrapping_add((index >> 5).cast_unsigned().wrapping_mul(4)))
}

fn doors(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    loop {
        c.ecx = c.edi & 31;
        c.edx = 1 << c.ecx;
        c.eax = (c.edi as i32 >> 5) as u32;
        if mask_word(g, c.esp.wrapping_add(0x2c), c.edi as i32)? & c.edx != 0 {
            c.ecx = g.u32(c.ebx.wrapping_add(0x228))?;
            c.edx = g.u32(c.ecx.wrapping_add(0x30))?;
            c.ecx = (g.u8(c.edx.wrapping_add(c.edi))? as i8 as i32) as u32;
            if c.ecx != u32::MAX {
                arithmetic(c, c.ecx, u32::MAX, true);
                return Ok(Next::Call {
                    target: 0x18cac0,
                    site: AFTER_OPENING,
                });
            }
        }
        if !next_door(c, g)? {
            return sources(c, g, true);
        }
    }
}

fn next_door(c: &mut CpuAbi, g: &mut Guest) -> Result<bool> {
    c.edx = frame::slot(c, g, 0x28)?;
    c.eax = g.u32(c.edx)?;
    c.edi = c.edi.wrapping_add(1);
    frame::set_slot(c, g, 0x18, c.edi)?;
    Ok((c.edi as i32) < c.eax as i32)
}

fn apply_door(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.esi = signed16(u32::from(g.u16(c.ebp.wrapping_add(4))?));
    let cluster = c.esi as i32;
    c.ecx = c.esi & 31;
    c.edx = 1 << c.ecx;
    c.eax = (cluster >> 5) as u32;
    let base = c.esp.wrapping_add(SIDES);
    let masks: [[u8; 64]; 2] = [g.read(base)?, g.read(base.wrapping_add(64))?];
    let masks = PortalSides(masks.map(|bytes| {
        std::array::from_fn(|i| u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap()))
    }));
    let side = if (0..512).contains(&cluster) {
        masks.opposite(cluster as usize)
    } else if mask_word(g, base, cluster)? & c.edx != 0 {
        Some(1)
    } else if mask_word(g, base.wrapping_add(64), cluster)? & c.edx != 0 {
        Some(0)
    } else {
        None
    };
    let Some(side) = side else {
        return Ok(());
    };
    frame::set_slot(c, g, 0x10, side as u32)?;

    let count = g.u32(c.ebx.wrapping_add(0x9c))? as i32;
    let grouped = count.max(0) & !3;
    c.ecx = count as u32;
    if grouped > 0 {
        frame::set_slot(c, g, 0x20, count as u32 - 3)?;
    }
    let door = frame::slot(c, g, 0x14)?;
    for i in 0..count {
        // The four-at-once loop loads the door even if none of its
        // clusters are on this side. The tail loads it on a match.
        let lane = i % 4;
        if i < grouped && lane == 0 {
            sse::load(c, 0, door);
        }
        if i < grouped && lane > 0 {
            c.ecx = i as u32;
        }
        if i != cluster {
            c.ecx = i as u32 & 31;
            let has = if i < 512 {
                masks.contains(side, i as usize)
            } else {
                mask_word(g, base.wrapping_add(64 * side as u32), i)? & (1 << c.ecx) != 0
            };
            if has {
                let dest = c
                    .esp
                    .wrapping_add(WEIGHTS)
                    .wrapping_add((i as u32).wrapping_mul(4));
                let old = g.u32(dest)?;
                let next = portal_max(&mut c.mxcsr, old, door);
                // lane 3 and the scalar tail select into xmm0, the
                // preceding three lanes select into xmm1.
                if i >= grouped {
                    sse::load(c, 0, next);
                    sse::load(c, 1, door);
                } else if lane == 3 {
                    sse::load(c, 0, next);
                    sse::load(c, 1, old);
                } else {
                    sse::load(c, 1, next);
                }
                g.set_u32(dest, next)?;
            }
        }
        if i < grouped && lane == 3 {
            c.ecx = count as u32 - 3;
        }
    }
    c.ebp = frame::slot(c, g, 0x1c)?;
    c.edi = frame::slot(c, g, 0x18)?;
    Ok(())
}

pub(super) fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        AFTER_AUDIBILITY => {
            c.eax = g.u32(c.esi)?;
            c.edi = 0;
            frame::set_slot(c, g, 0x18, 0)?;
            if c.eax as i32 <= 0 {
                sources(c, g, true)
            } else {
                doors(c, g)
            }
        }
        AFTER_OPENING => {
            frame::push(c, g, 0x31bc70)?;
            frame::push(c, g, 2)?;
            frame::push(c, g, 64)?;
            c.eax = c.esp.wrapping_add(0x48);
            frame::push(c, g, c.eax)?;
            frame::set_slot(c, g, 0x24, sse::lane0(c, 0))?;
            Ok(Next::Call {
                target: 0x20670,
                site: AFTER_INIT,
            })
        }
        AFTER_INIT => {
            c.ecx = c.esp.wrapping_add(0x7c);
            frame::push(c, g, c.ecx)?;
            frame::push(c, g, c.edi)?;
            c.eax = c.esp.wrapping_add(0x44);
            c.edx = c.ebx;
            Ok(Next::Call {
                target: 0x249c90,
                site: AFTER_SIDES,
            })
        }
        AFTER_SIDES => {
            apply_door(c, g)?;
            if next_door(c, g)? {
                doors(c, g)
            } else {
                sources(c, g, true)
            }
        }
        AFTER_SOURCE => {
            if next_source(c) {
                sources(c, g, false)
            } else {
                finish_list(c, g)
            }
        }
        _ => first(c, g),
    }
}
