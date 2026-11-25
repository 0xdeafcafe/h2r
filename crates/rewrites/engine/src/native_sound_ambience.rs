//! BSP ambience palette selection at a listener's cluster. Each portal
//! distance query and the final sort see the original scratch arrays.
//! Portal, cluster and palette pointers are read again after guest calls.

use crate::points;
use crate::sim_objects::frame::{pop, push, set_slot, slot};
use crate::sim_objects::{Body, Cx, Next, Result, alu, sse};
use crate::sim_physics::spatial::{finish_fpu, load_fpu};
use crate::sim_physics::{arithmetic, logic};
use runtime::{CpuAbi, Guest};
use sim_sound::ambience::{FadeCurve, PortalLink};
use sim_sound::environment::GainCurve;

pub const BODIES: &[Body] = &[
    Body {
        point: &points::SOUND_CLUSTER_AMBIENCE,
        sites: &SITES,
        run: select,
    },
    Body {
        point: &points::SOUND_AMBIENCE_FARTHER,
        sites: &[],
        run: farther,
    },
];

const SITES: [u32; 4] = [0x18d84d, 0x18d867, 0x18d994, 0x18dbcd];
const DISTANCE_TO_CLUSTER: u32 = 0x249d60;
const DISTANCE_TO_PORTAL: u32 = 0x14b240;
const SORT: u32 = 0x13de30;
const TO_DECIBELS: u32 = 0x2197f0;

const MASK: u32 = 0x24;
const DISTANCES: u32 = 0x38;
const PORTALS: u32 = 0x138;
const ORDER: u32 = 0x238;
const CLUSTERS: u32 = 0x338;
const BSP: u32 = 0x43c;
const OWN_CLUSTER: u32 = 0x440;
const SLOT_COUNT: u32 = 0x20;
const INFINITY: u32 = 0x44aed4;
// Immediate used by the authored silence endpoint at 18daef.
const SILENT_DB: u32 = 0x18daf3;

fn indexed(base: u32, index: u32, stride: u32) -> u32 {
    base.wrapping_add(index.wrapping_mul(stride))
}
fn row(c: &CpuAbi, offset: u32, index: u32) -> u32 {
    indexed(c.esp.wrapping_add(offset), index, 4)
}
fn signed_word(g: &Guest, a: u32) -> Result<u32> {
    Ok(g.u16(a)? as i16 as i32 as u32)
}
fn put_bit(c: &mut CpuAbi, g: &mut Guest, index: u32) -> Result<()> {
    let word = (index as i32 >> 5) as u32;
    c.eax = row(c, MASK, word);
    c.ecx = index & 31;
    c.edx = 1 << c.ecx;
    g.set_u32(c.eax, g.u32(c.eax)? | c.edx)
}
fn farther(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.eax = slot(c, g, 12)?;
    c.ecx = slot(c, g, 4)?;
    c.edx = slot(c, g, 8)?;
    let a = g.u32(indexed(c.eax, c.ecx, 4))?;
    let b = g.u32(indexed(c.eax, c.edx, 4))?;
    sse::load(c, 0, a);
    c.eax = u32::from(sse::comiss(c, a, b) & 0x41 == 0);
    if c.eax == 0 {
        logic(c, 0, 1 << 31);
    }
    Ok(Next::Ret { pop: 12 })
}

