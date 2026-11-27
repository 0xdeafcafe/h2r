//! Authored wave sets: the built-ins, and the map-specific sets
//! ([`MAP_SETS`]). The first five need only the authored Elite; Outbreak
//! and Endless's allies need classes the kit may have cut
//! ([`missing_classes`]).
//!
//! The sets are the `rules` items of the `modes` package
//! (`content/modes/firefight/*.firefight.json` and
//! `content/modes/firefight/maps/<map>/*.firefight.json`,
//! docs/game/firefight.md, "The definition" and "Map-specific sets");
//! the game and the tools read the same files the package carries.

use std::collections::BTreeSet;

use crate::{Def, MAX_TEAMS};

/// Points for killing a player of the other team in a built-in set played
/// by two teams: half an Elite.
pub const PLAYER_KILL_POINTS: u16 = 50;

/// The `modes` package's built-in wave set files, in store-value order:
/// three finite sets for one team, Crossfire for two, Endless (sets
/// of three rounds, one after another until the team is out, with Marine
/// allies), then Outbreak (Flood rounds, Covenant boss waves, Marine allies).
pub const SETS: [(&str, &str); 6] = [
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
    (
        "firefight/endless.firefight.json",
        include_str!("../../../../content/modes/firefight/endless.firefight.json"),
    ),
    (
        "firefight/outbreak.firefight.json",
        include_str!("../../../../content/modes/firefight/outbreak.firefight.json"),
    ),
];

/// The `modes` package's map-specific wave set files, as
/// `(base map's name, file, text)`, in map order then file order: the
/// sets tied to a kit map ([`map_sets`]), which the lobby's Wave set row
/// offers first on their map.
pub const MAP_SETS: [(&str, &str, &str); 1] = [(
    "beavercreek",
    "firefight/maps/beavercreek/basin.firefight.json",
    include_str!("../../../../content/modes/firefight/maps/beavercreek/basin.firefight.json"),
)];

/// The first store value a map set is named by, past the built-ins
/// ([`menu`], [`resolve`]).
pub const MAP_SET_BASE: i64 = SETS.len() as i64;

/// The built-in wave sets, in menu order ([`SETS`]).
pub fn definitions() -> [Def; 6] {
    SETS.map(|(file, text)| {
        serde_json::from_str(text).unwrap_or_else(|e| panic!("{file} is not a Firefight set: {e}"))
    })
}

/// The classes `def` needs the kit to carry besides the authored Elite:
/// every squad's and ally's class and every scored one, in name order.
pub fn required_classes(def: &Def) -> BTreeSet<String> {
    def.rounds
        .iter()
        .flat_map(|round| round.waves.iter().flat_map(|wave| &wave.squads).chain(&round.allies))
        .map(|squad| squad.class.clone())
        .chain(def.scoring.by_class.keys().cloned())
        .filter(|class| class != "elite")
        .collect()
}

/// The classes `def` needs that a kit with `have` lacks, in name order.
/// The lobby greys a set out with these named; the host refuses a set the
/// same way when it starts ([`crate::validate`]).
pub fn missing_classes(def: &Def, have: &BTreeSet<String>) -> Vec<String> {
    required_classes(def).into_iter().filter(|c| !have.contains(c)).collect()
}

/// `map`'s own wave sets, in file order: the sets the `modes` package
/// keeps under `firefight/maps/<map>/`. Empty when the map has none.
pub fn map_sets(map: &str) -> Vec<Def> {
    MAP_SETS
        .iter()
        .filter(|&&(own, _, _)| own == map)
        .map(|&(_, file, text)| {
            serde_json::from_str(text)
                .unwrap_or_else(|e| panic!("{file} is not a Firefight set: {e}"))
        })
        .collect()
}

/// Named game variants, including sets authored for a particular kit map.
/// The value is the settings value on that map, as accepted by `resolve`.
pub struct Variant {
    pub map: Option<&'static str>,
    pub value: usize,
    pub prefix: [u8; 8],
    pub def: Def,
}

