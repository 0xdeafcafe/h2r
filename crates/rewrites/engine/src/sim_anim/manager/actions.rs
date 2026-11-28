//! Action-name lookup and the root motion sampled from a manager.
use super::*;
use crate::function_eval::constant::ONE;
use guest_layout::points::TAG_ENTRIES;
use crate::sim_native::Memory;
use crate::sim_physics::{ARITH, arithmetic, byte};
use runtime::GuestFault;

pub(super) const BODIES: &[Body] = &[
    body!(0x1cbf50, 0x1cbfc9, &[0x1cbf81, 0x1cbf8d], root_transform),
    body!(0x1cbfd0, 0x1cc091, &[0x1cc02f, 0x1cc03e], root_velocity),
    body!(
        0x1cc0a0,
        0x1cc14d,
        &[0x1cc0d9, 0x1cc103, 0x1cc136],
        has_action
    ),
    body!(0x1cc150, 0x1cc191, &[0x1cc188], default_action),
    body!(
        0x1cc1a0,
        0x1cc258,
        &[0x1cc1ed, 0x1cc218, 0x1cc247],
        action_by_kind
    ),
    body!(0x1cc260, 0x1cc2a1, &[0x1cc296], named_action),
    body!(0x1cc2b0, 0x1cc2ca, &[0x1cc2c6], channel_request),
    body!(0x1cc2d0, 0x1cc2e7, &[], manager_graph),
    body!(0x1cc2f0, 0x1cc3e1, &[0x1cc394], action_names),
];

const SAMPLE_ROOT: u32 = 0x1cbec0;
const LOOKUP: u32 = 0x1dbc80;
const IDENTITY: u32 = 0x4687d8;

// The aligned frame holds one or two 32-byte animation records.
fn frame(c: &mut CpuAbi, g: &mut Bus<'_>, size: u32) -> Result<(), GuestFault> {
    push(c, g, c.ebp)?;
    c.ebp = c.esp;
    c.esp = arithmetic(c, c.esp & !15, size, true);
    Ok(())
}

// REP MOVSD is sequential, including through physical aliases.
fn copy_record(c: &mut CpuAbi, g: &mut Bus<'_>, dst: u32) -> Result<(), GuestFault> {
    c.edi = dst;
    c.ecx = 8;
    let stride = if c.eflags & 0x400 == 0 {
        4
    } else {
        u32::MAX - 3
    };
    while c.ecx != 0 {
        let word = g.u32(c.esi)?;
        g.set_u32(c.edi, word)?;
        c.esi = at(c.esi, stride);
        c.edi = at(c.edi, stride);
        c.ecx -= 1;
    }
    Ok(())
}

fn leave_frame(c: &mut CpuAbi, g: &mut Bus<'_>, cleanup: u32) -> sim_native::Result {
    c.esi = pop(c, g)?;
    c.esp = c.ebp;
    c.ebp = pop(c, g)?;
    g.ret(c, cleanup)
}

fn root_transform(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cbf50 => {
            frame(c, g, 40)?;
            c.edx = g.u32(at(c.ebp, 12))?;
            push(c, g, c.esi)?;
            c.esi = g.u32(IDENTITY)?;
            push(c, g, c.edi)?;
            copy_record(c, g, at(c.esp, 16))?;
            c.ecx = g.u32(at(c.ebp, 16))?;
            c.esi = g.u32(at(c.ebp, 8))?;
            c.eax = at(c.esp, 16);
            for value in [c.eax, c.ecx, c.edx] {
                push(c, g, value)?;
            }
            g.call(c, SAMPLE_ROOT, 0x1cbf81)
        }
        0x1cbf81 => {
            c.eax = at(c.ebx, 4);
            c.ecx = at(c.esp, 16);
            g.call(c, 0x141e10, 0x1cbf8d)
        }
        0x1cbf8d => {
            c.xmm[0] = [0; 16];
            c.ecx = g.u32(at(c.esp, 32))?;
            c.edx = g.u32(at(c.esp, 36))?;
            c.eax = at(c.ebx, 40);
            for i in [0, 4, 8] {
                g.set_u32(at(c.eax, i), 0)?;
            }
            let one = g.u32(ONE)?;
            c.xmm[0] = u128::from(one).to_le_bytes();
            g.set_u32(c.eax, c.ecx)?;
            c.ecx = g.u32(at(c.esp, 40))?;
            c.edi = pop(c, g)?;
            g.set_u32(at(c.eax, 4), c.edx)?;
            g.set_u32(c.ebx, one)?;
            g.set_u32(at(c.eax, 8), c.ecx)?;
            leave_frame(c, g, 16)
        }
        _ => Err(bad(pc)),
    }
}

