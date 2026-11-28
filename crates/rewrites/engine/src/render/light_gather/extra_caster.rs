//! The extra caster, `0x00031c20` (definition bit 22): the owner's first
//! ancestor that is not attached (body `+0xaa` clear, or no parent) is
//! appended to the light's object list with the caster bit (`0x1000`)
//! and, for a shadowed light, the receiver bit (`0x2000`), through
//! `0x00163c80`.

use super::guest::{LIGHT_VIS, List, NONE, Row, at, objects, ptr};
use super::{Refusal, Result, Writes};
use guest_layout::memory::GuestMemory;

/// `0x00031c20`: AL the shadow byte, ECX the owner, stack 0 the
/// visibility struct (only the light's own is taken).
pub fn list(m: &dyn GuestMemory, shadow: u8, owner: u32, vis: u32) -> Result<Writes> {
    let mut out = Writes::new();
    if owner == NONE {
        return Ok(out);
    }
    if vis != LIGHT_VIS {
        return Err(Refusal::Unsupported);
    }
    let objects = objects(m)?;
    let mut handle = owner;
    for _ in 0..objects.end {
        let body = objects.body(m, handle)?;
        let parent = m.u32(at(body, 0x14)?)?;
        if m.u8(at(body, 0xaa)?)? == 0 || parent == NONE {
            let list = List::read(m, ptr(m, at(vis, 0x14)?)?)?;
            if list.has_room(list.count) {
                let mut rows = list.rows(m)?;
                let mut row = Row::plain(handle);
                row.flags = if shadow == 0 { 0x1000 } else { 0x3000 };
                rows.push(row);
                list.publish(m, &mut out, &rows)?;
            }
            return Ok(out);
        }
        handle = parent;
    }
    Err(Refusal::Layout)
}
