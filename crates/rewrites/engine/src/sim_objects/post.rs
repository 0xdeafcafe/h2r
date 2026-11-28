//! The rest of the post-update shell (slice 4 of
//! docs/engine/sim-rewrite-objects.md; contract in
//! docs/engine/object-update.md, "Slice 4 bodies"): the physics component
//! to object transform `bc5e0`, the attachment refresh `bef30` and the
//! node-matrix notify `be1d0`. `bc820` calls all three; the bounds
//! `bdef0` is in `bounds.rs`, the node matrices `bd090` in `pose.rs`.
//!
//! **`bc5e0 (object: stack 0)`; `ret 4`.** `sub esp, 58`, `ebx`, `edi`;
//! with the component active (body `+c0` bit 6) also `ebp` (the component
//! record) and `esi`. With `S` the `esp` after all four pushes: `S+13`
//! `1d1c10`'s answer (the rigid body moved), `S+14` the body, `S+18` the
//! up vector's address, `S+1c` the angular and `S+28` the linear
//! velocity, `S+34` the `34`-byte transform `1d4360` fills (its forward
//! at `S+38`, up at `S+50`, position at `S+5c`), `S+6c` the object. The
//! order: `1d1c10(esi record)`; when it answered, clear component flags
//! bit 16; with rigid bodies (`+74` above 0) and component flags bit 8
//! clear, `1d1ca0(edi record)`, and when that answers, `1d4360(&S+34)`,
//! `b91d0(eax object, esi &S+38, edi &S+50)` (the basis made
//! parent-relative) unless component flags bit 1, then
//! `b7430(object, &position, forward, up, 0, 0, 0, 0, 1)`; when `1d1c10`
//! answered, the velocities (`1d09d0`, and `1d0ad0` unless flags bit 1);
//! then `b7740(object, linear, angular, 0)`, with the zero vector
//! `[4687a4]` for both when the component did not move. Last, outside
//! network mode 4, with body `+c0` bit 11, `1d96d0(eax component handle)`
//! and clear body `+c1` bit 3. Without an active component: body `+c1`
//! bit 0 becomes the parent's (1 for none). Every path ends setting or
//! clearing body `+c1` bit 0 from `bl`.
//!
//! **`bef30 (object, lights, light lists, recurse, ignore removal)`;
//! `ret 14`.** `bc820` passes `(object, 1, 1, 0, 0)`. `ebx`, `ebp`, `esi`,
//! `edi` saved; `esi` the object, `edi` its body, `bl` "being removed"
//! (`b9ce0(ecx object)`'s answer, or 0 with argument 4) across the calls.
//! A DELETE header or body `+04` bit 0 ends it at once with argument 4.
//! With body `+04` bit 4, for each of the `(+11c) >> 3` attachment
//! records at `[+11e]` (8 bytes: a type byte, a light datum): a light
//! with a datum, while not being removed, gets `c3220(eax light)` with
//! argument 1 and `c2d00(light)` with argument 2. With body `+04` bit 6,
//! `c1720(object, argument 1, argument 2)`. Then the first child
//! (`+10`) recursively, `bef30(child, argument 1, argument 2, 1, bl ==
//! 0)`, and with argument 3 the next sibling (`+0c`) by overwriting its
//! own object argument and looping.
//!
//! **`be1d0 (object: eax)`; `ret`.** `push ecx` (the count local), `ebx`
//! the object. With a model (definition `+38`) whose two references `+4`
//! and `+14` are set: `b8c00(eax object, &count)` (`ret 4`) and the
//! slot `+74` dispatcher `109140(object, count, matrices)` (`ret 8`).

use super::alu::{rmw8, shr, test8};
use super::transform::{RIGID_ANGULAR, RIGID_LINEAR, ZERO_VECTOR};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use runtime::{CpuAbi, Guest};

const COMPONENTS: u32 = 0x0051_e9b8;
const NONE: u32 = u32::MAX;

