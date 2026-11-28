//! Channel duration, reset and start, and the manager's named-action selection.
use super::*;
use crate::function_eval::constant::ONE;
use guest_layout::points::TAG_ENTRIES;
use crate::sim_native::sub_flags;
use crate::sim_physics::{arithmetic, byte, logic};

pub(super) const BODIES: &[Body] = &[
    body!(0x1ccae0, 0x1ccb36, &[0x1ccaff, 0x1ccb23, 0x1ccb30], start),
    body!(0x1ccb40, 0x1ccb7f, &[0x1ccb6e], duration),
    body!(0x1ccb80, 0x1ccba0, &[0x1ccb9f], seek),
    body!(0x1ccba0, 0x1ccc4c, &[], clear_channels),
    body!(0x1ccc50, 0x1cccc9, &[], clear_channels),
    body!(0x1cccd0, 0x1ccd95, &[0x1ccd39], select_action),
];

fn start(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1ccae0 => {
            // The manager's action defaults are immediate operands in the image.
            for address in [0x1ccae1, 0x1ccae3] {
                let arg = g.u8(address)? as i8 as u32;
                push(c, g, arg)?;
            }
            for address in [0x1ccae5, 0x1ccaea, 0x1ccaef] {
                let name = g.u32(address)?;
                push(c, g, name)?;
            }
            c.eax = g.u32(0x1ccaf4)?;
            c.ecx = c.esi;
            return g.call(c, 0x1cd070, 0x1ccaff);
        }
        0x1ccaff => {
            c.ecx = g.u32(at(c.esi, 0x68))?;
            c.eax = byte(c.eax, 0);
            sub_flags(c, c.ecx, u32::MAX, 32);
            if c.ecx == u32::MAX {
                return g.ret(c, 8);
            }
            c.eax = g.u32(at(c.esp, 4))?;
            push(c, g, c.ebx)?;
            c.ebx = g.u32(at(c.esp, 12))?;
            for arg in [u32::MAX, u32::MAX, u32::MAX, u32::MAX, c.eax, c.esi] {
                push(c, g, arg)?;
            }
            c.eax = c.esi;
            return g.call(c, 0x1cb410, 0x1ccb23);
        }
        0x1ccb23 => {
            c.ebx = byte(c.ebx, c.eax as u8);
            logic(c, c.ebx & 0xff, 0x80);
            if c.ebx as u8 != 0 {
                c.eax = c.esi;
                return g.call(c, 0x1ccc50, 0x1ccb30);
            }
        }
        0x1ccb30 => {}
        _ => return Err(bad(pc)),
    }
    c.eax = byte(c.eax, c.ebx as u8);
    c.ebx = pop(c, g)?;
    g.ret(c, 8)
}

fn duration(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1ccb40 {
        c.eax = g.u32(c.ecx)?;
        sub_flags(c, c.eax, u32::MAX, 32);
        c.xmm[0] = [0; 16];
        if c.eax == u32::MAX {
            return g.ret(c, 0);
        }
        let clip = g.u16(at(c.ecx, 6))?;
        sub_flags(c, u32::from(clip), 0xffff, 16);
        if clip == 0xffff {
            return g.ret(c, 0);
        }
        c.edx = g.u32(TAG_ENTRIES.base)?;
        c.ecx = g.u32(at(c.ecx, 4))?;
        let slot = (c.eax & 0xffff) << 4;
        logic(c, slot, 1 << 31);
        c.eflags_defined &= !(1 << 11);
        c.eax = g.u32(at(c.edx, slot + 8))?;
        push(c, g, c.ecx)?;
        c.edx = c.eax;
        return g.call(c, 0x1daea0, 0x1ccb6e);
    }
    c.edx = g.u16(at(c.eax, 0x14))? as i16 as u32;
    let value =
        simd::cvtsi2ss(&mut c.mxcsr, u128::from_le_bytes(c.xmm[0]), c.edx).map_err(|_| bad(pc))?;
    c.xmm[0] = simd::mulss(&mut c.mxcsr, value, g.u32(0x45dc6c)?)
        .map_err(|_| bad(pc))?
        .to_le_bytes();
    g.ret(c, 0)
}

