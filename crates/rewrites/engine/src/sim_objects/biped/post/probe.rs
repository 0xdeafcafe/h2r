//! The biped's ground probe `e30c0`, the post-update's second call.

use super::{NONE, above, and32, at, below_or_equal, call, cmp8, cmp32, flags8, inc32, store_ss, sx8, sx16};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_objects::alu;
use crate::sim_objects::biped::{fld_fistp, less};
use crate::sim_objects::frame::{set_slot, slot};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, byte, logic, pop, push};

/// `e30c0`'s locals.
const FRAME: u32 = 0xa0;

/// `.rdata` constants: f32 at `45dd30` and `45dbfc` (the probe's two
/// lengths along the down vector), `45dbd8` (zero), `43ff08`, `45dc0c`
/// (scales), `45dbbc` (a time in seconds).
const K_45DD30: u32 = 0x0045_dd30;
const K_45DBFC: u32 = 0x0045_dbfc;
const K_45DBD8: u32 = 0x0045_dbd8;
const K_43FF08: u32 = 0x0043_ff08;
const K_45DC0C: u32 = 0x0045_dc0c;
const K_45DBBC: u32 = 0x0045_dbbc;
/// Holds a pointer to the down vector.
const DOWN: u32 = 0x0046_87bc;
/// Holds the gravity record (`[[51e9c4]]` its first float).
const GRAVITY: u32 = 0x0051_e9c4;
/// The `imm32` of `cmp dword [eax+edi+7c], imm32` at `e314a` (an
/// animation name, a string ID).
const PROBE_NAME: u32 = 0x000e_314e;
/// The `imm32` of `push imm32` at `e3158` (a time in seconds).
const PROBE_SECONDS: u32 = 0x000e_3159;
/// The `imm32`s of the two `push imm32` at `e3236` and `e323d` (the
/// collision query's flags, the first when `ddf60` answers).
const PROBE_FLAGS_A: u32 = 0x000e_3237;
const PROBE_FLAGS_B: u32 = 0x000e_323e;

/// Callees that stay guest.
const X1E20B0: u32 = 0x001e_20b0;
const X114B60: u32 = 0x0011_4b60;

/// Every return address of `e30c0`'s calls.
pub(in crate::sim_objects) const PROBE_SITES: [u32; 8] =
    [0xe3162, 0xe3188, 0xe3221, 0xe3247, 0xe327f, 0xe3272, 0xe32a3, 0xe336f];

