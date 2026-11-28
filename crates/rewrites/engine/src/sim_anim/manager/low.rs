//! Playback channels. A channel holds its graph and animation ID, play
//! flags, speed and current frame. The event masks are 16-bit accumulators.
use super::*;
use ::sim_anim::manager_channels::WrappedInterval;
use crate::function_eval::constant::{ONE, SMALL};
use guest_layout::points::TAG_ENTRIES;
use crate::sim_native::sub_flags;
use crate::sim_physics::{ARITH, arithmetic, byte, logic};

pub(super) const BODIES: &[Body] = &[
    body!(0x1c6440, 0x1c6467, &[0x1c6466], find_clip),
    body!(0x1c6470, 0x1c64f6, &[], select_channel),
    body!(0x1c6500, 0x1c6554, &[0x1c6526], stop_at_end),
    body!(
        0x1c6560,
        0x1c6611,
        &[0x1c6589, 0x1c65fe, 0x1c660d],
        set_frame
    ),
    body!(0x1c6620, 0x1c669d, &[0x1c6646], set_fraction),
    body!(
        0x1c66a0,
        0x1c68b3,
        &[
            0x1c66b9, 0x1c66e5, 0x1c6754, 0x1c67a1, 0x1c67eb, 0x1c680e, 0x1c6857, 0x1c68a9
        ],
        advance
    ),
    body!(0x1c68c0, 0x1c6920, &[0x1c691d], tick),
    body!(0x1c6920, 0x1c69a2, &[0x1c6946, 0x1c699f], advance_fraction),
    body!(0x1c69b0, 0x1c6ba3, &[0x1c69e1], event_mask),
];

const RESOLVE: u32 = 0x1daea0;
const DISPATCH_EVENTS: u32 = 0x1c7450;
const TIME_GLOBAL: u32 = 0x510c54;
const FRAME_RATE: u32 = 0x45dcfc;

type Checked<T> = std::result::Result<T, GuestFault>;

fn word(r: u32, v: u16) -> u32 {
    (r & !0xffff) | u32::from(v)
}

fn low32(c: &CpuAbi, r: usize) -> u32 {
    u128::from_le_bytes(c.xmm[r]) as u32
}

fn scalar(c: &mut CpuAbi, r: usize, v: u32) {
    c.xmm[r] = u128::from(v).to_le_bytes();
}

fn float(c: &mut CpuAbi, r: usize, i: u32) -> Checked<()> {
    let dest = u128::from_le_bytes(c.xmm[r]);
    c.xmm[r] = simd::cvtsi2ss(&mut c.mxcsr, dest, i)
        .map_err(|_| bad(c.eip))?
        .to_le_bytes();
    Ok(())
}

type Binary = fn(&mut u32, u128, u32) -> std::result::Result<u128, simd::Xm>;
fn binary(c: &mut CpuAbi, r: usize, v: u32, op: Binary) -> Checked<()> {
    c.xmm[r] = op(&mut c.mxcsr, u128::from_le_bytes(c.xmm[r]), v)
        .map_err(|_| bad(c.eip))?
        .to_le_bytes();
    Ok(())
}

fn comiss(c: &mut CpuAbi, a: u32, b: u32) -> Checked<u32> {
    let flags = simd::comiss(&mut c.mxcsr, a, b).map_err(|_| bad(c.eip))?;
    c.eflags = c.eflags & !ARITH | flags;
    c.eflags_defined |= ARITH;
    Ok(flags)
}

fn has_clip(c: &mut CpuAbi, g: &Bus<'_>, channel: u32) -> Checked<bool> {
    let index = g.u16(at(channel, 6))?;
    sub_flags(c, index.into(), u16::MAX.into(), 16);
    Ok(index != u16::MAX)
}

/// SHL of the graph index is the last integer arithmetic before resolution.
fn graph(c: &mut CpuAbi, g: &Bus<'_>, channel: u32, table_in_ecx: bool) -> Checked<()> {
    let index = (g.u32(channel)? & 0xffff) << 4;
    let table = g.u32(TAG_ENTRIES.base)?;
    if table_in_ecx {
        c.ecx = table;
    }
    logic(c, index, 1 << 31);
    c.eflags_defined &= !0x800;
    c.edx = g.u32(at(table, index + 8))?;
    Ok(())
}

