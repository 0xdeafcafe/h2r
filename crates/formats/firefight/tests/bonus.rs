//! The bonus round after a set, Iron, the lives it grants, and the
//! `zero_holds` rule, on the compiled rules script.

mod common;

use std::collections::BTreeSet;

use common::*;
use firefight::director::SQUAD_SLOTS;
use firefight::{
    BONUS_SKULLS, Bonus, Command, Count, Def, Drop, Observed, Outcome, Refusal,
    ScriptDirector, Settings, Skull, SquadSpec, validate,
};

const BONUS_TICKS: u32 = 20;
const START_POOL: u16 = 3;

/// [`small_def`] with a 20-tick bonus round of one weak Elite at a time.
/// A kill is worth 165 here (Legendary, Famine on).
fn bonus_def(pool: u16, table: &[(u32, u16)]) -> Def {
    let mut def = small_def(pool, true);
    def.name = "Bonus".into();
    def.bonus = Some(Bonus {
        ticks: BONUS_TICKS,
        squad: SquadSpec {
            class: "elite".into(),
            count: Count {
                per_player: 1,
                min: 1,
                max: 1,
            },
            drop: Drop::Edge,
            weapon_tier: 0,
        },
        lives_by_points: table.to_vec(),
    });
    def
}

/// `bonus_def` over `rounds` rounds: the later ones schedule no skull.
fn rounds_def(def: &mut Def, rounds: usize) {
    let first = def.rounds[0].clone();
    for _ in 1..rounds {
        let mut round = first.clone();
        round.skulls_on.clear();
        def.rounds.push(round);
    }
}

fn play(
    def: &Def,
    players: u8,
    mut script: impl FnMut(u32, &mut World),
    check: impl FnMut(u32, &ScriptDirector, &World, &Observed, &[Command]),
) -> Run {
    let mut director = ScriptDirector::new(def.clone(), settings(7)).unwrap();
    let mut world = World::new(players, two_zones());
    let run = run(&mut director, &mut world, 600, &mut script, check);
    assert_eq!(director.error(), None);
    let replayed = replay(def, &settings(7), &run.observed);
    assert_eq!(replayed, run.commands, "a fresh script replays the same commands");
    run
}

/// What the fresh script says to the observed ticks.
fn replay(def: &Def, settings: &Settings, observed: &[Observed]) -> Vec<(u32, Command)> {
    let mut script = ScriptDirector::new(def.clone(), settings.clone()).unwrap();
    let mut log = Vec::new();
    for (tick, seen) in observed.iter().enumerate() {
        let tick = tick as u32;
        log.extend(script.step(tick, seen).into_iter().map(|c| (tick, c)));
    }
    assert_eq!(script.error(), None, "{}", def.name);
    log
}

fn no_check(_: u32, _: &ScriptDirector, _: &World, _: &Observed, _: &[Command]) {}

/// The ticks the bonus round starts and ends, in a match where every squad
/// dies to player 0 three ticks after it appears.
fn window(def: &Def, players: u8) -> (u32, u32) {
    let run = play(def, players, kill_at_age_three, no_check);
    window_of(&run.commands)
}

fn window_of(commands: &[(u32, Command)]) -> (u32, u32) {
    let at = |wanted: bool| {
        commands
            .iter()
            .find_map(|(tick, c)| matches!(c, Command::Bonus { on, .. } if *on == wanted).then_some(*tick))
            .unwrap_or_else(|| panic!("no Bonus {{ on: {wanted} }}"))
    };
    (at(true), at(false))
}

/// Kills like [`kill_at_age_three`], but only before the bonus round.
fn kill_until(until: u32) -> impl FnMut(u32, &mut World) {
    move |tick, world| {
        if tick < until {
            kill_at_age_three(tick, world);
        }
    }
}

fn index_of(commands: &[(u32, Command)], wanted: &Command) -> usize {
    commands
        .iter()
        .position(|(_, c)| c == wanted)
        .unwrap_or_else(|| panic!("{wanted:?} is not in the log"))
}

