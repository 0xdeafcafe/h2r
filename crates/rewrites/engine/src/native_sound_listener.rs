//! Listener update at the frame and effect-service boundary. The camera,
//! spatial query and mixer calls can re-enter the guest. The live seat is
//! recovered from its stack frame on each return.
use crate::{
    points,
    sim_objects::{Body, Cx, Next, Result, alu, frame, sse},
    sim_physics::{ARITH, arithmetic},
};
use runtime::{CpuAbi, Guest};
use sim_sound::listener::{self, Axis};
use sse::Ss::{Add, Div, Mul, Sub};

const OBSERVERS: u32 = 0x004e_9c8c;
const OBSERVER_STRIDE: u32 = 0x358;
const BSP_INDEX: u32 = 0x0046_86c4;
const BSP: u32 = 0x004e_0348;
const ONE: u32 = 0x0045_dbc0;
const TWO: u32 = 0x0045_dc0c;
const NO_ENVIRONMENT: u32 = 0x0044_a2a0;
const MODE: u32 = 0x155760;

pub const BODIES: &[Body] = &[Body {
    point: &points::SOUND_LISTENERS_UPDATE,
    sites: &[
        0x127498, 0x1274aa, 0x12754c, 0x1275c6, 0x1275d8, 0x1277fd, 0x12780a, 0x127ce8,
    ],
    run: update,
}];

fn axis(g: &Guest, a: u32) -> Result<Axis> {
    Ok([
        g.u32(a)?,
        g.u32(a.wrapping_add(4))?,
        g.u32(a.wrapping_add(8))?,
    ])
}

fn put_axis(g: &mut Guest, a: u32, v: Axis) -> Result<()> {
    for (off, word) in [0, 4, 8].into_iter().zip(v) {
        g.set_u32(a.wrapping_add(off), word)?;
    }
    Ok(())
}

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

fn finish(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = frame::pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0xe8, false);
    Ok(Next::Ret { pop: 4 })
}

fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x127400 => {
            c.eax = g.u32(points::GAME_GLOBALS.base)?;
            c.esp = c.esp.wrapping_sub(0xe8);
            frame::push(c, g, c.edi)?;
            c.edi = 0;
            if c.eax == 0 {
                return finish(c, g);
            }
            let active = g.u8(c.eax.wrapping_add(0x1120))?;
            c.ecx = (c.ecx & !0xff) | u32::from(active);
            if active == 0 {
                return finish(c, g);
            }
            c.eax = g.u32(points::PLAYER_GLOBALS.base)?.wrapping_add(0xc);
            c.edx = g.u32(points::SOUND_GLOBALS.base)?;
            frame::push(c, g, c.ebx)?;
            frame::push(c, g, c.ebp)?;
            frame::push(c, g, c.esi)?;
            frame::set_slot(c, g, 0x64, 0)?;
            frame::set_slot(c, g, 0x38, OBSERVERS)?;
            frame::set_slot(c, g, 0x44, c.eax)?;
            frame::set_slot(c, g, 0x48, 0)?;
            c.ebp = c.edx.wrapping_add(0xa8);
            seat(c, g)
        }
        0x127498 => {
            c.ebx = c.eax;
            arithmetic(c, c.esi, u32::MAX, true);
            if c.esi == u32::MAX {
                return free_camera(c, g);
            }
            c.eax = c.edi;
            call(MODE, 0x1274aa)
        }
        0x1274aa => {
            c.ecx = c.eax;
            c.edx = 1u32.wrapping_shl(c.ecx);
            if c.edx & 3 == 0 {
                return free_camera(c, g);
            }
            attached_camera(c, g)
        }
        0x12754c => {
            let sp = c.esp;
            for offset in [0, 4, 8] {
                let origin = g.u32(c.edi.wrapping_add(offset))?;
                sse::load(c, 1, origin);
                sse::load(c, 0, g.u32(sp + 0x84 + offset)?);
                sse::ss(c, Sub, 0, origin);
                sse::ss(c, Mul, 0, g.u32(c.ebx.wrapping_add(0x40))?);
                let v = sse::ss(c, Add, 0, origin);
                g.set_u32(sp + 0x28 + offset, v)?;
            }
            c.esi = c.edi.wrapping_add(0x20);
            c.ecx = c.esi;
            c.edx = arithmetic(c, frame::slot(c, g, 0x7c)?, 0x180, false);
            c.eax = sp + 0x6c;
            call(0x11d6a0, 0x1275c6)
        }
        0x1275c6 => {
            c.edx = g.u32(0x0046_87cc)?;
            sse::load(c, 0, g.u32(c.ebx.wrapping_add(0x40))?);
            c.ecx = c.eax;
            call(0x11d950, 0x1275d8)
        }
        0x1275d8 => rotated_axes(c, g),
        0x1277fd => {
            c.edx = c.esp + 0x28;
            c.esi = c.esp + 0x3c;
            call(0x11bed0, 0x12780a)
        }
        0x12780a => {
            c.ecx = frame::slot(c, g, 0x40)?;
            if c.ecx as u16 == u16::MAX {
                frame::set_slot(c, g, 0x3c, g.u32(c.edi.wrapping_add(0xc))?)?;
                c.ecx = g.u32(c.edi.wrapping_add(0x10))?;
                frame::set_slot(c, g, 0x40, c.ecx)?;
            }
            publish(c, g)
        }
        0x127ce8 => finish(c, g),
        _ => unreachable!("registered listener entry or return"),
    }
}

