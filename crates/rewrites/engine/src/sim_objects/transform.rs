//! The transform setters and getters of slice 4
//! (docs/engine/sim-rewrite-objects.md section 12; contract in
//! docs/engine/object-update.md, "Slice 4 bodies"): `b7430` sets an
//! object's position and basis, `b7740` its velocities, `b9dd0` reads its
//! world position, `b9fc0` its world basis and `ba1d0` its root
//! ancestor's velocities.
//!
//! The three getters are called from all over the image (the type
//! updates, weapons, AI), so each reproduces the arithmetic flags at its
//! return from the last instruction that sets them, as well as all eight
//! integer registers and the `xmm` registers. The setters do the same.
//!
//! **`b9dd0 (object: eax, out: esi) -> eax`; `ret`.** `eax` returns `out`.
//! An unparented object copies body `+64`. A child transforms its body
//! `+64` (parent-relative) through the parent's node matrix
//! `[parent + [parent+116]] + 34 × (i8) body[+18]`, with the matrix scale
//! applied when it is not 1 (`xform`). `edi` is pushed and popped around
//! the matrix lookup.
//!
//! **`b9fc0 (object: eax, forward: esi, up: edi)`; `ret`.** Either output
//! may be null. Unparented: copies body `+70` and `+7c`. A child rotates
//! them through the parent node matrix (no translation, no scale).
//!
//! **`ba1d0 (object: eax, linear: ecx, angular: stack 0)`; `ret 4`.** Walks
//! up the parent chain (`+14`) to the root. With a physics component whose
//! rigid body index (component record `+18`, below its count `+74`) is
//! valid: `1d09d0(eax index, edx record, ecx out)` for the linear and
//! `1d0ad0` for the angular velocity, or the vector at `[4687a4]` for
//! both when body `+c1` bit 0 (attached to an early mover) is set.
//! Otherwise body `+88` and `+94`. Either output may be null.
//!
//! **`b7430 (object, position, forward, up, update, wake, notify, value,
//! keep connected)`; `ret 24`.** Nine stack arguments. `push ecx` makes a
//! one-dword local whose byte 3 is the result. In the partition (body
//! `+04` bit 8) and argument 8 clear: `b87b0(object)` first. A position is
//! accepted for an unparented object only inside the image's world bounds
//! (`comiss` against `[45e4ac]` and `[45dffc]`, each component); a
//! rejected one sets the result 0 and stores nothing. Accepted positions
//! go to body `+64`, forward and up to `+70` and `+7c`; each with a
//! replication entity (body `+d4`) marks it dirty (`b58c0(entity, 2)` for
//! the position, `(entity, 4)` for the basis). Argument 6 calls
//! `1c4a80(eax object, position null, argument 7)`, argument 5
//! `bd020(object)` (the pose and bounds refresh); then, if `b87b0` ran,
//! `b8600(object, argument 4)`. Returns `al`.
//!
//! **`b7740 (object: eax, linear: ecx, angular: edi, quiet: stack 0)`;
//! `ret 4`.** Stores either vector to body `+88` / `+94` and, unless
//! `quiet`, marks the replication entity dirty (`b58c0(entity, 10 | 20)`).

use super::sse;
use super::xform::{self, load};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use runtime::{CpuAbi, Guest};

const COMPONENTS: u32 = 0x0051_e9b8;
/// Holds a pointer to the zero vector the getters and `bc5e0` hand out.
pub(super) const ZERO_VECTOR: u32 = 0x0046_87a4;
/// Rigid-body linear and angular velocity (`eax` index, `edx` record,
/// `ecx` out; `ret`).
pub(super) const RIGID_LINEAR: u32 = 0x001d_09d0;
pub(super) const RIGID_ANGULAR: u32 = 0x001d_0ad0;
/// Replication dirty bits (`(entity, bits)`, `ret 8`).
const REPLICATION_DIRTY: u32 = 0x000b_58c0;
const DISCONNECT: u32 = 0x000b_87b0;
const CONNECT: u32 = 0x000b_8600;
/// `(eax object, stack 0, stack 1)`, `ret 8`.
const NOTIFY: u32 = 0x001c_4a80;
/// The pose and bounds refresh (`ret 4`).
const REFRESH: u32 = 0x000b_d020;
/// The world bounds a position is checked against: low and high.
const WORLD_LOW: u32 = 0x0045_e4ac;
const WORLD_HIGH: u32 = 0x0045_dffc;
const NONE: u32 = u32::MAX;

