//! The two contiguous campaign AI datum arrays allocated by 0x200930.
//! They occupy one fixed-size region of game state. Move their boundary
//! before 0x200b30 seeds either array for the new scenario.
use guest_layout::datum::{self, DatumArray, flags, layout};
use guest_layout::memory::{GuestMemory, MemFault};

const SQUADS: u32 = 0x51e9d8;
const GROUPS: u32 = 0x51e9dc;
const RETAIL: (usize, usize) = (335, 100);
const SQUAD: usize = 0x98;
const GROUP: usize = 0x38;

const fn bytes(n: usize, stride: usize) -> usize {
    layout::HEADER_SIZE as usize + n * stride + n.div_ceil(32) * 4
}
const TOTAL: usize = bytes(RETAIL.0, SQUAD) + bytes(RETAIL.1, GROUP);

/// Select capacities without growing or moving the surrounding game state.
pub fn partition(squads: usize, groups: usize) -> Option<(usize, usize)> {
    if squads <= RETAIL.0 && groups <= RETAIL.1 {
        return Some(RETAIL);
    }
    if squads > TOTAL / SQUAD || groups > RETAIL.1 {
        return None;
    }
    let groups = groups.max(1);
    (bytes(squads, SQUAD) + bytes(groups, GROUP) <= TOTAL).then_some((squads, groups))
}

fn invalid(addr: u32) -> MemFault {
    MemFault {
        addr,
        len: 4,
        write: false,
    }
}

/// Called before the original initializer. Preserve the guest allocator
/// and restore the standard capacities when loading a smaller scenario.
pub fn prepare(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let scenario = m.u32(guest_layout::points::SCENARIO.base)?;
    let needed = partition(
        m.u32(scenario + 0x160)? as usize,
        m.u32(scenario + 0x158)? as usize,
    )
    .ok_or_else(|| invalid(scenario + 0x160))?;
    let squad = m.u32(SQUADS)?;
    let group = m.u32(GROUPS)?;
    let old = (
        m.u32(squad + layout::MAXIMUM_COUNT)? as usize,
        m.u32(group + layout::MAXIMUM_COUNT)? as usize,
    );
    if needed == old {
        return Ok(());
    }
    if partition(old.0, old.1) != Some(old)
        || squad.checked_add(TOTAL as u32).is_none()
        || group != squad + bytes(old.0, SQUAD) as u32
    {
        return Err(invalid(GROUPS));
    }
    let moved = squad + bytes(needed.0, SQUAD) as u32;
    let mut arrays = [
        (squad, squad, needed.0, SQUAD, 0x4561f0, 0),
        (group, moved, needed.1, GROUP, 0x4561e4, 0),
    ];
    for (src, _, _, stride, _, alloc) in &mut arrays {
        if m.u32(*src + layout::SIGNATURE)? != datum::SIGNATURE
            || m.u32(*src + layout::ELEMENT_SIZE)? != *stride as u32
            || m.u8(*src + layout::FLAGS)? & flags::CONTIGUOUS == 0
        {
            return Err(invalid(*src));
        }
        *alloc = m.u32(*src + layout::ALLOCATOR)?;
    }
    for (_, dst, count, stride, name, alloc) in arrays {
        DatumArray::at(dst).initialize_contiguous(
            m,
            name,
            count as i32,
            stride as i32,
            0,
            alloc,
        )?;
        m.set_u8(dst + layout::FLAGS, flags::CONTIGUOUS)?;
    }
    m.set_u32(GROUPS, moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn repartition_and_restore_preserve_neighboring_game_state() {
        let mut m = FlatMemory::new(0x450000, 0x100000);
        let (s, sc) = (0x480000, 0x460000);
        let g = s + bytes(RETAIL.0, SQUAD) as u32;
        m.set_u32(guest_layout::points::SCENARIO.base, sc).unwrap();
        m.set_u32(SQUADS, s).unwrap();
        m.set_u32(GROUPS, g).unwrap();
        for (at, n, stride, name) in [(s, 335, SQUAD, 0x4561f0), (g, 100, GROUP, 0x4561e4)] {
            DatumArray::at(at)
                .initialize_contiguous(&mut m, name, n, stride as i32, 0, 45)
                .unwrap();
            m.set_u8(at + layout::FLAGS, flags::CONTIGUOUS).unwrap();
        }
        m.set_u32(s + TOTAL as u32, 0x12345678).unwrap();
        m.set_u32(sc + 0x160, 344).unwrap();
        m.set_u32(sc + 0x158, 33).unwrap();
        prepare(&mut m).unwrap();
        let moved = m.u32(GROUPS).unwrap();
        assert_eq!(moved, s + bytes(344, SQUAD) as u32);
        assert_eq!(m.u32(moved + layout::ALLOCATOR).unwrap(), 45);
        m.set_u32(sc + 0x160, 20).unwrap();
        prepare(&mut m).unwrap();
        assert_eq!(m.u32(GROUPS).unwrap(), g);
        assert_eq!(m.u32(g + layout::MAXIMUM_COUNT).unwrap(), 100);
        assert_eq!(m.u32(s + TOTAL as u32).unwrap(), 0x12345678);
        assert_eq!(partition(384, 33), None);
    }
}
