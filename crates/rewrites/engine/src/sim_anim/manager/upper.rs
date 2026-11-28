//! Channel motion, sample masks and the three pose evaluation calls.
use super::{Body, Bus};
use ::sim_anim::manager_channels::{playback_rate, PlaybackRate};
use guest_layout::points::{self, TAG_ENTRIES};
use crate::sim_native::Memory;
use crate::sim_physics::{arithmetic, logic, pop, push};
use runtime::{CpuAbi, GuestFault, TransferAbi};

mod events;

pub(super) const BODIES: &[Body] = &[
    body!(0x1c6bb0, 0x1c6bfe, &[], graph),
    body!(0x1c6c00, 0x1c6c75, &[0x1c6c3a], velocity),
    body!(0x1c6c80, 0x1c6cf4, &[0x1c6cdb, 0x1c6cea], sample_velocity),
    body!(0x1c6d00, 0x1c6db2, &[0x1c6d68], root_motion),
    body!(0x1c6dc0, 0x1c6e2c, &[0x1c6e13, 0x1c6e25], sample_root),
    body!(0x1c6e30, 0x1c6e99, &[0x1c6e5d], progress),
    body!(0x1c6ea0, 0x1c6ed9, &[0x1c6ec8], duration),
    body!(0x1c6ee0, 0x1c6f54, &[0x1c6f13], event_time),
    body!(
        0x1c6f60,
        0x1c70f7,
        &[0x1c6f8c, 0x1c6fb8, 0x1c7073, 0x1c70b2, 0x1c70ee],
        base_pose
    ),
    body!(
        0x1c7100,
        0x1c71f2,
        &[0x1c7129, 0x1c7187, 0x1c71b1, 0x1c71ec],
        blend_pose
    ),
    body!(
        0x1c7200,
        0x1c72f3,
        &[0x1c7275, 0x1c72b4, 0x1c72ec],
        additive_pose
    ),
    body!(0x1c7300, 0x1c7335, &[0x1c732d], track_flag),
    body!(0x1c7340, 0x1c7378, &[0x1c736d], track_flag),
    body!(0x1c7380, 0x1c739f, &[0x1c739e], view),
    body!(0x1c73a0, 0x1c744a, &[0x1c73cf, 0x1c742f], view),
    body!(
        0x1c7450,
        0x1c7786,
        &[0x1c751f, 0x1c7643, 0x1c7764],
        events::run
    ),
];

type Checked<T> = std::result::Result<T, GuestFault>;
type Transfer = Checked<TransferAbi>;

