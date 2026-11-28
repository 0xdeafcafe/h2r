//! The after-root slot `+70` callees the dispatcher `1090d0` calls
//! (docs/engine/object-update-bodies-core.md): `unit` (`112dc0`) and
//! `scen` (`10a390`).
//!
//! **`(object: stack 0, matrix: stack 1)`; `ret 8`.** Both save `esi` (the
//! parent handle) and `edi` (the object data array, then the parent's
//! body). The parent handle is unit body `+ [body+33e] + 28` or scenery
//! body `+138`. When it is not none and `badc0(eax: handle, -1)` (`ret 4`,
//! a guest call) answers an object, the matrix is composed in place with
//! the parent's root node matrix (body `+ [body+116]`): `142a60(ecx:
//! parent, edx: matrix, out: matrix)`, run inline through
//! `sim_native::compose` with the push and return address the guest call
//! makes.
//!
//! Exit: `eax` the last load, `badc0`'s answer or the matrix; `ecx` and
//! `edx` as the last load or callee left them; flags from `cmp esi, -1`,
//! `test eax, eax` or `add ecx, ...` before the compose.

use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{arithmetic, logic, pop, push};
use runtime::{CpuAbi, Guest};

const NONE: u32 = u32::MAX;
/// The handle lookup both call (`eax` the handle, the type mask on the
/// stack; `ret 4`).
const OBJECT_LOOKUP: u32 = 0x000b_adc0;

/// After the lookup, then the compose's return (the epilogue).
pub(super) const UNIT_SITES: [u32; 1] = [0x112df5];
const UNIT_EPILOGUE: u32 = 0x112e19;
pub(super) const SCENERY_SITES: [u32; 1] = [0x10a3be];
const SCENERY_EPILOGUE: u32 = 0x10a3e2;

/// `mov eax, [objects]; push esi; push edi; mov edi, [eax+44]; mov eax,
/// [esp+0c]; and eax, ffff; lea ecx, [eax+eax*2]`: the object's body
/// pointer's slot address, `ecx` the index × 3.
fn prologue(c: &mut CpuAbi, g: &mut Guest) -> Result<u32> {
    c.eax = g.u32(points::OBJECTS.base)?;
    push(c, g, c.esi)?;
    push(c, g, c.edi)?;
    c.edi = g.u32(c.eax.wrapping_add(DATA))?;
    c.eax = g.u32(c.esp.wrapping_add(0xc))? & 0xffff;
    logic(c, c.eax, 1 << 31);
    c.ecx = c.eax * 3;
    Ok(c.edi.wrapping_add(c.ecx * 4 + 8))
}

/// `cmp esi, -1; je epilogue`, else `push -1; mov eax, esi; call badc0`.
fn lookup(c: &mut CpuAbi, g: &mut Guest, site: u32) -> Result<Next> {
    arithmetic(c, c.esi, NONE, true);
    if c.esi == NONE {
        return epilogue(c, g);
    }
    push(c, g, NONE)?;
    c.eax = c.esi;
    Ok(Next::Call {
        target: OBJECT_LOOKUP,
        site,
    })
}

/// `and esi, ffff; lea edx, [esi+esi*2]`: the parent's body pointer.
fn parent(c: &mut CpuAbi, g: &Guest) -> Result<u32> {
    c.esi &= 0xffff;
    logic(c, c.esi, 1 << 31);
    c.edx = c.esi * 3;
    g.u32(c.edi.wrapping_add(c.edx * 4 + 8))
}

/// `mov edx, [esp+10]; push edx; add ecx, body; call 142a60`, inline, then
/// the epilogue.
fn compose(c: &mut CpuAbi, g: &mut Guest, body: u32, back: u32) -> Result<Next> {
    c.ecx = u32::from(g.u16(body.wrapping_add(0x116))?) as i16 as i32 as u32;
    c.edx = g.u32(c.esp.wrapping_add(0x10))?;
    push(c, g, c.edx)?;
    c.ecx = arithmetic(c, c.ecx, body, false);
    push(c, g, back)?;
    crate::sim_native::compose(c, g)?;
    epilogue(c, g)
}

/// `pop edi; pop esi; ret 8`.
fn epilogue(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 8 })
}

/// `test eax, eax; je epilogue`.
fn found(c: &mut CpuAbi) -> bool {
    logic(c, c.eax, 1 << 31);
    c.eax != 0
}

/// `112dc0 (object: stack 0, matrix: stack 1)`; `ret 8`. Slot `+70` of
/// `unit`.
pub(super) fn unit(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::UNIT_AFTER_ROOT.base {
        let slot = prologue(c, g)?;
        c.ecx = g.u32(slot)?;
        c.eax = u32::from(g.u16(c.ecx.wrapping_add(0x33e))?) as i16 as i32 as u32;
        c.esi = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(0x28))?;
        c.eax = arithmetic(c, c.eax, c.ecx, false);
        return lookup(c, g, UNIT_SITES[0]);
    }
    if pc != UNIT_SITES[0] {
        return Err(unowned(pc));
    }
    if !found(c) {
        return epilogue(c, g);
    }
    c.edi = parent(c, g)?;
    let body = c.edi;
    compose(c, g, body, UNIT_EPILOGUE)
}

/// `10a390 (object: stack 0, matrix: stack 1)`; `ret 8`. Slot `+70` of
/// `scen`.
pub(super) fn scenery(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::SCENERY_AFTER_ROOT.base {
        let slot = prologue(c, g)?;
        c.eax = g.u32(slot)?;
        c.esi = g.u32(c.eax.wrapping_add(0x138))?;
        return lookup(c, g, SCENERY_SITES[0]);
    }
    if pc != SCENERY_SITES[0] {
        return Err(unowned(pc));
    }
    if !found(c) {
        return epilogue(c, g);
    }
    c.eax = parent(c, g)?;
    let body = c.eax;
    compose(c, g, body, SCENERY_EPILOGUE)
}
