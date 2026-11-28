//! The per-object updates and the type dispatchers
//! (docs/engine/object-update.md, "Slice 1: walks and dispatchers"):
//! pre-update `bc470`, post-update `bc820`, and the dispatchers
//! `108c60` (slot `+40`), `108cd0` (`+44`), `109050` (`+6c`) and `1090d0`
//! (`+70`).
//!
//! The two updates are the per-function ports (`sim_physics::object_update`,
//! which stay test oracles) taken into the body table. The current child is
//! looked up again after its callback: the callback may have changed its
//! sibling.
//!
//! A dispatcher walks the descriptor of the object's type (`[468630 +
//! 4 * type]`, the type byte at body `+aa`) and its ancestry list at
//! descriptor `+84` (descriptor pointers, null-terminated), and calls the
//! slot of every level that has one. The slot call is an indirect callout:
//! [`super::drive`] fuses it when a body starts at the slot's value.

use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout;
use guest_layout::points;
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, status};
use runtime::{CpuAbi, Guest};

const COMPONENTS: u32 = 0x0051_e9b8;
const COMPONENT_BYTES: u32 = 0xa0;
const NEXT_SIBLING: u32 = 0xc;
const FIRST_CHILD: u32 = 0x10;
const COMPONENT_HANDLE: u32 = 0xb4;
const PHYSICS_FLAGS: u32 = 0xc0;
const NONE: u32 = u32::MAX;

/// The type table: 13 descriptor pointers.
const TYPES: u32 = 0x0046_8630;
/// A descriptor's ancestry list.
const ANCESTRY: u32 = 0x84;
/// The type byte in the body.
const BODY_KIND: u32 = 0xaa;

pub(super) const PRE: [u32; 8] = [
    0xbc4ca, 0xbc4ed, 0xbc51c, 0xbc524, 0xbc52c, 0xbc534, 0xbc546, 0xbc59a,
];
pub(super) const POST: [u32; 8] = [
    0xbc853, 0xbc867, 0xbc8e1, 0xbc8eb, 0xbc916, 0xbc92e, 0xbc93c, 0xbc961,
];
pub(super) const UPDATE_SITES: [u32; 1] = [0x108cad];
pub(super) const POST_UPDATE_SITES: [u32; 1] = [0x108d13];
pub(super) const POSE_SITES: [u32; 1] = [0x1090a7];
pub(super) const AFTER_ROOT_SITES: [u32; 1] = [0x10911d];

fn call_arg(c: &mut CpuAbi, g: &mut Guest, arg: u32, target: u32, site: u32) -> Result<Next> {
    push(c, g, arg)?;
    Ok(Next::Call { target, site })
}

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

fn pre_ret(c: &mut CpuAbi, g: &Guest, al: u8) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.eax = byte(c.eax, al);
    c.ebx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