const ONE: u32 = crate::function_eval::constant::ONE;
const ZERO: u32 = crate::function_eval::constant::ZERO;
const FRAME_SCALE: u32 = 0x45dc6c;
const MOTION_SCALE: u32 = 0x45dcfc;
const TRACK: u32 = 0x1daea0;
const RESOURCE: u32 = 0x1ddb40;
const SAMPLE_MASK: u32 = 0x55e590;

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}
fn w16(reg: u32, v: u16) -> u32 {
    (reg & !0xffff) | u32::from(v)
}
fn w8(reg: u32, v: u8) -> u32 {
    (reg & !0xff) | u32::from(v)
}
fn signed(v: u16) -> u32 {
    v as i16 as u32
}
fn add_sp(c: &mut CpuAbi, n: u32) {
    c.esp = arithmetic(c, c.esp, n, false);
}
fn cmp(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    arithmetic(c, a, b, true) == 0
}
fn eq16(c: &mut CpuAbi, a: u16, b: u16) -> bool {
    crate::sim_native::sub_flags(c, a.into(), b.into(), 16);
    a == b
}
fn bits(c: &CpuAbi, n: usize) -> u32 {
    u128::from_le_bytes(c.xmm[n]) as u32
}
fn scalar(c: &mut CpuAbi, n: usize, v: u32) {
    c.xmm[n] = u128::from(v).to_le_bytes();
}
fn cvt(c: &mut CpuAbi, n: usize, v: u32) {
    let old = u128::from_le_bytes(c.xmm[n]);
    c.xmm[n] = simd::cvtsi2ss(&mut c.mxcsr, old, v)
        .expect("masked SSE")
        .to_le_bytes();
}
fn mul(c: &mut CpuAbi, n: usize, v: u32) {
    let old = u128::from_le_bytes(c.xmm[n]);
    c.xmm[n] = simd::mulss(&mut c.mxcsr, old, v)
        .expect("masked SSE")
        .to_le_bytes();
}
fn trunc(c: &mut CpuAbi) -> u32 {
    let value = bits(c, 0);
    simd::cvttss2si(&mut c.mxcsr, value).expect("masked SSE")
}
fn above(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    const ARITH: u32 = 0x8d5;
    let f = simd::comiss(&mut c.mxcsr, a, b).expect("masked SSE");
    c.eflags = (c.eflags & !ARITH) | f;
    c.eflags_defined |= ARITH;
    f == 0
}
/// Low datum index scaled to the 16-byte tag record. SHL by four
/// leaves overflow undefined, and the masked index can't shift out a bit.
fn tag_index(c: &mut CpuAbi, datum: u32) -> u32 {
    let offset = (datum & 0xffff) << 4;
    logic(c, offset, 1 << 31);
    c.eflags_defined &= !0x800;
    offset
}
fn data(g: &impl Memory, table: u32, index: u32) -> Checked<u32> {
    g.u32(at(at(table, index), 8))
}
fn graph(c: &mut CpuAbi, g: &mut Bus<'_>, _pc: u32) -> Transfer {
    c.eax = g.u32(c.ecx)?;
    c.ecx = w16(c.ecx, g.u16(at(c.ecx, 4))?);
    c.eax &= 0xffff;
    push(c, g, c.esi)?;
    c.esi = g.u32(TAG_ENTRIES.base)?;
    c.eax = tag_index(c, c.eax);
    let local = eq16(c, c.ecx as u16, u16::MAX);
    c.eax = data(g, c.esi, c.eax)?;
    if !local {
        c.ecx = signed(c.ecx as u16);
        push(c, g, c.edi)?;
        c.edi = g.u32(at(c.eax, 0x4c))?;
        c.edx = 0;
        cmp(c, c.ecx, c.edi);
        if (c.ecx as i32) < (c.edi as i32) {
            c.edi = g.u32(at(c.eax, 0x50))?;
            c.ecx = g.u32(at(at(c.edi, c.ecx << 5), 4))?;
            if !cmp(c, c.ecx, u32::MAX) {
                c.ecx = tag_index(c, c.ecx);
                c.edx = data(g, c.esi, c.ecx)?;
            }
        }
        c.eax = c.edx;
        c.edi = pop(c, g)?;
    }
    c.esi = pop(c, g)?;
    g.ret(c, 0)
}

fn scale_outputs(c: &mut CpuAbi, g: &mut impl Memory, src: u32, dst: u32) -> Checked<()> {
    for off in [0, 4, 8] {
        scalar(c, 1, g.u32(at(src, off))?);
        mul(c, 1, bits(c, 0));
        g.set_u32(at(dst, off), bits(c, 1))?;
    }
    Ok(())
}

fn velocity(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    if pc == 0x1c6c00 {
        c.edx = g.u32(c.ecx)?;
        c.esp = c.esp.wrapping_sub(16);
        c.eax = w8(c.eax, 0);
        if !cmp(c, c.edx, u32::MAX) && !eq16(c, g.u16(at(c.ecx, 6))?, 0xffff) {
            scalar(c, 0, g.u32(at(c.ecx, 0x18))?);
            mul(c, 0, g.u32(MOTION_SCALE)?);
            c.eax = g.u32(at(c.esp, 20))?;
            push(c, g, c.ebx)?;
            g.set_u32(at(c.esp, 4), bits(c, 0))?;
            scalar(c, 0, g.u32(at(c.ecx, 0x1c))?);
            push(c, g, c.eax)?;
            c.ebx = at(c.esp, 12);
            return g.call(c, 0x1c6c80, 0x1c6c3a);
        }
    } else {
        scalar(c, 0, g.u32(at(c.esp, 4))?);
        scale_outputs(c, g, at(c.esp, 8), c.esi)?;
        c.eax = w8(c.eax, 1);
        c.ebx = pop(c, g)?;
    }
    add_sp(c, 16);
    g.ret(c, 4)
}

