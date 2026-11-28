//! The walks' callouts in Rust (docs/engine/object-update.md, "Walk
//! callouts"): the replicated transform `a9570` the pre-update calls, the
//! type new dispatch `108a90`, the deferred weapon fire `fd910`, the
//! delete `b8540` and the two tree passes finalize runs per deleted object
//! (`b83b0`, `b8460`), the collector `bf380` finalize tail-jumps to, and
//! the spatial visit `bb050`.
//!
//! Each is a segment body over the retail frame: at every guest call the
//! registers, flags and frame slots are retail's. Calls between these
//! bodies (`b83b0` to `b8540` and to itself, `b8460` to itself) fuse in
//! [`super::drive`]. Every other callee is a guest call: the subsystems a
//! deletion notifies, the eight delete callbacks at `468664`, the
//! collector's table at `440578`, the descriptors' slot `+2c`.

use super::alu::{cmp16, rmw8, test8};
use super::{Cx, Next, Result, unowned};
use crate::datum::layout::DATA;
use crate::points;
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, status};
use runtime::{CpuAbi, Guest, abi};

const NONE: u32 = u32::MAX;
const SIGN: u32 = 1 << 31;
/// The type descriptors, by object type.
const DESCRIPTORS: u32 = 0x0046_8630;
/// The eight delete callbacks `b83b0` calls, each `(object)`, `ret 4`.
const DELETE_CALLBACKS: u32 = 0x0046_8664;
/// The collector's passes: `{ types, gated, pre, main }`, 16 bytes each,
/// ended by a zero `main`; `edi` points at `pre`.
const COLLECT_TABLE: u32 = 0x0044_0578;
const PHASE_STATE: u32 = 0x004d_e2f4;
const HEAP_STATE: u32 = 0x004d_e2ec;
const STACK_PROBE: u32 = 0x0032_0560;
const COLLECT_FRAME: u32 = 0x2a04;

/// `[[OBJECTS]+44]`: `ecx` the array, `edx` its data, as every one of
/// these bodies loads them first.
fn objects(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = g.u32(c.ecx.wrapping_add(DATA))?;
    Ok(())
}

/// `cmp a8, b8`.
fn cmp8(c: &mut CpuAbi, a: u32, b: u32) {
    let (a, b) = (a & 0xff, b & 0xff);
    let v = a.wrapping_sub(b) & 0xff;
    let of = u32::from((a ^ b) & (a ^ v) & 0x80 != 0) << 11;
    c.eflags = (c.eflags & !ARITH) | status(v, 0x80) | u32::from(a < b) | ((a ^ b ^ v) & 0x10) | of;
    c.eflags_defined |= ARITH;
}

/// `inc r32`: CF kept.
fn inc32(c: &mut CpuAbi, v: u32) -> u32 {
    let r = v.wrapping_add(1);
    let af = u32::from(r & 15 == 0) << 4;
    let of = u32::from(r == 0x8000_0000) << 11;
    let keep = ARITH & !1;
    c.eflags = (c.eflags & !keep) | status(r, SIGN) | af | of;
    c.eflags_defined |= keep;
    r
}

/// `test r8, r8` on the low byte of `v`.
fn test_low(c: &mut CpuAbi, v: u32) -> bool {
    logic(c, v & 0xff, 0x80);
    v & 0xff != 0
}

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

// `a9570`: the replicated transform.

/// After `a9640`, `a9820`, `a9a70`, `a9b40` and `a99f0`.
pub(super) const REPLICATED_SITES: [u32; 5] = [0xa95a4, 0xa95e7, 0xa95f9, 0xa960e, 0xa9620];

