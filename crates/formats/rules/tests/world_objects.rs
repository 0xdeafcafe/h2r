//! Objects in motion (docs/game/rules-world-api.md, "Objects"): the `vec`
//! front-end type, position reads, `teleport`, movers and `on arrive`, on
//! the fake game. Every invented fixture; no game data.

use rules::fake::FakeGame;
use rules::ty::fixed;
use rules::vm::{ErrorKind, GATE_DROP, Gate, Input, MAX_GATES, MAX_MOVERS, Mover, RulesExt};
use rules::{Severity, Vm, compile, diagnose};

const PLATFORM: u32 = 0x4001_0000;

fn at(x: i32, y: i32, z: i32) -> [i32; 3] {
    [fixed::from_int(x), fixed::from_int(y), fixed::from_int(z)]
}

/// A script with one labelled platform, risen four units over ten ticks
/// at the round's start, that counts the arrivals it hears.
const LIFT: &str = "rules \"Lift\" 1
type slayer
requires objects.move
label platform: objects
global var arrived: int = 0
global var last: int = 0
on round_start {
    for each platform p { move_to(p, p.position + vec(0.0, 0.0, 4.0), 10t) }
}
on arrive(o) {
    arrived = arrived + 1
    last = label_number(o)
}
";

fn lift() -> (Vm, FakeGame) {
    let ir = compile(LIFT).unwrap();
    let mut g = FakeGame::with_players(1);
    g.label("platform", PLATFORM, 7, 1);
    g.object_positions.insert(PLATFORM, at(0, 0, 2));
    (Vm::new(ir, 0, 0).unwrap(), g)
}

fn run_to(vm: &mut Vm, g: &mut FakeGame, from: u32, to: u32) {
    for t in from..=to {
        vm.begin_tick(t);
        vm.update(g);
    }
}

#[test]
fn a_mover_reaches_its_end_on_the_stated_tick() {
    let (mut vm, mut g) = lift();
    vm.begin_tick(0);
    vm.fire(&mut g, Input::RoundStart);
    assert_eq!(
        vm.movers(),
        [Mover {
            object: PLATFORM,
            from: at(0, 0, 2),
            to: at(0, 0, 6),
            start: 0,
            duration: 10,
        }]
    );
    assert!(g.steps.is_empty(), "no step on the tick it started");
    run_to(&mut vm, &mut g, 1, 5);
    assert_eq!(g.object_positions[&PLATFORM], at(0, 0, 4), "half way on tick 5");
    assert_eq!(g.object_velocities[&PLATFORM], [0, 0, fixed::from_int(12)], "4 units in 10 ticks, 12 a second");
    run_to(&mut vm, &mut g, 6, 9);
    assert_eq!(vm.global("arrived"), Some(0), "not before the end tick");
    run_to(&mut vm, &mut g, 10, 10);
    assert_eq!(vm.global("arrived"), Some(1), "on tick 10 itself");
    assert_eq!(vm.global("last"), Some(7), "the handler got the object");
    assert_eq!(g.object_positions[&PLATFORM], at(0, 0, 6), "exactly on the target");
    assert_eq!(g.object_velocities[&PLATFORM], [0; 3]);
    assert!(vm.movers().is_empty());
    assert_eq!(g.steps.len(), 10, "one step a tick");
    run_to(&mut vm, &mut g, 11, 15);
    assert_eq!((vm.global("arrived"), g.steps.len()), (Some(1), 10), "an ended mover steps no more");
}

#[test]
fn a_mover_survives_a_checkpoint_revert() {
    let (mut vm, mut g) = lift();
    vm.begin_tick(0);
    vm.fire(&mut g, Input::RoundStart);
    run_to(&mut vm, &mut g, 1, 4);
    // The checkpoint: the record's bytes and the world as it stood.
    let record = vm.ext().encode();
    let world = g.clone();
    run_to(&mut vm, &mut g, 5, 10);
    let end = vm.ext().encode();
    assert_eq!(vm.global("arrived"), Some(1));

    // Revert and play the same ticks again.
    let mut back = Vm::resume(vm.ir().clone(), 0, RulesExt::decode(&record).unwrap()).unwrap();
    let mut g2 = world;
    assert_eq!(back.movers().len(), 1, "the mover is in the record");
    run_to(&mut back, &mut g2, 5, 10);
    assert_eq!(back.ext().encode(), end, "the same state at the end");
    assert_eq!(g2.object_positions, g.object_positions);
    assert_eq!(back.global("arrived"), Some(1));
}