pub fn variants() -> &'static [Variant] {
    static ALL: std::sync::OnceLock<Vec<Variant>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let generic = definitions().into_iter().enumerate().map(|(i, def)| (None, i, def));
        let specific = MAP_SETS.iter().enumerate().map(|(i, &(map, path, json))| {
            let offset = MAP_SETS[..i].iter().filter(|(other, _, _)| *other == map).count();
            let def = serde_json::from_str(json).unwrap_or_else(|e| panic!("{path}: {e}"));
            (Some(map), SETS.len() + offset, def)
        });
        generic.chain(specific).map(|(map, value, def)| Variant {
            map,
            value,
            prefix: def.id().expect("valid packaged wave set").0[..8].try_into().unwrap(),
            def,
        }).collect()
    })
}

pub fn variant(prefix: [u8; 8]) -> Option<&'static Variant> {
    variants().iter().find(|v| v.prefix == prefix)
}

/// The name of the Map Waves entry: the Firefight game variant that plays
/// the running map variant's own wave set (docs/game/forge-firefight.md,
/// "Playing it").
pub const MAP_WAVES_NAME: &str = "Map Waves";

/// The hash prefix of the Map Waves entry: the first eight bytes of the
/// SHA-256 of `firefight.map-waves`. It names no definition ([`variant`]
/// and [`by_prefix`] have none for it): the runtime resolves it to the
/// running map variant's set.
pub const MAP_WAVES: [u8; 8] = [0x82, 0x42, 0xf8, 0x47, 0x54, 0xfb, 0x3e, 0x5d];

/// The built-in definition whose id ([`Def::id`]) starts with `prefix`:
/// what a mode-1 variant extension names (docs/game/firefight.md, "Live").
pub fn by_prefix(prefix: [u8; 8]) -> Option<Def> {
    variant(prefix).map(|v| v.def.clone())
}

/// The lobby's Wave set row on `map` (`None`: no map known): `map`'s own
/// sets first, then the built-ins, each with the store value that names
/// it ([`resolve`]). A map with none of its own gets the built-ins alone,
/// as does any map the build has no sets for.
pub fn menu(map: Option<&str>) -> Vec<(i64, Def)> {
    map.map(map_sets)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(place, def)| (MAP_SET_BASE + place as i64, def))
        .chain(
            definitions()
                .into_iter()
                .enumerate()
                .map(|(place, def)| (place as i64, def)),
        )
        .collect()
}

/// What the Wave set store value `value` plays on `map`: the built-in at
/// `value`; `map`'s own set at [`MAP_SET_BASE`] plus its place in
/// [`map_sets`]. A map-set value on a map that does not carry it —
/// another map, or no map known — falls back to the built-ins (Holdout,
/// the first). `None` when no set the build ships names the value.
pub fn resolve(value: i64, map: Option<&str>) -> Option<Def> {
    let index = usize::try_from(value).ok();
    if let Some(place) = index.filter(|&index| index < SETS.len()) {
        return definitions().get(place).cloned();
    }
    let place = index
        .and_then(|index| index.checked_sub(MAP_SET_BASE as usize))
        .filter(|_| names_set(value))?;
    map_sets(map.unwrap_or(""))
        .get(place)
        .cloned()
        .or_else(|| definitions().into_iter().next())
}

/// Whether `value` names a set the build ships, on some map: what the
/// Wave set store value may hold ([`resolve`] refuses the rest).
pub fn names_set(value: i64) -> bool {
    let index = usize::try_from(value).ok();
    index.is_some_and(|index| index < SETS.len())
        || index
            .and_then(|index| index.checked_sub(MAP_SET_BASE as usize))
            .is_some_and(|place| place < MAP_SETS.len())
}

