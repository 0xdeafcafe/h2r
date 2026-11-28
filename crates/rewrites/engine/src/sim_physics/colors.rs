//! Dynamic object change colours. Each channel starts from its base RGB,
//! evaluates all permutation functions, then all scale functions, then
//! clamps. The two function passes may read the object's live outputs.
//! A test oracle (`ORACLES`): `crate::sim_objects::functions::colors` is
//! the installed body.
use super::spatial::{fp_error, scalar};
use super::*;
use crate::{datum::layout, light_constants::Sse};

pub(super) const SITES: [u32; 3] = [0xbe9c1, 0xbe9e4, 0xbea2c];
const RGB_UPPER_BOUND: u32 = 0x0045_dbc0;
const INTERPOLATE_RGB: u32 = 0x0013_1c20;

fn leave(c: &mut CpuAbi, g: &Guest) -> Result<TransferAbi> {
    c.esi = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 36, false);
    g.ret(c, 4)
}

fn function(c: &mut CpuAbi, g: &mut Guest, out: u32, resume: u32) -> Result<TransferAbi> {
    logic(c, c.eax, 1 << 31);
    c.edx = g.u32(c.esp.wrapping_add(56))?;
    c.ecx = c.esp.wrapping_add(out);
    push(c, g, c.ecx)?;
    push(c, g, c.eax)?;
    call_arg(c, g, c.edx, points::OBJECT_FUNCTION_VALUE.base, resume)
}

