//! `spawn_areas` and `spawn_near` on the host (docs/game/rules-engine.md,
//! "The engine API"; the evidence is
//! docs/game/notes/2026-11-24-siege-spawns-and-bodies.md).
//!
//! The VM keeps each team's spawn rule in its state (`rules::vm::Spawns`).
//! The engine picks a spawn's starting location in
//! `PLAYER_SPAWN_LOCATION` (`0x14ef00`): it weighs every location of
//! `scnr+0x100` and keeps the heaviest that passes the free-spot test
//! `SPAWN_LOCATION_FREE` (`0x23bb70`). The feature reads the spawning
//! player's team rule as the chooser starts ([`filter`]) and answers the
//! free-spot test with "blocked" for every location outside it
//! ([`Filter::passes`]), so the chooser's own weighing picks among the
//! live locations and nothing else of the pick changes: the weights and
//! the random draws are the same ones. With every live location blocked
//! the chooser answers `0xffff`, and the engine retries the spawn each
//! tick, as it does on a full map.
//!
//! - `spawn_areas(team, label, n)`: the starting locations labelled
//!   `label` and numbered `n` ([`live`]). The host's census has no Forge
//!   document at a game's start, so a retail map's starting locations
//!   carry no label; the start gate refuses a script whose `spawns` label
//!   needs members the map lacks.
//! - `spawn_near(team, zone, n)`: the `n` starting locations nearest the
//!   zone's middle ([`super::zones::Shape::middle`]), nearest first, the
//!   lower index first on a tie.
//!
//! A rule whose locations the map does not have (a label with no member,
//! a zone with no shape) keeps no location, and the engine's own pick
//! stands for it ([`filter`] answers `None`).

use std::collections::BTreeSet;

use ::rules::vm::SpawnRule;
use engine::objectives::check_span;
use guest_layout::{GuestMemory, MemFault, points};
use forge_doc::{BlockKind, CENSUS_ENTRY_LIMIT};

use super::super::census::Table;
use super::zones::Zones;

/// A starting location's position (`+0x00`, three `f32`;
/// docs/editor/scenario-editor.md, "Player starting locations").
pub const START_POSITION: u32 = 0x00;

/// The loaded scenario's starting locations: the block's first entry and
/// stride, and each location's position in index order. No scenario reads
/// as none.
pub fn starts(m: &dyn GuestMemory) -> Result<Starts, MemFault> {
    let block = map::scenario_layout::block(BlockKind::StartingLocations);
    let s = m.u32(points::SCENARIO.base)?;
    if s == 0 {
        return Ok(Starts {
            base: 0,
            stride: block.stride,
            at: Vec::new(),
        });
    }
    check_span(s, block.offset + 8)?;
    let n = (m.i32(s + block.offset)?.max(0) as u32).min(CENSUS_ENTRY_LIMIT);
    let base = m.u32(s + block.offset + 4)?;
    check_span(base, n * block.stride)?;
    let mut at = Vec::with_capacity(n as usize);
    for i in 0..n {
        let p = base + i * block.stride + START_POSITION;
        at.push([m.f32(p)?, m.f32(p + 4)?, m.f32(p + 8)?]);
    }
    Ok(Starts {
        base,
        stride: block.stride,
        at,
    })
}

/// The scenario's starting locations, as [`starts`] reads them.
#[derive(Clone, Debug, PartialEq)]
pub struct Starts {
    /// The guest address of location 0 (`[scnr+0x104]`).
    pub base: u32,
    pub stride: u32,
    /// Each location's position, by index.
    pub at: Vec<[f32; 3]>,
}

/// The starting locations a rule keeps, by index: empty when the map has
/// none of them.
pub fn live(rule: SpawnRule<'_>, starts: &[[f32; 3]], table: &Table, zones: &Zones) -> BTreeSet<u32> {
    match rule {
        SpawnRule::Areas { label, number } => table
            .things()
            .iter()
            .filter(|t| {
                t.block == BlockKind::StartingLocations
                    && t.text == label
                    && t.number == Some(number)
                    && (t.index as usize) < starts.len()
            })
            .map(|t| t.index)
            .collect(),
        SpawnRule::Near { zone, count } => {
            let Some(mid) = zones.get(zone).map(|z| z.shape.middle()) else {
                return BTreeSet::new();
            };
            let mut by: Vec<(f32, u32)> = starts
                .iter()
                .zip(0u32..)
                .map(|(p, i)| {
                    let d = [p[0] - mid[0], p[1] - mid[1], p[2] - mid[2]];
                    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2], i)
                })
                .collect();
            by.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            by.into_iter().take(usize::from(count)).map(|(_, i)| i).collect()
        }
    }
}

/// The live starting locations of one spawn pick, as guest addresses go:
/// what [`Filter::passes`] answers the free-spot test from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub base: u32,
    pub stride: u32,
    pub live: BTreeSet<u32>,
}

impl Filter {
    /// Whether the location at guest address `location` may be picked. An
    /// address that is no entry of the block passes: the test is the
    /// engine's then.
    pub fn passes(&self, location: u32) -> bool {
        let off = location.wrapping_sub(self.base);
        if self.stride == 0 || off % self.stride != 0 {
            return true;
        }
        self.live.contains(&(off / self.stride))
    }
}