#[test]
fn the_bonus_round_follows_each_set_for_its_ticks() {
    let def = bonus_def(START_POOL, &[]);
    let run = play(&def, 1, kill_at_age_three, no_check);
    let (start, end) = window_of(&run.commands);
    assert_eq!(end - start, BONUS_TICKS);
    let on = Command::Bonus {
        on: true,
        ticks: BONUS_TICKS,
    };
    let off = Command::Bonus { on: false, ticks: 0 };
    assert_eq!(run.commands.iter().filter(|(_, c)| *c == on).count(), 1);
    assert_eq!(run.commands.iter().filter(|(_, c)| *c == off).count(), 1);

    // It comes after the last wave of the last round, nothing of the
    // round's own is between, and the squad refills while it runs.
    let before: Vec<_> = run.commands.iter().filter(|(t, _)| *t < start).collect();
    assert!(before.iter().any(|(_, c)| *c == Command::Wave { index: 4 }));
    let placed = run
        .commands
        .iter()
        .filter(|(t, c)| (start..end).contains(t) && matches!(c, Command::PlaceSquad { .. }))
        .count();
    assert!(placed > 4, "refilled past the cap by kills: {placed}");
    assert!(
        run.commands
            .iter()
            .filter(|(t, _)| (start..=end).contains(t))
            .all(|(_, c)| !matches!(c, Command::Wave { .. } | Command::Round { .. })),
        "the bonus round has no wave"
    );
    // It ends the set: the game finishes after it, not before.
    assert_eq!(run.outcome, Some(Outcome::Won));
    let finish = index_of(&run.commands, &Command::Finish { outcome: Outcome::Won });
    assert!(finish > index_of(&run.commands, &off));
    assert!(run.commands[finish].0 >= end);
}

#[test]
fn a_definition_without_a_bonus_round_has_none() {
    let run = play(&small_def(START_POOL, true), 1, kill_at_age_three, no_check);
    assert!(run.commands.iter().all(|(_, c)| !matches!(c, Command::Bonus { .. })));
    assert!(run.commands.iter().all(|(_, c)| {
        !matches!(c, Command::Skull { id: Skull::Iron, .. })
    }));
    assert_eq!(run.outcome, Some(Outcome::Won));
}

#[test]
fn a_bonus_death_costs_no_life_and_holds_the_respawn() {
    let def = bonus_def(START_POOL, &[]);
    let (start, end) = window(&def, 2);
    let death = start + 5;
    let run = play(
        &def,
        2,
        |tick, world| {
            if tick < start {
                kill_at_age_three(tick, world);
            }
            if tick == death {
                world.kill_player(1, None);
            }
        },
        no_check,
    );
    assert_eq!(window_of(&run.commands), (start, end));
    let respawns: Vec<_> = run
        .commands
        .iter()
        .filter(|(_, c)| matches!(c, Command::Respawn { .. }))
        .collect();
    assert!(
        respawns.iter().all(|(t, _)| *t >= end),
        "no respawn while the bonus round runs: {respawns:?}"
    );
    // At the end it comes back for free: no life was spent for it, and the
    // pool the match started with is the one the log last shows.
    let off = index_of(&run.commands, &Command::Bonus { on: false, ticks: 0 });
    let back = index_of(&run.commands, &Command::Respawn { player: 1 });
    assert!(back > off);
    assert_eq!(run.commands[back].0, end);
    assert!(
        run.commands
            .iter()
            .filter(|(t, _)| *t >= start)
            .all(|(_, c)| !matches!(c, Command::Lives { .. })),
        "no life is spent or granted"
    );
}

