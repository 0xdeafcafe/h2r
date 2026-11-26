mod common;

use std::collections::BTreeSet;

use common::*;
use firefight::director::{Kill, ShipSeen};
use firefight::{Command, Observed, Outcome, ScoreReason, ScriptDirector, Skull};

fn place(slot: u8) -> Command {
    Command::PlaceSquad {
        slot,
        class: "elite".into(),
        count: 1,
        zone: 0,
        weapon_tier: 0,
    }
}

fn score(points: u32) -> Command {
    Command::Score {
        player: 0,
        points,
        reason: ScoreReason::Kill,
    }
}

/// The golden match: one player who dies on tick 4, a squad of one killed
/// three ticks after it appears, and the drop ship as the kit reports it.
fn golden_match(ship: ShipSeen) -> Run {
    let mut director = ScriptDirector::new(small_def(1, true), settings(7)).unwrap();
    let mut world = World::new(1, two_zones());
    world.ship = ship;
    run(
        &mut director,
        &mut world,
        100,
        |tick, world| {
            if tick == 4 {
                world.players.insert(0, false);
            }
            kill_at_age_three(tick, world);
        },
        |_, _, _, _, _| {},
    )
}

fn golden() -> Vec<(u32, Command)> {
    // 100 * 1.5 (Legendary) * 1.1 (Famine) = 165.
    vec![
        (
            0,
            Command::Lives {
                player: None,
                pool: 1,
            },
        ),
        (2, Command::Round { index: 0 }),
        (
            2,
            Command::Skull {
                id: Skull::Famine,
                on: true,
            },
        ),
        (2, Command::Wave { index: 0 }),
        (2, place(0)),
        (4, Command::Respawn { player: 0 }),
        (
            4,
            Command::Lives {
                player: None,
                pool: 0,
            },
        ),
        (6, score(165)),
        (8, Command::Wave { index: 1 }),
        (8, place(0)),
        (12, score(165)),
        (14, Command::Wave { index: 2 }),
        (14, place(0)),
        (18, score(165)),
        (20, Command::Wave { index: 3 }),
        (20, place(0)),
        (24, score(165)),
        (26, Command::Wave { index: 4 }),
        (26, place(0)),
        (30, score(165)),
        (
            30,
            Command::Finish {
                outcome: Outcome::Won,
            },
        ),
    ]
}

#[test]
fn a_scripted_match_issues_the_golden_command_list() {
    let run = golden_match(ShipSeen::default());
    assert_eq!(run.commands, golden());
    assert_eq!(run.outcome, Some(Outcome::Won));
}

/// A map whose kit has no ship, or a ship and no path to a zone, keeps
/// every squad on foot: the log is the golden's.
#[test]
fn a_map_without_a_ship_places_squads_as_today() {
    for ship in [
        ShipSeen { ready: false, paths: vec![true, true] },
        ShipSeen { ready: true, paths: vec![false, false] },
        ShipSeen { ready: true, paths: Vec::new() },
    ] {
        let run = golden_match(ship.clone());
        assert_eq!(run.commands, golden(), "{ship:?}");
        assert_eq!(run.outcome, Some(Outcome::Won));
    }
}

#[test]
fn live_actors_never_exceed_the_cap() {
    let mut outcomes = BTreeSet::new();
    for index in 0..1_000 {
        let (def, run) = random_match(index, |tick, director, world, _, _| {
            let cap = director.cap(world.players.len() as u8);
            assert!(world.applied_live <= cap, "match {index} tick {tick}");
            assert!(director.live_actors() <= cap, "match {index} tick {tick}");
        });
        let outcome = run
            .outcome
            .unwrap_or_else(|| panic!("match {index} never ended"));
        outcomes.insert(format!("{outcome:?}"));

        // The reserve: each wave places its own squads, in order, once.
        let (mut round, mut wave, mut placed) = (0, 0, 0);
        for (_, command) in &run.commands {
            match command {
                Command::Round { index } => round = usize::from(*index),
                Command::Wave { index } => (wave, placed) = (usize::from(*index), 0),
                Command::PlaceSquad { class, count, .. } => {
                    let squads = &def.rounds[round].waves[wave].squads;
                    let squad = squads.get(placed).expect("placed beyond the reserve");
                    assert_eq!(*class, squad.class);
                    assert!((squad.count.min..=squad.count.max).contains(count));
                    placed += 1;
                }
                _ => {}
            }
        }
    }
    // One team ends Won or Lost; two end with a team winning (or a draw).
    for one in ["Won", "Lost"] {
        assert!(outcomes.contains(one), "{one} occurs: {outcomes:?}");
    }
    assert!(
        outcomes.iter().any(|o| o.starts_with("TeamWon")),
        "a team wins: {outcomes:?}"
    );
}

#[test]
fn no_zone_in_sight_is_chosen_while_another_exists() {
    let mut placed = 0;
    for index in 0..200 {
        random_match(index, |_, _, _, seen, out| {
            for command in out {
                if let Command::PlaceSquad { zone, .. } = command {
                    placed += 1;
                    let zone = &seen.zones[usize::from(*zone)];
                    if seen.zones.iter().any(|zone| !zone.seen) {
                        assert!(!zone.seen);
                    }
                }
            }
        });
    }
    assert!(placed > 1_000);
}

