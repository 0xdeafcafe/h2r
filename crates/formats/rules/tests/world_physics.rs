//! Physics reads and events of the world API (docs/game/rules-world-api.md,
//! "Physics"): `airborne`, `crouching`, `primed`, `on touchdown` and the
//! sight table behind `sees`, on the fake game. Every invented fixture; no
//! game data. `push` and `launch` wait on R9 and have no entry.

use rules::caps::{self, id};
use rules::fake::FakeGame;
use rules::ir::Op;
use rules::scenario::{self, Scenario, ScenarioBody, ScenarioEvent, ScenarioPlayer, ScenarioTick};
use rules::ty::fixed;
use rules::verify::unhostable;
use rules::vm::{Arg, ErrorKind, Fired, Input, MAX_SIGHT_PAIRS, RulesExt, SIGHT_KEEP};
use rules::{Event, Vm, build, builtin, compile};

const EYE: u32 = 0x4001_0000;
const TARGET: u32 = 0x4001_0001;

/// The Superbounce chain's shape, with no strips: a landing that follows a
/// primed tick and rises past 8 a second is a bounce, and its chain grows;
/// any other landing breaks the chain.
const CHAIN: &str = r#"
rules "Chain" 1
type slayer
player var was_primed: bool = false
player var chain: int = 0
global var last_down: fixed = 0.0
on touchdown(p, down, up) {
    last_down = down
    if p.was_primed and up > 8.0 {
        p.chain = p.chain + 1
        score(p, p.chain)
    } else {
        p.chain = 0
    }
}
on tick {
    for each players p { p.was_primed = p.primed }
}
"#;

fn fixed_at(n: i32) -> i32 {
    fixed::from_int(n)
}

fn tick(vm: &mut Vm, g: &mut FakeGame, t: u32) {
    vm.begin_tick(t);
    assert_ne!(vm.update(g), Fired::Stopped);
}

fn land(vm: &mut Vm, g: &mut FakeGame, p: u8, down: i32, up: i32) {
    let input = Input::Touchdown {
        player: p,
        down: fixed_at(down),
        up: fixed_at(up),
    };
    assert!(g.apply(&input));
    vm.fire(g, input);
}

#[test]
fn a_primed_landing_that_rises_past_the_launch_grows_the_chain() {
    let (ir, _) = build(CHAIN).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = FakeGame::with_players(2);
    g.set_body(0, true, false, true);
    // The tick before the landing: the script keeps what `primed` read.
    tick(&mut vm, &mut g, 1);
    assert_eq!(vm.player_var(0, "was_primed"), Some(1));
    assert_eq!(vm.player_var(1, "was_primed"), Some(0));
    land(&mut vm, &mut g, 0, 9, 10);
    assert_eq!(vm.player_var(0, "chain"), Some(1));
    assert_eq!(
        vm.global("last_down"),
        Some(fixed_at(9)),
        "the fall speed before"
    );
    assert!(!g.airborne.contains(&0), "the fake lands the player");
    // Primed again and another bounce: the chain is 2.
    g.set_body(0, true, false, true);
    tick(&mut vm, &mut g, 2);
    land(&mut vm, &mut g, 0, 9, 14);
    assert_eq!(vm.player_var(0, "chain"), Some(2));
    let scored: Vec<_> = g.calls_to("score").into_iter().cloned().collect();
    assert_eq!(
        scored,
        [
            vec![Arg::Player(Some(0)), Arg::Int(1)],
            vec![Arg::Player(Some(0)), Arg::Int(2)]
        ]
    );
    // A landing from a tick that was not primed, or too slow, breaks it.
    g.set_body(0, true, false, false);
    tick(&mut vm, &mut g, 3);
    land(&mut vm, &mut g, 0, 9, 14);
    assert_eq!(
        vm.player_var(0, "chain"),
        Some(0),
        "not primed the tick before"
    );
    g.set_body(0, true, false, true);
    tick(&mut vm, &mut g, 4);
    land(&mut vm, &mut g, 0, 9, 3);
    assert_eq!(vm.player_var(0, "chain"), Some(0), "too slow");
    assert_eq!(g.calls_to("score").len(), 2);
    // Player 1 never chained.
    assert_eq!(vm.player_var(1, "chain"), Some(0));
}

