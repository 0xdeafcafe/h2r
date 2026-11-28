//! The machine and crate levels' import handler `11bd60` (slot `+4c`;
//! docs/engine/object-update.md, "Slice 3 bodies").
//!
//! `11bd60 (object, name, value out, active out) -> al` answers through
//! the object's component (body `+b4`, `a0`-byte records in the array at
//! `[51e9b8]`): `1d3550 (record: esi, name: ecx, value: edx; active:
//! stack 0) -> al` (`ret 4`). `al` is 1 when it answered, else 0; with
//! no component it returns 0 and stores nothing.

use super::{byte, call, pop, push, ret, slot, unowned, OBJECTS};
use guest_layout::datum::layout;
use crate::sim_objects::{Cx, Next, Result};

const ENTRY: u32 = 0x0011_bd60;
/// The component array global; data at `+44`, `a0` bytes each.
const COMPONENTS: u32 = 0x0051_e9b8;
/// The component's import: `(record: esi, name: ecx, value: edx, active:
/// stack 0) -> al`, `ret 4`.
const COMPONENT_IMPORT: u32 = 0x001d_3550;

pub(in crate::sim_objects) const MACHINE_SITES: [u32; 1] = [0x11bdaf];

/// `11bd60 (object: stack 0, name: stack 1, value: stack 2, active:
/// stack 3) -> al`; `ret 10`.
pub(in crate::sim_objects) fn machine(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        ENTRY => {
            c.eax = slot(c, g, 4)?;
            c.ecx = g.u32(OBJECTS)?;
            c.edx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
            c.eax = (c.eax & 0xffff) * 3;
            c.eax = g.u32(c.edx.wrapping_add(c.eax * 4).wrapping_add(8))?;
            c.eax = g.u32(c.eax.wrapping_add(0xb4))?;
            push(c, g, c.ebx)?;
            c.ebx = byte(c.ebx, 0);
            if c.eax == u32::MAX {
                // `11bdb6`.
                c.eax = byte(c.eax, c.ebx as u8);
                c.ebx = pop(c, g)?;
                return ret(c, 0x10);
            }
            c.edx = slot(c, g, 0x10)?;
            push(c, g, c.esi)?;
            c.eax &= 0xffff;
            c.esi = c.eax * 5;
            c.eax = g.u32(COMPONENTS)?;
            c.ecx = g.u32(c.eax.wrapping_add(layout::DATA))?;
            c.esi <<= 5;
            c.esi = c.esi.wrapping_add(c.ecx);
            c.ecx = slot(c, g, 0x18)?;
            push(c, g, c.ecx)?;
            c.ecx = slot(c, g, 0x14)?;
            call(c, COMPONENT_IMPORT, MACHINE_SITES[0])
        }
        0x11bdaf => {
            let answered = c.eax as u8 != 0;
            c.esi = pop(c, g)?;
            c.eax = byte(c.eax, if answered { 1 } else { c.ebx as u8 });
            c.ebx = pop(c, g)?;
            ret(c, 0x10)
        }
        _ => Err(unowned(pc)),
    }
}
