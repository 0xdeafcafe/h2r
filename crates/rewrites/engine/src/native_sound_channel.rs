//! One channel's pitch, gain, stream submission and next-chunk scheduling.
//! The submitted descriptors live in the guest frame through both the mixer
//! calls and their possible completion callbacks. Resumes reload the instance
//! and the permutation at the same points as the sound driver.

use crate::points;
use crate::sim_objects::{Body, Cx, Next, Result, alu, frame, sse};
use crate::sim_objects::sse::Ss::{Add, Mul, Sub};
use crate::sim_physics::{arithmetic, byte, logic};
use crate::sim_objects::projectile_step::store_f32;
use runtime::{CpuAbi, Guest};
use sim_sound::channel::ChannelLevels;

const GESTALT: u32 = 0x51ebd4;
const COMPANIONS: u32 = 0x51ebd8;
const GAIN_FLOOR: u32 = 0x44a0b4;
const GAIN_CEILING: u32 = 0x45dbd8;

pub const BODIES: &[Body] = &[Body {
    point: &points::SOUND_CHANNEL_UPDATE,
    sites: &[0x128d9c, 0x128db0, 0x128e2e, 0x128e7a, 0x128f16, 0x128f23,
             0x128f36, 0x128fe8, 0x129016, 0x12902d, 0x12907c, 0x129082],
    run: update,
}];

fn index(base: u32, row: u32, stride: u32) -> u32 {
    base.wrapping_add(row.wrapping_mul(stride))
}

fn sw(v: u16) -> u32 { v as i16 as i32 as u32 }
fn sb(v: u8) -> u32 { v as i8 as i32 as u32 }
fn word(r: u32, v: u32) -> u32 { (r & 0xffff_0000) | (v & 0xffff) }

fn call(target: u32, site: u32) -> Result<Next> { Ok(Next::Call { target, site }) }

fn clear_descriptor(c: &mut CpuAbi, g: &mut Guest, start: u32, words: u32) -> Result<()> {
    // The two descriptor headers are cleared separately; REP STOSD observes DF.
    g.set_u32(start, 0)?;
    c.edi = start.wrapping_add(4);
    if c.eflags & 0x400 == 0 {
        let bytes = [0; 0x444];
        g.write_bytes(c.edi, &bytes[..words as usize * 4], 1)?;
        c.edi = c.edi.wrapping_add(words * 4);
    } else {
        for _ in 0..words {
            g.set_u32(c.edi, 0)?;
            c.edi = c.edi.wrapping_sub(4);
        }
    }
    Ok(())
}

fn enter(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    frame::push(c, g, c.ebp)?;
    c.ebp = c.esp;
    c.esp = (c.esp & !7).wrapping_sub(0x494);
    let offset = sw(g.u16(c.ebp + 8)?).wrapping_mul(0x24);
    g.set_u32(c.esp + 0x14, offset)?;
    let channel = g.u32(points::SOUND_CHANNELS.base)?.wrapping_add(offset);
    let pool = g.u32(points::SOUND_INSTANCES.base)?;
    let rows = g.u32(points::TAG_ENTRIES.base)?;
    frame::push(c, g, c.ebx)?;
    let owner = g.u32(channel)? & 0xffff;
    frame::push(c, g, c.esi)?;
    g.set_u32(c.esp + 0x10, channel)?;
    c.ebx = index(g.u32(pool + 0x44)?, owner, 0xbc);
    c.esi = g.u32(index(rows, g.u32(c.ebx + 0xc)? & 0xffff, 16) + 8)?;
    frame::push(c, g, c.edi)?;
    let sp = c.esp;
    logic(c, 0, 1 << 31);
    clear_descriptor(c, g, sp + 0x28, 11)?;
    clear_descriptor(c, g, sp + 0x58, 0x111)?;
    let class = sb(g.u8(c.esi + 0xb)?);
    let gestalt = g.u32(GESTALT)?;
    let bounds = index(g.u32(gestalt + 0xc)?, class, 20);
    c.ecx = g.u32(c.ebp + 12)?;
    let input = ChannelLevels {
        pitch_range: [g.u16(bounds + 8)? as i16, g.u16(bounds + 10)? as i16],
        fraction: g.u32(c.ebx + 0x1c)?,
        pitch_offset: g.u32(c.ebx + 0x90)?,
        gain: g.u32(c.ebx + 0x20)?,
        gain_offset: g.u32(c.ebx + 0x94)?,
        gain_delta: c.ecx,
    };
    let levels = input.evaluate(&mut c.mxcsr);
    sse::load(c, 0, levels.gain_offset);
    sse::load(c, 1, levels.gain);
    sse::load(c, 2, input.fraction);
    g.set_u32(sp + 0x10, levels.pitch)?;
    g.set_u32(sp + 0x24, input.fraction)?;
    g.set_u32(sp + 0x0c, levels.gain)?;
    c.edx = input.fraction;
    c.eax = levels.gain;
    frame::push(c, g, c.edx)?;
    frame::push(c, g, c.eax)?;
    g.set_u32(sp + 0x18, c.ebx)?;
    g.set_u32(sp + 0x1c, c.esi)?;
    call(0x1251e0, 0x128d9c)
}