#[test]
fn the_physics_reads_follow_the_biped_and_read_false_for_the_rest() {
    let src = "rules \"R\" 1\ntype slayer\n\
               player var a: bool = false\nplayer var c: bool = false\nplayer var p: bool = false\n\
               player var f: bool = false\n\
               on tick { for each players q {\n q.a = airborne(q)\n q.c = crouching(q)\n q.p = q.primed\n \
               q.f = q.airborne\n } }\n";
    let ir = compile(src).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = FakeGame::with_players(2);
    let reads = |vm: &Vm, p: u8| ["a", "c", "p", "f"].map(|n| vm.player_var(p, n));
    tick(&mut vm, &mut g, 0);
    assert_eq!(
        reads(&vm, 0),
        [Some(0); 4],
        "a standing player on the ground reads false"
    );
    g.set_body(0, true, true, false);
    tick(&mut vm, &mut g, 1);
    assert_eq!(reads(&vm, 0), [Some(1), Some(1), Some(0), Some(1)]);
    assert_eq!(
        reads(&vm, 1),
        [Some(0); 4],
        "each player reads its own biped"
    );
    g.set_body(0, false, false, true);
    tick(&mut vm, &mut g, 2);
    assert_eq!(reads(&vm, 0), [Some(0), Some(0), Some(1), Some(0)]);
}

#[test]
fn a_physics_read_takes_no_capability_and_is_read_only() {
    let ok = "rules \"R\" 1\ntype slayer\non tick { for each players p { if p.airborne and p.crouching and p.primed { score(p, 1) } } }\n";
    let (ir, _) = build(ok).unwrap();
    assert!(ir.header.requires.is_empty(), "reads need no capability");
    let e =
        compile("rules \"R\" 1\ntype slayer\non tick { for each players p { p.primed = true } }\n")
            .unwrap_err();
    assert!(!e.to_string().is_empty());
}

#[test]
fn push_and_launch_wait_on_the_open_research() {
    for call in [
        "push(p, vec(0.0, 0.0, 1.0))",
        "launch(p, vec(0.0, 0.0, 9.0))",
        "set_velocity(p, vec(0.0, 0.0, 1.0))",
    ] {
        let src =
            format!("rules \"R\" 1\ntype slayer\non tick {{ for each players p {{ {call} }} }}\n");
        assert!(compile(&src).is_err(), "{call} has no entry yet");
    }
}

#[test]
fn touchdown_is_an_event_with_a_player_and_two_speeds() {
    assert_eq!(
        Event::from_code(Event::Touchdown.code()),
        Some(Event::Touchdown)
    );
    assert_eq!(Event::named("touchdown"), Some(Event::Touchdown));
    assert_eq!(
        Event::Touchdown.args(),
        [rules::Ty::Player, rules::Ty::Fixed, rules::Ty::Fixed]
    );
    assert_eq!(Event::Touchdown.worst_per_tick(), 16);
    // A handler naming more than the event gives does not compile.
    assert!(compile("rules \"T\" 1\ntype slayer\non touchdown(p, a, b, c) { }\n").is_err());
    assert!(
        compile("rules \"T\" 1\ntype slayer\non touchdown(p, down, up) { score(down, 1) }\n")
            .is_err()
    );
    // Without a handler the event is no handler: the built-in Slayer script
    // has none (a script with no handler at all does not compile).
    let (ir, _) = build(builtin::SLAYER).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = FakeGame::with_players(1);
    vm.begin_tick(0);
    assert_eq!(
        vm.fire(
            &mut g,
            Input::Touchdown {
                player: 0,
                down: 0,
                up: 0
            }
        ),
        Fired::NoHandler
    );
}

const SIGHT: &str = r#"
rules "Sight" 1
type slayer
label eye: object
label target: object
global var ask: bool = true
global var seen: bool = false
on tick { if ask { seen = sees(eye, target) } }
"#;

fn sight() -> (Vm, FakeGame) {
    let ir = compile(SIGHT).unwrap();
    let mut g = FakeGame::with_players(1);
    g.label("eye", EYE, 0, EYE);
    g.label("target", TARGET, 0, TARGET);
    g.clear_sight.insert((EYE, TARGET));
    (Vm::new(ir, 0, 0).unwrap(), g)
}

