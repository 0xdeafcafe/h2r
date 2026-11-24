//! The engine ports and the payoff examples: every script compiles and
//! verifies, names the entries a host lacks, and does its core rule on the
//! fake game.

use std::collections::BTreeSet;

use rules::builtin::{self, builtin, examples};
use rules::caps;
use rules::fake::FakeGame;
use rules::ir::{Op, OptionValue};
use rules::vm::{Arg, Placed};
use rules::{Fired, Input, Ir, Vm, build, compile};

const BALL: &str = "objects/weapons/multiplayer/ball/ball";
const FLAG: &str = "objects/weapons/multiplayer/flag/flag";
const BOMB: &str = "objects/weapons/multiplayer/assault_bomb/assault_bomb";

fn player(p: u8) -> Arg {
    Arg::Player(Some(p))
}

fn object(o: u32) -> Arg {
    Arg::Object(Some(o))
}

fn scores(g: &FakeGame) -> Vec<(u8, i32)> {
    g.calls_to("score")
        .into_iter()
        .map(|args| match args.as_slice() {
            [Arg::Player(Some(p)), Arg::Int(n)] => (*p, *n),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn start(src: &str, g: &mut FakeGame, seed: u64) -> Vm {
    let (ir, report) = build(src).unwrap();
    assert!(report.fits());
    let options = ir.default_options();
    let mut vm = Vm::new(ir, options, seed).unwrap();
    vm.begin_tick(1);
    assert_eq!(vm.fire(g, Input::RoundStart), Fired::Ran);
    vm
}

/// [`start`] with the named options packed, on a game with no carrier slot:
/// the record is the script index's, whose type area is zero.
fn start_scripted(src: &str, g: &mut FakeGame, seed: u64, given: &[(&str, i64)]) -> Vm {
    g.carrier = rules::SCRIPT_INDEX;
    let (ir, report) = build(src).unwrap();
    assert!(report.fits());
    let given: Vec<(String, OptionValue)> = given
        .iter()
        .map(|(k, v)| (k.to_string(), OptionValue::Int(*v)))
        .collect();
    let options = ir.pack_options(&given).unwrap();
    let mut vm = Vm::new(ir, options, seed).unwrap();
    vm.begin_tick(1);
    assert_eq!(vm.fire(g, Input::RoundStart), Fired::Ran);
    vm
}

/// Runs `n` per-tick updates from tick `from`.
fn ticks(vm: &mut Vm, g: &mut FakeGame, from: u32, n: u32) {
    for t in from..from + n {
        vm.begin_tick(t);
        assert_ne!(vm.update(g), Fired::Stopped);
    }
}

fn all_scripts() -> Vec<(String, &'static str)> {
    let mut list: Vec<(String, &'static str)> = (0..=u8::MAX)
        .filter_map(|i| builtin(i).map(|s| (format!("engine {i}"), s)))
        .collect();
    list.extend(examples().iter().map(|&(n, s)| (n.to_string(), s)));
    list
}

/// The entries a script calls that a host cannot make yet.
fn unhostable_entries(ir: &Ir) -> BTreeSet<&'static str> {
    ir.handlers
        .iter()
        .flat_map(|h| &h.code)
        .filter_map(|op| match *op {
            Op::Call { entry, .. } => caps::entry(entry, ir.header.api_version),
            _ => None,
        })
        .filter(|e| !e.hostable_on(ir.header.engine()))
        .map(caps::spelled)
        .collect()
}

/// The entries a script's header comment lists under "Entries a host
/// cannot make yet".
fn listed_entries(src: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut inside = false;
    for line in src.lines() {
        if line.contains("Entries a host cannot make yet") {
            inside = true;
            continue;
        }
        if inside {
            match line.strip_prefix("//   ") {
                Some(rest) if !rest.starts_with(' ') => {
                    out.insert(rest.split_whitespace().next().unwrap_or("").to_string());
                }
                Some(_) => {}
                None => inside = false,
            }
        }
    }
    out
}

#[test]
fn every_script_compiles_verifies_and_fits_a_tick() {
    for (name, src) in all_scripts() {
        let (ir, report) = build(src).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(report.fits(), "{name}: worst tick {}", report.worst_tick);
        let again = compile(src).unwrap();
        assert_eq!(ir.id(), again.id(), "{name}");
    }
    assert_eq!(examples().len(), 7);
}

#[test]
fn each_header_names_exactly_the_entries_a_host_lacks() {
    for (name, src) in all_scripts() {
        let ir = compile(src).unwrap();
        let used: BTreeSet<String> = unhostable_entries(&ir).into_iter().map(String::from).collect();
        assert_eq!(listed_entries(src), used, "{name}");
        assert_eq!(rules::verify::unhostable(&ir).is_none(), used.is_empty(), "{name}");
    }
    // Slayer, VIP, Bounce Hunt and Superbounce need nothing a host
    // lacks.
    for src in [
        builtin::SLAYER,
        builtin::VIP,
        builtin::BOUNCE_HUNT,
        builtin::SUPERBOUNCE,
    ] {
        assert!(unhostable_entries(&compile(src).unwrap()).is_empty());
    }
}

// ---- the language extensions the ports use ----

#[test]
fn a_set_argument_names_its_member() {
    let src = r#"
rules "T" 1
type slayer
global var n: int = 0
on round_start { n = count(players p where p.team == team 1) }
"#;
    let (ir, _) = build(src).unwrap();
    let mut g = FakeGame::with_players(3);
    g.players.get_mut(&2).unwrap().team = 1;
    let mut vm = Vm::new(ir, 0, 1).unwrap();
    vm.begin_tick(1);
    vm.fire(&mut g, Input::RoundStart);
    assert_eq!(vm.global("n"), Some(1));

    let bad = r#"
rules "T" 1
type slayer
on round_start { for each pick(players p, 1) q { score(q, 1) } }
"#;
    assert!(compile(bad).is_err());
}

#[test]
fn objectives_are_visited_by_handle_and_bounded_at_eight() {
    let src = r#"
rules "T" 1
type slayer
global var first_seen: object? = none
global var n: int = 0
on round_start {
    first_seen = first(objectives)
    n = count(objectives o where o.carrier != none)
}
"#;
    let (ir, report) = build(src).unwrap();
    // Two loops of at most 8 records each.
    assert!(report.handler_ops[0] < 8 * 20);
    let mut g = FakeGame::with_players(2);
    for o in [30u32, 10, 20] {
        g.objectives.insert(o, None);
    }
    g.objectives.insert(20, Some(1));
    let mut vm = Vm::new(ir, 0, 1).unwrap();
    vm.begin_tick(1);
    vm.fire(&mut g, Input::RoundStart);
    assert_eq!(vm.global("first_seen"), Some(10));
    assert_eq!(vm.global("n"), Some(1));
}

/// Real object datums are salted handles at or above 2^31; 0xFFFF_FFFF is
/// `none`. The VM keeps all 32 bits.
#[test]
fn salted_object_datums_round_trip_through_the_vm() {
    const A: u32 = 0xE1A3_0012;
    const B: u32 = 0x8000_0001;
    let src = r#"
rules "T" 1
type slayer
global var first_seen: object? = none
global var carried: object? = none
global var n: int = 0
global var absent: bool = false
object var hits: int = 0
on round_start {
    first_seen = first(objectives)
    n = count(objectives o where o.carrier != none)
    for each objectives o { o.hits = o.hits + 1 }
}
on pickup(p, o) {
    carried = o
    absent = (carried == none)
}
"#;
    let (ir, _) = build(src).unwrap();
    let mut g = FakeGame::with_players(2);
    g.objectives.insert(A, Some(1));
    g.objectives.insert(B, None);
    let mut vm = Vm::new(ir, 0, 1).unwrap();
    vm.begin_tick(1);
    assert_eq!(vm.fire(&mut g, Input::RoundStart), Fired::Ran);
    // Sorted by datum as unsigned: B (0x8000_0001) before A.
    assert_eq!(vm.global("first_seen"), Some(B as i32));
    assert_eq!(vm.global("n"), Some(1), "the carried salted ball is read");
    assert_eq!(vm.ext().objects.get(&A).map(|r| r[0]), Some(1), "a record under the full datum");
    assert_eq!(vm.ext().objects.get(&B).map(|r| r[0]), Some(1));
    vm.fire(&mut g, Input::Pickup(0, A));
    assert_eq!(vm.global("carried"), Some(A as i32));
    assert_eq!(vm.global("absent"), Some(0));
    // The host sees the same datum back.
    vm.fire(&mut g, Input::Pickup(0, u32::MAX));
    assert_eq!(vm.global("carried"), Some(-1), "0xFFFF_FFFF is none");
    assert_eq!(vm.global("absent"), Some(1));
}

// ---- Oddball ----

/// An object the weapon system made, in the registry and carried by none.
fn made(g: &mut FakeGame, o: u32) {
    g.objectives.insert(o, None);
}

#[test]
fn oddball_makes_its_balls_one_at_a_time_and_scores_time_held() {
    let mut g = FakeGame::with_players(2);
    g.labelled.insert(
        "oddball_spawn".into(),
        vec![
            Placed { placement: 0, object: 40 },
            Placed { placement: 1, object: 41 },
        ],
    );
    // The type area's ball count is zero on the script index, and the
    // record's own field is ignored: the script's option says how many.
    g.variant.insert("oddball_ball_count".into(), 1);
    let mut vm = start_scripted(builtin::ODDBALL, &mut g, 3, &[("ball_count", 2)]);
    // The first ball is made at the round's start, at the first spawn, and
    // the second once the first is seen, at the second.
    vm.flush_queue(&mut g);
    assert_eq!(
        g.calls_to("spawn_objective"),
        vec![&vec![Arg::Tag(Some(BALL.into())), object(40), Arg::Team(None)]]
    );
    made(&mut g, 100);
    ticks(&mut vm, &mut g, 2, 1);
    vm.flush_queue(&mut g);
    assert_eq!(
        g.calls_to("spawn_objective"),
        vec![
            &vec![Arg::Tag(Some(BALL.into())), object(40), Arg::Team(None)],
            &vec![Arg::Tag(Some(BALL.into())), object(41), Arg::Team(None)],
        ]
    );
    made(&mut g, 101);
    ticks(&mut vm, &mut g, 3, 1);
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("spawn_objective").len(), 2);

    // Held, a ball scores each second.
    vm.fire(&mut g, Input::Pickup(1, 100));
    g.objectives.insert(100, Some(1));
    assert_eq!(g.calls_to("role_traits"), vec![&vec![player(1), Arg::Bool(true)]]);
    ticks(&mut vm, &mut g, 4, 29);
    assert!(scores(&g).is_empty());
    ticks(&mut vm, &mut g, 33, 1);
    assert_eq!(scores(&g), vec![(1, 1)]);

    // The second ball is picked up and dropped: no points, and it returns
    // after the reset time, by the script, to its own spawn.
    vm.fire(&mut g, Input::Pickup(0, 101));
    vm.fire(&mut g, Input::Drop(0, 101));
    g.objectives.insert(101, None);
    ticks(&mut vm, &mut g, 34, 30 * 30 - 1);
    vm.flush_queue(&mut g);
    assert!(g.calls_to("delete").is_empty());
    ticks(&mut vm, &mut g, 34 + 899, 1);
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("delete"), vec![&vec![object(101)]]);
    assert_eq!(
        g.calls_to("spawn_objective").last(),
        Some(&&vec![Arg::Tag(Some(BALL.into())), object(41), Arg::Team(None)])
    );
    assert!(g.calls_to("reset_objective").is_empty());
    assert_eq!(scores(&g).first(), Some(&(1, 1)));

    // The old object may stay in the registry; it is ignored, and the
    // replacement takes the spawn the old one had.
    made(&mut g, 102);
    ticks(&mut vm, &mut g, 934, 2000);
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("delete"), vec![&vec![object(101)]]);
    assert_eq!(g.calls_to("spawn_objective").len(), 3);
}

