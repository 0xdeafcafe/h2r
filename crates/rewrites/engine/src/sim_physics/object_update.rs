//! Per-object pre/post-physics traversal. The current child is looked up
//! again after its callback: the callback may have changed its sibling.
use super::*;
use guest_layout::datum::layout;

const COMPONENTS: u32 = 0x0051_e9b8;
const COMPONENT_BYTES: u32 = 0xa0;
const NEXT_SIBLING: u32 = 0xc;
const FIRST_CHILD: u32 = 0x10;
const COMPONENT_HANDLE: u32 = 0xb4;
const PHYSICS_FLAGS: u32 = 0xc0;
const NONE: u32 = u32::MAX;

pub(super) const PRE: [u32; 8] = [
    0xbc4ca, 0xbc4ed, 0xbc51c, 0xbc524, 0xbc52c, 0xbc534, 0xbc546, 0xbc59a,
];
pub(super) const POST: [u32; 8] = [
    0xbc853, 0xbc867, 0xbc8e1, 0xbc8eb, 0xbc916, 0xbc92e, 0xbc93c, 0xbc961,
];

fn sibling(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    c.eax = g.u32(points::OBJECTS.base)?;
    c.ecx = g.u32(c.eax.wrapping_add(layout::DATA))?;
    c.edx = g.u32(c.ecx.wrapping_add(12 * (c.esi & 0xffff) + 8))?;
    c.esi = g.u32(c.edx.wrapping_add(NEXT_SIBLING))?;
    Ok(())
}

fn accumulate(c: &mut CpuAbi) {
    c.ebx = byte(c.ebx, (c.ebx | c.eax) as u8);
    logic(c, c.ebx & 0xff, 0x80);
}

fn pre_ret(c: &mut CpuAbi, g: &Guest, al: u8) -> Result<TransferAbi> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.eax = byte(c.eax, al);
    c.ebx = pop(c, g)?;
    g.ret(c, 4)
}

