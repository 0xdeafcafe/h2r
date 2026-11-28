//! HaloScript's syntax node array. Retail's `0x20a700` allocates its header
//! and an occupancy bitmap for `0x9000` slots in one `0x124c`-byte block
//! from the system heap. The nodes are the scenario's own syntax data
//! (`scnr+0x238`): at each map load `0x20a750` attaches them through
//! `0x16b6b0`, which takes the scenario's count as the capacity and sets or
//! clears one bitmap bit per slot. A converted scenario with more slots than
//! the retail bitmap covers gets a larger bitmap here first.
use guest_layout::datum::{self, flags, layout};
use guest_layout::{GuestMemory, MemFault, points};

/// Holds the syntax array header.
pub const NODES: u32 = 0x004f_9394;
/// Holds the loaded scenario's index; `0x20a750` attaches nothing at -1.
const SCENARIO_INDEX: u32 = 0x0046_86c0;
/// The scenario's syntax data: count, then address.
const SCENARIO_NODES: u32 = 0x238;
const NODE: u32 = 0x14;
/// Slots retail's bitmap covers (`push 9000h` at `0x20a722`).
pub const RETAIL_CAPACITY: u32 = 0x9000;
/// Most slots a scenario may hold with [`BITMAP_BYTES`]. Slot `0xffff`
/// is left out: with salt `0xffff` its datum would be none.
pub const CAPACITY: u32 = 0xffff;

const fn bitmap(n: u32) -> u32 {
    n.div_ceil(32) * 4
}

/// Bytes of the larger bitmap the caller reserves.
pub const BITMAP_BYTES: u32 = bitmap(CAPACITY);

fn invalid(addr: u32) -> MemFault {
    MemFault {
        addr,
        len: 4,
        write: false,
    }
}