fn resolve(
    c: &mut CpuAbi,
    g: &mut Bus<'_>,
    channel: u32,
    keep_ecx: bool,
    site: u32,
) -> sim_native::Result {
    graph(c, g, channel, !keep_ecx)?;
    c.eax = g.u32(at(channel, 4))?;
    push(c, g, c.eax)?;
    g.call(c, RESOLVE, site)
}

fn find_clip(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1c6440 {
        c.eax = 0;
        if has_clip(c, g, c.ecx)? {
            return resolve(c, g, c.ecx, true, 0x1c6466);
        }
    }
    g.ret(c, 0)
}

fn select_channel(c: &mut CpuAbi, g: &mut Bus<'_>, _: u32) -> sim_native::Result {
    push(c, g, c.esi)?;
    c.esi = c.ecx;
    c.ecx = u32::MAX;
    let valid = if c.esi == c.ecx {
        false
    } else {
        push(c, g, c.edi)?;
        let index = g.u32(at(c.esp, 12))? >> 16;
        c.edi = pop(c, g)?;
        index != 0xffff
    };
    if valid {
        let ch = c.eax;
        g.set_u16(at(ch, 4), u16::MAX)?;
        g.set_u16(at(ch, 6), u16::MAX)?;
        scalar(c, 0, 0);
        g.set_u8(at(ch, 0x10), 0)?;
        g.set_u8(at(ch, 0x11), 0)?;
        g.set_u16(at(ch, 0x14), 0)?;
        g.set_u16(at(ch, 0x16), 0)?;
        c.ecx = g.u32(at(c.esp, 12))?;
        g.set_u32(at(ch, 8), c.ecx)?;
        c.ecx = byte(c.ecx, g.u8(at(c.esp, 16))?);
        g.set_u8(at(ch, 12), c.ecx as u8)?;
        c.ecx = byte(c.ecx, g.u8(at(c.esp, 20))?);
        g.set_u16(at(ch, 0x12), c.edx as u16)?;
        g.set_u8(at(ch, 13), c.ecx as u8)?;
        c.ecx = word(c.ecx, g.u8(at(c.esp, 24))? as i8 as i16 as u16);
        g.set_u32(ch, c.esi)?;
        c.esi = g.u32(at(c.esp, 8))?;
        c.edx >>= 14;
        g.set_u32(at(ch, 0x1c), 0)?;
        scalar(c, 0, g.u32(ONE)?);
        c.edx = byte(c.edx, c.edx as u8 & 2);
        logic(c, c.edx & 255, 128);
        g.set_u32(at(ch, 4), c.esi)?;
        g.set_u32(at(ch, 0x18), low32(c, 0))?;
        g.set_u16(at(ch, 14), c.ecx as u16)?;
        g.set_u8(at(ch, 0x10), c.edx as u8)?;
        c.eax = byte(ch, 1);
    } else {
        c.eax = byte(c.eax, 0);
        logic(c, 0, 128);
    }
    c.esi = pop(c, g)?;
    g.ret(c, 20)
}

fn last_frame(c: &mut CpuAbi, g: &Bus<'_>) -> Checked<()> {
    c.ecx = (g.u16(at(c.eax, 0x14))? as i16 as i32).wrapping_sub(1) as u32;
    float(c, 0, c.ecx)?;
    binary(c, 0, g.u32(SMALL)?, simd::addss)
}

fn stop_at_end(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1c6500 {
        c.eax = 0;
        if has_clip(c, g, c.esi)? {
            return resolve(c, g, c.esi, false, 0x1c6526);
        }
    }
    scalar(c, 1, 0);
    last_frame(c, g)?;
    if comiss(c, 0, low32(c, 0))? == 0 {
        c.xmm[0] = c.xmm[1];
    }
    c.eax = byte(c.eax, g.u8(at(c.esi, 0x11))? | 10);
    logic(c, c.eax & 255, 128);
    g.set_u32(at(c.esi, 0x1c), low32(c, 0))?;
    g.set_u8(at(c.esi, 0x11), c.eax as u8)?;
    g.set_u8(at(c.esi, 0x10), 0)?;
    g.ret(c, 0)
}