/// `kills` of the squads the bonus round has placed die once they are all
/// out, six ticks in.
fn bonus_kills(start: u32, kills: usize) -> impl FnMut(u32, &mut World) {
    move |tick, world| {
        if tick < start {
            kill_at_age_three(tick, world);
        } else if tick == start + 6 {
            let alive: Vec<usize> = (0..SQUAD_SLOTS).filter(|&s| world.squads[s].0 > 0).collect();
            for &slot in alive.iter().take(kills) {
                world.kill_squad(slot, Some(0));
            }
        }
    }
}

#[test]
fn bonus_points_grant_lives_by_the_table() {
    let def = bonus_def(START_POOL, &[(150, 1), (500, 3)]);
    let (start, _) = window(&def, 1);
    // 165 points a kill: (kills, lives granted by the last row reached).
    for (kills, granted) in [(0, 0), (1, 1), (3, 1), (4, 3)] {
        let run = play(&def, 1, bonus_kills(start, kills), no_check);
        let off = index_of(&run.commands, &Command::Bonus { on: false, ticks: 0 });
        let after = &run.commands[off..];
        let lives = after.iter().find_map(|(_, c)| match c {
            Command::Lives { player: None, pool } => Some(*pool),
            _ => None,
        });
        let expected = (granted > 0).then_some(START_POOL + granted);
        assert_eq!(lives, expected, "{kills} kills");
        let score: u32 = run
            .commands
            .iter()
            .filter(|(t, _)| *t > start)
            .filter_map(|(_, c)| match c {
                Command::Score { points, .. } => Some(*points),
                _ => None,
            })
            .sum();
        assert_eq!(score, 165 * kills as u32, "{kills} kills");
    }
}

#[test]
fn bonus_points_grant_each_players_own_lives_when_not_shared() {
    let mut def = bonus_def(START_POOL, &[(150, 2)]);
    def.lives.shared = false;
    let (start, _) = window(&def, 2);
    let run = play(&def, 2, bonus_kills(start, 1), no_check);
    let off = index_of(&run.commands, &Command::Bonus { on: false, ticks: 0 });
    let granted: BTreeSet<(u8, u16)> = run.commands[off..]
        .iter()
        .filter_map(|(_, c)| match c {
            Command::Lives { player: Some(p), pool } => Some((*p, *pool)),
            _ => None,
        })
        .collect();
    assert_eq!(granted, BTreeSet::from([(0, START_POOL + 2), (1, START_POOL + 2)]));
}

#[test]
fn all_dead_ends_the_bonus_round_not_the_game() {
    // No lives at all: everyone dead in a round would lose the match.
    let def = bonus_def(0, &[]);
    let (start, end) = window(&def, 1);
    let death = start + 4;
    let run = play(
        &def,
        1,
        |tick, world| {
            if tick < start {
                kill_at_age_three(tick, world);
            }
            if tick == death {
                world.kill_player(0, None);
            }
        },
        no_check,
    );
    let (_, ended) = window_of(&run.commands);
    assert_eq!(ended, death, "it ends the tick everyone is seen dead");
    assert!(ended < end);
    assert_ne!(run.outcome, Some(Outcome::Lost));
    assert_eq!(run.outcome, Some(Outcome::Won));
    let off = index_of(&run.commands, &Command::Bonus { on: false, ticks: 0 });
    assert!(index_of(&run.commands, &Command::Respawn { player: 0 }) > off);
}