/// The filter for one pick: `rule` over the loaded map, or `None` when the
/// team has no rule or the rule keeps no location the map has.
pub fn filter(rule: Option<SpawnRule<'_>>, starts: &Starts, table: &Table, zones: &Zones) -> Option<Filter> {
    let live = live(rule?, &starts.at, table, zones);
    (!live.is_empty()).then_some(Filter {
        base: starts.base,
        stride: starts.stride,
        live,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::census::Thing;
    use ::rules::labels::Class;

    fn thing(text: &str, number: Option<i32>, block: BlockKind, index: u32) -> Thing {
        Thing {
            text: text.into(),
            label: None,
            number,
            class: if block == BlockKind::StartingLocations {
                Class::Spawn
            } else {
                Class::Flag
            },
            designator: None,
            block,
            index,
            area: None,
        }
    }

    /// Four starting locations along x, and one flag at x = 9.
    fn map() -> (Vec<[f32; 3]>, Table, Zones) {
        let starts = vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [8.0, 0.0, 0.0], [10.0, 0.0, 0.0]];
        let table = Table::from_things(vec![
            thing("territory", None, BlockKind::NetgameFlags, 0),
            thing("attack", Some(1), BlockKind::StartingLocations, 0),
            thing("attack", Some(1), BlockKind::StartingLocations, 2),
            thing("attack", Some(0), BlockKind::StartingLocations, 3),
            // A member past the block's end keeps nothing.
            thing("attack", Some(1), BlockKind::StartingLocations, 9),
        ]);
        let zones = Zones::new(&table, &[([9.0, 0.0, 0.0], 0.0)]);
        (starts, table, zones)
    }

    #[test]
    fn spawn_near_keeps_the_nearest_locations_to_the_zone() {
        let (starts, table, zones) = map();
        let flag = table.things()[0].handle();
        let near = |count| live(SpawnRule::Near { zone: flag, count }, &starts, &table, &zones);
        // 8 and 10 are each 1 from the flag: the lower index first.
        assert_eq!(near(1), BTreeSet::from([2]));
        assert_eq!(near(2), BTreeSet::from([2, 3]));
        assert_eq!(near(3), BTreeSet::from([1, 2, 3]));
        assert_eq!(near(255), BTreeSet::from([0, 1, 2, 3]));
        // A handle with no zone keeps nothing.
        assert!(live(SpawnRule::Near { zone: 7, count: 2 }, &starts, &table, &zones).is_empty());
    }

    #[test]
    fn spawn_areas_keeps_the_labelled_locations_of_the_number() {
        let (starts, table, zones) = map();
        let areas = |label, number| live(SpawnRule::Areas { label, number }, &starts, &table, &zones);
        assert_eq!(areas("attack", 1), BTreeSet::from([0, 2]));
        assert_eq!(areas("attack", 0), BTreeSet::from([3]));
        assert!(areas("attack", 2).is_empty());
        // A flag of the text is no starting location.
        assert!(areas("territory", 0).is_empty());
    }

    #[test]
    fn the_free_spot_test_blocks_every_other_location() {
        let (at, table, zones) = map();
        let starts = Starts {
            base: 0x1000,
            stride: 0x34,
            at,
        };
        let flag = table.things()[0].handle();
        let f = filter(Some(SpawnRule::Near { zone: flag, count: 2 }), &starts, &table, &zones).unwrap();
        let entry = |i: u32| 0x1000 + i * 0x34;
        assert!(!f.passes(entry(0)));
        assert!(!f.passes(entry(1)));
        assert!(f.passes(entry(2)));
        assert!(f.passes(entry(3)));
        // Not an entry of the block: the engine's own test.
        assert!(f.passes(0x1001));
        // No rule, or one the map has nothing for: the engine's own pick.
        assert_eq!(filter(None, &starts, &table, &zones), None);
        assert_eq!(
            filter(Some(SpawnRule::Areas { label: "defend", number: 0 }), &starts, &table, &zones),
            None
        );
    }

    /// A team whose live starting locations are all taken has no spawn
    /// spot: the filter blocks the rest and the engine's own free-spot
    /// test blocks the live ones, so the chooser answers `0xffff` and the
    /// engine retries the spawn each tick (the module's doc). The pick
    /// does not fall back to a start outside the rule.
    #[test]
    fn every_live_start_taken_leaves_the_engine_retrying() {
        let (at, table, zones) = map();
        let starts = Starts {
            base: 0x1000,
            stride: 0x34,
            at,
        };
        let f = filter(Some(SpawnRule::Areas { label: "attack", number: 1 }), &starts, &table, &zones).unwrap();
        assert_eq!(f.live, BTreeSet::from([0, 2]));
        let entry = |i: u32| 0x1000 + i * 0x34;
        // The engine's test says "taken" for the live locations.
        let engine_free = |i: u32| !f.live.contains(&i);
        let spots: Vec<u32> = (0..4).filter(|&i| f.passes(entry(i)) && engine_free(i)).collect();
        assert_eq!(spots, Vec::<u32>::new());
        // One live location freed, and it is the only spot.
        let spots: Vec<u32> = (0..4).filter(|&i| f.passes(entry(i)) && i == 2).collect();
        assert_eq!(spots, [2]);
    }
}