fn set_frame(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if matches!(pc, 0x1c65fe | 0x1c660d) {
        c.esi = pop(c, g)?;
        return g.ret(c, 4);
    }
    if pc == 0x1c6560 {
        push(c, g, c.esi)?;
        c.esi = c.eax;
        c.eax = 0;
        if has_clip(c, g, c.esi)? {
            return resolve(c, g, c.esi, false, 0x1c6589);
        }
    }
    scalar(c, 0, 0);
    scalar(c, 1, g.u32(at(c.esp, 8))?);
    let negative = comiss(c, 0, low32(c, 1))? == 0;
    scalar(c, 2, g.u32(SMALL)?);
    if !negative {
        c.ecx = g.u16(at(c.eax, 0x14))? as i16 as u32;
        float(c, 0, c.ecx)?;
        binary(c, 0, low32(c, 2), simd::subss)?;
        if comiss(c, low32(c, 1), low32(c, 0))? != 0 {
            c.xmm[0] = c.xmm[1];
        }
    }
    let playing = g.u8(at(c.esi, 0x12))? & 1;
    logic(c, playing.into(), 128);
    g.set_u32(at(c.esi, 0x1c), low32(c, 0))?;
    if playing != 0 {
        c.ecx = byte(c.ecx, g.u8(at(c.esi, 0x11))?);
        let stopped = c.ecx & 9;
        logic(c, stopped, 128);
        if stopped == 0 {
            c.edx = g.u32(TIME_GLOBAL)?;
            scalar(c, 1, g.u32(at(c.edx, 4))?);
            binary(c, 1, g.u32(at(c.esi, 0x18))?, simd::mulss)?;
            c.eax = g.u16(at(c.eax, 0x14))? as i16 as u32;
            binary(c, 1, g.u32(FRAME_RATE)?, simd::mulss)?;
            binary(c, 1, low32(c, 0), simd::addss)?;
            float(c, 0, c.eax)?;
            binary(c, 0, low32(c, 2), simd::subss)?;
            let reached = comiss(c, low32(c, 1), low32(c, 0))? & 1 == 0;
            let flags = if reached {
                c.ecx as u8 | 2
            } else {
                c.ecx as u8 & !2
            };
            c.ecx = byte(c.ecx, flags);
            logic(c, flags.into(), 128);
            g.set_u8(at(c.esi, 0x11), flags)?;
            if reached {
                return g.call(c, 0x1c69b0, 0x1c65fe);
            }
        }
    }
    g.call(c, 0x1c69b0, 0x1c660d)
}

fn clamp_fraction(c: &mut CpuAbi, g: &mut Bus<'_>) -> Checked<()> {
    scalar(c, 2, 0);
    last_frame(c, g)?;
    c.xmm[1] = c.xmm[0];
    let arg = at(c.esp, 4);
    binary(c, 1, g.u32(arg)?, simd::mulss)?;
    if comiss(c, 0, low32(c, 1))? == 0 {
        g.set_u32(arg, 0)?;
    } else {
        let over = comiss(c, low32(c, 1), low32(c, 0))? == 0;
        g.set_u32(arg, low32(c, 0))?;
        if !over {
            g.set_u32(arg, low32(c, 1))?;
        }
    }
    Ok(())
}

fn set_fraction(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1c6620 {
        c.eax = 0;
        if has_clip(c, g, c.esi)? {
            return resolve(c, g, c.esi, false, 0x1c6646);
        }
    }
    clamp_fraction(c, g)?;
    let arg = at(c.esp, 4);
    c.edx = g.u32(arg)?;
    g.set_u32(arg, c.edx)?;
    c.eax = c.esi;
    Ok(TransferAbi::jump(0x1c6560))
}