/// `a9570 (object: eax)`; `ret`. Nothing when body `+d8` bit 0 is clear.
/// An attached object (`+14` not none) goes through `a9640`. Else a zeroed
/// 40-byte record at `[esp+8]` (after `esi`, `edi`) is filled by
/// `a9820(record)` with `ecx` the object; record bit 0 runs `a9a70`
/// (`eax` object, `edx` record), bit 1 `a9b40(record)`, and either runs
/// `a99f0(record)`; a zero first word after it, or neither bit, clears
/// `+d8` bit 0.
pub(super) fn replicated_transform(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        _ if pc == points::OBJECT_REPLICATED_TRANSFORM.base => {
            objects(c, g)?;
            c.esp = arithmetic(c, c.esp, 0x28, true);
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = c.eax;
            c.eax &= 0xffff;
            logic(c, c.eax, SIGN);
            c.eax *= 3;
            c.esi = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
            if !test8(c, g, c.esi.wrapping_add(0xd8), 1)? {
                return replicated_end(c, g);
            }
            let parent = g.u32(c.esi.wrapping_add(0x14))?;
            arithmetic(c, parent, NONE, true);
            if parent != NONE {
                return call(0xa9640, REPLICATED_SITES[0]);
            }
            c.eax = 0;
            logic(c, 0, SIGN);
            for off in [0x0c, 0x10, 0x14, 0x18, 0x1c, 0x20] {
                g.set_u32(c.esp.wrapping_add(off), 0)?;
            }
            push(c, g, c.ebx)?;
            g.set_u32(c.esp.wrapping_add(0x28), 0)?;
            c.ecx = c.esp.wrapping_add(0x0c);
            g.set_u32(c.esp.wrapping_add(0x2c), 0)?;
            push(c, g, c.ecx)?;
            c.ecx = c.edi;
            c.ebx = byte(c.ebx, 0);
            logic(c, 0, 0x80);
            g.set_u32(c.esp.wrapping_add(0x10), 0)?;
            g.set_u32(c.esp.wrapping_add(0x34), c.eax)?;
            call(0xa9820, REPLICATED_SITES[1])
        }
        0xa95a4 => replicated_end(c, g),
        0xa95e7 => {
            if test8(c, g, c.esp.wrapping_add(0x0c), 1)? {
                c.edx = c.esp.wrapping_add(0x0c);
                c.eax = c.edi;
                return call(0xa9a70, REPLICATED_SITES[2]);
            }
            replicated_second(c, g)
        }
        0xa95f9 => {
            c.ebx = byte(c.ebx, 1);
            replicated_second(c, g)
        }
        0xa960e => replicated_apply(c, g),
        0xa9620 => {
            c.eax = g.u32(c.esp.wrapping_add(0x0c))?;
            logic(c, c.eax, SIGN);
            if c.eax == 0 {
                rmw8(c, g, c.esi.wrapping_add(0xd8), |v| v & 0xfe)?;
            }
            c.ebx = pop(c, g)?;
            replicated_end(c, g)
        }
        _ => Err(unowned(pc)),
    }
}

/// `a95fb`: record bit 1, else the apply when bit 0 ran.
fn replicated_second(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if test8(c, g, c.esp.wrapping_add(0x0c), 2)? {
        c.edx = c.esp.wrapping_add(0x0c);
        push(c, g, c.edx)?;
        c.eax = c.edi;
        return call(0xa9b40, REPLICATED_SITES[3]);
    }
    if test_low(c, c.ebx) {
        return replicated_apply(c, g);
    }
    rmw8(c, g, c.esi.wrapping_add(0xd8), |v| v & 0xfe)?;
    c.ebx = pop(c, g)?;
    replicated_end(c, g)
}

/// `a9614`.
fn replicated_apply(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = c.esp.wrapping_add(0x0c);
    push(c, g, c.eax)?;
    c.eax = c.edi;
    call(0xa99f0, REPLICATED_SITES[4])
}

/// `a9630`.
fn replicated_end(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x28, false);
    Ok(Next::Ret { pop: 0 })
}

// `108a90`: the type new dispatch.

/// After a slot `+2c` call.
pub(super) const TYPE_NEW_SITES: [u32; 1] = [0x108ae6];

/// `108a90 (object: ebx, a: stack 0, b: stack 1) -> al`; `ret 8`. Walks
/// the object's descriptor's ancestry list at `+84` (`edi` the index,
/// `ecx` the entry's address); each type's slot `+2c`, while the result
/// is still 1, is called `(object, a, b)` and its `al` kept. `al` 1 when
/// the list is empty.
pub(super) fn type_new(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        _ if pc == points::OBJECT_TYPE_NEW.base => {
            objects(c, g)?;
            c.eax = c.ebx & 0xffff;
            logic(c, c.eax, SIGN);
            c.eax *= 3;
            c.eax = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
            c.ecx = g.u8(c.eax.wrapping_add(0xaa))? as i8 as i32 as u32;
            push(c, g, c.ebp)?;
            c.ebp = g.u32(c.esp.wrapping_add(0x0c))?;
            push(c, g, c.esi)?;
            c.esi = g.u32(c.ecx.wrapping_mul(4).wrapping_add(DESCRIPTORS))?;
            c.edx = g.u32(c.esi.wrapping_add(0x84))?;
            c.ecx = c.esi.wrapping_add(0x84);
            push(c, g, c.edi)?;
            c.edi = 0;
            logic(c, c.edx, SIGN);
            c.eax = byte(c.eax, 1);
            if c.edx == 0 {
                return type_new_end(c, g);
            }
            type_new_walk(c, g)
        }
        0x108ae6 => {
            if test_low(c, c.eax) {
                c.eax = byte(c.eax, 1);
            } else {
                c.eax = byte(c.eax, 0);
                logic(c, 0, 0x80);
            }
            if !type_new_advance(c, g)? {
                return type_new_end(c, g);
            }
            type_new_walk(c, g)
        }
        _ => Err(unowned(pc)),
    }
}

