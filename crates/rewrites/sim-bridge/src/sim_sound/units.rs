//! A unit's dialogue line end over guest memory: `0x18c8c0`'s gate, the
//! unit test `0xbadc0` and the end `0xd0dc0` (docs/engine/sound-instances.md,
//! "Owners").
//!
//! | Data | Where |
//! |---|---|
//! | object headers | `[0x4e0300]`: count `+0x38`, header size `+0x24`, headers `+0x44`; a header's salt `+0`, type `+3`, object `+8` |
//! | dialogue on | `[0x4ed28c] + 0x29` |
//! | the unit's line | object `+0x33e`: the offset of its dialogue state; `+0x38` past it, the line's tag |

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::OBJECTS;

const NONE: u32 = u32::MAX;
/// `[0x4ed28c] + 0x29`: dialogue is on (`0x18c8c0`'s gate).
pub const DIALOGUE: u32 = 0x004e_d28c;
pub const DIALOGUE_ON: u32 = 0x29;
/// `0xbadc0`'s type mask for a unit: a biped (type 0) or a vehicle (type 1).
pub const UNIT_TYPES: u32 = 3;
/// `0xd0dc0`'s header stride, fixed in the code rather than read at `+0x24`.
const HEADER: u32 = 12;

/// `0xbadc0(eax = object, types) -> eax`: the object's data when `object`
/// names a live header whose type bit is in `types`, else 0.
pub fn object_of_types(m: &dyn GuestMemory, object: u32, types: u32) -> Result<u32, MemFault> {
    if object == NONE {
        return Ok(0);
    }
    let array = m.u32(OBJECTS.base)?;
    let index = object & 0xffff;
    if index as i32 >= m.u32(array.wrapping_add(0x38))? as i32 {
        return Ok(0);
    }
    let header = m.u32(array.wrapping_add(0x24))?.wrapping_mul(index).wrapping_add(m.u32(array.wrapping_add(0x44))?);
    let salt = m.u16(header)?;
    if salt == 0 || i32::from(salt as i16) != (object as i32) >> 16 {
        return Ok(0);
    }
    // `shl edx, cl` takes the type's low five bits.
    let bit = 1u32 << (u32::from(m.u8(header.wrapping_add(3))?) & 31);
    if types & bit == 0 {
        return Ok(0);
    }
    m.u32(header.wrapping_add(8))
}

/// `0xd0dc0(eax = unit, tag)`: the unit's line slot forgets `tag` when it
/// still holds it.
pub fn unit_line_ended(m: &mut dyn GuestMemory, unit: u32, tag: u32) -> Result<(), MemFault> {
    let headers = m.u32(m.u32(OBJECTS.base)?.wrapping_add(0x44))?;
    let object = m.u32(headers.wrapping_add((unit & 0xffff) * HEADER).wrapping_add(8))?;
    let offset = i32::from(m.u16(object.wrapping_add(0x33e))? as i16) as u32;
    let slot = object.wrapping_add(offset).wrapping_add(0x38);
    if m.u32(slot)? == tag {
        m.set_u32(slot, NONE)?;
    }
    Ok(())
}

/// `0x18c8c0` past its pool read: with dialogue on and `object` a unit, its
/// line `tag` ends. True when the end ran.
pub fn line_end(m: &mut dyn GuestMemory, object: u32, tag: u32) -> Result<bool, MemFault> {
    if m.u8(m.u32(DIALOGUE)?.wrapping_add(DIALOGUE_ON))? == 0 {
        return Ok(false);
    }
    if object_of_types(&*m, object, UNIT_TYPES)? == 0 {
        return Ok(false);
    }
    unit_line_ended(m, object, tag)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const ARRAY: u32 = 0x0060_0000;
    const HEADERS: u32 = 0x0060_1000;
    const OBJECT: u32 = 0x0060_2000;
    const DIALOGUE_AT: u32 = 0x0060_3000;
    const UNIT: u32 = 0x0007_0002;
    const TAG: u32 = 0xe100_0001;
    const STATE: u32 = 0x100;

    /// Object 2, salt 7, of `kind`, its line slot holding `TAG`.
    fn guest(kind: u8) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.put_u32(OBJECTS.base, ARRAY).put_u32(ARRAY + 0x38, 4).put_u32(ARRAY + 0x24, HEADER).put_u32(ARRAY + 0x44, HEADERS);
        let h = HEADERS + 2 * HEADER;
        g.put_u16(h, 7).put_u8(h + 3, kind).put_u32(h + 8, OBJECT);
        g.put_u16(OBJECT + 0x33e, STATE as u16).put_u32(OBJECT + STATE + 0x38, TAG);
        g.put_u32(DIALOGUE, DIALOGUE_AT).put_u8(DIALOGUE_AT + DIALOGUE_ON, 1);
        g
    }

    #[test]
    fn a_live_header_of_a_masked_type_answers_its_object() {
        let g = guest(1);
        assert_eq!(object_of_types(&g, UNIT, UNIT_TYPES), Ok(OBJECT));
        assert_eq!(object_of_types(&g, UNIT, 1 << 2), Ok(0), "type 1 is not in the mask");
        assert_eq!(object_of_types(&g, 0x0008_0002, UNIT_TYPES), Ok(0), "a stale salt");
        assert_eq!(object_of_types(&g, 0x0007_0004, UNIT_TYPES), Ok(0), "past the count");
        assert_eq!(object_of_types(&g, NONE, UNIT_TYPES), Ok(0));
    }

    #[test]
    fn a_line_end_forgets_the_tag_only_when_the_slot_holds_it() {
        let mut g = guest(0);
        assert_eq!(line_end(&mut g, UNIT, TAG + 1), Ok(true));
        assert_eq!(g.u32(OBJECT + STATE + 0x38), Ok(TAG), "another line's end keeps it");
        assert_eq!(line_end(&mut g, UNIT, TAG), Ok(true));
        assert_eq!(g.u32(OBJECT + STATE + 0x38), Ok(NONE));
    }

    #[test]
    fn a_line_end_needs_dialogue_on_and_a_unit() {
        let mut g = guest(0);
        g.put_u8(DIALOGUE_AT + DIALOGUE_ON, 0);
        assert_eq!(line_end(&mut g, UNIT, TAG), Ok(false));
        let mut g = guest(2);
        assert_eq!(line_end(&mut g, UNIT, TAG), Ok(false));
        assert_eq!(g.u32(OBJECT + STATE + 0x38), Ok(TAG));
    }
}