fn permutation(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = c.esi;
    c.esi = g.u32(c.esp + 0x18)?;
    let gestalt = g.u32(GESTALT)?;
    let range_index = sw(g.u16(c.edx + 8)?).wrapping_add(sb(g.u8(c.esi + 0x9c)?));
    let range = index(g.u32(gestalt + 0x24)?, range_index, 12);
    let permutation = sw(g.u16(range + 8)?).wrapping_add(sb(g.u8(c.esi + 0x9d)?));
    g.set_u16(c.esp + 0x5c, 0)?;
    c.edi = index(g.u32(gestalt + 0x2c)?, permutation, 16);
    c.ebx = g.u32(c.esi + 0xa8)?;
    g.set_u32(c.esp + 0x54, c.ebx)?;
    c.eax = g.u32(gestalt + 0x1c)?;
    c.ecx = sw(g.u16(index(c.eax, sw(g.u16(range + 2)?), 10))?);
    sse::load(c, 0, g.u32(c.esp + 0x10)?);
    sse::cvtsi2ss(c, 1, c.ecx);
    let pitch = sse::ss_reg(c, Sub, 0, 1);
    g.set_u32(c.esp + 0x30, pitch)?;
    let adjust = g.u8(c.esi + 6)? & 2;
    logic(c, u32::from(adjust), 0x80);
    g.set_u32(c.esp + 0x18, c.edi)?;
    if adjust == 0 {
        return submit(c, g);
    }
    c.edx = word(c.edx, sb(g.u8(c.edx + 2)?));
    frame::push(c, g, c.edx)?;
    call(0x221810, 0x128e2e)
}

fn adjust(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ebx = c.eax;
    c.eax = g.u32(c.esi + 0xa8)?;
    arithmetic(c, c.eax, u32::MAX, true);
    if c.eax == u32::MAX {
        c.eax = sb(g.u8(c.esi + 0xa1)?);
        c.edx = c.esi + 0x24;
        return call(0x12a9d0, 0x128e7a);
    }
    c.ecx = g.u32(COMPANIONS)?;
    c.edx = g.u32(c.ecx + 0x44)?;
    c.eax = index(c.edx, c.eax & 0xffff, 0x1c);
    c.ecx = g.u32(c.eax + 4)?;
    if c.ecx == u32::MAX {
        sse::zero(c, 0);
    } else {
        c.edx = sb(g.u8(c.esi + 0xa1)?);
        sse::load(c, 0, g.u32(index(c.eax, c.edx, 4) + 0xc)?);
    }
    apply_adjustment(c, g)
}

fn apply_adjustment(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    sse::load(c, 1, g.u32(c.ebx + 0x48)?);
    c.eax = g.u32(c.ebp + 16)?;
    sse::ss(c, Mul, 1, g.u32(c.eax + 20)?);
    sse::ss_reg(c, Mul, 1, 0);
    let pitch = sse::ss(c, Add, 1, g.u32(c.esp + 0x30)?);
    g.set_u32(c.esp + 0x30, pitch)?;
    submit(c, g)
}

