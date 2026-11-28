//! The light-list operations: link `c2d00`, unlink `c3220` and expire
//! `c3260`. The lists themselves are kept by guest code (`1cac60` adds
//! a light, `1cae40` removes it, `16ba40` frees the datum); these bodies
//! decide when, and compute what the lists are given.
//!
//! **Link `c2d00 (light: stack 0)`; `ret 4`.** Does nothing unless the
//! light is active (flags bit 1). `sub esp, 7c`, then `ebx`, `ebp`, `edi`,
//! and `esi` on the active path. With `F` the `esp` after the four
//! pushes: `F+13` the byte `1cac60` answers through, `F+14` the
//! definition, `F+18` the owner's node matrices (`b8c40`) and then the
//! out pointer for `16a8e0`, `F+1c` the `b8d30` marker buffer; `edi`
//! holds the element throughout. Steps:
//!
//! 1. Flags bit 2: `c0840(element +90, element +a0, element +9c)`
//!    (`esi` element `+84`, `ret 8`).
//! 2. The world position, by the first of: a function source (`+54` not
//!    `ffff`): the owner's node matrices from `b8c40(ecx = owner)`; with
//!    definition flags bit 5, `16640f(matrices, +84, +ac, +b8)` (`ecx`
//!    the owner, `ret 10`) and, when it answers, position `+90` is the
//!    sum of `+84` and `+ac`. Otherwise (or when it does not answer), use
//!    the owner's marker through `b8d30(owner, matrices, 1)` (`cl` 0,
//!    `ebx` the buffer, `ret c`), whose origin and two directions go to `+84`,
//!    `+ac`, `+b8` before the sum. Else a marker parent (`+58` not none):
//!    `142570(edx +84, eax +60, matrix)` (`ret 4`) puts the origin
//!    through the parent's node matrix (body `+116` plus `34` × node
//!    `+5c`), then the local directions `+6c` and `+78` are rotated by
//!    the matrix's three rows into `+ac` and `+b8` (scalar SSE, retail's
//!    order) and `+90` = `+84` + `+ac`. Else the position stays.
//! 3. `c28b0(eax = handle)`.
//! 4. Unlinked (bit 3 clear), with no location word (`+48` `ffff`), a
//!    definition flags bit 15 and a radius above zero (`comiss` with the
//!    image's zero): the cluster query `16a8e0(0d800001, +18, radius,
//!    handle, -1, +18, F+18)` (`ret 1c`) and its result copied from
//!    `+18` to `+38`.
//! 5. `11bed0(edx +38, esi +44)`; then, with no location word and
//!    either flags bit 0 clear or definition flags bit 3 set, the
//!    location from the owner or marker parent (`ba300(eax +44, ecx
//!    the object)`) or, with neither, `11bed0(edx +18, esi +44)`.
//! 6. `1cac60(4e0310, handle, +10, +18, radius, +44, 0, 0, F+13)` with
//!    `eax` 0 (`ret 24`), then flags bit 3 set (linked).
//!
//! **Unlink `c3220 (light: eax)`; `ret`.** An active light is removed:
//! `1cae40(handle, +10)` with `edi` the light lists (saved), then flags
//! bit 3 cleared. Saves `esi` (the element).
//!
//! **Expire `c3260 (light: stack 0, notify: stack 1)`; `ret 8`.** Saves
//! `ebx` (the element), `ebp` (the handle), `esi` (the light array) and
//! `edi`. A linked, active light is removed from the lists first. With a
//! marker parent (`+58`) and `notify` set, it looks through every other
//! live light (the iterator `16bc00`, `ecx` the array, `edx` the next
//! index) for one with the same parent, and sets (any found) or clears
//! (none) bit 6 of the parent's body flags `+4`; the parent's body
//! pointer is stored over the `notify` slot, and `bl` is the found byte.
//! Then `16ba40(eax handle, edx array)` frees the light.

use super::{DATA, ELEMENT_SIZE, LIGHT_LISTS, LIGHTS, STRIDE, at, flags_undefined};
use guest_layout::points;
use crate::sim_objects::{Cx, Next, Result, sse, unowned};
use crate::sim_physics::{byte, pop, push};
use runtime::{CpuAbi, Guest};
use sse::Ss::{Add, Mul};

const NONE: u32 = u32::MAX;