fn root_velocity(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cbfd0 => {
            frame(c, g, 72)?;
            let mut frame_value = g.u32(at(c.ebp, 16))?;
            let distance = g.u32(0x45dc6c)?;
            c.xmm[0] = u128::from(frame_value).to_le_bytes();
            c.xmm[1] = u128::from(distance).to_le_bytes();
            let f = simd::comiss(&mut c.mxcsr, frame_value, distance).map_err(|_| bad(pc))?;
            c.eflags = c.eflags & !ARITH | f;
            c.eflags_defined |= ARITH;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            if f & 0x41 != 0 {
                frame_value = distance;
                c.xmm[0] = c.xmm[1];
                g.set_u32(at(c.ebp, 16), frame_value)?;
            }
            c.eax = g.u32(IDENTITY)?;
            for loc in [48, 16] {
                c.esi = c.eax;
                copy_record(c, g, at(c.esp, loc))?;
            }
            c.edi = g.u32(at(c.ebp, 12))?;
            c.esi = g.u32(at(c.ebp, 8))?;
            c.eax = at(c.esp, 48);
            push(c, g, c.eax)?;
            push(c, g, c.ecx)?;
            let previous = simd::subss(&mut c.mxcsr, u128::from(frame_value), distance)
                .map_err(|_| bad(pc))?;
            c.xmm[0] = previous.to_le_bytes();
            g.set_u32(c.esp, previous as u32)?;
            push(c, g, c.edi)?;
            g.call(c, SAMPLE_ROOT, 0x1cc02f)
        }
        0x1cc02f => {
            c.edx = g.u32(at(c.ebp, 16))?;
            c.ecx = at(c.esp, 16);
            for value in [c.ecx, c.edx, c.edi] {
                push(c, g, value)?;
            }
            g.call(c, SAMPLE_ROOT, 0x1cc03e)
        }
        0x1cc03e => {
            let speed = g.u32(at(c.ebp, 20))?;
            let factor = simd::mulss(&mut c.mxcsr, u128::from(speed), g.u32(0x45dcfc)?)
                .map_err(|_| bad(pc))?;
            c.xmm[0] = factor.to_le_bytes();
            let sp = c.esp;
            for i in [0, 4, 8] {
                let pos = g.u32(at(sp, 32 + i))?;
                let delta = simd::subss(&mut c.mxcsr, u128::from(pos), g.u32(at(sp, 64 + i))?)
                    .map_err(|_| bad(pc))?;
                if i == 8 {
                    c.edi = pop(c, g)?;
                }
                let velocity =
                    simd::mulss(&mut c.mxcsr, delta, factor as u32).map_err(|_| bad(pc))?;
                c.xmm[1] = velocity.to_le_bytes();
                g.set_u32(at(c.ebx, i), velocity as u32)?;
            }
            leave_frame(c, g, 20)
        }
        _ => Err(bad(pc)),
    }
}

// SHL by four on a zero-extended u16. CF is zero, AF and OF are undefined.
fn tag_offset(c: &mut CpuAbi, handle: u32) -> u32 {
    let offset = (handle & 0xffff) << 4;
    sim_native::logic_flags(c, offset);
    c.eflags_defined &= !0x800;
    offset
}