fn seat(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.esi = u32::MAX;
    let offset = frame::slot(c, g, 0x48)?;
    if offset != 0u32.wrapping_sub(OBSERVER_STRIDE) {
        let player = g.u32(frame::slot(c, g, 0x44)?)?;
        if player != u32::MAX {
            c.edx = g.u32(points::PLAYERS.base)?;
            c.ecx = g.u32(c.edx.wrapping_add(0x44))?;
            let slot = (player & 0xffff) * 0x21c;
            c.esi = g.u32(slot.wrapping_add(c.ecx).wrapping_add(0x2c))?;
            frame::set_slot(c, g, 0x34, c.esi)?;
            c.eax = c.edi;
            // This bounded slot multiply fits signed 32 bits. IMUL only
            // defines carry and overflow at the call.
            c.eflags &= !ARITH;
            c.eflags_defined = (c.eflags_defined & !ARITH) | 0x801;
            return call(MODE, 0x127498);
        }
    }
    g.set_u32(c.ebp.wrapping_sub(0x20), u32::MAX)?;
    g.set_u16(c.ebp.wrapping_sub(0x1c), u16::MAX)?;
    next_seat(c, g)
}

fn next_seat(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let player = frame::slot(c, g, 0x44)?.wrapping_add(4);
    let camera = frame::slot(c, g, 0x38)?.wrapping_add(OBSERVER_STRIDE);
    c.ecx = frame::slot(c, g, 0x48)?.wrapping_add(OBSERVER_STRIDE);
    c.edi = c.edi.wrapping_add(1);
    c.ebp = c.ebp.wrapping_add(0x48);
    frame::set_slot(c, g, 0x44, player)?;
    frame::set_slot(c, g, 0x64, c.edi)?;
    frame::set_slot(c, g, 0x48, c.ecx)?;
    frame::set_slot(c, g, 0x38, camera)?;
    if (camera as i32) < (OBSERVERS + 4 * OBSERVER_STRIDE) as i32 {
        return seat(c, g);
    }
    let submit = g.u8(c.esp + 0xfc)?;
    c.eax = (camera & !0xff) | u32::from(submit);
    c.esi = frame::pop(c, g)?;
    c.ebp = frame::pop(c, g)?;
    c.ebx = frame::pop(c, g)?;
    if submit == 0 {
        finish(c, g)
    } else {
        environments(c, g)
    }
}