/// A film re-runs the VM on the same inputs: every tick's record is the
/// same bytes, so the same hash.
#[test]
fn a_film_of_a_mover_replays_to_the_same_record_every_tick() {
    let play = || {
        let (mut vm, mut g) = lift();
        let mut records = Vec::new();
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        records.push(vm.ext().encode());
        for t in 1..=12 {
            vm.begin_tick(t);
            vm.update(&mut g);
            records.push(vm.ext().encode());
        }
        (records, g.steps)
    };
    let (a, steps_a) = play();
    let (b, steps_b) = play();
    assert_eq!(a, b);
    assert_eq!(steps_a, steps_b);
    assert_ne!(a[0], a[5], "the record changes while it moves");
}

#[test]
fn stop_ends_a_mover_with_no_arrival_and_move_to_restarts_it() {
    let src = "rules \"Stop\" 1
type slayer
requires objects.move
label platform: objects
global var arrived: int = 0
global var phase: int = 0
on round_start { for each platform p { move_to(p, p.position + vec(10.0, 0.0, 0.0), 10t) } }
on tick {
    phase = phase + 1
    for each platform p {
        if phase == 3 { stop(p) }
        if phase == 5 { move_to(p, vec(0.0, 0.0, 0.0), 4t) }
    }
}
on arrive(o) { arrived = arrived + 1 }
";
    let mut g = FakeGame::with_players(1);
    g.label("platform", PLATFORM, 0, 1);
    g.object_positions.insert(PLATFORM, at(0, 0, 0));
    let mut vm = Vm::new(compile(src).unwrap(), 0, 0).unwrap();
    vm.begin_tick(0);
    vm.fire(&mut g, Input::RoundStart);
    run_to(&mut vm, &mut g, 1, 3);
    assert!(vm.movers().is_empty(), "stopped on the third tick");
    let stopped_at = g.object_positions[&PLATFORM];
    assert_eq!(stopped_at, at(3, 0, 0), "three steps of a unit: the stop comes after the tick's step");
    run_to(&mut vm, &mut g, 4, 4);
    assert_eq!(g.object_positions[&PLATFORM], stopped_at, "it stays where it stopped");
    run_to(&mut vm, &mut g, 5, 5);
    assert_eq!(vm.movers().len(), 1, "a new move_to starts a new mover from where it is");
    assert_eq!(vm.movers()[0].from, stopped_at);
    run_to(&mut vm, &mut g, 6, 9);
    assert_eq!(vm.global("arrived"), Some(1), "only the second mover arrives");
    assert_eq!(g.object_positions[&PLATFORM], at(0, 0, 0));
}

#[test]
fn a_new_move_to_replaces_the_objects_mover_in_place() {
    let src = "rules \"Again\" 1
type slayer
requires objects.move
label platform: objects
on round_start {
    for each platform p {
        move_to(p, vec(1.0, 0.0, 0.0), 10t)
        move_to(p, vec(2.0, 0.0, 0.0), 20t)
    }
}
";
    let mut g = FakeGame::with_players(1);
    g.label("platform", PLATFORM, 0, 1);
    let mut vm = Vm::new(compile(src).unwrap(), 0, 0).unwrap();
    vm.begin_tick(3);
    vm.fire(&mut g, Input::RoundStart);
    assert_eq!(vm.movers().len(), 1);
    assert_eq!((vm.movers()[0].to, vm.movers()[0].duration, vm.movers()[0].start), (at(2, 0, 0), 20, 3));
}

#[test]
fn a_mover_for_every_object_up_to_sixteen_and_a_seventeenth_stops_the_script() {
    let src = "rules \"Many\" 1
type slayer
requires objects.move
label platform: objects
on round_start { for each platform p { move_to(p, vec(1.0, 1.0, 1.0), 5t) } }
";
    let run = |n: u32| {
        let mut g = FakeGame::with_players(1);
        for i in 0..n {
            g.label("platform", PLATFORM + i, i as i32, i);
        }
        let mut vm = Vm::new(compile(src).unwrap(), 0, 0).unwrap();
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        (vm.movers().len(), g.script_error.map(|e| e.kind))
    };
    assert_eq!(run(MAX_MOVERS as u32), (MAX_MOVERS, None));
    assert_eq!(run(MAX_MOVERS as u32 + 1), (MAX_MOVERS, Some(ErrorKind::Movers)));
}