fn tick(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1c691d {
        return g.ret(c, 12);
    }
    let delay = g.u8(at(c.eax, 0x10))?;
    c.ecx = byte(c.ecx, delay);
    c.edx = 0;
    sub_flags(c, delay.into(), 0, 8);
    g.set_u16(at(c.eax, 0x14), 0)?;
    g.set_u16(at(c.eax, 0x16), 0)?;
    if delay != 0 {
        // DEC preserves CF, here clear from CMP delay,0.
        sub_flags(c, delay.into(), 1, 8);
        c.ecx = byte(c.ecx, delay - 1);
        g.set_u8(at(c.eax, 0x10), delay - 1)?;
        return g.ret(c, 12);
    }
    let playing = g.u8(at(c.eax, 0x12))? & 1;
    logic(c, playing.into(), 128);
    if playing != 0 {
        let stopped = g.u8(at(c.eax, 0x11))? & 9;
        logic(c, stopped.into(), 128);
        if stopped == 0 {
            c.ecx = g.u32(at(c.esp, 12))?;
            c.edx = g.u32(at(c.esp, 8))?;
            push(c, g, c.ecx)?;
            c.ecx = g.u32(at(c.esp, 8))?;
            push(c, g, c.edx)?;
            c.edx = g.u32(TIME_GLOBAL)?;
            scalar(c, 0, g.u32(at(c.edx, 4))?);
            binary(c, 0, g.u32(at(c.eax, 0x18))?, simd::mulss)?;
            binary(c, 0, g.u32(FRAME_RATE)?, simd::mulss)?;
            binary(c, 0, g.u32(at(c.eax, 0x1c))?, simd::addss)?;
            push(c, g, c.ecx)?;
            push(c, g, c.ecx)?;
            g.set_u32(c.esp, low32(c, 0))?;
            push(c, g, c.eax)?;
            return g.call(c, 0x1c66a0, 0x1c691d);
        }
    }
    g.ret(c, 12)
}

fn advance_fraction(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1c699f {
        return g.ret(c, 16);
    }
    if pc == 0x1c6920 {
        c.eax = 0;
        if has_clip(c, g, c.esi)? {
            return resolve(c, g, c.esi, false, 0x1c6946);
        }
    }
    g.set_u8(at(c.esi, 0x10), 0)?;
    clamp_fraction(c, g)?;
    c.edx = g.u32(at(c.esp, 16))?;
    c.eax = g.u32(at(c.esp, 12))?;
    c.ecx = g.u32(at(c.esp, 8))?;
    push(c, g, c.edx)?;
    c.edx = g.u32(at(c.esp, 8))?;
    for arg in [c.eax, c.ecx, c.edx, c.esi] {
        push(c, g, arg)?;
    }
    g.call(c, 0x1c66a0, 0x1c699f)
}

fn finish_advance(c: &mut CpuAbi, g: &Bus<'_>) -> sim_native::Result {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 12, false);
    g.ret(c, 20)
}

fn same_slot(c: &mut CpuAbi, g: &Bus<'_>) -> Checked<bool> {
    let slot = g.u16(at(c.ebp, 4))?;
    sub_flags(c, c.edi, slot.into(), 16);
    Ok(c.edi as u16 == slot)
}

fn same_index(c: &mut CpuAbi, g: &Bus<'_>, use_eax: bool) -> Checked<bool> {
    let saved = g.u16(at(c.esp, 0x12))?;
    if use_eax {
        c.eax = word(c.eax, saved);
    } else {
        c.ecx = word(c.ecx, saved);
    }
    let index = g.u16(at(c.ebp, 6))?;
    sub_flags(c, saved.into(), index.into(), 16);
    Ok(saved == index)
}

fn merge_events(c: &CpuAbi, g: &mut Bus<'_>) -> Checked<()> {
    let p = at(c.ebp, 0x14);
    g.set_u16(p, g.u16(p)? | c.eax as u16)
}

fn commit_frame(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    if same_slot(c, g)? && same_index(c, g, true)? {
        c.ecx = g.u32(at(c.esp, 0x20))?;
        push(c, g, c.ecx)?;
        c.eax = c.ebp;
        g.call(c, 0x1c6560, 0x1c68a9)
    } else {
        finish_advance(c, g)
    }
}