fn select(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        pc if pc == points::SOUND_CLUSTER_AMBIENCE.base => {
            c.esp = c.esp.wrapping_sub(0x428);
            c.ecx = slot(c, g, 0x430)?;
            sse::load(c, 0, g.u32(INFINITY)?);
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            push(c, g, c.esi)?;
            c.esi = slot(c, g, 0x438)?;
            c.ebp = indexed(g.u32(c.esi.wrapping_add(0xa0))?, c.ecx, 0xb0);
            c.eax = 0;
            c.edx = u32::MAX;
            set_slot(c, g, 0x20, 0)?;
            push(c, g, c.edi)?;
            set_slot(c, g, 0x28, 0)?;
            set_slot(c, g, 0x1c, sse::lane0(c, 0))?;

            let local = signed_word(g, c.ebp.wrapping_add(0x72))?;
            if local != u32::MAX {
                sse::zero(c, 1);
                g.set_u32(row(c, CLUSTERS, local), c.ecx)?;
                g.set_u32(row(c, DISTANCES, local), 0)?;
                g.set_u32(row(c, PORTALS, local), u32::MAX)?;
                put_bit(c, g, local)?;
            }

            let group = signed_word(g, c.ebp.wrapping_add(0x78))?;
            c.ecx = g.u32(c.esi.wrapping_add(0x14c))?;
            c.edx = indexed(c.ecx, group, 0x14);
            c.eax = g.u32(c.edx.wrapping_add(4))?;
            set_slot(c, g, 0x18, c.edx)?;
            set_slot(c, g, 0x14, 0)?;
            if c.eax as i32 > 0 {
                visit_portals(c, g)
            } else {
                sort_palettes(c, g)
            }
        }
        0x18d84d => {
            c.edx = slot(c, g, 0x444)?;
            c.ecx = c.edi;
            c.edi = slot(c, g, BSP)?;
            c.eax = c.edi;
            sse::movaps(c, 7, 0);
            Ok(Next::Call {
                target: DISTANCE_TO_PORTAL,
                site: SITES[1],
            })
        }
        0x18d867 => {
            accept_portal(c, g)?;
            c.edx = slot(c, g, 0x18)?;
            if next_portal(c, g)? {
                visit_portals(c, g)
            } else {
                sort_palettes(c, g)
            }
        }
        0x18d994 => {
            c.eax = slot(c, g, 0x45c)?;
            if c.eax != 0 {
                g.set_u32(c.eax, 0)?;
            }
            c.eax = g.u32(c.esi.wrapping_add(0xd4))?;
            set_slot(c, g, 0x30, 0)?;
            let count = (c.eax as i32).min(64);
            set_slot(c, g, SLOT_COUNT, count as u32)?;
            if count <= 0 {
                return leave(c, g);
            }
            c.ebx = slot(c, g, 0x450)?;
            c.ecx = slot(c, g, 0x454)?.wrapping_sub(c.ebx);
            c.eax = slot(c, g, 0x44c)?;
            for (reg, ptr) in [(7, 0x45dc70), (4, 0x45dbc0), (5, 0x45dbd4), (6, 0x45dc0c)] {
                sse::load(c, reg, g.u32(ptr)?);
            }
            c.ebp = row(c, ORDER, 0).wrapping_sub(c.eax);
            c.eax = c.eax.wrapping_sub(c.ebx);
            set_slot(c, g, 0x1c, c.ecx)?;
            c.ecx = slot(c, g, 0x458)?.wrapping_sub(c.ebx);
            c.esi = c.ebx;
            set_slot(c, g, 0x34, c.eax)?;
            set_slot(c, g, 0x14, c.ecx)?;
            output(c, g)
        }
        0x18dbcd => {
            g.set_u32(c.esi, c.eax)?;
            write_source(c, g)?;
            if next_output(c, g)? {
                output(c, g)
            } else {
                leave(c, g)
            }
        }
        _ => unreachable!("only the ambience entry and its four return sites are installed"),
    }
}

/// Walk in tag order. The high bit on each portal reference selects the
/// neighbour on that side. The distance routines can change stack or BSP
/// data, so the iteration count and list are read on every continuation.
fn visit_portals(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    loop {
        let n = slot(c, g, 0x14)?;
        let links = g.u32(c.edx.wrapping_add(8))?;
        let link = PortalLink::from(g.u16(indexed(links, n, 2))?);
        c.ebx = link.index;
        c.edi = indexed(g.u32(c.esi.wrapping_add(0x60))?, link.index, 0x24);
        c.ecx = signed_word(g, indexed(c.edi, link.side, 2))?;
        c.eax = slot(c, g, BSP)?;
        c.esi = indexed(g.u32(c.eax.wrapping_add(0xa0))?, c.ecx, 0xb0);
        set_slot(c, g, 0x20, c.ecx)?;
        if g.u8(c.edi.wrapping_add(0x18))? & 0x28 == 0 {
            let other = g.u16(c.esi.wrapping_add(0x72))?;
            let local = g.u16(c.ebp.wrapping_add(0x72))?;
            if other != local {
                alu::cmp16(c, other.into(), local.into());
                c.eax = slot(c, g, OWN_CLUSTER)?;
                c.edx = slot(c, g, BSP)?;
                return Ok(Next::Call {
                    target: DISTANCE_TO_CLUSTER,
                    site: SITES[0],
                });
            }
        }
        if !next_portal(c, g)? {
            return sort_palettes(c, g);
        }
    }
}