#[test]
fn a_move_with_no_time_takes_one_tick() {
    let src = "rules \"Quick\" 1
type slayer
requires objects.move
label platform: objects
global var arrived: int = 0
on round_start { for each platform p { move_to(p, vec(1.0, 0.0, 0.0), 0t) } }
on arrive(o) { arrived = arrived + 1 }
";
    let (mut vm, mut g) = (
        Vm::new(compile(src).unwrap(), 0, 0).unwrap(),
        FakeGame::with_players(1),
    );
    g.label("platform", PLATFORM, 0, 1);
    vm.begin_tick(0);
    vm.fire(&mut g, Input::RoundStart);
    assert_eq!(vm.movers()[0].duration, 1);
    run_to(&mut vm, &mut g, 1, 1);
    assert_eq!((vm.global("arrived"), g.object_positions[&PLATFORM]), (Some(1), at(1, 0, 0)));
}

#[test]
fn teleport_moves_an_object_and_a_position_outside_the_world_is_a_refused_call() {
    let src = "rules \"Pad\" 1
type slayer
requires objects.move
label rider: object
label pad: object
global var phase: int = 0
on tick {
    phase = phase + 1
    if phase == 1 { teleport(rider, vec(10.0, 20.0, 1.0)) }
    if phase == 2 { teleport(rider, vec(500.0, 0.0, 0.0)) }
    if phase == 3 { teleport(rider, pad) }
    if phase == 4 { teleport(rider, pad, vec(0.0, 0.0, 2.0)) }
}
";
    let ir = compile(src).unwrap();
    let mut g = FakeGame::with_players(1);
    g.label("rider", PLATFORM, 0, 1);
    g.label("pad", PLATFORM + 1, 0, 2);
    g.object_positions.insert(PLATFORM + 1, at(30, 40, 5));
    g.world_limit = Some(fixed::from_int(100));
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let tick = |vm: &mut Vm, g: &mut FakeGame, t: u32| {
        vm.begin_tick(t);
        vm.update(g);
        vm.flush_queue(g);
    };
    tick(&mut vm, &mut g, 1);
    assert_eq!(g.object_positions[&PLATFORM], at(10, 20, 1));
    tick(&mut vm, &mut g, 2);
    assert_eq!(g.object_positions[&PLATFORM], at(10, 20, 1), "outside the world: nothing moved");
    assert_eq!(g.refused.len(), 1);
    assert_eq!(g.refused[0].0, "teleport");
    assert!(g.script_error.is_none(), "a refused call is recorded, not a script error");
    tick(&mut vm, &mut g, 3);
    assert_eq!(g.object_positions[&PLATFORM], at(30, 40, 5), "to an object's place");
    tick(&mut vm, &mut g, 4);
    assert_eq!(g.object_positions[&PLATFORM], at(30, 40, 7), "to an object's place and an offset");
    assert_eq!(g.calls_to("teleport").len(), 4);
}