fn manager_graph(c: &mut CpuAbi, g: &mut Bus<'_>, _: u32) -> sim_native::Result {
    c.ecx = g.u32(at(c.eax, 104))?;
    c.edx = g.u32(TAG_ENTRIES.base)?;
    c.ecx = tag_offset(c, c.ecx);
    c.eax = g.u32(at(at(c.ecx, c.edx), 8))?;
    g.ret(c, 0)
}

fn channel_request(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cc2b0 {
        push(c, g, c.ebx)?;
        push(c, g, c.eax)?;
        c.eax = g.u32(at(c.esp, 12))?;
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(c.esp, 20))?;
        c.ebx = c.edx as i16 as i32 as u32;
        c.eax = arithmetic(c, c.eax, 100, false);
        return g.call(c, 0x1d9470, 0x1cc2c6);
    }
    c.ebx = pop(c, g)?;
    g.ret(c, 8)
}

fn push_zeroes(c: &mut CpuAbi, g: &mut Bus<'_>) -> Result<(), GuestFault> {
    for _ in 0..3 {
        push(c, g, 0)?;
    }
    Ok(())
}

fn has_action(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cc0a0 => {
            push(c, g, c.ecx)?;
            c.eax = g.u32(at(c.esi, 104))?;
            c.ecx = g.u32(TAG_ENTRIES.base)?;
            c.edx = g.u32(at(c.esi, 120))?;
            push(c, g, c.edi)?;
            push_zeroes(c, g)?;
            c.eax = tag_offset(c, c.eax);
            c.edi = g.u32(at(at(c.eax, c.ecx), 8))?;
            c.eax = g.u32(at(c.esi, 116))?;
            c.ecx = g.u32(at(c.esi, 112))?;
            for value in [c.edx, c.eax, c.ecx] {
                push(c, g, value)?;
            }
            c.edx = at(c.esp, 28);
            push(c, g, c.edx)?;
            push(c, g, c.edi)?;
            c.eax = g.u32(0x1cc0d0)?;
            return g.call(c, LOOKUP, 0x1cc0d9);
        }
        0x1cc0d9 => {
            let index = u32::from(g.u16(at(c.esp, 6))?);
            sim_native::sub_flags(c, index, 0xffff, 16);
            if index == 0xffff {
                c.eax = g.u32(at(c.esi, 120))?;
                c.ecx = g.u32(at(c.esi, 116))?;
                c.edx = g.u32(at(c.esi, 112))?;
                push_zeroes(c, g)?;
                for value in [c.eax, c.ecx, c.edx] {
                    push(c, g, value)?;
                }
                c.eax = at(c.esp, 28);
                push(c, g, c.eax)?;
                push(c, g, c.edi)?;
                c.eax = g.u32(0x1cc0fa)?;
                return g.call(c, LOOKUP, 0x1cc103);
            }
        }
        0x1cc103 => {
            c.ecx = g.u32(c.eax)?;
            c.eax = c.ecx >> 16;
            sim_native::sub_flags(c, c.eax, 0xffff, 16);
            g.set_u32(at(c.esp, 4), c.ecx)?;
            if c.eax == 0xffff {
                c.edx = g.u32(at(c.esi, 120))?;
                c.eax = g.u32(at(c.esi, 116))?;
                c.ecx = g.u32(at(c.esi, 112))?;
                push_zeroes(c, g)?;
                for value in [c.edx, c.eax, c.ecx] {
                    push(c, g, value)?;
                }
                c.edx = at(c.esp, 28);
                push(c, g, c.edx)?;
                push(c, g, c.edi)?;
                c.eax = g.u32(0x1cc12d)?;
                return g.call(c, LOOKUP, 0x1cc136);
            }
        }
        0x1cc136 => {
            c.eax = g.u32(c.eax)?;
            g.set_u32(at(c.esp, 4), c.eax)?;
        }
        _ => return Err(bad(pc)),
    }
    c.eax = g.u32(at(c.esp, 4))? >> 16;
    sim_native::sub_flags(c, c.eax, 0xffff, 16);
    c.eax = byte(c.eax, u8::from(c.eax != 0xffff));
    c.edi = pop(c, g)?;
    c.ecx = pop(c, g)?;
    g.ret(c, 0)
}