pub(super) fn update(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    let entry = points::OBJECT_CHANGE_COLORS.base;
    if pc == entry {
        c.esp = c.esp.wrapping_sub(36);
        let handle = g.u32(c.esp.wrapping_add(40))?;
        c.ecx = g.u32(points::OBJECTS.base)?;
        c.edx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
        c.eax = g.u32(c.edx.wrapping_add((handle & 0xffff) * 12 + 8))?;
        c.ecx = (g.u32(c.eax)? & 0xffff) << 4;
        c.edx = g.u32(points::TAG_ENTRIES.base)?;
        push(c, g, c.esi)?;
        c.esi = g.u32(c.edx.wrapping_add(c.ecx).wrapping_add(8))?;
        c.edx = byte(c.edx, g.u8(c.esi.wrapping_add(28))?);
        c.ecx &= !0xff;
        g.set_u32(c.esp.wrapping_add(36), c.esi)?;
        if c.edx & 1 == 0 {
            c.eax &= !0xff;
            return leave(c, g);
        }
        let bytes = g.u16(c.eax.wrapping_add(292))? as i16 as i32 as u32;
        push(c, g, c.ebx)?;
        let offset = g.u16(c.eax.wrapping_add(294))? as i16 as i32 as u32;
        c.ebx = c.eax.wrapping_add(offset);
        c.eax = bytes / 24;
        c.edx = 0;
        if c.eax == 0 {
            c.ebx = pop(c, g)?;
            return leave(c, g);
        }
        push(c, g, c.ebp)?;
        c.ecx = c.eax * 3;
        g.set_u32(c.esp.wrapping_add(24), c.ebx)?;
        push(c, g, c.edi)?;
        g.set_u8(c.esp.wrapping_add(19), 1)?;
        g.set_u32(c.esp.wrapping_add(24), 0)?;
        c.ebx = c.ebx.wrapping_add(c.ecx * 4);
        g.set_u32(c.esp.wrapping_add(44), c.eax)?;
    }
    let sp = c.esp;
    if pc == SITES[0] {
        logic(c, c.eax & 0xff, 0x80);
        if c.eax as u8 == 0 {
            scalar(c, 0, 0);
            g.set_u32(sp.wrapping_add(32), 0)?;
            logic(c, 0, 0x80); // TEST AL,AL
        }
        c.eax = g.u32(c.edi.wrapping_add(4))?;
        scalar(c, 5, g.u32(sp.wrapping_add(32))?);
        c.esi = c.edi.wrapping_add(20);
        c.ecx = c.edi.wrapping_add(8);
        c.edi = c.ebx;
        return g.call(c, INTERPOLATE_RGB, SITES[1]);
    }
    let mut new_color = pc == entry;
    let mut resumed_permutation = pc == SITES[1];
    let mut resumed_scale = pc == SITES[2];
    let mut s = Sse { mxcsr: c.mxcsr };
    if resumed_scale {
        let scale = if c.eax as u8 == 0 {
            g.set_u32(sp.wrapping_add(40), 0)?;
            0
        } else { g.u32(sp.wrapping_add(40))? };
        scalar(c, 0, scale);
        for off in [0, 4, 8] {
            let dest = c.ebx.wrapping_add(off);
            let v = g.u32(dest)?;
            let result = if off == 0 { s.mul(scale, v) } else { s.mul(v, scale) }.map_err(fp_error)?;
            g.set_u32(dest, result)?;
            scalar(c, 1, result);
        }
        c.mxcsr = s.mxcsr;
    }
    loop {
        if new_color {
            c.ebp = g.u32(c.esi.wrapping_add(176))?.wrapping_add(g.u32(sp.wrapping_add(24))?);
            let src = g.u32(sp.wrapping_add(28))?;
            c.ecx = c.ebx;
            c.esi = g.u32(src)?;
            g.set_u32(c.ebx, c.esi)?;
            c.esi = g.u32(src.wrapping_add(4))?;
            g.set_u32(c.ebx.wrapping_add(4), c.esi)?;
            c.eax = g.u32(src.wrapping_add(8))?;
            g.set_u32(c.ebx.wrapping_add(8), c.eax)?;
            g.set_u32(sp.wrapping_add(36), 0)?;
            if (g.u32(c.ebp.wrapping_add(8))? as i32) > 0 {
                g.set_u32(sp.wrapping_add(20), 0)?;
            }
        }
        if !resumed_scale {
            let mut index = g.u32(sp.wrapping_add(36))?;
            let mut count = g.u32(c.ebp.wrapping_add(8))? as i32;
            while resumed_permutation || (index as i32) < count {
                if !resumed_permutation {
                    let offset = g.u32(sp.wrapping_add(20))?;
                    c.edi = g.u32(c.ebp.wrapping_add(12))?.wrapping_add(offset);
                    c.eax = g.u32(c.edi.wrapping_add(36))?;
                    if c.eax != 0 { return function(c, g, 32, SITES[0]); }
                }
                resumed_permutation = false;
                index = g.u32(sp.wrapping_add(36))?.wrapping_add(1);
                c.edx = g.u32(sp.wrapping_add(20))?.wrapping_add(40);
                c.ecx = g.u32(c.ebp.wrapping_add(8))?;
                count = c.ecx as i32;
                g.set_u32(sp.wrapping_add(36), index)?;
                g.set_u32(sp.wrapping_add(20), c.edx)?;
            }
            c.eax = g.u32(c.ebp.wrapping_add(8))?;
            c.esi = 0;
            if (c.eax as i32) > 0 { c.edi = 0; }
        }
        while resumed_scale || (c.esi as i32) < c.eax as i32 {
            if !resumed_scale {
                let record = g.u32(c.ebp.wrapping_add(12))?.wrapping_add(c.edi);
                c.eax = g.u32(record.wrapping_add(32))?;
                if c.eax != 0 { return function(c, g, 40, SITES[2]); }
            }
            resumed_scale = false;
            c.eax = g.u32(c.ebp.wrapping_add(8))?;
            c.esi = c.esi.wrapping_add(1);
            c.edi = c.edi.wrapping_add(40);
        }
        let max = g.u32(RGB_UPPER_BOUND)?;
        scalar(c, 2, max);
        for (i, reg) in [(0,1), (4,1), (8,0)] {
            let dest = c.ebx.wrapping_add(i);
            let v = s.clamp(g.u32(dest)?, 0, max).map_err(fp_error)?;
            g.set_u32(dest, v)?;
            scalar(c, reg, v);
        }
        c.mxcsr = s.mxcsr;
        c.esi = g.u32(sp.wrapping_add(24))?.wrapping_add(16);
        c.ecx = g.u32(sp.wrapping_add(28))?.wrapping_add(12);
        c.ebx = c.ebx.wrapping_add(12);
        c.eax = g.u32(sp.wrapping_add(44))?.wrapping_sub(1);
        g.set_u32(sp.wrapping_add(24), c.esi)?;
        g.set_u32(sp.wrapping_add(28), c.ecx)?;
        g.set_u32(sp.wrapping_add(44), c.eax)?;
        if c.eax == 0 {
            c.eax = byte(c.eax, g.u8(sp.wrapping_add(19))?);
            c.edi = pop(c, g)?;
            c.ebp = pop(c, g)?;
            c.ebx = pop(c, g)?;
            return leave(c, g);
        }
        new_color = true;
        c.esi = g.u32(sp.wrapping_add(48))?;
        c.edx = 0;
    }
}