fn next_portal(c: &mut CpuAbi, g: &mut Guest) -> Result<bool> {
    c.eax = slot(c, g, 0x14)?.wrapping_add(1);
    c.ecx = g.u32(c.edx.wrapping_add(4))?;
    c.esi = slot(c, g, BSP)?;
    set_slot(c, g, 0x14, c.eax)?;
    if (c.eax as i32) < c.ecx as i32 {
        return Ok(true);
    }
    sse::load(c, 0, g.u32(INFINITY)?);
    Ok(false)
}

fn accept_portal(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let previous = slot(c, g, 0x1c)?;
    sse::load(c, 1, previous);
    let distance = sse::ss_reg(c, sse::Ss::Add, 0, 7);
    if sse::comiss(c, previous, distance) & 1 == 0 {
        let own = signed_word(g, c.ebp.wrapping_add(0x72))?;
        if own != u32::MAX {
            g.set_u32(row(c, PORTALS, own), c.ebx)?;
            set_slot(c, g, 0x1c, distance)?;
        }
    }
    let palette = signed_word(g, c.esi.wrapping_add(0x72))?;
    if palette == u32::MAX {
        return Ok(());
    }
    c.eax = indexed(g.u32(c.edi.wrapping_add(0xd8))?, palette, 0x64);
    let radius = if c.eax != 0 {
        g.u32(c.eax.wrapping_add(0x44))?
    } else {
        slot(c, g, 0x448)?
    };
    sse::load(c, 1, radius);
    if sse::comiss(c, radius, sse::lane0(c, 7)) & 1 != 0 {
        return Ok(());
    }
    let bit = 1 << (palette & 31);
    let word = row(c, MASK, (palette as i32 >> 5) as u32);
    let limit = if g.u32(word)? & bit != 0 {
        let d = g.u32(row(c, DISTANCES, palette))?;
        sse::load(c, 1, d);
        d
    } else {
        radius
    };
    if sse::comiss(c, limit, distance) & 1 == 0 {
        g.set_u32(row(c, DISTANCES, palette), distance)?;
        g.set_u32(row(c, CLUSTERS, palette), slot(c, g, 0x20)?)?;
        g.set_u32(row(c, PORTALS, palette), c.ebx)?;
        put_bit(c, g, palette)?;
    }
    Ok(())
}

fn sort_palettes(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(c.esi.wrapping_add(0xd4))?;
    c.edx = 0;
    logic(c, c.eax, 1 << 31);
    while (c.edx as i32) < c.eax as i32 {
        c.edi = 1 << (c.edx & 31);
        let word = g.u32(row(c, MASK, c.edx / 32))?;
        g.set_u32(row(c, ORDER, c.edx), c.edx)?;
        if word & c.edi == 0 {
            g.set_u32(row(c, DISTANCES, c.edx), sse::lane0(c, 0))?;
        }
        c.edx = c.edx.wrapping_add(1);
        arithmetic(c, c.edx, c.eax, true);
    }
    c.edx = row(c, DISTANCES, 0);
    let order = row(c, ORDER, 0);
    let comparator_context = c.esp.wrapping_add(0x2c);
    push(c, g, c.edx)?;
    push(c, g, points::SOUND_AMBIENCE_FARTHER.base)?;
    push(c, g, comparator_context)?;
    c.ecx = order;
    Ok(Next::Call {
        target: SORT,
        site: SITES[2],
    })
}

