//! Traverse the element's frame, sound and effect lists over [low, high).
//! Each callback gets its own record in the suspended guest frame.
use super::*;

fn reload_interval(c: &mut CpuAbi, g: &impl Memory) -> Checked<()> {
    scalar(c, 2, g.u32(at(c.esp, 0x48))?);
    scalar(c, 1, g.u32(at(c.esp, 0x44))?);
    Ok(())
}

fn in_interval(c: &mut CpuAbi, g: &mut impl Memory, frame: u16, local: u32) -> Checked<bool> {
    c.ecx = signed(frame);
    cvt(c, 0, c.ecx);
    let f = bits(c, 0);
    let lower = g.u32(at(c.esp, 0x10))?;
    // JB includes unordered. The upper test rejects unordered and equal.
    let flags = simd::comiss(&mut c.mxcsr, f, lower).expect("masked SSE");
    g.set_u32(at(c.esp, local), f)?;
    if flags & 1 != 0 {
        return Ok(false);
    }
    scalar(c, 3, g.u32(at(c.esp, 0x14))?);
    Ok(above(c, bits(c, 3), f))
}

fn frame_event(c: &mut CpuAbi, g: &mut impl Memory) -> Checked<bool> {
    c.ecx = g.u32(at(c.esp, 0x3c))?;
    c.edx = g.u32(at(c.ecx, 0x50))?;
    c.eax = at(c.edx, c.esi.wrapping_mul(4));
    c.edx = w16(c.edx, g.u16(at(c.eax, 2))?);
    if !in_interval(c, g, c.edx as u16, 0x18)? {
        return Ok(false);
    }
    c.ecx = signed(g.u16(c.eax)?);
    c.ebp = g.u32(at(c.esp, 12))? | 1u32.wrapping_shl(c.ecx);
    g.set_u32(at(c.esp, 12), c.ebp)?;
    if c.ebx == 0 {
        return Ok(false);
    }
    c.eax = 0;
    logic(c, 0, 1 << 31);
    g.set_u32(at(c.esp, 0x28), c.ecx)?;
    c.ecx = at(c.esp, 0x20);
    g.set_u16(at(c.esp, 0x22), c.edx as u16)?;
    c.edx = g.u32(at(c.esp, 0x18))?;
    push(c, g, c.ecx)?;
    g.set_u16(at(c.esp, 0x24), 0)?;
    g.set_u32(at(c.esp, 0x30), 0)?;
    c.eax = g.u32(at(c.esp, 0x50))?;
    push(c, g, c.edx)?;
    push(c, g, c.eax)?;
    g.set_u32(at(c.esp, 0x30), u32::MAX)?;
    let name = g.u32(0x1c7519)?;
    g.set_u32(at(c.esp, 0x3c), name)?;
    Ok(true)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sound,
    Effect,
}

/// Both resource event lists filter the graph reference by its direction
/// and object flags. They write different payloads for the callback.
fn resource_event(c: &mut CpuAbi, g: &mut impl Memory, kind: Kind) -> Checked<bool> {
    let sound = kind == Kind::Sound;
    let local = if sound { 0x18 } else { 0x1c };
    c.edx = g.u32(at(c.esp, 0x3c))?;
    c.eax = g.u32(at(c.edx, if sound { 0x58 } else { 0x60 }))?;
    let event = if sound {
        at(c.eax, c.ebp.wrapping_mul(8))
    } else {
        at(c.eax, c.edi.wrapping_mul(4))
    };
    let frame = g.u16(at(event, 2))?;
    if sound {
        c.edi = w16(c.edi, frame);
        c.esi = event;
    } else {
        c.esi = w16(c.esi, frame);
        c.eax = event;
    }
    if !in_interval(c, g, frame, local)? {
        return Ok(false);
    }
    c.eax = w16(c.eax, g.u16(event)?);
    if c.eax as u16 == u16::MAX {
        return Ok(false);
    }
    c.edx = signed(c.eax as u16).wrapping_mul(3);
    c.eax = g.u32(at(c.esp, 0x38))?;
    c.ecx = g.u32(at(c.eax, if sound { 0x18 } else { 0x20 }))?;
    c.eax = at(c.ecx, c.edx.wrapping_mul(4));
    if c.eax == 0 {
        return Ok(false);
    }
    c.edx = g.u32(at(c.eax, 4))?;
    if c.edx == u32::MAX || (sound && c.ebx == 0) {
        return Ok(false);
    }
    c.eax = w16(c.eax, g.u16(at(c.eax, 8))?);
    for (flag, require) in [(2, 0x2000), (4, 0x1000)] {
        if c.eax & flag != 0 {
            c.ecx = g.u32(at(c.esp, 0x40))?;
            if c.ecx & require == 0 {
                return Ok(false);
            }
        }
    }
    c.ecx = w8(c.ecx, 1);
    if c.eax & 16 != 0 && above(c, bits(c, 1), bits(c, 2)) {
        c.ecx = w8(c.ecx, 0);
    }
    if c.eax & 32 != 0 && above(c, bits(c, 2), bits(c, 1)) {
        return Ok(false);
    }
    if c.ecx as u8 == 0 || c.ebx == 0 {
        return Ok(false);
    }
    logic(
        c,
        if sound { 1 } else { c.ebx },
        if sound { 128 } else { 1 << 31 },
    );
    if sound {
        g.set_u32(at(c.esp, 0x24), c.edx)?;
        c.edx = g.u32(at(c.esi, 4))?;
        c.eax &= 0xffff;
        c.ecx = at(c.esp, 0x20);
        g.set_u32(at(c.esp, 0x30), c.edx)?;
        c.edx = g.u32(at(c.esp, 0x18))?;
        push(c, g, c.ecx)?;
        g.set_u32(at(c.esp, 0x30), c.eax)?;
        c.eax = g.u32(at(c.esp, 0x50))?;
        push(c, g, c.edx)?;
        push(c, g, c.eax)?;
    } else {
        c.ecx = g.u32(at(c.esp, 0x1c))?;
        g.set_u32(at(c.esp, 0x24), c.edx)?;
        c.edx = c.eax & 0xffff;
        c.eax = at(c.esp, 0x20);
        push(c, g, c.eax)?;
        g.set_u32(at(c.esp, 0x30), c.edx)?;
        c.edx = g.u32(at(c.esp, 0x50))?;
        push(c, g, c.ecx)?;
        push(c, g, c.edx)?;
    }
    g.set_u16(at(c.esp, 0x2c), if sound { 1 } else { 2 })?;
    g.set_u32(at(c.esp, 0x34), u32::MAX)?;
    g.set_u16(at(c.esp, 0x2e), frame)?;
    if !sound {
        let name = g.u32(0x1c775e)?;
        g.set_u32(at(c.esp, 0x3c), name)?;
    }
    Ok(true)
}