// Default and named requests pass the same action triple. The default
// request gives the guest a local result; the named request reuses arg 1.
fn lookup_name(c: &mut CpuAbi, g: &mut Bus<'_>, named: bool) -> sim_native::Result {
    if !named {
        push(c, g, c.ecx)?;
    }
    c.ecx = g.u32(at(c.eax, 120))?;
    c.edx = g.u32(at(c.eax, 116))?;
    push_zeroes(c, g)?;
    push(c, g, c.ecx)?;
    c.ecx = g.u32(at(c.eax, 112))?;
    c.eax = g.u32(at(c.eax, 104))?;
    push(c, g, c.edx)?;
    push(c, g, c.ecx)?;
    c.ecx = g.u32(TAG_ENTRIES.base)?;
    c.edx = at(c.esp, if named { 28 } else { 24 });
    c.eax = tag_offset(c, c.eax);
    push(c, g, c.edx)?;
    c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
    c.eax = if named {
        g.u32(at(c.esp, 32))?
    } else {
        g.u32(0x1cc17f)?
    };
    push(c, g, c.edx)?;
    g.call(c, LOOKUP, if named { 0x1cc296 } else { 0x1cc188 })
}

fn default_action(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cc150 {
        return lookup_name(c, g, false);
    }
    c.eax = g.u32(c.esp)?;
    g.set_u32(c.esi, c.eax)?;
    c.eax = c.esi;
    c.ecx = pop(c, g)?;
    g.ret(c, 0)
}

fn named_action(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cc260 {
        return lookup_name(c, g, true);
    }
    c.eax = g.u32(at(c.esp, 4))?;
    g.set_u32(c.esi, c.eax)?;
    c.eax = c.esi;
    g.ret(c, 4)
}

fn action_by_kind(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cc1a0 {
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(c.eax, 104))?;
        push(c, g, c.edi)?;
        c.edi = g.u32(TAG_ENTRIES.base)?;
        c.ecx = tag_offset(c, c.ecx);
        c.ecx = g.u32(at(at(c.ecx, c.edi), 8))?;
        c.edi = u32::MAX;
        arithmetic(c, c.edx, 1, true);
        g.set_u16(at(c.esp, 4), 0xffff)?;
        g.set_u16(at(c.esp, 6), 0xffff)?;
        c.edi = pop(c, g)?;

        let kind = c.edx;
        if kind != 1 {
            sim_native::logic_flags(c, kind);
            if kind != 0 {
                arithmetic(c, kind, 2, true);
            }
        }
        if let Some((name, site)) = match kind {
            0 => Some((0x1cc20f, 0x1cc218)),
            1 => Some((0x1cc1e4, 0x1cc1ed)),
            2 => Some((0x1cc23e, 0x1cc247)),
            _ => None,
        } {
            if kind != 0 {
                c.edx = g.u32(at(c.eax, 120))?;
            }
            push_zeroes(c, g)?;
            if kind == 0 {
                c.edx = g.u32(at(c.eax, 120))?;
            }
            push(c, g, c.edx)?;
            c.edx = g.u32(at(c.eax, 116))?;
            c.eax = g.u32(at(c.eax, 112))?;
            push(c, g, c.edx)?;
            push(c, g, c.eax)?;
            c.edx = at(c.esp, 24);
            push(c, g, c.edx)?;
            push(c, g, c.ecx)?;
            c.eax = g.u32(name)?;
            return g.call(c, LOOKUP, site);
        }
        c.eax = g.u32(c.esp)?;
    } else {
        c.eax = g.u32(c.eax)?;
    }
    g.set_u32(c.esi, c.eax)?;
    c.eax = c.esi;
    c.ecx = pop(c, g)?;
    g.ret(c, 0)
}

