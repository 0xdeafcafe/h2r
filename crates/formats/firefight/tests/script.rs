//! The director compiled to a rules script gives the Rust director's
//! commands and state, tick by tick, over the director's own scenarios and
//! every built-in definition.

mod common;

use std::collections::BTreeSet;

use common::*;
use firefight::director::{Kill, PlayerSeen, ZoneSeen};
use firefight::{
    Command, Def, Difficulty, Director, Observed, Outcome, ScriptDirector, Settings, Skull,
    builtin,
};
use rules::vm::RulesExt;

/// Feeds one observed sequence to the director and to the script, and
/// checks that each tick gives the same commands and the same state.
/// Returns the commands with their tick.
fn parity(def: &Def, settings: &Settings, observed: &[Observed]) -> Vec<(u32, Command)> {
    let mut director = Director::new(def.clone(), settings.clone()).unwrap();
    let mut script = ScriptDirector::new(def.clone(), settings.clone()).unwrap();
    let mut log = Vec::new();
    for (tick, seen) in observed.iter().enumerate() {
        let tick = tick as u32;
        let want = director.step(tick, seen);
        let got = script.step(tick, seen);
        assert_eq!(got, want, "{}: tick {tick}", def.name);
        assert_eq!(script.state(), *director.state(), "{}: tick {tick}", def.name);
        log.extend(want.into_iter().map(|command| (tick, command)));
    }
    assert_eq!(script.error(), None);
    log
}

/// Runs the director in the fake world, then replays what it observed
/// through [`parity`].
fn check(
    def: &Def,
    settings: &Settings,
    players: u8,
    zones: Vec<ZoneSeen>,
    limit: u32,
    script: impl FnMut(u32, &mut World),
) -> Run {
    let mut director = Director::new(def.clone(), settings.clone()).unwrap();
    let mut world = World::new(players, zones);
    let run = run(&mut director, &mut world, limit, script, |_, _, _, _, _| {});
    assert_eq!(parity(def, settings, &run.observed), run.commands);
    run
}

fn with(seed: u64, difficulty: Difficulty, skulls: &[Skull]) -> Settings {
    Settings {
        seed,
        difficulty,
        skulls: skulls.iter().copied().collect(),
    }
}

#[test]
fn the_golden_match_gives_the_same_commands() {
    let run = check(&small_def(1, true), &settings(7), 1, two_zones(), 100, |tick, world| {
        if tick == 4 {
            world.players.insert(0, false);
        }
        kill_at_age_three(tick, world);
    });
    assert_eq!(run.outcome, Some(Outcome::Won));
}

#[test]
fn an_empty_pool_holds_respawns_until_the_wave_ends() {
    let run = check(&small_def(0, true), &settings(1), 2, two_zones(), 100, |tick, world| {
        if tick == 4 {
            world.players.insert(1, false);
        }
        kill_at_age_three(tick, world);
    });
    assert_eq!(run.outcome, Some(Outcome::Won));
}

#[test]
fn everyone_dead_finishes_lost_and_then_nothing() {
    let def = small_def(0, true);
    let run = check(&def, &settings(1), 1, two_zones(), 100, |tick, world| {
        if tick == 4 {
            world.players.insert(0, false);
        }
    });
    assert_eq!(run.outcome, Some(Outcome::Lost));
    let mut script = ScriptDirector::new(def, settings(1)).unwrap();
    for (tick, seen) in run.observed.iter().enumerate() {
        script.step(tick as u32, seen);
    }
    assert!(script.step(5, &Observed::default()).is_empty());
}

#[test]
fn unshared_lives_are_counted_per_player() {
    check(&small_def(1, false), &settings(1), 2, two_zones(), 5, |tick, world| {
        if tick == 3 {
            world.players.insert(1, false);
        }
    });
}

#[test]
fn kills_score_by_the_definition_with_assists() {
    let def = small_def(1, true);
    let mut world = World::new(2, two_zones());
    let mut seen = world.observe();
    seen.kills = vec![
        Kill {
            class: "elite".into(),
            killer: Some(1),
            assist: Some(0),
        },
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
        Kill {
            class: "elite".into(),
            killer: Some(1),
            assist: Some(1),
        },
    ];
    let log = parity(&def, &settings(1), &[seen]);
    assert_eq!(log.len(), 4, "{log:?}");
}

/// Class points and multipliers at their limits, every skull on: the
/// script forms the product exactly and rounds once, as the director does.
#[test]
fn kill_points_are_exact_at_the_limits() {
    let mut def = small_def(1, true);
    def.rounds[0].skulls_on.clear();
    def.scoring.by_class = [
        ("elite".into(), 1_000),
        ("grunt".into(), 7),
        ("jackal".into(), 999),
    ]
    .into();
    def.scoring.difficulty_multiplier.easy = 1;
    def.scoring.difficulty_multiplier.heroic = 1_999;
    def.scoring.difficulty_multiplier.legendary = 2_000;
    def.scoring.skull_multipliers = [
        (Skull::Assassins, 2_000),
        (Skull::Thunderstorm, 1_999),
        (Skull::Famine, 1_100),
        (Skull::Blind, 1),
        (Skull::Ghost, 1_333),
        (Skull::Catch, 2_000),
        (Skull::Mythic, 1_500),
        (Skull::Angry, 1_200),
    ]
    .into();
    def.scoring.assist = 999;
    let kills = |class: &str| Kill {
        class: class.into(),
        killer: Some(0),
        assist: Some(1),
    };
    let observed = || {
        let mut world = World::new(2, two_zones());
        let mut seen = world.observe();
        seen.kills = ["elite", "grunt", "jackal"].map(kills).to_vec();
        vec![seen]
    };
    let all = [
        Skull::Assassins,
        Skull::Thunderstorm,
        Skull::Famine,
        Skull::Blind,
        Skull::Ghost,
        Skull::Catch,
        Skull::Mythic,
        Skull::Angry,
        Skull::LowGravity,
    ];
    for difficulty in [
        Difficulty::Easy,
        Difficulty::Normal,
        Difficulty::Heroic,
        Difficulty::Legendary,
    ] {
        let sets: [&[Skull]; 5] = [&all, &all[..1], &all[3..4], &all[6..8], &[]];
        for on in sets {
            let log = parity(&def, &with(0, difficulty, on), &observed());
            assert!(!log.is_empty());
        }
    }
}

