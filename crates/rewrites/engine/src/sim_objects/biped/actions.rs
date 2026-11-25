//! Shared biped and vehicle action callbacks (`e6830`). Each callback
//! can change the bitmap, including other actions in its own word.
//! The guest frame carries the action record and cursor across each call.

use super::*;

pub(in crate::sim_objects) const ACTIONS_SITES: [u32; 1] = [0x000e_6890];
pub(super) const DESCRIPTORS: u32 = 0x0046_77c8;
/// The signed immediate of the loop limit at `e68a8`.
pub(super) const ACTION_LIMIT: u32 = 0x000e_68aa;

pub(in crate::sim_objects) fn actions_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let mut index = if pc == points::UNIT_ACTIONS_UPDATE.base {
        push(c, g, c.ecx)?;
        c.eax = slot(c, g, 8)?;
        c.ecx = g.u32(points::OBJECTS.base)?;
        c.edx = g.u32(at(c.ecx, DATA))?;
        push(c, g, c.ebx)?;
        c.eax = body_at(g, c.edx, (c.eax & 0xffff) * 12)?;
        c.ebx = g.u16(at(c.eax, 0x346))? as i16 as i32 as u32;
        push(c, g, c.ebp)?;
        push(c, g, c.esi)?;
        c.ebx = at(c.eax, c.ebx);
        push(c, g, c.edi)?;
        g.set_u8(at(c.esp, 0x13), 0)?;
        0
    } else if pc == ACTIONS_SITES[0] {
        if c.eax as u8 != 0 {
            g.set_u8(at(c.esp, 0x13), 1)?;
        } else {
            let word = at(at(c.ebx, c.esi), 4);
            c.eax = g.u32(word)? & !c.edi;
            g.set_u32(word, c.eax)?;
        }
        c.ebp.wrapping_add(1)
    } else {
        return Err(unowned(pc));
    };

    let limit = g.u8(ACTION_LIMIT)? as i8 as i32;
    // The first iteration runs before the loop tests the limit.
    let end = if pc == points::UNIT_ACTIONS_UPDATE.base { limit.max(1) } else { limit };
    let end = end.max(0) as u32;
    let bitmap = at(c.ebx, 4);
    while index < end {
        let Some(active) = ::sim_objects::walk::next_live(index..end, |slot| {
            g.u32(at(bitmap, (slot >> 5) * 4))
        })? else {
            index = end;
            break;
        };
        index = active;
        let descriptor = g.u32(at(DESCRIPTORS, index.wrapping_mul(4)))?;
        c.eax = g.u32(at(descriptor, 4))?;
        if c.eax != 0 {
            logic(c, c.eax, 1 << 31);
            c.ebp = index;
            c.esi = (index >> 5) * 4;
            c.edi = 1 << (index & 31);
            c.ecx = slot(c, g, 0x18)?;
            push(c, g, c.ebp)?;
            push(c, g, c.ecx)?;
            return call(c.eax, ACTIONS_SITES[0]);
        }
        index += 1;
    }

    arithmetic(c, index, limit as u32, true);
    c.eax = byte(c.eax, g.u8(at(c.esp, 0x13))?);
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}