// ---- Juggernaut ----

#[test]
fn juggernaut_passes_to_its_killer_and_only_it_scores() {
    let mut g = FakeGame::with_players(3);
    let mut vm = start(builtin::JUGGERNAUT, &mut g, 11);
    let j = vm.global("juggernaut").unwrap();
    assert!((0..3).contains(&j));
    let j = j as u8;
    let k = (j + 1) % 3;
    let other = (j + 2) % 3;

    // A kill between the others scores nothing.
    vm.fire(&mut g, Input::Kill { victim: other, killer: Some(k) });
    assert!(scores(&g).is_empty());
    // The Juggernaut's kill scores.
    vm.fire(&mut g, Input::Kill { victim: k, killer: Some(j) });
    assert_eq!(scores(&g), vec![(j, 1)]);
    // Killing the Juggernaut takes the role.
    vm.fire(&mut g, Input::Kill { victim: j, killer: Some(k) });
    assert_eq!(vm.global("juggernaut"), Some(k as i32));
    assert_eq!(
        g.calls_to("role_traits"),
        vec![
            &vec![player(j), Arg::Bool(true)],
            &vec![player(j), Arg::Bool(false)],
            &vec![player(k), Arg::Bool(true)],
        ]
    );
    assert_eq!(scores(&g), vec![(j, 1)]);
    // The new Juggernaut scores; the old one no longer does.
    vm.fire(&mut g, Input::Kill { victim: j, killer: Some(k) });
    vm.fire(&mut g, Input::Kill { victim: other, killer: Some(j) });
    assert_eq!(scores(&g), vec![(j, 1), (k, 1)]);
}