fn first_interval(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    let sp = c.esp;
    scalar(c, 0, g.u32(at(c.ebp, 0x1c))?);
    scalar(c, 1, g.u32(at(sp, 0x24))?);
    c.eax = 0;
    g.set_u16(at(c.ebp, 0x14), 0)?;
    g.set_u16(at(c.ebp, 0x16), 0)?;
    c.ecx = g.u16(at(c.esi, 0x14))? as i16 as u32;
    g.set_u32(at(sp, 0x14), low32(c, 0))?;
    float(c, 0, c.ecx)?;
    binary(c, 0, g.u32(SMALL)?, simd::subss)?;
    g.set_u32(at(sp, 0x24), low32(c, 0))?;
    scalar(c, 0, 0);
    let negative = comiss(c, 0, low32(c, 1))? == 0;
    g.set_u32(at(sp, 0x20), low32(c, 1))?;
    if negative {
        g.set_u32(at(sp, 0x20), 0)?;
    }
    c.ecx = byte(c.ecx, g.u8(at(c.ebp, 0x11))?);
    c.edx = g.u32(at(sp, 0x30))?;
    c.eax = g.u32(at(sp, 0x20))?;
    push(c, g, c.edx)?;
    c.edx = u32::from(g.u16(at(c.ebp, 0x12))?);
    c.ecx = byte(c.ecx, c.ecx as u8 & !14);
    logic(c, c.ecx & 255, 128);
    push(c, g, c.eax)?;
    g.set_u8(at(c.ebp, 0x11), c.ecx as u8)?;
    c.ecx = g.u32(at(c.esp, 0x1c))?;
    for arg in [c.ecx, c.edx, c.esi, c.ebx] {
        push(c, g, arg)?;
    }
    c.ebx = g.u32(at(c.esp, 0x44))?;
    g.call(c, DISPATCH_EVENTS, 0x1c6754)
}

fn wrapped_interval(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    let loop_frame = g.u16(at(c.esi, 0x2e))? as i16;
    c.eax = loop_frame as u32;
    let interval = WrappedInterval::new(
        g.u32(at(c.esp, 0x20))?, g.u32(at(c.esp, 0x24))?, loop_frame, &mut c.mxcsr,
    );
    c.ecx = g.u32(at(c.esp, 0x30))?;
    c.ebx = g.u32(at(c.esp, 0x2c))?;
    scalar(c, 0, interval.end);
    c.xmm[1][..4].copy_from_slice(&interval.start.to_le_bytes());
    push(c, g, c.ecx)?;
    c.ecx = u32::from(g.u16(at(c.ebp, 0x12))?);
    g.set_u32(at(c.esp, 0x24), interval.end)?;
    g.set_u32(at(c.esp, 0x18), interval.start)?;
    c.edx = g.u32(at(c.esp, 0x24))?;
    c.eax = g.u32(at(c.esp, 0x18))?;
    push(c, g, c.edx)?;
    c.edx = g.u32(at(c.esp, 0x20))?;
    for arg in [c.eax, c.ecx, c.esi, c.edx] {
        push(c, g, arg)?;
    }
    g.call(c, DISPATCH_EVENTS, 0x1c6857)
}

