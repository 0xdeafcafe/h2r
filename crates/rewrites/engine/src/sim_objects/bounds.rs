//! The bounds update `bdef0` (slice 4 of docs/engine/sim-rewrite-objects.md;
//! contract in docs/engine/object-update.md, "Slice 4 bodies"): the
//! object's bounding spheres from its root node matrix, and its place in
//! the spatial partition.
//!
//! **`bdef0 (object: stack 0) -> al`; `ret 4`.** `sub esp, 14`, then
//! `ebx`, `ebp`, `esi`, `edi`. With `S` the `esp` after the pushes:
//! `S+13` the "in the partition" byte (body `+04` bit 8), `S+14` the
//! partition radius, `S+18..S+23` the partition centre, `S+28` the
//! object. `esi` holds the body and `ebx` the definition from the entry to
//! the return; `ebp` the header offset until the root matrix is read.
//!
//! A crate (`bloc`) with an active physics component asks `1d44f0` for
//! the sphere first (`(component record, &body+30, &body+3c)`, `ret 0c`);
//! when it answers, the centre and radius are its. Otherwise the centre is
//! the definition's offset (`+08..+10`, scaled by the root matrix scale
//! when that is not 1) through the root node matrix (`xform::rows`), and
//! the radius body scale `+a0` × definition `+04`. The second sphere is
//! the centre plus definition `+24..+2c` with radius scale × `+20` when
//! `+20` is above zero, else a copy of the first. Definition flags bit 12
//! picks which sphere the partition registers.
//!
//! In the partition: when the registered centre moved by more than the
//! image's threshold (squared distance, `comiss`; `ja`) or the radius
//! changed (`ucomiss`), disconnect (`b87b0(object)`, `ret 4`), store the
//! new registered sphere, connect (`b8600(object, 0)`, `ret 8`) and
//! return 1; else return 0 and store nothing more. Not in the partition:
//! store it and return 1.
//!
//! Flags at each call and at the return are the last instruction's that
//! sets them (`add esp, 14` at the return).

use super::sse::{self, Ss};
use super::xform::{self, load, ss_mem, store};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use runtime::Guest;

const COMPONENTS: u32 = 0x0051_e9b8;
/// The crate's component sphere (`ret 0c`).
const COMPONENT_SPHERE: u32 = 0x001d_44f0;
/// Partition disconnect (`ret 4`) and connect (`ret 8`).
const DISCONNECT: u32 = 0x000b_87b0;
const CONNECT: u32 = 0x000b_8600;
/// The image's 0.0 the second sphere's radius scale is compared with.
const ZERO: u32 = 0x0045_dbd8;
/// The image's squared-distance threshold for a moved centre.
const MOVED: u32 = 0x0045_e02c;
/// `1 << type` for `bloc` (type 11).
const CRATE_BIT: u32 = 0x800;

/// After `1d44f0`, `b87b0`, `b8600`.
pub(super) const BOUNDS_SITES: [u32; 3] = [0xbdf84, 0xbe18e, 0xbe1b8];

fn ld(g: &Guest, a: u32, off: u32) -> Result<u32> {
    g.u32(a.wrapping_add(off))
}

