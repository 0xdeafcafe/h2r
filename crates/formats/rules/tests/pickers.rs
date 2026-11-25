//! Pickers (docs/game/rules-world-api.md, "Pickers"): `offer_pick`,
//! `pick_row` and `picked` over the fake game, the host's report
//! (`Vm::choose`), the state and the limits. Every invented fixture.

use rules::fake::FakeGame;
use rules::scenario::{self, Scenario, ScenarioPick};
use rules::vm::{Choice, ErrorKind, Input, MAX_PICK_ROWS, MAX_PICKS, PickOffer, RulesExt};
use rules::{Severity, Vm, compile, diagnose};

/// Offers one picker of three rows at the round's start, and records each
/// player's row as it spawns.
const PICKER: &str = "rules \"Picker\" 1
type slayer
requires player.pick
player var row: int = -9
on round_start {
    offer_pick(0, \"LOADOUT\")
    pick_row(0, \"A\")
    pick_row(0, \"B\")
    pick_row(0, \"C\")
}
on spawn(p) {
    p.row = picked(p, 0)
}
";

fn started() -> (Vm, FakeGame) {
    let mut g = FakeGame::with_players(2);
    let mut vm = Vm::new(compile(PICKER).unwrap(), 0, 0).unwrap();
    vm.begin_tick(0);
    vm.fire(&mut g, Input::RoundStart);
    (vm, g)
}

fn words(vm: &Vm) -> Vec<(u8, String, Vec<String>)> {
    let text = |id: u16| vm.string(id).unwrap().to_owned();
    vm.picks()
        .iter()
        .map(|o| (o.slot, text(o.title), o.rows.iter().map(|&r| text(r)).collect()))
        .collect()
}

#[test]
fn the_offers_are_in_the_state_for_the_host_to_open() {
    let (vm, g) = started();
    assert_eq!(g.script_error, None);
    assert_eq!(words(&vm), [(0, "LOADOUT".to_string(), vec!["A".to_string(), "B".to_string(), "C".to_string()])]);
    assert!(vm.choices().is_empty());
}

#[test]
fn picked_reads_the_row_a_player_chose_and_minus_one_before() {
    let (mut vm, mut g) = started();
    vm.begin_tick(1);
    vm.fire(&mut g, Input::Spawn(0));
    vm.fire(&mut g, Input::Spawn(1));
    assert_eq!((vm.player_var(0, "row"), vm.player_var(1, "row")), (Some(-1), Some(-1)), "none made");
    assert!(vm.choose(0, 0, 2));
    vm.begin_tick(2);
    vm.fire(&mut g, Input::Spawn(0));
    vm.fire(&mut g, Input::Spawn(1));
    assert_eq!((vm.player_var(0, "row"), vm.player_var(1, "row")), (Some(2), Some(-1)));
    // A second pick replaces the first; each player's is their own.
    assert!(vm.choose(0, 0, 1));
    assert!(vm.choose(1, 0, 0));
    assert_eq!(vm.choices().len(), 2);
    vm.begin_tick(3);
    vm.fire(&mut g, Input::Spawn(0));
    vm.fire(&mut g, Input::Spawn(1));
    assert_eq!((vm.player_var(0, "row"), vm.player_var(1, "row")), (Some(1), Some(0)));
    assert_eq!(g.script_error, None);
}

#[test]
fn a_pick_the_vm_cannot_hold_is_refused() {
    let (mut vm, _) = started();
    assert!(!vm.choose(0, 0, 3), "a fourth row of three");
    assert!(!vm.choose(0, 1, 0), "a picker never offered");
    assert!(!vm.choose(16, 0, 0), "a player outside the roster");
    assert!(vm.choices().is_empty());
}

#[test]
fn the_rounds_end_takes_the_offers_and_the_picks() {
    let (mut vm, mut g) = started();
    assert!(vm.choose(0, 0, 1));
    vm.begin_tick(5);
    vm.fire(&mut g, Input::RoundEnd);
    assert!(vm.picks().is_empty() && vm.choices().is_empty());
    assert!(!vm.choose(0, 0, 1), "nothing is offered now");
}

#[test]
fn offering_a_picker_again_replaces_it_and_the_picks_made_in_it() {
    let src = "rules \"Again\" 1
type slayer
requires player.pick
on round_start { offer_pick(0, \"ONE\")
 pick_row(0, \"A\")
 pick_row(0, \"B\") }
on tick { offer_pick(0, \"TWO\")
 pick_row(0, \"C\") }
";
    let mut g = FakeGame::with_players(1);
    let mut vm = Vm::new(compile(src).unwrap(), 0, 0).unwrap();
    vm.begin_tick(0);
    vm.fire(&mut g, Input::RoundStart);
    assert!(vm.choose(0, 0, 1));
    vm.begin_tick(1);
    vm.update(&mut g);
    assert_eq!(words(&vm), [(0, "TWO".to_string(), vec!["C".to_string()])]);
    assert!(vm.choices().is_empty(), "the pick was of the old rows");
}

#[test]
fn the_limits_are_script_errors() {
    let run = |body: &str| {
        let src = format!("rules \"Limits\" 1\ntype slayer\nrequires player.pick\non round_start {{ {body} }}\n");
        let mut g = FakeGame::with_players(1);
        let mut vm = Vm::new(compile(&src).unwrap(), 0, 0).unwrap();
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        g.script_error.map(|e| e.kind)
    };
    assert_eq!(run("offer_pick(0, \"A\")"), None);
    assert_eq!(run(&format!("offer_pick({}, \"A\")", MAX_PICKS - 1)), None);
    assert_eq!(run(&format!("offer_pick({}, \"A\")", MAX_PICKS)), Some(ErrorKind::Picks));
    assert_eq!(run("offer_pick(-1, \"A\")"), Some(ErrorKind::Picks));
    assert_eq!(run("pick_row(0, \"A\")"), Some(ErrorKind::Picks), "no picker offered");
    let rows = |n: usize| format!("offer_pick(0, \"T\")\n{}", "pick_row(0, \"R\")\n".repeat(n));
    assert_eq!(run(&rows(MAX_PICK_ROWS)), None);
    assert_eq!(run(&rows(MAX_PICK_ROWS + 1)), Some(ErrorKind::Picks));
}

#[test]
fn the_state_round_trips_and_a_pick_for_nothing_offered_is_refused() {
    let (mut vm, _) = started();
    assert!(vm.choose(1, 0, 2));
    let bytes = vm.ext().encode();
    let ext = RulesExt::decode(&bytes).unwrap();
    assert_eq!(ext.picks, vm.picks());
    assert_eq!(ext.choices, [Choice { player: 1, slot: 0, row: 2 }]);
    // A saved pick of a row the picker does not hold does not decode.
    let mut bad = ext.clone();
    bad.choices[0].row = 3;
    assert!(RulesExt::decode(&bad.encode()).is_err());
    let mut bad = ext.clone();
    bad.picks = vec![
        PickOffer { slot: 1, title: 0, rows: vec![] },
        PickOffer { slot: 0, title: 0, rows: vec![] },
    ];
    bad.choices.clear();
    assert!(RulesExt::decode(&bad.encode()).is_err(), "pickers go in slot order");
}

#[test]
fn a_script_that_offers_a_pick_declares_the_capability() {
    let src = "rules \"NoCap\" 1\ntype slayer\non round_start { offer_pick(0, \"X\") }\n";
    let found = diagnose(src);
    assert!(
        found.diagnostics.iter().any(|d| d.code == "cap.missing" && d.severity == Severity::Error),
        "{:#?}",
        found.diagnostics
    );
    assert!(compile(PICKER).is_ok());
}

/// The scenario's picks are reported after their tick, as the host reports
/// them, and one the VM refuses is told.
#[test]
fn a_scenario_reports_picks_and_names_the_refused_ones() {
    let s: Scenario = serde_json::from_str(
        r#"{"players":[{"slot":0},{"slot":1}],"ticks":[{},
            {"picks":[{"player":0,"slot":0,"row":1},{"player":1,"slot":0,"row":9}]},
            {"events":[{"spawn":0}]}]}"#,
    )
    .unwrap();
    let r = scenario::run(compile(PICKER).unwrap(), &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(r.refused_picks, [(1, ScenarioPick { player: 1, slot: 0, row: 9 })]);
}