/// A caller asking for a compact list supplies a count. Other callers
/// receive a sentinel for every missing palette.
fn output(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    loop {
        c.eax = c.eax.wrapping_add(c.esi);
        c.edx = g.u32(c.eax.wrapping_add(c.ebp))?;
        c.ecx = (c.edx as i32 >> 5) as u32;
        c.edi = 1 << (c.edx & 31);
        let mask = g.u32(row(c, MASK, c.ecx))?;
        if mask & c.edi != 0 {
            g.set_u32(c.eax, c.edx)?;
            if c.ebx != 0 {
                if c.edx != u32::MAX {
                    return fade(c, g);
                }
                g.set_u32(c.esi, g.u32(SILENT_DB)?)?;
            }
            write_source(c, g)?;
        } else {
            c.ecx = slot(c, g, 0x45c)?;
            if c.ecx != 0 {
                return leave(c, g);
            }
            g.set_u32(c.eax, u32::MAX)?;
            if c.ebx != 0 {
                g.set_u32(c.esi, g.u32(SILENT_DB)?)?;
            }
            c.eax = slot(c, g, 0x454)?;
            if c.eax != 0 {
                c.ecx = slot(c, g, 0x1c)?;
                g.set_u32(c.esi.wrapping_add(c.ecx), u32::MAX)?;
            }
            c.eax = slot(c, g, 0x458)?;
            if c.eax != 0 {
                c.edx = slot(c, g, 0x14)?;
                g.set_u32(c.esi.wrapping_add(c.edx), u32::MAX)?;
            }
        }
        if !next_output(c, g)? {
            return leave(c, g);
        }
    }
}

fn fade(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = slot(c, g, BSP)?;
    c.edi = g.u32(c.ecx.wrapping_add(0xd8))?;
    c.eax = indexed(c.edi, c.edx, 0x64);
    let radius = g.u32(c.eax.wrapping_add(0x44))?;
    let distance = g.u32(row(c, DISTANCES, c.edx))?;
    let curve = FadeCurve {
        minimum_radius: sse::lane0(c, 7),
        epsilon: g.u32(0x45dbdc)?,
        one: sse::lane0(c, 4),
        quadratic: sse::lane0(c, 5),
        cubic: sse::lane0(c, 6),
        db_threshold: g.u32(0x44a0b4)?,
        silent_db: g.u32(SILENT_DB)?,
        gain: GainCurve {
            decibel_scale: g.u32(0x45dc1c)?,
            exponent_scale: g.u32(0x44a0bc)?,
        },
    };
    let mut fpu = load_fpu(c);
    let result = curve.at(radius, distance, &mut c.mxcsr, &mut fpu.0);
    set_slot(c, g, 0x18, result.radius)?;
    set_slot(c, g, 0x2c, result.low)?;
    set_slot(c, g, 0x10, result.high)?;
    // The final FSTP consumes the high endpoint. The other scratch
    // registers contain the low endpoint, smoothed weight and zero.
    finish_fpu(c, g, fpu, 0x18db9d, c.esp.wrapping_add(0x10))?;
    sse::load(c, 0, result.linear);
    sse::load(c, 1, result.weight);
    sse::load(c, 2, result.low);
    sse::zero(c, 3);
    c.eflags = (c.eflags & !0x8d5) | result.flags;
    c.eflags_defined |= 0x8d5;
    c.eax = result.high_db;
    Ok(Next::Call {
        target: TO_DECIBELS,
        site: SITES[3],
    })
}

fn write_source(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.eax = slot(c, g, 0x454)?;
    if c.eax != 0 {
        c.eax = g.u32(row(c, PORTALS, c.edx))?;
        c.ecx = slot(c, g, 0x1c)?;
        g.set_u32(c.esi.wrapping_add(c.ecx), c.eax)?;
    }
    c.eax = slot(c, g, 0x458)?;
    if c.eax != 0 {
        c.edx = g.u32(row(c, CLUSTERS, c.edx))?;
        c.eax = slot(c, g, 0x14)?;
        g.set_u32(c.esi.wrapping_add(c.eax), c.edx)?;
    }
    c.eax = slot(c, g, 0x45c)?;
    if c.eax != 0 {
        g.set_u32(c.eax, g.u32(c.eax)?.wrapping_add(1))?;
    }
    Ok(())
}

fn next_output(c: &mut CpuAbi, g: &mut Guest) -> Result<bool> {
    c.eax = slot(c, g, 0x30)?.wrapping_add(1);
    c.ecx = slot(c, g, SLOT_COUNT)?;
    c.esi = c.esi.wrapping_add(4);
    set_slot(c, g, 0x30, c.eax)?;
    let more = (c.eax as i32) < c.ecx as i32;
    if more {
        c.eax = slot(c, g, 0x34)?;
    }
    Ok(more)
}

fn leave(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x428, false);
    Ok(Next::Ret { pop: 36 })
}
