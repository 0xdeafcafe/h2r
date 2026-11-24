//! Vehicles and drop ships on the fake game (docs/game/rules-world-api.md,
//! "Vehicles and drop ships"): a `path` label with a gap blocks the start,
//! a ship flies a path in the stated ticks facing its tangent, and riders
//! are placed and unloaded. The invented map is a line of points.

use rules::caps::{self, API_VERSION, id};
use rules::fake::FakeGame;
use rules::labels::{self, Class, ProblemKind};
use rules::scenario::{RunError, Scenario, ScenarioLabel, ScenarioPlayer, game_of, step};
use rules::vehicles::ONE;
use rules::vm::Arg;
use rules::{Vm, build};

const SHIP: u32 = 0x200;
const POINT: u32 = 0x300;

const SRC: &str = r#"
rules "Drop" 1
type slayer
requires vehicles.drive

label ship: object
label run_in: path

table waves: (class: Class, n: int) = [("elite", 4)]
global var phase: int = 0
global var deadline: time = 0t
global var aboard: int = 0
global var pilot: int = 0

on tick {
    aboard = vehicle_riders(ship)
    if phase == 0 {
        place_squad_in(squad(0), waves[0].class, waves[0].n, ship)
        if let p = first(players q where q.alive) { seat(p, ship) }
        fly(ship, run_in, 10t)
        deadline = now() + 11t
        phase = 1
    } else if phase == 1 and now() >= deadline {
        unload(ship)
        phase = 2
    }
}
"#;

fn pilot() -> ScenarioPlayer {
    ScenarioPlayer {
        slot: 0,
        team: 0,
        alive: true,
        score: 0,
        lives: 0,
        unit: Some(100),
    }
}

/// The ship and `n` points of `run_in`, numbered from 0.
fn map(numbers: &[i32]) -> Vec<ScenarioLabel> {
    let mut map = vec![ScenarioLabel {
        class: Some(Class::Object),
        ..ScenarioLabel::new("ship", SHIP)
    }];
    for (i, &n) in numbers.iter().enumerate() {
        map.push(ScenarioLabel {
            number: Some(n),
            class: Some(Class::Object),
            ..ScenarioLabel::new("run_in", POINT + i as u32)
        });
    }
    map
}

fn w(n: i32) -> i32 {
    n * ONE
}

/// A game on the line (0, 0, 0), (10, 0, 0), (10, 10, 0), the ship parked
/// at the first point with 8 seats.
fn game(ir: &rules::Ir) -> FakeGame {
    let scenario = Scenario {
        players: vec![pilot()],
        map: Some(map(&[0, 1, 2])),
        ..Scenario::default()
    };
    let mut g = game_of(ir, &scenario).unwrap();
    g.object_at.insert(POINT, [0, 0, 0]);
    g.object_at.insert(POINT + 1, [w(10), 0, 0]);
    g.object_at.insert(POINT + 2, [w(10), w(10), 0]);
    g.add_vehicle(SHIP, 8, [0, 0, 0]);
    g
}