fn zero_view(g: &mut impl Memory, p: u32) -> Checked<()> {
    g.set_u32(p, 0)?;
    g.set_u32(at(p, 4), 0)?;
    g.set_u8(at(p, 8), 0)?;
    g.set_u8(at(p, 9), 0)
}

fn sample_velocity(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    match pc {
        0x1c6c80 => {
            c.esp = c.esp.wrapping_sub(16);
            push(c, g, c.esi)?;
            c.esi = g.u32(c.ecx)?;
            c.edx = 0;
            c.eax = w8(c.eax, 0);
            if !cmp(c, c.esi, u32::MAX) && !eq16(c, g.u16(at(c.ecx, 6))?, 0xffff) {
                push(c, g, c.edi)?;
                g.set_u32(at(c.esp, 8), bits(c, 0))?;
                scalar(c, 0, g.u32(at(c.esp, 8))?);
                c.esi = trunc(c);
                c.eax = g.u32(at(c.ecx, 4))?;
                push(c, g, c.eax)?;
                c.eax = g.u32(c.ecx)?;
                c.ecx = g.u32(TAG_ENTRIES.base)?;
                c.eax = tag_index(c, c.eax);
                zero_view(g, at(c.esp, 16))?;
                c.edx = data(g, c.ecx, c.eax)?;
                push(c, g, c.edx)?;
                c.edi = at(c.esp, 20);
                return g.call(c, RESOURCE, 0x1c6cdb);
            }
        }
        0x1c6cdb => {
            c.edx = g.u32(at(c.esp, 28))?;
            c.eax = c.esi;
            c.edi = at(c.esp, 12);
            return g.call(c, 0x20ad40, 0x1c6cea);
        }
        _ => {
            c.eax = w8(c.eax, 1);
            c.edi = pop(c, g)?;
        }
    }
    c.esi = pop(c, g)?;
    add_sp(c, 16);
    g.ret(c, 4)
}

fn root_motion(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    if pc == 0x1c6d00 {
        c.eax = g.u32(0x4687a4)?;
        scalar(c, 0, 0);
        c.esp = c.esp.wrapping_sub(16);
        push(c, g, c.ebp)?;
        c.ebp = g.u32(at(c.esp, 24))?;
        push(c, g, c.esi)?;
        c.esi = g.u32(c.eax)?;
        c.edx = c.edi;
        g.set_u32(c.edx, c.esi)?;
        c.esi = g.u32(at(c.eax, 4))?;
        g.set_u32(at(c.edx, 4), c.esi)?;
        c.eax = g.u32(at(c.eax, 8))?;
        g.set_u32(at(c.edx, 8), c.eax)?;
        g.set_u32(c.ebp, 0)?;
        if !eq16(c, g.u16(at(c.ecx, 6))?, 0xffff) {
            scalar(c, 0, g.u32(at(c.ecx, 0x1c))?);
            push(c, g, c.ebx)?;
            g.set_u32(at(c.esp, 32), bits(c, 0))?;
            scalar(c, 0, g.u32(at(c.esp, 32))?);
            c.eax = trunc(c);
            scalar(c, 0, g.u32(at(c.ecx, 0x18))?);
            mul(c, 0, g.u32(MOTION_SCALE)?);
            push(c, g, c.eax)?;
            c.ebx = at(c.esp, 16);
            c.esi = at(c.esp, 20);
            c.eax = c.ecx;
            g.set_u32(at(c.esp, 36), bits(c, 0))?;
            return g.call(c, 0x1c6dc0, 0x1c6d68);
        }
    } else {
        scalar(c, 0, g.u32(at(c.esp, 32))?);
        scale_outputs(c, g, at(c.esp, 16), c.edi)?;
        scalar(c, 1, g.u32(at(c.esp, 12))?);
        mul(c, 1, bits(c, 0));
        g.set_u32(c.ebp, bits(c, 1))?;
        c.ebx = pop(c, g)?;
    }
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    add_sp(c, 16);
    g.ret(c, 4)
}