#[test]
fn a_sight_pair_answers_one_tick_after_it_is_asked() {
    let (mut vm, mut g) = sight();
    tick(&mut vm, &mut g, 0);
    assert_eq!(
        vm.global("seen"),
        Some(0),
        "false until its first trace, though the way is clear"
    );
    assert_eq!(
        g.traces,
        [(EYE, TARGET)],
        "traced at the end of the tick it was asked"
    );
    assert_eq!(vm.sight().len(), 1);
    tick(&mut vm, &mut g, 1);
    assert_eq!(vm.global("seen"), Some(1), "the next tick reads the trace");
    assert_eq!(g.traces.len(), 2, "kept pairs are traced every tick");
    // Something solid moves between them: the answer follows a tick later.
    g.clear_sight.clear();
    tick(&mut vm, &mut g, 2);
    assert_eq!(
        vm.global("seen"),
        Some(1),
        "this tick still reads the last trace"
    );
    tick(&mut vm, &mut g, 3);
    assert_eq!(vm.global("seen"), Some(0));
}

#[test]
fn a_pair_is_asked_in_one_direction_only() {
    let (mut vm, mut g) = sight();
    tick(&mut vm, &mut g, 0);
    tick(&mut vm, &mut g, 1);
    assert_eq!(g.traces.iter().filter(|t| **t == (TARGET, EYE)).count(), 0);
}

#[test]
fn a_pair_nobody_asks_for_is_dropped_after_thirty_ticks() {
    let (mut vm, mut g) = sight();
    tick(&mut vm, &mut g, 0);
    tick(&mut vm, &mut g, 1);
    assert!(vm.set_global("ask", 0));
    // Last asked on tick 1: kept through tick 30, gone at the end of 31.
    for t in 2..=(1 + SIGHT_KEEP - 1) {
        tick(&mut vm, &mut g, t);
        assert_eq!(vm.sight().len(), 1, "tick {t}");
    }
    tick(&mut vm, &mut g, 1 + SIGHT_KEEP);
    assert!(vm.sight().is_empty());
    let traced = g.traces.len();
    tick(&mut vm, &mut g, 2 + SIGHT_KEEP);
    assert_eq!(g.traces.len(), traced, "a dropped pair is traced no more");
    // Asking again starts from false.
    assert!(vm.set_global("ask", 1));
    tick(&mut vm, &mut g, 3 + SIGHT_KEEP);
    assert_eq!(vm.global("seen"), Some(0));
    tick(&mut vm, &mut g, 4 + SIGHT_KEEP);
    assert_eq!(vm.global("seen"), Some(1));
}

#[test]
fn a_handler_never_traces() {
    // A `sees` read from any handler leaves the trace to the tick's end:
    // the fake counts none while the handler runs.
    let src = "rules \"S\" 1\ntype slayer\nlabel eye: object\nlabel target: object\n\
               global var seen: bool = false\non kill(v, k) { seen = sees(eye, target) }\n";
    let ir = compile(src).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = FakeGame::with_players(2);
    g.label("eye", EYE, 0, EYE);
    g.label("target", TARGET, 0, TARGET);
    g.clear_sight.insert((EYE, TARGET));
    vm.begin_tick(0);
    vm.fire(
        &mut g,
        Input::Kill {
            victim: 0,
            killer: Some(1),
        },
    );
    assert!(g.traces.is_empty());
    assert_eq!(vm.global("seen"), Some(0));
    assert_eq!(vm.sight().len(), 1);
    vm.update(&mut g);
    assert_eq!(g.traces, [(EYE, TARGET)]);
}

#[test]
fn a_thirty_third_pair_stops_the_script() {
    let src = "rules \"S\" 1\ntype slayer\nlabel eye: object\nlabel things: objects\n\
               global var n: int = 0\n\
               on tick { for each things t { if sees(eye, t) { n = n + 1 } } }\n";
    let ir = compile(src).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = FakeGame::with_players(1);
    g.label("eye", EYE, 0, EYE);
    for i in 0..=MAX_SIGHT_PAIRS as u32 {
        g.label("things", 0x4002_0000 + i, i as i32, i);
    }
    vm.begin_tick(0);
    assert_eq!(vm.update(&mut g), Fired::Stopped);
    assert_eq!(g.script_error.map(|e| e.kind), Some(ErrorKind::Sight));
    assert_eq!(vm.sight().len(), MAX_SIGHT_PAIRS);
}

