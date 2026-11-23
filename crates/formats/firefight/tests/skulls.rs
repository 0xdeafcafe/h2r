//! The skull schedule and the score multipliers, on invented definitions.

use std::collections::{BTreeMap, BTreeSet};

use firefight::director::{Kill, PlayerSeen, SQUAD_SLOTS, ZoneSeen};
use firefight::scoring::kill_points;
use firefight::{
    Caps, Command, Count, Def, Difficulty, DifficultyMultipliers, Director, Drop, FORMAT, Lives,
    Observed, Round, Scoring, Skull, SquadSpec, Wave,
};

fn round(skulls_on: Vec<Skull>) -> Round {
    let waves = (0..5)
        .map(|wave| Wave {
            squads: vec![SquadSpec {
                class: "elite".into(),
                count: Count {
                    per_player: 1,
                    min: 1,
                    max: 1,
                },
                drop: Drop::Edge,
                weapon_tier: 0,
            }],
            boss: wave == 4,
            gap_ticks: 2,
        })
        .collect();
    Round { waves, skulls_on }
}

fn scoring(skull_multipliers: BTreeMap<Skull, u16>) -> Scoring {
    Scoring {
        by_class: [("elite".into(), 100), ("grunt".into(), 7)].into(),
        player_kill: 0,
        difficulty_multiplier: DifficultyMultipliers {
            easy: 1_000,
            normal: 1_000,
            heroic: 1_000,
            legendary: 1_500,
        },
        skull_multipliers,
        assist: 250,
    }
}

fn three_rounds() -> Def {
    Def {
        format: FORMAT,
        name: "Skulls".into(),
        teams: 1,
        rounds: vec![
            round(vec![Skull::Famine]),
            round(vec![Skull::Mythic, Skull::Angry]),
            round(vec![Skull::Ghost]),
        ],
        lives: Lives {
            pool_start: 3,
            per_round_bonus: 0,
            shared: true,
        },
        caps: Caps {
            live_actors_by_players: vec![(4, 4)],
        },
        scoring: scoring(BTreeMap::new()),
    }
}

/// Runs a match in which every squad dies to player 0 a tick after it
/// appears, and returns the commands with their tick.
fn play(def: Def, skulls: BTreeSet<Skull>) -> Vec<(u32, Command)> {
    let mut director = Director::new(
        def,
        firefight::Settings {
            seed: 3,
            difficulty: Difficulty::Normal,
            skulls,
        },
    )
    .unwrap();
    let mut alive = [0u16; SQUAD_SLOTS];
    let mut classes: [String; SQUAD_SLOTS] = Default::default();
    let mut placed_at = [0u32; SQUAD_SLOTS];
    let mut pending: Vec<Command> = Vec::new();
    let mut all = Vec::new();
    for tick in 0..2_000 {
        for command in std::mem::take(&mut pending) {
            match command {
                Command::PlaceSquad {
                    slot, class, count, ..
                } => {
                    alive[usize::from(slot)] = count;
                    classes[usize::from(slot)] = class;
                    placed_at[usize::from(slot)] = tick;
                }
                Command::EraseSquad { slot } => alive[usize::from(slot)] = 0,
                Command::Finish { .. } => return all,
                _ => {}
            }
        }
        let mut kills = Vec::new();
        for slot in 0..SQUAD_SLOTS {
            if alive[slot] > 0 && tick > placed_at[slot] {
                for _ in 0..alive[slot] {
                    kills.push(Kill {
                        class: classes[slot].clone(),
                        killer: Some(0),
                        assist: None,
                    });
                }
                alive[slot] = 0;
            }
        }
        let seen = Observed {
            players: vec![PlayerSeen {
                slot: 0,
                alive: true,
                team: 0,
            }],
            actors_alive_by_squad: alive,
            zones: vec![ZoneSeen {
                seen: false,
                distance: 10,
                team_distance: [10, 10],
            }],
            kills,
            player_kills: Vec::new(),
        };
        let out = director.step(tick, &seen);
        all.extend(out.iter().cloned().map(|c| (tick, c)));
        pending = out;
    }
    panic!("the match did not finish");
}

fn skull(id: Skull) -> Command {
    Command::Skull { id, on: true }
}

#[test]
fn a_round_switches_its_skulls_on() {
    let commands: Vec<Command> = play(three_rounds(), BTreeSet::new())
        .into_iter()
        .map(|(_, c)| c)
        .filter(|c| matches!(c, Command::Round { .. } | Command::Skull { .. }))
        .collect();
    // Each round announces itself, then switches its own skulls on, in the
    // schedule's order; nothing is ever switched off.
    assert_eq!(
        commands,
        [
            Command::Round { index: 0 },
            skull(Skull::Famine),
            Command::Round { index: 1 },
            skull(Skull::Mythic),
            skull(Skull::Angry),
            Command::Round { index: 2 },
            skull(Skull::Ghost),
        ]
    );
}

#[test]
fn a_skull_already_on_is_not_switched_on_again() {
    let on = [Skull::Famine, Skull::Angry].into();
    let skulls: Vec<Command> = play(three_rounds(), on)
        .into_iter()
        .map(|(_, c)| c)
        .filter(|c| matches!(c, Command::Skull { .. }))
        .collect();
    assert_eq!(skulls, [skull(Skull::Mythic), skull(Skull::Ghost)]);
}

#[test]
fn a_round_skull_scores_from_its_first_wave() {
    let mut def = three_rounds();
    def.scoring = scoring([(Skull::Famine, 2_000)].into());
    let scores: Vec<u32> = play(def, BTreeSet::new())
        .into_iter()
        .filter_map(|(_, c)| match c {
            Command::Score { points, .. } => Some(points),
            _ => None,
        })
        .collect();
    // Normal difficulty. Famine is on from round 0's first wave.
    assert!(!scores.is_empty());
    assert!(scores.iter().all(|&p| p == 200), "{scores:?}");
}

#[test]
fn multipliers_multiply() {
    let table = scoring(
        [
            (Skull::Mythic, 1_500),
            (Skull::Angry, 1_200),
            (Skull::Famine, 1_100),
        ]
        .into(),
    );
    let points = |class: &str, difficulty, on: &[Skull]| {
        kill_points(&table, class, difficulty, &on.iter().copied().collect())
    };
    assert_eq!(points("elite", Difficulty::Normal, &[]), Some(100));
    // 100 * 1.5 * 1.5 * 1.2 = 270, formed exactly.
    assert_eq!(
        points(
            "elite",
            Difficulty::Legendary,
            &[Skull::Mythic, Skull::Angry]
        ),
        Some(270)
    );
    assert_eq!(
        points("elite", Difficulty::Normal, &[Skull::Mythic, Skull::Angry]),
        Some(180)
    );
    // A skull without an entry counts 1,000.
    assert_eq!(
        points("elite", Difficulty::Normal, &[Skull::Ghost]),
        Some(100)
    );
    // Rounded down once: 7 * 1.1 * 1.2 = 9.24 (rounding each step gives 8).
    assert_eq!(
        points("grunt", Difficulty::Normal, &[Skull::Famine, Skull::Angry]),
        Some(9)
    );
    assert_eq!(
        points(
            "grunt",
            Difficulty::Normal,
            &[Skull::Famine, Skull::Mythic, Skull::Angry]
        ),
        Some(13)
    );
    // A class without a score scores nothing.
    assert_eq!(points("hunter", Difficulty::Normal, &[]), None);
}