/// `108ad0`, from the entry at `[ecx]`.
fn type_new_walk(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    loop {
        c.ecx = g.u32(c.ecx)?;
        c.ecx = g.u32(c.ecx.wrapping_add(0x2c))?;
        logic(c, c.ecx, SIGN);
        if c.ecx != 0 {
            if test_low(c, c.eax) {
                c.edx = g.u32(c.esp.wrapping_add(0x10))?;
                push(c, g, c.ebp)?;
                push(c, g, c.edx)?;
                push(c, g, c.ebx)?;
                return call(c.ecx, TYPE_NEW_SITES[0]);
            }
            c.eax = byte(c.eax, 0);
            logic(c, 0, 0x80);
        }
        if !type_new_advance(c, g)? {
            return type_new_end(c, g);
        }
    }
}

/// `108af0`: the next entry; false at the list's end.
fn type_new_advance(c: &mut CpuAbi, g: &Guest) -> Result<bool> {
    c.edi = inc32(c, c.edi);
    c.ecx = c.edi as u16 as i16 as i32 as u32;
    let at = c.esi.wrapping_add(c.ecx.wrapping_mul(4)).wrapping_add(0x84);
    c.edx = g.u32(at)?;
    logic(c, c.edx, SIGN);
    c.ecx = at;
    Ok(c.edx != 0)
}

/// `108b06`.
fn type_new_end(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    Ok(Next::Ret { pop: 8 })
}

// `fd910`: the deferred weapon fire.

/// After the first iterator step, `fd980`, and the next step.
pub(super) const DEFERRED_FIRE_SITES: [u32; 3] = [0xfd941, 0xfd963, 0xfd96d];

/// `fd910`; `ret`. An object iterator on its frame (`[esp+4]`: types 4,
/// byte 2, word 0, the current handle none, `86868686`) stepped by
/// `baeb0(iterator)`; each weapon whose body `+16c` bit 0 is set goes to
/// `fd980(handle)`. `[esp]` holds the current body.
pub(super) fn deferred_fire(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        _ if pc == points::WEAPONS_DEFERRED_FIRE.base => {
            c.esp = arithmetic(c, c.esp, 0x14, true);
            c.eax = c.esp.wrapping_add(4);
            push(c, g, c.eax)?;
            g.set_u32(c.esp.wrapping_add(0x14), 0x8686_8686)?;
            g.set_u32(c.esp.wrapping_add(8), 4)?;
            g.set_u8(c.esp.wrapping_add(0x0c), 2)?;
            g.set_u16(c.esp.wrapping_add(0x0e), 0)?;
            g.set_u32(c.esp.wrapping_add(0x10), NONE)?;
            call(0xbaeb0, DEFERRED_FIRE_SITES[0])
        }
        0xfd941 | 0xfd96d => {
            logic(c, c.eax, SIGN);
            g.set_u32(c.esp, c.eax)?;
            if c.eax == 0 {
                c.esp = arithmetic(c, c.esp, 0x14, false);
                return Ok(Next::Ret { pop: 0 });
            }
            if test8(c, g, c.eax.wrapping_add(0x16c), 1)? {
                c.ecx = g.u32(c.esp.wrapping_add(0x0c))?;
                push(c, g, c.ecx)?;
                return call(0xfd980, DEFERRED_FIRE_SITES[1]);
            }
            deferred_step(c, g)
        }
        0xfd963 => deferred_step(c, g),
        _ => Err(unowned(pc)),
    }
}

/// `fd963`.
fn deferred_step(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = c.esp.wrapping_add(4);
    push(c, g, c.edx)?;
    call(0xbaeb0, DEFERRED_FIRE_SITES[2])
}

// `b8540`: the delete.

/// After `b9ce0`, `b9d20`, `bef30` (and the branches that skip it),
/// `b8b70`, `10a250`, `bf090`, `15b220`, `109400`, `bb950`, `a7a60` and
/// `10ace0`.
pub(super) const DELETE_SITES: [u32; 11] =
    [0xb8568, 0xb8573, 0xb8585, 0xb858e, 0xb85a2, 0xb85a9, 0xb85d8, 0xb85de, 0xb85e9, 0xb85f0, 0xb85f7];