const MARKER_FOLLOW: u32 = 0x000c_0840;
const NODE_MATRICES: u32 = 0x000b_8c40;
const MARKER_FRAME: u32 = 0x0016_640f;
const MARKER_GET: u32 = 0x000b_8d30;
const TRANSFORM_POINT: u32 = 0x0014_2570;
const LIGHT_CHANGED: u32 = 0x000c_28b0;
const CLUSTER_QUERY: u32 = 0x0016_a8e0;
const LOCATION: u32 = 0x0011_bed0;
const OBJECT_LOCATION: u32 = 0x000b_a300;
const LIST_ADD: u32 = 0x001c_ac60;
const LIST_REMOVE: u32 = 0x001c_ae40;
const NEXT_LIVE: u32 = 0x0016_bc00;
const DATUM_FREE: u32 = 0x0016_ba40;
/// The cluster query's flags argument (pushed as an immediate).
const CLUSTER_FLAGS: u32 = 0x0d80_0001;
/// Binary32 zero.
const ZERO: u32 = 0x0045_dbd8;

/// After each of `c2d00`'s calls.
pub(in crate::sim_objects) const LINK_SITES: [u32; 11] =
    [0xc2d71, 0xc2d8b, 0xc2db1, 0xc2e07, 0xc2edd, 0xc3045, 0xc3081, 0xc309f, 0xc30c0, 0xc30d2, 0xc30f5];
/// After `c3220`'s call.
pub(in crate::sim_objects) const UNLINK_SITES: [u32; 1] = [0xc3253];
/// After each of `c3260`'s calls.
pub(in crate::sim_objects) const EXPIRE_SITES: [u32; 4] = [0xc329b, 0xc32da, 0xc3317, 0xc332d];

/// `movss xmm0, [a]; addss xmm0, [b]; movss [dst], xmm0`.
fn sum(c: &mut CpuAbi, g: &mut Guest, a: u32, b: u32, dst: u32) -> Result<()> {
    sse::load(c, 0, g.u32(a)?);
    sse::ss(c, Add, 0, g.u32(b)?);
    g.set_u32(dst, sse::lane0(c, 0))
}

/// One rotated direction (`c2edd` to `c2f65`, and `c2f6d` to `c2ff8`):
/// the local vector at `v` through the matrix rows at `esi +04..+24`,
/// into `out`, `out + 4`, `out + 8`, in retail's operand order.
fn rotate(c: &mut CpuAbi, g: &mut Guest, v: u32, out: u32) -> Result<()> {
    let m = c.esi;
    sse::load(c, 1, g.u32(at(v, 4))?);
    sse::load(c, 2, g.u32(at(v, 8))?);
    sse::load(c, 0, g.u32(v)?);
    // x
    sse::load(c, 3, g.u32(at(m, 0x1c))?);
    sse::load(c, 4, g.u32(at(m, 0x10))?);
    sse::ss_reg(c, Mul, 3, 2);
    sse::ss_reg(c, Mul, 4, 1);
    sse::ss_reg(c, Add, 3, 4);
    sse::load(c, 4, g.u32(at(m, 4))?);
    sse::ss_reg(c, Mul, 4, 0);
    sse::ss_reg(c, Add, 3, 4);
    g.set_u32(out, sse::lane0(c, 3))?;
    // y
    sse::load(c, 3, g.u32(at(m, 8))?);
    sse::load(c, 4, g.u32(at(m, 0x20))?);
    sse::ss_reg(c, Mul, 3, 0);
    sse::ss_reg(c, Mul, 4, 2);
    sse::ss_reg(c, Add, 3, 4);
    sse::load(c, 4, g.u32(at(m, 0x14))?);
    sse::ss_reg(c, Mul, 4, 1);
    sse::ss_reg(c, Add, 3, 4);
    g.set_u32(at(out, 4), sse::lane0(c, 3))?;
    // z
    sse::load(c, 3, g.u32(at(m, 0xc))?);
    sse::ss_reg(c, Mul, 3, 0);
    sse::load(c, 0, g.u32(at(m, 0x24))?);
    sse::ss_reg(c, Mul, 0, 2);
    sse::ss_reg(c, Add, 3, 0);
    sse::load(c, 0, g.u32(at(m, 0x18))?);
    sse::ss_reg(c, Mul, 0, 1);
    sse::ss_reg(c, Add, 3, 0);
    g.set_u32(at(out, 8), sse::lane0(c, 3))
}