fn observer(c: &CpuAbi, g: &Guest) -> Result<u32> {
    if g.u16(BSP_INDEX)? != u16::MAX {
        frame::slot(c, g, 0x38)
    } else {
        Ok(0)
    }
}

fn free_camera(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let camera = observer(c, g)?;
    copy_camera(c, g, camera)
}

fn attached_camera(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let array = g.u32(points::OBJECTS.base)?;
    let headers = g.u32(array.wrapping_add(0x44))?;
    let object = |g: &Guest, h: u32| g.u32(headers.wrapping_add((h & 0xffff) * 12).wrapping_add(8));
    let unit = object(g, c.esi)?;
    let parent = g.u32(unit.wrapping_add(0x14))?;
    frame::set_slot(c, g, 0x7c, unit)?;
    c.edi = observer(c, g)?;
    if parent == u32::MAX {
        return copy_camera(c, g, c.edi);
    }
    let seat = g.u16(unit.wrapping_add(0x1fc))?;
    if seat == u16::MAX || c.ebx as u16 == 0 {
        return copy_camera(c, g, c.edi);
    }
    let vehicle = g.u32(object(g, parent)?)?;
    c.ecx = g.u32(points::TAG_ENTRIES.base)?;
    let tag = g.u32(c.ecx.wrapping_add((vehicle & 0xffff) << 4).wrapping_add(8))?;
    c.edx = g.u32(tag.wrapping_add(0x1cc))?;
    c.ebx = arithmetic(
        c,
        (i32::from(seat as i16) as u32).wrapping_mul(0xb0),
        c.edx,
        false,
    );
    c.eax = frame::slot(c, g, 0x34)?;
    c.esi = c.esp + 0x84;
    call(0xcaf60, 0x12754c)
}

fn copy_camera(c: &mut CpuAbi, g: &mut Guest, camera: u32) -> Result<Next> {
    let sp = c.esp;
    for (source, dest) in [(0x20, 0x10), (0x2c, 0x1c), (0, 0x28), (0x14, 0x54)] {
        let v = axis(g, camera.wrapping_add(source))?;
        put_axis(g, sp + dest, v)?;
    }
    frame::set_slot(c, g, 0x3c, g.u32(camera.wrapping_add(0xc))?)?;
    c.ecx = g.u32(camera.wrapping_add(0x10))?;
    frame::set_slot(c, g, 0x40, c.ecx)?;
    for (reg, off) in [(5, 0x18), (3, 0x14), (4, 0x24), (6, 0x20)] {
        sse::load(c, reg, g.u32(sp + off)?);
    }
    publish(c, g)
}

fn rotated_axes(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let sp = c.esp;
    let [qx, qy, qz] = axis(g, sp + 0x6c)?;
    let r = listener::rotate_seat(
        axis(g, c.esi)?,
        axis(g, c.edi.wrapping_add(0x2c))?,
        [qx, qy, qz, g.u32(sp + 0x78)?],
        g.u32(ONE)?,
        g.u32(TWO)?,
        c.mxcsr,
    );
    c.mxcsr = r.mxcsr;
    put_axis(g, sp + 0x10, r.forward)?;
    put_axis(g, sp + 0x1c, r.up)?;
    for off in [0x60, 0x68, 0x4c] {
        g.set_u32(sp + off, r.cross_scale)?;
    }
    for off in [0x80, 0x50] {
        g.set_u32(sp + off, r.vector_scale)?;
    }
    for (reg, word) in [
        r.up_z_scaled,
        r.up_dot,
        qy,
        r.forward[1],
        r.up[2],
        r.forward[2],
        r.up[1],
        r.up_x_times_qy,
    ]
    .into_iter()
    .enumerate()
    {
        sse::load(c, reg, word);
    }
    c.eax = frame::slot(c, g, 0x34)?;
    frame::push(c, g, 0)?;
    c.ecx = sp + 0x54;
    call(0xba1d0, 0x1277fd)
}

