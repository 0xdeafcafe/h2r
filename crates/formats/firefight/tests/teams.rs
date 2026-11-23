//! Two player teams: a pool and a score each, player kills across the
//! teams, the end when a team runs out of lives or the last round is
//! cleared, and squads sent at both teams.

mod common;

use common::*;
use firefight::director::SQUAD_SLOTS;
use firefight::{Command, Count, Def, Director, Outcome, ScoreReason};

/// Every squad dies `killer`'s way three ticks after it appears.
fn clear_at_age_three(killer: Option<u8>) -> impl FnMut(u32, &mut World) {
    move |tick, world| {
        for slot in 0..SQUAD_SLOTS {
            if world.squads[slot].0 > 0 && tick - world.squads[slot].1 == 3 {
                world.kill_squad(slot, killer);
            }
        }
    }
}

fn play(def: Def, players: u8, limit: u32, script: impl FnMut(u32, &mut World)) -> (Director, Run) {
    let mut director = Director::new(def, settings(1)).unwrap();
    let mut world = World::new(players, two_zones());
    let run = run(&mut director, &mut world, limit, script, |_, _, _, _, _| {});
    (director, run)
}

fn has(run: &Run, tick: u32, command: Command) -> bool {
    run.commands.contains(&(tick, command))
}

#[test]
fn two_teams_start_with_a_pool_each_and_spend_their_own() {
    let (director, run) = play(pair_def(3, true), 2, 6, |tick, world| {
        if tick == 4 {
            world.players.insert(1, false);
        }
    });
    assert_eq!(
        run.commands[..2],
        [
            (0, Command::TeamLives { team: 0, pool: 3 }),
            (0, Command::TeamLives { team: 1, pool: 3 }),
        ]
    );
    assert!(has(&run, 4, Command::Respawn { player: 1 }));
    assert!(has(&run, 4, Command::TeamLives { team: 1, pool: 2 }));
    assert_eq!(director.state().pools, [3, 2]);
    assert!(
        !run.commands
            .iter()
            .any(|(_, c)| matches!(c, Command::Lives { player: None, .. }))
    );
}

#[test]
fn a_team_out_of_lives_loses_to_the_other() {
    let (_, run) = play(pair_def(0, true), 2, 20, |tick, world| {
        if tick == 4 {
            world.players.insert(1, false);
        }
    });
    assert!(has(
        &run,
        4,
        Command::Finish {
            outcome: Outcome::TeamWon { team: 0 }
        }
    ));
    assert_eq!(run.outcome, Some(Outcome::TeamWon { team: 0 }));
}

#[test]
fn both_teams_out_at_once_go_to_the_higher_score() {
    // Player 1 clears wave 0 (team 1 scores), then everyone dies in wave 1.
    let (_, run) = play(pair_def(0, true), 2, 20, |tick, world| {
        if tick == 6 {
            world.kill_squad(0, Some(1));
        }
        if tick == 10 {
            world.players.insert(0, false);
            world.players.insert(1, false);
        }
    });
    assert_eq!(run.outcome, Some(Outcome::TeamWon { team: 1 }));
}

#[test]
fn both_teams_out_level_on_score_and_lives_is_a_draw() {
    let (_, run) = play(pair_def(0, true), 2, 20, |tick, world| {
        if tick == 4 {
            world.players.insert(0, false);
            world.players.insert(1, false);
        }
    });
    assert!(has(
        &run,
        4,
        Command::Finish {
            outcome: Outcome::Draw
        }
    ));
}

#[test]
fn a_lone_team_of_two_out_of_lives_has_lost() {
    let (_, run) = play(pair_def(0, true), 1, 20, |tick, world| {
        if tick == 4 {
            world.players.insert(0, false);
        }
    });
    assert_eq!(run.outcome, Some(Outcome::Lost));
}

