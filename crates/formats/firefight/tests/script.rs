//! The director's scenarios over the compiled rules script: every match
//! the fake world plays is replayed through a fresh script over what it
//! observed, and gives the same commands.

mod common;

use std::collections::BTreeSet;

use common::*;
use firefight::director::{Kill, PlayerSeen, ZoneSeen};
use firefight::{
    Command, Def, Difficulty, Observed, Outcome, ScriptDirector, Settings, Skull, builtin,
};
use rules::vm::RulesExt;

/// Feeds one observed sequence to a fresh script and returns the commands
/// with their tick. A script error fails the replay.
fn replay(def: &Def, settings: &Settings, observed: &[Observed]) -> Vec<(u32, Command)> {
    let mut script = ScriptDirector::new(def.clone(), settings.clone()).unwrap();
    let mut log = Vec::new();
    for (tick, seen) in observed.iter().enumerate() {
        let tick = tick as u32;
        log.extend(
            script
                .step(tick, seen)
                .into_iter()
                .map(|command| (tick, command)),
        );
    }
    assert_eq!(script.error(), None, "{}", def.name);
    log
}

/// Runs the script in the fake world, then replays what it observed
/// through a fresh one for the same commands.
fn check(
    def: &Def,
    settings: &Settings,
    players: u8,
    zones: Vec<ZoneSeen>,
    limit: u32,
    script: impl FnMut(u32, &mut World),
) -> Run {
    let mut director = ScriptDirector::new(def.clone(), settings.clone()).unwrap();
    let mut world = World::new(players, zones);
    let run = run(&mut director, &mut world, limit, script, |_, _, _, _, _| {});
    assert_eq!(replay(def, settings, &run.observed), run.commands);
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
fn the_golden_match_replays_the_same_commands() {
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
    let log = replay(&def, &settings(1), &[seen]);
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
        (Skull::BlackEye, 1),
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
        Skull::BlackEye,
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
            let log = replay(&def, &with(0, difficulty, on), &observed());
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
fn random_matches_over_the_built_ins_replay_the_same_commands() {
    let mut outcomes = BTreeSet::new();
    for index in 0..48 {
        let (def, run) = random_match(index, |_, _, _, _, _| {});
        assert_eq!(replay(&def, &settings(index), &run.observed), run.commands);
        outcomes.insert(format!("{:?}", run.outcome));
    }
    assert!(outcomes.len() >= 2, "{outcomes:?}");
}

#[test]
fn the_built_ins_as_authored_replay_the_same_commands() {
    let difficulties = [
        Difficulty::Normal,
        Difficulty::Heroic,
        Difficulty::Legendary,
        Difficulty::Easy,
        Difficulty::Normal,
    ];
    for (i, def) in builtin::definitions().into_iter().enumerate() {
        let settings = with(40 + i as u64, difficulties[i], &[]);
        let players = (1 + 5 * i as u8).min(16);
        let run = check(&def, &settings, players, eight_zones(), 50_000, random_script(41 + i as u64));
        assert!(run.outcome.is_some(), "{} never ended", def.name);
    }
}

#[test]
fn starting_skulls_and_difficulty_replay_the_same_commands() {
    let skulls = [Skull::Famine, Skull::Angry, Skull::Ghost, Skull::BlackEye, Skull::Mythic];
    for (i, def) in quick_builtins().into_iter().enumerate() {
        let settings = with(7 + i as u64, Difficulty::Heroic, &skulls[..=i]);
        check(&def, &settings, 3, eight_zones(), 50_000, random_script(9 + i as u64));
    }
}

#[test]
fn unshared_lives_over_whole_matches_replay_the_same_commands() {
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
fn a_player_listed_twice_or_leaving_replays_the_same_commands() {
    let def = small_def(2, false);
    let mut world = World::new(3, two_zones());
    let mut observed = Vec::new();
    for tick in 0..60u32 {
        let mut seen = world.observe();
        if tick % 7 == 0 {
            seen.players.push(PlayerSeen {
                slot: 1,
                alive: false,
                team: 1,
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
    replay(&def, &settings(3), &observed);
}

#[test]
fn resumed_script_state_replays_the_same_commands() {
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

/// Two-team matches over Crossfire and two-team forms of the other sets,
/// with shared and own lives: player kills, targets, each team's pool and
/// score, and every way a match ends.
#[test]
fn two_team_matches_replay_the_same_commands() {
    let mut outcomes = BTreeSet::new();
    for index in 0..24u64 {
        let base = quick_builtins()[(index % 4) as usize].clone();
        let mut def = builtin::with_teams(base, 2);
        def.lives.shared = index % 3 != 0;
        let players = 2 + (index % 7) as u8;
        let run = check(
            &def,
            &settings(index),
            players,
            eight_zones(),
            50_000,
            random_script(index),
        );
        outcomes.insert(format!("{:?}", run.outcome));
    }
    assert!(outcomes.len() >= 2, "{outcomes:?}");
}

/// The two-team ends, scripted: a team out of lives, both out at once
/// level and on score, a lone team out, and the last round on score, on
/// lives (shared and own) and level.
#[test]
fn two_team_ends_replay_the_same_commands() {
    let die = |at: u32, who: &'static [u8]| {
        move |tick: u32, world: &mut World| {
            if tick == at {
                for &slot in who {
                    world.players.insert(slot, false);
                }
            }
        }
    };
    check(&pair_def(0, true), &settings(1), 2, two_zones(), 20, die(4, &[1]));
    check(&pair_def(0, true), &settings(1), 2, two_zones(), 20, die(4, &[0, 1]));
    check(&pair_def(0, true), &settings(1), 1, two_zones(), 20, die(4, &[0]));
    check(&pair_def(0, true), &settings(1), 2, two_zones(), 20, |tick, world| {
        if tick == 6 {
            world.kill_squad(0, Some(1));
        }
        die(10, &[0, 1])(tick, world);
    });
    for (pool, shared, players, killer) in [
        (5, true, 2, Some(1)),
        (5, true, 2, None),
        (2, false, 4, None),
        (5, true, 1, None),
    ] {
        check(
            &pair_def(pool, shared),
            &settings(2),
            players,
            two_zones(),
            100,
            move |tick, world| {
                if tick == 4 {
                    world.players.insert(players - 1, false);
                }
                for slot in 0..firefight::director::SQUAD_SLOTS {
                    if world.squads[slot].0 > 0 && tick - world.squads[slot].1 == 3 {
                        world.kill_squad(slot, killer);
                    }
                }
            },
        );
    }
}

/// Player kills across the teams, within a team, by oneself and by a fall,
/// and a player whose game team changes after it is first seen.
#[test]
fn player_kills_and_fixed_teams_replay_the_same_commands() {
    check(&pair_def(5, true), &settings(3), 4, two_zones(), 30, |tick, world| {
        match tick {
            1 => {
                world.kill_player(1, Some(0));
                world.kill_player(2, Some(0));
                world.kill_player(3, Some(3));
                world.kill_player(0, None);
            }
            8 => {
                world.teams.insert(1, 0);
                world.kill_player(0, Some(1));
            }
            _ => {}
        }
        kill_at_age_three(tick, world);
    });
}

/// Squads sent at each team and dropped on its side, with zones that lean
/// from one team to the other, and zones all on one team's side.
#[test]
fn targets_and_team_sides_replay_the_same_picks() {
    let mut def = pair_def(3, true);
    for wave in def.rounds.iter_mut().flat_map(|round| &mut round.waves) {
        wave.squads = vec![wave.squads[0].clone(); 4];
    }
    let mut zones = eight_zones();
    for (i, z) in zones.iter_mut().enumerate() {
        let i = i as u32;
        z.team_distance = [i * 10, 70 - i * 10];
        z.seen = i == 1;
    }
    check(&def, &settings(5), 4, zones.clone(), 400, kill_at_age_three);
    for z in &mut zones {
        z.team_distance = [90, 10];
    }
    check(&def, &settings(6), 4, zones, 400, kill_at_age_three);
}