/// `b8540 (object: stack 0)`; `ret 4`. `esi` the handle, `ebp` the body,
/// `edi` the header. Unless `b9ce0` (`ecx` handle) answers yes, a yes
/// from `b9d20` (`eax` handle) reports `bef30(handle, 1, 0, 0, 0)`. Then
/// DELETE on the header, `b8b70`, `10a250(handle)` for type 6, `bf090`
/// (`edi` handle), body `+a4`, `+a8` and `+ab` none, `15b220(+af)` (`eax`
/// handle) when `+af` is not none, `109400(handle)`,
/// `bb950(0, none)` (`eax` handle), `a7a60` (`eax`) and `10ace0` (`edx`).
pub(super) fn delete(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        _ if pc == points::OBJECT_DELETE.base => {
            objects(c, g)?;
            push(c, g, c.ebp)?;
            push(c, g, c.esi)?;
            c.esi = g.u32(c.esp.wrapping_add(0x0c))?;
            c.eax = c.esi & 0xffff;
            logic(c, c.eax, SIGN);
            c.eax *= 3;
            c.ebp = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
            push(c, g, c.edi)?;
            c.edi = c.edx.wrapping_add(c.eax * 4);
            c.ecx = c.esi;
            call(0xb9ce0, DELETE_SITES[0])
        }
        0xb8568 => {
            if test_low(c, c.eax) {
                return delete_mark(c, g);
            }
            c.eax = c.esi;
            call(0xb9d20, DELETE_SITES[1])
        }
        0xb8573 => {
            if !test_low(c, c.eax) {
                return delete_mark(c, g);
            }
            for arg in [0, 0, 0, 1, c.esi] {
                push(c, g, arg)?;
            }
            call(0xbef30, DELETE_SITES[2])
        }
        0xb8585 => delete_mark(c, g),
        0xb858e => {
            let kind = g.u8(c.edi.wrapping_add(3))?;
            c.ecx = byte(c.ecx, kind);
            c.eax = 1u32 << (kind & 31);
            if test_low(c, c.eax & 0x40) {
                push(c, g, c.esi)?;
                return call(0x10a250, DELETE_SITES[4]);
            }
            delete_body(c)
        }
        0xb85a2 => delete_body(c),
        0xb85a9 => {
            g.set_u8(c.ebp.wrapping_add(0xab), 0xff)?;
            g.set_u16(c.ebp.wrapping_add(0xa8), 0xffff)?;
            g.set_u32(c.ebp.wrapping_add(0xa4), NONE)?;
            let al = g.u8(c.ebp.wrapping_add(0xaf))?;
            c.eax = byte(c.eax, al);
            cmp8(c, al.into(), 0xff);
            if al != 0xff {
                c.ecx = al as i8 as i32 as u32;
                push(c, g, c.ecx)?;
                c.eax = c.esi;
                return call(0x15b220, DELETE_SITES[6]);
            }
            delete_unlink(c, g)
        }
        0xb85d8 => delete_unlink(c, g),
        0xb85de => {
            push(c, g, NONE)?;
            push(c, g, 0)?;
            c.eax = c.esi;
            call(0xbb950, DELETE_SITES[8])
        }
        0xb85e9 => {
            c.eax = c.esi;
            call(0xa7a60, DELETE_SITES[9])
        }
        0xb85f0 => {
            c.edx = c.esi;
            call(0x10ace0, DELETE_SITES[10])
        }
        0xb85f7 => {
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            c.ebp = pop(c, g)?;
            Ok(Next::Ret { pop: 4 })
        }
        _ => Err(unowned(pc)),
    }
}

/// `b8585`: DELETE on the header, then `b8b70`.
fn delete_mark(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    rmw8(c, g, c.edi.wrapping_add(2), |v| v | 0x10)?;
    call(0xb8b70, DELETE_SITES[3])
}

/// `b85a2`.
fn delete_body(c: &mut CpuAbi) -> Result<Next> {
    c.edi = c.esi;
    call(0xbf090, DELETE_SITES[5])
}

/// `b85d8`.
fn delete_unlink(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, c.esi)?;
    call(0x109400, DELETE_SITES[7])
}

// `b83b0`: the delete tree.

/// After `b8540`, each delete callback, `cc590`, `b9a90` and `b83b0`.
pub(super) const DELETE_TREE_SITES: [u32; 5] = [0xb83d4, 0xb83dd, 0xb8440, 0xb8449, 0xb8453];