pub(super) fn run(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    if pc == 0x1c7450 {
        c.esp = c.esp.wrapping_sub(40);
        scalar(c, 2, g.u32(at(c.esp, 0x3c))?);
        scalar(c, 1, g.u32(at(c.esp, 0x38))?);
        push(c, g, c.ebp)?;
        push(c, g, c.esi)?;
        c.esi = 0;
        let a = bits(c, 1);
        let b = bits(c, 2);
        let descending = above(c, a, b);
        push(c, g, c.edi)?;
        g.set_u32(at(c.esp, 12), 0)?;
        g.set_u32(at(c.esp, 16), b)?;
        if !descending {
            g.set_u32(at(c.esp, 16), a)?;
        }
        let descending = above(c, a, b);
        g.set_u32(at(c.esp, 20), a)?;
        if !descending {
            g.set_u32(at(c.esp, 20), b)?;
        }
        c.eax = w8(c.eax, g.u8(at(c.esp, 0x40))?);
        if c.eax & 128 == 0 {
            c.eax = g.u32(at(c.esp, 0x3c))?;
            c.edi = g.u32(at(c.eax, 0x4c))?;
        }
    } else {
        reload_interval(c, g)?;
    }

    if pc == 0x1c751f || (pc == 0x1c7450 && g.u8(at(c.esp, 0x40))? & 128 == 0) {
        if pc == 0x1c751f {
            c.esi = c.esi.wrapping_add(1);
        }
        while (c.esi as i32) < (c.edi as i32) {
            if frame_event(c, g)? {
                return g.call(c, c.ebx, 0x1c751f);
            }
            c.esi = c.esi.wrapping_add(1);
        }
    }
    if pc != 0x1c7764 {
        let sound_count = if pc == 0x1c7643 {
            c.eax = g.u32(at(c.esp, 28))?;
            c.ebp = c.ebp.wrapping_add(1);
            Some(c.eax)
        } else {
            c.eax = g.u32(at(c.esp, 0x40))?;
            if c.eax & 0x100 == 0 {
                c.ecx = g.u32(at(c.esp, 0x3c))?;
                c.eax = g.u32(at(c.ecx, 0x54))?;
                c.ebp = 0;
                g.set_u32(at(c.esp, 28), c.eax)?;
                Some(c.eax)
            } else {
                None
            }
        };
        if let Some(mut count) = sound_count {
            while (c.ebp as i32) < (count as i32) {
                if resource_event(c, g, Kind::Sound)? {
                    return g.call(c, c.ebx, 0x1c7643);
                }
                c.eax = g.u32(at(c.esp, 28))?;
                count = c.eax;
                c.ebp = c.ebp.wrapping_add(1);
            }
        }
    }
    let effects = if pc == 0x1c7764 {
        c.edi = c.edi.wrapping_add(1);
        true
    } else {
        c.eax = g.u32(at(c.esp, 0x40))?;
        if c.eax & 0x200 == 0 {
            c.ecx = g.u32(at(c.esp, 0x3c))?;
            c.ebp = g.u32(at(c.ecx, 0x5c))?;
            c.edi = 0;
            true
        } else {
            false
        }
    };
    if effects {
        while (c.edi as i32) < (c.ebp as i32) {
            if resource_event(c, g, Kind::Effect)? {
                return g.call(c, c.ebx, 0x1c7764);
            }
            c.edi = c.edi.wrapping_add(1);
        }
    }
    c.eax = g.u32(at(c.esp, 12))?;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    add_sp(c, 40);
    g.ret(c, 24)
}