#[test]
fn the_sight_table_is_state_and_ends_with_the_round() {
    let (mut vm, mut g) = sight();
    tick(&mut vm, &mut g, 0);
    tick(&mut vm, &mut g, 1);
    let bytes = vm.ext().encode();
    let back = RulesExt::decode(&bytes).unwrap();
    assert_eq!(&back, vm.ext());
    assert_eq!(back.sight.len(), 1);
    assert_eq!((back.sight[0].asked, back.sight[0].clear), (1, true));
    // A machine resumed from the bytes reads the same and traces the same.
    let ir = compile(SIGHT).unwrap();
    let mut other = Vm::resume(ir, 0, back).unwrap();
    let mut g2 = FakeGame::with_players(1);
    g2.label("eye", EYE, 0, EYE);
    g2.label("target", TARGET, 0, TARGET);
    g2.clear_sight.insert((EYE, TARGET));
    tick(&mut vm, &mut g, 2);
    tick(&mut other, &mut g2, 2);
    assert_eq!(vm.ext().encode(), other.ext().encode());
    // The round's end clears the table.
    vm.fire(&mut g, Input::RoundEnd);
    assert!(vm.sight().is_empty());
}

#[test]
fn physics_reads_have_named_host_entries() {
    for (src, e) in [
        ("on tick { for each players p { if p.crouching { score(p, 1) } } }", id::CROUCHING),
        ("on tick { for each players p { if p.primed { score(p, 1) } } }", id::PRIMED),
        ("on tick { for each players p { if airborne(p) { score(p, 1) } } }", id::AIRBORNE),
        ("on tick { if sees(eye, target) { end_round(team 0) } }", id::SEES),
    ] {
        let full = format!("rules \"H\" 1\ntype slayer\nlabel eye: object\nlabel target: object\n{src}\n");
        let (ir, _) = build(&full).unwrap();
        assert_eq!(unhostable(&ir), None);
        assert!(caps::ENTRIES[e as usize].status.hostable());
    }
    // The call is a plain op, which the verifier knows.
    let (ir, _) = build("rules \"H\" 1\ntype slayer\non tick { for each players p { if p.primed { score(p, 1) } } }\n").unwrap();
    assert!(
        ir.handlers
            .iter()
            .flat_map(|h| &h.code)
            .any(|op| matches!(op, Op::Call { entry, .. } if *entry == id::PRIMED))
    );
}

#[test]
fn a_scenario_sets_the_bodies_the_sight_and_a_touchdown() {
    let s = Scenario {
        players: vec![ScenarioPlayer {
            slot: 0,
            team: 0,
            alive: true,
            score: 0,
            lives: 0,
            unit: None,
            body: ScenarioBody {
                airborne: true,
                crouching: false,
                primed: true,
            },
        }],
        ticks: vec![
            ScenarioTick::default(),
            ScenarioTick {
                events: vec![ScenarioEvent::Touchdown {
                    player: 0,
                    down: fixed_at(9),
                    up: fixed_at(12),
                }],
                ..ScenarioTick::default()
            },
        ],
        ..Scenario::default()
    };
    let (ir, _) = build(CHAIN).unwrap();
    let r = scenario::run(ir.clone(), &s).unwrap();
    assert_eq!(
        r.game.calls_to("score").len(),
        1,
        "primed at the start, landed with a launch"
    );
    assert!(
        r.game.airborne.is_empty(),
        "the touchdown landed the player"
    );
    // The same run through JSON.
    let json = r#"{"players":[{"slot":0,"body":{"airborne":true,"primed":true}}],
                   "sight":[[1,2]],
                   "ticks":[{},{"events":[{"touchdown":{"player":0,"down":589824,"up":786432}}]}]}"#;
    let parsed: Scenario = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.sight, [[1, 2]]);
    let r = scenario::run(ir, &parsed).unwrap();
    assert_eq!(r.game.calls_to("score").len(), 1);
    assert!(r.game.clear_sight.contains(&(1, 2)));
    let bad: Result<Scenario, _> =
        serde_json::from_str(r#"{"players":[{"slot":0,"body":{"flying":true}}]}"#);
    assert!(bad.is_err(), "an unknown body field is an error");
}

