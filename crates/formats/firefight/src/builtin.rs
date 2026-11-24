//! Authored wave sets for the Elite-only stage-one kit.
//!
//! The sets are the `rules` items of the `modes` package
//! (`content/modes/firefight/*.firefight.json`, docs/game/firefight.md,
//! "Wave sets as package items"); the game and the tools read the same
//! files the package carries.

use crate::{Def, MAX_TEAMS};

/// Points for killing a player of the other team in a built-in set played
/// by two teams: half an Elite.
pub const PLAYER_KILL_POINTS: u16 = 50;

/// The `modes` package's wave set files, in menu order: three for one
/// team, then Crossfire for two.
pub const SETS: [(&str, &str); 4] = [
    (
        "firefight/holdout.firefight.json",
        include_str!("../../../../content/modes/firefight/holdout.firefight.json"),
    ),
    (
        "firefight/attrition.firefight.json",
        include_str!("../../../../content/modes/firefight/attrition.firefight.json"),
    ),
    (
        "firefight/last-stand.firefight.json",
        include_str!("../../../../content/modes/firefight/last-stand.firefight.json"),
    ),
    (
        "firefight/crossfire.firefight.json",
        include_str!("../../../../content/modes/firefight/crossfire.firefight.json"),
    ),
];

/// The built-in wave sets, in menu order ([`SETS`]).
pub fn definitions() -> [Def; 4] {
    SETS.map(|(file, text)| {
        serde_json::from_str(text).unwrap_or_else(|e| panic!("{file} is not a Firefight set: {e}"))
    })
}

/// The built-in definition whose id ([`Def::id`]) starts with `prefix`:
/// what a mode-1 variant extension names (docs/game/firefight.md, "Live").
pub fn by_prefix(prefix: [u8; 8]) -> Option<Def> {
    definitions()
        .into_iter()
        .find(|d| d.id().is_ok_and(|id| id.0[..8] == prefix))
}

/// `def` played by `teams` player teams (1 or 2, clamped): with two, a
/// player kill scores the set's own points, or [`PLAYER_KILL_POINTS`]
/// when it has none; with one, nothing.
pub fn with_teams(mut def: Def, teams: u8) -> Def {
    def.teams = teams.clamp(1, MAX_TEAMS as u8);
    def.scoring.player_kill = match (def.teams, def.scoring.player_kill) {
        (1, _) => 0,
        (_, 0) => PLAYER_KILL_POINTS,
        (_, points) => points,
    };
    def
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Caps, Count, DifficultyMultipliers, Drop, FORMAT, Lives, Round, Scoring, Skull, SquadSpec,
        WAVES_PER_ROUND, Wave,
    };

    /// The sets written as Rust: the reference the package files are held to.
    fn reference() -> [Def; 4] {
        [
            Preset {
                name: "Holdout",
                actors: [12, 12, 12],
                squad_max: 5,
                gap_ticks: 600,
                pool: 12,
                bonus: 4,
                skulls: [Skull::Blind, Skull::Famine, Skull::Thunderstorm],
                teams: 1,
            },
            Preset {
                name: "Attrition",
                actors: [16, 16, 16],
                squad_max: 6,
                gap_ticks: 450,
                pool: 8,
                bonus: 3,
                skulls: [Skull::Catch, Skull::Angry, Skull::Mythic],
                teams: 1,
            },
            Preset {
                name: "Last Stand",
                actors: [24, 20, 16],
                squad_max: 8,
                gap_ticks: 300,
                pool: 5,
                bonus: 2,
                skulls: [Skull::Ghost, Skull::Thunderstorm, Skull::Assassins],
                teams: 1,
            },
            Preset {
                name: "Crossfire",
                actors: [24, 24, 16],
                squad_max: 6,
                gap_ticks: 450,
                pool: 10,
                bonus: 3,
                skulls: [Skull::Famine, Skull::Catch, Skull::Angry],
                teams: 2,
            },
        ]
        .map(Preset::definition)
    }

    #[test]
    fn the_package_files_are_the_sets_rust_built() {
        assert_eq!(definitions(), reference());
        for def in definitions() {
            def.canonical_json().unwrap();
        }
    }

    struct Preset {
        name: &'static str,
        actors: [u16; 3],
        squad_max: u16,
        gap_ticks: u32,
        pool: u16,
        bonus: u16,
        skulls: [Skull; 3],
        teams: u8,
    }

    impl Preset {
        fn definition(self) -> Def {
            let rounds = self
                .skulls
                .iter()
                .enumerate()
                .map(|(index, &skull)| {
                    let waves = (0..WAVES_PER_ROUND)
                        .map(|wave| {
                            let boss = wave + 1 == WAVES_PER_ROUND;
                            let mut squads =
                                vec![self.squad(index as u16 + 1, if boss { 2 } else { 1 })];
                            if wave >= 2 {
                                squads.push(self.squad(1, 1));
                            }
                            Wave {
                                squads,
                                boss,
                                gap_ticks: self.gap_ticks,
                            }
                        })
                        .collect();
                    Round {
                        waves,
                        skulls_on: vec![skull],
                    }
                })
                .collect();

            Def {
                format: FORMAT,
                name: self.name.into(),
                teams: self.teams,
                rounds,
                lives: Lives {
                    pool_start: self.pool,
                    per_round_bonus: self.bonus,
                    shared: true,
                },
                caps: Caps {
                    live_actors_by_players: [4, 8, 16].into_iter().zip(self.actors).collect(),
                },
                scoring: Scoring {
                    by_class: [("elite".into(), 100)].into(),
                    player_kill: if self.teams > 1 {
                        PLAYER_KILL_POINTS
                    } else {
                        0
                    },
                    difficulty_multiplier: DifficultyMultipliers {
                        easy: 750,
                        normal: 1_000,
                        heroic: 1_250,
                        legendary: 1_500,
                    },
                    skull_multipliers: self
                        .skulls
                        .into_iter()
                        .map(|skull| (skull, 1_100))
                        .collect(),
                    assist: 250,
                },
            }
        }

        fn squad(&self, min: u16, per_player: u8) -> SquadSpec {
            SquadSpec {
                class: "elite".into(),
                count: Count {
                    per_player,
                    min,
                    max: self.squad_max,
                },
                drop: Drop::Edge,
                weapon_tier: 0,
            }
        }
    }
}