/// `c30fa`: restore and `ret 4` (`esi` already popped or never pushed).
fn link_leave(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = c.esp.wrapping_add(0x7c);
    flags_undefined(c);
    Ok(Next::Ret { pop: 4 })
}

/// `c2d00`; `ret 4`.
pub(in crate::sim_objects) fn link(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::LIGHT_LINK.base;
    if pc != entry && !LINK_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut next = pc;
    loop {
        next = match next {
            _ if next == entry => {
                c.esp = c.esp.wrapping_sub(0x7c);
                c.eax = g.u32(LIGHTS)?;
                c.edx = g.u32(points::TAG_ENTRIES.base)?;
                push(c, g, c.ebx)?;
                c.ebx = g.u32(at(c.eax, DATA))?;
                push(c, g, c.ebp)?;
                c.ebp = g.u32(at(c.esp, 0x88))?;
                push(c, g, c.edi)?;
                c.edi = (c.ebp & 0xffff) * STRIDE;
                c.ecx = g.u32(at(at(c.edi, c.ebx), 4))?;
                let flags = g.u16(at(at(c.edi, c.ebx), 2))?;
                c.eax = (c.eax & !0xffff) | u32::from(flags);
                c.edi = at(c.edi, c.ebx);
                c.ecx = (c.ecx & 0xffff) << 4;
                c.ebx = g.u32(at(at(c.ecx, c.edx), 8))?;
                g.set_u32(at(c.esp, 0x10), c.ebx)?;
                if flags & 2 == 0 {
                    return link_leave(c, g);
                }
                push(c, g, c.esi)?;
                if flags & 4 == 0 {
                    0xc2d71
                } else {
                    c.eax = at(c.edi, 0x9c);
                    push(c, g, c.eax)?;
                    c.ecx = at(c.edi, 0xa0);
                    push(c, g, c.ecx)?;
                    c.eax = at(c.edi, 0x90);
                    c.esi = at(c.edi, 0x84);
                    flags_undefined(c);
                    return Ok(Next::Call { target: MARKER_FOLLOW, site: 0xc2d71 });
                }
            }
            0xc2d71 => {
                c.eax = u32::from(g.u16(at(c.edi, 0x54))?);
                if c.eax == 0xffff {
                    0xc2e9b
                } else {
                    c.ebp = g.u32(at(c.edi, 0x4c))?;
                    c.ecx = c.ebp;
                    flags_undefined(c);
                    return Ok(Next::Call { target: NODE_MATRICES, site: 0xc2d8b });
                }
            }
            0xc2d8b => {
                let marker = g.u8(c.ebx)? & 0x20 != 0;
                g.set_u32(at(c.esp, 0x18), c.eax)?;
                if !marker {
                    0xc2df5
                } else {
                    c.edx = at(c.edi, 0xb8);
                    push(c, g, c.edx)?;
                    c.esi = at(c.edi, 0xac);
                    push(c, g, c.esi)?;
                    c.ebx = at(c.edi, 0x84);
                    push(c, g, c.ebx)?;
                    push(c, g, c.eax)?;
                    c.ecx = c.ebp;
                    flags_undefined(c);
                    return Ok(Next::Call { target: MARKER_FRAME, site: 0xc2db1 });
                }
            }
            0xc2db1 => {
                if c.eax & 0xff == 0 {
                    c.eax = g.u32(at(c.esp, 0x18))?;
                    0xc2df5
                } else {
                    sse::load(c, 0, g.u32(c.ebx)?);
                    sse::ss(c, Add, 0, g.u32(c.esi)?);
                    c.ebp = g.u32(at(c.esp, 0x90))?;
                    g.set_u32(at(c.edi, 0x90), sse::lane0(c, 0))?;
                    sum(c, g, at(c.ebx, 4), at(c.esi, 4), at(c.edi, 0x94))?;
                    sse::load(c, 0, g.u32(at(c.ebx, 8))?);
                    sse::ss(c, Add, 0, g.u32(at(c.esi, 8))?);
                    c.ebx = g.u32(at(c.esp, 0x14))?;
                    0xc3036
                }
            }
            0xc2df5 => {
                push(c, g, 1)?;
                push(c, g, c.eax)?;
                c.eax = g.u32(at(c.edi, 0x4c))?;
                push(c, g, c.eax)?;
                c.ecx = byte(c.ecx, 0);
                c.ebx = at(c.esp, 0x28);
                flags_undefined(c);
                return Ok(Next::Call { target: MARKER_GET, site: 0xc2e07 });
            }
            0xc2e07 => {
                let sp = c.esp;
                c.edx = g.u32(at(sp, 0x7c))?;
                c.eax = at(c.edi, 0x84);
                c.ecx = c.eax;
                g.set_u32(c.ecx, c.edx)?;
                c.edx = g.u32(at(sp, 0x80))?;
                g.set_u32(at(c.ecx, 4), c.edx)?;
                c.edx = g.u32(at(sp, 0x84))?;
                g.set_u32(at(c.ecx, 8), c.edx)?;
                c.esi = g.u32(at(sp, 0x58))?;
                c.ebp = g.u32(at(sp, 0x90))?;
                c.ebx = g.u32(at(sp, 0x14))?;
                c.ecx = at(c.edi, 0xac);
                c.edx = c.ecx;
                g.set_u32(c.edx, c.esi)?;
                c.esi = g.u32(at(sp, 0x5c))?;
                g.set_u32(at(c.edx, 4), c.esi)?;
                c.esi = g.u32(at(sp, 0x60))?;
                g.set_u32(at(c.edx, 8), c.esi)?;
                c.esi = g.u32(at(sp, 0x70))?;
                c.edx = at(c.edi, 0xb8);
                g.set_u32(c.edx, c.esi)?;
                c.esi = g.u32(at(sp, 0x74))?;
                g.set_u32(at(c.edx, 4), c.esi)?;
                c.esi = g.u32(at(sp, 0x78))?;
                g.set_u32(at(c.edx, 8), c.esi)?;
                sum(c, g, c.ecx, c.eax, at(c.edi, 0x90))?;
                sum(c, g, at(c.eax, 4), at(c.ecx, 4), at(c.edi, 0x94))?;
                sse::load(c, 0, g.u32(at(c.eax, 8))?);
                sse::ss(c, Add, 0, g.u32(at(c.ecx, 8))?);
                0xc3036
            }
            0xc2e9b => {
                c.eax = g.u32(at(c.edi, 0x58))?;
                if c.eax == NONE {
                    0xc303e
                } else {
                    c.ecx = g.u32(points::OBJECTS.base)?;
                    c.edx = g.u32(at(c.ecx, DATA))?;
                    c.ecx = g.u16(at(c.edi, 0x5c))? as i16 as i32 as u32;
                    c.ecx = c.ecx.wrapping_mul(0x34);
                    c.eax &= 0xffff;
                    c.eax *= 3;
                    c.eax = g.u32(at(c.edx, c.eax * 4 + 8))?;
                    c.esi = g.u16(at(c.eax, 0x116))? as i16 as i32 as u32;
                    c.ecx = at(c.ecx, c.eax);
                    c.esi = at(c.esi, c.ecx);
                    c.edx = at(c.edi, 0x84);
                    c.eax = at(c.edi, 0x60);
                    push(c, g, c.esi)?;
                    flags_undefined(c);
                    return Ok(Next::Call { target: TRANSFORM_POINT, site: 0xc2edd });
                }
            }
            0xc2edd => {
                rotate(c, g, at(c.edi, 0x6c), at(c.edi, 0xac))?;
                rotate(c, g, at(c.edi, 0x78), at(c.edi, 0xb8))?;
                sum(c, g, c.edx, at(c.edi, 0xac), at(c.edi, 0x90))?;
                sum(c, g, at(c.edx, 4), at(c.edi, 0xb0), at(c.edi, 0x94))?;
                sse::load(c, 0, g.u32(at(c.edx, 8))?);
                sse::ss(c, Add, 0, g.u32(at(c.edi, 0xb4))?);
                0xc3036
            }
            0xc3036 => {
                g.set_u32(at(c.edi, 0x98), sse::lane0(c, 0))?;
                0xc303e
            }
            0xc303e => {
                c.eax = c.ebp;
                flags_undefined(c);
                return Ok(Next::Call { target: LIGHT_CHANGED, site: 0xc3045 });
            }
            0xc3045 => {
                let query = g.u8(at(c.edi, 2))? & 8 == 0 && g.u16(at(c.edi, 0x48))? == 0xffff && {
                    c.eax = g.u32(c.ebx)?;
                    c.eax & 0x8000 != 0
                } && {
                    sse::load(c, 0, g.u32(at(c.edi, 0x24))?);
                    sse::comiss(c, sse::lane0(c, 0), g.u32(ZERO)?);
                    !sse::below_or_equal(c)
                };
                if !query {
                    0xc3094
                } else {
                    c.eax = g.u32(at(c.edi, 0x24))?;
                    c.edx = at(c.esp, 0x18);
                    push(c, g, c.edx)?;
                    c.esi = at(c.edi, 0x18);
                    push(c, g, c.esi)?;
                    push(c, g, NONE)?;
                    push(c, g, c.ebp)?;
                    push(c, g, c.eax)?;
                    push(c, g, c.esi)?;
                    push(c, g, CLUSTER_FLAGS)?;
                    flags_undefined(c);
                    return Ok(Next::Call { target: CLUSTER_QUERY, site: 0xc3081 });
                }
            }
            0xc3081 => {
                c.edx = g.u32(c.esi)?;
                c.eax = g.u32(at(c.esi, 4))?;
                c.ecx = at(c.edi, 0x38);
                g.set_u32(c.ecx, c.edx)?;
                c.edx = g.u32(at(c.esi, 8))?;
                g.set_u32(at(c.ecx, 4), c.eax)?;
                g.set_u32(at(c.ecx, 8), c.edx)?;
                0xc3094
            }
            0xc3094 => {
                c.esi = at(c.edi, 0x44);
                c.edx = at(c.edi, 0x38);
                flags_undefined(c);
                return Ok(Next::Call { target: LOCATION, site: 0xc309f });
            }
            0xc309f => {
                if g.u16(at(c.edi, 0x48))? != 0xffff
                    || (g.u8(at(c.edi, 2))? & 1 != 0 && g.u8(c.ebx)? & 8 == 0)
                {
                    0xc30d2
                } else {
                    c.ecx = g.u32(at(c.edi, 0x4c))?;
                    let object = c.ecx != NONE || {
                        c.ecx = g.u32(at(c.edi, 0x58))?;
                        c.ecx != NONE
                    };
                    flags_undefined(c);
                    if object {
                        c.eax = c.esi;
                        return Ok(Next::Call { target: OBJECT_LOCATION, site: 0xc30c0 });
                    }
                    c.edx = at(c.edi, 0x18);
                    return Ok(Next::Call { target: LOCATION, site: 0xc30d2 });
                }
            }
            0xc30c0 => 0xc30d2,
            0xc30d2 => {
                c.ecx = g.u32(at(c.edi, 0x24))?;
                c.eax = at(c.esp, 0x13);
                push(c, g, c.eax)?;
                push(c, g, 0)?;
                push(c, g, 0)?;
                push(c, g, c.esi)?;
                push(c, g, c.ecx)?;
                c.edx = at(c.edi, 0x18);
                push(c, g, c.edx)?;
                c.eax = at(c.edi, 0x10);
                push(c, g, c.eax)?;
                push(c, g, c.ebp)?;
                push(c, g, LIGHT_LISTS)?;
                c.eax = 0;
                flags_undefined(c);
                return Ok(Next::Call { target: LIST_ADD, site: 0xc30f5 });
            }
            0xc30f5 => {
                let a = at(c.edi, 2);
                let v = g.u8(a)? | 8;
                g.set_u8(a, v)?;
                c.esi = pop(c, g)?;
                return link_leave(c, g);
            }
            _ => return Err(unowned(next)),
        };
    }
}