#[test]
fn vec_arithmetic_reads_and_a_let() {
    let src = "rules \"Math\" 1
type slayer
requires objects.move
label rider: object
global var sum_x: fixed = 0.0
global var diff_z: fixed = 0.0
global var scaled_y: fixed = 0.0
global var scaled_x: fixed = 0.0
global var out_len: fixed = 0.0
global var out_dist: fixed = 0.0
global var out_speed: fixed = 0.0
global var vel_y: fixed = 0.0
on tick {
    let a = vec(1.0, 2.0, 3.0)
    let b = vec(10.0, 20.0, 30.0)
    let s = a + b
    let d = b - a
    let twice = a * 2.0
    let thrice = 3 * a
    sum_x = s.x
    diff_z = d.z
    scaled_y = twice.y
    scaled_x = thrice.x
    out_len = length(vec(3.0, 4.0, 0.0))
    out_dist = distance(b, a)
    out_speed = rider.speed
    vel_y = rider.velocity.y
}
";
    let ir = compile(src).unwrap();
    let mut g = FakeGame::with_players(1);
    g.label("rider", PLATFORM, 0, 1);
    g.object_velocities.insert(PLATFORM, at(0, 3, 4));
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    vm.begin_tick(1);
    vm.update(&mut g);
    let fx = fixed::from_int;
    assert_eq!(vm.global("sum_x"), Some(fx(11)));
    assert_eq!(vm.global("diff_z"), Some(fx(27)));
    assert_eq!(vm.global("scaled_y"), Some(fx(4)));
    assert_eq!(vm.global("scaled_x"), Some(fx(3)));
    assert_eq!(vm.global("out_len"), Some(fx(5)));
    assert_eq!(vm.global("out_speed"), Some(fx(5)));
    assert_eq!(vm.global("vel_y"), Some(fx(3)));
    // (9, 18, 27): the square of its length, 1134, is far past 16.16's
    // 32768, and the length is still exact: 9 * sqrt(14), floored.
    let want = {
        let (x, y, z) = (i128::from(fx(9)), i128::from(fx(18)), i128::from(fx(27)));
        let sum = (x * x + y * y + z * z) as f64;
        sum.sqrt().floor() as i32
    };
    assert_eq!(vm.global("out_dist"), Some(want));
}

#[test]
fn an_object_with_no_position_reads_zero() {
    let src = "rules \"Zero\" 1
type slayer
label rider: object
global var x: fixed = 1.0
on tick { x = rider.position.x }
";
    let mut g = FakeGame::with_players(1);
    g.label("rider", PLATFORM, 0, 1);
    let mut vm = Vm::new(compile(src).unwrap(), 0, 0).unwrap();
    vm.begin_tick(1);
    vm.update(&mut g);
    assert_eq!(vm.global("x"), Some(0));
    assert!(g.reads.contains(&"object_pos"));
}

// ---- the type errors ----

const HEAD: &str = "rules \"T\" 1\ntype slayer\nrequires objects.move\nlabel o: object\nlabel p: object\n";

fn error_of(body: &str, code: &str) -> String {
    let src = format!("{HEAD}{body}");
    let c = diagnose(&src);
    match c.diagnostics.iter().find(|d| d.code == code && d.severity == Severity::Error) {
        Some(d) => d.message.clone(),
        None => panic!("no {code} error in {:#?}\nfor\n{src}", c.diagnostics),
    }
}

#[test]
fn each_vec_type_error() {
    // The wrong number of components.
    assert!(error_of("on tick { let v = vec(1.0, 2.0) }\n", "type.mismatch").contains("vec takes 3"));
    // A component that is not a fixed.
    let m = error_of("on tick { let n = 3\n let v = vec(n, 0.0, 0.0) }\n", "type.mismatch");
    assert!(m.contains("expected fixed"), "{m}");
    // A field a vec does not have.
    assert!(error_of("on tick { let v = vec(1.0, 2.0, 3.0)\n let w = v.w }\n", "name.unknown").contains("x, y and z"));
    // A vec plus a fixed, and a fixed times a vec read first.
    let m = error_of("on tick { let v = vec(1.0, 2.0, 3.0) + 1.0 }\n", "type.mismatch");
    assert!(m.contains("expected vec"), "{m}");
    let m = error_of("on tick { let k = 2.0\n let v = k * vec(1.0, 2.0, 3.0) }\n", "type.mismatch");
    assert!(m.contains("expected fixed, found vec"), "{m}");
    // A vec times a vec takes a fixed; a vec divided or compared has no
    // such operator.
    let m = error_of("on tick { let v = vec(1.0, 1.0, 1.0) * vec(1.0, 1.0, 1.0) }\n", "type.mismatch");
    assert!(m.contains("expected fixed, found vec"), "{m}");
    for body in [
        "let v = vec(1.0, 1.0, 1.0) / 2.0",
        "let v = vec(1.0, 1.0, 1.0) == vec(1.0, 1.0, 1.0)",
        "let v = vec(1.0, 1.0, 1.0) < vec(1.0, 1.0, 1.0)",
    ] {
        let m = error_of(&format!("on tick {{ {body} }}\n"), "type.mismatch");
        assert!(m.contains("a vec takes"), "{body}: {m}");
    }
    // A vec is a value, not something to store.
    for decl in ["global var v: vec = 0", "player var v: vec = 0"] {
        let m = error_of(&format!("{decl}\n"), "type.mismatch");
        assert!(m.contains("not a type to store"), "{decl}: {m}");
    }
    let m = error_of("on tick { let v: vec = vec(1.0, 1.0, 1.0) }\n", "type.mismatch");
    assert!(m.contains("not a type to store"), "{m}");
    let m = error_of("fn f(v: vec) {}\non tick {}\n", "type.mismatch");
    assert!(m.contains("not a type to store"), "{m}");
    // A vec where a scalar goes, and a scalar where a vec goes.
    assert!(error_of("global var x: fixed = 0.0\non tick { x = vec(1.0, 1.0, 1.0) }\n", "type.mismatch").contains("found vec"));
    assert!(error_of("on tick { let d = length(1.0) }\n", "type.mismatch").contains("expected vec"));
    assert!(error_of("on tick { let d = distance(vec(0.0, 0.0, 0.0)) }\n", "type.mismatch").contains("distance takes 2"));
}

