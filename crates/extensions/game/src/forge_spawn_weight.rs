//! A starting point's weight in play (docs/game/forge-map-variants.md,
//! "Properties without a retail field"; the reading is
//! docs/engine/notes/2026-11-27-forge-spawner-rows.md).
//!
//! The weights are host state, by starting location index, in
//! `agent::forge::spawn_weight::TABLE`, which the Forge session's world
//! (`ForgeStep::SpawnWeight`) or the built map's Forge record
//! (`crate::forge_record`) sets. `SPAWN_LOCATION_WEIGHT` (`0x23ba90`)
//! takes the location in EAX, which its return no longer holds: an
//! observer before keeps it, and a hook after adds the location's bias to
//! the weight in XMM0 that the chooser compares (`0x14efb6`). Before
//! `GAME_ACTIVATE` the table is emptied, since a new map's locations are
//! not the last one's.
//!
//! The chooser runs where players spawn, on the authority. The table
//! follows the Forge session, which a snapshot does not rewind either, so
//! the feature is stateless to snapshots.
use std::sync::Mutex;

use agent::forge::spawn_weight;
use guest_layout::GuestMemory;

/// The feature's id (`features.forge-spawn-weight`).
pub const FEATURE: &str = "forge-spawn-weight";

/// The location the weighing in progress was called with.
static LOCATION: Mutex<Option<u32>> = Mutex::new(None);

/// The weight `weight` of the location at `location`, with its bias.
pub fn biased(m: &dyn GuestMemory, location: u32, weight: f32) -> f32 {
    weight + spawn_weight::bias_at(m, location)
}

pub struct SpawnWeight;

impl hooks::Feature for SpawnWeight {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "A Forge starting point's weight")
            .doc("docs/game/forge-map-variants.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::points::{GAME_ACTIVATE, SPAWN_LOCATION_WEIGHT};
        b.raw().observe_before(&GAME_ACTIVATE, "forge spawn weight: a new map", |_| {
            spawn_weight::clear();
        });
        b.raw().observe_before(
            &SPAWN_LOCATION_WEIGHT,
            "forge spawn weight: the location",
            |o| {
                *LOCATION.lock().unwrap_or_else(|e| e.into_inner()) = Some(o.cpu.eax);
            },
        );
        b.raw().after(
            &SPAWN_LOCATION_WEIGHT,
            "forge spawn weight: the bias",
            |call| {
                let Some(location) = LOCATION.lock().unwrap_or_else(|e| e.into_inner()).take()
                else {
                    return Ok(());
                };
                let weight = f32::from_le_bytes(call.cpu.xmm[0][..4].try_into().unwrap());
                let out = {
                    let guest = call.ctx.runtime().memory();
                    biased(&*guest, location, weight)
                };
                if out != weight {
                    call.cpu.xmm[0][..4].copy_from_slice(&out.to_le_bytes());
                }
                Ok(())
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
    use forge_doc::SpawnWeight::High;
    use map::scenario_layout::{BlockKind, block};

    #[test]
    fn a_weighed_location_takes_its_bias_and_another_address_none() {
        const SCNR: u32 = 0x0070_0000;
        const STARTS: u32 = 0x0071_0000;
        let def = block(BlockKind::StartingLocations);
        let mut g = FakeGuest::new();
        g.put_u32(guest_layout::points::SCENARIO.base, SCNR)
            .map(SCNR + def.offset, 8)
            .put_u32(SCNR + def.offset, 2)
            .put_u32(SCNR + def.offset + 4, STARTS);
        spawn_weight::set(1, Some(High));
        assert_eq!(biased(&g, STARTS + def.stride, 0.25), 0.25 + spawn_weight::BIAS);
        assert_eq!(biased(&g, STARTS, 0.25), 0.25, "Normal");
        assert_eq!(biased(&g, 0x1234, 0.25), 0.25, "no location");
        spawn_weight::set(1, None);
    }
}
