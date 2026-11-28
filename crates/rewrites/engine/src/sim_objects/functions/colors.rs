//! `be8e0` OBJECT_CHANGE_COLORS `(object: stack 0) -> al; ret 4`
//! (docs/engine/object-update.md, "Slice 3 bodies").
//!
//! The per-function port (`sim_physics::colors`, now a test oracle) taken
//! into the body table. When the definition's byte `+1c` has bit 0 set,
//! each change colour (body `+124` bytes, `+126` offset, `0c`-byte RGB
//! records; definition `+b0` per-colour blocks of `10` bytes) starts from
//! its base RGB, evaluates every permutation function (`bab40`, then
//! `131c20` to interpolate the RGB), then every scale function (`bab40`,
//! then a multiply), and clamps each channel to `[0, 1]`.
//!
//! Frame (`E` the entry `esp`, after the four pushes `esp = E - 34`):
//! `[esp+13]` the result byte, `[esp+14]` the permutation record offset,
//! `[esp+18]` the colour block offset, `[esp+1c]` the base RGB, `[esp+20]`
//! the permutation value, `[esp+24]` the permutation index (after the
//! entry's store of the definition there), `[esp+28]` the scale value,
//! `[esp+2c]` the colours left, `[esp+30]` the definition, `[esp+38]` the
//! object.

use super::{byte, call, fp_error, load, pop, push, ret, unowned, ONE};
use guest_layout::datum::layout;
use crate::light_constants::Sse;
use guest_layout::points;
use crate::sim_objects::{Cx, Next, Result};
use runtime::{CpuAbi, Guest};

pub(in crate::sim_objects) const COLORS_SITES: [u32; 3] = [0xbe9c1, 0xbe9e4, 0xbea2c];
const INTERPOLATE_RGB: u32 = 0x0013_1c20;

/// `pop esi; add esp, 24h; ret 4`.
fn leave(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.esi = pop(c, g)?;
    c.esp = c.esp.wrapping_add(36);
    ret(c, 4)
}

/// `bab40(object, function, &[esp + out])`.
fn function(c: &mut CpuAbi, g: &mut Guest, out: u32, site: u32) -> Result<Next> {
    c.edx = g.u32(c.esp.wrapping_add(0x38))?;
    c.ecx = c.esp.wrapping_add(out);
    push(c, g, c.ecx)?;
    push(c, g, c.eax)?;
    push(c, g, c.edx)?;
    call(c, points::OBJECT_FUNCTION_VALUE.base, site)
}

pub(in crate::sim_objects) fn colors(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::OBJECT_CHANGE_COLORS.base;
    if pc != entry && !COLORS_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
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
            // `beb1b`: `mov al, cl`.
            c.eax = byte(c.eax, c.ecx as u8);
            return leave(c, g);
        }
        c.edx = g.u16(c.eax.wrapping_add(292))? as i16 as i32 as u32;
        push(c, g, c.ebx)?;
        let offset = g.u16(c.eax.wrapping_add(294))? as i16 as i32 as u32;
        c.ebx = offset.wrapping_add(c.eax);
        // `mul` by `aaaaaaab`, `shr 3`, `cdq`, `sub`, `sar 1`: an unsigned
        // division by 24 (the high product is below 2^32, so `cdq` gives 0).
        c.eax = c.edx / 24;
        c.edx = 0;
        if c.eax == 0 {
            // `beb11`.
            c.ebx = pop(c, g)?;
            c.eax = byte(c.eax, c.ecx as u8);
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
    if pc == COLORS_SITES[0] {
        if c.eax as u8 == 0 {
            load(c, 0, 0);
            g.set_u32(sp.wrapping_add(32), 0)?;
        }
        c.eax = g.u32(c.edi.wrapping_add(4))?;
        load(c, 5, g.u32(sp.wrapping_add(32))?);
        c.esi = c.edi.wrapping_add(20);
        c.ecx = c.edi.wrapping_add(8);
        c.edi = c.ebx;
        return call(c, INTERPOLATE_RGB, COLORS_SITES[1]);
    }
    let mut new_color = pc == entry;
    let mut resumed_permutation = pc == COLORS_SITES[1];
    let mut resumed_scale = pc == COLORS_SITES[2];
    let mut s = Sse { mxcsr: c.mxcsr };
    if resumed_scale {
        let scale = if c.eax as u8 == 0 {
            g.set_u32(sp.wrapping_add(40), 0)?;
            0
        } else {
            g.u32(sp.wrapping_add(40))?
        };
        load(c, 0, scale);
        for off in [0, 4, 8] {
            let dest = c.ebx.wrapping_add(off);
            let v = g.u32(dest)?;
            let result = if off == 0 { s.mul(scale, v) } else { s.mul(v, scale) }.map_err(fp_error)?;
            g.set_u32(dest, result)?;
            load(c, 1, result);
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
            let count = g.u32(c.ebp.wrapping_add(8))?;
            g.set_u32(sp.wrapping_add(36), 0)?;
            if (count as i32) > 0 {
                g.set_u32(sp.wrapping_add(20), 0)?;
            }
        }
        if !resumed_scale {
            let mut index = g.u32(sp.wrapping_add(36))?;
            let mut count = g.u32(c.ebp.wrapping_add(8))? as i32;
            while resumed_permutation || (index as i32) < count {
                if !resumed_permutation {
                    let offset = g.u32(sp.wrapping_add(20))?;
                    c.edi = g.u32(c.ebp.wrapping_add(12))?;
                    c.edx = offset;
                    c.eax = g.u32(c.edi.wrapping_add(offset).wrapping_add(36))?;
                    c.edi = c.edi.wrapping_add(offset);
                    if c.eax != 0 {
                        return function(c, g, 32, COLORS_SITES[0]);
                    }
                }
                resumed_permutation = false;
                index = g.u32(sp.wrapping_add(36))?.wrapping_add(1);
                c.eax = index;
                c.edx = g.u32(sp.wrapping_add(20))?.wrapping_add(40);
                c.ecx = g.u32(c.ebp.wrapping_add(8))?;
                count = c.ecx as i32;
                g.set_u32(sp.wrapping_add(36), index)?;
                g.set_u32(sp.wrapping_add(20), c.edx)?;
            }
            c.eax = g.u32(c.ebp.wrapping_add(8))?;
            c.esi = 0;
            if (c.eax as i32) > 0 {
                c.edi = 0;
            }
        }
        while resumed_scale || (c.esi as i32) < c.eax as i32 {
            if !resumed_scale {
                c.eax = g.u32(c.ebp.wrapping_add(12))?.wrapping_add(c.edi);
                c.eax = g.u32(c.eax.wrapping_add(32))?;
                if c.eax != 0 {
                    return function(c, g, 40, COLORS_SITES[2]);
                }
            }
            resumed_scale = false;
            c.eax = g.u32(c.ebp.wrapping_add(8))?;
            c.esi = c.esi.wrapping_add(1);
            c.edi = c.edi.wrapping_add(40);
        }
        // `bea73`: clamp each channel to `[0, 1]`.
        load(c, 0, 0);
        let max = g.u32(ONE)?;
        load(c, 2, max);
        for (i, reg) in [(0, 1), (4, 1), (8, 0)] {
            let dest = c.ebx.wrapping_add(i);
            let v = s.clamp(g.u32(dest)?, 0, max).map_err(fp_error)?;
            g.set_u32(dest, v)?;
            load(c, reg, v);
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
