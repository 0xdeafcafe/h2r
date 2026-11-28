//! An item spawner's place at start in the game
//! (docs/game/forge-map-variants.md, "Properties without a retail field";
//! the reading is docs/engine/notes/2026-11-27-forge-spawner-rows.md,
//! "Place at start").
//!
//! The round reset (`ENGINE_ROUND_RESET`) leaves each netgame item entry
//! with no object and a countdown of 1, so every entry spawns on its first
//! update. An entry held back has its countdown set to 0 after the reset:
//! with no object and no countdown, the update arms the respawn time, so
//! the item comes after its respawn time, scaled as any respawn is.
//!
//! [`TABLE`] holds the held-back entries of the loaded map. A Forge
//! session's `ForgeStep::PlaceAtStart` or the built map's Forge record
//! (`game::forge_record`) sets it; the game's `forge_place_at_start`
//! feature writes the countdowns after each round reset and empties the
//! table at `GAME_ACTIVATE`.
use std::collections::BTreeSet;
use std::sync::Mutex;

use guest_layout::{GuestMemory, MemFault};

use super::spare_clips::STATE_STRIDE;

/// The entry's countdown in its state (`+4`, an i16).
pub const COUNTDOWN: u32 = 4;
/// The countdown the reset leaves an entry with (`0x15ad86`).
pub const RESET_COUNTDOWN: u16 = 1;
/// The entries the state array has room for (`0x15acf4`).
pub const ENTRIES: u32 = 100;

/// The entries of the loaded map that are not placed at a round's start.
pub static TABLE: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());

/// Hold entry `index` back at a round's start, or place it.
pub fn set(index: u32, held: bool) {
    let mut t = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    if held {
        t.insert(index);
    } else {
        t.remove(&index);
    }
}

/// Whether entry `index` is held back.
pub fn held(index: u32) -> bool {
    TABLE.lock().unwrap_or_else(|e| e.into_inner()).contains(&index)
}

/// Forget every entry: a new map is starting.
pub fn clear() {
    TABLE.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// After a round reset: zero the countdown of each entry of `indices` in
/// the state array at `state` that the reset left to spawn (no object, a
/// countdown of 1). The entries written.
pub fn hold(
    m: &mut dyn GuestMemory,
    state: u32,
    indices: impl IntoIterator<Item = u32>,
) -> Result<Vec<u32>, MemFault> {
    let mut written = Vec::new();
    for index in indices.into_iter().filter(|&i| i < ENTRIES) {
        let entry = state + index * STATE_STRIDE;
        if m.u32(entry)? != u32::MAX || m.u16(entry + COUNTDOWN)? != RESET_COUNTDOWN {
            continue;
        }
        m.set_u16(entry + COUNTDOWN, 0)?;
        written.push(index);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn a_held_entry_left_to_spawn_has_its_countdown_zeroed_and_no_other() {
        const STATE: u32 = 0x0080_07dc;
        let mut g = FakeGuest::new();
        g.map(STATE, 4 * STATE_STRIDE);
        // As the reset leaves them: no object, a countdown of 1.
        for i in 0..4 {
            g.put_u32(STATE + i * STATE_STRIDE, u32::MAX)
                .put_u16(STATE + i * STATE_STRIDE + COUNTDOWN, 1);
        }
        // Entry 3 has an object out: not the reset's state.
        g.put_u32(STATE + 3 * STATE_STRIDE, 0x0002_0000);
        let written = hold(&mut g, STATE, [1, 3, 200]).unwrap();
        assert_eq!(written, [1]);
        let countdown = |g: &FakeGuest, i: u32| g.u16(STATE + i * STATE_STRIDE + COUNTDOWN).unwrap();
        assert_eq!([0, 1, 2, 3].map(|i| countdown(&g, i)), [1, 0, 1, 1]);
        // A second pass finds nothing left to write.
        assert!(hold(&mut g, STATE, [1]).unwrap().is_empty());
    }
}
