//! Authored wave sets for the Elite-only stage-one kit.

use crate::{
    Caps, Count, Def, DifficultyMultipliers, Drop, FORMAT, Lives, Round, Scoring, Skull, SquadSpec,
    WAVES_PER_ROUND, Wave,
};

/// The three built-in wave sets, in menu order.
pub fn definitions() -> [Def; 3] {
    [
        Preset {
            name: "Holdout",
            actors: [12, 12, 12],
            squad_max: 5,
            gap_ticks: 600,
            pool: 12,
            bonus: 4,
            skulls: [Skull::Blind, Skull::Famine, Skull::Thunderstorm],
        },
        Preset {
            name: "Attrition",
            actors: [16, 16, 16],
            squad_max: 6,
            gap_ticks: 450,
            pool: 8,
            bonus: 3,
            skulls: [Skull::Catch, Skull::Angry, Skull::Mythic],
        },
        Preset {
            name: "Last Stand",
            actors: [24, 20, 16],
            squad_max: 8,
            gap_ticks: 300,
            pool: 5,
            bonus: 2,
            skulls: [Skull::Ghost, Skull::Thunderstorm, Skull::Assassins],
        },
    ]
    .map(Preset::definition)
}

struct Preset {
    name: &'static str,
    actors: [u16; 3],
    squad_max: u16,
    gap_ticks: u32,
    pool: u16,
    bonus: u16,
    skulls: [Skull; 3],
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