fn sample_root(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    match pc {
        0x1c6dc0 => {
            scalar(c, 0, 0);
            for off in [0, 4, 8] {
                g.set_u32(at(c.esi, off), 0)?;
            }
            c.esp = c.esp.wrapping_sub(12);
            g.set_u32(c.ebx, 0)?;
            if !eq16(c, g.u16(at(c.eax, 6))?, 0xffff) {
                c.ecx = 0;
                zero_view(g, c.esp)?;
                c.ecx = g.u32(at(c.eax, 4))?;
                c.eax = g.u32(c.eax)?;
                push(c, g, c.edi)?;
                push(c, g, c.ecx)?;
                c.ecx = g.u32(TAG_ENTRIES.base)?;
                c.eax = tag_index(c, c.eax);
                c.edx = data(g, c.ecx, c.eax)?;
                push(c, g, c.edx)?;
                c.edi = at(c.esp, 12);
                return g.call(c, RESOURCE, 0x1c6e13);
            }
        }
        0x1c6e13 => {
            c.eax = g.u32(at(c.esp, 20))?;
            push(c, g, c.eax)?;
            c.eax = c.ebx;
            c.ecx = c.esi;
            c.edx = at(c.esp, 8);
            return g.call(c, 0x20aa70, 0x1c6e25);
        }
        _ => c.edi = pop(c, g)?,
    }
    add_sp(c, 12);
    g.ret(c, 4)
}

fn channel_track(c: &mut CpuAbi, g: &mut impl Memory, site: u32) -> Transfer {
    c.ecx = g.u32(c.eax)?;
    c.edx = g.u32(TAG_ENTRIES.base)?;
    c.eax = g.u32(at(c.eax, 4))?;
    c.ecx = tag_index(c, c.ecx);
    c.edx = data(g, c.edx, c.ecx)?;
    push(c, g, c.eax)?;
    g.call(c, TRACK, site)
}

fn progress(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    if pc == 0x1c6e30 {
        push(c, g, c.ecx)?;
        let none = eq16(c, g.u16(at(c.esi, 6))?, 0xffff);
        scalar(c, 0, 0);
        g.set_u32(c.esp, 0)?;
        if !none {
            c.eax = g.u32(c.esi)?;
            c.ecx = g.u32(TAG_ENTRIES.base)?;
            c.eax = tag_index(c, c.eax);
            c.edx = data(g, c.ecx, c.eax)?;
            c.eax = g.u32(at(c.esi, 4))?;
            push(c, g, c.eax)?;
            return g.call(c, TRACK, 0x1c6e5d);
        }
    } else {
        c.ecx = signed(g.u16(at(c.eax, 20))?);
        scalar(c, 2, 0);
        cvt(c, 1, c.ecx);
        if above(c, bits(c, 1), 0) {
            scalar(c, 0, g.u32(at(c.esi, 28))?);
            let initial = u128::from_le_bytes(c.xmm[0]);
            let divisor = bits(c, 1);
            c.xmm[0] = simd::divss(&mut c.mxcsr, initial, divisor)
                .expect("masked SSE")
                .to_le_bytes();
            if above(c, 0, bits(c, 0)) {
                c.xmm[0] = c.xmm[2];
            } else {
                scalar(c, 1, g.u32(ONE)?);
                if above(c, bits(c, 0), bits(c, 1)) {
                    c.xmm[0] = c.xmm[1];
                }
            }
        } else {
            scalar(c, 0, g.u32(c.esp)?);
        }
    }
    c.ecx = pop(c, g)?;
    g.ret(c, 0)
}

fn duration(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    if pc == 0x1c6ea0 {
        let none = eq16(c, g.u16(at(c.eax, 6))?, 0xffff);
        scalar(c, 0, 0);
        if !none {
            return channel_track(c, g, 0x1c6ec8);
        }
    } else {
        c.ecx = signed(g.u16(at(c.eax, 20))?);
        cvt(c, 0, c.ecx);
        mul(c, 0, g.u32(FRAME_SCALE)?);
    }
    g.ret(c, 0)
}