#[test]
fn every_skull_is_on_in_the_bonus_round_and_restored_after() {
    let def = bonus_def(START_POOL, &[]);
    let (start, end) = window(&def, 1);
    let mut seen = Vec::new();
    let run = play(&def, 1, kill_until(start), |tick, director, _, _, _| {
        seen.push((tick, director.state().skulls));
    });
    let every: BTreeSet<Skull> = BONUS_SKULLS
        .iter()
        .copied()
        .chain([Skull::Iron, Skull::Famine])
        .collect();
    let during = &seen.iter().find(|(t, _)| *t == start + 1).unwrap().1;
    assert_eq!(during, &every);
    let after = &seen.iter().find(|(t, _)| *t == end).unwrap().1;
    // The set's own skulls (Famine, from round one) are the ones left.
    assert_eq!(after, &BTreeSet::from([Skull::Famine]));

    let switched = |on: bool| -> BTreeSet<Skull> {
        run.commands
            .iter()
            .filter_map(|(_, c)| match c {
                Command::Skull { id, on: to } if *to == on => Some(*id),
                _ => None,
            })
            .filter(|id| *id != Skull::Famine || on)
            .collect()
    };
    // Famine came on with round one, before the bonus round.
    let mut turned_on = BONUS_SKULLS.to_vec();
    turned_on.push(Skull::Iron);
    assert_eq!(
        switched(false),
        turned_on.iter().copied().collect::<BTreeSet<_>>(),
        "exactly the skulls it switched on go off"
    );
    assert!(switched(true).is_superset(&turned_on.iter().copied().collect()));
}

#[test]
fn iron_means_no_respawn_whatever_the_pool() {
    let def = bonus_def(50, &[]);
    let (start, end) = window(&def, 2);
    let run = play(
        &def,
        2,
        |tick, world| {
            if tick < start {
                kill_at_age_three(tick, world);
            }
            if (start + 2..start + 12).contains(&tick) && tick % 3 == 0 {
                world.kill_player(1, None);
            }
        },
        no_check,
    );
    assert!(run
        .commands
        .iter()
        .filter(|(t, _)| (start..end).contains(t))
        .all(|(_, c)| !matches!(c, Command::Respawn { .. } | Command::Lives { .. })));
}

#[test]
fn at_zero_a_dead_player_waits_for_the_grant() {
    let deaths = |tick: u32, world: &mut World| {
        kill_at_age_three(tick, world);
        // The pool of one pays for the first; the second finds it empty.
        if tick == 3 || tick == 4 {
            world.kill_player(1, None);
        }
    };
    let grant = |commands: &[(u32, Command)]| {
        commands
            .iter()
            .find_map(|(t, c)| matches!(c, Command::Lives { pool: 2, .. }).then_some(*t))
            .expect("the round's end grants lives")
    };
    let last_respawn = |commands: &[(u32, Command)]| {
        commands
            .iter()
            .rev()
            .find_map(|(t, c)| matches!(c, Command::Respawn { player: 1 }).then_some(*t))
            .expect("the player comes back")
    };
    let waves_between = |commands: &[(u32, Command)], to: u32| {
        commands
            .iter()
            .filter(|(t, c)| (5..to).contains(t) && matches!(c, Command::Wave { .. }))
            .count()
    };
    let hold = |zero_holds: bool| {
        let mut def = small_def(1, true);
        def.lives.per_round_bonus = 2;
        def.lives.zero_holds = zero_holds;
        rounds_def(&mut def, 2);
        play(&def, 2, deaths, no_check).commands
    };

    let held = hold(true);
    let at = grant(&held);
    assert!(waves_between(&held, at) >= 1, "waves end before the round does");
    assert!(
        last_respawn(&held) >= at,
        "held until the grant at {at}, not the wave's end"
    );
    assert!(held
        .iter()
        .filter(|(t, c)| *t > 4 && *t < at && matches!(c, Command::Respawn { .. }))
        .count()
        == 0);

    // Without the hold the wave's end brings the player back free.
    let free = hold(false);
    let first = free
        .iter()
        .filter(|(t, _)| *t > 4)
        .find_map(|(t, c)| matches!(c, Command::Respawn { player: 1 }).then_some(*t))
        .unwrap();
    assert!(first < grant(&free));
}

#[test]
fn zero_holds_still_loses_when_everyone_is_out() {
    let mut def = small_def(0, true);
    def.lives.zero_holds = true;
    let run = play(
        &def,
        1,
        |tick, world| {
            kill_at_age_three(tick, world);
            if tick == 3 {
                world.kill_player(0, None);
            }
        },
        no_check,
    );
    assert_eq!(run.outcome, Some(Outcome::Lost));
}