// `bc5e0`'s callees.
/// The rigid body moved (`esi` record) -> `al`; `ret`.
const COMPONENT_MOVED: u32 = 0x001d_1c10;
/// The component has a transform (`edi` record) -> `al`; `ret`.
const COMPONENT_HAS_TRANSFORM: u32 = 0x001d_1ca0;
/// The component's transform into a `34`-byte matrix (`edi` record,
/// stack 0 out); `ret 4`.
const COMPONENT_TRANSFORM: u32 = 0x001d_4360;
/// The basis made parent-relative (`eax` object, `esi`, `edi`); `ret`.
const PARENT_RELATIVE: u32 = 0x000b_91d0;
/// `(eax component handle)`; `ret`.
const COMPONENT_SETTLE: u32 = 0x001d_96d0;

// `bef30`'s callees.
/// Whether the object is being removed (`ecx` object) -> `al`; `ret`.
const REMOVING: u32 = 0x000b_9ce0;
/// Light list removal (`eax` light); `ret`.
const LIGHT_UNLINK: u32 = 0x000c_3220;
/// Light list update (`light`); `ret 4`.
const LIGHT_LINK: u32 = 0x000c_2d00;
/// `(object, a, b)`; `ret 0c`.
const OBJECT_LIGHTS: u32 = 0x000c_1720;

// `be1d0`'s callees.
/// `(eax object, count out: stack 0) -> eax` the node matrices; `ret 4`.
const NODE_MATRICES_OF: u32 = 0x000b_8c00;
/// The slot `+74` dispatcher `(ebx object, count, matrices)`; `ret 8`.
const NODES_DISPATCH: u32 = 0x0010_9140;

pub(super) const COMPONENT_SITES: [u32; 9] =
    [0xbc639, 0xbc678, 0xbc68a, 0xbc6b7, 0xbc6d6, 0xbc6ed, 0xbc704, 0xbc723, 0xbc75a];
pub(super) const ATTACH_SITES: [u32; 5] = [0xbef85, 0xbefd4, 0xbefe4, 0xbf009, 0xbf029];
pub(super) const NOTIFY_SITES: [u32; 2] = [0xbe22a, 0xbe235];

fn ld(g: &Guest, a: u32, off: u32) -> Result<u32> {
    g.u32(a.wrapping_add(off))
}

