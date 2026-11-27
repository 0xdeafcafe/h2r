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

/// A Siege map for the HUD: two territories (600, 601) and each team's flag
/// stand (20, 21), team 1 being player 1's.
fn siege_game() -> FakeGame {
    let mut g = FakeGame::with_players(2);
    g.players.get_mut(&1).unwrap().team = 1;
    g.variant.insert("team_play".into(), 1);
    g.labelled.insert(
        "territory".into(),
        vec![Placed { placement: 0, object: 600 }, Placed { placement: 1, object: 601 }],
    );
    g.labelled.insert(
        "ctf_flag_spawn".into(),
        vec![Placed { placement: 0, object: 20 }, Placed { placement: 1, object: 21 }],
    );
    g.object_teams.extend([(20, 0), (21, 1)]);
    g
}

/// What viewer 0 reads of the Siege front row: (shown, style, value).
fn front(vm: &Vm) -> Option<(bool, u8, i32)> {
    let viewer = Viewer { slot: Some(0), team: None };
    view(vm.show(), vm.ir(), viewer).at("koth.hill").map(|w| (w.shown, w.style, w.value))
}

/// The banner viewer 0 reads, when it is shown.
fn banner(vm: &Vm) -> Option<String> {
    let viewer = Viewer { slot: Some(0), team: None };
    let v = view(vm.show(), vm.ir(), viewer);
    v.at("koth.banner").filter(|w| w.shown).and_then(|w| w.text.clone())
}

#[test]
fn the_siege_mark_and_row_follow_the_front_and_its_holder() {
    let mut g = siege_game();
    let mut vm = start(builtin::SIEGE, &mut g, &[("capture_time", 5)]);

    // Nobody on a site: the row is open and the mark is over the first one.
    ticks(&mut vm, &mut g, 2, 1);
    assert_eq!(front(&vm), Some((true, 0, 0)));
    assert_eq!(marked(&vm, "koth.mark", 0), Some(600));

    // An attacker alone on the second site: held by side 0, and the mark
    // follows the site they are on.
    g.inside.insert((0, 601));
    ticks(&mut vm, &mut g, 3, 2);
    assert_eq!(front(&vm), Some((true, 1, 0)));
    assert_eq!(marked(&vm, "koth.mark", 0), Some(601));

    // A defender on it too: contested.
    g.inside.insert((1, 601));
    ticks(&mut vm, &mut g, 5, 1);
    assert_eq!(front(&vm), Some((true, 2, 0)));

    // A defender alone: held by side 1.
    g.inside.remove(&(0, 601));
    ticks(&mut vm, &mut g, 6, 1);
    assert_eq!(front(&vm), Some((true, 1, 1)));

    // The front stays on the occupied site even as attackers move elsewhere.
    g.inside.insert((0, 600));
    ticks(&mut vm, &mut g, 7, 1);
    assert_eq!(marked(&vm, "koth.mark", 0), Some(601));

    // Once empty, the old front's accumulated capture time does not keep
    // the mark there.
    g.inside.remove(&(1, 601));
    g.inside.insert((1, 600));
    ticks(&mut vm, &mut g, 8, 1);
    assert_eq!(marked(&vm, "koth.mark", 0), Some(600));

    // The front can follow attackers onto a contested site immediately.
    g.inside.clear();
    g.inside.extend([(0, 601), (1, 601)]);
    ticks(&mut vm, &mut g, 9, 1);
    assert_eq!(marked(&vm, "koth.mark", 0), Some(601));
    assert_eq!(front(&vm).map(|(shown, style, _)| (shown, style)), Some((true, 2)));
}

/// Holds site 600 alone until a banner other than the round's number
/// shows, and returns its text, the mark's object that tick, and whether
/// it is hidden two ticks on.
fn first_banner(options: &[(&str, i64)]) -> (String, Option<u32>, bool) {
    let mut g = siege_game();
    let mut vm = start(builtin::SIEGE, &mut g, options);
    g.inside.insert((0, 600));
    for t in 2..800 {
        ticks(&mut vm, &mut g, t, 1);
        if let Some(text) = banner(&vm).filter(|b| b != "ROUND 1") {
            let mark = marked(&vm, "koth.mark", 0);
            ticks(&mut vm, &mut g, t + 1, 2);
            return (text, mark, banner(&vm).is_none());
        }
    }
    panic!("no banner");
}

#[test]
fn the_siege_banner_says_a_site_taken_and_the_final_assault() {
    // Two sites: the first taken is a site, and the front moves to the other.
    let (text, mark, hidden) = first_banner(&[("capture_time", 5)]);
    assert_eq!((text.as_str(), mark, hidden), ("SITE TAKEN", Some(601), true));
    // One site: taking it opens the final assault, and the front is the stand.
    let (text, mark, hidden) = first_banner(&[("capture_time", 5), ("sites", 1)]);
    assert_eq!((text.as_str(), mark, hidden), ("FINAL ASSAULT", Some(21), true));
}