/// `b83b0 (object: stack 0, top: stack 1)`; `ret 8`. `edi` the handle,
/// `ebx` the body. `b8540(handle)`, the eight callbacks at `468664`
/// (`esi` the offset), then each child from body `+10` along `+0c`
/// (`eax` the child, `esi` the next): in network mode 4 (`[game
/// globals]+0c`) a child with an owner at `+d4` is kept, through `cc590`
/// for a unit or biped (type bits 3) and `b9a90` (`ebx` the child) for
/// the rest when it is attached (`+14`); every other child is deleted by
/// `b83b0(child, 0)`.
pub(super) fn delete_tree(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        _ if pc == points::OBJECT_DELETE_TREE.base => {
            objects(c, g)?;
            push(c, g, c.ebx)?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = g.u32(c.esp.wrapping_add(0x10))?;
            c.eax = c.edi & 0xffff;
            logic(c, c.eax, SIGN);
            c.eax *= 3;
            c.ebx = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
            push(c, g, c.edi)?;
            call(points::OBJECT_DELETE.base, DELETE_TREE_SITES[0])
        }
        0xb83d4 => {
            c.esi = 0;
            logic(c, 0, SIGN);
            delete_callback(c, g)
        }
        0xb83dd => {
            c.esi = arithmetic(c, c.esi, 4, false);
            arithmetic(c, c.esi, 0x20, true);
            if c.esi < 0x20 {
                return delete_callback(c, g);
            }
            c.eax = g.u32(c.ebx.wrapping_add(0x10))?;
            c.edi = NONE;
            logic(c, NONE, SIGN);
            arithmetic(c, c.eax, c.edi, true);
            if c.eax == c.edi {
                return delete_tree_end(c, g);
            }
            delete_child(c, g)
        }
        0xb8440 | 0xb8449 | 0xb8453 => delete_next_child(c, g),
        _ => Err(unowned(pc)),
    }
}

/// `b83d6`: callback `esi / 4`.
fn delete_callback(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, c.edi)?;
    let target = g.u32(c.esi.wrapping_add(DELETE_CALLBACKS))?;
    call(target, DELETE_TREE_SITES[1])
}

/// `b83f0`: the child in `eax`.
fn delete_child(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    loop {
        c.edx = g.u32(points::OBJECTS.base)?;
        c.edx = g.u32(c.edx.wrapping_add(DATA))?;
        c.ecx = c.eax & 0xffff;
        logic(c, c.ecx, SIGN);
        c.ecx *= 3;
        c.ecx = g.u32(c.edx.wrapping_add(c.ecx * 4 + 8))?;
        c.edx = g.u32(points::GAME_GLOBALS.base)?;
        let mode = g.u8(c.edx.wrapping_add(0x0c))?;
        cmp8(c, mode.into(), 4);
        c.esi = g.u32(c.ecx.wrapping_add(0x0c))?;
        if mode == 4 {
            let owner = g.u32(c.ecx.wrapping_add(0xd4))?;
            arithmetic(c, owner, c.edi, true);
            c.edx = byte(c.edx, u8::from(owner != c.edi));
            if test_low(c, c.edx) {
                let parent = g.u32(c.ecx.wrapping_add(0x14))?;
                arithmetic(c, parent, c.edi, true);
                if parent != c.edi {
                    let kind = g.u8(c.ecx.wrapping_add(0xaa))?;
                    c.ecx = byte(c.ecx, kind);
                    c.edx = 1u32 << (kind & 31);
                    if test_low(c, c.edx & 3) {
                        return call(0xcc590, DELETE_TREE_SITES[2]);
                    }
                    c.ebx = c.eax;
                    return call(0xb9a90, DELETE_TREE_SITES[3]);
                }
                if !delete_advance(c) {
                    return delete_tree_end(c, g);
                }
                continue;
            }
        }
        push(c, g, 0)?;
        push(c, g, c.eax)?;
        return call(points::OBJECT_DELETE_TREE.base, DELETE_TREE_SITES[4]);
    }
}

/// `b8453`, after a child.
fn delete_next_child(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if !delete_advance(c) {
        return delete_tree_end(c, g);
    }
    delete_child(c, g)
}

/// `cmp esi, edi; mov eax, esi`: false at the list's end.
fn delete_advance(c: &mut CpuAbi) -> bool {
    arithmetic(c, c.esi, c.edi, true);
    c.eax = c.esi;
    c.esi != c.edi
}

/// `b8459`.
fn delete_tree_end(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    Ok(Next::Ret { pop: 8 })
}

// `b8460`: the free tree.

/// After `b9890`, `b7300`, `b87b0`, `d4a60`, `bee60`, `b8460`, the first
/// `146bf0`, `1c37f0`, `278f00`, the second `146bf0`, `108bf0` and
/// `bc300`.
pub(super) const FREE_TREE_SITES: [u32; 12] =
    [0xb8496, 0xb849d, 0xb84ae, 0xb84b5, 0xb84bc, 0xb84e6, 0xb850d, 0xb8514, 0xb851d, 0xb8522, 0xb8529, 0xb852f];