pub(super) fn pre(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    if pc == points::OBJECT_PRE_UPDATE.base {
        push(c, g, c.ebx)?;
        push(c, g, c.esi)?;
        push(c, g, c.edi)?;
        c.edi = g.u32(c.esp.wrapping_add(16))?;
        c.esi = 12 * (c.edi & 0xffff);
        c.eax = g.u32(points::OBJECTS.base)?;
        c.ecx = g.u32(c.eax.wrapping_add(layout::DATA))?.wrapping_add(c.esi);
        let flags = g.u8(c.ecx.wrapping_add(2))?;
        c.eax = byte(c.eax, flags);
        c.ebx &= !0xff;
        g.set_u32(c.esp.wrapping_add(16), c.ecx)?;
        logic(c, u32::from(flags & 2), 0x80);
        if flags & 2 == 0 { return pre_ret(c, g, 0); }
        logic(c, u32::from(flags & 32), 0x80);
        if flags & 32 != 0 { return pre_ret(c, g, 1); }

        c.eax = byte(c.eax, flags & !4);
        push(c, g, c.ebp)?;
        c.ebp = g.u32(c.ecx.wrapping_add(8))?;
        g.set_u8(c.ecx.wrapping_add(2), c.eax as u8)?;
        let bits = g.u8(c.ebp.wrapping_add(PHYSICS_FLAGS))? >> 2;
        c.edx = byte(c.edx, bits);
        if bits & 1 != 0 {
            logic(c, 1, 0x80);
            c.eax = c.edi;
            return g.call(c, 0xbba20, PRE[0]);
        }
    }
    if pc == points::OBJECT_PRE_UPDATE.base || pc == PRE[0] {
        c.eax = g.u8(g.u32(points::GAME_GLOBALS.base)?.wrapping_add(12))? as i8 as i32 as u32;
        if matches!(c.eax, 4 | 5) && g.u8(c.ebp.wrapping_add(216))? & 1 != 0 {
            logic(c, 1, 0x80);
            c.eax = c.edi;
            return g.call(c, 0xa9570, PRE[1]);
        }
    }
    if pc == points::OBJECT_PRE_UPDATE.base || PRE[..2].contains(&pc) {
        c.ecx = g.u32(points::OBJECTS.base)?;
        c.edx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
        c.ecx = g.u32(c.edx.wrapping_add(c.esi).wrapping_add(8))?;
        c.eax = NONE;
        let offset = g.u16(c.ecx.wrapping_add(298))?;
        if offset != u16::MAX {
            c.esi = (g.u16(c.ebp.wrapping_add(298))? as i16 as i32 as u32).wrapping_add(c.ebp);
            c.ecx = g.u32(c.esi.wrapping_add(104))?;
            arithmetic(c, c.ecx, c.eax, true);
            if c.ecx != c.eax {
                return g.call(c, 0x1cd4e0, PRE[2]);
            }
        } else {
            arithmetic(c, 0, 0, true);
        }
    }
    if pc == points::OBJECT_PRE_UPDATE.base || PRE[..3].contains(&pc) {
        if pc == PRE[2] { c.ebx = byte(c.ebx, c.eax as u8); }
        return call_arg(c, g, c.edi, 0x108c60, PRE[3]);
    }
    if pc == PRE[3] || pc == PRE[4] {
        accumulate(c);
        let (target, resume) = if pc == PRE[3] { (0xd5de0, PRE[4]) } else { (0xbe8e0, PRE[5]) };
        return call_arg(c, g, c.edi, target, resume);
    }
    if pc == PRE[5] || pc == PRE[6] {
        accumulate(c);
        if pc == PRE[5] { c.esi = g.u32(c.ebp.wrapping_add(FIRST_CHILD))?; }
        else { sibling(c, g)?; }
        arithmetic(c, c.esi, NONE, true);
        if c.esi != NONE {
            return call_arg(c, g, c.esi, points::OBJECT_PRE_UPDATE.base, PRE[6]);
        }
        c.eax = g.u32(c.ebp.wrapping_add(COMPONENT_HANDLE))?;
        if c.eax != NONE {
            c.ecx = byte(c.ecx, g.u8(c.ebp.wrapping_add(PHYSICS_FLAGS))? >> 6);
            if c.ecx & 1 != 0 {
                c.edx = g.u32(COMPONENTS)?;
                c.edi = (c.eax & 0xffff) * COMPONENT_BYTES;
                c.eax = g.u32(c.edx.wrapping_add(layout::DATA))?;
                c.edi = arithmetic(c, c.edi, c.eax, false);
                return g.call(c, 0x1ced00, PRE[7]);
            }
        }
    }
    let timer = g.u8(c.ebp.wrapping_add(179))?;
    c.eax = byte(c.eax, timer);
    if timer != 0 {
        let next = timer - 1;
        // TEST clears carry, and DEC preserves it.
        let af = u32::from(timer & 15 == 0) << 4;
        let of = u32::from(timer == 0x80) << 11;
        c.eflags = (c.eflags & !ARITH) | status(next.into(), 0x80) | af | of;
        c.eflags_defined |= ARITH;
        g.set_u8(c.ebp.wrapping_add(179), next)?;
        c.ebp = pop(c, g)?;
        return pre_ret(c, g, 1);
    }
    let result = c.ebx as u8;
    logic(c, result.into(), 0x80);
    if result == 0 {
        c.ecx = g.u32(c.esp.wrapping_add(20))?;
        let flags = g.u8(c.ecx.wrapping_add(2))?;
        c.eax = byte(c.eax, flags);
        logic(c, (flags & 4).into(), 0x80);
        if flags & 4 == 0 {
            g.set_u8(c.ecx.wrapping_add(2), flags & !2)?;
            logic(c, (flags & !2).into(), 0x80);
        }
    }
    c.ebp = pop(c, g)?;
    pre_ret(c, g, result)
}