#[test]
fn script_index_carries_the_crown_and_its_trait_choices_across_a_restore() {
    let source = builtin::JUGGERNAUT.replace(
        "type juggernaut",
        r#"type juggernaut
option role_active_camo: bool = true "Camo"
option role_extra_damage: bool = false "Damage""#,
    );
    let mut g = FakeGame::with_players(3);
    g.carrier = rules::SCRIPT_INDEX;
    let mut vm = start(&source, &mut g, 11);
    let holder = vm.global("juggernaut").unwrap() as u8;
    let killer = (holder + 1) % 3;
    assert!(vm.has_role(holder));
    assert!(!vm.has_role(killer));
    assert_eq!(vm.role_trait(1), Some(true));
    assert_eq!(vm.role_trait(2), Some(false));
    assert_eq!(vm.role_trait(3), Some(false));
    assert_eq!(vm.role_trait(4), None);
    vm.fire(&mut g, Input::Kill { victim: holder, killer: Some(killer) });
    let ext = rules::RulesExt::decode(&vm.ext().encode()).unwrap();
    let mut vm = Vm::resume(vm.ir().clone(), vm.ir().default_options(), ext).unwrap();
    assert!(!vm.has_role(holder));
    assert!(vm.has_role(killer));
    vm.fire(&mut g, Input::Quit(killer));
    assert!(!vm.has_role(killer));
    vm.fire(&mut g, Input::RoundStart);
    assert_ne!(vm.ext().role_mask, 0);
    vm.fire(&mut g, Input::RoundEnd);
    assert_eq!(vm.ext().role_mask, 0);
}

// ---- King of the Hill ----