#[test]
fn a_ship_flies_its_path_in_the_stated_ticks_and_unloads_its_riders() {
    let ir = build(SRC).unwrap().0;
    let mut g = game(&ir);
    let mut vm = Vm::new(ir, 0, 0).unwrap();

    // Tick 0 queues the load and the flight; they are made at the start of
    // tick 1, which is the flight's first tick.
    step(&mut vm, &mut g, 0, true, &[]);
    assert!(g.calls_to("fly").is_empty());
    step(&mut vm, &mut g, 1, false, &[]);
    assert_eq!(g.calls_to("fly").len(), 1);
    assert!(g.refused.is_empty(), "{:?}", g.refused);
    assert_eq!(g.vehicles[&SHIP].driver, Some(0));
    assert_eq!(g.vehicles[&SHIP].squads, [(0, 4)]);
    assert_eq!(g.vehicle_of(0), Some(SHIP));

    // 20 world units in 10 ticks: two a tick along +x, then +y, facing
    // each leg's tangent; the corner (10, 0) is reached at tick 6.
    for t in 2..=5 {
        step(&mut vm, &mut g, t, false, &[]);
    }
    assert_eq!(g.object_at[&SHIP], [w(8), 0, 0], "tick 5");
    assert_eq!(g.facing[&SHIP], [ONE, 0, 0]);
    step(&mut vm, &mut g, 6, false, &[]);
    assert_eq!(g.object_at[&SHIP], [w(10), 0, 0]);
    assert_eq!(
        g.facing[&SHIP],
        [0, ONE, 0],
        "the corner belongs to the second leg"
    );
    for t in 7..=10 {
        step(&mut vm, &mut g, t, false, &[]);
    }
    assert_eq!(g.object_at[&SHIP], [w(10), w(8), 0]);
    assert!(g.take_arrived().is_empty(), "not there yet");
    assert_eq!(vm.global("aboard"), Some(5), "the pilot and four actors");

    // The flight ends on tick 11 exactly, at the last point.
    step(&mut vm, &mut g, 11, false, &[]);
    assert_eq!(g.object_at[&SHIP], [w(10), w(10), 0]);
    assert_eq!(g.take_arrived(), [SHIP]);
    assert!(g.vehicles[&SHIP].flight.is_none());
    assert!(g.take_arrived().is_empty(), "reported once");

    // The script saw the flight's end and unloads; everyone is out.
    step(&mut vm, &mut g, 12, false, &[]);
    assert_eq!(g.calls_to("unload").len(), 1);
    assert_eq!(g.vehicles[&SHIP].aboard(), 0);
    assert_eq!(g.unloaded, [(SHIP, 0, 4)]);
    assert_eq!(g.vehicle_of(0), None);
    step(&mut vm, &mut g, 13, false, &[]);
    assert_eq!(vm.global("aboard"), Some(0));
    assert!(g.refused.is_empty(), "{:?}", g.refused);
}

#[test]
fn a_path_with_a_gap_blocks_the_start() {
    let ir = build(SRC).unwrap().0;
    let census = |numbers: &[i32]| {
        let things = map(numbers);
        let scenario = Scenario {
            map: Some(things),
            ..Scenario::default()
        };
        (
            game_of(&ir, &scenario),
            rules::scenario::census_of(&ir, &map(numbers)),
        )
    };

    // 0, 1, 2 is a path.
    assert!(census(&[0, 1, 2]).0.is_ok());
    // 0, 2 has no point 1.
    let (run, c) = census(&[0, 2]);
    let Err(RunError::Map(problems)) = run else {
        panic!("a gap must block the start")
    };
    assert!(
        problems.iter().any(|p| p.contains("no point numbered 1")),
        "{problems:?}"
    );
    let found = labels::check(&ir, &c, &[0, 1]);
    assert!(
        found
            .iter()
            .any(|p| matches!(p.kind, ProblemKind::PathGap { number: 1, .. })),
        "{found:?}"
    );
    // 1, 2 has no point 0.
    let found = labels::check(&ir, &census(&[1, 2]).1, &[0, 1]);
    assert!(
        found
            .iter()
            .any(|p| matches!(p.kind, ProblemKind::PathGap { number: 0, .. }))
    );
    // A single point is no path.
    let found = labels::check(&ir, &census(&[0]).1, &[0, 1]);
    assert!(
        found
            .iter()
            .any(|p| matches!(p.kind, ProblemKind::Count { found: 1, .. })),
        "{found:?}"
    );
    // Two points with the same number.
    let found = labels::check(&ir, &census(&[0, 1, 1]).1, &[0, 1]);
    assert!(
        found
            .iter()
            .any(|p| matches!(p.kind, ProblemKind::PathDuplicate { number: 1, .. }))
    );
    // A point with no number.
    let mut c = census(&[0, 1]).1;
    c.labels.get_mut("run_in").unwrap()[1].number = None;
    assert!(
        labels::check(&ir, &c, &[0, 1])
            .iter()
            .any(|p| matches!(p.kind, ProblemKind::Unnumbered { .. }))
    );
    // The smallest map the check accepts for the script passes it.
    assert!(labels::check(&ir, &labels::minimal(&ir, &[0, 1]), &[0, 1]).is_empty());
}

#[test]
fn the_longest_path_still_flies_in_its_ticks() {
    // 256 points a unit apart, 255 units in 51 ticks: five a tick.
    let ir = build(SRC).unwrap().0;
    let numbers: Vec<i32> = (0..256).collect();
    let scenario = Scenario {
        map: Some(map(&numbers)),
        ..Scenario::default()
    };
    let mut g = game_of(&ir, &scenario).unwrap();
    for i in 0..256u32 {
        g.object_at.insert(POINT + i, [w(i as i32), 0, 0]);
    }
    g.add_vehicle(SHIP, 2, [0; 3]);
    let call = rules::Call {
        entry: caps::entry(id::FLY, API_VERSION).unwrap(),
        args: vec![
            Arg::Object(Some(SHIP)),
            Arg::Label("run_in".into()),
            Arg::Int(51),
        ],
    };
    use rules::Game;
    g.begin_tick(10);
    g.act(&call);
    g.begin_tick(11);
    assert_eq!(g.object_at[&SHIP][0], w(5));
    g.begin_tick(61);
    assert_eq!(g.object_at[&SHIP], [w(255), 0, 0]);
    assert_eq!(g.take_arrived(), [SHIP]);
}