fn event_time(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    if pc == 0x1c6ee0 {
        push(c, g, c.ecx)?;
        let none = eq16(c, g.u16(at(c.eax, 6))?, 0xffff);
        scalar(c, 0, g.u32(0x45dbcc)?);
        g.set_u32(c.esp, bits(c, 0))?;
        if !none {
            return channel_track(c, g, 0x1c6f13);
        }
    } else {
        c.edx = g.u32(at(c.eax, 0x4c))?;
        c.ecx = 0;
        let mut frame = u16::MAX;
        if c.edx as i32 > 0 {
            c.eax = g.u32(at(c.eax, 0x50))?;
            while c.ecx < c.edx {
                if g.u16(c.eax)? == 0 {
                    frame = g.u16(at(c.eax, 2))?;
                    break;
                }
                c.ecx += 1;
                c.eax = at(c.eax, 4);
            }
        }
        c.eax = signed(frame);
        if cmp(c, c.eax, u32::MAX) {
            scalar(c, 0, g.u32(c.esp)?);
        } else {
            cvt(c, 0, c.eax);
            mul(c, 0, g.u32(FRAME_SCALE)?);
        }
    }
    c.ecx = pop(c, g)?;
    g.ret(c, 0)
}

/// Select one of the graph's two inline node masks. On the first path
/// the scaled datum stays in EAX; on the second it stays in ECX.
fn mask(c: &mut CpuAbi, g: &impl Memory, flags: u16, record: u32, tags: u32) -> Checked<u32> {
    if flags & (1 << 11) != 0 {
        c.eax = tag_index(c, g.u32(record)?);
        Ok(at(data(g, tags, c.eax)?, 0x5c))
    } else if flags & (1 << 10) != 0 {
        c.ecx = tag_index(c, g.u32(record)?);
        Ok(at(data(g, tags, c.ecx)?, 0x7c))
    } else {
        Ok(0)
    }
}

fn copy_mask(c: &mut CpuAbi, g: &mut impl Memory, site: u32) -> Transfer {
    c.ecx = 8;
    c.edi = SAMPLE_MASK;
    let stride = if c.eflags & 0x400 != 0 {
        0u32.wrapping_sub(4)
    } else {
        4
    };
    while c.ecx != 0 {
        let word = g.u32(c.esi)?;
        g.set_u32(c.edi, word)?;
        c.esi = at(c.esi, stride);
        c.edi = at(c.edi, stride);
        c.ecx -= 1;
    }
    c.ecx = c.edx;
    c.eax = SAMPLE_MASK;
    g.call(c, 0x27a380, site)
}

fn valid_track(c: &mut CpuAbi, g: &impl Memory, channel: u32, none: u16) -> Checked<bool> {
    Ok(!eq16(c, g.u16(at(channel, 6))?, none) && !eq16(c, g.u16(at(channel, 4))?, none))
}

