//! Sets: three rounds, then the next set, without end; round skulls that
//! clear at a new set, set skulls that arrive at sets 2 to 4 and stay;
//! drops by zone role; and the script's commands over a replay.

mod common;

use std::collections::BTreeSet;

use common::*;
use firefight::director::ZoneSeen;
use firefight::{
    Command, Count, Def, Drop, Observed, Outcome, ScriptDirector, SetRules, Settings, Skull,
    ZoneRole, builtin,
};

/// Three one-round copies of [`small_def`]'s round, each with its own round
/// skull, played again and again; the set skulls are Thunderstorm,
/// Famine and Mythic.
fn endless_def() -> Def {
    let mut def = small_def(5, true);
    def.name = "Endless Golden".into();
    let round = def.rounds[0].clone();
    def.rounds = [Skull::Ghost, Skull::Catch, Skull::BlackEye]
        .map(|skull| {
            let mut round = round.clone();
            round.skulls_on = vec![skull];
            round
        })
        .to_vec();
    def.set = SetRules {
        endless: true,
        scale: 0,
        skulls: vec![Skull::Thunderstorm, Skull::Famine, Skull::Mythic],
    };
    def
}

fn play(def: &Def, settings: &Settings, limit: u32, players: u8, zones: Vec<ZoneSeen>) -> Run {
    let mut director = ScriptDirector::new(def.clone(), settings.clone()).unwrap();
    let mut world = World::new(players, zones);
    run(&mut director, &mut world, limit, kill_at_age_three, |_, _, _, _, _| {})
}

fn commands(run: &Run) -> Vec<Command> {
    run.commands.iter().map(|(_, c)| c.clone()).collect()
}

fn on(id: Skull) -> Command {
    Command::Skull { id, on: true }
}

fn off(id: Skull) -> Command {
    Command::Skull { id, on: false }
}

fn sets_and_skulls(run: &Run) -> Vec<Command> {
    commands(run)
        .into_iter()
        .filter(|c| matches!(c, Command::Set { .. } | Command::Round { .. } | Command::Skull { .. }))
        .collect()
}

#[test]
fn a_set_is_three_rounds_and_the_next_begins() {
    let run = play(&endless_def(), &settings(1), 600, 1, two_zones());
    let marks: Vec<Command> = commands(&run)
        .into_iter()
        .filter(|c| matches!(c, Command::Set { .. } | Command::Round { .. }))
        .collect();
    let set = |index| Command::Set { index };
    let round = |index| Command::Round { index };
    assert_eq!(
        marks[..8],
        [
            set(0),
            round(0),
            round(1),
            round(2),
            set(1),
            round(0),
            round(1),
            round(2),
        ]
    );
    assert_eq!(run.outcome, None, "an endless game does not end by clearing rounds");
}

#[test]
fn a_finite_set_has_no_set_commands() {
    let run = play(&small_def(1, true), &settings(1), 100, 1, two_zones());
    assert_eq!(run.outcome, Some(Outcome::Won));
    assert!(!commands(&run).iter().any(|c| matches!(c, Command::Set { .. })));
}

#[test]
fn round_skulls_clear_at_a_new_set() {
    let run = play(&endless_def(), &settings(1), 600, 1, two_zones());
    let all = sets_and_skulls(&run);
    let start = all
        .iter()
        .position(|c| *c == Command::Set { index: 1 })
        .expect("set 2 begins");
    // The round skulls go off in the director's skull order, the set's own
    // skull comes on, and the first round of the set switches its skull on
    // again.
    assert_eq!(
        all[start..start + 7],
        [
            Command::Set { index: 1 },
            off(Skull::BlackEye),
            off(Skull::Ghost),
            off(Skull::Catch),
            on(Skull::Thunderstorm),
            Command::Round { index: 0 },
            on(Skull::Ghost),
        ]
    );
}

