//! Walk both object-cluster reference lists. Mark a slot before testing
//! its bounds, so a later reference is suppressed even if the first missed.
use super::spatial::{fp_error, scalar};
use super::*;
use crate::{datum::layout, light_constants::Sse};

pub(crate) const SITES: [u32; 3] = [0xbb0e9, 0xbb17e, 0xbb2c4];
const FRAME: u32 = 0x420;
const ARG: u32 = FRAME + 20;
const QUERY_ACTIVE: u32 = 0x004d_e2f8;
const GENERATION: u32 = 0x004d_e2fc;
const VISITED: u32 = 0x004d_e300;
const PORTAL_ACTIVE: u32 = 0x004e_7411;
const PORTAL_EPOCH: u32 = 0x004e_7414;
const RADIUS_THRESHOLD: u32 = 0x0045_dbd8;
const LIST_A: u32 = 0x004d_e2e4;
const LIST_B: u32 = 0x004d_e2d8;

fn end(c: &mut CpuAbi, g: &mut Guest, count: u16, active: u8) -> Result<TransferAbi> {
    c.eax = (c.eax & !0xffff) | u32::from(count);
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    g.set_u8(QUERY_ACTIVE, active)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    g.ret(c, 28)
}

fn overlaps(c: &mut CpuAbi, g: &Guest, first: bool) -> Result<bool> {
    let mut s = Sse { mxcsr: c.mxcsr };
    let mut delta = |off: u32| -> Result<u32> {
        s.sub(g.u32(c.ebx.wrapping_add(4 + off))?, g.u32(c.edi.wrapping_add(off))?).map_err(fp_error)
    };
    let z = delta(8)?;
    let (x,y) = if first {
        let y = delta(4)?; (delta(0)?, y)
    } else {
        (delta(0)?, delta(4)?)
    };
    let radius = u128::from_le_bytes(c.xmm[4]) as u32;
    let zz = s.mul(z,z).map_err(fp_error)?;
    let (middle, last) = if first { (y,x) } else { (x,y) };
    let mm = s.mul(middle,middle).map_err(fp_error)?;
    let ll = s.mul(last,last).map_err(fp_error)?;
    let reach = s.add(g.u32(c.ebx.wrapping_add(16))?, radius).map_err(fp_error)?;
    let partial = s.add(zz,mm).map_err(fp_error)?;
    let dist = s.add(partial,ll).map_err(fp_error)?;
    let rr = s.mul(reach,reach).map_err(fp_error)?;
    let hit = !s.below(rr, dist).map_err(fp_error)?;
    c.mxcsr = s.mxcsr;
    scalar(c, 0, reach);
    scalar(c, 1, rr);
    scalar(c, 2, if first { ll } else { y });
    scalar(c, 3, mm);
    scalar(c, 5, dist);
    Ok(hit)
}

// EAX holds the object handle, EBX the first reference's bounds, EDX its
// next-reference datum. Return true when the caller's result array is full.
fn candidates(c: &mut CpuAbi, g: &mut Guest, first: bool) -> Result<bool> {
    let sp = c.esp;
    let table = if first { LIST_A } else { LIST_B };
    let next_at = sp.wrapping_add(if first { 32 } else { 28 });
    if c.eax == u32::MAX { return Ok(false); }
    c.ebx = g.u32(sp.wrapping_add(24))?;
    c.edx = g.u32(next_at)?;
    c.ebp = g.u32(table)?;
    loop {
        c.esi = 1u32.wrapping_shl(u32::from(g.u8(c.ebx)?));
        c.ecx = g.u32(sp.wrapping_add(ARG + 4))?;
        if c.ecx & c.esi != 0 {
            c.esi = g.u32(GENERATION)?;
            c.ecx = c.eax & 0xffff;
            let mark = VISITED + c.ecx * 4;
            if g.u32(mark)? != c.esi {
                g.set_u32(mark, c.esi)?;
                if overlaps(c, g, first)? {
                    c.ecx = g.u32(sp.wrapping_add(16))?;
                    if c.ecx as i16 >= g.u16(sp.wrapping_add(ARG + 24))? as i16 {
                        return Ok(true);
                    }
                    let out = g.u32(sp.wrapping_add(ARG + 20))?;
                    c.esi = c.ecx as i16 as i32 as u32;
                    c.ecx = c.ecx.wrapping_add(1);
                    g.set_u32(out.wrapping_add(c.esi.wrapping_mul(4)), c.eax)?;
                    c.ebp = g.u32(table)?;
                    g.set_u32(sp.wrapping_add(16), c.ecx)?;
                }
            }
        }
        if c.edx == u32::MAX {
            c.ecx = u32::MAX;
        } else {
            c.esi = g.u32(c.ebp.wrapping_add(layout::ELEMENT_SIZE))?;
            c.edi = g.u32(c.ebp.wrapping_add(layout::DATA))?;
            let record = c.edi.wrapping_add((c.edx & 0xffff).wrapping_mul(c.esi));
            c.edx = g.u32(record.wrapping_add(8))?;
            c.ebx = record.wrapping_add(12);
            c.ecx = g.u32(record.wrapping_add(4))?;
            c.edi = g.u32(sp.wrapping_add(ARG + 12))?;
            g.set_u32(next_at, c.edx)?;
        }
        c.eax = c.ecx;
        if c.eax == u32::MAX { break; }
    }
    if first { c.esi = g.u32(sp.wrapping_add(44))?; }
    Ok(false)
}