/// `b8460 (object: stack 0, top: stack 1)`; `ret 8`. `ebp` the handle,
/// `edi` the header offset, `esi` the body. With `top`: `b9890` (`eax`)
/// when attached (`+14`), `b7300` (`edx`), and `b87b0(handle)` when body
/// `+4` bit 8 is set. Then `d4a60` (`eax`), `bee60` (`ebx`), each child
/// from `+10` along `+0c` by `b8460(child, 0)`, `146bf0` around `1c37f0`
/// (`eax`) and `278f00` when the body's `+c0` bit 6 is set, `108bf0`
/// (`ebx`) and `bc300(handle)`.
pub(super) fn free_tree(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        _ if pc == points::OBJECT_FREE_TREE.base => {
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = g.u32(c.esp.wrapping_add(0x0c))?;
            push(c, g, c.esi)?;
            c.eax = c.ebp & 0xffff;
            logic(c, c.eax, SIGN);
            push(c, g, c.edi)?;
            c.edi = c.eax * 3;
            c.eax = g.u32(points::OBJECTS.base)?;
            c.ecx = g.u32(c.eax.wrapping_add(DATA))?;
            let top = g.u8(c.esp.wrapping_add(0x18))?;
            c.eax = byte(c.eax, top);
            c.edi <<= 2;
            c.esi = g.u32(c.ecx.wrapping_add(c.edi).wrapping_add(8))?;
            if !test_low(c, c.eax) {
                return free_subsystems(c);
            }
            let parent = g.u32(c.esi.wrapping_add(0x14))?;
            arithmetic(c, parent, NONE, true);
            if parent != NONE {
                c.eax = c.ebp;
                return call(0xb9890, FREE_TREE_SITES[0]);
            }
            free_unlink(c)
        }
        0xb8496 => free_unlink(c),
        0xb849d => {
            c.edx = g.u32(c.esi.wrapping_add(4))? >> 8;
            if test_low(c, c.edx & 1) {
                push(c, g, c.ebp)?;
                return call(0xb87b0, FREE_TREE_SITES[2]);
            }
            free_subsystems(c)
        }
        0xb84ae => free_subsystems(c),
        0xb84b5 => {
            c.ebx = c.ebp;
            call(0xbee60, FREE_TREE_SITES[4])
        }
        0xb84bc => {
            c.ecx = g.u32(c.esi.wrapping_add(0x10))?;
            arithmetic(c, c.ecx, NONE, true);
            if c.ecx == NONE {
                return free_globals(c, g);
            }
            free_child(c, g)
        }
        0xb84e6 => {
            arithmetic(c, c.esi, NONE, true);
            c.ecx = c.esi;
            if c.esi == NONE {
                return free_globals(c, g);
            }
            free_child(c, g)
        }
        0xb850d => free_render(c),
        0xb8514 => {
            if test_low(c, c.ebx) {
                return call(0x278f00, FREE_TREE_SITES[8]);
            }
            free_datum(c)
        }
        0xb851d => call(0x146bf0, FREE_TREE_SITES[9]),
        0xb8522 => free_datum(c),
        0xb8529 => {
            push(c, g, c.ebp)?;
            call(0xbc300, FREE_TREE_SITES[11])
        }
        0xb852f => {
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            c.ebp = pop(c, g)?;
            c.ebx = pop(c, g)?;
            Ok(Next::Ret { pop: 8 })
        }
        _ => Err(unowned(pc)),
    }
}

/// `b8496`.
fn free_unlink(c: &mut CpuAbi) -> Result<Next> {
    c.edx = c.ebp;
    call(0xb7300, FREE_TREE_SITES[1])
}

/// `b84ae`.
fn free_subsystems(c: &mut CpuAbi) -> Result<Next> {
    c.eax = c.ebp;
    call(0xd4a60, FREE_TREE_SITES[3])
}

/// `b84c4`: the child in `ecx`.
fn free_child(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = g.u32(points::OBJECTS.base)?;
    c.edx = g.u32(c.edx.wrapping_add(DATA))?;
    c.eax = c.ecx & 0xffff;
    logic(c, c.eax, SIGN);
    c.eax *= 3;
    c.eax = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
    c.esi = g.u32(c.eax.wrapping_add(0x0c))?;
    push(c, g, 0)?;
    push(c, g, c.ecx)?;
    call(points::OBJECT_FREE_TREE.base, FREE_TREE_SITES[5])
}

/// `b84ed`: the body re-read through the header offset, its `+c0` bit 6
/// into `bl`.
fn free_globals(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = g.u32(c.ecx.wrapping_add(DATA))?;
    c.eax = g.u32(c.edx.wrapping_add(c.edi).wrapping_add(8))?;
    let bit = (g.u8(c.eax.wrapping_add(0xc0))? >> 6) & 1;
    c.ebx = byte(c.ebx, bit);
    logic(c, bit.into(), 0x80);
    if bit != 0 {
        return call(0x146bf0, FREE_TREE_SITES[6]);
    }
    free_render(c)
}

/// `b850d`.
fn free_render(c: &mut CpuAbi) -> Result<Next> {
    c.eax = c.ebp;
    call(0x1c37f0, FREE_TREE_SITES[7])
}

/// `b8522`.
fn free_datum(c: &mut CpuAbi) -> Result<Next> {
    c.ebx = c.ebp;
    call(0x108bf0, FREE_TREE_SITES[10])
}

// `bf380`: the collector.

