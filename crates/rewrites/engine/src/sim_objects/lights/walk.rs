//! The tick's light walk `c1400` (`GAME_TICK` calls it after the effects
//! update): no arguments, `ret`.
//!
//! **Frame.** `sub esp, c`, then `ebx`, `esi`; `ebp`, `edi` once a first
//! light exists. With `T` the `esp` after the four pushes: `T+10` the
//! current light's index, `T+14` the tick read at entry (game time `+8`),
//! `T+18` the light's definition. `ebx` holds the light's handle and
//! `esi` its element across every call.
//!
//! **Order.** The first live light comes from the datum iterator
//! `16bc00(array: ecx, start: edx = 0) -> eax` (`ret`); each later one
//! from an inline scan of the live bitmap from the index after the
//! current one, re-reading the array header, its high-water mark and its
//! bitmap pointer after each light, so a light created or deleted by a
//! callee is seen as retail sees it. Per light:
//!
//! 1. `c0c80(handle)`, the record update (a slice 5 body).
//! 2. A linked light (flags bit 3) whose owner, or else marker parent, is
//!    a live object with POST set (header bit 2): if it is active (bit 1),
//!    `1cae40(handle, element +10)` with `edi` the light lists, then clear
//!    bit 3; then `c2d00(handle)` relinks it.
//! 3. A timed light (bit 4) older than its definition's lifetime
//!    (`(tick − element +14) > ticks per second × definition +b0`,
//!    `comiss`) expires: `c3260(handle, 1)`.

use super::{BITMAP, DATA, ELEMENT_SIZE, HIGH_WATER, LIGHT_LISTS, LIGHTS, STRIDE, at, flags_undefined};
use guest_layout::points;
use crate::sim_objects::{Cx, Next, Result, sse, unowned};
use crate::sim_physics::{byte, pop, push};
use runtime::Guest;

const NONE: u32 = u32::MAX;
/// The datum array iterator.
const NEXT_LIVE: u32 = 0x0016_bc00;
/// Remove a light from the lists.
const LIST_REMOVE: u32 = 0x001c_ae40;
/// The record update, the link and the expiry (slice 5 bodies).
const RECORD: u32 = 0x000c_0c80;
const LINK: u32 = 0x000c_2d00;
const EXPIRE: u32 = 0x000c_3260;

/// After `16bc00`, `c0c80`, `1cae40`, `c2d00` and `c3260`.
pub(in crate::sim_objects) const WALK_SITES: [u32; 5] = [0xc1420, 0xc1489, 0xc14de, 0xc14e8, 0xc1526];

/// `movsx ebx, word ptr [element]; shl ebx, 10; or ebx, index`: the
/// handle of the element at `index`.
fn handle_of(g: &Guest, array: u32, index: u32) -> Result<u32> {
    let size = g.u32(at(array, ELEMENT_SIZE))?;
    let data = g.u32(at(array, DATA))?;
    let salt = g.u16(at(size.wrapping_mul(index), data))? as i16 as i32 as u32;
    Ok((salt << 16) | index)
}