// Normalize mode, class, type and action names. All string IDs, including
// defaults and sentinels, come from their operands in the mapped image.
fn action_names(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cc2f0 {
        let inherit = c.eax == g.u32(0x1cc2f1)?;
        push(c, g, c.ebx)?;
        c.ebx = g.u32(at(c.esp, 8))?;
        push(c, g, c.ebp)?;
        c.ebp = g.u32(at(c.esp, 16))?;
        for (offset, value) in [(0, c.eax), (4, c.ecx), (8, c.edx), (12, c.ebp)] {
            g.set_u32(at(c.esi, offset), value)?;
        }

        if inherit {
            c.eax = g.u32(at(c.ebx, 112))?;
            g.set_u32(c.esi, c.eax)?;
        }
        if (inherit && c.eax == u32::MAX) || g.u32(c.esi)? == g.u32(0x1cc318)? {
            g.set_u32(c.esi, g.u32(0x1cc320)?)?;
        }
        if c.ecx == g.u32(0x1cc326)? {
            c.eax = g.u32(at(c.ebx, 116))?;
            g.set_u32(at(c.esi, 4), c.eax)?;
        }
        c.eax = g.u32(at(c.esi, 4))?;
        if c.eax == g.u32(0x1cc336)? || c.eax == u32::MAX {
            g.set_u32(at(c.esi, 4), g.u32(0x1cc344)?)?;
        }
        if c.edx == g.u32(0x1cc34a)? {
            c.ecx = g.u32(at(c.ebx, 120))?;
            g.set_u32(at(c.esi, 8), c.ecx)?;
        }
        c.eax = g.u32(at(c.esi, 8))?;
        if c.eax == g.u32(0x1cc35a)? || c.eax == u32::MAX {
            g.set_u32(at(c.esi, 8), g.u32(0x1cc368)?)?;
        }
        c.eax = g.u32(at(c.ebx, 104))?;
        c.edx = g.u32(at(c.esi, 8))?;
        c.ecx = g.u32(TAG_ENTRIES.base)?;
        push(c, g, c.edi)?;
        c.edi = g.u32(at(c.esi, 4))?;
        push(c, g, 8)?;
        push(c, g, c.edx)?;
        c.eax = tag_offset(c, c.eax);
        c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
        c.edx = arithmetic(c, c.edx, 84, false);
        push(c, g, c.edx)?;
        return g.call(c, 0x1dd560, 0x1cc394);
    }

    if c.eax != 0 {
        c.eax = g.u32(at(c.eax, 4))?;
        if c.eax != u32::MAX && c.eax != g.u32(0x1cc3a1)? && c.edi != g.u32(0x1cc3a9)? {
            c.edi = c.eax;
        }
    }
    let inherit = c.ebp == g.u32(0x1cc3b3)?;
    g.set_u32(at(c.esi, 4), c.edi)?;
    c.ecx = g.u32(0x1cc3bb)?;
    c.edi = pop(c, g)?;
    if inherit {
        c.eax = g.u32(at(c.ebx, 124))?;
        g.set_u32(at(c.esi, 12), c.eax)?;
        if c.eax == u32::MAX {
            g.set_u32(at(c.esi, 12), c.ecx)?;
        }
    }
    let current = g.u32(at(c.esi, 12))?;
    let replace = arithmetic(c, current, g.u32(0x1cc3d3)?, true) == 0;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    if replace {
        g.set_u32(at(c.esi, 12), c.ecx)?;
    }
    g.ret(c, 8)
}
