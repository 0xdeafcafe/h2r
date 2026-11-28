//! A starting point's weight in the game (docs/game/forge-map-variants.md,
//! "Properties without a retail field"; the reading is
//! docs/engine/notes/2026-11-27-forge-spawner-rows.md).
//!
//! The spawn chooser (`PLAYER_SPAWN_LOCATION`) keeps the starting location
//! with the heaviest weight that passes the free-spot test. The weight
//! (`SPAWN_LOCATION_WEIGHT`) is the sum of influence spheres at the
//! location, each weighing about 0.05 to 1 (an enemy's -0.75 or -1), plus a
//! random term uniform over `[0, 1)` (the multiplayer globals' scale is
//! 1.0). Low and High add [`BIAS`] less or more: half the random term's
//! span, so High beats a location of equal influence seven times in eight,
//! and less than one enemy sphere, so a biased point next to an enemy still
//! loses to a clear one.
//!
//! [`TABLE`] holds the weight by starting location index for the loaded
//! map. A Forge session's `ForgeStep::SpawnWeight` or a built map's Forge
//! record (`game::forge_record`) sets it; the game's
//! `forge_spawn_weight` feature adds the bias after each weighing and
//! empties the table at `GAME_ACTIVATE`.
use std::collections::BTreeMap;
use std::sync::Mutex;

use guest_layout::{GuestMemory, MemFault};
use forge_doc::SpawnWeight;
use map::scenario_layout::{BlockKind, block};

/// What Low takes from a location's weight and High adds.
pub const BIAS: f32 = 0.5;

/// The weight of each starting location of the loaded map that is not
/// Normal.
pub static TABLE: Mutex<BTreeMap<u32, SpawnWeight>> = Mutex::new(BTreeMap::new());

/// Give starting location `index` a weight; none is Normal.
pub fn set(index: u32, weight: Option<SpawnWeight>) {
    let mut t = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    match weight {
        Some(w) => t.insert(index, w),
        None => t.remove(&index),
    };
}

/// Forget every location's weight: a new map is starting.
pub fn clear() {
    TABLE.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The bias of a weight.
pub fn bias(weight: SpawnWeight) -> f32 {
    match weight {
        SpawnWeight::Low => -BIAS,
        SpawnWeight::High => BIAS,
    }
}

/// The index of the starting location at guest address `location` in the
/// loaded scenario's block, or none when it is not one of its rows.
pub fn index_of(m: &dyn GuestMemory, location: u32) -> Result<Option<u32>, MemFault> {
    let scenario = m.u32(guest_layout::points::SCENARIO.base)?;
    if scenario == 0 {
        return Ok(None);
    }
    let def = block(BlockKind::StartingLocations);
    let count = m.u32(scenario + def.offset)?;
    let first = m.u32(scenario + def.offset + 4)?;
    let Some(off) = location.checked_sub(first) else {
        return Ok(None);
    };
    let index = off / def.stride;
    Ok((off % def.stride == 0 && index < count).then_some(index))
}

/// The bias to add to the weight of the location at `location`: zero for
/// a Normal one or an address that is no starting location.
pub fn bias_at(m: &dyn GuestMemory, location: u32) -> f32 {
    let Ok(Some(index)) = index_of(m, location) else {
        return 0.0;
    };
    TABLE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&index)
        .map_or(0.0, |&w| bias(w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const SCNR: u32 = 0x0070_0000;
    const STARTS: u32 = 0x0071_0000;

    #[test]
    fn a_location_is_found_by_its_row_and_takes_its_weights_bias() {
        let def = block(BlockKind::StartingLocations);
        let mut g = FakeGuest::new();
        g.put_u32(guest_layout::points::SCENARIO.base, SCNR)
            .map(SCNR + def.offset, 8)
            .put_u32(SCNR + def.offset, 3)
            .put_u32(SCNR + def.offset + 4, STARTS);
        let row = |i: u32| STARTS + i * def.stride;
        assert_eq!(index_of(&g, row(2)).unwrap(), Some(2));
        assert_eq!(index_of(&g, row(3)).unwrap(), None, "past the count");
        assert_eq!(index_of(&g, row(1) + 4).unwrap(), None, "inside a row");
        assert_eq!(index_of(&g, STARTS - def.stride).unwrap(), None);
        // The table is the process's; this test is its only writer.
        set(1, Some(SpawnWeight::High));
        set(2, Some(SpawnWeight::Low));
        assert_eq!(
            [bias_at(&g, row(0)), bias_at(&g, row(1)), bias_at(&g, row(2))],
            [0.0, BIAS, -BIAS]
        );
        set(2, None);
        assert_eq!(bias_at(&g, row(2)), 0.0);
        clear();
        assert_eq!(bias_at(&g, row(1)), 0.0);
    }
}