#[test]
fn zone_choice_follows_the_definition_and_the_seed() {
    let def = zone_choice_def();
    for seed in 0..20 {
        let run = check(&def, &settings(seed), 1, six_zones(), 100, kill_at_age_three);
        let zones: Vec<u8> = run
            .commands
            .iter()
            .filter_map(|(_, command)| match command {
                Command::PlaceSquad { zone, .. } => Some(*zone),
                _ => None,
            })
            .collect();
        assert_eq!(zones.len(), 5);
        assert_eq!(zones[0], 2);
    }
}

/// Every pairing of a quick built-in set with a player count from 1 to 16.
#[test]
fn random_matches_over_the_built_ins_give_the_same_commands() {
    let mut outcomes = BTreeSet::new();
    for index in 0..48 {
        let (def, run) = random_match(index, |_, _, _, _, _| {});
        assert_eq!(parity(&def, &settings(index), &run.observed), run.commands);
        outcomes.insert(format!("{:?}", run.outcome));
    }
    assert!(outcomes.len() >= 2, "{outcomes:?}");
}

#[test]
fn the_built_ins_as_authored_give_the_same_commands() {
    let difficulties = [Difficulty::Normal, Difficulty::Heroic, Difficulty::Legendary];
    for (i, def) in builtin::definitions().into_iter().enumerate() {
        let settings = with(40 + i as u64, difficulties[i], &[]);
        let players = 1 + 5 * i as u8;
        let run = check(&def, &settings, players, eight_zones(), 50_000, random_script(41 + i as u64));
        assert!(run.outcome.is_some(), "{} never ended", def.name);
    }
}

#[test]
fn starting_skulls_and_difficulty_give_the_same_commands() {
    let skulls = [Skull::Famine, Skull::Angry, Skull::Ghost];
    for (i, def) in quick_builtins().into_iter().enumerate() {
        let settings = with(7 + i as u64, Difficulty::Heroic, &skulls[..=i]);
        check(&def, &settings, 3, eight_zones(), 50_000, random_script(9 + i as u64));
    }
}

#[test]
fn unshared_lives_over_whole_matches_give_the_same_commands() {
    for (i, mut def) in quick_builtins().into_iter().enumerate() {
        def.lives.shared = false;
        let seed = 4 * i as u64;
        check(&def, &settings(seed), 5, eight_zones(), 50_000, random_script(seed));
    }
}

#[test]
fn zero_gaps_chain_waves_in_one_step() {
    for (i, mut def) in quick_builtins().into_iter().enumerate() {
        for wave in def.rounds.iter_mut().flat_map(|round| &mut round.waves) {
            wave.gap_ticks = 0;
        }
        check(&def, &settings(i as u64), 2, eight_zones(), 50_000, random_script(1 + i as u64));
    }
}

#[test]
fn zones_in_sight_and_ties_give_the_same_picks() {
    let def = quick_builtins()[2].clone();
    // Every zone in sight, then ties on distance broken by kit order.
    let mut zones = eight_zones();
    for (i, zone) in zones.iter_mut().enumerate() {
        zone.seen = true;
        zone.distance = (i as u32 / 3) * 10;
    }
    check(&def, &settings(11), 4, zones, 3_000, |tick, world| {
        kill_at_age_three(tick, world);
    });
    check(&def, &settings(12), 4, Vec::new(), 200, kill_at_age_three);
}

#[test]
fn a_player_listed_twice_or_leaving_gives_the_same_commands() {
    let def = small_def(2, false);
    let mut world = World::new(3, two_zones());
    let mut observed = Vec::new();
    for tick in 0..60u32 {
        let mut seen = world.observe();
        if tick % 7 == 0 {
            seen.players.push(PlayerSeen {
                slot: 1,
                alive: false,
            });
        }
        if (20..30).contains(&tick) {
            seen.players.retain(|p| p.slot != 2);
        }
        if tick == 12 {
            seen.players.iter_mut().for_each(|p| p.alive = false);
        }
        observed.push(seen);
    }
    parity(&def, &settings(3), &observed);
}

#[test]
fn resumed_script_state_gives_the_same_commands() {
    let (def, original) = random_match(9, |_, _, _, _, _| {});
    let settings = settings(9);
    let split = original.observed.len() / 2;
    let mut script = ScriptDirector::new(def.clone(), settings.clone()).unwrap();
    let mut commands = Vec::new();
    for (tick, seen) in original.observed[..split].iter().enumerate() {
        let tick = tick as u32;
        commands.extend(script.step(tick, seen).into_iter().map(|c| (tick, c)));
    }
    let bytes = script.ext().encode();
    let ext = RulesExt::decode(&bytes).unwrap();
    let mut resumed = ScriptDirector::resume(def, &settings, ext).unwrap();
    assert_eq!(resumed.state(), script.state());
    for (tick, seen) in original.observed.iter().enumerate().skip(split) {
        let tick = tick as u32;
        commands.extend(resumed.step(tick, seen).into_iter().map(|c| (tick, c)));
    }
    assert_eq!(commands, original.commands);
}