fn advance(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1c66a0 => {
            c.esp = arithmetic(c, c.esp, 12, true);
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = g.u32(at(c.esp, 0x18))?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = g.u32(at(c.ebp, 4))?;
            c.ecx = c.ebp;
            g.set_u32(at(c.esp, 0x10), c.edi)?;
            g.call(c, 0x1c6bb0, 0x1c66b9)
        }
        0x1c66b9 => {
            c.ebx = c.eax;
            c.esi = 0;
            let valid = has_clip(c, g, c.ebp)?;
            g.set_u32(at(c.esp, 0x18), c.ebx)?;
            if valid {
                graph(c, g, c.ebp, true)?;
                c.eax = c.edi;
                push(c, g, c.eax)?;
                return g.call(c, RESOLVE, 0x1c66e5);
            }
            first_interval(c, g)
        }
        0x1c66e5 => {
            c.esi = c.eax;
            first_interval(c, g)
        }
        0x1c6754 => {
            if !same_slot(c, g)? || !same_index(c, g, false)? {
                return finish_advance(c, g);
            }
            merge_events(c, g)?;
            scalar(c, 0, g.u32(at(c.esp, 0x20))?);
            if comiss(c, low32(c, 0), g.u32(at(c.esp, 0x24))?)? & 1 != 0 {
                return commit_frame(c, g);
            }
            c.edx = g.u32(at(c.esp, 0x30))?;
            c.eax = g.u32(at(c.esp, 0x14))?;
            c.ecx = u32::from(g.u16(at(c.ebp, 0x12))?);
            push(c, g, c.edx)?;
            c.edx = g.u32(at(c.esp, 0x1c))?;
            let end = g.u32(0x1c6794)?; // upper bound carried by the image
            for arg in [end, c.eax, c.ecx, c.esi, c.edx] {
                push(c, g, arg)?;
            }
            g.call(c, DISPATCH_EVENTS, 0x1c67a1)
        }
        0x1c67a1 => {
            if !same_slot(c, g)? {
                return finish_advance(c, g);
            }
            if !same_index(c, g, false)? {
                return commit_frame(c, g);
            }
            merge_events(c, g)?;
            c.eax = word(c.eax, g.u16(at(c.ebp, 0x12))?);
            c.edx = byte(c.edx, (c.eax as u8) >> 1);
            if c.edx & 1 == 0 {
                let p = at(c.ebp, 0x11);
                g.set_u8(p, g.u8(p)? | 8)?;
                c.edx = g.u16(at(c.esi, 0x14))? as i16 as u32;
                float(c, 0, c.edx)?;
                binary(c, 0, g.u32(SMALL)?, simd::subss)?;
                g.set_u32(at(c.esp, 0x20), low32(c, 0))?;
                return commit_frame(c, g);
            }
            if c.eax & 0x20 != 0 {
                let loop_frame = g.u16(at(c.esi, 0x2e))?;
                sub_flags(c, loop_frame.into(), 0, 16);
                if loop_frame == 0 {
                    c.eax = g.u32(at(c.ebp, 4))?;
                    c.edi = g.u32(at(c.esp, 0x28))?;
                    push(c, g, c.eax)?;
                    c.esi = at(c.esp, 0x2c);
                    return g.call(c, 0x1cb3b0, 0x1c67eb);
                }
            } else {
                logic(c, 0, 128);
            }
            wrapped_interval(c, g)
        }
        0x1c67eb => {
            c.edi = g.u32(c.eax)?;
            let reset_speed = g.u8(at(c.ebp, 0x12))? & 8;
            logic(c, reset_speed.into(), 128);
            g.set_u32(at(c.ebp, 4), c.edi)?;
            if reset_speed != 0 {
                scalar(c, 0, g.u32(ONE)?);
                g.set_u32(at(c.ebp, 0x18), low32(c, 0))?;
            }
            c.ecx = c.ebp;
            g.set_u32(at(c.esp, 0x10), c.edi)?;
            g.call(c, 0x1c6440, 0x1c680e)
        }
        0x1c680e => {
            c.esi = c.eax;
            wrapped_interval(c, g)
        }
        0x1c6857 => {
            if !same_slot(c, g)? {
                return finish_advance(c, g);
            }
            if same_index(c, g, false)? {
                merge_events(c, g)?;
                let p = at(c.ebp, 0x11);
                g.set_u8(p, g.u8(p)? | 4)?;
            }
            commit_frame(c, g)
        }
        0x1c68a9 => finish_advance(c, g),
        _ => Err(bad(pc)),
    }
}