/// `bc5e0 (object: stack 0)`; `ret 4`.
pub(super) fn component_transform(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_COMPONENT_TRANSFORM.base && !COMPONENT_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    loop {
        pc = match pc {
            0xbc5e0 => {
                c.esp = arithmetic(c, c.esp, 0x58, true);
                c.eax = g.u32(points::OBJECTS.base)?;
                c.ecx = ld(g, c.eax, DATA)?;
                push(c, g, c.ebx)?;
                c.ebx = ld(g, c.esp, 0x60)?;
                c.eax = c.ebx & 0xffff;
                c.edx = c.eax.wrapping_mul(3);
                push(c, g, c.edi)?;
                c.edi = ld(g, c.ecx, c.edx.wrapping_mul(4).wrapping_add(8))?;
                let bits = g.u8(c.edi.wrapping_add(0xc0))? >> 6;
                c.eax = byte(c.eax, bits);
                logic(c, u32::from(bits & 1), 0x80);
                g.set_u32(c.esp.wrapping_add(0xc), c.edi)?;
                if bits & 1 == 0 {
                    0xbc763
                } else {
                    c.eax = ld(g, c.edi, 0xb4)?;
                    c.ecx = g.u32(COMPONENTS)?;
                    c.edx = ld(g, c.ecx, DATA)?;
                    push(c, g, c.ebp)?;
                    c.ebp = ((c.eax & 0xffff).wrapping_mul(5)) << 5;
                    c.ebp = arithmetic(c, c.ebp, c.edx, false);
                    c.eax &= 0xffff;
                    push(c, g, c.esi)?;
                    c.esi = c.ebp;
                    return Ok(Next::Call { target: COMPONENT_MOVED, site: COMPONENT_SITES[0] });
                }
            }
            0xbc639 => {
                logic(c, c.eax & 0xff, 0x80);
                g.set_u8(c.esp.wrapping_add(0x13), c.eax as u8)?;
                if c.eax & 0xff != 0 {
                    let flags = ld(g, c.ebp, 4)? & 0xfffe_ffff;
                    g.set_u32(c.ebp.wrapping_add(4), flags)?;
                    logic(c, flags, 1 << 31);
                    0xbc657
                } else if test8(c, g, c.edi.wrapping_add(0xc1), 1)? {
                    0xbc657
                } else {
                    0xbc727
                }
            }
            0xbc657 => {
                c.eax = ld(g, c.ebp, 0x74)?;
                logic(c, c.eax, 1 << 31);
                if c.eax as i32 <= 0 {
                    0xbc727
                } else {
                    c.edx = ld(g, c.ebp, 4)?;
                    c.edx = shr(c, c.edx, 8, 32);
                    logic(c, c.edx & 1, 0x80);
                    if c.edx & 1 != 0 {
                        0xbc712
                    } else {
                        c.edi = c.ebp;
                        return Ok(Next::Call { target: COMPONENT_HAS_TRANSFORM, site: COMPONENT_SITES[1] });
                    }
                }
            }
            0xbc678 => {
                logic(c, c.eax & 0xff, 0x80);
                if c.eax & 0xff == 0 {
                    0xbc712
                } else {
                    c.eax = c.esp.wrapping_add(0x34);
                    push(c, g, c.eax)?;
                    return Ok(Next::Call { target: COMPONENT_TRANSFORM, site: COMPONENT_SITES[2] });
                }
            }
            0xbc68a => {
                c.edx = ld(g, c.ebp, 4)?;
                c.edx = shr(c, c.edx, 1, 32);
                logic(c, c.edx & 1, 0x80);
                c.ecx = c.esp.wrapping_add(0x50);
                c.ebx = c.esp.wrapping_add(0x38);
                g.set_u32(c.esp.wrapping_add(0x18), c.ecx)?;
                if c.edx & 1 != 0 {
                    c.ebx = 0;
                    c.eax = 0;
                    logic(c, 0, 1 << 31);
                    0xbc6bb
                } else {
                    c.eax = ld(g, c.esp, 0x6c)?;
                    c.edi = c.esp.wrapping_add(0x50);
                    c.esi = c.esp.wrapping_add(0x38);
                    return Ok(Next::Call { target: PARENT_RELATIVE, site: COMPONENT_SITES[3] });
                }
            }
            0xbc6b7 => {
                c.eax = ld(g, c.esp, 0x18)?;
                0xbc6bb
            }
            0xbc6bb => {
                c.esi = ld(g, c.esp, 0x6c)?;
                for v in [1, 0, 0, 0, 0] {
                    push(c, g, v)?;
                }
                push(c, g, c.eax)?;
                push(c, g, c.ebx)?;
                c.eax = c.esp.wrapping_add(0x78);
                push(c, g, c.eax)?;
                push(c, g, c.esi)?;
                return Ok(Next::Call { target: points::OBJECT_SET_TRANSFORM.base, site: COMPONENT_SITES[4] });
            }
            0xbc6d6 => {
                let moved = g.u8(c.esp.wrapping_add(0x13))?;
                c.eax = byte(c.eax, moved);
                logic(c, u32::from(moved), 0x80);
                if moved == 0 {
                    c.eax = c.esi;
                    0xbc714
                } else {
                    c.ecx = c.esp.wrapping_add(0x28);
                    c.eax = 0;
                    c.edx = c.ebp;
                    c.edi = 0;
                    logic(c, 0, 1 << 31);
                    return Ok(Next::Call { target: RIGID_LINEAR, site: COMPONENT_SITES[5] });
                }
            }
            0xbc6ed => {
                c.ecx = ld(g, c.ebp, 4)?;
                c.ecx = shr(c, c.ecx, 1, 32);
                logic(c, c.ecx & 1, 0x80);
                if c.ecx & 1 != 0 {
                    0xbc706
                } else {
                    c.ecx = c.esp.wrapping_add(0x1c);
                    c.eax = 0;
                    c.edx = c.ebp;
                    logic(c, 0, 1 << 31);
                    return Ok(Next::Call { target: RIGID_ANGULAR, site: COMPONENT_SITES[6] });
                }
            }
            0xbc704 => {
                c.edi = c.ecx;
                0xbc706
            }
            0xbc706 => {
                c.ecx = c.esp.wrapping_add(0x28);
                c.eax = c.esi;
                0xbc71c
            }
            0xbc712 => {
                c.eax = c.ebx;
                0xbc714
            }
            0xbc714 => {
                c.ecx = g.u32(ZERO_VECTOR)?;
                c.edi = c.ecx;
                0xbc71c
            }
            0xbc71c => {
                push(c, g, 0)?;
                return Ok(Next::Call { target: points::OBJECT_SET_VELOCITY.base, site: COMPONENT_SITES[7] });
            }
            0xbc723 => {
                c.edi = ld(g, c.esp, 0x14)?;
                0xbc727
            }
            0xbc727 => {
                let moved = g.u8(c.esp.wrapping_add(0x13))?;
                c.ecx = byte(c.ecx, moved);
                c.edx = g.u32(points::GAME_GLOBALS.base)?;
                let mode = g.u8(c.edx.wrapping_add(0xc))?;
                c.eax = byte(c.eax, mode);
                c.ebx = byte(c.ebx, u8::from(moved == 0));
                byte_cmp(c, mode, 4);
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                if mode == 4 {
                    0xbc784
                } else {
                    c.eax = u32::from(g.u16(c.edi.wrapping_add(0xc0))?);
                    c.eax = shr(c, c.eax, 0xb, 32);
                    logic(c, c.eax & 1, 0x80);
                    if c.eax & 1 == 0 {
                        0xbc784
                    } else {
                        c.eax = ld(g, c.edi, 0xb4)?;
                        return Ok(Next::Call { target: COMPONENT_SETTLE, site: COMPONENT_SITES[8] });
                    }
                }
            }
            0xbc75a => {
                rmw8(c, g, c.edi.wrapping_add(0xc1), |b| b & 0xf7)?;
                0xbc784
            }
            0xbc763 => {
                c.eax = ld(g, c.edi, 0x14)?;
                arithmetic(c, c.eax, NONE, true);
                if c.eax == NONE {
                    c.ebx = byte(c.ebx, 1);
                } else {
                    c.eax &= 0xffff;
                    c.edx = c.eax.wrapping_mul(3);
                    c.eax = ld(g, c.ecx, c.edx.wrapping_mul(4).wrapping_add(8))?;
                    let attached = g.u8(c.eax.wrapping_add(0xc1))? & 1;
                    c.ebx = byte(c.ebx, attached);
                    logic(c, u32::from(attached), 0x80);
                }
                0xbc784
            }
            0xbc784 => {
                let set = c.ebx as u8;
                logic(c, u32::from(set), 0x80);
                let at = c.edi.wrapping_add(0xc1);
                if set != 0 {
                    rmw8(c, g, at, |b| b | 1)?;
                } else {
                    rmw8(c, g, at, |b| b & 0xfe)?;
                }
                c.edi = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x58, false);
                return Ok(Next::Ret { pop: 4 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}

/// `cmp r8, imm8`: the flags of the byte subtraction.
fn byte_cmp(c: &mut CpuAbi, a: u8, b: u8) {
    let (a, b) = (u32::from(a), u32::from(b));
    let v = a.wrapping_sub(b) & 0xff;
    let cf = u32::from(a < b);
    let of = u32::from((a ^ b) & (a ^ v) & 0x80 != 0) << 11;
    let flags = crate::sim_physics::status(v, 0x80) | cf | ((a ^ b ^ v) & 0x10) | of;
    c.eflags = (c.eflags & !crate::sim_physics::ARITH) | flags;
    c.eflags_defined |= crate::sim_physics::ARITH;
}

/// `bef30 (object, lights, light lists, recurse, ignore removal)`; `ret 14`.
pub(super) fn attachments(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_ATTACHMENTS.base && !ATTACH_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    loop {
        pc = match pc {
            0xbef30 => {
                for r in [c.ebx, c.ebp, c.esi, c.edi] {
                    push(c, g, r)?;
                }
                0xbef40
            }
            0xbef40 => {
                c.esi = ld(g, c.esp, 0x14)?;
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = ld(g, c.ecx, DATA)?;
                let ignore = g.u8(c.esp.wrapping_add(0x24))?;
                c.ecx = byte(c.ecx, ignore);
                c.eax = (c.esi & 0xffff).wrapping_mul(3);
                logic(c, u32::from(ignore), 0x80);
                c.edi = ld(g, c.edx, c.eax.wrapping_mul(4).wrapping_add(8))?;
                c.eax = c.edx.wrapping_add(c.eax.wrapping_mul(4));
                if ignore == 0 {
                    c.ecx = c.esi;
                    return Ok(Next::Call { target: REMOVING, site: ATTACH_SITES[0] });
                }
                if test8(c, g, c.eax.wrapping_add(2), 0x10)? || test8(c, g, c.edi.wrapping_add(4), 1)? {
                    return finish(c, g);
                }
                c.ebx = byte(c.ebx, 0);
                logic(c, 0, 0x80);
                0xbef8f
            }
            0xbef85 => {
                c.ebx = byte(c.ebx, c.eax as u8);
                logic(c, c.ebx & 0xff, 0x80);
                if c.ebx & 0xff != 0 {
                    return finish(c, g);
                }
                0xbef8f
            }
            0xbef8f => {
                c.eax = ld(g, c.edi, 4)?;
                c.eax = shr(c, c.eax, 4, 32);
                logic(c, c.eax & 1, 0x80);
                if c.eax & 1 == 0 {
                    0xbefee
                } else {
                    c.eax = g.u16(c.edi.wrapping_add(0x11c))? as i16 as i32 as u32;
                    c.ecx = g.u16(c.edi.wrapping_add(0x11e))? as i16 as i32 as u32;
                    c.eax = shr(c, c.eax, 3, 32);
                    c.ecx = arithmetic(c, c.ecx, c.edi, false);
                    logic(c, c.eax, 1 << 31);
                    if c.eax as i32 <= 0 {
                        0xbefee
                    } else {
                        c.esi = c.ecx.wrapping_add(4);
                        c.ebp = c.eax;
                        0xbefb5
                    }
                }
            }
            0xbefb5 => {
                let kind = g.u8(c.esi.wrapping_sub(4))?;
                c.eax = byte(c.eax, kind);
                logic(c, u32::from(kind), 0x80);
                if kind != 0 {
                    0xbefe4
                } else {
                    c.eax = g.u32(c.esi)?;
                    arithmetic(c, c.eax, NONE, true);
                    if c.eax == NONE {
                        0xbefe4
                    } else {
                        logic(c, c.ebx & 0xff, 0x80);
                        if c.ebx & 0xff != 0 {
                            0xbefe4
                        } else {
                            let lights = g.u8(c.esp.wrapping_add(0x18))?;
                            c.ecx = byte(c.ecx, lights);
                            logic(c, u32::from(lights), 0x80);
                            if lights != 0 {
                                return Ok(Next::Call { target: LIGHT_UNLINK, site: ATTACH_SITES[1] });
                            }
                            0xbefd4
                        }
                    }
                }
            }
            0xbefd4 => {
                let lists = g.u8(c.esp.wrapping_add(0x1c))?;
                c.eax = byte(c.eax, lists);
                logic(c, u32::from(lists), 0x80);
                if lists == 0 {
                    0xbefe4
                } else {
                    c.ecx = g.u32(c.esi)?;
                    push(c, g, c.ecx)?;
                    return Ok(Next::Call { target: LIGHT_LINK, site: ATTACH_SITES[2] });
                }
            }
            0xbefe4 => {
                c.esi = arithmetic(c, c.esi, 8, false);
                c.ebp = super::alu::dec32(c, c.ebp);
                if c.ebp != 0 {
                    0xbefb5
                } else {
                    c.esi = ld(g, c.esp, 0x14)?;
                    0xbefee
                }
            }
            0xbefee => {
                c.edx = ld(g, c.edi, 4)?;
                c.edx = shr(c, c.edx, 6, 32);
                logic(c, c.edx & 1, 0x80);
                if c.edx & 1 == 0 {
                    0xbf009
                } else {
                    c.eax = ld(g, c.esp, 0x1c)?;
                    c.ecx = ld(g, c.esp, 0x18)?;
                    push(c, g, c.eax)?;
                    push(c, g, c.ecx)?;
                    push(c, g, c.esi)?;
                    return Ok(Next::Call { target: OBJECT_LIGHTS, site: ATTACH_SITES[3] });
                }
            }
            0xbf009 => {
                c.eax = ld(g, c.edi, 0x10)?;
                arithmetic(c, c.eax, NONE, true);
                if c.eax == NONE {
                    0xbf029
                } else {
                    c.ecx = ld(g, c.esp, 0x1c)?;
                    logic(c, c.ebx & 0xff, 0x80);
                    c.edx = byte(c.edx, u8::from(c.ebx & 0xff == 0));
                    push(c, g, c.edx)?;
                    c.edx = ld(g, c.esp, 0x1c)?;
                    push(c, g, 1)?;
                    push(c, g, c.ecx)?;
                    push(c, g, c.edx)?;
                    push(c, g, c.eax)?;
                    return Ok(Next::Call { target: points::OBJECT_ATTACHMENTS.base, site: ATTACH_SITES[4] });
                }
            }
            0xbf029 => {
                let siblings = g.u8(c.esp.wrapping_add(0x20))?;
                c.eax = byte(c.eax, siblings);
                logic(c, u32::from(siblings), 0x80);
                if siblings == 0 {
                    return finish(c, g);
                }
                c.eax = ld(g, c.edi, 0xc)?;
                arithmetic(c, c.eax, NONE, true);
                if c.eax == NONE {
                    return finish(c, g);
                }
                g.set_u32(c.esp.wrapping_add(0x14), c.eax)?;
                0xbef40
            }
            _ => return Err(unowned(pc)),
        };
    }
}

/// `bf042`: restore and `ret 14`.
fn finish(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    Ok(Next::Ret { pop: 0x14 })
}

/// `be1d0 (object: eax)`; `ret`.
pub(super) fn nodes_notify(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        p if p == points::OBJECT_NODES_NOTIFY.base => {
            push(c, g, c.ecx)?;
            c.ecx = g.u32(points::OBJECTS.base)?;
            c.edx = ld(g, c.ecx, DATA)?;
            c.ecx = g.u32(points::TAG_ENTRIES.base)?;
            push(c, g, c.ebx)?;
            c.ebx = c.eax;
            c.eax = ld(g, c.edx, (c.eax & 0xffff).wrapping_mul(12).wrapping_add(8))?;
            c.edx = (g.u32(c.eax)? & 0xffff) << 4;
            c.eax = ld(g, c.edx, c.ecx.wrapping_add(8))?;
            c.eax = ld(g, c.eax, 0x38)?;
            c.edx = NONE;
            arithmetic(c, c.eax, c.edx, true);
            if c.eax == NONE {
                return notify_ret(c, g);
            }
            c.eax = (c.eax & 0xffff) << 4;
            c.eax = ld(g, c.eax, c.ecx.wrapping_add(8))?;
            for off in [4, 0x14] {
                let r = ld(g, c.eax, off)?;
                arithmetic(c, r, c.edx, true);
                if r == NONE {
                    return notify_ret(c, g);
                }
            }
            c.eax = c.esp.wrapping_add(4);
            push(c, g, c.eax)?;
            c.eax = c.ebx;
            Ok(Next::Call { target: NODE_MATRICES_OF, site: NOTIFY_SITES[0] })
        }
        p if p == NOTIFY_SITES[0] => {
            c.ecx = ld(g, c.esp, 4)?;
            push(c, g, c.eax)?;
            push(c, g, c.ecx)?;
            Ok(Next::Call { target: NODES_DISPATCH, site: NOTIFY_SITES[1] })
        }
        p if p == NOTIFY_SITES[1] => notify_ret(c, g),
        _ => Err(unowned(pc)),
    }
}

/// `be235`: `pop ebx; pop ecx; ret`.
fn notify_ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}