/// `def` played by `teams` player teams (1 or 2, clamped): with two, a
/// player kill scores the set's own points, or [`PLAYER_KILL_POINTS`]
/// when it has none; with one, nothing. An endless set has one team
/// ([`crate::validate`] refuses two), so it is returned as it is.
pub fn with_teams(mut def: Def, teams: u8) -> Def {
    if def.set.endless {
        return def;
    }
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
        Caps, Count, DifficultyMultipliers, Drop, FORMAT, Lives, Round, Scoring, SetRules, Skull,
        Side, SquadSpec, WAVES_PER_ROUND, Wave,
    };

    /// The sets written as Rust: the reference the package files are held to.
    fn reference() -> [Def; 6] {
        [
            Preset {
                name: "Holdout",
                actors: [12, 12, 12],
                squad_max: 5,
                gap_ticks: 600,
                pool: 12,
                bonus: 4,
                skulls: [Skull::Ghost, Skull::Famine, Skull::Thunderstorm],
                set: SetRules::finite(),
                teams: 1,
                allies: false,
            },
            Preset {
                name: "Attrition",
                actors: [16, 16, 16],
                squad_max: 6,
                gap_ticks: 450,
                pool: 8,
                bonus: 3,
                skulls: [Skull::Catch, Skull::Angry, Skull::Mythic],
                set: SetRules::finite(),
                teams: 1,
                allies: false,
            },
            Preset {
                name: "Last Stand",
                actors: [24, 20, 16],
                squad_max: 8,
                gap_ticks: 300,
                pool: 5,
                bonus: 2,
                skulls: [Skull::Ghost, Skull::Thunderstorm, Skull::Assassins],
                set: SetRules::finite(),
                teams: 1,
                allies: false,
            },
            Preset {
                name: "Crossfire",
                actors: [24, 24, 16],
                squad_max: 6,
                gap_ticks: 450,
                pool: 10,
                bonus: 3,
                skulls: [Skull::Famine, Skull::Catch, Skull::Angry],
                set: SetRules::finite(),
                teams: 2,
                allies: false,
            },
            Preset {
                name: "Endless",
                actors: [16, 16, 16],
                squad_max: 6,
                gap_ticks: 450,
                pool: 7,
                bonus: 1,
                skulls: [Skull::Ghost, Skull::Catch, Skull::BlackEye],
                set: SetRules {
                    endless: true,
                    scale: 100,
                    skulls: vec![Skull::Thunderstorm, Skull::Famine, Skull::Mythic],
                },
                teams: 1,
                allies: true,
            },
        ]
        .map(Preset::definition)
        .into_iter()
        .chain([outbreak()])
        .collect::<Vec<_>>()
        .try_into()
        .expect("six sets")
    }

    /// Outbreak written as Rust: three rounds of Flood waves, a Covenant
    /// boss wave to close each, and Marines on the players' side.
    fn outbreak() -> Def {
        let squad = |class: &str, per_player: u8, min: u16| SquadSpec {
            class: class.into(),
            count: Count { per_player, min, max: 4 },
            drop: Drop::Edge,
            weapon_tier: 0,
            side: Side::Enemy,
            arrival: crate::Arrival::Auto,
        };
        let skulls = [Skull::Famine, Skull::Catch, Skull::Angry];
        let rounds = skulls
            .iter()
            .enumerate()
            .map(|(i, &skull)| {
                let i = i as u16;
                let plans: [Vec<SquadSpec>; 5] = [
                    vec![squad("flood_combat_human", 1, i + 2)],
                    vec![squad("flood_combat_human", 1, i + 2), squad("flood_carrier", 1, 1)],
                    vec![squad("flood_combat_elite", 1, i + 1), squad("flood_carrier", 1, 2)],
                    vec![
                        squad("flood_combat_elite", 1, i + 2),
                        squad("flood_combat_human", 1, i + 2),
                        squad("flood_carrier", 1, 2),
                    ],
                    vec![squad("elite", 2, i + 1), squad("flood_combat_elite", 1, 1)],
                ];
                Round {
                    waves: plans
                        .into_iter()
                        .enumerate()
                        .map(|(wave, squads)| Wave {
                            squads,
                            boss: wave == 4,
                            gap_ticks: 450,
                        })
                        .collect(),
                    skulls_on: vec![skull],
                    allies: vec![SquadSpec {
                        class: "marine".into(),
                        count: Count { per_player: 1, min: 2, max: 3 },
                        side: Side::Ally,
                        ..squad("marine", 1, 2)
                    }],
                }
            })
            .collect();
        Def {
            format: FORMAT,
            name: "Outbreak".into(),
            teams: 1,
            rounds,
            set: SetRules::finite(),
            lives: Lives { pool_start: 10, per_round_bonus: 3, shared: true, zero_holds: false },
            caps: Caps { live_actors_by_players: [4, 8, 16].into_iter().zip([16, 16, 16]).collect() },
            scoring: Scoring {
                by_class: [
                    ("elite".into(), 100),
                    ("flood_combat_elite".into(), 100),
                    ("flood_combat_human".into(), 50),
                    ("flood_carrier".into(), 75),
                ]
                .into(),
                player_kill: 0,
                difficulty_multiplier: DifficultyMultipliers {
                    easy: 750,
                    normal: 1_000,
                    heroic: 1_250,
                    legendary: 1_500,
                },
                skull_multipliers: skulls.into_iter().map(|skull| (skull, 1_100)).collect(),
                assist: 250,
            },
            bonus: None,
        }
    }

    #[test]
    fn outbreak_needs_the_flood_and_marine_classes_and_endless_the_marine() {
        let [.., endless, outbreak] = definitions();
        let all = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<BTreeSet<_>>();
        assert_eq!(
            required_classes(&outbreak),
            all(&["flood_carrier", "flood_combat_elite", "flood_combat_human", "marine"])
        );
        assert_eq!(required_classes(&endless), all(&["marine"]));
        assert_eq!(missing_classes(&outbreak, &all(&["marine"])).len(), 3);
        assert!(missing_classes(&outbreak, &required_classes(&outbreak)).is_empty());
        // The five before them need the Elite alone, except Endless.
        for def in &definitions()[..4] {
            assert!(required_classes(def).is_empty(), "{}", def.name);
        }
    }

    #[test]
    fn the_package_files_are_the_sets_rust_built() {
        assert_eq!(definitions(), reference());
        for def in definitions() {
            def.canonical_json().unwrap();
        }
    }

    /// The map-specific sets written as Rust: the reference their files
    /// are held to ([`reference`]).
    fn map_reference() -> Vec<Def> {
        vec![Preset {
            name: "Basin",
            actors: [12, 12, 12],
            squad_max: 6,
            gap_ticks: 450,
            pool: 10,
            bonus: 3,
            skulls: [Skull::Catch, Skull::Angry, Skull::Ghost],
            set: SetRules::finite(),
            teams: 1,
            allies: false,
        }
        .definition()]
    }

    #[test]
    fn the_map_set_files_are_the_sets_rust_built() {
        let own: Vec<Def> = MAP_SETS
            .iter()
            .map(|&(_, file, text)| {
                serde_json::from_str(text)
                    .unwrap_or_else(|e| panic!("{file} is not a Firefight set: {e}"))
            })
            .collect();
        assert_eq!(own, map_reference());
        for def in &own {
            def.canonical_json().unwrap();
        }
        assert_eq!(map_sets("beavercreek"), map_reference());
        assert!(map_sets("coagulation").is_empty());
    }

    #[test]
    fn map_waves_is_the_hash_of_its_name_and_no_built_in() {
        assert_eq!(
            crate::Sha256::of(b"firefight.map-waves").0[..8],
            MAP_WAVES,
            "the prefix is the SHA-256 of the entry's key"
        );
        assert!(variant(MAP_WAVES).is_none());
        assert!(by_prefix(MAP_WAVES).is_none());
        assert!(variants().iter().all(|v| v.prefix != MAP_WAVES));
    }

    struct Preset {
        name: &'static str,
        actors: [u16; 3],
        squad_max: u16,
        gap_ticks: u32,
        pool: u16,
        bonus: u16,
        skulls: [Skull; 3],
        set: SetRules,
        teams: u8,
        /// A Marine ally squad in every round.
        allies: bool,
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
                        allies: if self.allies {
                            vec![SquadSpec {
                                side: Side::Ally,
                                ..self.ally()
                            }]
                        } else {
                            Vec::new()
                        },
                    }
                })
                .collect();

            Def {
                format: FORMAT,
                name: self.name.into(),
                teams: self.teams,
                rounds,
                set: self.set.clone(),
                lives: Lives {
                    pool_start: self.pool,
                    per_round_bonus: self.bonus,
                    shared: true,
                    zero_holds: false,
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
                        .chain(self.set.skulls.iter().copied())
                        .map(|skull| (skull, 1_100))
                        .collect(),
                    assist: 250,
                },
                bonus: None,
            }
        }

        /// One Marine at least, two at most, on the players' side.
        fn ally(&self) -> SquadSpec {
            SquadSpec {
                class: "marine".into(),
                count: Count { per_player: 1, min: 1, max: 2 },
                drop: Drop::Edge,
                weapon_tier: 0,
                side: Side::Ally,
                arrival: crate::Arrival::Auto,
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
                side: Side::Enemy,
                arrival: crate::Arrival::Auto,
            }
        }
    }
}