fn act(g: &mut FakeGame, entry: u16, args: Vec<Arg>) {
    use rules::Game;
    g.act(&rules::Call {
        entry: caps::entry(entry, API_VERSION).unwrap(),
        args,
    });
}

fn read(g: &mut FakeGame, entry: u16, o: u32) -> i32 {
    use rules::Game;
    g.read(&rules::Call {
        entry: caps::entry(entry, API_VERSION).unwrap(),
        args: vec![Arg::Object(Some(o))],
    })
}

#[test]
fn seats_fill_driver_first_and_a_full_vehicle_refuses() {
    let mut g = FakeGame::with_players(4);
    g.add_vehicle(SHIP, 2, [0; 3]);
    let seat = |g: &mut FakeGame, p| {
        act(
            g,
            id::SEAT,
            vec![Arg::Player(Some(p)), Arg::Object(Some(SHIP))],
        )
    };
    seat(&mut g, 2);
    seat(&mut g, 0);
    seat(&mut g, 1);
    assert_eq!(g.vehicles[&SHIP].driver, Some(2));
    assert_eq!(g.vehicles[&SHIP].riders, [0]);
    assert_eq!(g.refused, [("seat", "the vehicle has no free seat".to_string())]);
    assert_eq!(read(&mut g, id::VEHICLE_RIDERS, SHIP), 2);
    assert_eq!(read(&mut g, id::VEHICLE_DRIVER, SHIP), 2);
    // Boarding what you are in changes nothing.
    seat(&mut g, 0);
    assert_eq!(g.vehicles[&SHIP].riders, [0]);
    assert_eq!(g.refused.len(), 1);

    // Out, and the next in takes the empty driver's seat.
    act(&mut g, id::EJECT, vec![Arg::Player(Some(2))]);
    assert_eq!(read(&mut g, id::VEHICLE_DRIVER, SHIP), -1);
    seat(&mut g, 3);
    assert_eq!(g.vehicles[&SHIP].driver, Some(3));

    // A dead player is out of the seat and cannot board.
    g.apply(&rules::Input::Death(3));
    assert_eq!(read(&mut g, id::VEHICLE_DRIVER, SHIP), -1);
    seat(&mut g, 3);
    assert_eq!(g.refused.last(), Some(&("seat", "the player is not alive".to_string())));
}

#[test]
fn a_squad_takes_the_free_seats_and_no_more() {
    let mut g = FakeGame::with_players(1);
    g.add_vehicle(SHIP, 4, [0; 3]);
    let place = |g: &mut FakeGame, s: u8, n: i32| {
        act(
            g,
            id::PLACE_SQUAD_IN,
            vec![
                Arg::Squad(s),
                Arg::Class(Some("elite".into())),
                Arg::Int(n),
                Arg::Object(Some(SHIP)),
            ],
        )
    };
    place(&mut g, 0, 3);
    place(&mut g, 1, 3);
    assert_eq!(
        g.vehicles[&SHIP].squads,
        [(0, 3), (1, 1)],
        "one seat was left"
    );
    assert_eq!(read(&mut g, id::VEHICLE_RIDERS, SHIP), 4);
    place(&mut g, 2, 1);
    assert_eq!(
        g.refused,
        [("place_squad_in", "the vehicle has no free seat".to_string())]
    );
    act(&mut g, id::UNLOAD, vec![Arg::Object(Some(SHIP))]);
    assert_eq!(g.unloaded, [(SHIP, 0, 3), (SHIP, 1, 1)]);
    assert_eq!(read(&mut g, id::VEHICLE_RIDERS, SHIP), 0);
    place(&mut g, 0, 0);
    assert_eq!(
        g.refused.last(),
        Some(&("place_squad_in", "a squad is at least 1 actor".to_string()))
    );
}