/// `bc470 (object: stack 0) -> al`.
pub(super) fn pre(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::OBJECT_PRE_UPDATE.base;
    if pc != entry && !PRE.contains(&pc) {
        return Err(unowned(pc));
    }
    if pc == entry {
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
            return Ok(Next::Call { target: 0xbba20, site: PRE[0] });
        }
    }
    if pc == entry || pc == PRE[0] {
        c.eax = g.u8(g.u32(points::GAME_GLOBALS.base)?.wrapping_add(12))? as i8 as i32 as u32;
        if matches!(c.eax, 4 | 5) && g.u8(c.ebp.wrapping_add(216))? & 1 != 0 {
            logic(c, 1, 0x80);
            c.eax = c.edi;
            return Ok(Next::Call { target: 0xa9570, site: PRE[1] });
        }
    }
    if pc == entry || PRE[..2].contains(&pc) {
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
                return Ok(Next::Call { target: 0x1cd4e0, site: PRE[2] });
            }
        } else {
            arithmetic(c, 0, 0, true);
        }
    }
    if pc == entry || PRE[..3].contains(&pc) {
        if pc == PRE[2] { c.ebx = byte(c.ebx, c.eax as u8); }
        return call_arg(c, g, c.edi, points::OBJECT_TYPE_UPDATE.base, PRE[3]);
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
            return call_arg(c, g, c.esi, entry, PRE[6]);
        }
        c.eax = g.u32(c.ebp.wrapping_add(COMPONENT_HANDLE))?;
        if c.eax != NONE {
            c.ecx = byte(c.ecx, g.u8(c.ebp.wrapping_add(PHYSICS_FLAGS))? >> 6);
            if c.ecx & 1 != 0 {
                c.edx = g.u32(COMPONENTS)?;
                c.edi = (c.eax & 0xffff) * COMPONENT_BYTES;
                c.eax = g.u32(c.edx.wrapping_add(layout::DATA))?;
                c.edi = arithmetic(c, c.edi, c.eax, false);
                return Ok(Next::Call { target: 0x1ced00, site: PRE[7] });
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

/// `bc820 (object: stack 0)`.
pub(super) fn post(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::OBJECT_POST_UPDATE.base;
    if pc != entry && !POST.contains(&pc) {
        return Err(unowned(pc));
    }
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
        if c.eax & 0x80 == 0 { return call_arg(c, g, c.edi, points::OBJECT_NODE_MATRICES.base, POST[1]); }
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
        return Ok(Next::Call { target: points::OBJECT_TYPE_POST_UPDATE.base, site: POST[3] });
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
        return Ok(Next::Call { target: 0xbe1d0, site: POST[7] });
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 32, false);
    Ok(Next::Ret { pop: 4 })
}

/// The object's descriptor: `[468630 + 4 * (i8) body[+aa]]`, through the
/// header of `object` (`(object & 0xffff) * 0xc`, body at `+8`). Leaves
/// `ecx` the sign-extended type, as every dispatcher does.
fn descriptor(c: &mut CpuAbi, g: &Guest, object: u32) -> Result<u32> {
    let array = g.u32(points::OBJECTS.base)?;
    c.ecx = array;
    c.edx = g.u32(array.wrapping_add(layout::DATA))?;
    let body = g.u32(c.edx.wrapping_add((object & 0xffff) * 12 + 8))?;
    c.ecx = g.u8(body.wrapping_add(BODY_KIND))? as i8 as i32 as u32;
    g.u32(TYPES.wrapping_add(c.ecx.wrapping_mul(4)))
}

/// Level `i` of `desc`'s ancestry: `ecx` the entry, `eax` its address,
/// and the flags of `test ecx, ecx`. Null ends the list.
fn level(c: &mut CpuAbi, g: &Guest, desc: u32, i: u32) -> Result<bool> {
    c.eax = desc.wrapping_add(ANCESTRY).wrapping_add(i.wrapping_mul(4));
    c.ecx = g.u32(c.eax)?;
    logic(c, c.ecx, 1 << 31);
    Ok(c.ecx != 0)
}

/// `108c60 (object: stack 0) -> al`: slot `+40` of every level, called
/// `(object)`; `al` the OR of the results. Keeps the object in `ebp`, the
/// descriptor in `edi`, the level in `esi` and the result in `bl`.
pub(super) fn type_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::OBJECT_TYPE_UPDATE.base {
        push(c, g, c.ebx)?;
        push(c, g, c.ebp)?;
        c.ebp = g.u32(c.esp.wrapping_add(12))?;
        c.eax = (c.ebp & 0xffff) * 3;
        let desc = descriptor(c, g, c.ebp)?;
        c.eax = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
        push(c, g, c.esi)?;
        push(c, g, c.edi)?;
        c.edi = desc;
        c.ebx &= !0xff;
        c.esi = 0;
        if !level(c, g, c.edi, 0)? {
            return type_update_ret(c, g);
        }
    } else if pc == UPDATE_SITES[0] {
        // `or bl, al`.
        c.ebx = byte(c.ebx, (c.ebx | c.eax) as u8);
        return type_update_next(c, g);
    } else {
        return Err(unowned(pc));
    }
    type_update_slot(c, g)
}