#[test]
fn a_starting_skull_survives_a_new_set() {
    let started = Settings {
        skulls: BTreeSet::from([Skull::Ghost]),
        ..settings(1)
    };
    let run = play(&endless_def(), &started, 600, 1, two_zones());
    assert!(
        !commands(&run).contains(&off(Skull::Ghost)),
        "a skull the match began with is no round's to switch off"
    );
    assert!(commands(&run).contains(&off(Skull::Catch)));
}

#[test]
fn set_skulls_arrive_at_sets_two_three_four_and_stay() {
    let run = play(&endless_def(), &settings(1), 1_500, 1, two_zones());
    let all = sets_and_skulls(&run);
    let set_skull = |set: u8| {
        let at = all
            .iter()
            .position(|c| *c == Command::Set { index: set })
            .unwrap_or_else(|| panic!("set {} never began", set + 1));
        all[at + 1..]
            .iter()
            .take_while(|c| !matches!(c, Command::Round { .. }))
            .filter(|c| matches!(c, Command::Skull { on: true, .. }))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(set_skull(1), [on(Skull::Thunderstorm)]);
    assert_eq!(set_skull(2), [on(Skull::Famine)]);
    assert_eq!(set_skull(3), [on(Skull::Mythic)]);
    assert_eq!(set_skull(4), [], "none arrives after the list ends");
    for kept in [Skull::Thunderstorm, Skull::Famine, Skull::Mythic] {
        assert!(!commands(&run).contains(&off(kept)), "{kept:?} stays on");
    }
}

#[test]
fn each_set_grows_the_squads_up_to_the_cap() {
    let mut def = endless_def();
    def.set.scale = 500;
    for round in &mut def.rounds {
        for wave in &mut round.waves {
            wave.squads[0].count = Count {
                per_player: 0,
                min: 2,
                max: 2,
            };
        }
    }
    let run = play(&def, &settings(1), 1_500, 1, two_zones());
    let mut set = 0;
    let mut sizes = [None; 4];
    for command in commands(&run) {
        match command {
            Command::Set { index } => set = usize::from(index),
            Command::PlaceSquad { count, .. } if set < 4 => {
                assert_eq!(*sizes[set].get_or_insert(count), count);
            }
            _ => {}
        }
    }
    // 2, then +50 percent a set, held at the cap of 4.
    assert_eq!(sizes, [Some(2), Some(3), Some(4), Some(4)]);
}

#[test]
fn blind_is_not_a_firefight_skull() {
    assert!(serde_json::from_str::<Skull>("\"blind\"").is_err());
    assert_eq!(serde_json::from_str::<Skull>("\"black_eye\"").unwrap(), Skull::BlackEye);
    assert_eq!(Skull::BlackEye as u8, 8);
    for (file, text) in builtin::SETS {
        assert!(!text.contains("blind"), "{file}");
    }
    for def in builtin::definitions() {
        assert!(!def.canonical_json().unwrap().contains("blind"), "{}", def.name);
    }
}

fn zone_with(seen: bool, distance: u32, roles: &[ZoneRole]) -> ZoneSeen {
    ZoneSeen {
        roles: roles.iter().fold(0, |mask, role| mask | role.bit()),
        ..zone(seen, distance)
    }
}

fn placed_zones(run: &Run) -> Vec<u8> {
    commands(run)
        .into_iter()
        .filter_map(|c| match c {
            Command::PlaceSquad { zone, .. } => Some(zone),
            _ => None,
        })
        .collect()
}

#[test]
fn a_role_drop_picks_only_zones_with_the_role() {
    let mut def = small_def(1, true);
    for wave in &mut def.rounds[0].waves {
        wave.squads[0].drop = Drop::Role(ZoneRole::Heavy);
    }
    // Zone 1 is the only unseen heavy zone, and the nearest zone and the
    // farthest unseen one are both line zones.
    let zones = vec![
        zone_with(false, 10, &[ZoneRole::Line]),
        zone_with(false, 40, &[ZoneRole::Heavy]),
        zone_with(true, 99, &[ZoneRole::Heavy, ZoneRole::Flank]),
        zone_with(false, 95, &[ZoneRole::Line, ZoneRole::Flank]),
    ];
    let run = play(&def, &settings(3), 100, 1, zones);
    assert_eq!(run.outcome, Some(Outcome::Won));
    assert_eq!(placed_zones(&run), [1; 5]);

    // With every heavy zone in sight, the choice is among the heavy zones.
    let zones = vec![
        zone_with(false, 10, &[ZoneRole::Line]),
        zone_with(true, 40, &[ZoneRole::Heavy]),
        zone_with(true, 99, &[ZoneRole::Heavy]),
        zone_with(false, 95, &[ZoneRole::Line]),
    ];
    let run = play(&def, &settings(3), 100, 1, zones);
    assert!(placed_zones(&run).iter().all(|&zone| zone == 1 || zone == 2));
    assert_eq!(placed_zones(&run).len(), 5);
}

#[test]
fn an_endless_game_only_ends_lost() {
    let def = endless_def();
    let still = play(&def, &settings(1), 1_500, 1, two_zones());
    assert_eq!(still.outcome, None);
    assert!(
        !commands(&still)
            .iter()
            .any(|c| matches!(c, Command::Finish { .. })),
        "clearing sets finishes nothing"
    );

    // With no lives, the only player dies in the second set's fighting.
    let mut empty = def.clone();
    empty.lives.pool_start = 0;
    let mut director = ScriptDirector::new(empty, settings(1)).unwrap();
    let mut world = World::new(1, two_zones());
    let lost = run(
        &mut director,
        &mut world,
        1_500,
        |tick, world| {
            kill_at_age_three(tick, world);
            if tick >= 300 && world.live() > 0 && world.players[&0] {
                world.kill_player(0, None);
            }
        },
        |_, _, _, _, _| {},
    );
    assert_eq!(lost.outcome, Some(Outcome::Lost));
    assert!(
        commands(&lost).contains(&Command::Set { index: 1 }),
        "it ended after a set was cleared"
    );
}

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

#[test]
fn sets_give_the_same_commands() {
    // The director is the script alone: a fresh script over what a match
    // observed gives the commands the match was played with, across sets,
    // skull resets and scaling.
    let mut def = endless_def();
    def.set.scale = 250;
    for seed in 0..4 {
        let settings = settings(seed);
        let run = play(&def, &settings, 900, 1 + seed as u8, two_zones());
        assert!(commands(&run).contains(&Command::Set { index: 2 }));
        assert_eq!(replay(&def, &settings, &run.observed), run.commands);
    }
    let endless = builtin::definitions().into_iter().last().unwrap();
    assert_eq!(endless.name, "Endless");
    let quick = quick_builtins().into_iter().last().unwrap();
    let mut director = ScriptDirector::new(quick.clone(), settings(5)).unwrap();
    let mut world = World::new(3, eight_zones());
    let run = run(&mut director, &mut world, 40_000, random_script(5), |_, _, _, _, _| {});
    assert_eq!(replay(&quick, &settings(5), &run.observed), run.commands);
}

#[test]
fn a_role_no_zone_carries_is_refused_by_name() {
    let mut def = small_def(1, true);
    def.rounds[0].waves[1].squads[0].drop = Drop::Role(ZoneRole::Heavy);
    let kit = firefight::Limits::new(["elite"]);
    firefight::validate(&def, &kit).unwrap();
    let line_only = kit.with_zone_roles(ZoneRole::Line.bit() | ZoneRole::Flank.bit());
    assert_eq!(
        firefight::validate(&def, &line_only),
        Err(firefight::Refusal::Invalid {
            field: "drop",
            reason: "a squad's zone role is carried by no zone of the kit",
        })
    );
}
