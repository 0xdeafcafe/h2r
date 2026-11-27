//! `1774f0 (effect: eax)`: delete an effect (docs/engine/effects-parts.md,
//! "The per-effect bodies").
//!
//! Frame: `esi` (the handle), `ebx` (the array header `[4ea93c]`), `edi`
//! (the datum) pushed in that order. A handle of -1, an index past the
//! header's count (`+38`), a free slot or a stale salt returns at once.
//! Otherwise:
//!
//! 1. `177590` (`esi` the handle, `edx` the salt word sign-extended);
//! 2. with datum `+8` not -1: the attachment record's byte `+4` of the
//!    array `[4ed28c]` gets bit 1, and `+8` becomes -1;
//! 3. `1771a0(datum)`, `1773a0` (`eax` the handle), `177460(datum)`, each
//!    `ret 4` or `ret`;
//! 4. a tail jump to `16ba40` with `eax` the handle and `edx` the header.
//!
//! Flags: each transfer carries the last flag-setting instruction's. The
//! `and`, `imul` and `sar` before a `cmp` are not modelled: the `cmp`
//! overwrites all six before anything reads them.

use super::{at, unowned};
use crate::sim_objects::alu::{rmw8, test16};
use crate::sim_objects::{Cx, Next, Result};
use crate::sim_physics::{arithmetic, pop, push};

pub(crate) const ENTRY: u32 = 0x0017_74f0;
const ARRAY: u32 = 0x004e_a93c;
const ATTACHMENTS: u32 = 0x004e_d28c;
const NONE: u32 = u32::MAX;

const SALT_CHECK: u32 = 0x0017_7590;
const RELEASE: u32 = 0x0017_71a0;
const UNLINK: u32 = 0x0017_73a0;
const CLEAR: u32 = 0x0017_7460;
const FREE: u32 = 0x0016_ba40;

/// The return addresses of its calls.
pub(crate) const SITES: [u32; 4] = [0x0017_7535, 0x0017_7564, 0x0017_756b, 0x0017_7571];

/// `pop edi; pop ebx; pop esi; ret` from the depth `edi` was pushed at
/// (`skip` 0), `ebx` (1) or `esi` (2).
fn leave(cx: &mut Cx<'_>, skip: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if skip == 0 {
        c.edi = pop(c, g)?;
    }
    if skip <= 1 {
        c.ebx = pop(c, g)?;
    }
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}

pub(crate) fn delete(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    match pc {
        ENTRY => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            let v = c.esi;
            push(c, g, v)?;
            c.esi = c.eax;
            arithmetic(c, c.esi, NONE, true);
            if c.esi == NONE {
                return leave(cx, 2);
            }
            let v = c.ebx;
            push(c, g, v)?;
            c.ebx = g.u32(ARRAY)?;
            c.ecx = g.u32(at(c.ebx, 0x38))?;
            c.eax &= 0xffff;
            arithmetic(c, c.eax, c.ecx, true);
            if (c.eax as i32) >= (c.ecx as i32) {
                return leave(cx, 1);
            }
            c.edx = g.u32(at(c.ebx, 0x44))?;
            let v = c.edi;
            push(c, g, v)?;
            c.edi = g.u32(at(c.ebx, 0x24))?.wrapping_mul(c.eax);
            let word = u32::from(g.u16(c.edi.wrapping_add(c.edx))?);
            c.eax = (c.eax & 0xffff_0000) | word;
            c.edi = arithmetic(c, c.edi, c.edx, false);
            test16(c, word);
            if word == 0 {
                return leave(cx, 0);
            }
            c.ecx = ((c.esi as i32) >> 16) as u32;
            c.edx = word as u16 as i16 as i32 as u32;
            arithmetic(c, c.edx, c.ecx, true);
            if c.edx != c.ecx {
                return leave(cx, 0);
            }
            Ok(Next::Call { target: SALT_CHECK, site: SITES[0] })
        }
        0x0017_7535 => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            c.eax = g.u32(at(c.edi, 8))?;
            arithmetic(c, c.eax, NONE, true);
            if c.eax != NONE {
                c.ecx = g.u32(ATTACHMENTS)?;
                c.edx = g.u32(at(c.ecx, 0x44))?;
                c.eax &= 0xffff;
                c.eax = c.eax.wrapping_mul(3);
                let byte = c.edx.wrapping_add(c.eax.wrapping_mul(8)).wrapping_add(4);
                rmw8(c, g, byte, |b| b | 2)?;
                c.eax = byte;
                g.set_u32(at(c.edi, 8), NONE)?;
            }
            let v = c.edi;
            push(c, g, v)?;
            Ok(Next::Call { target: RELEASE, site: SITES[1] })
        }
        0x0017_7564 => {
            cx.cpu.eax = cx.cpu.esi;
            Ok(Next::Call { target: UNLINK, site: SITES[2] })
        }
        0x0017_756b => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            let v = c.edi;
            push(c, g, v)?;
            Ok(Next::Call { target: CLEAR, site: SITES[3] })
        }
        0x0017_7571 => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            c.edi = pop(c, g)?;
            c.edx = c.ebx;
            c.ebx = pop(c, g)?;
            c.eax = c.esi;
            c.esi = pop(c, g)?;
            Ok(Next::Tail { target: FREE })
        }
        _ => Err(unowned(pc)),
    }
}