#[test]
fn a_vehicle_entry_on_anything_else_is_a_refused_call() {
    let mut g = FakeGame::with_players(1);
    let crate_ = 0x999;
    let before = g.clone().refused.len();
    for (entry, args) in [
        (
            id::FLY,
            vec![
                Arg::Object(Some(crate_)),
                Arg::Label("run_in".into()),
                Arg::Int(10),
            ],
        ),
        (id::HOVER, vec![Arg::Object(Some(crate_)), Arg::Bool(true)]),
        (
            id::SEAT,
            vec![Arg::Player(Some(0)), Arg::Object(Some(crate_))],
        ),
        (id::UNLOAD, vec![Arg::Object(Some(crate_))]),
        (id::UNLOAD, vec![Arg::Object(None)]),
    ] {
        act(&mut g, entry, args);
    }
    assert_eq!(g.refused.len() - before, 5);
    assert!(g.vehicles.is_empty() && g.object_at.is_empty());
    assert_eq!(read(&mut g, id::VEHICLE_RIDERS, crate_), 0);
    assert_eq!(read(&mut g, id::VEHICLE_DRIVER, crate_), -1);

    // A flight needs a path that is placed and at least a tick.
    g.add_vehicle(SHIP, 1, [0; 3]);
    act(
        &mut g,
        id::FLY,
        vec![
            Arg::Object(Some(SHIP)),
            Arg::Label("run_in".into()),
            Arg::Int(10),
        ],
    );
    assert_eq!(
        g.refused.last(),
        Some(&("fly", "a point of the path is not placed".to_string()))
    );
    act(
        &mut g,
        id::FLY,
        vec![
            Arg::Object(Some(SHIP)),
            Arg::Label("run_in".into()),
            Arg::Int(0),
        ],
    );
    assert_eq!(
        g.refused.last(),
        Some(&("fly", "a flight takes at least 1 tick".to_string()))
    );
}

#[test]
fn the_vehicle_entries_are_refused_by_a_host_until_they_are_named() {
    for name in [
        "fly",
        "hover",
        "seat",
        "eject",
        "place_squad_in",
        "unload",
        "vehicle_riders",
        "vehicle_driver",
    ] {
        let e = caps::entry_named(name).unwrap();
        let named = matches!(name, "hover" | "unload");
        assert_eq!(e.status.hostable(), named, "{name}");
        assert_eq!(e.hostable_on(rules::SCRIPT_INDEX), named, "{name}");
        if e.kind == caps::Kind::Action {
            assert_eq!(e.cap, Some("vehicles.drive"), "{name}");
            assert_eq!(e.order, caps::Order::Queued, "{name}");
        } else {
            assert_eq!(e.cap, None, "{name}");
        }
    }
    assert!(caps::capability("vehicles.drive", API_VERSION).is_some_and(|c| !c.grantable));
}

#[test]
fn the_compiler_keeps_a_path_to_fly_and_a_vehicle_call_to_its_capability() {
    let with = |body: &str, requires: &str| {
        format!(
            "rules \"T\" 1\ntype slayer\n{requires}\nlabel ship: object\nlabel run_in: path\nlabel pads: objects\non tick {{ {body} }}\n"
        )
    };
    let ok = |body: &str| build(&with(body, "requires vehicles.drive")).map(|_| ());
    assert!(ok("fly(ship, run_in, 5s)").is_ok());
    // A path is not visited, indexed or tested, and fits only `fly`.
    assert!(ok("for each run_in p { delete(p) }").is_err());
    assert!(ok("let p = first(run_in)").is_err());
    assert!(ok("let p = run_in[0]").is_err());
    assert!(
        ok("fly(ship, pads, 5s)").is_err(),
        "a set of objects is no path"
    );
    assert!(ok("fly(ship, ship, 5s)").is_err());
    assert!(ok("place_squad(squad(0), \"elite\", 1, run_in)").is_err());
    // The vehicle entries need `vehicles.drive`.
    assert!(build(&with("fly(ship, run_in, 5s)", "")).is_err());
    assert!(build(&with("unload(ship)", "")).is_err());
    // A path asks no `numbered`, `per team` or one value.
    let decl = |kind: &str| {
        build(&format!(
            "rules \"T\" 1\ntype slayer\nlabel r: {kind}\non tick {{ }}\n"
        ))
        .map(|_| ())
    };
    assert!(decl("path").is_ok());
    assert!(decl("path at least 3").is_ok());
    assert!(decl("path at most 1").is_err(), "a path has two points");
    assert!(decl("path numbered").is_err());
    assert!(decl("path per team").is_err());
    assert!(decl("path?").is_err());
}