/// Scan one 4-byte event. The unrolled scan uses EAX for the first three
/// frame numbers. Its fourth record and its scalar remainder use EDX and
/// leave the opposite interval's bit mask in a different register.
fn check_event(c: &mut CpuAbi, g: &mut Bus<'_>, record: u32, frame_in_edx: bool) -> Checked<()> {
    let current = g.u32(at(c.esi, 0x1c))?;
    let lhs = low32(c, 0);
    let relation = simd::ucomiss(&mut c.mxcsr, lhs, current).map_err(|_| bad(c.eip))?;
    let frame = g.u16(at(record, 2))? as i16 as u32;
    if frame_in_edx {
        c.edx = frame;
    } else {
        c.eax = frame;
    }
    c.ecx = record;
    float(c, 1, frame)?;
    // LAHF, TEST AH,44h: equality selects (start, end]; every other
    // comparison selects [start, end), including an unordered one.
    c.eax = (c.eax & !0xff00) | ((relation | 2) << 8);
    let include_end = relation == 0x40;
    let lo = comiss(c, low32(c, 1), low32(c, 0))?;
    if (lo & 1) != 0 || (include_end && lo != 0) {
        return Ok(());
    }
    let hi = comiss(c, low32(c, 2), low32(c, 1))?;
    if (hi & 1) != 0 || (!include_end && hi != 0) {
        return Ok(());
    }
    c.ecx = byte(c.ecx, g.u8(record)?);
    let mask = 1u32 << (c.ecx & 31);
    if frame_in_edx && !include_end {
        c.edx = mask;
    } else {
        c.eax = mask;
    }
    let p = at(c.esi, 0x16);
    g.set_u16(p, g.u16(p)? | mask as u16)
}

fn event_mask(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1c69b0 {
        push(c, g, c.ecx)?;
        push(c, g, c.ebx)?;
        c.ebx = 0;
        let valid = has_clip(c, g, c.esi)?;
        g.set_u16(at(c.esi, 0x16), 0)?;
        if valid {
            graph(c, g, c.esi, true)?;
            c.eax = g.u32(at(c.esi, 4))?;
            push(c, g, c.edi)?;
            push(c, g, c.eax)?;
            return g.call(c, RESOLVE, 0x1c69e1);
        }
    } else {
        scalar(c, 0, g.u32(at(c.esi, 0x18))?);
        scalar(c, 1, 0);
        let forward = comiss(c, low32(c, 0), 0)? == 0;
        scalar(c, 2, g.u32(at(c.esi, 0x1c))?);
        c.edi = c.eax;
        if forward {
            c.ecx = g.u16(at(c.edi, 0x14))? as i16 as u32;
            float(c, 1, c.ecx)?;
            binary(c, 1, g.u32(ONE)?, simd::addss)?;
        }
        let upper = comiss(c, low32(c, 2), low32(c, 1))? == 0;
        c.xmm[0] = if upper { c.xmm[1] } else { c.xmm[2] };
        if comiss(c, low32(c, 2), low32(c, 1))? != 0 {
            c.xmm[2] = c.xmm[1];
        }
        push(c, g, c.ebp)?;
        c.ebp = g.u32(at(c.edi, 0x4c))?;
        g.set_u32(at(c.esp, 12), c.ebp)?;
        if (c.ebp as i32) >= 4 {
            c.ebp = c.ebp.wrapping_sub(3);
            loop {
                c.edx = c.ebx.wrapping_mul(4);
                for offset in [0, 4, 8, 12] {
                    let table = g.u32(at(c.edi, 0x50))?;
                    let record = at(at(table, c.edx), offset);
                    check_event(c, g, record, offset == 12)?;
                }
                c.ebx = at(c.ebx, 4);
                if c.ebx as i32 >= c.ebp as i32 {
                    break;
                }
            }
            c.ebp = g.u32(at(c.esp, 12))?;
        }
        while (c.ebx as i32) < c.ebp as i32 {
            c.eax = g.u32(at(c.edi, 0x50))?;
            check_event(c, g, at(c.eax, c.ebx.wrapping_mul(4)), true)?;
            c.ebx = at(c.ebx, 1);
        }
        sub_flags(c, c.ebx, c.ebp, 32);
        c.ebp = pop(c, g)?;
        c.edi = pop(c, g)?;
    }
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    g.ret(c, 0)
}