pub(super) const SET_TRANSFORM_SITES: [u32; 6] = [0xb7471, 0xb74ef, 0xb753f, 0xb755f, 0xb7571, 0xb7590];
pub(super) const SET_VELOCITY_SITES: [u32; 1] = [0xb77bf];
pub(super) const VELOCITY_SITES: [u32; 2] = [0xba286, 0xba2b3];

fn ld(g: &Guest, a: u32, off: u32) -> Result<u32> {
    g.u32(a.wrapping_add(off))
}

/// The body of `handle` (`[[4e0300]+44] + 12 × (handle & ffff) + 8`).
fn body_of(g: &Guest, data: u32, handle: u32) -> Result<u32> {
    ld(g, data, (handle & 0xffff).wrapping_mul(12).wrapping_add(8))
}

/// `b9dd0 (object: eax, out: esi) -> eax`; `ret`.
pub(super) fn world_position(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_WORLD_POSITION.base {
        return Err(unowned(pc));
    }
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = ld(g, c.ecx, DATA)?;
    c.eax = body_of(g, c.edx, c.eax)?;
    c.ecx = ld(g, c.eax, 0x14)?;
    arithmetic(c, c.ecx, NONE, true);
    if c.ecx == NONE {
        c.eax = arithmetic(c, c.eax, 0x64, false);
        c.ecx = c.esi;
        let v = xform::copy3(g, c.eax, c.ecx)?;
        c.edx = v[1];
        c.eax = c.esi;
        return Ok(Next::Ret { pop: 0 });
    }
    load(c, g, 0, c.eax.wrapping_add(0x64))?;
    load(c, g, 1, c.eax.wrapping_add(0x68))?;
    load(c, g, 2, c.eax.wrapping_add(0x6c))?;
    push(c, g, c.edi)?;
    let node = g.u8(c.eax.wrapping_add(0x18))? as i8 as i32 as u32;
    c.edi = node.wrapping_mul(0x34);
    c.ecx = (c.ecx & 0xffff).wrapping_mul(3);
    c.edx = ld(g, c.edx, c.ecx.wrapping_mul(4).wrapping_add(8))?;
    c.ecx = g.u16(c.edx.wrapping_add(0x116))? as i16 as i32 as u32;
    c.edi = c.edi.wrapping_add(c.edx);
    load(c, g, 3, c.ecx.wrapping_add(c.edi))?;
    c.ecx = c.ecx.wrapping_add(c.edi);
    let unit_scale = xform::ucomiss_equal(c, g, 3, xform::ONE)?;
    c.edi = pop(c, g)?;
    if !unit_scale {
        xform::scale(c, g, c.ecx)?;
    }
    xform::rows(c, g, c.ecx, c.esi, true)?;
    c.eax = c.esi;
    Ok(Next::Ret { pop: 0 })
}