#[test]
fn bonus_rounds_give_the_same_commands() {
    for seed in 0..48u64 {
        let mut def = bonus_def(START_POOL, &[(100, 1), (400, 2), (900, 4)]);
        rounds_def(&mut def, 1 + (seed % 2) as usize);
        def.lives.zero_holds = seed % 3 == 0;
        def.lives.shared = seed % 5 != 0;
        let players = 1 + (seed % 3) as u8;
        let mut director = ScriptDirector::new(def.clone(), settings(seed)).unwrap();
        let mut world = World::new(players, eight_zones());
        let run = run(&mut director, &mut world, 4_000, random_script(seed), no_check);
        assert_eq!(director.error(), None, "seed {seed}");
        assert_eq!(
            replay(&def, &settings(seed), &run.observed),
            run.commands,
            "seed {seed}"
        );
    }
}

#[test]
fn two_teams_each_earn_their_own_bonus_lives() {
    let mut def = pair_def(START_POOL, true);
    def.bonus = bonus_def(START_POOL, &[(150, 2)]).bonus;
    let (start, _) = window(&def, 2);
    let run = play(&def, 2, bonus_kills(start, 1), no_check);
    let off = index_of(&run.commands, &Command::Bonus { on: false, ticks: 0 });
    let pools: Vec<(u8, u16)> = run.commands[off..]
        .iter()
        .filter_map(|(_, c)| match c {
            Command::TeamLives { team, pool } => Some((*team, *pool)),
            _ => None,
        })
        .collect();
    // Player 0 (team 0) scored the one kill; team 1 scored nothing.
    assert_eq!(pools, [(0, START_POOL + 2)]);
}

#[test]
fn the_bonus_definition_is_checked() {
    let limits = firefight::Limits::new(["elite"]);
    let good = bonus_def(START_POOL, &[(100, 1), (200, 2)]);
    validate(&good, &limits).unwrap();
    assert_eq!(
        Def::from_bytes(good.canonical_json().unwrap().as_bytes(), &limits).unwrap(),
        good
    );

    let cases: [(&str, fn(&mut Def)); 7] = [
        ("time", |d| d.bonus.as_mut().unwrap().ticks = 0),
        ("time", |d| d.bonus.as_mut().unwrap().ticks = 9_001),
        ("table order", |d| d.bonus.as_mut().unwrap().lives_by_points = vec![(200, 1), (100, 2)]),
        ("zero lives", |d| d.bonus.as_mut().unwrap().lives_by_points = vec![(100, 0)]),
        ("rows", |d| {
            d.bonus.as_mut().unwrap().lives_by_points = (1..=7).map(|n| (n * 10, 1)).collect()
        }),
        ("class", |d| d.bonus.as_mut().unwrap().squad.class = "grunt".into()),
        ("over the cap", |d| {
            d.bonus.as_mut().unwrap().squad.count = Count {
                per_player: 2,
                min: 1,
                max: 8,
            }
        }),
    ];
    for (what, change) in cases {
        let mut def = good.clone();
        change(&mut def);
        assert!(validate(&def, &limits).is_err(), "{what}");
    }

    // Iron is the bonus round's alone.
    let mut def = good.clone();
    def.rounds[0].skulls_on.push(Skull::Iron);
    assert!(matches!(validate(&def, &limits), Err(Refusal::Invalid { field: "skulls_on", .. })));
    let mut def = good;
    def.scoring.skull_multipliers.insert(Skull::Iron, 1_100);
    assert!(validate(&def, &limits).is_err());
}

#[test]
fn a_definition_with_no_bonus_keeps_its_canonical_json() {
    let json = small_def(START_POOL, true).canonical_json().unwrap();
    assert!(!json.contains("bonus") && !json.contains("zero_holds"));
}