// Base and additive wrappers pass the resulting mask in different stack slots.
fn mask_and_track(c: &mut CpuAbi, g: &mut impl Memory, base: bool, mut pc: u32) -> Transfer {
    let mask_off = if base { 16 } else { 12 };
    if pc == 0x1c7004 || pc == 0x1c7200 {
        let flags = if base {
            push(c, g, c.ebp)?;
            c.ebp = g.u32(TAG_ENTRIES.base)?;
            push(c, g, c.esi)?;
            c.ecx as u16
        } else {
            push(c, g, c.ecx)?;
            c.eax = w16(c.eax, g.u16(at(c.ebx, 0x12))?);
            push(c, g, c.ebp)?;
            c.ebp = g.u32(TAG_ENTRIES.base)?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.eax as u16
        };
        c.esi = mask(c, g, flags, c.ebx, c.ebp)?;
        if base {
            c.edx = g.u32(at(c.esp, 28))?;
            c.eax = 0;
        }
        g.set_u32(at(c.esp, mask_off), 0)?;
        if c.esi != 0 || c.edx != 0 {
            let src = if c.esi != 0 { c.esi } else { c.edx };
            if c.esi != 0 && c.edx != 0 && !cmp(c, c.esi, c.edx) {
                return copy_mask(c, g, if base { 0x1c7073 } else { 0x1c7275 });
            }
            c.eax = src;
            g.set_u32(at(c.esp, mask_off), src)?;
        }
    } else if pc == 0x1c7073 || pc == 0x1c7275 {
        c.eax = SAMPLE_MASK;
        if base {
            c.edi = u32::MAX;
        }
        g.set_u32(at(c.esp, mask_off), c.eax)?;
    }
    if pc != 0x1c70b2 && pc != 0x1c72b4 {
        c.eax = 0;
        if !base {
            c.ecx = u32::MAX;
        }
        let none = if base { c.edi } else { c.ecx };
        if valid_track(c, g, c.ebx, none as u16)? {
            c.eax = g.u32(c.ebx)?;
            c.edx = g.u32(at(c.ebx, 4))?;
            c.eax = tag_index(c, c.eax);
            c.eax = data(g, c.ebp, c.eax)?;
            push(c, g, c.edx)?;
            c.edi = c.eax;
            return g.call(c, 0x1daff0, if base { 0x1c70b2 } else { 0x1c72b4 });
        }
    } else {
        c.ebp = g.u32(TAG_ENTRIES.base)?;
    }
    pc = if base { 0x1c70ee } else { 0x1c72ec };
    c.edx = g.u32(at(c.esp, if base { 12 } else { 32 }))?;
    c.ecx = g.u32(at(c.ebx, 4))?;
    push(c, g, c.edx)?;
    c.edx = g.u32(at(c.esp, if base { 40 } else { 16 }))?;
    push(c, g, c.edx)?;
    c.edx = g.u32(at(c.esp, if base { 24 } else { 28 }))?;
    if base {
        push(c, g, c.edx)?;
        c.edx = g.u32(at(c.ebx, 28))?;
    }
    push(c, g, c.eax)?;
    c.eax = g.u32(at(c.esp, if base { 40 } else { 36 }))?;
    push(c, g, c.eax)?;
    c.eax = g.u32(at(c.esp, if base { 52 } else { 44 }))?;
    for v in [c.edx, c.eax, c.ecx] {
        push(c, g, v)?;
    }
    c.ecx = tag_index(c, g.u32(c.ebx)?);
    c.eax = data(g, c.ebp, c.ecx)?;
    g.call(c, if base { 0x279d80 } else { 0x27a060 }, pc)
}

fn base_pose(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    match pc {
        0x1c6f60 => {
            c.esp = c.esp.wrapping_sub(8);
            push(c, g, c.edi)?;
            c.edi = u32::MAX;
            c.eax = 0;
            if !eq16(c, g.u16(at(c.ebx, 6))?, 0xffff) {
                c.eax = g.u32(c.ebx)?;
                c.ecx = g.u32(TAG_ENTRIES.base)?;
                c.eax = tag_index(c, c.eax);
                c.edx = data(g, c.ecx, c.eax)?;
                c.eax = g.u32(at(c.ebx, 4))?;
                push(c, g, c.eax)?;
                return g.call(c, TRACK, 0x1c6f8c);
            }
        }
        0x1c6fb8 | 0x1c70ee => {
            if pc == 0x1c70ee {
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
            }
            c.edi = pop(c, g)?;
            add_sp(c, 8);
            return g.ret(c, 16);
        }
        0x1c6f8c => {}
        _ => return mask_and_track(c, g, true, pc),
    }
    c.ecx = w8(c.ecx, g.u8(at(c.ebx, 16))?);
    if c.ecx as u8 != 0 {
        c.ecx = w8(c.ecx, g.u8(at(c.eax, 16))?);
        if c.ecx as u8 != 0 {
            c.ecx = g.u32(c.ebx)?;
            c.eax = g.u32(at(c.ebx, 4))?;
            c.edx = g.u32(TAG_ENTRIES.base)?;
            c.ecx = tag_index(c, c.ecx);
            c.edx = data(g, c.edx, c.ecx)?;
            push(c, g, c.eax)?;
            return g.call(c, 0x1dd9d0, 0x1c6fb8);
        }
    }
    c.ecx = w16(c.ecx, g.u16(at(c.ebx, 0x12))?);
    c.eax = w8(c.eax, ((c.ecx >> 6) & 1) as u8);
    g.set_u8(at(c.esp, 4), c.eax as u8)?;
    if c.ecx & 1 != 0 {
        let rate = g.u32(at(c.ebx, 24))?;
        scalar(c, 0, rate);
        let (mode, comparison) = playback_rate(rate, g.u32(ZERO)?, || g.u32(ONE), &mut c.mxcsr)?;
        let ah = comparison | 2;
        c.eax = (c.eax & !0xff00) | (ah << 8);
        logic(c, ah & 0x44, 0x80);
        let force = match mode {
            PlaybackRate::Stopped => false,
            PlaybackRate::Unit => {
                c.edx = g.u32(0x510c54)?;
                g.u16(at(c.edx, 2))? != u16::from(g.u8(0x1c6ffc)?)
            }
            PlaybackRate::Other => true,
        };
        if force {
            g.set_u8(at(c.esp, 4), 1)?;
        }
    }
    mask_and_track(c, g, true, 0x1c7004)
}

