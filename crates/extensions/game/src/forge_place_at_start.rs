//! An item spawner's place at start in play (docs/game/forge-map-variants.md,
//! "Properties without a retail field"; the reading is
//! docs/engine/notes/2026-11-27-forge-spawner-rows.md, "Place at start").
//!
//! The held-back entries are host state in
//! `agent::forge::place_at_start::TABLE`, which the built map's Forge record
//! (`crate::forge_record`) or a Forge session's world sets. An observer
//! after `ENGINE_ROUND_RESET` zeroes each held entry's countdown the reset
//! left at 1, so the spawner arms the respawn time instead of spawning on
//! its first update. The first round's reset runs inside `GAME_ACTIVATE`,
//! after the record is read before it. Before `GAME_ACTIVATE` the table is
//! emptied, since a new map's entries are not the last one's.
//!
//! The reset and the spawner run on the authority. The table follows the
//! map and the session, which a snapshot does not rewind either, so the
//! feature is stateless to snapshots.
use agent::forge::{place_at_start, spare_clips};
use guest_layout::GuestMemory;

/// The feature's id (`features.forge-place-at-start`).
pub const FEATURE: &str = "forge-place-at-start";

/// After a round reset: hold back each entry the table holds. The entries
/// written.
pub fn after_reset(m: &mut dyn GuestMemory) -> Vec<u32> {
    let held: Vec<u32> = place_at_start::TABLE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .copied()
        .collect();
    if held.is_empty() {
        return Vec::new();
    }
    let Ok(Some(state)) = spare_clips::state(&*m) else {
        return Vec::new();
    };
    place_at_start::hold(m, state, held).unwrap_or_default()
}

pub struct PlaceAtStart;

impl hooks::Feature for PlaceAtStart {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "A Forge spawner's place at start")
            .doc("docs/game/forge-map-variants.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::points::GAME_ACTIVATE;
        b.raw().observe_before(&GAME_ACTIVATE, "forge place at start: a new map", |_| {
            place_at_start::clear();
        });
        b.raw().observe_after(
            &guest_layout::points::ENGINE_ROUND_RESET,
            "forge place at start: hold back",
            |o| {
                let mut mem = o.guest();
                after_reset(&mut *mem);
            },
        );
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;
    use guest_layout::global;

    #[test]
    fn a_round_reset_leaves_a_held_entry_to_its_respawn_time() {
        const ENGINE: u32 = 0x0080_0000;
        let state = ENGINE + spare_clips::STATE;
        let mut g = FakeGuest::new();
        g.put_u32(global::GAME_ENGINE.base, ENGINE)
            .map(state, 100 * spare_clips::STATE_STRIDE);
        // The reset left every entry to spawn on its first update.
        for i in 0..100 {
            let e = state + i * spare_clips::STATE_STRIDE;
            g.put_u32(e, u32::MAX).put_u16(e + place_at_start::COUNTDOWN, 1);
        }
        // The process's table: entries 87 and 88 are this test's alone.
        place_at_start::set(87, true);
        let written = after_reset(&mut g);
        assert!(written.contains(&87));
        let countdown = |i: u32| {
            g.u16(state + i * spare_clips::STATE_STRIDE + place_at_start::COUNTDOWN)
                .unwrap()
        };
        assert_eq!((countdown(87), countdown(88)), (0, 1));
        place_at_start::set(87, false);
    }
}