#[test]
fn king_scores_time_on_the_hill_unless_contested() {
    let mut g = FakeGame::with_players(2);
    g.players.get_mut(&1).unwrap().team = 1;
    g.variant.insert("team_play".into(), 1);
    g.labelled.insert(
        "hill_corner".into(),
        vec![
            Placed { placement: 0, object: 500 },
            Placed { placement: 1, object: 501 },
            Placed { placement: 2, object: 510 },
        ],
    );
    g.numbers.extend([(500, 0), (501, 0), (510, 1)]);
    g.inside.insert((0, 500));
    // Player 1 stands on hill 1, which is not in play.
    g.inside.insert((1, 510));
    // A stock record's type area would move the hill every second; the
    // script index's is zero, so only the script's own option counts.
    g.variant.insert("king_moving_hill".into(), 1);
    let mut vm = start_scripted(builtin::KING, &mut g, 5, &[("move_time", 0)]);
    assert_eq!(vm.global("hills"), Some(2));
    ticks(&mut vm, &mut g, 2, 30);
    assert_eq!(scores(&g), vec![(0, 1)]);

    // An enemy on the hill contests it.
    g.inside.insert((1, 500));
    ticks(&mut vm, &mut g, 32, 60);
    assert_eq!(scores(&g), vec![(0, 1)]);

    // A teammate does not.
    g.players.get_mut(&1).unwrap().team = 0;
    ticks(&mut vm, &mut g, 92, 30);
    assert_eq!(scores(&g), vec![(0, 1), (0, 1), (1, 1)]);
    // Hill 0 stayed in play throughout, and the script made no call into a
    // retail objective registry.
    assert_eq!(vm.global("hill"), Some(0));
    assert!(g.calls_to("register_objective").is_empty());
}

#[test]
fn king_moves_the_hill_on_its_own_option() {
    let mut g = FakeGame::with_players(1);
    g.labelled.insert(
        "hill_corner".into(),
        vec![
            Placed { placement: 0, object: 500 },
            Placed { placement: 1, object: 510 },
        ],
    );
    g.numbers.extend([(500, 0), (510, 1)]);
    let mut vm = start_scripted(builtin::KING, &mut g, 5, &[("move_time", 2)]);
    assert_eq!(vm.global("hill"), Some(0));
    // Two seconds are 60 ticks at 30 ticks a second (`caps::TICKS_PER_SECOND`);
    // the timer set at tick 1 runs out at tick 61.
    ticks(&mut vm, &mut g, 2, 57);
    assert_eq!(vm.global("hill"), Some(0));
    ticks(&mut vm, &mut g, 59, 5);
    assert_eq!(vm.global("hill"), Some(1));
}

// ---- CTF and Assault ----

/// Two teams' flag stands (objects 10 and 11) and a game on the script
/// index, whose type area is zero.
fn ctf_game() -> FakeGame {
    let mut g = FakeGame::with_players(2);
    g.players.get_mut(&1).unwrap().team = 1;
    g.variant.insert("team_play".into(), 1);
    g.labelled.insert(
        "ctf_flag_spawn".into(),
        vec![
            Placed { placement: 0, object: 10 },
            Placed { placement: 1, object: 11 },
        ],
    );
    g.object_teams.extend([(10, 0), (11, 1)]);
    g
}

fn flag_at(o: u32, team: u8, g: &mut FakeGame) {
    g.objectives.insert(o, None);
    g.object_teams.insert(o, team);
}

fn made_at(tag: &str, at: u32, team: Option<u8>) -> Vec<Arg> {
    vec![Arg::Tag(Some(tag.into())), object(at), Arg::Team(team)]
}

#[test]
fn ctf_capture_scores_and_returns_the_flag() {
    let mut g = ctf_game();
    // The record's own fields are ignored on the script index.
    g.variant.insert("ctf_touch_return".into(), 1);
    let mut vm = start_scripted(builtin::CTF, &mut g, 2, &[]);
    vm.flush_queue(&mut g);
    assert_eq!(
        g.calls_to("spawn_objective"),
        vec![&made_at(FLAG, 10, Some(0)), &made_at(FLAG, 11, Some(1))]
    );

    // Team 1's flag is object 21; player 0 takes it.
    flag_at(21, 1, &mut g);
    vm.fire(&mut g, Input::Pickup(0, 21));
    g.objectives.insert(21, Some(0));
    assert_eq!(g.calls_to("role_traits"), vec![&vec![player(0), Arg::Bool(true)]]);
    ticks(&mut vm, &mut g, 2, 5);
    assert!(scores(&g).is_empty());

    // At its own flag's spawn: a capture, and the flag goes home: the old
    // object is removed and another is made at team 1's stand.
    g.inside.insert((0, 10));
    ticks(&mut vm, &mut g, 7, 1);
    vm.flush_queue(&mut g);
    assert_eq!(scores(&g), vec![(0, 1)]);
    assert_eq!(g.calls_to("delete"), vec![&vec![object(21)]]);
    assert_eq!(g.calls_to("spawn_objective").last(), Some(&&made_at(FLAG, 11, Some(1))));
    assert!(g.calls_to("reset_objective").is_empty());
    assert_eq!(g.calls_to("role_traits").last(), Some(&&vec![player(0), Arg::Bool(false)]));

    // The removed object may stay in the registry still held; it scores
    // nothing more.
    ticks(&mut vm, &mut g, 8, 10);
    vm.flush_queue(&mut g);
    assert_eq!(scores(&g), vec![(0, 1)]);
    assert_eq!(g.calls_to("delete").len(), 1);
}