/// `c1400`; `ret`.
pub(in crate::sim_objects) fn walk(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::LIGHTS_UPDATE.base;
    if pc != entry && !WALK_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut next = pc;
    loop {
        next = match next {
            _ if next == entry => {
                c.esp = c.esp.wrapping_sub(0xc);
                c.eax = g.u32(points::GAME_TIME.base)?;
                c.ecx = g.u32(at(c.eax, 8))?;
                push(c, g, c.ebx)?;
                push(c, g, c.esi)?;
                c.esi = g.u32(LIGHTS)?;
                g.set_u32(at(c.esp, 0xc), c.ecx)?;
                c.edx = 0;
                c.ecx = c.esi;
                flags_undefined(c);
                return Ok(Next::Call { target: NEXT_LIVE, site: 0xc1420 });
            }
            0xc1420 => {
                c.ebx = NONE;
                if c.eax != NONE {
                    c.edx = g.u32(at(c.esi, ELEMENT_SIZE))?;
                    c.ecx = g.u32(at(c.esi, DATA))?;
                    c.edx = c.edx.wrapping_mul(c.eax);
                    c.ebx = handle_of(g, c.esi, c.eax)?;
                }
                if c.ebx == NONE {
                    0xc1596
                } else {
                    push(c, g, c.ebp)?;
                    push(c, g, c.edi)?;
                    0xc1450
                }
            }
            0xc1447 => {
                c.esi = g.u32(LIGHTS)?;
                0xc1450
            }
            0xc1450 => {
                c.esi = g.u32(at(c.esi, DATA))?;
                c.eax = g.u32(points::TAG_ENTRIES.base)?;
                c.ebp = c.ebx & 0xffff;
                c.edi = c.ebp * STRIDE;
                c.edx = g.u32(at(at(c.esi, c.edi), 4))?;
                c.esi = at(c.esi, c.edi);
                c.edx = (c.edx & 0xffff) << 4;
                c.ecx = g.u32(at(at(c.edx, c.eax), 8))?;
                push(c, g, c.ebx)?;
                g.set_u32(at(c.esp, 0x14), c.ebp)?;
                g.set_u32(at(c.esp, 0x1c), c.ecx)?;
                flags_undefined(c);
                return Ok(Next::Call { target: RECORD, site: 0xc1489 });
            }
            0xc1489 => {
                if g.u8(at(c.esi, 2))? & 8 == 0 {
                    0xc14ec
                } else {
                    c.eax = g.u32(at(c.esi, 0x4c))?;
                    let parent = c.eax != NONE || {
                        c.eax = g.u32(at(c.esi, 0x58))?;
                        c.eax != NONE
                    };
                    if !parent {
                        0xc14ec
                    } else {
                        c.eax &= 0xffff;
                        c.edx = c.eax * 3;
                        c.eax = g.u32(points::OBJECTS.base)?;
                        c.ecx = g.u32(at(c.eax, DATA))?;
                        let header = g.u8(at(c.ecx, c.edx * 4 + 2))?;
                        c.eax = byte(c.eax, (header >> 2) & 1);
                        if c.eax & 0xff == 0 {
                            0xc14ec
                        } else {
                            c.edx = g.u32(LIGHTS)?;
                            c.ebp = g.u32(at(c.edx, DATA))?;
                            let flags = g.u8(at(at(c.ebp, c.edi), 2))?;
                            c.eax = byte(c.eax, flags);
                            c.ebp = at(c.ebp, c.edi);
                            if flags & 2 == 0 {
                                0xc14e2
                            } else {
                                c.eax = at(c.ebp, 0x10);
                                push(c, g, c.eax)?;
                                push(c, g, c.ebx)?;
                                c.edi = LIGHT_LISTS;
                                flags_undefined(c);
                                return Ok(Next::Call { target: LIST_REMOVE, site: 0xc14de });
                            }
                        }
                    }
                }
            }
            0xc14de => {
                let a = at(c.ebp, 2);
                let v = g.u8(a)? & 0xf7;
                g.set_u8(a, v)?;
                0xc14e2
            }
            0xc14e2 => {
                push(c, g, c.ebx)?;
                flags_undefined(c);
                return Ok(Next::Call { target: LINK, site: 0xc14e8 });
            }
            0xc14e8 => {
                c.ebp = g.u32(at(c.esp, 0x10))?;
                0xc14ec
            }
            0xc14ec => {
                if g.u8(at(c.esi, 2))? & 0x10 == 0 {
                    0xc1526
                } else {
                    c.eax = g.u32(at(c.esi, 0x14))?;
                    c.ecx = g.u32(at(c.esp, 0x14))?;
                    c.edx = g.u32(points::GAME_TIME.base)?;
                    c.ecx = c.ecx.wrapping_sub(c.eax);
                    c.eax = g.u16(at(c.edx, 2))? as i16 as i32 as u32;
                    sse::cvtsi2ss(c, 0, c.ecx);
                    c.ecx = g.u32(at(c.esp, 0x18))?;
                    sse::cvtsi2ss(c, 1, c.eax);
                    sse::ss(c, sse::Ss::Mul, 1, g.u32(at(c.ecx, 0xb0))?);
                    sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
                    if sse::below_or_equal(c) {
                        0xc1526
                    } else {
                        push(c, g, 1)?;
                        push(c, g, c.ebx)?;
                        flags_undefined(c);
                        return Ok(Next::Call { target: EXPIRE, site: 0xc1526 });
                    }
                }
            }
            0xc1526 => {
                c.eax = if c.ebx == NONE { 0 } else { c.ebp.wrapping_add(1) };
                c.ebx = g.u32(LIGHTS)?;
                c.esi = NONE;
                if (c.eax as i32) >= 0 {
                    c.edi = g.u32(at(c.ebx, HIGH_WATER))?;
                    if (c.eax as i32) < (c.edi as i32) {
                        c.edx = g.u32(at(c.ebx, BITMAP))?;
                        loop {
                            c.ecx = c.eax & 0x1f;
                            c.ebp = 1 << c.ecx;
                            c.ecx = (c.eax as i32 >> 5) as u32;
                            if g.u32(at(c.edx, c.ecx.wrapping_mul(4)))? & c.ebp != 0 {
                                c.esi = c.eax;
                                break;
                            }
                            c.eax = c.eax.wrapping_add(1);
                            if (c.eax as i32) >= (c.edi as i32) {
                                break;
                            }
                        }
                    }
                }
                c.eax = NONE;
                if c.esi != NONE {
                    c.edx = g.u32(at(c.ebx, ELEMENT_SIZE))?.wrapping_mul(c.esi);
                    c.eax = handle_of(g, c.ebx, c.esi)?;
                }
                c.ebx = c.eax;
                if c.eax != NONE {
                    0xc1447
                } else {
                    c.edi = pop(c, g)?;
                    c.ebp = pop(c, g)?;
                    0xc1596
                }
            }
            0xc1596 => {
                c.esi = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = c.esp.wrapping_add(0xc);
                flags_undefined(c);
                return Ok(Next::Ret { pop: 0 });
            }
            _ => return Err(unowned(next)),
        };
    }
}