/// Before `0x20a750` attaches the new scenario's nodes. A scenario within
/// [`RETAIL_CAPACITY`] keeps (or gets back) retail's bitmap after the
/// header; a larger one uses the bitmap at `extended`, [`BITMAP_BYTES`]
/// long. Only the bitmap pointer changes: the attach sets the capacity and
/// rebuilds occupancy from the salts, as it does for a retail map. The array
/// is not in game state, and nothing else keeps the bitmap's address.
pub fn prepare(m: &mut dyn GuestMemory, extended: u32) -> Result<(), MemFault> {
    let header = m.u32(NODES)?;
    let scenario = m.u32(points::SCENARIO.base)?;
    // The attach's own conditions: a scenario, and a detached array.
    if header == 0
        || scenario == 0
        || m.u32(SCENARIO_INDEX)? == u32::MAX
        || m.u8(header + layout::FLAGS)? & flags::BIT1 == 0
    {
        return Ok(());
    }
    let retail = header + layout::HEADER_SIZE;
    let current = m.u32(header + layout::BITMAP)?;
    if m.u32(header + layout::SIGNATURE)? != datum::SIGNATURE
        || m.u32(header + layout::ELEMENT_SIZE)? != NODE
        || (current != retail && current != extended)
    {
        return Err(invalid(header));
    }
    let count = m.u32(scenario + SCENARIO_NODES)?;
    let next = match count {
        0..=RETAIL_CAPACITY => retail,
        _ if count <= CAPACITY => extended,
        _ => return Err(invalid(scenario + SCENARIO_NODES)),
    };
    m.set_u32(header + layout::BITMAP, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::datum::DatumArray;
    use guest_layout::memory::FlatMemory;

    const HEADER: u32 = 0x500000;
    const EXTENDED: u32 = 0x520000;
    const SCENARIO: u32 = 0x540000;
    const DATA: u32 = 0x560000;
    const SENTINEL: u32 = 0xa11ce;

    /// The array as `0x20a700` leaves it, a scenario of `count` live nodes,
    /// and a word after each bitmap that an overrun would clobber.
    fn guest(count: u32) -> FlatMemory {
        let mut m = FlatMemory::new(0x400000, 0x300000);
        let bitmap = HEADER + layout::HEADER_SIZE;
        DatumArray::at(HEADER)
            .initialize(&mut m, 0x456228, RETAIL_CAPACITY as i32, NODE as i32, 0, 7, bitmap)
            .unwrap();
        m.set_u32(NODES, HEADER).unwrap();
        m.set_u32(points::SCENARIO.base, SCENARIO).unwrap();
        m.set_u32(SCENARIO_INDEX, 0).unwrap();
        m.set_u32(bitmap + super::bitmap(RETAIL_CAPACITY), SENTINEL).unwrap();
        m.set_u32(EXTENDED + BITMAP_BYTES, SENTINEL).unwrap();
        load(&mut m, count);
        m
    }

    fn load(m: &mut FlatMemory, count: u32) {
        m.set_u32(SCENARIO + SCENARIO_NODES, count).unwrap();
        m.set_u32(SCENARIO + SCENARIO_NODES + 4, DATA).unwrap();
        for i in 0..count {
            m.set_u16(DATA + i * NODE, 0x8000 | (i as u16 & 0x7fff)).unwrap();
        }
    }

    /// `0x16b6b0` on the scenario's nodes, then `0x20a7d0`'s detach.
    fn attach_and_unload(m: &mut FlatMemory, count: u32) {
        let nodes = DatumArray::at(HEADER);
        nodes.attach(m, count as i32, DATA).unwrap();
        let h = nodes.header(&*m).unwrap();
        assert_eq!((h.high_water, h.active_count), (count as i32, count as i32));
        let last = nodes.handle_of(&*m, count as i32 - 1).unwrap().unwrap();
        assert_eq!(nodes.try_get(&*m, last).unwrap(), Some(DATA + (count - 1) * NODE));
        let tail = nodes.next_index(&*m, count as i32 - 1).unwrap();
        assert_eq!(tail, Some(count as i32 - 1));
        for end in [HEADER + layout::HEADER_SIZE + bitmap(RETAIL_CAPACITY), EXTENDED + BITMAP_BYTES] {
            assert_eq!(m.u32(end).unwrap(), SENTINEL);
        }
        let fl = m.u8(HEADER + layout::FLAGS).unwrap();
        m.set_u8(HEADER + layout::FLAGS, fl | flags::BIT1).unwrap();
        m.set_u32(HEADER + layout::DATA, 0).unwrap();
    }

    #[test]
    fn a_scenario_past_the_retail_bitmap_attaches_on_the_larger_one() {
        let mut m = guest(48529);
        for count in [48529, 0x9000, CAPACITY, 120, 0x9001] {
            load(&mut m, count);
            prepare(&mut m, EXTENDED).unwrap();
            let expected = if count > RETAIL_CAPACITY { EXTENDED } else { HEADER + layout::HEADER_SIZE };
            assert_eq!(m.u32(HEADER + layout::BITMAP).unwrap(), expected);
            attach_and_unload(&mut m, count);
        }
    }

    #[test]
    fn the_retail_bitmap_alone_is_overrun_by_a_larger_scenario() {
        let mut m = guest(0x9001 + 32);
        DatumArray::at(HEADER).attach(&mut m, 0x9001 + 32, DATA).unwrap();
        assert_ne!(m.u32(HEADER + layout::HEADER_SIZE + bitmap(RETAIL_CAPACITY)).unwrap(), SENTINEL);
    }

    #[test]
    fn nothing_changes_when_the_engine_attaches_nothing() {
        let attached = |m: &mut FlatMemory| {
            let fl = m.u8(HEADER + layout::FLAGS).unwrap();
            m.set_u8(HEADER + layout::FLAGS, fl & !flags::BIT1).unwrap();
        };
        let none = |m: &mut FlatMemory| m.set_u32(SCENARIO_INDEX, u32::MAX).unwrap();
        let unloaded = |m: &mut FlatMemory| m.set_u32(points::SCENARIO.base, 0).unwrap();
        for skip in [attached as fn(&mut FlatMemory), none, unloaded] {
            let mut m = guest(0x10000);
            skip(&mut m);
            prepare(&mut m, EXTENDED).unwrap();
            assert_eq!(m.u32(HEADER + layout::BITMAP).unwrap(), HEADER + layout::HEADER_SIZE);
        }
    }

    #[test]
    fn a_scenario_past_the_capacity_or_an_unknown_bitmap_is_refused() {
        let mut m = guest(CAPACITY + 1);
        assert_eq!(prepare(&mut m, EXTENDED), Err(invalid(SCENARIO + SCENARIO_NODES)));
        let mut m = guest(10);
        m.set_u32(HEADER + layout::BITMAP, 0x530000).unwrap();
        assert_eq!(prepare(&mut m, EXTENDED), Err(invalid(HEADER)));
    }
}