#[test]
fn the_last_round_goes_to_the_higher_score_then_lives_then_a_draw() {
    // Score: team 1's player clears every wave.
    let (_, run) = play(pair_def(5, true), 2, 100, clear_at_age_three(Some(1)));
    assert_eq!(run.outcome, Some(Outcome::TeamWon { team: 1 }));

    // Lives: nobody scores, and team 1 spends a life.
    let mut clear = clear_at_age_three(None);
    let (director, run) = play(pair_def(5, true), 2, 100, move |tick, world| {
        if tick == 4 {
            world.players.insert(1, false);
        }
        clear(tick, world);
    });
    assert_eq!(director.state().scores, [0, 0]);
    assert_eq!(director.state().pools, [5, 4]);
    assert_eq!(run.outcome, Some(Outcome::TeamWon { team: 0 }));

    // Own lives count per team the same way.
    let mut clear = clear_at_age_three(None);
    let (_, run) = play(pair_def(2, false), 4, 100, move |tick, world| {
        if tick == 4 {
            world.players.insert(2, false);
        }
        clear(tick, world);
    });
    assert_eq!(run.outcome, Some(Outcome::TeamWon { team: 1 }));

    // Level on both: a draw.
    let (_, run) = play(pair_def(5, true), 2, 100, clear_at_age_three(None));
    assert_eq!(run.outcome, Some(Outcome::Draw));

    // A lone team that clears the set wins it.
    let (_, run) = play(pair_def(5, true), 1, 100, clear_at_age_three(None));
    assert_eq!(run.outcome, Some(Outcome::TeamWon { team: 0 }));
}

#[test]
fn player_kills_score_across_the_teams_only() {
    let kills = |world: &mut World| {
        world.kill_player(1, Some(0)); // across: scores
        world.kill_player(2, Some(0)); // a team kill
        world.kill_player(3, Some(3)); // a suicide
        world.kill_player(0, None); // a fall
    };
    let (director, run) = play(pair_def(5, true), 4, 2, |tick, world| {
        if tick == 1 {
            kills(world);
        }
    });
    let scores: Vec<_> = run
        .commands
        .iter()
        .filter(|(_, c)| matches!(c, Command::Score { .. }))
        .collect();
    assert_eq!(
        scores,
        [&(
            1,
            Command::Score {
                player: 0,
                points: 40,
                reason: ScoreReason::PlayerKill,
            }
        )]
    );
    assert_eq!(director.state().scores, [40, 0]);

    // One team: a player kill scores nothing.
    let (director, run) = play(small_def(5, true), 4, 2, |tick, world| {
        if tick == 1 {
            kills(world);
        }
    });
    assert!(
        !run.commands
            .iter()
            .any(|(_, c)| matches!(c, Command::Score { .. }))
    );
    assert_eq!(director.state().scores, [0, 0]);
}

#[test]
fn actor_kills_score_for_the_killers_team() {
    let (director, _) = play(pair_def(5, true), 2, 7, |tick, world| {
        if tick == 6 {
            world.kill_squad(0, Some(1));
        }
    });
    // 100 * 1.5 (Legendary) * 1.1 (Famine).
    assert_eq!(director.state().scores, [0, 165]);
}

/// [`pair_def`] with four one-actor squads a wave.
fn four_squads() -> Def {
    let mut def = pair_def(3, true);
    for wave in def.rounds.iter_mut().flat_map(|round| &mut round.waves) {
        wave.squads = vec![wave.squads[0].clone(); 4];
    }
    def
}

/// The targets of the first wave, in placement order.
fn first_wave_targets(players: u8) -> Vec<u8> {
    let (_, run) = play(four_squads(), players, 4, |_, _| {});
    let mut targets = Vec::new();
    for pair in run.commands.windows(2) {
        if let [(_, Command::PlaceSquad { slot, .. }), (_, next)] = pair {
            let Command::Target { slot: target_slot, team } = *next else {
                panic!("no Target after placing slot {slot}: {next:?}");
            };
            assert_eq!(target_slot, *slot);
            targets.push(team);
        }
    }
    targets
}

#[test]
fn squads_are_sent_at_both_teams() {
    assert_eq!(first_wave_targets(4), [0, 1, 0, 1]);
    // With only team 0 present every squad is sent at it.
    assert_eq!(first_wave_targets(1), [0, 0, 0, 0]);
}

#[test]
fn the_team_fewer_actors_press_gets_the_next_squad() {
    // Three actors go at team 0, one at team 1; the third squad goes to
    // team 1, where alternating alone would send it to team 0.
    let mut def = pair_def(3, true);
    let one = def.rounds[0].waves[0].squads[0].clone();
    let mut three = one.clone();
    three.count = Count {
        per_player: 3,
        min: 3,
        max: 3,
    };
    def.rounds[0].waves[0].squads = vec![three, one.clone(), one];
    let (_, run) = play(def, 2, 4, |_, _| {});
    let targets: Vec<u8> = run
        .commands
        .iter()
        .filter_map(|(_, c)| match *c {
            Command::Target { team, .. } => Some(team),
            _ => None,
        })
        .collect();
    assert_eq!(targets, [0, 1, 1]);
}

