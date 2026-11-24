//! What the built-in objective scripts put on the show state for the HUD
//! (docs/game/rules-world-api.md, "The HUDs a script feeds"): the mark over
//! a thing that moves, and the Territories count that is each viewer's own.
//! Run on the fake game, read the way a viewer's HUD reads it
//! (`rules::show::view`).

use rules::builtin;
use rules::fake::FakeGame;
use rules::show::{Viewer, view};
use rules::vm::Placed;
use rules::{Fired, Input, Vm, build};

const BALL: u32 = 100;
const FLAG: u32 = 21;

fn start(src: &str, g: &mut FakeGame, options: &[(&str, i64)]) -> Vm {
    let (ir, report) = build(src).unwrap();
    assert!(report.fits());
    let given: Vec<(String, rules::ir::OptionValue)> = options
        .iter()
        .map(|(k, v)| (k.to_string(), rules::ir::OptionValue::Int(*v)))
        .collect();
    let bits = if options.is_empty() {
        ir.default_options()
    } else {
        ir.pack_options(&given).unwrap()
    };
    let mut vm = Vm::new(ir, bits, 5).unwrap();
    vm.begin_tick(1);
    assert_eq!(vm.fire(g, Input::RoundStart), Fired::Ran);
    vm
}

fn ticks(vm: &mut Vm, g: &mut FakeGame, from: u32, n: u32) {
    for t in from..from + n {
        vm.begin_tick(t);
        assert_ne!(vm.update(g), Fired::Stopped);
    }
}

/// The object a viewer's navpoint `slot` marks, if any.
fn marked(vm: &Vm, slot: &str, viewer: u8) -> Option<u32> {
    let viewer = Viewer { slot: Some(viewer), team: None };
    view(vm.show(), vm.ir(), viewer).waypoint_at(slot).map(|w| w.at)
}

#[test]
fn the_oddball_mark_follows_the_ball_lying_then_carried_then_gone() {
    let mut g = FakeGame::with_players(2);
    g.labelled.insert("oddball_spawn".into(), vec![Placed { placement: 0, object: 40 }]);
    g.variant.insert("oddball_ball_count".into(), 1);
    let mut vm = start(builtin::ODDBALL, &mut g, &[]);

    // No ball registered: nothing to mark.
    ticks(&mut vm, &mut g, 2, 1);
    assert_eq!(marked(&vm, "odd.mark", 0), None);

    g.objectives.insert(BALL, None);
    ticks(&mut vm, &mut g, 3, 1);
    assert_eq!(marked(&vm, "odd.mark", 0), Some(BALL), "lying");

    vm.fire(&mut g, Input::Pickup(1, BALL));
    g.objectives.insert(BALL, Some(1));
    ticks(&mut vm, &mut g, 4, 1);
    assert_eq!(marked(&vm, "odd.mark", 0), Some(BALL), "carried");
    assert_eq!(marked(&vm, "odd.mark", 1), Some(BALL), "the carrier sees it too");

    g.objectives.clear();
    ticks(&mut vm, &mut g, 5, 1);
    assert_eq!(marked(&vm, "odd.mark", 0), None, "gone");
}

#[test]
fn the_ctf_mark_is_over_a_flag_that_is_away_and_not_one_at_home() {
    let mut g = FakeGame::with_players(2);
    g.players.get_mut(&1).unwrap().team = 1;
    g.variant.insert("engine".into(), 1);
    g.variant.insert("team_play".into(), 1);
    g.labelled.insert(
        "ctf_flag_spawn".into(),
        vec![Placed { placement: 0, object: 10 }, Placed { placement: 1, object: 11 }],
    );
    g.object_teams.extend([(10, 0), (11, 1)]);
    let mut vm = start(builtin::CTF, &mut g, &[]);
    g.objectives.insert(FLAG, None);
    g.object_teams.insert(FLAG, 1);

    // Both at home: no mark.
    ticks(&mut vm, &mut g, 2, 1);
    assert_eq!(marked(&vm, "ctf.mark", 0), None);

    // Team 1's flag is taken: the mark is over it, and the bomb's is not.
    vm.fire(&mut g, Input::Pickup(0, FLAG));
    g.objectives.insert(FLAG, Some(0));
    ticks(&mut vm, &mut g, 3, 1);
    assert_eq!(marked(&vm, "ctf.mark", 0), Some(FLAG));
    assert_eq!(marked(&vm, "asl.mark", 0), None);

    // Back at its spawn, the mark goes.
    g.inside.insert((0, 10));
    ticks(&mut vm, &mut g, 4, 1);
    g.inside.clear();
    ticks(&mut vm, &mut g, 5, 1);
    assert_eq!(marked(&vm, "ctf.mark", 0), None);
}

/// The Territories count a viewer reads (`ter.owned`'s value).
fn owned(vm: &Vm, viewer: u8) -> Option<i32> {
    let viewer = Viewer { slot: Some(viewer), team: None };
    view(vm.show(), vm.ir(), viewer).at("ter.owned").map(|w| w.value)
}

#[test]
fn the_territories_count_is_those_the_viewers_side_holds() {
    let mut g = FakeGame::with_players(3);
    g.players.get_mut(&1).unwrap().team = 1;
    g.carrier = rules::SCRIPT_INDEX;
    g.variant.insert("team_play".into(), 0);
    g.labelled.insert(
        "territory".into(),
        vec![Placed { placement: 0, object: 600 }, Placed { placement: 1, object: 601 }],
    );
    let mut vm = start(builtin::TERRITORIES, &mut g, &[("in_play", 2), ("contest_time", 1)]);

    // Nobody holds anything: every viewer reads 0.
    ticks(&mut vm, &mut g, 2, 1);
    assert_eq!([0, 1, 2].map(|v| owned(&vm, v)), [Some(0); 3]);

    // Player 0 takes 600, and with no teams only player 0 holds it.
    g.inside.insert((0, 600));
    ticks(&mut vm, &mut g, 3, 90);
    g.inside.clear();
    assert_eq!([0, 1, 2].map(|v| owned(&vm, v)), [Some(1), Some(0), Some(0)]);

    // With teams, player 2 (team 0, like player 0) holds it too; player 1
    // (team 1) does not.
    g.variant.insert("team_play".into(), 1);
    ticks(&mut vm, &mut g, 93, 1);
    assert_eq!([0, 1, 2].map(|v| owned(&vm, v)), [Some(1), Some(0), Some(1)]);

    // Player 1 takes 601: the sides' counts are their own.
    g.inside.insert((1, 601));
    ticks(&mut vm, &mut g, 94, 90);
    assert_eq!([0, 1, 2].map(|v| owned(&vm, v)), [Some(1), Some(1), Some(1)]);
    // The total, shared, is the territories in play.
    let seen = view(vm.show(), vm.ir(), Viewer { slot: Some(1), team: Some(1) });
    assert_eq!(seen.at("ter.owned").map(|w| (w.style, w.shown)), Some((2, true)));
}
