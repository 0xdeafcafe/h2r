//! A weapon spawner's spare clips in play (docs/game/forge-map-variants.md,
//! "Properties without a retail field"; the reading is
//! docs/engine/notes/2026-11-27-forge-spawner-rows.md).
//!
//! The clips are host state, by netgame item entry, in
//! `agent::forge::spare_clips::TABLE`, which the Forge session's world
//! (`ForgeStep::SpareClips`) or the built map's Forge record
//! (`crate::forge_record`) sets. This feature fills each weapon the spawner
//! makes: an observer after `NETGAME_ITEM_SPAWN` (`0x15aad0`, `ret 8`)
//! reads the state array and the entry's index from the arguments the
//! callee popped, and writes the weapon's reserve. Before `GAME_ACTIVATE`
//! it empties the table, since a new map's entries are not the last one's.
//!
//! The spawner runs only on the authority, so only its table matters. The
//! table follows the Forge session, which a snapshot does not rewind
//! either, so the feature is stateless to snapshots.
use agent::forge::spare_clips;
use guest_layout::GuestMemory;

/// The feature's id (`features.forge-spare-clips`).
pub const FEATURE: &str = "forge-spare-clips";

/// After `NETGAME_ITEM_SPAWN` returned with `esp` past its two arguments:
/// fill the weapon of the entry it made, when the entry has spare clips.
/// The reserve written, if any.
pub fn after_spawn(m: &mut dyn GuestMemory, esp: u32) -> Option<i16> {
    let index = m.u32(esp.wrapping_sub(4)).ok()?;
    let clips = spare_clips::get(index)?;
    let state = m.u32(esp.wrapping_sub(8)).ok()?;
    spare_clips::fill(m, state, index, clips).ok().flatten()
}

pub struct SpareClips;

impl hooks::Feature for SpareClips {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "A Forge spawner's spare clips")
            .doc("docs/game/forge-map-variants.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::points::{GAME_ACTIVATE, NETGAME_ITEM_SPAWN};
        b.raw().observe_before(&GAME_ACTIVATE, "forge spare clips: a new map", |_| {
            spare_clips::clear();
        });
        b.raw().observe_after(
            &NETGAME_ITEM_SPAWN,
            "forge spare clips: fill the new weapon",
            |o| {
                let mut mem = o.guest();
                after_spawn(&mut *mem, o.cpu.esp);
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

    #[test]
    fn the_hook_reads_the_popped_arguments_and_skips_an_entry_with_none() {
        // The stack the spawner returned on: state then index below `esp`.
        const ESP: u32 = 0x0010_0010;
        let mut g = FakeGuest::new();
        g.map(ESP - 8, 8).put_u32(ESP - 8, 0x0080_07dc).put_u32(ESP - 4, 41);
        // Entry 41 has no clips: nothing is read past the index.
        spare_clips::set(41, None);
        assert_eq!(after_spawn(&mut g, ESP), None);
        // With clips, the state it names is read; an unmapped one is no write.
        spare_clips::set(41, Some(2));
        assert_eq!(after_spawn(&mut g, ESP), None);
        spare_clips::set(41, None);
    }
}