pub(crate) fn objects(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    if pc == points::OBJECT_SPATIAL_VISIT.base {
        c.esp = c.esp.wrapping_sub(FRAME);
        c.eax = g.u32(c.esp.wrapping_add(FRAME + 8))?;
        push(c, g, c.ebx)?;
        push(c, g, c.ebp)?;
        c.ebx = 0;
        push(c, g, c.esi)?;
        push(c, g, c.edi)?;
        let sp = c.esp;
        g.set_u32(sp.wrapping_add(16), 0)?;
        if c.eax == 0 { g.set_u32(sp.wrapping_add(ARG + 4), u32::MAX)?; }
        if g.u32(sp.wrapping_add(ARG))? == 0 { g.set_u32(sp.wrapping_add(ARG), u32::MAX)?; }
        let location = g.u32(sp.wrapping_add(ARG + 8))?;
        let cluster = g.u16(location.wrapping_add(4))?;
        c.eax = (location & !0xffff) | u32::from(cluster);
        c.edi = g.u32(sp.wrapping_add(ARG + 12))?;
        g.set_u32(sp.wrapping_add(20), 0)?;
        if cluster != u16::MAX {
            let radius = g.u32(sp.wrapping_add(ARG + 16))?;
            scalar(c, 4, radius);
            let mut s = Sse { mxcsr: c.mxcsr };
            let query = s.above(radius, g.u32(RADIUS_THRESHOLD)?).map_err(fp_error)?;
            c.mxcsr = s.mxcsr;
            if query {
                let epoch = g.u32(PORTAL_EPOCH)?;
                let next = arithmetic(c, epoch, 1, false);
                // COMISS above cleared carry; INC preserves it.
                c.eflags &= !1;
                g.set_u32(PORTAL_EPOCH, next)?;
                c.ecx = sp.wrapping_add(48);
                c.edx = radius;
                for arg in [c.ecx, 512, c.edx, c.edi, c.eax] { push(c, g, arg)?; }
                g.set_u8(PORTAL_ACTIVE, 1)?;
                return g.call(c, 0x14a5b0, SITES[0]);
            }
            g.set_u32(sp.wrapping_add(20), 1)?;
            g.set_u16(sp.wrapping_add(48), cluster)?;
        }
    }
    let sp = c.esp;
    if pc == SITES[0] {
        g.set_u8(PORTAL_ACTIVE, c.ebx as u8)?;
        let n = if c.eax as i16 > 512 { 512 } else { c.eax };
        g.set_u32(sp.wrapping_add(20), n)?;
    }
    if pc == points::OBJECT_SPATIAL_VISIT.base || pc == SITES[0] {
        scalar(c, 4, g.u32(sp.wrapping_add(ARG + 16))?);
        c.edx = g.u32(GENERATION)?.wrapping_add(1);
        c.eax = 0;
        g.set_u32(GENERATION, c.edx)?;
        g.set_u8(QUERY_ACTIVE, 1)?;
        g.set_u32(sp.wrapping_add(36), 0)?;
        if g.u16(sp.wrapping_add(20))? as i16 <= c.ebx as i16 {
            return end(c, g, g.u16(sp.wrapping_add(16))?, c.ebx as u8);
        }
        c.ecx = g.u32(sp.wrapping_add(ARG))? & 1;
        g.set_u32(sp.wrapping_add(40), c.ecx)?;
    }
    let mut second = pc == SITES[1];
    let mut advance = pc == SITES[2];
    if second || advance {
        if candidates(c, g, second)? { return end(c, g, c.ecx as u16, 0); }
        c.ebx = 0;
    }
    loop {
        if advance {
            c.eax = g.u32(sp.wrapping_add(36))?.wrapping_add(1);
            g.set_u32(sp.wrapping_add(36), c.eax)?;
            if c.eax as i16 >= g.u16(sp.wrapping_add(20))? as i16 {
                return end(c, g, g.u16(sp.wrapping_add(16))?, c.ebx as u8);
            }
            c.ecx = g.u32(sp.wrapping_add(40))?;
        }
        if !second {
            c.eax = c.eax as i16 as i32 as u32;
            c.esi = g.u16(sp.wrapping_add(48).wrapping_add(c.eax.wrapping_mul(2)))?.into();
            g.set_u32(sp.wrapping_add(44), c.esi)?;
            arithmetic(c, c.ecx, c.ebx, true);
            if c.ecx != c.ebx {
                c.ecx = sp.wrapping_add(24);
                c.edx = c.esi as i16 as i32 as u32;
                push(c, g, c.ecx)?;
                g.set_u32(sp.wrapping_add(24), c.ebx)?;
                push(c, g, c.edx)?;
                c.ebx = sp.wrapping_add(32);
                return g.call(c, 0xb8940, SITES[1]);
            }
        }
        let bit = g.u8(sp.wrapping_add(ARG))? & 2;
        logic(c, bit.into(), 0x80);
        if bit != 0 {
            c.edx = sp.wrapping_add(24);
            c.eax = c.esi as i16 as i32 as u32;
            push(c, g, c.edx)?;
            g.set_u32(sp.wrapping_add(24), c.ebx)?;
            push(c, g, c.eax)?;
            c.ebx = sp.wrapping_add(28);
            return g.call(c, 0xb8a10, SITES[2]);
        }
        second = false;
        advance = true;
    }
}