/// `b9fc0 (object: eax, forward: esi, up: edi)`; `ret`.
pub(super) fn world_basis(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_WORLD_BASIS.base {
        return Err(unowned(pc));
    }
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = ld(g, c.ecx, DATA)?;
    c.ecx = body_of(g, c.edx, c.eax)?;
    c.eax = ld(g, c.ecx, 0x14)?;
    arithmetic(c, c.eax, NONE, true);
    if c.eax == NONE {
        logic(c, c.esi, 1 << 31);
        if c.esi != 0 {
            c.edx = c.ecx.wrapping_add(0x70);
            let v = xform::copy3(g, c.edx, c.esi)?;
            c.eax = v[1];
            c.edx = v[2];
        }
        logic(c, c.edi, 1 << 31);
        if c.edi == 0 {
            return Ok(Next::Ret { pop: 0 });
        }
        c.ecx = arithmetic(c, c.ecx, 0x7c, false);
        let v = xform::copy3(g, c.ecx, c.edi)?;
        c.edx = v[1];
        c.eax = v[2];
        return Ok(Next::Ret { pop: 0 });
    }
    push(c, g, c.ebx)?;
    let node = g.u8(c.ecx.wrapping_add(0x18))? as i8 as i32 as u32;
    c.ebx = node.wrapping_mul(0x34);
    c.eax = (c.eax & 0xffff).wrapping_mul(3);
    c.edx = ld(g, c.edx, c.eax.wrapping_mul(4).wrapping_add(8))?;
    c.eax = g.u16(c.edx.wrapping_add(0x116))? as i16 as i32 as u32;
    c.ebx = c.ebx.wrapping_add(c.edx);
    c.eax = c.eax.wrapping_add(c.ebx);
    logic(c, c.esi, 1 << 31);
    c.ebx = pop(c, g)?;
    for (out, v) in [(c.esi, 0x70u32), (c.edi, 0x7c)] {
        logic(c, out, 1 << 31);
        if out == 0 {
            if v == 0x70 {
                continue;
            }
            break;
        }
        load(c, g, 1, c.ecx.wrapping_add(v + 4))?;
        load(c, g, 2, c.ecx.wrapping_add(v + 8))?;
        load(c, g, 0, c.ecx.wrapping_add(v))?;
        xform::rows(c, g, c.eax, out, false)?;
    }
    // The last flags are `test edi, edi`'s.
    logic(c, c.edi, 1 << 31);
    Ok(Next::Ret { pop: 0 })
}

/// `ba1d0 (object: eax, linear: ecx, angular: stack 0)`; `ret 4`.
pub(super) fn root_velocity(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        p if p == points::OBJECT_ROOT_VELOCITY.base => {
            push(c, g, c.ebx)?;
            push(c, g, c.esi)?;
            c.edx = NONE;
            logic(c, c.edx, 1 << 31);
            arithmetic(c, c.eax, NONE, true);
            push(c, g, c.edi)?;
            c.edi = g.u32(points::OBJECTS.base)?;
            if c.eax != NONE {
                loop {
                    c.esi = ld(g, c.edi, DATA)?;
                    c.edx = c.eax;
                    c.eax = body_of(g, c.esi, c.eax)?;
                    c.eax = ld(g, c.eax, 0x14)?;
                    arithmetic(c, c.eax, NONE, true);
                    if c.eax == NONE {
                        break;
                    }
                }
            }
            c.eax = ld(g, c.edi, DATA)?;
            c.edx = body_of(g, c.eax, c.edx)?;
            c.eax = ld(g, c.edx, 0xb4)?;
            arithmetic(c, c.eax, NONE, true);
            if c.eax == NONE {
                return no_component(c, g);
            }
            c.esi = (c.eax & 0xffff).wrapping_mul(5);
            c.eax = g.u32(COMPONENTS)?;
            c.ebx = ld(g, c.eax, DATA)?;
            c.esi <<= 5;
            let index = g.u8(c.esi.wrapping_add(c.ebx).wrapping_add(0x18))?;
            c.eax = byte(c.eax, index);
            c.esi = c.esi.wrapping_add(c.ebx);
            logic(c, u32::from(index), 0x80);
            let mut valid = (index as i8) >= 0;
            if valid {
                c.ebx = ld(g, c.esi, 0x74)?;
                c.edi = index as i8 as i32 as u32;
                arithmetic(c, c.edi, c.ebx, true);
                valid = (c.edi as i32) < (c.ebx as i32);
            }
            if valid {
                c.eax = (c.eax & 0xffff_0000) | u32::from(index as i8 as i16 as u16);
            } else {
                c.eax = NONE;
                logic(c, c.eax, 1 << 31);
            }
            c.edi = c.eax as u16 as i16 as i32 as u32;
            arithmetic(c, c.edi, NONE, true);
            if c.edi == NONE {
                return no_component(c, g);
            }
            let attached = g.u8(c.edx.wrapping_add(0xc1))? & 1;
            c.ebx = byte(c.ebx, attached);
            logic(c, c.ecx, 1 << 31);
            push(c, g, c.ebp)?;
            c.ebp = g.u32(ZERO_VECTOR)?;
            if c.ecx != 0 {
                logic(c, u32::from(attached), 0x80);
                if attached == 0 {
                    c.eax = c.edi;
                    c.edx = c.esi;
                    return Ok(Next::Call { target: RIGID_LINEAR, site: VELOCITY_SITES[0] });
                }
                c.edx = c.ebp;
                let v = xform::copy3(g, c.edx, c.ecx)?;
                c.eax = v[1];
                c.edx = v[2];
            }
            angular(c, g)
        }
        p if p == VELOCITY_SITES[0] => angular(c, g),
        p if p == VELOCITY_SITES[1] => {
            c.ebp = pop(c, g)?;
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            c.ebx = pop(c, g)?;
            Ok(Next::Ret { pop: 4 })
        }
        _ => Err(unowned(pc)),
    }
}