/// Determine whether the camera lies inside its cluster's acoustic region.
/// A region with a signed plane index uses the listener position to select
/// its side. The spatial helpers above return the cluster in CX.
fn inside_region(c: &mut CpuAbi, g: &Guest) -> Result<bool> {
    let cluster = c.ecx as i16;
    sse::zero(c, 0);
    if cluster == -1 {
        return Ok(false);
    }
    let bsp = g.u32(BSP)?;
    let clusters = g.u32(bsp.wrapping_add(0xa0))?;
    c.ebx = clusters;
    let record = clusters.wrapping_add((cluster as u32).wrapping_mul(0xb0));
    let region = g.u8(record.wrapping_add(0x70))?;
    if region == 0xff {
        return Ok(false);
    }
    let table = g.u32(bsp.wrapping_add(0x68))?;
    c.ebx = u32::from(region & 0x7f);
    let entry = table.wrapping_add(u32::from(region & 0x7f) * 24);
    if g.u16(entry.wrapping_add(2))? == u16::MAX {
        return Ok(false);
    }
    if region & 0x80 == 0 {
        return Ok(true);
    }
    sse::load(c, 1, g.u32(entry.wrapping_add(12))?);
    sse::ss(c, Mul, 1, frame::slot(c, g, 0x30)?);
    sse::load(c, 0, g.u32(entry.wrapping_add(8))?);
    sse::ss(c, Mul, 0, frame::slot(c, g, 0x2c)?);
    sse::ss_reg(c, Add, 1, 0);
    sse::load(c, 0, g.u32(entry.wrapping_add(4))?);
    sse::ss(c, Mul, 0, frame::slot(c, g, 0x28)?);
    sse::ss_reg(c, Add, 1, 0);
    let distance = sse::ss(c, Sub, 1, g.u32(entry.wrapping_add(16))?);
    sse::zero(c, 0);
    sse::comiss(c, 0, distance);
    Ok(!sse::below_or_equal(c))
}

