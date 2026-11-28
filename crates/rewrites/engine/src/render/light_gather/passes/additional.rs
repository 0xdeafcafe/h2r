//! The empty additional-caster pass (`0x00030ed0`). Each candidate looks
//! up its first matching object in the running visibility list. Only a
//! match with flag 0x1000 can reach the marker lookup, suffix generation
//! and layer draws. Otherwise the sole surviving store restores the
//! cursor the function saved on entry (docs/graphics/native-light-gather.md).

use guest_layout::memory::{GuestMemory, MemFault};
use crate::render::light_gather::guest::{List, at, ptr};
use crate::render::light_gather::{Refusal, Result};

const CURRENT_VIS: u32 = 0x004c_0b80;
const CANDIDATES: u32 = 0x004b_a074;
const COUNT: u32 = 0x004b_a134;
const CURSOR: u32 = 0x004c_19ac;
const STRIDE: u32 = 12;
const CAPACITY: u32 = (COUNT - CANDIDATES) / STRIDE;

/// A loop that generates no suffix and makes no draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Empty {
    cursor: u32,
}

impl Empty {
    /// At `0x0003114c` only AL is replaced with the rebuild byte, zero.
    pub fn eax(&self) -> u32 {
        self.cursor & !0xff
    }

    pub fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        m.set_u32(CURSOR, self.cursor)
    }
}

/// Plan against the current visibility list before any guest or device
/// write. Once a marker is needed, the original function owns the whole
/// loop, including the rebuild of the light's lists after the last suffix.
pub fn empty(m: &dyn GuestMemory) -> Result<Empty> {
    let vis = ptr(m, CURRENT_VIS)?;
    let objects = m.u32(at(vis, 0x14)?)?;
    let plan = Empty { cursor: m.u32(CURSOR)? };
    let count = m.i32(COUNT)?;
    if count <= 0 {
        return Ok(plan);
    }
    if count as u32 > CAPACITY {
        return Err(Refusal::Layout);
    }
    let list = List::read(m, objects)?;
    if list.count == 0 {
        return Ok(plan);
    }
    let handles = m.bytes(list.column(m, 0xc)?, 4 * list.count)?;
    for i in 0..count as u32 {
        let handle = m.u32(CANDIDATES + STRIDE * i)?.to_le_bytes();
        if let Some(row) = handles.chunks_exact(4).position(|word| word == handle) {
            let flags = list.column(m, 8)?;
            if m.u16(at(flags, 2 * row as u32)?)? & 0x1000 != 0 {
                return Err(Refusal::Unsupported);
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::FlatMemory;

    const VIS: u32 = 0x1000;
    const OBJECTS: u32 = 0x1200;
    const FLAGS: u32 = 0x2000;
    const HANDLES: u32 = 0x3000;

    #[test]
    fn only_the_first_match_in_the_current_list_can_start_a_suffix() {
        let mut m = FlatMemory::new(0, 0x500000);
        for (addr, word) in [
            (CURRENT_VIS, VIS),
            (VIS + 0x14, OBJECTS),
            (OBJECTS, 8),
            (OBJECTS + 4, 3),
            (OBJECTS + 8, FLAGS),
            (OBJECTS + 12, HANDLES),
            (HANDLES, 7),
            (HANDLES + 4, 7),
            (HANDLES + 8, 8),
            (COUNT, 2),
            (CURSOR, 0xabcd_1234),
            (CANDIDATES, 23),
            (CANDIDATES + STRIDE, 7),
        ] {
            m.set_u32(addr, word).unwrap();
        }
        m.set_u16(FLAGS + 2, 0x1000).unwrap();
        m.set_u16(FLAGS + 4, 0x1000).unwrap();
        let p = empty(&m).unwrap();
        assert_eq!(p.eax(), 0xabcd_1200);
        p.write(&mut m).unwrap();
        assert_eq!(m.u32(CURSOR).unwrap(), 0xabcd_1234);

        m.set_u32(CANDIDATES + STRIDE, 8).unwrap();
        assert_eq!(empty(&m), Err(Refusal::Unsupported));
        assert_eq!(m.u32(CURSOR).unwrap(), 0xabcd_1234);

        // A marked matching row beyond the live count belongs to a
        // previous gather of this shared visibility list.
        m.set_u16(OBJECTS + 4, 2).unwrap();
        assert_eq!(empty(&m), Ok(p));
        m.set_u32(COUNT, CAPACITY + 1).unwrap();
        assert_eq!(empty(&m), Err(Refusal::Layout));
    }
}