#[test]
fn the_siege_clock_is_called_at_thirty_and_ten_seconds() {
    let mut g = siege_game();
    let mut vm = start(builtin::SIEGE, &mut g, &[("phase_time", 60)]);
    let mut said = Vec::new();
    for t in 2..1805 {
        ticks(&mut vm, &mut g, t, 1);
        if let Some(b) = banner(&vm)
            && said.last() != Some(&b)
        {
            said.push(b);
        }
    }
    // The round's number opens it (two rounds by default).
    assert_eq!(said, ["ROUND 1", "30 SECONDS", "10 SECONDS"]);
}

/// `mark(wp, at)` is every viewer, `mark(wp, at, all)` is the same, and
/// `mark(wp, at, to)` with a player or a team is that audience only: the
/// team's mask comes from the roster the game answers (`players_of_team`).
#[test]
fn a_mark_names_its_audience_and_the_view_hides_the_rest() {
    let src = "rules \"Aim\" 1\ntype slayer\nrequires hud.waypoints\n\
        label hill: zones numbered\n\
        waypoint all_mark at ctf.mark\n\
        waypoint team_mark at ter.mark\n\
        waypoint one_mark at asl.mark\n\
        on round_start {\n\
            if let z = hill[0] {\n\
                mark(all_mark, z)\n\
                mark(team_mark, z, team 1)\n\
                for each players p { if slot_of(p) == 0 { mark(one_mark, z, p) } }\n\
            }\n\
        }\n";
    let mut g = FakeGame::with_players(3);
    g.players.get_mut(&1).unwrap().team = 1;
    g.players.get_mut(&2).unwrap().team = 1;
    g.label("hill", 0x4001_0000, 0, 1);
    let vm = start(src, &mut g, &[]);
    assert_eq!(g.script_error, None, "{:?}", g.script_error);

    let at = |vm: &Vm, slot: &str, viewer: u8, team: Option<u8>| {
        let viewer = Viewer { slot: Some(viewer), team };
        view(vm.show(), vm.ir(), viewer).waypoint_at(slot).is_some()
    };
    // Everyone's mark is seen by all three; team 1's by its two players;
    // the player form by slot 0 alone.
    assert_eq!([0, 1, 2].map(|v| at(&vm, "ctf.mark", v, None)), [true; 3]);
    assert_eq!(
        [0, 1, 2].map(|v| at(&vm, "ter.mark", v, None)),
        [false, true, true]
    );
    assert_eq!(
        [0, 1, 2].map(|v| at(&vm, "asl.mark", v, None)),
        [true, false, false]
    );

    // The bytes a viewer's machine holds show the same: the state encodes,
    // decodes and views the same as the VM's own.
    let held = rules::show::State::decode(&vm.show().encode()).unwrap();
    let other = Viewer { slot: Some(1), team: Some(1) };
    assert_eq!(view(&held, vm.ir(), other), view(vm.show(), vm.ir(), other));
}

/// The announcer lines (`announcer/<clip>`) the script has played, each
/// with its audience, in order.
fn said(g: &FakeGame) -> Vec<(String, u16)> {
    g.sounds
        .iter()
        .filter_map(|s| Some((s.path.strip_prefix("announcer/")?.to_string(), s.audience)))
        .collect()
}

#[test]
fn siege_says_attack_and_defend_to_each_side_and_the_final_round() {
    use rules::show::{EVERYONE, player};
    // Round one of two: each side hears its own word, nobody a final round.
    let mut g = siege_game();
    let mut vm = start(builtin::SIEGE, &mut g, &[("phase_time", 60)]);
    assert_eq!(
        said(&g),
        [("attack".to_string(), player(0)), ("defend".to_string(), player(1))]
    );
    // The clock runs out: the defenders hold, the round is over for all.
    ticks(&mut vm, &mut g, 2, 1900);
    let at = said(&g);
    assert!(at.contains(&("round-over".to_string(), EVERYONE)), "{at:?}");
    // The second round of two is the final one, and the sides swap.
    g.sounds.clear();
    vm.begin_tick(2000);
    assert_eq!(vm.fire(&mut g, Input::RoundStart), Fired::Ran);
    assert_eq!(
        said(&g),
        [
            ("final-round".to_string(), EVERYONE),
            ("attack".to_string(), player(1)),
            ("defend".to_string(), player(0)),
        ]
    );
}

#[test]
fn siege_says_a_site_captured_the_round_over_and_each_sides_reinforcements() {
    use rules::show::{EVERYONE, player};
    let mut g = siege_game();
    let mut vm = start(builtin::SIEGE, &mut g, &[("capture_time", 5)]);
    g.inside.insert((0, 600));
    ticks(&mut vm, &mut g, 2, 60);
    let at = said(&g);
    assert!(at.contains(&("site-captured".to_string(), EVERYONE)), "{at:?}");
    // A defender dies: the side's wave lands after its time, with a line to
    // that side alone.
    g.players.get_mut(&1).unwrap().alive = false;
    vm.fire(&mut g, Input::Death(1));
    ticks(&mut vm, &mut g, 62, 400);
    let at = said(&g);
    assert!(at.contains(&("reinforcements".to_string(), player(1))), "{at:?}");
    assert!(!at.contains(&("reinforcements".to_string(), player(0))), "{at:?}");
}