pub(super) fn post(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    let entry = points::OBJECT_POST_UPDATE.base;
    if pc == entry {
        c.ecx = g.u32(points::OBJECTS.base)?;
        c.edx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
        c.esp = c.esp.wrapping_sub(32);
        push(c, g, c.ebp)?;
        push(c, g, c.esi)?;
        push(c, g, c.edi)?;
        c.edi = g.u32(c.esp.wrapping_add(48))?;
        c.eax = (c.edi & 0xffff) * 3;
        c.ebp = c.edx.wrapping_add(c.eax * 4);
        c.esi = g.u32(c.ebp.wrapping_add(8))?;
        let handle = g.u32(c.esi.wrapping_add(COMPONENT_HANDLE))?;
        arithmetic(c, handle, NONE, true);
        if handle != NONE { return call_arg(c, g, c.edi, 0xbc5e0, POST[0]); }
    }
    if pc == entry || pc == POST[0] {
        c.ecx = byte(c.ecx, g.u8(c.ebp.wrapping_add(3))?);
        c.eax = 1u32.wrapping_shl(c.ecx);
        logic(c, c.eax & 0xff, 0x80);
        if c.eax & 0x80 == 0 { return call_arg(c, g, c.edi, 0xbd090, POST[1]); }
        c.eax = g.u32(c.esi.wrapping_add(COMPONENT_HANDLE))?;
        arithmetic(c, c.eax, NONE, true);
        if c.eax != NONE {
            c.ecx = byte(c.ecx, g.u8(c.esi.wrapping_add(PHYSICS_FLAGS))? >> 6);
            logic(c, c.ecx & 1, 0x80);
            if c.ecx & 1 != 0 {
                c.edx = g.u32(COMPONENTS)?;
                c.edi = g.u32(c.edx.wrapping_add(layout::DATA))?;
                push(c, g, c.ebx)?;
                c.ebx = c.edi.wrapping_add((c.eax & 0xffff) * COMPONENT_BYTES);
                c.eax = g.u32(c.ebx.wrapping_add(4))? >> 15;
                logic(c, c.eax & 1, 0x80);
                if c.eax & 1 != 0 {
                    // One bit per 52-byte record, rounded up to a dword.
                    let bytes = g.u16(c.esi.wrapping_add(276))? as i16 as i32 as u32;
                    let words = (bytes / 52).div_ceil(32);
                    c.edx = words * 4;
                    c.eax = 0;
                    c.edi = c.esp.wrapping_add(16);
                    for _ in 0..words {
                        g.set_u32(c.edi, 0)?;
                        c.edi = if c.eflags & 0x400 != 0 { c.edi.wrapping_sub(4) } else { c.edi.wrapping_add(4) };
                    }
                    c.ecx = 0;
                    logic(c, 0, 1 << 31);
                    push(c, g, 0)?;
                    c.eax = c.esp.wrapping_add(20);
                    push(c, g, c.eax)?;
                    return call_arg(c, g, c.ebx, 0x1d3920, POST[2]);
                }
                c.edi = g.u32(c.esp.wrapping_add(52))?;
                c.ebx = pop(c, g)?;
            }
        }
    }
    if pc == POST[2] {
        c.edi = g.u32(c.esp.wrapping_add(52))?;
        c.ebx = pop(c, g)?;
    }
    if pc == entry || POST[..3].contains(&pc) {
        return g.call(c, 0x108cd0, POST[3]);
    }
    if pc == POST[3] {
        let at = c.esi.wrapping_add(PHYSICS_FLAGS);
        let old = g.u8(at)?;
        c.ecx = byte(c.ecx, old >> 2);
        let mapped = (old & !2) | ((old & 4) >> 1);
        g.set_u8(at, mapped)?;
        g.set_u8(at, mapped & !4)?;
        logic(c, (mapped & !4).into(), 0x80);
        return call_arg(c, g, c.edi, 0xbdef0, POST[4]);
    }
    if pc == POST[4] {
        let flag = g.u8(c.ebp.wrapping_add(2))? & 64;
        logic(c, flag.into(), 0x80);
        if flag != 0 {
            logic(c, c.eax & 0xff, 0x80);
            if c.eax as u8 == 0 {
                for v in [0, 0, 1, 1] { push(c, g, v)?; }
                return call_arg(c, g, c.edi, 0xbef30, POST[5]);
            }
        }
    }
    if POST[4..7].contains(&pc) {
        if pc == POST[6] { sibling(c, g)?; }
        else { c.esi = g.u32(c.esi.wrapping_add(FIRST_CHILD))?; }
        arithmetic(c, c.esi, NONE, true);
        if c.esi != NONE {
            return call_arg(c, g, c.esi, entry, POST[6]);
        }
        c.eax = c.edi;
        return g.call(c, 0xbe1d0, POST[7]);
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 32, false);
    g.ret(c, 4)
}