#[test]
fn zone_choice_follows_the_definition_and_the_seed() {
    let def = zone_choice_def();
    let zones = six_zones;
    let picks = |seed| {
        let mut director = ScriptDirector::new(def.clone(), settings(seed)).unwrap();
        let mut world = World::new(1, zones());
        run(
            &mut director,
            &mut world,
            100,
            kill_at_age_three,
            |_, _, _, _, _| {},
        )
        .commands
        .into_iter()
        .filter_map(|(_, command)| match command {
            Command::PlaceSquad { zone, .. } => Some(zone),
            _ => None,
        })
        .collect::<Vec<_>>()
    };

    let first = picks(3);
    assert_eq!(first.len(), 5);
    // An unseen preferred zone is honoured.
    assert_eq!(first[0], 2);
    // A preferred zone in sight, or one the kit lacks, falls back to the
    // farther half of the unseen zones: 3, 4 and 0.
    for &zone in &first[1..] {
        assert!([3, 4, 0].contains(&zone), "{first:?}");
    }
    assert_eq!(picks(3), first);
}

#[test]
fn same_seed_same_commands() {
    for index in [1, 17, 302] {
        let (_, a) = random_match(index, |_, _, _, _, _| {});
        let (_, b) = random_match(index, |_, _, _, _, _| {});
        assert_eq!(a.commands, b.commands);
        assert!(a.outcome.is_some());
    }
}

#[test]
fn a_replay_of_the_observed_sequence_issues_the_same_commands() {
    let (def, original) = random_match(5, |_, _, _, _, _| {});
    let mut director = ScriptDirector::new(def, settings(5)).unwrap();
    let mut replayed = Vec::new();
    for (tick, seen) in original.observed.iter().enumerate() {
        let tick = tick as u32;
        replayed.extend(
            director
                .step(tick, seen)
                .into_iter()
                .map(|command| (tick, command)),
        );
    }
    assert_eq!(replayed, original.commands);
}

#[test]
fn the_pool_at_zero_holds_respawns_until_the_wave_ends() {
    let mut director = ScriptDirector::new(small_def(0, true), settings(1)).unwrap();
    let mut world = World::new(2, two_zones());
    let run = run(
        &mut director,
        &mut world,
        100,
        |tick, world| {
            if tick == 4 {
                world.players.insert(1, false);
            }
            kill_at_age_three(tick, world);
        },
        |_, _, _, _, _| {},
    );
    let respawns: Vec<_> = run
        .commands
        .iter()
        .filter(|(_, command)| matches!(command, Command::Respawn { .. }))
        .collect();
    // Player 1 dies on tick 4; the first wave's actor dies on tick 6.
    assert_eq!(respawns, [&(6, Command::Respawn { player: 1 })]);
    assert_eq!(run.outcome, Some(Outcome::Won));
}

#[test]
fn everyone_dead_and_no_lives_finishes_lost() {
    let mut director = ScriptDirector::new(small_def(0, true), settings(1)).unwrap();
    let mut world = World::new(1, two_zones());
    let run = run(
        &mut director,
        &mut world,
        100,
        |tick, world| {
            if tick == 4 {
                world.players.insert(0, false);
            }
        },
        |_, _, _, _, _| {},
    );
    assert_eq!(
        run.commands.last(),
        Some(&(
            4,
            Command::Finish {
                outcome: Outcome::Lost
            }
        ))
    );
    assert_eq!(run.outcome, Some(Outcome::Lost));
    // A finished director issues nothing more.
    assert!(director.step(5, &Observed::default()).is_empty());
}

#[test]
fn unshared_lives_are_counted_per_player() {
    let mut director = ScriptDirector::new(small_def(1, false), settings(1)).unwrap();
    let mut world = World::new(2, two_zones());
    let run = run(
        &mut director,
        &mut world,
        5,
        |tick, world| {
            if tick == 3 {
                world.players.insert(1, false);
            }
        },
        |_, _, _, _, _| {},
    );
    let lives: Vec<_> = run
        .commands
        .iter()
        .filter(|(_, command)| matches!(command, Command::Lives { .. } | Command::Respawn { .. }))
        .cloned()
        .collect();
    assert_eq!(
        lives,
        [
            (
                0,
                Command::Lives {
                    player: Some(0),
                    pool: 1
                }
            ),
            (
                0,
                Command::Lives {
                    player: Some(1),
                    pool: 1
                }
            ),
            (3, Command::Respawn { player: 1 }),
            (
                3,
                Command::Lives {
                    player: Some(1),
                    pool: 0
                }
            ),
        ]
    );
}

#[test]
fn kills_score_by_the_definition_with_assists() {
    let mut director = ScriptDirector::new(small_def(1, true), settings(1)).unwrap();
    let mut world = World::new(2, two_zones());
    let mut seen = world.observe();
    seen.kills = vec![
        Kill {
            class: "elite".into(),
            killer: Some(1),
            assist: Some(0),
        },
        // The world's kills and unknown classes score nothing.
        Kill {
            class: "elite".into(),
            killer: None,
            assist: Some(0),
        },
        Kill {
            class: "grunt".into(),
            killer: Some(1),
            assist: None,
        },
    ];
    let out = director.step(0, &seen);
    // No skull is on before the first round: 100 * 1.5 = 150; assist 25%.
    assert_eq!(
        out[1..],
        [
            Command::Score {
                player: 1,
                points: 150,
                reason: ScoreReason::Kill
            },
            Command::Score {
                player: 0,
                points: 37,
                reason: ScoreReason::Assist
            },
        ]
    );
}

#[test]
fn commands_have_canonical_integer_json() {
    let command = Command::PlaceSquad {
        slot: 3,
        class: "elite".into(),
        count: 4,
        zone: 7,
        weapon_tier: 1,
    };
    let value = serde_json::to_value(&command).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"place_squad": {"slot": 3, "class": "elite", "count": 4, "zone": 7, "weapon_tier": 1}})
    );
    let back: Command = serde_json::from_value(value).unwrap();
    assert_eq!(back, command);
}