/// `e30c0 (object: ebx)`; `ret`. Frame: `sub esp, a0`, then `ebp`, `esi`,
/// `edi` pushed (`S` = that `esp`); `edi` the body, `ebx` the object,
/// `ebp` the matg's `+144` record. With `S`: `[S+c]` the position less
/// the down vector times `[45dd30]` (`esi` while the query runs), `[S+20]`
/// the down vector times `[45dbfc]`, `[S+2c]` the hit point, `[S+38]` the
/// root velocity, `[S+44]` the position again, `[S+50]` the query's out
/// record (`[S+74]` its word set to none first), `[S+18]` and `[S+1c]` the
/// `fistp` pair.
///
/// Not when body `+348` bit 7 is set, the state `+3dc` is not 1, `+134`
/// bit 9 or definition `+1f0` bit 2 is set, there is no actor (`+12c`), the
/// animation name is the image's, or the counter `+399` has not passed
/// the ticks of the image's seconds (`1469f0`, fused). Otherwise a
/// collision query `1697c0` down from the position (`b9dd0`, fused), its
/// flags by the surface test `ddf60` (fused); when it hits, the root
/// velocity (`ba1d0`, fused) and an actor's speed (`1e20b0`, or the
/// record's `+64`) decide whether the biped is stuck. The stuck counter
/// `+3ab` counts up to `fe` or is cleared; at a second's ticks, unless the
/// tick count at `+38c` is recent, it is stamped and `114b60` (`eax`
/// none, `edx` `e`, the object, `d`, 0) runs.
pub(in crate::sim_objects) fn ground_probe(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_GROUND_PROBE.base;
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                c.esp = arithmetic(c, c.esp, FRAME, true);
                push(c, g, c.ebp)?;
                c.eax = c.ebx;
                c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
                push(c, g, c.esi)?;
                push(c, g, c.edi)?;
                c.edi = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
                let al = alu::shr(c, g.u8(at(c.edi, 0x348))?.into(), 7, 8) as u8;
                c.eax = byte(c.eax, al);
                'probe: {
                    if flags8(c, al & 1) != 0 {
                        break 'probe;
                    }
                    let state = g.u8(at(c.edi, 0x3dc))?;
                    cmp8(c, state, 1);
                    if state != 1 {
                        break 'probe;
                    }
                    c.ecx = g.u32(at(c.edi, 0x134))?;
                    c.ecx = alu::shr(c, c.ecx, 9, 32);
                    if flags8(c, c.ecx as u8 & 1) != 0 {
                        break 'probe;
                    }
                    c.edx = g.u32(c.edi)?;
                    c.eax = g.u32(points::TAG_ENTRIES.base)?;
                    c.edx = and32(c, c.edx & 0xffff);
                    c.edx = alu::shl(c, c.edx, 4, 32);
                    c.ecx = g.u32(at(at(c.edx, c.eax), 8))?;
                    c.edx = g.u32(at(c.ecx, 0x1f0))?;
                    c.edx = alu::shr(c, c.edx, 2, 32);
                    if flags8(c, c.edx as u8 & 1) != 0 {
                        break 'probe;
                    }
                    let actor = g.u32(at(c.edi, 0x12c))?;
                    cmp32(c, actor, NONE);
                    if actor == NONE {
                        break 'probe;
                    }
                    c.eax = sx16(g.u16(at(c.edi, 0x12a))?);
                    let name = g.u32(at(c.eax.wrapping_add(c.edi), 0x7c))?;
                    let image = g.u32(PROBE_NAME)?;
                    cmp32(c, name, image);
                    if name == image {
                        break 'probe;
                    }
                    push(c, g, g.u32(PROBE_SECONDS)?)?;
                    return call(points::GAME_SECONDS_TO_TICKS.base, 0xe3162);
                }
                0xe32f5
            }
            0xe3162 => {
                c.ecx = sx8(g.u8(at(c.edi, 0x399))?);
                cmp32(c, c.ecx, c.eax);
                if alu::less_or_equal(c) {
                    0xe32f5
                } else {
                    c.edx = g.u32(points::GLOBALS_TAG.base)?;
                    c.ebp = g.u32(at(c.edx, 0x144))?;
                    c.esi = at(c.esp, 0xc);
                    c.eax = c.ebx;
                    return call(points::OBJECT_WORLD_POSITION.base, 0xe3188);
                }
            }
            0xe3188 => {
                // The start: the position less the down vector times
                // `[45dd30]`; the delta: the down vector times `[45dbfc]`.
                c.eax = g.u32(DOWN)?;
                sse::load(c, 0, g.u32(K_45DD30)?);
                sse::load(c, 1, g.u32(c.eax)?);
                sse::load(c, 2, slot(c, g, 0xc)?);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                sse::ss_reg(c, Ss::Sub, 2, 1);
                sse::load(c, 1, g.u32(at(c.eax, 4))?);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                store_ss(c, g, at(c.esp, 0xc), 2)?;
                sse::load(c, 2, slot(c, g, 0x10)?);
                sse::ss_reg(c, Ss::Sub, 2, 1);
                sse::load(c, 1, g.u32(at(c.eax, 8))?);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                sse::load(c, 0, slot(c, g, 0x14)?);
                sse::ss_reg(c, Ss::Sub, 0, 1);
                sse::load(c, 1, g.u32(c.eax)?);
                store_ss(c, g, at(c.esp, 0x14), 0)?;
                sse::load(c, 0, g.u32(K_45DBFC)?);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                store_ss(c, g, at(c.esp, 0x20), 1)?;
                sse::load(c, 1, g.u32(at(c.eax, 4))?);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                store_ss(c, g, at(c.esp, 0x24), 1)?;
                sse::load(c, 1, g.u32(at(c.eax, 8))?);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                c.eax = c.ebx;
                store_ss(c, g, at(c.esp, 0x10), 2)?;
                store_ss(c, g, at(c.esp, 0x28), 1)?;
                g.set_u16(at(c.esp, 0x74), 0xffff)?;
                return call(points::BIPED_GROUND_FLAG.base, 0xe3221);
            }
            0xe3221 => {
                let found = flags8(c, c.eax as u8) != 0;
                c.eax = at(c.esp, 0x50);
                push(c, g, c.eax)?;
                push(c, g, NONE)?;
                push(c, g, NONE)?;
                c.ecx = at(c.esp, 0x2c);
                c.edx = c.esi;
                push(c, g, c.ecx)?;
                push(c, g, c.edx)?;
                push(c, g, g.u32(if found { PROBE_FLAGS_A } else { PROBE_FLAGS_B })?)?;
                return call(points::SIM_COLLISION_QUERY.base, 0xe3247);
            }
            0xe3247 => {
                if flags8(c, c.eax as u8) == 0 {
                    0xe32df
                } else {
                    c.eax = slot(c, g, 0x58)?;
                    c.ecx = slot(c, g, 0x5c)?;
                    c.edx = slot(c, g, 0x60)?;
                    set_slot(c, g, 0x2c, c.eax)?;
                    c.esi = at(c.esp, 0x44);
                    c.eax = c.ebx;
                    set_slot(c, g, 0x30, c.ecx)?;
                    set_slot(c, g, 0x34, c.edx)?;
                    return call(points::OBJECT_WORLD_POSITION.base, 0xe3272);
                }
            }
            0xe3272 => {
                push(c, g, 0)?;
                c.ecx = at(c.esp, 0x3c);
                c.eax = c.ebx;
                return call(points::OBJECT_ROOT_VELOCITY.base, 0xe327f);
            }
            0xe327f => {
                sse::load(c, 1, slot(c, g, 0x40)?);
                let x1 = sse::lane0(c, 1);
                if above(c, x1, g.u32(K_45DBD8)?) {
                    0xe32f5
                } else {
                    c.eax = g.u32(at(c.edi, 0x12c))?;
                    cmp32(c, c.eax, NONE);
                    sse::load(c, 0, g.u32(at(c.ebp, 0x64))?);
                    if c.eax == NONE {
                        0xe32a3
                    } else {
                        return call(X1E20B0, 0xe32a3);
                    }
                }
            }
            0xe32a3 => {
                // Stuck when the speed squared exceeds the fall below the
                // hit point plus the root velocity's `z` squared.
                sse::load(c, 2, slot(c, g, 0x4c)?);
                sse::ss(c, Ss::Sub, 2, slot(c, g, 0x34)?);
                c.eax = g.u32(GRAVITY)?;
                sse::ss(c, Ss::Mul, 0, g.u32(K_43FF08)?);
                sse::ss(c, Ss::Mul, 2, g.u32(c.eax)?);
                sse::ss(c, Ss::Mul, 2, g.u32(K_45DC0C)?);
                sse::movaps(c, 3, 1);
                sse::ss_reg(c, Ss::Mul, 3, 1);
                sse::movaps(c, 1, 0);
                sse::ss_reg(c, Ss::Add, 2, 3);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                let (x1, x2) = (sse::lane0(c, 1), sse::lane0(c, 2));
                if above(c, x1, x2) { 0xe32f5 } else { 0xe32df }
            }
            0xe32df => {
                c.eax = u32::from(g.u8(at(c.edi, 0x3ab))?);
                c.eax = inc32(c, c.eax);
                cmp32(c, c.eax, 0xfe);
                if !alu::less_or_equal(c) {
                    c.eax = 0xfe;
                }
                0xe32f7
            }
            0xe32f5 => {
                c.eax = and32(c, 0);
                0xe32f7
            }
            0xe32f7 => {
                g.set_u8(at(c.edi, 0x3ab), c.eax as u8)?;
                c.eax = g.u32(points::GAME_TIME.base)?;
                c.ecx = sx16(g.u16(at(c.eax, 2))?);
                sse::cvtsi2ss(c, 0, c.ecx);
                store_ss(c, g, at(c.esp, 0x1c), 0)?;
                fld_fistp(c, g, at(c.esp, 0x1c), at(c.esp, 0x18), 0xe3314)?;
                c.edx = u32::from(g.u8(at(c.edi, 0x3ab))?);
                cmp32(c, c.edx, slot(c, g, 0x18)?);
                if less(c) {
                    0xe336f
                } else {
                    c.ecx = g.u32(at(c.edi, 0x38c))?;
                    c.edx = c.ecx.wrapping_sub(1);
                    logic(c, c.edx, 1 << 31);
                    let stamp = if c.edx != 0 {
                        true
                    } else {
                        c.edx = sx16(g.u16(at(c.eax, 2))?);
                        sse::cvtsi2ss(c, 0, c.edx);
                        sse::ss(c, Ss::Mul, 0, g.u32(K_45DBBC)?);
                        sse::cvtsi2ss(c, 1, c.ecx);
                        sse::ss_reg(c, Ss::Add, 0, 1);
                        sse::cvtsi2ss(c, 1, g.u32(at(c.eax, 8))?);
                        let (x0, x1) = (sse::lane0(c, 0), sse::lane0(c, 1));
                        !below_or_equal(c, x1, x0)
                    };
                    if !stamp {
                        0xe336f
                    } else {
                        // `e3354`.
                        c.eax = g.u32(at(c.eax, 8))?;
                        push(c, g, 0)?;
                        push(c, g, 0xd)?;
                        g.set_u32(at(c.edi, 0x38c), c.eax)?;
                        push(c, g, c.ebx)?;
                        c.eax = NONE;
                        logic(c, NONE, 1 << 31);
                        c.edx = 0xe;
                        return call(X114B60, 0xe336f);
                    }
                }
            }
            0xe336f => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, FRAME, false);
                return Ok(Next::Ret { pop: 0 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}