/// `108ca1`: the slot of the level `eax` points at.
fn type_update_slot(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    loop {
        c.eax = g.u32(c.eax)?;
        c.eax = g.u32(c.eax.wrapping_add(0x40))?;
        logic(c, c.eax, 1 << 31);
        if c.eax != 0 {
            push(c, g, c.ebp)?;
            return Ok(Next::Call { target: c.eax, site: UPDATE_SITES[0] });
        }
        if let Some(next) = type_update_advance(c, g)? {
            return Ok(next);
        }
    }
}

/// `108caf`: the next level, or the return.
fn type_update_next(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    match type_update_advance(c, g)? {
        Some(next) => Ok(next),
        None => type_update_slot(c, g),
    }
}

/// `inc esi; movsx edx, si`, then the level: `None` to visit it, the
/// return when the list ends.
fn type_update_advance(c: &mut CpuAbi, g: &mut Guest) -> Result<Option<Next>> {
    c.esi = c.esi.wrapping_add(1);
    c.edx = c.esi as u16 as i16 as i32 as u32;
    if level(c, g, c.edi, c.edx)? {
        return Ok(None);
    }
    type_update_ret(c, g).map(Some)
}

fn type_update_ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.eax = byte(c.eax, c.ebx as u8);
    c.ebx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

/// `108cd0 (object: edi)`: slot `+44` of every level, called `(object)`.
/// Keeps the address of the current level in `esi` (saved only when the
/// list is not empty); returns with `eax` that address.
pub(super) fn type_post_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::OBJECT_TYPE_POST_UPDATE.base {
        c.eax = (c.edi & 0xffff) * 3;
        let desc = descriptor(c, g, c.edi)?;
        c.eax = desc;
        c.ecx = g.u32(desc.wrapping_add(ANCESTRY))?;
        c.eax = arithmetic(c, desc, ANCESTRY, false);
        logic(c, c.ecx, 1 << 31);
        if c.ecx == 0 {
            return Ok(Next::Ret { pop: 0 });
        }
        push(c, g, c.esi)?;
        c.esi = c.eax;
    } else if pc == POST_UPDATE_SITES[0] {
        // `108d13`: the next level.
        c.ecx = g.u32(c.esi.wrapping_add(4))?;
        c.esi = arithmetic(c, c.esi, 4, false);
        logic(c, c.ecx, 1 << 31);
        c.eax = c.esi;
        if c.ecx == 0 {
            c.esi = pop(c, g)?;
            return Ok(Next::Ret { pop: 0 });
        }
    } else {
        return Err(unowned(pc));
    }
    // `108d07`: the slot of the level `eax` points at.
    loop {
        c.eax = g.u32(c.eax)?;
        c.eax = g.u32(c.eax.wrapping_add(0x44))?;
        logic(c, c.eax, 1 << 31);
        if c.eax != 0 {
            push(c, g, c.edi)?;
            return Ok(Next::Call { target: c.eax, site: POST_UPDATE_SITES[0] });
        }
        c.ecx = g.u32(c.esi.wrapping_add(4))?;
        c.esi = arithmetic(c, c.esi, 4, false);
        logic(c, c.ecx, 1 << 31);
        c.eax = c.esi;
        if c.ecx == 0 {
            c.esi = pop(c, g)?;
            return Ok(Next::Ret { pop: 0 });
        }
    }
}