/// `bdef0 (object: stack 0) -> al`; `ret 4`.
pub(super) fn bounds(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_BOUNDS.base && !BOUNDS_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    loop {
        pc = match pc {
            0xbdef0 => {
                c.esp = arithmetic(c, c.esp, 0x14, true);
                c.eax = ld(g, c.esp, 0x18)?;
                push(c, g, c.ebx)?;
                c.eax &= 0xffff;
                push(c, g, c.ebp)?;
                c.ebp = c.eax.wrapping_mul(3);
                c.eax = g.u32(points::OBJECTS.base)?;
                c.ecx = ld(g, c.eax, DATA)?;
                c.eax = g.u32(points::TAG_ENTRIES.base)?;
                c.ebp <<= 2;
                push(c, g, c.esi)?;
                c.esi = ld(g, c.ecx, c.ebp.wrapping_add(8))?;
                c.edx = (g.u32(c.esi)? & 0xffff) << 4;
                c.ebx = ld(g, c.edx, c.eax.wrapping_add(8))?;
                c.edx = ld(g, c.esi, 4)? >> 8;
                c.edx = byte(c.edx, c.edx as u8 & 1);
                c.eax = ld(g, c.esi, 0xb4)?;
                arithmetic(c, c.eax, u32::MAX, true);
                push(c, g, c.edi)?;
                g.set_u8(c.esp.wrapping_add(0x13), c.edx as u8)?;
                if c.eax == u32::MAX {
                    0xbdf90
                } else {
                    c.ecx = byte(c.ecx, g.u8(c.esi.wrapping_add(0xaa))?);
                    c.edi = 1u32.wrapping_shl(c.ecx & 31);
                    logic(c, c.edi & CRATE_BIT, 1 << 31);
                    if c.edi & CRATE_BIT == 0 {
                        0xbdf90
                    } else {
                        let bits = g.u8(c.esi.wrapping_add(0xc0))? >> 6;
                        c.ecx = byte(c.ecx, bits);
                        logic(c, u32::from(bits & 1), 0x80);
                        if bits & 1 == 0 {
                            0xbdf90
                        } else {
                            c.eax &= 0xffff;
                            c.edx = c.eax.wrapping_mul(5);
                            c.eax = g.u32(COMPONENTS)?;
                            c.ecx = c.esi.wrapping_add(0x3c);
                            push(c, g, c.ecx)?;
                            c.ecx = ld(g, c.eax, DATA)?;
                            c.edx <<= 5;
                            c.edi = c.esi.wrapping_add(0x30);
                            push(c, g, c.edi)?;
                            c.edx = arithmetic(c, c.edx, c.ecx, false);
                            push(c, g, c.edx)?;
                            return Ok(Next::Call { target: COMPONENT_SPHERE, site: BOUNDS_SITES[0] });
                        }
                    }
                }
            }
            0xbdf84 => {
                logic(c, c.eax & 0xff, 0x80);
                c.edx = byte(c.edx, g.u8(c.esp.wrapping_add(0x13))?);
                if c.eax & 0xff != 0 { 0xbe07f } else { 0xbdf90 }
            }
            0xbdf90 => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.eax = ld(g, c.ecx, DATA)?;
                c.ebp = ld(g, c.eax, c.ebp.wrapping_add(8))?;
                c.ecx = g.u16(c.ebp.wrapping_add(0x116))? as i16 as i32 as u32;
                load(c, g, 3, c.ecx.wrapping_add(c.ebp))?;
                load(c, g, 0, c.ebx.wrapping_add(8))?;
                load(c, g, 1, c.ebx.wrapping_add(0xc))?;
                load(c, g, 2, c.ebx.wrapping_add(0x10))?;
                c.ecx = arithmetic(c, c.ecx, c.ebp, false);
                let unit_scale = xform::ucomiss_equal(c, g, 3, xform::ONE)?;
                c.edi = c.esi.wrapping_add(0x30);
                if !unit_scale {
                    xform::scale(c, g, c.ecx)?;
                }
                xform::rows(c, g, c.ecx, c.edi, true)?;
                load(c, g, 0, c.esi.wrapping_add(0xa0))?;
                ss_mem(c, g, Ss::Mul, 0, c.ebx.wrapping_add(4))?;
                store(c, g, c.esi.wrapping_add(0x3c), 0)?;
                0xbe07f
            }
            0xbe07f => {
                load(c, g, 0, c.ebx.wrapping_add(0x20))?;
                let zero = g.u32(ZERO)?;
                sse::comiss(c, sse::lane0(c, 0), zero);
                c.eax = c.esi.wrapping_add(0x50);
                if sse::below_or_equal(c) {
                    // A copy of the first sphere.
                    c.ecx = c.edi;
                    c.ebp = g.u32(c.ecx)?;
                    c.edx = c.eax;
                    g.set_u32(c.edx, c.ebp)?;
                    c.ebp = ld(g, c.ecx, 4)?;
                    c.ecx = ld(g, c.ecx, 8)?;
                    g.set_u32(c.edx.wrapping_add(4), c.ebp)?;
                    g.set_u32(c.edx.wrapping_add(8), c.ecx)?;
                    c.edx = ld(g, c.esi, 0x3c)?;
                    g.set_u32(c.esi.wrapping_add(0x5c), c.edx)?;
                    c.edx = byte(c.edx, g.u8(c.esp.wrapping_add(0x13))?);
                } else {
                    for i in 0..3u32 {
                        load(c, g, 0, c.edi.wrapping_add(4 * i))?;
                        ss_mem(c, g, Ss::Add, 0, c.ebx.wrapping_add(0x24 + 4 * i))?;
                        store(c, g, c.eax.wrapping_add(4 * i), 0)?;
                    }
                    load(c, g, 0, c.esi.wrapping_add(0xa0))?;
                    ss_mem(c, g, Ss::Mul, 0, c.ebx.wrapping_add(0x20))?;
                    store(c, g, c.esi.wrapping_add(0x5c), 0)?;
                }
                // The sphere the partition uses: definition flags bit 12.
                c.ecx = u32::from(g.u16(c.ebx.wrapping_add(2))?);
                c.ecx >>= 0xc;
                logic(c, c.ecx & 1, 0x80);
                let (from, radius) = if c.ecx & 1 != 0 { (c.eax, 0x5c) } else { (c.edi, 0x3c) };
                c.ebx = g.u32(from)?;
                c.ebp = ld(g, from, 4)?;
                c.edi = ld(g, from, 8)?;
                load(c, g, 0, c.esi.wrapping_add(radius))?;
                let in_partition = c.edx as u8;
                logic(c, u32::from(in_partition), 0x80);
                g.set_u32(c.esp.wrapping_add(0x18), c.ebx)?;
                g.set_u32(c.esp.wrapping_add(0x1c), c.ebp)?;
                g.set_u32(c.esp.wrapping_add(0x20), c.edi)?;
                store(c, g, c.esp.wrapping_add(0x14), 0)?;
                if in_partition == 0 {
                    0xbe192
                } else {
                    load(c, g, 2, c.esp.wrapping_add(0x20))?;
                    ss_mem(c, g, Ss::Sub, 2, c.esi.wrapping_add(0x48))?;
                    load(c, g, 0, c.esp.wrapping_add(0x18))?;
                    ss_mem(c, g, Ss::Sub, 0, c.esi.wrapping_add(0x40))?;
                    load(c, g, 1, c.esp.wrapping_add(0x1c))?;
                    ss_mem(c, g, Ss::Sub, 1, c.esi.wrapping_add(0x44))?;
                    sse::movaps(c, 3, 2);
                    sse::ss_reg(c, Ss::Mul, 3, 2);
                    sse::movaps(c, 2, 0);
                    sse::ss_reg(c, Ss::Mul, 2, 0);
                    sse::movaps(c, 0, 1);
                    sse::ss_reg(c, Ss::Add, 3, 2);
                    sse::ss_reg(c, Ss::Mul, 0, 1);
                    sse::ss_reg(c, Ss::Add, 3, 0);
                    let threshold = g.u32(MOVED)?;
                    sse::comiss(c, sse::lane0(c, 3), threshold);
                    let moved = !sse::below_or_equal(c);
                    let changed = moved || {
                        load(c, g, 0, c.esi.wrapping_add(0x4c))?;
                        let radius = ld(g, c.esp, 0x14)?;
                        !xform::ucomiss_equal_bits(c, 0, radius)
                    };
                    if !changed {
                        // `be1c4`: nothing moved.
                        c.edi = pop(c, g)?;
                        c.esi = pop(c, g)?;
                        c.ebp = pop(c, g)?;
                        c.eax = byte(c.eax, 0);
                        logic(c, 0, 0x80);
                        c.ebx = pop(c, g)?;
                        c.esp = arithmetic(c, c.esp, 0x14, false);
                        return Ok(Next::Ret { pop: 4 });
                    }
                    c.edx = ld(g, c.esp, 0x28)?;
                    push(c, g, c.edx)?;
                    return Ok(Next::Call { target: DISCONNECT, site: BOUNDS_SITES[1] });
                }
            }
            0xbe18e => {
                c.edx = byte(c.edx, g.u8(c.esp.wrapping_add(0x13))?);
                0xbe192
            }
            0xbe192 => {
                let in_partition = c.edx as u8;
                logic(c, u32::from(in_partition), 0x80);
                load(c, g, 0, c.esp.wrapping_add(0x14))?;
                c.eax = c.esi.wrapping_add(0x40);
                g.set_u32(c.eax, c.ebx)?;
                g.set_u32(c.eax.wrapping_add(4), c.ebp)?;
                g.set_u32(c.eax.wrapping_add(8), c.edi)?;
                store(c, g, c.esi.wrapping_add(0x4c), 0)?;
                if in_partition == 0 {
                    0xbe1b8
                } else {
                    c.ecx = ld(g, c.esp, 0x28)?;
                    push(c, g, 0)?;
                    push(c, g, c.ecx)?;
                    return Ok(Next::Call { target: CONNECT, site: BOUNDS_SITES[2] });
                }
            }
            0xbe1b8 => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.eax = byte(c.eax, 1);
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x14, false);
                return Ok(Next::Ret { pop: 4 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}

#[cfg(test)]
mod tests;