/// After the stack probe, `bf1c0`, a pass's `pre` and its `main`.
pub(super) const COLLECT_SITES: [u32; 4] = [0xbf38a, 0xbf449, 0xbf48a, 0xbf4b1];

/// `bf380`; `ret`. Frame `2a04` from the probe; after its pushes (`ebx`,
/// `ebp`, `esi`, `edi`) the done byte is `[esp+13]`, the scratch at
/// `[esp+214]` (`2800` bytes) and the handle buffer at `[esp+14]` (`200`
/// entries). `ebp` is the mode: from phase state `+1`/`+2` when a
/// collection was asked (`+2` clear gives 1, else 0), else 2 when memory
/// is short (heap `[4de2ec]` free space above `cccc`, fewer than `766`
/// objects, and `+4` at least `32`) or 3. Each pass of the table at
/// `440578` (`edi`) runs while `bf1c0` (`eax` mode, `esi` the done byte)
/// says yes: a pass whose type bits hold the mode (and, gated, only once
/// done) calls `pre(mode, scratch, 2800)` once and `main(mode, scratch,
/// 2800, &done, buffer, 200)` until done.
pub(super) fn collect(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        _ if pc == points::OBJECTS_COLLECT.base => {
            c.eax = COLLECT_FRAME;
            call(STACK_PROBE, COLLECT_SITES[0])
        }
        0xbf38a => {
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            push(c, g, c.esi)?;
            c.esi = g.u32(PHASE_STATE)?;
            let asked = g.u8(c.esi.wrapping_add(1))?;
            c.eax = byte(c.eax, asked);
            if test_low(c, c.eax) {
                let full = g.u8(c.esi.wrapping_add(2))?;
                c.ecx = byte(c.ecx, full);
                c.eax = 0;
                test_low(c, c.ecx);
                c.eax = u32::from(full == 0);
                c.ebp = c.eax;
                arithmetic(c, c.ebp, NONE, true);
                return collect_start(c, g, None);
            }
            c.edx = g.u32(HEAP_STATE)?;
            c.eax = g.u32(c.edx.wrapping_add(0x38))?;
            logic(c, c.eax, SIGN);
            if c.eax != 0 {
                c.ecx = c.eax;
                c.eax = g.u32(c.ecx)?;
                let used = g.u32(c.edx.wrapping_add(0x28))?;
                c.eax = arithmetic(c, c.eax, used, true);
                c.eax = arithmetic(c, c.eax, c.ecx, false);
            } else {
                c.eax = 0;
                logic(c, 0, SIGN);
            }
            c.ecx = g.u32(c.edx.wrapping_add(0x2c))?;
            c.ecx = arithmetic(c, c.ecx, c.eax, true);
            arithmetic(c, c.ecx, 0xcccc, true);
            if c.ecx as i32 <= 0xcccc {
                return collect_start(c, g, Some(3));
            }
            c.eax = g.u32(points::OBJECTS.base)?;
            c.ecx = g.u32(c.eax.wrapping_add(0x3c))?;
            c.edx = arithmetic(c, 0x800, c.ecx, true);
            arithmetic(c, c.edx, 0x66, true);
            if c.edx as i32 <= 0x66 {
                return collect_start(c, g, Some(3));
            }
            let count = g.u16(c.esi.wrapping_add(4))?;
            cmp16(c, count.into(), 0x32);
            if (count as i16) < 0x32 {
                return collect_end(c, g, c.esi);
            }
            collect_start(c, g, Some(2))
        }
        0xbf449 => {
            if !test_low(c, c.eax) {
                c.eax = g.u32(PHASE_STATE)?;
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                g.set_u8(c.eax.wrapping_add(1), 0)?;
                g.set_u8(c.eax.wrapping_add(2), 0)?;
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, COLLECT_FRAME, false);
                return Ok(Next::Ret { pop: 0 });
            }
            let types = g.u32(c.edi.wrapping_sub(8))?;
            c.edx = 1u32 << (c.ebp & 31);
            c.ecx = c.ebp;
            logic(c, c.edx & types, SIGN);
            c.eax = types;
            if c.edx & types == 0 {
                return collect_next(c, g);
            }
            let gated = g.u8(c.edi.wrapping_sub(4))?;
            c.eax = byte(c.eax, gated);
            if test_low(c, c.eax) {
                let done = g.u8(c.esp.wrapping_add(0x13))?;
                c.eax = byte(c.eax, done);
                if !test_low(c, c.eax) {
                    return collect_next(c, g);
                }
            }
            if test_low(c, c.ebx) {
                return collect_main(c, g);
            }
            c.eax = g.u32(c.edi)?;
            logic(c, c.eax, SIGN);
            if c.eax != 0 {
                push(c, g, 0x2800)?;
                c.ecx = c.esp.wrapping_add(0x218);
                push(c, g, c.ecx)?;
                push(c, g, c.ebp)?;
                return call(c.eax, COLLECT_SITES[2]);
            }
            c.ebx = byte(c.ebx, 1);
            collect_main(c, g)
        }
        0xbf48a => {
            c.ebx = byte(c.ebx, 1);
            collect_main(c, g)
        }
        0xbf4b1 => {
            let done = g.u8(c.esp.wrapping_add(0x12))?;
            c.edx = byte(c.edx, done);
            test_low(c, c.edx);
            c.eax = byte(c.eax, u8::from(done == 0));
            if test_low(c, c.eax) {
                return collect_next(c, g);
            }
            c.esi = g.u32(PHASE_STATE)?;
            collect_pass(c, g)
        }
        _ => Err(unowned(pc)),
    }
}