#[test]
fn a_squad_drops_on_its_target_teams_side() {
    let side = |zones: Vec<firefight::director::ZoneSeen>| {
        let mut director = Director::new(four_squads(), settings(1)).unwrap();
        let mut world = World::new(4, zones);
        run(&mut director, &mut world, 4, |_, _| {}, |_, _, _, _, _| {})
            .commands
            .into_iter()
            .filter_map(|(_, c)| match c {
                Command::PlaceSquad { zone, .. } => Some(zone),
                _ => None,
            })
            .collect::<Vec<u8>>()
    };
    let teamed = |team_distance: [u32; 2]| {
        let mut z = zone(false, 50);
        z.team_distance = team_distance;
        z
    };
    // Zones 0 and 1 are team 0's side, 2 and 3 team 1's; each squad takes
    // the farther of its target's two (the farther half of two is one).
    let zones = vec![
        teamed([10, 90]),
        teamed([20, 80]),
        teamed([90, 10]),
        teamed([80, 20]),
    ];
    assert_eq!(side(zones), [1, 3, 1, 3]);
    // With no zone on a team's side, its squads use them all.
    let zones = vec![teamed([90, 10]), teamed([80, 20])];
    assert_eq!(side(zones), [0, 1, 0, 1]);
}

#[test]
fn one_team_sends_no_target_and_no_team_lives() {
    for index in [0, 1, 2, 4, 5, 6] {
        let (def, run) = random_match(index, |_, director, _, _, _| {
            assert!(director.state().slots.iter().all(|slot| !matches!(
                slot,
                firefight::director::Slot::Placed { target: 1, .. }
            )));
        });
        assert_eq!(def.teams, 1);
        assert!(!run.commands.iter().any(|(_, c)| matches!(
            c,
            Command::Target { .. }
                | Command::TeamLives { .. }
                | Command::Finish {
                    outcome: Outcome::TeamWon { .. } | Outcome::Draw
                }
        )));
    }
}

#[test]
fn a_players_team_is_fixed_when_first_seen() {
    let (director, _) = play(pair_def(3, true), 2, 4, |tick, world| {
        if tick == 1 {
            world.teams.insert(1, 0);
        }
    });
    assert_eq!(director.state().players[&1].team, 1);
}

#[test]
fn two_team_matches_always_end_with_a_team_result() {
    let mut outcomes = std::collections::BTreeSet::new();
    for index in (3..400).step_by(4) {
        let (def, run) = random_match(index, |tick, director, _, _, _| {
            let players = 1 + (index % 16) as u8;
            assert!(director.live_actors() <= director.cap(players), "tick {tick}");
        });
        assert_eq!(def.teams, 2);
        let outcome = run.outcome.expect("every match ends");
        assert!(matches!(
            outcome,
            Outcome::TeamWon { .. } | Outcome::Draw | Outcome::Lost
        ));
        outcomes.insert(format!("{outcome:?}"));
    }
    assert!(outcomes.len() >= 2, "{outcomes:?}");
}

#[test]
fn two_team_commands_have_canonical_integer_json() {
    for (command, json) in [
        (
            Command::Target { slot: 2, team: 1 },
            serde_json::json!({"target": {"slot": 2, "team": 1}}),
        ),
        (
            Command::TeamLives { team: 1, pool: 7 },
            serde_json::json!({"team_lives": {"team": 1, "pool": 7}}),
        ),
        (
            Command::Score {
                player: 3,
                points: 50,
                reason: ScoreReason::PlayerKill,
            },
            serde_json::json!({"score": {"player": 3, "points": 50, "reason": "player_kill"}}),
        ),
        (
            Command::Finish {
                outcome: Outcome::TeamWon { team: 1 },
            },
            serde_json::json!({"finish": {"outcome": {"team_won": {"team": 1}}}}),
        ),
        (
            Command::Finish {
                outcome: Outcome::Draw,
            },
            serde_json::json!({"finish": {"outcome": "draw"}}),
        ),
    ] {
        let value = serde_json::to_value(&command).unwrap();
        assert_eq!(value, json);
        assert_eq!(serde_json::from_value::<Command>(value).unwrap(), command);
    }
}