#[test]
fn launch_and_grounded_are_events_with_their_arguments_and_caps() {
    for (e, name, args) in [
        (
            Event::Launch,
            "launch",
            &[rules::Ty::Player, rules::Ty::Object, rules::Ty::Fixed][..],
        ),
        (Event::Grounded, "grounded", &[rules::Ty::Player][..]),
    ] {
        assert_eq!(Event::from_code(e.code()), Some(e));
        assert_eq!(Event::named(name), Some(e));
        assert_eq!(e.args(), args);
        assert_eq!(e.worst_per_tick(), 16);
    }
    // The strip is a zone that may be `none`; the launch a `fixed`.
    compile(
        "rules \"L\" 1\ntype slayer\non launch(p, spot, speed) { if spot == none and speed > 8.0 { score(p, 1) } }\n",
    )
    .unwrap();
    assert!(compile("rules \"L\" 1\ntype slayer\non launch(p, spot, speed, x) { }\n").is_err());
    assert!(compile("rules \"L\" 1\ntype slayer\non grounded(p, x) { }\n").is_err());
}

/// A script's chain on the host's events: a launch in a run adds to it, a
/// launch after the ground starts it again, and a launch off no marked
/// strip hears `none`.
const RUN: &str = r#"
rules "Run" 1
type slayer
player var chain: int = 0
player var flying: bool = false
player var unmarked: int = 0
on launch(p, spot, speed) {
    if p.flying { p.chain = p.chain + 1 } else { p.chain = 1 }
    p.flying = true
    if spot == none { p.unmarked = p.unmarked + 1 }
    score(p, p.chain)
}
on grounded(p) { p.flying = false }
"#;

#[test]
fn a_run_of_launches_counts_until_the_ground() {
    let (ir, _) = build(RUN).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = FakeGame::with_players(2);
    let launch = |player: u8, spot: Option<u32>| Input::Launch {
        player,
        spot,
        speed: fixed_at(12),
    };
    vm.begin_tick(0);
    for input in [
        launch(0, Some(77)),
        launch(0, None),
        launch(1, Some(77)),
        Input::Grounded(0),
        launch(0, Some(78)),
    ] {
        assert!(g.apply(&input));
        assert_eq!(vm.fire(&mut g, input), Fired::Ran);
    }
    assert_eq!(vm.player_var(0, "chain"), Some(1));
    assert_eq!(vm.player_var(0, "unmarked"), Some(1));
    assert_eq!(vm.player_var(1, "chain"), Some(1));
    let points: Vec<_> = g.calls_to("score").into_iter().map(|a| a[1].clone()).collect();
    assert_eq!(
        points,
        [Arg::Int(1), Arg::Int(2), Arg::Int(1), Arg::Int(1)]
    );
    assert!(g.airborne.contains(&0), "the fake puts a launched player in the air");
}

#[test]
fn a_scenario_carries_launch_and_grounded() {
    let json = r#"{"players":[{"slot":0}],
                   "ticks":[{"events":[{"launch":{"player":0,"spot":77,"speed":786432}},
                                       {"launch":{"player":0,"speed":786432}}]},
                            {"events":[{"grounded":0},{"launch":{"player":0,"spot":77,"speed":786432}}]}]}"#;
    let s: Scenario = serde_json::from_str(json).unwrap();
    assert_eq!(
        s.ticks[0].events[1],
        ScenarioEvent::Launch {
            player: 0,
            spot: None,
            speed: fixed_at(12)
        }
    );
    assert_eq!(s.ticks[1].events[0].input().unwrap(), Input::Grounded(0));
    let (ir, _) = build(RUN).unwrap();
    let r = scenario::run(ir, &s).unwrap();
    let points: Vec<_> = r
        .game
        .calls_to("score")
        .into_iter()
        .map(|a| a[1].clone())
        .collect();
    assert_eq!(points, [Arg::Int(1), Arg::Int(2), Arg::Int(1)]);
}