/// `bf3af`/`bf405`/`bf419`: `push edi`, the mode into `ebp` (`None`: it
/// is already there), the done byte and `bl` cleared, the table's first
/// pass.
fn collect_start(c: &mut CpuAbi, g: &mut Guest, mode: Option<u32>) -> Result<Next> {
    if mode.is_none() && c.ebp == NONE {
        return collect_end(c, g, c.esi);
    }
    push(c, g, c.edi)?;
    if let Some(mode) = mode {
        c.ebp = mode;
    }
    g.set_u8(c.esp.wrapping_add(0x13), 0)?;
    c.ebx = byte(c.ebx, 0);
    logic(c, 0, 0x80);
    c.edi = COLLECT_TABLE;
    collect_pass(c, g)
}

/// `bf433`: the pass at `edi`, or the end of the table.
fn collect_pass(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(c.edi.wrapping_add(4))?;
    logic(c, c.eax, SIGN);
    if c.eax == 0 {
        c.edi = pop(c, g)?;
        return collect_end(c, g, c.esi);
    }
    c.esi = c.esp.wrapping_add(0x13);
    c.eax = c.ebp;
    call(0xbf1c0, COLLECT_SITES[1])
}

/// `bf48c`: the pass's `main`.
fn collect_main(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, 0x200)?;
    c.edx = c.esp.wrapping_add(0x18);
    push(c, g, c.edx)?;
    c.eax = c.esp.wrapping_add(0x1a);
    push(c, g, c.eax)?;
    push(c, g, 0x2800)?;
    c.ecx = c.esp.wrapping_add(0x224);
    push(c, g, c.ecx)?;
    push(c, g, c.ebp)?;
    g.set_u8(c.esp.wrapping_add(0x2a), 0)?;
    let main = g.u32(c.edi.wrapping_add(4))?;
    call(main, COLLECT_SITES[3])
}

/// `bf4c2`: the next pass.
fn collect_next(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edi = arithmetic(c, c.edi, 0x10, false);
    c.ebx = byte(c.ebx, 0);
    logic(c, 0, 0x80);
    c.esi = g.u32(PHASE_STATE)?;
    collect_pass(c, g)
}

/// `bf4cc`/`bf4f7`: the phase state's `+1` and `+2` cleared, the frame
/// gone. `edi` is already popped.
fn collect_end(c: &mut CpuAbi, g: &mut Guest, state: u32) -> Result<Next> {
    g.set_u8(state.wrapping_add(1), 0)?;
    g.set_u8(state.wrapping_add(2), 0)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, COLLECT_FRAME, false);
    Ok(Next::Ret { pop: 0 })
}

// `bb050`: the spatial visit.

/// `bb050`'s sites, as the register-level port lists them.
pub(super) const VISIT_SITES: [u32; 3] = crate::sim_physics::visit::SITES;

/// `bb050 (lists, types, location, centre, radius, out, capacity)`;
/// `ret 1c`. The body is the register-level port `sim_physics::visit`
/// taken into the table, as `bd090` takes `sim_physics::node_matrices`
/// ([`super::pose`]): its calls to the cluster flood `14a5b0` and the list
/// heads `b8940` and `b8a10` are guest calls.
pub(super) fn spatial_visit(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_SPATIAL_VISIT.base && !VISIT_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let t = crate::sim_physics::visit::objects(c, g, pc)?;
    match t.kind {
        abi::CALL => {
            // `Guest::call` pushed the return address; `drive` pushes it.
            c.esp = c.esp.wrapping_add(4);
            Ok(Next::Call { target: t.target_pc, site: t.resume_pc })
        }
        abi::RETURN => {
            // `Guest::ret` popped the return address and the arguments;
            // `drive` pops them.
            c.esp = c.esp.wrapping_sub(4 + 28);
            Ok(Next::Ret { pop: 28 })
        }
        _ => unimplemented!("objects bb050: the port made a transfer that is neither a call nor a return"),
    }
}

#[cfg(test)]
mod tests;
