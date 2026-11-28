//! Campaign squad storage reserved by the retail game-state allocator.
//! The group array keeps its retail capacity and does not move between maps.

use guest_layout::datum::{self, DatumArray, flags, layout};
use guest_layout::memory::{GuestMemory, MemFault};

/// a50 has 384 live or script-addressed squads and two further maneuver targets.
pub const LIMIT: usize = 386;
const RETAIL_SQUADS: usize = 335;
const GROUPS: usize = 100;
const SQUAD_SIZE: usize = 0x98;
const GROUP_SIZE: usize = 0x38;
const SQUAD_ARRAY: u32 = 0x51e9d8;
const GROUP_ARRAY: u32 = 0x51e9dc;
const SQUAD_NAME: u32 = 0x4561f0;
const GROUP_NAME: u32 = 0x4561e4;

const fn bytes(count: usize, stride: usize) -> u32 {
    (layout::HEADER_SIZE as usize + count * stride + count.div_ceil(32) * 4) as u32
}

pub const SQUAD_BYTES: u32 = bytes(LIMIT, SQUAD_SIZE);
pub const GROUP_BYTES: u32 = bytes(GROUPS, GROUP_SIZE);
const ACTORS: u32 = 256;
/// Last heard weapon report, after the actor's focus timestamp.
pub const ACTOR_LAST_REPORT: u32 = 0x18;
const ACTOR_STATE_SIZE: u32 = ACTOR_LAST_REPORT + 4;
const ACTOR_STATE_BYTES: u32 = ACTORS * ACTOR_STATE_SIZE;
pub const GROUP_ALLOCATION_BYTES: u32 = GROUP_BYTES + ACTOR_STATE_BYTES;

pub fn holds(squads: usize, groups: usize) -> bool {
    squads <= LIMIT && groups <= GROUPS
}

fn invalid(addr: u32) -> MemFault {
    MemFault {
        addr,
        len: 4,
        write: false,
    }
}

fn initialize(
    m: &mut dyn GuestMemory,
    at: u32,
    name: u32,
    count: usize,
    size: usize,
    allocator: u32,
) -> Result<(), MemFault> {
    DatumArray::at(at).initialize_contiguous(m, name, count as i32, size as i32, 0, allocator)?;
    m.set_u8(at + layout::FLAGS, flags::CONTIGUOUS)
}

/// Publish the two actual arrays at the native addresses after allocation.
pub fn publish(
    m: &mut dyn GuestMemory,
    squad: u32,
    group: u32,
    allocator: u32,
) -> Result<(), MemFault> {
    if squad == 0 || group == 0 {
        return Err(invalid(SQUAD_ARRAY));
    }
    if squad.checked_add(SQUAD_BYTES) != Some(group) {
        return Err(invalid(GROUP_ARRAY));
    }
    initialize(m, squad, SQUAD_NAME, RETAIL_SQUADS, SQUAD_SIZE, allocator)?;
    initialize(m, group, GROUP_NAME, GROUPS, GROUP_SIZE, allocator)?;
    clear_actor_state(m, group)?;
    m.set_u32(SQUAD_ARRAY, squad)?;
    m.set_u32(GROUP_ARRAY, group)
}

/// Called between maps, at the same point the native initializer clears all
/// active slots. The larger physical reservation never changes address.
pub fn prepare(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let scenario = m.u32(guest_layout::points::SCENARIO.base)?;
    let squads = m.u32(scenario + 0x160)? as usize;
    let groups = m.u32(scenario + 0x158)? as usize;
    if !holds(squads, groups) {
        return Err(invalid(scenario + 0x160));
    }
    let count = squads.max(RETAIL_SQUADS);
    let squad = m.u32(SQUAD_ARRAY)?;
    let group = m.u32(GROUP_ARRAY)?;
    if squad.checked_add(SQUAD_BYTES) != Some(group)
        || m.u32(squad + layout::SIGNATURE)? != datum::SIGNATURE
        || m.u32(squad + layout::ELEMENT_SIZE)? != SQUAD_SIZE as u32
        || m.u8(squad + layout::FLAGS)? & flags::CONTIGUOUS == 0
    {
        return Err(invalid(SQUAD_ARRAY));
    }
    clear_actor_state(m, group)?;
    if m.u32(squad + layout::MAXIMUM_COUNT)? as usize == count {
        return Ok(());
    }
    let allocator = m.u32(squad + layout::ALLOCATOR)?;
    initialize(m, squad, SQUAD_NAME, count, SQUAD_SIZE, allocator)
}

fn clear_actor_state(m: &mut dyn GuestMemory, group: u32) -> Result<(), MemFault> {
    m.write(group + GROUP_BYTES, &[0; ACTOR_STATE_BYTES as usize])
}

/// Rewindable campaign state adjacent to the native group array. The caller
/// verifies the full actor handle before reusing a slot.
pub fn actor_state(m: &dyn GuestMemory, index: u16) -> Result<Option<u32>, MemFault> {
    let index = u32::from(index);
    if index >= ACTORS {
        return Ok(None);
    }
    let squad = m.u32(SQUAD_ARRAY)?;
    let group = m.u32(GROUP_ARRAY)?;
    Ok((squad != 0 && squad.checked_add(SQUAD_BYTES) == Some(group))
        .then(|| group + GROUP_BYTES + index * ACTOR_STATE_SIZE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::{DatumHandle, memory::FlatMemory};

    #[test]
    fn named_destinations_have_independent_native_records_and_keep_group_storage() {
        let mut m = FlatMemory::new(0x450000, 0x100000);
        let squad = 0x480000;
        let group = squad + SQUAD_BYTES;
        let scenario = 0x470000;
        m.set_u32(guest_layout::points::SCENARIO.base, scenario).unwrap();
        publish(&mut m, squad, group, 0x40000).unwrap();
        m.set_u32(group + GROUP_BYTES, 0x7856_3412).unwrap();
        m.set_u32(scenario + 0x160, LIMIT as u32).unwrap();
        m.set_u32(scenario + 0x158, 75).unwrap();
        prepare(&mut m).unwrap();
        let data = m.u32(squad + layout::DATA).unwrap();
        let bank = DatumArray::at(squad);
        m.set_u32(squad + layout::HIGH_WATER, LIMIT as u32).unwrap();
        for index in [344, 361, 384, 385] {
            let row = data + index * SQUAD_SIZE as u32;
            m.set_u16(row, 0x8000).unwrap();
            m.set_u32(row + 0x10, index).unwrap();
            let at = bank
                .try_get(&m, DatumHandle::new(0x8000, index))
                .unwrap()
                .unwrap();
            assert_eq!(at, row);
            assert_eq!(m.u32(at + 0x10).unwrap(), index);
        }
        m.set_u32(scenario + 0x160, 20).unwrap();
        prepare(&mut m).unwrap();
        assert_eq!(m.u32(squad + layout::MAXIMUM_COUNT).unwrap(), 335);
        assert_eq!(m.u32(GROUP_ARRAY).unwrap(), group);
        assert_eq!(m.u32(group + layout::MAXIMUM_COUNT).unwrap(), 100);
        assert_eq!(actor_state(&m, 0).unwrap(), Some(group + GROUP_BYTES));
        assert_eq!(m.u32(group + GROUP_BYTES).unwrap(), 0);
        assert_eq!(actor_state(&m, 255).unwrap(), Some(group + GROUP_ALLOCATION_BYTES - ACTOR_STATE_SIZE));
        assert_eq!(actor_state(&m, 256).unwrap(), None);
        assert!(!holds(LIMIT + 1, 75));
    }
}