#[test]
fn each_place_error() {
    // teleport and move_to need the capability.
    let src = "rules \"T\" 1\ntype slayer\nlabel o: object\non tick { teleport(o, vec(0.0, 0.0, 0.0)) }\n";
    let c = diagnose(src);
    assert!(c.diagnostics.iter().any(|d| d.code == "cap.missing"), "{:#?}", c.diagnostics);
    // A place that is neither a vec nor an object.
    let m = error_of("on tick { teleport(o, 3) }\n", "type.mismatch");
    assert!(m.contains("a vec or an object"), "{m}");
    // A vec where an offset form wants an object.
    let m = error_of("on tick { teleport(o, vec(0.0, 0.0, 0.0), vec(0.0, 0.0, 0.0)) }\n", "type.mismatch");
    assert!(m.contains("with an offset"), "{m}");
    // An offset that is not a vec.
    assert!(error_of("on tick { teleport(o, p, 3) }\n", "type.mismatch").contains("expected vec"));
    // A time that is an int.
    let m = error_of("on tick { move_to(o, vec(0.0, 0.0, 0.0), 10) }\n", "unit.mix");
    assert!(m.contains("10t") || m.contains("seconds"), "{m}");
    // Wrong arity.
    assert!(error_of("on tick { move_to(o, vec(0.0, 0.0, 0.0)) }\n", "type.mismatch").contains("takes 3"));
    assert!(error_of("on tick { teleport(o) }\n", "type.mismatch").contains("takes 2"));
    // A maybe-none object.
    let m = error_of(
        "global var q: object? = none\non tick { teleport(q, vec(0.0, 0.0, 0.0)) }\n",
        "null.arg",
    );
    assert!(m.contains("may be none"), "{m}");
    let m = error_of(
        "global var q: object? = none\non tick { teleport(o, q) }\n",
        "null.arg",
    );
    assert!(m.contains("may be none"), "{m}");
}

#[test]
fn a_players_unit_and_the_unbuilt_forms_are_refused_by_name() {
    let m = error_of("on tick { for each players q { teleport(q, vec(0.0, 0.0, 0.0)) } }\n", "type.mismatch");
    assert!(m.contains("(R9)") && m.contains("is a player"), "{m}");
    let m = error_of("on tick { for each players q { move_to(q, vec(0.0, 0.0, 0.0), 10t) } }\n", "type.mismatch");
    assert!(m.contains("(R9)"), "{m}");
    for (word, why) in [("move_along", "`path` label"), ("face", "(R2)"), ("attach", "not traced"), ("detach", "not traced")] {
        let m = error_of(&format!("on tick {{ {word}(o, p) }}\n"), "name.unknown");
        assert!(m.starts_with(&format!("{word} is not built")) && m.contains(why), "{word}: {m}");
    }
}

#[test]
fn a_function_may_not_take_a_built_ins_name() {
    for name in ["vec", "length", "distance"] {
        let src = format!("rules \"T\" 1\ntype slayer\nfn {name}(x: fixed) -> fixed = x\non tick {{ }}\n");
        let c = diagnose(&src);
        assert!(
            c.diagnostics.iter().any(|d| d.code == "decl" && d.message.contains("built-in")),
            "{name}: {:#?}",
            c.diagnostics
        );
    }
}