#[test]
fn ctf_returns_a_dropped_flag_by_time_and_by_touch() {
    let mut g = ctf_game();
    let mut vm = start_scripted(
        builtin::CTF,
        &mut g,
        2,
        &[("reset_time", 2), ("touch_return", 1)],
    );
    vm.flush_queue(&mut g);

    // Dropped, it returns after the reset time.
    flag_at(21, 1, &mut g);
    vm.fire(&mut g, Input::Pickup(0, 21));
    vm.fire(&mut g, Input::Drop(0, 21));
    g.objectives.insert(21, None);
    ticks(&mut vm, &mut g, 2, 59);
    vm.flush_queue(&mut g);
    assert!(g.calls_to("delete").is_empty());
    ticks(&mut vm, &mut g, 61, 1);
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("delete"), vec![&vec![object(21)]]);
    assert_eq!(g.calls_to("spawn_objective").last(), Some(&&made_at(FLAG, 11, Some(1))));
    // It is not returned twice.
    ticks(&mut vm, &mut g, 62, 120);
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("delete").len(), 1);

    // With touch return, its own team's touch returns it at once.
    flag_at(22, 1, &mut g);
    vm.fire(&mut g, Input::Pickup(0, 22));
    vm.fire(&mut g, Input::Drop(0, 22));
    g.objectives.insert(22, None);
    vm.fire(&mut g, Input::Pickup(1, 22));
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("delete"), vec![&vec![object(21)], &vec![object(22)]]);
    assert_eq!(g.calls_to("spawn_objective").len(), 4);
    assert!(scores(&g).is_empty());

    // Without it, the touch changes nothing.
    let mut g = ctf_game();
    let mut vm = start_scripted(builtin::CTF, &mut g, 2, &[("reset_time", 0)]);
    flag_at(21, 1, &mut g);
    vm.fire(&mut g, Input::Pickup(0, 21));
    vm.fire(&mut g, Input::Drop(0, 21));
    vm.fire(&mut g, Input::Pickup(1, 21));
    ticks(&mut vm, &mut g, 2, 1000);
    vm.flush_queue(&mut g);
    assert!(g.calls_to("delete").is_empty());
}

#[test]
fn ctf_flag_at_home_needs_the_teams_own_flag_at_its_stand() {
    let mut g = ctf_game();
    let mut vm = start_scripted(
        builtin::CTF,
        &mut g,
        2,
        &[("flag_at_home", 1), ("touch_return", 1)],
    );
    // Team 0's flag is out with player 1; player 0 holds team 1's flag at
    // team 0's stand, and cannot capture.
    flag_at(20, 0, &mut g);
    flag_at(21, 1, &mut g);
    vm.fire(&mut g, Input::Pickup(1, 20));
    g.objectives.insert(20, Some(1));
    vm.fire(&mut g, Input::Pickup(0, 21));
    g.objectives.insert(21, Some(0));
    g.inside.insert((0, 10));
    ticks(&mut vm, &mut g, 2, 5);
    assert!(scores(&g).is_empty());

    // Dropped and touched by team 0, the flag returns, and the capture scores.
    vm.fire(&mut g, Input::Drop(1, 20));
    g.objectives.insert(20, None);
    vm.fire(&mut g, Input::Pickup(0, 20));
    g.objectives.insert(20, None);
    ticks(&mut vm, &mut g, 7, 1);
    vm.flush_queue(&mut g);
    assert_eq!(scores(&g), vec![(0, 1)]);
    assert_eq!(g.calls_to("delete"), vec![&vec![object(20)], &vec![object(21)]]);
}

#[test]
fn ctf_with_one_flag_makes_only_the_defenders() {
    let mut g = ctf_game();
    let mut vm = start_scripted(builtin::CTF, &mut g, 2, &[("flag_type", 1)]);
    vm.flush_queue(&mut g);
    // The defender swaps at each round start, from team 1 to team 0 first.
    assert_eq!(g.calls_to("spawn_objective"), vec![&made_at(FLAG, 10, Some(0))]);
    assert_eq!(vm.global("defender"), Some(0));
    vm.fire(&mut g, Input::RoundStart);
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("spawn_objective").last(), Some(&&made_at(FLAG, 11, Some(1))));
    assert_eq!(g.calls_to("spawn_objective").len(), 2);
}