fn seek(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1ccb80 {
        let graph = g.u32(c.eax)?;
        sub_flags(c, graph, u32::MAX, 32);
        if graph == u32::MAX {
            return g.ret(c, 0);
        }
        let clip = g.u16(at(c.eax, 6))?;
        sub_flags(c, u32::from(clip), 0xffff, 16);
        if clip == 0xffff {
            return g.ret(c, 0);
        }
        let value = simd::mulss(
            &mut c.mxcsr,
            u128::from_le_bytes(c.xmm[0]),
            g.u32(0x45dcfc)?,
        )
        .map_err(|_| bad(pc))?;
        c.xmm[0] = value.to_le_bytes();
        push(c, g, c.ecx)?;
        g.set_u32(c.esp, value as u32)?;
        return g.call(c, 0x1c6560, 0x1ccb9f);
    }
    g.ret(c, 0)
}

fn clear_channels(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    c.xmm[0] = [0; 16];
    let speed = g.u32(ONE)?;
    c.xmm[1] = u128::from(speed).to_le_bytes();
    c.ecx = u32::MAX;
    c.edx = 0;
    logic(c, 0, 1 << 31);

    // Starting a replacement animation leaves the primary channel running.
    let first = if pc == 0x1ccba0 { 0 } else { 1 };
    for channel in first..3 {
        let base = at(c.eax, 0x20 * channel);
        g.set_u32(base, u32::MAX)?;
        g.set_u16(at(base, 4), u16::MAX)?;
        g.set_u16(at(base, 6), u16::MAX)?;
        g.set_u32(at(base, 0x1c), 0)?;
        g.set_u8(at(base, 0x10), 0)?;
        g.set_u8(at(base, 0x11), 0)?;
        g.set_u16(at(base, 0x12), 0)?;
        g.set_u32(at(base, 0x18), speed)?;
        g.set_u16(at(base, 0x14), 0)?;
        g.set_u16(at(base, 0x16), 0)?;
        g.set_u32(at(base, 8), u32::MAX)?;
        g.set_u8(at(base, 0xc), u8::MAX)?;
        g.set_u8(at(base, 0xd), u8::MAX)?;
        g.set_u16(at(base, 0xe), u16::MAX)?;
    }
    g.ret(c, 0)
}

fn select_action(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cccd0 {
        c.esp = c.esp.wrapping_sub(0x20);
        push(c, g, c.ebp)?;
        c.ebp = g.u32(at(c.esp, 0x38))?;
        push(c, g, c.esi)?;
        push(c, g, c.edi)?;
        c.edi = u32::MAX;
        g.set_u16(at(c.esp, 0x40), u16::MAX)?;
        g.set_u16(at(c.esp, 0x42), u16::MAX)?;

        c.eax = 0;
        c.esi = u32::MAX;
        while c.eax < 4 {
            let row = 0x46fbf8 + 8 * c.eax;
            if c.ecx == g.u32(row)? {
                c.esi = g.u32(row + 4)?;
                break;
            }
            c.eax += 1;
        }
        let enabled = g.u8(0x46fbf4)?;
        c.eax = byte(c.eax, enabled);
        logic(c, u32::from(enabled), 0x80);
        g.set_u32(c.ebp, c.edi)?;
        if enabled != 0 {
            sub_flags(c, c.esi, c.edi, 32);
            if c.esi != c.edi {
                c.ecx = g.u32(at(c.esp, 0x3c))?;
                c.edx = g.u32(at(c.esp, 0x38))?;
                c.eax = at(c.esp, 0x40);
                push(c, g, c.eax)?;
                c.eax = g.u32(at(c.esp, 0x38))?;
                for arg in [0, c.esi, c.ecx, c.edx, c.eax] {
                    push(c, g, arg)?;
                }
                c.eax = g.u32(at(c.esp, 0x48))?;
                c.ecx = at(c.esp, 0x24);
                c.edx = at(c.esp, 0x34);
                return g.call(c, 0x1cc3f0, 0x1ccd39);
            }
            c.eax = g.u32(at(c.esp, 0x40))?;
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            g.set_u32(c.ebx, c.eax)?;
        } else {
            c.ecx = g.u32(at(c.esp, 0x40))?;
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            g.set_u32(c.ebx, c.ecx)?;
        }
        c.eax = c.ebx;
    } else {
        let success = c.eax as u8 != 0;
        c.eax = c.ebx;
        if success {
            c.ecx = g.u32(at(c.esp, 0x40))?;
            g.set_u32(c.ebp, c.esi)?;
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            g.set_u32(c.ebx, c.ecx)?;
        } else {
            g.set_u16(at(c.esp, 0x40), c.edi as u16)?;
            g.set_u16(at(c.esp, 0x42), c.edi as u16)?;
            c.edx = g.u32(at(c.esp, 0x40))?;
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            g.set_u32(c.ebx, c.edx)?;
        }
    }
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x20, false);
    g.ret(c, 0x14)
}
