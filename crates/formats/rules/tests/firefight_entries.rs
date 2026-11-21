//! The entries and the event the Firefight director script adds: `now`,
//! `random`, the queued `ff_*` host-log entries and `on actor_assist`.

use rules::fake::FakeGame;
use rules::vm::{Arg, seed, splitmix64};
use rules::{Input, Vm, build};

const SRC: &str = r#"
rules "Entries" 1
carrier slayer
requires firefight.director
global var t: int = 0
global var r: int = 0
global var z: int = 7
on tick {
    t = now()
    r = random(10)
    z = random(0)
}
on actor_assist(killer, assist, class) {
    if assist != none and assist != killer { ff_score(assist, 2, true) }
}
"#;

#[test]
fn now_is_the_begun_tick_and_random_draws_modulo_n() {
    let (ir, _) = build(SRC).unwrap();
    let mut state = seed(5, &ir);
    let mut vm = Vm::new(ir, 0, 5).unwrap();
    let mut g = FakeGame::with_players(1);
    vm.begin_tick(42);
    vm.update(&mut g);
    assert_eq!(vm.global("t"), Some(42));
    assert_eq!(vm.global("r"), Some((splitmix64(&mut state) % 10) as i32));
    // random(0) is 0 and draws nothing.
    assert_eq!(vm.global("z"), Some(0));
    assert_eq!(vm.ext().rng, state);
}

#[test]
fn actor_assist_carries_the_killer_the_assist_and_the_class() {
    let (ir, _) = build(SRC).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = FakeGame::with_players(2);
    vm.begin_tick(1);
    let assist = |killer, assist| Input::ActorAssist {
        killer,
        assist,
        class: "elite".into(),
    };
    vm.fire(&mut g, assist(Some(0), Some(1)));
    vm.fire(&mut g, assist(Some(1), Some(1)));
    vm.fire(&mut g, assist(Some(0), None));
    // The ff_* entries are queued for the tick boundary.
    assert!(g.calls.is_empty());
    vm.flush_queue(&mut g);
    assert_eq!(
        g.calls_to("ff_score"),
        [&vec![Arg::Player(Some(1)), Arg::Int(2), Arg::Bool(true)]]
    );
}

#[test]
fn the_director_entries_need_their_capability() {
    let src = SRC.replace("requires firefight.director\n", "");
    let error = build(&src).unwrap_err().to_string();
    assert!(error.contains("firefight.director"), "{error}");
}