#[test]
fn assault_arms_a_bomb_on_the_enemy_plate() {
    let mut g = FakeGame::with_players(2);
    g.players.get_mut(&1).unwrap().team = 1;
    g.labelled.insert("assault_bomb_spawn".into(), vec![Placed { placement: 0, object: 10 }]);
    g.labelled.insert("assault_bomb_return".into(), vec![Placed { placement: 1, object: 12 }]);
    g.object_teams.extend([(10, 0), (12, 1)]);
    let mut vm = start_scripted(builtin::ASSAULT, &mut g, 2, &[("arm_time", 1)]);
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("spawn_objective"), vec![&made_at(BOMB, 10, Some(0))]);
    // Player 0 carries team 0's bomb onto team 1's plate.
    flag_at(30, 0, &mut g);
    g.objectives.insert(30, Some(0));
    vm.fire(&mut g, Input::Pickup(0, 30));
    g.inside.insert((0, 12));
    ticks(&mut vm, &mut g, 2, 29);
    assert!(scores(&g).is_empty());
    ticks(&mut vm, &mut g, 31, 1);
    vm.flush_queue(&mut g);
    assert_eq!(scores(&g), vec![(0, 1)]);
    assert_eq!(g.calls_to("delete"), vec![&vec![object(30)]]);
    assert_eq!(g.calls_to("spawn_objective").last(), Some(&&made_at(BOMB, 10, Some(0))));
    assert!(g.calls_to("reset_objective").is_empty());
}

#[test]
fn assault_with_one_bomb_is_neutral_and_armed_on_the_defenders_plate() {
    let mut g = FakeGame::with_players(2);
    g.players.get_mut(&1).unwrap().team = 1;
    g.labelled.insert("assault_bomb_spawn".into(), vec![Placed { placement: 0, object: 14 }]);
    g.labelled.insert(
        "assault_bomb_return".into(),
        vec![Placed { placement: 1, object: 12 }, Placed { placement: 2, object: 13 }],
    );
    g.object_teams.extend([(12, 0), (13, 1)]);
    let mut vm = start_scripted(
        builtin::ASSAULT,
        &mut g,
        2,
        &[("flag_type", 1), ("arm_time", 1)],
    );
    vm.flush_queue(&mut g);
    // One neutral bomb, and team 0 defends the first round.
    assert_eq!(g.calls_to("spawn_objective"), vec![&made_at(BOMB, 14, None)]);
    assert_eq!(vm.global("defender"), Some(0));
    // The attacker, player 1, carries it onto the defenders' plate only.
    flag_at(31, 0, &mut g);
    g.object_teams.remove(&31);
    vm.fire(&mut g, Input::Pickup(1, 31));
    g.objectives.insert(31, Some(1));
    g.inside.insert((1, 13));
    ticks(&mut vm, &mut g, 2, 60);
    assert!(scores(&g).is_empty());
    g.inside.clear();
    g.inside.insert((1, 12));
    ticks(&mut vm, &mut g, 62, 31);
    vm.flush_queue(&mut g);
    assert_eq!(scores(&g), vec![(1, 1)]);
    assert_eq!(g.calls_to("delete"), vec![&vec![object(31)]]);
    assert_eq!(g.calls_to("spawn_objective").last(), Some(&&made_at(BOMB, 14, None)));
}

// ---- Territories ----

#[test]
fn territories_held_score_each_interval() {
    let mut g = FakeGame::with_players(2);
    g.players.get_mut(&1).unwrap().team = 1;
    // The type area's fields are zero on the script index; they are
    // ignored, as the script's options say how many and how long.
    g.variant.insert("territories_count".into(), 1);
    g.labelled.insert(
        "territory".into(),
        vec![
            Placed { placement: 0, object: 600 },
            Placed { placement: 1, object: 601 },
            Placed { placement: 2, object: 602 },
        ],
    );
    let mut vm = start_scripted(
        builtin::TERRITORIES,
        &mut g,
        4,
        &[("in_play", 2), ("contest_time", 1)],
    );
    // The script tracks its own territories: none is registered with the
    // game, and the third is out of play.
    assert!(g.calls_to("register_objective").is_empty());
    assert_eq!(
        g.calls_to("show_area"),
        vec![&vec![object(600), Arg::Team(None)], &vec![object(601), Arg::Team(None)]]
    );

    // Player 0 takes 600 in a second (contest time 0 reads as 1); the next
    // interval pays for it, and keeps paying while it is held.
    g.inside.insert((0, 600));
    ticks(&mut vm, &mut g, 2, 59);
    assert!(scores(&g).is_empty());
    ticks(&mut vm, &mut g, 61, 1);
    assert_eq!(scores(&g), vec![(0, 1)]);
    g.inside.clear();
    ticks(&mut vm, &mut g, 62, 30);
    assert_eq!(scores(&g), vec![(0, 1), (0, 1)]);

    // A contested territory is not taken.
    g.inside.extend([(0, 601), (1, 601)]);
    g.variant.insert("team_play".into(), 1);
    ticks(&mut vm, &mut g, 92, 90);
    assert_eq!(scores(&g).iter().filter(|s| s.0 == 1).count(), 0);
}

// ---- Infection ----

#[test]
fn infection_switches_the_victims_team() {
    let mut g = FakeGame::with_players(4);
    g.round_time_left = 100;
    let mut vm = start(builtin::INFECTION, &mut g, 99);
    let zombie = (0..4u8)
        .find(|&p| vm.player_var(p, "infected") == Some(1))
        .unwrap();
    let human = (zombie + 1) % 4;
    assert_eq!(g.players[&human].team, 0);
    vm.fire(&mut g, Input::Kill { victim: human, killer: Some(zombie) });
    assert_eq!(g.players[&human].team, 1);
    assert_eq!(vm.player_var(human, "infected"), Some(1));
    assert_eq!(
        g.calls_to("set_team").last(),
        Some(&&vec![player(human), Arg::Team(Some(1))])
    );
    assert_eq!(scores(&g), vec![(zombie, 1)]);
}

