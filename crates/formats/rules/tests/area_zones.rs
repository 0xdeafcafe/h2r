//! A zone with an area on the fake game (docs/game/gametype-markers.md,
//! "In a script"): `koth.rules` scores a player inside a path hill and
//! not one in its notch, and a zone with no area is moved by `enter` and
//! `leave` alone.

use game_markers::Area;
use rules::builtin;
use rules::labels::Class;
use rules::scenario::{Scenario, ScenarioLabel, ScenarioPlayer, game_of, step};
use rules::vm::Input;
use rules::Vm;

const HILL: u32 = 50;

/// An L across the floor round (0, 0): its notch is the corner over
/// x > 0 and y > 0.
fn l() -> Area {
    Area::path(vec![
        [-4.0, -4.0],
        [4.0, -4.0],
        [4.0, 0.0],
        [0.0, 0.0],
        [0.0, 4.0],
        [-4.0, 4.0],
    ])
}

fn seat(slot: u8, unit: u32) -> ScenarioPlayer {
    ScenarioPlayer {
        slot,
        team: 0,
        alive: true,
        score: 0,
        lives: 0,
        unit: Some(unit),
        body: Default::default(),
    }
}

#[test]
fn koth_scores_a_player_in_a_path_hill_and_not_one_in_its_notch() {
    let ir = rules::build(builtin::KING).unwrap().0;
    let scenario = Scenario {
        players: vec![seat(0, 100), seat(1, 101)],
        map: Some(vec![ScenarioLabel {
            number: Some(0),
            class: Some(Class::Flag),
            ..ScenarioLabel::new("hill_corner", HILL)
        }]),
        ..Scenario::default()
    };
    let mut game = game_of(&ir, &scenario).unwrap();
    game.carrier = ir.header.engine();
    game.set_area(HILL, [0.0; 3], l());
    let mut vm = Vm::new(ir, 0, 0).unwrap();

    // Player 0 walks into the L's leg; player 1 stands in its notch.
    let first = game.move_to(0, [-2.0, -2.0, 0.0]);
    assert_eq!(first, vec![Input::Enter { zone: HILL, object: 100 }]);
    assert!(game.move_to(1, [2.0, 2.0, 0.0]).is_empty(), "the notch is outside");
    step(&mut vm, &mut game, 0, true, &first);
    for t in 1..70 {
        step(&mut vm, &mut game, t, true, &[]);
    }
    assert!(game.players[&0].score >= 1, "inside: {}", game.players[&0].score);
    assert_eq!(game.players[&1].score, 0, "in the notch");

    // Player 1 steps into the leg and player 0 climbs out of the hill's
    // height: only a point inside the area is in the zone.
    let moved = game.move_to(1, [-3.0, 3.0, 0.0]);
    assert_eq!(moved, vec![Input::Enter { zone: HILL, object: 101 }]);
    let up = game.move_to(0, [-2.0, -2.0, 5.0]);
    assert_eq!(up, vec![Input::Leave { zone: HILL, object: 100 }]);
    // Once the engine has reported it, a move that changes no zone
    // reports nothing.
    for input in moved.iter().chain(&up) {
        game.apply(input);
    }
    assert!(game.move_to(0, [-2.0, -2.0, 5.5]).is_empty());
    assert!(game.move_to(1, [-3.0, 2.0, 0.0]).is_empty());
}

#[test]
fn a_zone_with_no_area_is_moved_by_events_alone() {
    let mut g = rules::fake::FakeGame::with_players(1);
    g.units.insert(100, 0);
    assert!(g.move_to(0, [0.0; 3]).is_empty());
    assert_eq!(g.positions[&0], [0.0; 3]);
}

fn hills() -> Scenario {
    let hill = |number: i32, object: u32| ScenarioLabel {
        number: Some(number),
        class: Some(Class::Flag),
        ..ScenarioLabel::new("hill_corner", object)
    };
    Scenario {
        players: vec![seat(0, 100)],
        map: Some(vec![hill(0, HILL), hill(1, HILL + 1)]),
        ..Scenario::default()
    }
}

/// The hill in play shows its boundary, one area at a time: the last
/// hill's comes off as the hill moves (docs/game/gametype-markers.md,
/// "The boundary in play").
#[test]
fn koth_shows_the_hill_in_play_and_one_area_at_a_time_as_it_moves() {
    let ir = rules::build(builtin::KING).unwrap().0;
    let mut game = game_of(&ir, &hills()).unwrap();
    game.carrier = rules::SCRIPT_INDEX;
    // The hill moves every second: the script's own option, as the record's
    // type area is zero on the script index.
    let options = ir
        .pack_options(&[("move_time".to_string(), rules::ir::OptionValue::Int(1))])
        .unwrap();
    let mut vm = Vm::new(ir, options, 0).unwrap();
    let mut seen = Vec::new();
    for t in 0..200 {
        step(&mut vm, &mut game, t, true, &[]);
        assert!(game.shown.len() <= 1, "tick {t}: {:?}", game.shown);
        assert_eq!(game.shown, vm.shown_areas(), "the fake keeps what the VM does");
        if seen.last() != Some(&game.shown) {
            seen.push(game.shown.clone());
        }
    }
    assert_eq!(
        seen[..3],
        [vec![(HILL, None)], vec![(HILL + 1, None)], vec![(HILL, None)]],
        "{seen:?}"
    );
}

/// A script that never calls `show_area` shows none.
#[test]
fn a_script_that_never_calls_show_area_shows_none() {
    let ir = rules::build(builtin::SLAYER).unwrap().0;
    let mut game = game_of(&ir, &hills()).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    for t in 0..40 {
        step(&mut vm, &mut game, t, true, &[]);
    }
    assert!(game.shown.is_empty());
    assert!(vm.shown_areas().is_empty());
}