// Lower saturation is an immediate chosen by the guest, upper saturation
// clears the value; unordered comparisons leave the input NaN alone.
fn volume(c: &mut CpuAbi, g: &mut Guest, delta: u32, floor_imm: u32) -> Result<u32> {
    let at = c.esp + 0xc;
    let floor = g.u32(GAIN_FLOOR)?;
    sse::load(c, 1, floor);
    sse::cvtsi2ss(c, 0, delta);
    let sum = sse::ss(c, Add, 0, g.u32(at)?);
    g.set_u32(at, sum)?;
    g.set_u32(c.esp + 0x10, sum)?;
    sse::load(c, 0, sum);
    if sse::comiss(c, floor, sum) & 0x41 == 0 {
        return g.u32(floor_imm);
    }
    if sse::comiss(c, sum, g.u32(GAIN_CEILING)?) & 0x41 == 0 {
        logic(c, 0, 1 << 31);
        return Ok(0);
    }
    Ok(sum)
}

fn submit(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = u32::from(g.u16(c.esi + 4)?) >> 9;
    logic(c, c.ecx & 1, 0x80);
    if c.ecx & 1 == 0 {
        c.edx = sb(g.u8(c.edi + 4)?);
        let gain = volume(c, g, c.edx, 0x128eeb)?;
        c.ecx = g.u32(c.esp + 0x14)?;
        g.set_u32(c.esp + 0x34, gain)?;
        c.edx = sw(g.u16(c.ecx + 0xc)?);
        let sp = c.esp;
        frame::push(c, g, c.edx)?;
        c.ebx = sp + 0x70;
        c.eax = sp + 0x28;
        return call(0x21f8a0, 0x128f16);
    }
    c.edx = g.u32(c.esp + 0x14)?;
    c.eax = g.u32(c.edx + 0x1c)?;
    logic(c, c.eax, 1 << 31);
    if c.eax == 0 {
        return check_chunk(c, g);
    }
    let offset = sb(g.u8(c.eax + 4)?);
    let gain = volume(c, g, offset, 0x128fba)?;
    g.set_u32(c.esp + 0x34, gain)?;
    c.eax = g.u32(c.esp + 0x14)?;
    c.ecx = sw(g.u16(c.eax + 0xc)?);
    let sp = c.esp;
    frame::push(c, g, 1)?;
    c.edx = sp + 0x70;
    frame::push(c, g, c.edx)?;
    frame::push(c, g, c.ecx)?;
    c.edi = sp + 0x28;
    call(0x21fa80, 0x128fe8)
}

fn submit_chunk(c: &mut CpuAbi, g: &mut Guest, site: u32) -> Result<Next> {
    c.eax = g.u32(c.ebp + 8)?;
    logic(c, 0, 1 << 31);
    c.ecx = u32::from(g.u16(c.esi + 0x9e)?);
    frame::push(c, g, c.ecx)?;
    frame::push(c, g, c.edi)?;
    call(0x129f20, site)
}

fn submitted(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(c.esi + 0xa8)?;
    c.edx = u32::MAX;
    if c.eax != u32::MAX {
        c.ecx = g.u32(COMPANIONS)?;
        c.ebx = g.u32(c.ecx + 0x44)?;
        c.eax = index(c.ebx, c.eax & 0xffff, 0x1c);
        c.ecx = g.u32(c.eax + 4)?;
        if c.ecx != u32::MAX {
            let old = g.u8(c.eax + 3)?;
            let next = old.wrapping_sub(1) & 0x7f;
            c.edx = byte(c.edx, old & 0x7f);
            c.ecx = byte(c.ecx, next);
            g.set_u8(c.eax + 3, next)?;
        }
        g.set_u8(c.esi + 3, c.edx as u8)?;
    }
    alu::rmw8(c, g, c.esi + 5, |v| v | 2)?;
    check_chunk(c, g)
}

fn check_chunk(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = word(c.eax, g.u16(c.esi + 4)?.into());
    let flags = c.eax as u8;
    c.edx = byte(c.edx, flags >> 6);
    logic(c, c.edx & 1, 0x80);
    if c.edx & 1 != 0 { return leave(c, g); }
    c.eax = byte(c.eax, flags >> 3);
    logic(c, c.eax & 1, 0x80);
    if c.eax & 1 != 0 {
        c.eax = g.u32(c.esp + 0x14)?;
        c.ecx = g.u32(c.eax + 0x20)?;
        logic(c, c.ecx, 1 << 31);
        if c.ecx == 0 {
            frame::push(c, g, c.esi)?;
            return call(0x125e60, 0x129016);
        }
    }
    next_chunk(c, g)
}