/// `109050 (object: ebx, s0, s1, s2)`: slot `+6c` of every level, called
/// `(object, s0, s1, s2)`; `ret 0c`. Keeps the descriptor in `esi`, the
/// level in `edi` and `s2` in `ebp`; `s0` and `s1` are read from the frame
/// before each call.
pub(super) fn type_pose(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::OBJECT_TYPE_POSE.base {
        c.eax = (c.ebx & 0xffff) * 3;
        let desc = descriptor(c, g, c.ebx)?;
        c.eax = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
        push(c, g, c.ebp)?;
        c.ebp = g.u32(c.esp.wrapping_add(0x10))?;
        push(c, g, c.esi)?;
        c.esi = desc;
        level(c, g, c.esi, 0)?;
        push(c, g, c.edi)?;
        c.edi = 0;
        logic(c, c.ecx, 1 << 31);
        if c.ecx == 0 {
            return dispatch_ret(c, g, 0x0c);
        }
    } else if pc == POSE_SITES[0] {
        if !dispatch_advance(c, g, c.esi, Reg::Edi)? {
            return dispatch_ret(c, g, 0x0c);
        }
    } else {
        return Err(unowned(pc));
    }
    loop {
        c.eax = g.u32(c.eax)?;
        c.eax = g.u32(c.eax.wrapping_add(0x6c))?;
        logic(c, c.eax, 1 << 31);
        if c.eax != 0 {
            c.edx = g.u32(c.esp.wrapping_add(0x14))?;
            c.ecx = g.u32(c.esp.wrapping_add(0x10))?;
            push(c, g, c.ebp)?;
            push(c, g, c.edx)?;
            push(c, g, c.ecx)?;
            push(c, g, c.ebx)?;
            return Ok(Next::Call { target: c.eax, site: POSE_SITES[0] });
        }
        if !dispatch_advance(c, g, c.esi, Reg::Edi)? {
            return dispatch_ret(c, g, 0x0c);
        }
    }
}

/// `1090d0 (object: ebx, matrix: stack 0)`: slot `+70` of every level,
/// called `(object, matrix)`; `ret 4`. Keeps the descriptor in `edi`, the
/// level in `esi` and the matrix in `ebp`.
pub(super) fn type_after_root(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::OBJECT_TYPE_AFTER_ROOT.base {
        c.eax = (c.ebx & 0xffff) * 3;
        let desc = descriptor(c, g, c.ebx)?;
        c.eax = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
        push(c, g, c.ebp)?;
        c.ebp = g.u32(c.esp.wrapping_add(8))?;
        push(c, g, c.esi)?;
        push(c, g, c.edi)?;
        c.edi = desc;
        level(c, g, c.edi, 0)?;
        c.esi = 0;
        logic(c, c.ecx, 1 << 31);
        if c.ecx == 0 {
            return dispatch_ret(c, g, 4);
        }
    } else if pc == AFTER_ROOT_SITES[0] {
        if !dispatch_advance(c, g, c.edi, Reg::Esi)? {
            return dispatch_ret(c, g, 4);
        }
    } else {
        return Err(unowned(pc));
    }
    loop {
        c.eax = g.u32(c.eax)?;
        c.eax = g.u32(c.eax.wrapping_add(0x70))?;
        logic(c, c.eax, 1 << 31);
        if c.eax != 0 {
            push(c, g, c.ebp)?;
            push(c, g, c.ebx)?;
            return Ok(Next::Call { target: c.eax, site: AFTER_ROOT_SITES[0] });
        }
        if !dispatch_advance(c, g, c.edi, Reg::Esi)? {
            return dispatch_ret(c, g, 4);
        }
    }
}

/// Which register a dispatcher counts levels in.
#[derive(Clone, Copy)]
enum Reg {
    Esi,
    Edi,
}

/// `inc r; movsx edx, r16`, then the level (`ecx`, `eax`, flags). False
/// when the list ends.
fn dispatch_advance(c: &mut CpuAbi, g: &Guest, desc: u32, counter: Reg) -> Result<bool> {
    let r = match counter {
        Reg::Esi => &mut c.esi,
        Reg::Edi => &mut c.edi,
    };
    *r = r.wrapping_add(1);
    let n = *r;
    c.edx = n as u16 as i16 as i32 as u32;
    level(c, g, desc, c.edx)
}

/// `pop edi; pop esi; pop ebp; ret pop`.
fn dispatch_ret(c: &mut CpuAbi, g: &Guest, pop_bytes: u16) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    Ok(Next::Ret { pop: pop_bytes })
}