/// `c3220`; `ret`.
pub(in crate::sim_objects) fn unlink(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::LIGHT_UNLINK.base {
        c.ecx = g.u32(LIGHTS)?;
        c.edx = g.u32(at(c.ecx, DATA))?;
        push(c, g, c.esi)?;
        c.esi = (c.eax & 0xffff) * STRIDE;
        let flags = g.u8(at(at(c.esi, c.edx), 2))?;
        c.ecx = byte(c.ecx, flags);
        c.esi = at(c.esi, c.edx);
        flags_undefined(c);
        if flags & 2 == 0 {
            c.esi = pop(c, g)?;
            return Ok(Next::Ret { pop: 0 });
        }
        push(c, g, c.edi)?;
        c.edx = at(c.esi, 0x10);
        push(c, g, c.edx)?;
        push(c, g, c.eax)?;
        c.edi = LIGHT_LISTS;
        return Ok(Next::Call { target: LIST_REMOVE, site: UNLINK_SITES[0] });
    }
    if pc == UNLINK_SITES[0] {
        let a = at(c.esi, 2);
        let v = g.u8(a)? & 0xf7;
        g.set_u8(a, v)?;
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
        flags_undefined(c);
        return Ok(Next::Ret { pop: 0 });
    }
    Err(unowned(pc))
}