fn next_chunk(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = byte(c.edx, g.u8(c.esi + 4)? >> 3);
    logic(c, c.edx & 1, 0x80);
    if c.edx & 1 != 0 { return leave(c, g); }
    logic(c, c.edi, 1 << 31);
    c.eax = word(c.eax, g.u16(c.esi + 0x9e)?.into());
    if c.edi != 0 {
        c.eax = sw(c.eax as u16);
        c.ecx = sw(g.u16(c.edi + 0xe)?).wrapping_sub(1);
        arithmetic(c, c.eax, c.ecx, true);
        if (c.eax as i32) < (c.ecx as i32) {
            c.eax = c.eax.wrapping_add(1);
            alu::cmp16(c, c.eax, 0xffff);
            if c.eax as u16 != u16::MAX {
                c.edx = word(c.edx, sb(g.u8(c.esi + 0x9d)?));
                c.ecx = g.u32(c.esi + 0xc)?;
                frame::push(c, g, c.eax)?;
                c.eax = word(c.eax, sb(g.u8(c.esi + 0x9c)?));
                frame::push(c, g, c.edx)?;
                c.edx = g.u32(c.esp + 0x1c)?;
                frame::push(c, g, c.eax)?;
                c.eax = g.u32(c.edx)?;
                frame::push(c, g, c.ecx)?;
                return call(0x1286b0, 0x12907c);
            }
        }
    }
    alu::rmw8(c, g, c.esi + 4, |v| v | 0x40)?;
    let offset = g.u32(c.esp + 0x20)?;
    c.ecx = g.u32(points::SOUND_CHANNELS.base)?;
    c.edx = sw(g.u16(offset.wrapping_add(c.ecx) + 0xc)?).wrapping_mul(0x34);
    c.eax = g.u32(points::SOUND_DRIVER.base)?.wrapping_add(c.edx).wrapping_add(0xc);
    let flags = g.u8(c.eax + 3)? | 8;
    c.ecx = byte(c.ecx, flags);
    logic(c, flags.into(), 0x80);
    g.set_u8(c.eax + 3, flags)?;
    leave(c, g)
}

fn leave(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = frame::pop(c, g)?;
    c.esi = frame::pop(c, g)?;
    c.ebx = frame::pop(c, g)?;
    c.esp = c.ebp;
    c.ebp = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 12 })
}

fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x128ca0 => enter(c, g),
        0x128d9c => {
            g.set_u32(c.esp + 0xc, c.eax)?;
            c.eax = g.u32(c.ebp + 8)?;
            frame::push(c, g, c.ebx)?;
            c.ebx = g.u32(c.ebp + 16)?;
            c.edi = c.esp + 0x2c;
            call(0x12a1b0, 0x128db0)
        }
        0x128db0 => permutation(c, g),
        0x128e2e => adjust(c, g),
        0x128e7a => {
            let at = c.esp + 0x10;
            store_f32(c, g, pc, at, true)?;
            sse::load(c, 0, g.u32(at)?);
            apply_adjustment(c, g)
        }
        0x128f16 => {
            c.eax = c.esp + 0x490;
            frame::push(c, g, c.eax)?;
            call(0x21f720, 0x128f23)
        }
        0x128f23 => submit_chunk(c, g, 0x128f36),
        0x128f36 => submitted(c, g),
        0x128fe8 => { c.edi = g.u32(c.esp + 0x18)?; check_chunk(c, g) }
        0x129016 => {
            logic(c, c.eax & 0xff, 0x80);
            if c.eax as u8 == 0 { return next_chunk(c, g); }
            submit_chunk(c, g, 0x12902d)
        }
        0x12902d => {
            alu::rmw8(c, g, c.esi + 4, |v| v & !8)?;
            next_chunk(c, g)
        }
        0x12907c => {
            frame::push(c, g, c.esi)?;
            call(0x125e60, 0x129082)
        }
        0x129082 => {
            alu::rmw8(c, g, c.esi + 4, |v| v | 8)?;
            leave(c, g)
        }
        _ => unreachable!("channel update entry or resume"),
    }
}