fn additive_pose(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    if pc != 0x1c72ec {
        return mask_and_track(c, g, false, pc);
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ecx = pop(c, g)?;
    g.ret(c, 16)
}

fn blend_pose(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    match pc {
        0x1c7100 => {
            if eq16(c, g.u16(at(c.esi, 6))?, 0xffff) {
                return g.ret(c, 24);
            }
            c.eax = g.u32(c.esi)?;
            c.ecx = g.u32(TAG_ENTRIES.base)?;
            c.edx = g.u32(at(c.esi, 4))?;
            c.eax &= 0xffff;
            push(c, g, c.ebp)?;
            c.eax = tag_index(c, c.eax);
            c.eax = data(g, c.ecx, c.eax)?;
            push(c, g, c.edx)?;
            return g.call(c, 0x1daf30, 0x1c7129);
        }
        0x1c7129 => {
            c.ebp = c.eax;
            logic(c, c.ebp, 1 << 31);
            if c.ebp == 0 {
                c.ebp = pop(c, g)?;
                return g.ret(c, 24);
            }
            c.ecx = w16(c.ecx, g.u16(at(c.esi, 0x12))?);
            push(c, g, c.ebx)?;
            c.eax = 0;
            push(c, g, c.edi)?;
            c.edi = g.u32(TAG_ENTRIES.base)?;
            c.eax = mask(c, g, c.ecx as u16, c.esi, c.edi)?;
            c.ebx = 0;
            logic(c, c.eax, 1 << 31);
            if c.eax == 0 {
                c.ecx = g.u32(at(c.esp, 28))?;
                logic(c, c.ecx, 1 << 31);
            }
            if c.eax != 0 || c.ecx != 0 {
                c.edx = g.u32(at(c.esp, 28))?;
                return g.call(c, 0x279d40, 0x1c7187);
            }
        }
        0x1c7187 => c.ebx = c.eax,
        0x1c71ec => {
            c.edi = pop(c, g)?;
            c.ebx = pop(c, g)?;
            c.ebp = pop(c, g)?;
            return g.ret(c, 24);
        }
        _ => {}
    }
    if pc != 0x1c71b1 {
        c.ecx = u32::MAX;
        c.eax = 0;
        if valid_track(c, g, c.esi, u16::MAX)? {
            c.eax = g.u32(c.esi)?;
            c.edx = g.u32(at(c.esi, 4))?;
            c.eax = tag_index(c, c.eax);
            c.edi = data(g, c.edi, c.eax)?;
            push(c, g, c.edx)?;
            return g.call(c, 0x1daff0, 0x1c71b1);
        }
    } else {
        c.edi = g.u32(TAG_ENTRIES.base)?;
    }
    c.edx = g.u32(at(c.esp, 36))?;
    c.ecx = g.u32(at(c.esi, 4))?;
    push(c, g, c.edx)?;
    c.edx = g.u32(at(c.esp, 24))?;
    push(c, g, c.ebx)?;
    push(c, g, c.eax)?;
    c.eax = g.u32(at(c.esp, 36))?;
    push(c, g, c.eax)?;
    c.eax = g.u32(at(c.esp, 32))?;
    push(c, g, c.edx)?;
    c.edx = g.u32(at(c.esp, 52))?;
    push(c, g, c.eax)?;
    c.eax = g.u32(c.esi)? & 0xffff;
    push(c, g, c.edx)?;
    c.eax = tag_index(c, c.eax);
    push(c, g, c.ecx)?;
    c.ecx = data(g, c.edi, c.eax)?;
    push(c, g, c.ecx)?;
    c.eax = c.ebp;
    g.call(c, 0x279e40, 0x1c71ec)
}

fn track_flag(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    if matches!(pc, 0x1c7300 | 0x1c7340) {
        c.ecx = g.u32(c.esi)?;
        let none = cmp(c, c.ecx, u32::MAX);
        c.eax = w8(c.eax, 1);
        if !none && !eq16(c, g.u16(at(c.esi, 6))?, 0xffff) {
            c.eax = g.u32(TAG_ENTRIES.base)?;
            c.edx = g.u32(at(c.esi, 4))?;
            c.ecx = tag_index(c, c.ecx);
            c.ecx = data(g, c.eax, c.ecx)?;
            push(c, g, c.edx)?;
            c.edx = c.ecx;
            return g.call(c, TRACK, if pc == 0x1c7300 { 0x1c732d } else { 0x1c736d });
        }
    } else {
        let flag = g.u8(at(c.eax, 24))? >> if pc == 0x1c732d { 0 } else { 6 };
        let flag = !flag & 1;
        c.eax = w8(c.eax, flag);
        logic(c, flag.into(), 128);
    }
    g.ret(c, 0)
}

fn view(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    if pc == 0x1c7380 || pc == 0x1c73a0 {
        let vector = pc == 0x1c73a0;
        if vector {
            c.esp = c.esp.wrapping_sub(44);
            c.ecx = 0;
            g.set_u32(c.esp, 0)?;
            g.set_u32(at(c.esp, 4), 0)?;
        }
        c.ecx = g.u32(at(c.eax, 4))?;
        c.eax = g.u32(c.eax)?;
        if vector {
            push(c, g, c.edi)?;
        }
        push(c, g, c.ecx)?;
        c.ecx = g.u32(TAG_ENTRIES.base)?;
        c.eax = tag_index(c, c.eax);
        c.edx = data(g, c.ecx, c.eax)?;
        push(c, g, c.edx)?;
        if vector {
            c.edi = at(c.esp, 12);
        }
        return g.call(c, RESOURCE, if vector { 0x1c73cf } else { 0x1c739e });
    }
    if pc == 0x1c739e {
        return g.ret(c, 0);
    }
    if pc == 0x1c73cf {
        c.eax = g.u32(at(c.esp, 8))?;
        c.ecx = signed(g.u16(at(c.eax, 6))?);
        c.eax = g.u32(at(c.esp, 4))?;
        scalar(c, 0, g.u32(at(c.esp, 52))?);
        c.ecx = arithmetic(c, c.ecx, c.eax, false);
        g.set_u32(at(c.esp, 52), bits(c, 0))?;
        scalar(c, 0, g.u32(at(c.esp, 52))?);
        c.eax = trunc(c);
        scalar(c, 0, 0);
        c.edx = signed(g.u16(at(c.esp, 56))?);
        g.set_u32(0x504464, c.eax)?;
        g.set_u32(0x504468, c.eax)?;
        c.eax = at(c.esp, 16);
        g.set_u32(0x504480, c.ecx)?;
        g.set_u32(0x50446c, 0)?;
        g.set_u32(0x5044c0, c.eax)?;
        c.eax = g.u8(c.ecx)?.into();
        c.ecx = c.eax * 5;
        g.set_u32(0x5044b8, c.edx)?;
        let codec = g.u32(at(0x47fb28, c.ecx * 8))?;
        return g.call(c, codec, 0x1c742f);
    }
    c.edx = g.u32(at(c.esp, 32))?;
    c.eax = g.u32(at(c.esp, 36))?;
    c.ecx = g.u32(at(c.esp, 40))?;
    g.set_u32(c.esi, c.edx)?;
    g.set_u32(at(c.esi, 4), c.eax)?;
    g.set_u32(at(c.esi, 8), c.ecx)?;
    c.edi = pop(c, g)?;
    add_sp(c, 44);
    g.ret(c, 8)
}