/// `c3317`/`c332d`: restore and `ret 8`.
fn expire_leave(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    flags_undefined(c);
    Ok(Next::Ret { pop: 8 })
}

/// `c3260`; `ret 8`.
pub(in crate::sim_objects) fn expire(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::LIGHT_EXPIRE.base;
    if pc != entry && !EXPIRE_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut next = pc;
    loop {
        next = match next {
            _ if next == entry => {
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                c.ebp = g.u32(at(c.esp, 0xc))?;
                c.eax = (c.ebp & 0xffff) * STRIDE;
                push(c, g, c.esi)?;
                c.esi = g.u32(LIGHTS)?;
                c.ecx = g.u32(at(c.esi, DATA))?;
                c.ebx = at(c.eax, c.ecx);
                let flags = g.u8(at(c.ebx, 2))?;
                c.eax = byte(c.eax, flags);
                push(c, g, c.edi)?;
                if flags & 8 == 0 || flags & 2 == 0 {
                    0xc329f
                } else {
                    c.edx = at(c.ebx, 0x10);
                    push(c, g, c.edx)?;
                    push(c, g, c.ebp)?;
                    c.edi = LIGHT_LISTS;
                    flags_undefined(c);
                    return Ok(Next::Call { target: LIST_REMOVE, site: 0xc329b });
                }
            }
            0xc329b => {
                let a = at(c.ebx, 2);
                let v = g.u8(a)? & 0xf7;
                g.set_u8(a, v)?;
                0xc329f
            }
            0xc329f => {
                c.edi = g.u32(at(c.ebx, 0x58))?;
                if c.edi == NONE {
                    0xc3324
                } else {
                    let notify = g.u8(at(c.esp, 0x18))?;
                    c.eax = byte(c.eax, notify);
                    if notify == 0 {
                        0xc3324
                    } else {
                        c.ecx = g.u32(points::OBJECTS.base)?;
                        c.edx = g.u32(at(c.ecx, DATA))?;
                        c.eax = (c.edi & 0xffff) * 3;
                        c.eax = g.u32(at(c.edx, c.eax * 4 + 8))?;
                        g.set_u32(at(c.esp, 0x18), c.eax)?;
                        c.ebx = byte(c.ebx, 0);
                        c.eax = NONE;
                        0xc32d0
                    }
                }
            }
            0xc32d0 => {
                c.edx = c.eax.wrapping_add(1);
                c.ecx = c.esi;
                flags_undefined(c);
                return Ok(Next::Call { target: NEXT_LIVE, site: 0xc32da });
            }
            0xc32da => {
                if c.eax == NONE {
                    0xc32fd
                } else {
                    c.ecx = g.u32(at(c.esi, ELEMENT_SIZE))?;
                    c.edx = g.u32(at(c.esi, DATA))?;
                    c.ecx = c.ecx.wrapping_mul(c.eax);
                    c.ecx = at(c.ecx, c.edx);
                    c.edx = g.u16(c.ecx)? as i16 as i32 as u32;
                    c.edx = (c.edx << 16) | c.eax;
                    if c.edx == c.ebp || g.u32(at(c.ecx, 0x58))? != c.edi {
                        0xc32d0
                    } else {
                        c.ebx = byte(c.ebx, 1);
                        0xc32fd
                    }
                }
            }
            0xc32fd => {
                c.eax = g.u32(at(c.esp, 0x18))?;
                c.ecx = g.u32(at(c.eax, 4))?;
                if c.ebx & 0xff != 0 {
                    c.ecx |= 0x40;
                    g.set_u32(at(c.eax, 4), c.ecx)?;
                    c.eax = c.ebp;
                    c.edx = c.esi;
                    flags_undefined(c);
                    return Ok(Next::Call { target: DATUM_FREE, site: 0xc3317 });
                }
                c.ecx &= !0x40;
                g.set_u32(at(c.eax, 4), c.ecx)?;
                0xc3324
            }
            0xc3324 => {
                c.eax = c.ebp;
                c.edx = c.esi;
                flags_undefined(c);
                return Ok(Next::Call { target: DATUM_FREE, site: 0xc332d });
            }
            0xc3317 | 0xc332d => return expire_leave(c, g),
            _ => return Err(unowned(next)),
        };
    }
}