fn publish(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let sp = c.esp;
    let row = c.ebp.wrapping_sub(0x20);
    g.set_u8(row.wrapping_add(6), 1)?;
    let inside = inside_region(c, g)?;
    g.set_u8(row.wrapping_add(7), u8::from(inside))?;
    let one = g.u32(ONE)?;
    sse::load(c, 7, one);
    let forward = axis(g, sp + 0x10)?;
    let up = axis(g, sp + 0x1c)?;
    put_axis(g, row.wrapping_add(0xc), forward)?;
    g.set_u32(row.wrapping_add(8), one)?;

    // The right axis is up × forward. Four operands are still in SSE
    // registers at this site, including on return from a spatial query.
    sse::movaps(c, 2, 4);
    sse::ss_reg(c, Mul, 2, 3);
    sse::movaps(c, 1, 6);
    sse::ss_reg(c, Mul, 1, 5);
    let right_x = sse::ss_reg(c, Sub, 1, 2);
    sse::load(c, 2, up[0]);
    sse::load(c, 1, forward[0]);
    sse::ss_reg(c, Mul, 4, 1);
    sse::ss_reg(c, Mul, 3, 2);
    sse::ss_reg(c, Mul, 5, 2);
    sse::load(c, 2, frame::slot(c, g, 0x58)?);
    sse::ss_reg(c, Mul, 6, 1);
    let right_y = sse::ss_reg(c, Sub, 4, 5);
    sse::load(c, 4, frame::slot(c, g, 0x54)?);
    let right_z = sse::ss_reg(c, Sub, 3, 6);
    sse::load(c, 3, frame::slot(c, g, 0x5c)?);
    put_axis(g, row.wrapping_add(0x18), [right_x, right_y, right_z])?;
    put_axis(g, row.wrapping_add(0x24), up)?;
    put_axis(g, row.wrapping_add(0x30), [sse::lane0(c, 0); 3])?;
    let position = axis(g, sp + 0x28)?;
    put_axis(g, row.wrapping_add(0x30), position)?;

    // Scale the velocity into listener space. The layer reads the scale
    // it just published: unordered comparison takes the division path.
    let scale = g.u32(row.wrapping_add(8))?;
    sse::load(c, 0, scale);
    let flags = sse::ucomiss(c, scale, one);
    sse::movaps(c, 1, 4);
    if flags & 0x40 == 0 || flags & 4 != 0 {
        sse::movaps(c, 0, 7);
        sse::ss(c, Div, 0, scale);
        sse::movaps(c, 1, 0);
        sse::ss_reg(c, Mul, 1, 4);
        sse::ss_reg(c, Mul, 2, 0);
        sse::ss_reg(c, Mul, 3, 0);
    }
    let vx = sse::lane0(c, 1);
    let vy = sse::lane0(c, 2);
    let vz = sse::lane0(c, 3);
    for (component, (basis, order)) in [(0xc, [1, 2, 0]), (0x18, [0, 1, 2]), (0x24, [1, 2, 0])]
        .into_iter()
        .enumerate()
    {
        let velocity = [vx, vy, vz];
        for (i, lane) in order.into_iter().enumerate() {
            let dst = if i == 0 {
                0
            } else if component == 2 {
                2
            } else {
                4
            };
            let v = g.u32(row.wrapping_add(basis + lane as u32 * 4))?;
            if component == 0 && i == 2 {
                // movaps xmm4,xmm1 / mulss xmm4,[forward.x]
                sse::movaps(c, dst, 1);
                sse::ss(c, Mul, dst, v);
            } else {
                sse::load(c, dst, v);
                sse::ss(c, Mul, dst, velocity[lane]);
            }
            if i != 0 {
                sse::ss_reg(c, Add, 0, dst);
            }
        }
        g.set_u32(
            row.wrapping_add(0x3c + 4 * component as u32),
            sse::lane0(c, 0),
        )?;
    }
    c.edi = frame::slot(c, g, 0x64)?;
    g.set_u32(row, frame::slot(c, g, 0x3c)?)?;
    c.edx = g.u32(points::SOUND_GLOBALS.base)?;
    g.set_u16(row.wrapping_add(4), c.ecx as u16)?;
    next_seat(c, g)
}

fn environments(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    // Both environment records are submitted together with their weights.
    let sp = c.esp;
    for (tag_off, dst) in [(0x1b0, 0x84), (0x1cc, 0x9c)] {
        let tag = g.u32(c.edx.wrapping_add(tag_off))?;
        let data = if tag == u32::MAX {
            arithmetic(c, tag, u32::MAX, true);
            NO_ENVIRONMENT
        } else {
            let entries = g.u32(points::TAG_ENTRIES.base)?;
            let index = (tag & 0xffff) << 4;
            alu::shl(c, tag & 0xffff, 4, 32);
            g.u32(entries.wrapping_add(index).wrapping_add(8))?
        };
        let weight = g.u32(c.edx.wrapping_add(tag_off - 4))?;
        sse::load(c, 0, weight);
        g.set_u32(sp + dst, data)?;
        for off in [0, 4, 8, 12] {
            let v = g.u32(c.edx.wrapping_add(tag_off + 4 + off))?;
            g.set_u32(sp + dst + 4 + off, v)?;
        }
        g.set_u32(sp + dst + 20, weight)?;
    }
    for (addr, off) in [
        (0x0046_87a8, 0xc0),
        (0x0046_87b0, 0xcc),
        (0x0046_8788, 0xb4),
        (0x0046_87a4, 0xd8),
    ] {
        let v = axis(g, g.u32(addr)?)?;
        put_axis(g, sp + off, v)?;
        c.ecx = v[2];
    }
    c.eax = sp + 0xb4;
    c.edx = sp + 0x84;
    frame::push(c, g, c.eax)?;
    g.set_u32(sp + 0xe4, 2)?;
    g.set_u32(sp + 0xe8, c.edx)?;
    call(0x220790, 0x127ce8)
}