// ---- Superbounce ----

/// A bounce at chain count `chain` by `player` off strip `spot`.
fn bounce(player: u8, spot: u32, chain: u32, monster: bool) -> Input {
    Input::Bounce {
        player,
        spot,
        chain,
        monster,
    }
}

#[test]
fn superbounce_scores_bounces_only_by_their_chain() {
    let (ir, report) = build(builtin::SUPERBOUNCE).unwrap();
    assert!(report.fits());
    assert_eq!(ir.header.name, "Superbounce");
    assert_eq!(ir.header.engine(), 2);
    assert_eq!(ir.header.game_type, rules::types::Type::Research);
    assert_eq!(ir.header.requires, ["hud.superbounce"]);
    // The options in the order the record packs them: the suicide penalty
    // (bit 0), bounce points raw 1 (bits 1 to 3), the Bounced penalty
    // (bit 4), the Monster double (bit 5); then the HUD's, which the host
    // reads: the seam targets (bits 6 and 7, `likely` raw 1), velocity
    // (bit 8), the fall alignment (bit 9), the miss info (bit 10), the
    // medals (bit 11), the second bounce's targets (bit 12), the approach
    // guide (bit 13) and the charge segments (bit 14), each on; then the
    // Pogo Stick's, the host's too: the stick (bit 15, on) and its style
    // (bits 16 and 17, `unsc` raw 0).
    let names: Vec<&str> = (0..ir.options.len()).map(|i| ir.option_name(i)).collect();
    assert_eq!(
        names,
        [
            "suicide_costs",
            "bounce_points",
            "bounced_costs",
            "monster_double",
            "targets",
            "velocity",
            "alignment",
            "miss_info",
            "medals",
            "second_bounce",
            "approach_guide",
            "charge_segments",
            "pogo",
            "pogo_style"
        ]
    );
    assert_eq!(ir.option_offsets(), [0, 1, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    let scoring = 1 | (1 << 1) | (1 << 4) | (1 << 5);
    let hud = (1 << 6) | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 12) | (1 << 13) | (1 << 14);
    let pogo = 1 << 15;
    assert_eq!(ir.default_options(), scoring | hud | pogo);
    assert_eq!(ir.option_values(4), ["common", "likely", "all"]);
    assert_eq!(ir.option_value(ir.default_options(), "targets"), Some(1));
    assert_eq!(ir.option_value(scoring, "targets"), Some(0));
    assert_eq!(ir.option_value(scoring | (2 << 6), "targets"), Some(2));
    assert_eq!(ir.option_value(ir.default_options(), "medals"), Some(1));
    assert_eq!(ir.option_value(scoring, "medals"), Some(0));
    // The stick is on and UNSC by default; a variant turns it off, or
    // picks a style by the enum's index.
    assert_eq!(ir.option_values(13), ["unsc", "covenant", "forerunner"]);
    assert_eq!(ir.option_value(ir.default_options(), "pogo"), Some(1));
    assert_eq!(ir.option_value(ir.default_options(), "pogo_style"), Some(0));
    assert_eq!(ir.option_value(scoring | hud, "pogo"), Some(0));
    for style in 0..3u32 {
        let bits = scoring | hud | pogo | (style << 16);
        assert_eq!(ir.option_value(bits, "pogo_style"), Some(style as i32));
        assert_eq!(ir.option_value(bits, "pogo"), Some(1));
        assert_eq!(ir.option_value(bits, "targets"), Some(1));
    }

    // The defaults: a kill scores nothing, a single scores 1, a double 2
    // (3 for the chain of two), and a death with no killer or by the
    // player's own hand costs a point.
    let mut g = FakeGame::with_players(2);
    let mut vm = Vm::new(ir.clone(), ir.default_options(), 1).unwrap();
    vm.begin_tick(1);
    assert_eq!(vm.fire(&mut g, Input::Kill { victim: 0, killer: Some(1) }), Fired::Ran);
    assert!(scores(&g).is_empty(), "a kill scores 0");
    assert_eq!(vm.fire(&mut g, bounce(1, 77, 1, false)), Fired::Ran);
    assert_eq!(vm.fire(&mut g, bounce(1, 78, 2, false)), Fired::Ran);
    assert_eq!(vm.player_var(1, "bounces"), Some(2));
    assert_eq!(scores(&g), vec![(1, 1), (1, 2)]);
    assert_eq!(scores(&g).iter().map(|s| s.1).sum::<i32>(), 3);
    vm.fire(&mut g, Input::Kill { victim: 1, killer: None });
    vm.fire(&mut g, Input::Kill { victim: 1, killer: Some(1) });
    vm.fire(&mut g, Input::Kill { victim: 1, killer: Some(0) });
    assert_eq!(scores(&g)[2..], [(1, -1), (1, -1)]);

    // A chain of eight scores 1 to 6 and then 6 a bounce: the multiplier
    // stops at Bouncetacular's 6.
    let mut g = FakeGame::with_players(2);
    let mut vm = Vm::new(ir.clone(), ir.default_options(), 1).unwrap();
    vm.begin_tick(1);
    for chain in 1..=8 {
        vm.fire(&mut g, bounce(0, 77, chain, false));
    }
    let points: Vec<i32> = scores(&g).iter().map(|s| s.1).collect();
    assert_eq!(points, [1, 2, 3, 4, 5, 6, 6, 6]);

    // A Bounced is a death with no killer: with the suicide penalty on,
    // `on kill` takes its one point and `on bounced` none. A Monster
    // doubles after the cap.
    let mut g = FakeGame::with_players(2);
    let mut vm = Vm::new(ir.clone(), ir.default_options(), 1).unwrap();
    vm.begin_tick(1);
    assert_eq!(vm.fire(&mut g, Input::Bounced(0)), Fired::Ran);
    vm.fire(&mut g, Input::Kill { victim: 0, killer: None });
    vm.fire(&mut g, bounce(1, 77, 1, true));
    vm.fire(&mut g, bounce(1, 77, 9, true));
    assert_eq!(scores(&g), vec![(0, -1), (1, 2), (1, 12)]);

    // With the suicide penalty off, the Bounced penalty takes the point:
    // bounce points raw 0, the Bounced penalty on (bit 4).
    let mut g = FakeGame::with_players(2);
    let mut vm = Vm::new(ir.clone(), 1 << 4, 1).unwrap();
    vm.begin_tick(1);
    vm.fire(&mut g, Input::Bounced(0));
    vm.fire(&mut g, Input::Kill { victim: 0, killer: None });
    assert_eq!(scores(&g), vec![(0, -1)]);
    assert_eq!(vm.player_var(0, "bounces"), Some(0));

    // Bounce points 2, every penalty and the double off: the suicide option
    // off, bounce points raw 2 (bits 1 to 3). Kills and falls score
    // nothing.
    let mut g = FakeGame::with_players(2);
    let mut vm = Vm::new(ir.clone(), 2 << 1, 1).unwrap();
    vm.begin_tick(1);
    vm.fire(&mut g, bounce(0, 77, 1, false));
    vm.fire(&mut g, bounce(0, 77, 3, true));
    vm.fire(&mut g, Input::Bounced(0));
    vm.fire(&mut g, Input::Kill { victim: 0, killer: None });
    vm.fire(&mut g, Input::Kill { victim: 1, killer: Some(0) });
    assert_eq!(scores(&g), vec![(0, 2), (0, 6)]);
    assert_eq!(vm.player_var(0, "bounces"), Some(2));

    // Bounce points 0 scores no bounce.
    let mut g = FakeGame::with_players(2);
    let mut vm = Vm::new(ir, 0, 1).unwrap();
    vm.begin_tick(1);
    vm.fire(&mut g, bounce(0, 77, 2, true));
    assert!(scores(&g).is_empty());
    assert_eq!(vm.player_var(0, "bounces"), Some(1));
}

#[test]
fn a_round_starts_with_the_declared_types_line_even_with_no_handler() {
    use rules::fake::RoundLine;
    use rules::types::{StartLine, TABLE};
    for row in TABLE {
        let src = format!("rules \"Test variant\" 1\ntype {}\nuses slayer\n", row.name);
        let ir = compile(&src).unwrap();
        let mut vm = Vm::new(ir, 0, 1).unwrap();
        let mut game = FakeGame::with_players(2);
        vm.fire(&mut game, Input::Init);
        assert!(game.start_lines.is_empty());
        for _ in 0..2 {
            assert_eq!(vm.fire(&mut game, Input::RoundStart), Fired::NoHandler);
        }
        match row.start {
            StartLine::None => assert!(game.start_lines.is_empty(), "{}", row.name),
            StartLine::Retail(kind) => {
                assert_eq!(game.start_lines, [RoundLine::Event { kind, index: 0 }; 2], "{}", row.name);
            }
            StartLine::Ours => assert_eq!(game.start_lines, [RoundLine::Clip(row.ty); 2], "{}", row.name),
        }
    }
}

/// The scoreboard's label is the header's `points`: the type's own with no
/// `points` line, the script's with one, told at every round start.
#[test]
fn a_round_start_tells_the_scoreboard_what_a_point_is() {
    use rules::types::TABLE;
    for row in TABLE {
        let src = format!("rules \"Test variant\" 1\ntype {}\nuses slayer\n", row.name);
        let mut vm = Vm::new(compile(&src).unwrap(), 0, 1).unwrap();
        let mut game = FakeGame::with_players(2);
        assert_eq!(game.scoreboard_label, None);
        vm.fire(&mut game, Input::Init);
        assert_eq!(game.scoreboard_label, None, "{}: init is no round", row.name);
        vm.fire(&mut game, Input::RoundStart);
        assert_eq!(game.scoreboard_label.as_deref(), Some(row.points), "{}", row.name);
    }
    let src = "rules \"Test variant\" 1\ntype ctf\nuses ctf\npoints \"Flags\"\n";
    let mut vm = Vm::new(compile(src).unwrap(), 0, 1).unwrap();
    let mut game = FakeGame::with_players(2);
    vm.fire(&mut game, Input::RoundStart);
    assert_eq!(game.scoreboard_label.as_deref(), Some("Flags"));
}