#[test]
fn on_arrive_names_its_object() {
    let src = format!("{HEAD}on arrive(a, b) {{ }}\n");
    let c = diagnose(&src);
    assert!(c.diagnostics.iter().any(|d| d.code == "event.args"), "{:#?}", c.diagnostics);
    let ok = format!("{HEAD}on arrive(a) {{ }}\n");
    assert!(compile(&ok).is_ok());
}

/// A script that opens its barriers at the round's start and closes them
/// on a global's say-so.
const GATES: &str = "rules \"Gates\" 1
type slayer
requires objects.move
label barrier: objects
global var shut: bool = false
on round_start { for each barrier b { set_open(b, true) } }
on tick { if shut { for each barrier b { set_open(b, false) } } }
";

fn gates(n: u32) -> (Vm, FakeGame) {
    let mut g = FakeGame::with_players(1);
    for i in 0..n {
        g.label("barrier", PLATFORM + i, i as i32, 1);
        g.object_positions.insert(PLATFORM + i, at(i as i32, 5, 2));
    }
    (Vm::new(compile(GATES).unwrap(), 0, 0).unwrap(), g)
}

#[test]
fn set_open_steps_a_barrier_down_and_back_to_the_place_it_closed_at() {
    let (mut vm, mut g) = gates(1);
    vm.begin_tick(0);
    vm.fire(&mut g, Input::RoundStart);
    let home = at(0, 5, 2);
    let open = [home[0], home[1], home[2] - GATE_DROP];
    assert_eq!(vm.gates(), [Gate { object: PLATFORM, home }]);
    assert_eq!(g.steps, [(PLATFORM, open, [0; 3])], "put below its closed place");
    // Asking again changes nothing, and the home is not read from the
    // lowered place.
    vm.begin_tick(1);
    vm.fire(&mut g, Input::RoundStart);
    assert_eq!(vm.gates().len(), 1);
    assert_eq!(g.steps.len(), 1);
    assert_eq!(vm.gates()[0].home, home);
    // Closing steps it back and forgets it; closing a closed one is nothing.
    vm.set_global("shut", 1);
    run_to(&mut vm, &mut g, 2, 3);
    assert!(vm.gates().is_empty());
    assert_eq!(g.steps, [(PLATFORM, open, [0; 3]), (PLATFORM, home, [0; 3])]);
    assert_eq!(g.object_positions[&PLATFORM], home);
    assert_eq!(g.script_error, None);
}

#[test]
fn opening_then_closing_in_one_tick_ends_where_it_began() {
    let src = "rules \"Blink\" 1
type slayer
requires objects.move
label barrier: objects
on round_start { for each barrier b { set_open(b, true)
 set_open(b, false) } }
";
    let mut g = FakeGame::with_players(1);
    g.label("barrier", PLATFORM, 0, 1);
    g.object_positions.insert(PLATFORM, at(1, 2, 3));
    let mut vm = Vm::new(compile(src).unwrap(), 0, 0).unwrap();
    vm.begin_tick(0);
    vm.fire(&mut g, Input::RoundStart);
    assert_eq!(g.steps.last().map(|s| s.1), Some(at(1, 2, 3)));
    assert!(vm.gates().is_empty());
}

#[test]
fn a_seventeenth_open_barrier_stops_the_script_and_the_state_round_trips() {
    for (n, error) in [(MAX_GATES as u32, None), (MAX_GATES as u32 + 1, Some(ErrorKind::Gates))] {
        let (mut vm, mut g) = gates(n);
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        assert_eq!((vm.gates().len(), g.script_error.map(|e| e.kind)), (MAX_GATES, error));
        let ext = RulesExt::decode(&vm.ext().encode()).unwrap();
        assert_eq!(ext.gates, vm.gates());
    }
}

#[test]
fn the_rounds_end_closes_every_open_barrier() {
    let (mut vm, mut g) = gates(2);
    vm.begin_tick(0);
    vm.fire(&mut g, Input::RoundStart);
    assert_eq!(vm.gates().len(), 2);
    vm.begin_tick(1);
    vm.fire(&mut g, Input::RoundEnd);
    assert!(vm.gates().is_empty());
    assert_eq!(g.object_positions[&PLATFORM], at(0, 5, 2), "back where it closed");
    assert_eq!(g.object_positions[&(PLATFORM + 1)], at(1, 5, 2));
}