/// `ba286`: the angular velocity, with `ebp` pushed.
fn angular(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = ld(g, c.esp, 0x14)?;
    logic(c, c.ecx, 1 << 31);
    if c.ecx != 0 {
        let attached = c.ebx as u8;
        logic(c, u32::from(attached), 0x80);
        if attached == 0 {
            c.eax = c.edi;
            c.edx = c.esi;
            return Ok(Next::Call { target: RIGID_ANGULAR, site: VELOCITY_SITES[1] });
        }
        let v = xform::copy3(g, c.ebp, c.ecx)?;
        c.edx = v[1];
        c.eax = v[2];
    }
    c.ebp = pop(c, g)?;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

/// `ba2ba`: no component, or no valid rigid body: body `+88` and `+94`
/// (`edx` the root's body).
fn no_component(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    logic(c, c.ecx, 1 << 31);
    if c.ecx != 0 {
        c.eax = c.edx.wrapping_add(0x88);
        let v = xform::copy3(g, c.eax, c.ecx)?;
        c.esi = v[1];
        c.eax = v[2];
    }
    c.eax = ld(g, c.esp, 0x10)?;
    logic(c, c.eax, 1 << 31);
    if c.eax != 0 {
        c.edx = arithmetic(c, c.edx, 0x94, false);
        let v = xform::copy3(g, c.edx, c.eax)?;
        c.ecx = v[1];
        c.edx = v[2];
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

/// `b7740 (object: eax, linear: ecx, angular: edi, quiet: stack 0)`; `ret 4`.
pub(super) fn set_velocity(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == SET_VELOCITY_SITES[0] {
        c.esi = pop(c, g)?;
        c.ebx = pop(c, g)?;
        return Ok(Next::Ret { pop: 4 });
    }
    if pc != points::OBJECT_SET_VELOCITY.base {
        return Err(unowned(pc));
    }
    c.eax &= 0xffff;
    push(c, g, c.ebx)?;
    c.ebx = g.u32(points::OBJECTS.base)?;
    c.edx = ld(g, c.ebx, DATA)?;
    c.eax = c.eax.wrapping_mul(3) << 2;
    push(c, g, c.esi)?;
    c.esi = ld(g, c.edx, c.eax.wrapping_add(8))?;
    c.edx = 0;
    logic(c, c.ecx, 1 << 31);
    if c.ecx != 0 {
        push(c, g, c.ebp)?;
        c.edx = c.esi.wrapping_add(0x88);
        let v = xform::copy3(g, c.ecx, c.edx)?;
        c.ebp = v[1];
        c.ecx = v[2];
        c.edx = 0x10;
        c.ebp = pop(c, g)?;
    }
    logic(c, c.edi, 1 << 31);
    if c.edi != 0 {
        c.esi = arithmetic(c, c.esi, 0x94, false);
        let v = xform::copy3(g, c.edi, c.esi)?;
        c.ecx = v[2];
        c.edx |= 0x20;
        logic(c, c.edx, 1 << 31);
    }
    c.ecx = byte(c.ecx, g.u8(c.esp.wrapping_add(0xc))?);
    logic(c, c.ecx & 0xff, 0x80);
    if c.ecx & 0xff == 0 {
        logic(c, c.edx, 1 << 31);
        if c.edx != 0 {
            c.ecx = ld(g, c.ebx, DATA)?;
            c.eax = ld(g, c.ecx, c.eax.wrapping_add(8))?;
            c.eax = ld(g, c.eax, 0xd4)?;
            arithmetic(c, c.eax, NONE, true);
            if c.eax != NONE {
                push(c, g, c.edx)?;
                push(c, g, c.eax)?;
                return Ok(Next::Call { target: REPLICATION_DIRTY, site: SET_VELOCITY_SITES[0] });
            }
        }
    }
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

/// `b7430`: nine stack arguments; `ret 24`.
pub(super) fn set_transform(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_SET_TRANSFORM.base && !SET_TRANSFORM_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    loop {
        pc = match pc {
            0xb7430 => {
                push(c, g, c.ecx)?;
                c.ecx = ld(g, c.esp, 8)?;
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                c.ebp = g.u32(points::OBJECTS.base)?;
                c.eax = c.ecx & 0xffff;
                push(c, g, c.esi)?;
                c.esi = c.eax.wrapping_mul(3) << 2;
                c.eax = ld(g, c.ebp, DATA)?;
                push(c, g, c.edi)?;
                c.edi = ld(g, c.eax, c.esi.wrapping_add(8))?;
                c.ebx = ld(g, c.edi, 4)? >> 8;
                c.ebx = byte(c.ebx, c.ebx as u8 & 1);
                logic(c, c.ebx & 0xff, 0x80);
                g.set_u8(c.esp.wrapping_add(0x13), 1)?;
                if c.ebx & 0xff != 0 {
                    c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x38))?);
                    logic(c, c.eax & 0xff, 0x80);
                    if c.eax & 0xff == 0 {
                        push(c, g, c.ecx)?;
                        return Ok(Next::Call { target: DISCONNECT, site: SET_TRANSFORM_SITES[0] });
                    }
                }
                0xb7471
            }
            0xb7471 => {
                c.ecx = ld(g, c.esp, 0x1c)?;
                logic(c, c.ecx, 1 << 31);
                if c.ecx == 0 {
                    0xb74f3
                } else {
                    let parent = ld(g, c.edi, 0x14)?;
                    arithmetic(c, parent, NONE, true);
                    if parent != NONE || in_world(c, g)? {
                        0xb74c2
                    } else {
                        g.set_u8(c.esp.wrapping_add(0x13), 0)?;
                        0xb74f3
                    }
                }
            }
            0xb74c2 => {
                c.edx = c.edi.wrapping_add(0x64);
                let v = xform::copy3(g, c.ecx, c.edx)?;
                c.eax = v[1];
                c.ecx = v[2];
                c.edx = ld(g, c.ebp, DATA)?;
                c.eax = ld(g, c.edx, c.esi.wrapping_add(8))?;
                c.eax = ld(g, c.eax, 0xd4)?;
                arithmetic(c, c.eax, NONE, true);
                if c.eax == NONE {
                    0xb74ef
                } else {
                    push(c, g, 2)?;
                    push(c, g, c.eax)?;
                    return Ok(Next::Call { target: REPLICATION_DIRTY, site: SET_TRANSFORM_SITES[1] });
                }
            }
            0xb74ef => {
                c.ecx = ld(g, c.esp, 0x1c)?;
                0xb74f3
            }
            0xb74f3 => {
                c.eax = ld(g, c.esp, 0x20)?;
                logic(c, c.eax, 1 << 31);
                if c.eax == 0 {
                    0xb7543
                } else {
                    c.ecx = c.edi.wrapping_add(0x70);
                    let v = xform::copy3(g, c.eax, c.ecx)?;
                    c.edx = v[1];
                    c.eax = v[2];
                    c.eax = ld(g, c.esp, 0x24)?;
                    c.edi = arithmetic(c, c.edi, 0x7c, false);
                    let v = xform::copy3(g, c.eax, c.edi)?;
                    c.ecx = v[0];
                    c.edx = v[1];
                    c.eax = v[2];
                    c.ecx = ld(g, c.ebp, DATA)?;
                    c.esi = ld(g, c.ecx, c.esi.wrapping_add(8))?;
                    c.eax = ld(g, c.esi, 0xd4)?;
                    arithmetic(c, c.eax, NONE, true);
                    if c.eax == NONE {
                        0xb753f
                    } else {
                        push(c, g, 4)?;
                        push(c, g, c.eax)?;
                        return Ok(Next::Call { target: REPLICATION_DIRTY, site: SET_TRANSFORM_SITES[2] });
                    }
                }
            }
            0xb753f => {
                c.ecx = ld(g, c.esp, 0x1c)?;
                0xb7543
            }
            0xb7543 => {
                c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x30))?);
                logic(c, c.eax & 0xff, 0x80);
                if c.eax & 0xff == 0 {
                    0xb755f
                } else {
                    c.edx = ld(g, c.esp, 0x34)?;
                    logic(c, c.ecx, 1 << 31);
                    c.eax = byte(c.eax, u8::from(c.ecx == 0));
                    push(c, g, c.edx)?;
                    push(c, g, c.eax)?;
                    c.eax = ld(g, c.esp, 0x20)?;
                    return Ok(Next::Call { target: NOTIFY, site: SET_TRANSFORM_SITES[3] });
                }
            }
            0xb755f => {
                c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x2c))?);
                logic(c, c.eax & 0xff, 0x80);
                if c.eax & 0xff == 0 {
                    0xb7571
                } else {
                    c.ecx = ld(g, c.esp, 0x18)?;
                    push(c, g, c.ecx)?;
                    return Ok(Next::Call { target: REFRESH, site: SET_TRANSFORM_SITES[4] });
                }
            }
            0xb7571 => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                let connected = c.ebx as u8;
                logic(c, u32::from(connected), 0x80);
                c.ebx = pop(c, g)?;
                if connected == 0 {
                    0xb7590
                } else {
                    c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(0x28))?);
                    logic(c, c.eax & 0xff, 0x80);
                    if c.eax & 0xff != 0 {
                        0xb7590
                    } else {
                        c.edx = ld(g, c.esp, 0x18)?;
                        c.eax = ld(g, c.esp, 8)?;
                        push(c, g, c.edx)?;
                        push(c, g, c.eax)?;
                        return Ok(Next::Call { target: CONNECT, site: SET_TRANSFORM_SITES[5] });
                    }
                }
            }
            0xb7590 => {
                c.eax = byte(c.eax, g.u8(c.esp.wrapping_add(3))?);
                c.ecx = pop(c, g)?;
                return Ok(Next::Ret { pop: 0x24 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}

/// `b747f..b74b9`: each position component inside `[low, high]`, compared
/// as retail does (`comiss`, `jb`), with `xmm0` to `xmm2` and the flags
/// left as the last compare leaves them. `ecx` is the position.
fn in_world(c: &mut CpuAbi, g: &Guest) -> Result<bool> {
    load(c, g, 0, c.ecx)?;
    load(c, g, 1, WORLD_LOW)?;
    sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
    if sse::below(c) {
        return Ok(false);
    }
    load(c, g, 0, WORLD_HIGH)?;
    sse::comiss(c, sse::lane0(c, 0), g.u32(c.ecx)?);
    if sse::below(c) {
        return Ok(false);
    }
    for off in [4, 8] {
        load(c, g, 2, c.ecx.wrapping_add(off))?;
        sse::comiss(c, sse::lane0(c, 2), sse::lane0(c, 1));
        if sse::below(c) {
            return Ok(false);
        }
        sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 2));
        if sse::below(c) {
            return Ok(false);
        }
    }
    Ok(true)
}

