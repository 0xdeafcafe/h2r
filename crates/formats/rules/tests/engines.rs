//! The engine ports and the payoff examples: every script compiles and
//! verifies, names the entries a host lacks, and does its core rule on the
//! fake game.

use std::collections::BTreeSet;

use rules::builtin::{self, builtin, examples};
use rules::caps;
use rules::fake::FakeGame;
use rules::ir::Op;
use rules::vm::{Arg, Placed};
use rules::{Fired, Input, Ir, Vm, build, compile};

const BALL: &str = "objects/weapons/multiplayer/ball/ball";
const FLAG: &str = "objects/weapons/multiplayer/flag/flag";

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
        .filter(|e| !e.hostable_on(ir.header.carrier))
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
    // Slayer, VIP, Bounce Practice and Superbounce need nothing a host
    // lacks.
    for src in [
        builtin::SLAYER,
        builtin::VIP,
        builtin::BOUNCE_PRACTICE,
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
carrier slayer
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
carrier slayer
on round_start { for each pick(players p, 1) q { score(q, 1) } }
"#;
    assert!(compile(bad).is_err());
}

#[test]
fn objectives_are_visited_by_handle_and_bounded_at_eight() {
    let src = r#"
rules "T" 1
carrier slayer
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

// ---- Oddball ----

#[test]
fn oddball_scores_time_held_and_resets_a_dropped_ball() {
    let mut g = FakeGame::with_players(2);
    g.labelled.insert(
        "oddball_spawn".into(),
        vec![
            Placed { placement: 0, object: 40 },
            Placed { placement: 1, object: 41 },
        ],
    );
    g.variant.insert("oddball_ball_count".into(), 1);
    let mut vm = start(builtin::ODDBALL, &mut g, 3);
    vm.flush_queue(&mut g);
    // One ball, at the first spawn.
    assert_eq!(
        g.calls_to("spawn_objective"),
        vec![&vec![Arg::Tag(Some(BALL.into())), object(40), Arg::Team(None)]]
    );

    g.objectives.insert(100, None);
    vm.fire(&mut g, Input::Pickup(1, 100));
    g.objectives.insert(100, Some(1));
    assert_eq!(g.calls_to("role_traits"), vec![&vec![player(1), Arg::Bool(true)]]);
    ticks(&mut vm, &mut g, 2, 29);
    assert!(scores(&g).is_empty());
    ticks(&mut vm, &mut g, 31, 1);
    assert_eq!(scores(&g), vec![(1, 1)]);

    // Dropped: no more points, and the ball returns after the reset time.
    vm.fire(&mut g, Input::Drop(1, 100));
    g.objectives.insert(100, None);
    ticks(&mut vm, &mut g, 32, 30 * 30 - 1);
    vm.flush_queue(&mut g);
    assert!(g.calls_to("reset_objective").is_empty());
    ticks(&mut vm, &mut g, 32 + 899, 1);
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("reset_objective"), vec![&vec![object(100)]]);
    assert_eq!(scores(&g), vec![(1, 1)]);
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
    let mut vm = start(builtin::KING, &mut g, 5);
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
}

// ---- CTF and Assault ----

#[test]
fn ctf_capture_scores_and_returns_the_flag() {
    let mut g = FakeGame::with_players(2);
    g.players.get_mut(&1).unwrap().team = 1;
    g.variant.insert("engine".into(), 1);
    g.variant.insert("team_play".into(), 1);
    g.labelled.insert(
        "ctf_flag_spawn".into(),
        vec![
            Placed { placement: 0, object: 10 },
            Placed { placement: 1, object: 11 },
        ],
    );
    g.object_teams.extend([(10, 0), (11, 1)]);
    let mut vm = start(builtin::CTF, &mut g, 2);
    vm.flush_queue(&mut g);
    assert_eq!(
        g.calls_to("spawn_objective"),
        vec![
            &vec![Arg::Tag(Some(FLAG.into())), object(10), Arg::Team(Some(0))],
            &vec![Arg::Tag(Some(FLAG.into())), object(11), Arg::Team(Some(1))],
        ]
    );

    // Team 1's flag is object 21; player 0 takes it.
    g.objectives.insert(21, None);
    g.object_teams.insert(21, 1);
    vm.fire(&mut g, Input::Pickup(0, 21));
    g.objectives.insert(21, Some(0));
    assert_eq!(g.calls_to("role_traits"), vec![&vec![player(0), Arg::Bool(true)]]);
    ticks(&mut vm, &mut g, 2, 5);
    assert!(scores(&g).is_empty());

    // At its own flag's spawn: a capture, and the flag goes home.
    g.inside.insert((0, 10));
    ticks(&mut vm, &mut g, 7, 1);
    vm.flush_queue(&mut g);
    assert_eq!(scores(&g), vec![(0, 1)]);
    assert_eq!(g.calls_to("reset_objective"), vec![&vec![object(21)]]);
    assert_eq!(g.calls_to("role_traits").last(), Some(&&vec![player(0), Arg::Bool(false)]));
    g.inside.clear();

    // Dropped, it returns after the reset time.
    g.variant.insert("ctf_reset_time".into(), 2);
    vm.fire(&mut g, Input::Pickup(0, 21));
    vm.fire(&mut g, Input::Drop(0, 21));
    g.objectives.insert(21, None);
    ticks(&mut vm, &mut g, 8, 59);
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("reset_objective").len(), 1);
    ticks(&mut vm, &mut g, 67, 1);
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("reset_objective").len(), 2);

    // With touch return, its own team's touch returns it at once.
    g.variant.insert("ctf_touch_return".into(), 1);
    vm.fire(&mut g, Input::Pickup(0, 21));
    vm.fire(&mut g, Input::Drop(0, 21));
    vm.fire(&mut g, Input::Pickup(1, 21));
    vm.flush_queue(&mut g);
    assert_eq!(g.calls_to("reset_objective").len(), 3);
    assert_eq!(scores(&g), vec![(0, 1)]);
}

#[test]
fn assault_arms_a_bomb_on_the_enemy_plate() {
    let mut g = FakeGame::with_players(2);
    g.players.get_mut(&1).unwrap().team = 1;
    g.variant.insert("engine".into(), 9);
    g.variant.insert("assault_arm_time".into(), 1);
    g.labelled.insert("assault_bomb_spawn".into(), vec![Placed { placement: 0, object: 10 }]);
    g.labelled.insert("assault_bomb_return".into(), vec![Placed { placement: 1, object: 12 }]);
    g.object_teams.extend([(10, 0), (12, 1)]);
    let mut vm = start(builtin::ASSAULT, &mut g, 2);
    // Player 0 carries team 0's bomb onto team 1's plate.
    g.objectives.insert(30, Some(0));
    g.object_teams.insert(30, 0);
    vm.fire(&mut g, Input::Pickup(0, 30));
    g.inside.insert((0, 12));
    ticks(&mut vm, &mut g, 2, 29);
    assert!(scores(&g).is_empty());
    ticks(&mut vm, &mut g, 31, 1);
    vm.flush_queue(&mut g);
    assert_eq!(scores(&g), vec![(0, 1)]);
    assert_eq!(g.calls_to("reset_objective"), vec![&vec![object(30)]]);
}

// ---- Territories ----

#[test]
fn territories_held_score_each_interval() {
    let mut g = FakeGame::with_players(2);
    g.players.get_mut(&1).unwrap().team = 1;
    g.variant.insert("territories_count".into(), 2);
    g.labelled.insert(
        "territory".into(),
        vec![
            Placed { placement: 0, object: 600 },
            Placed { placement: 1, object: 601 },
            Placed { placement: 2, object: 602 },
        ],
    );
    let mut vm = start(builtin::TERRITORIES, &mut g, 4);
    assert_eq!(
        g.calls_to("register_objective"),
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
    assert_eq!(ir.header.carrier, 2);
    assert_eq!(ir.header.list, rules::ir::List::Tweakers);
    assert_eq!(ir.header.requires, ["hud.superbounce"]);
    // The options in the order the record packs them: the suicide penalty
    // (bit 0), bounce points raw 1 (bits 1 to 3), the Bounced penalty
    // (bit 4), the Monster double (bit 5).
    let names: Vec<&str> = (0..ir.options.len()).map(|i| ir.option_name(i)).collect();
    assert_eq!(names, ["suicide_costs", "bounce_points", "bounced_costs", "monster_double"]);
    assert_eq!(ir.option_offsets(), [0, 1, 4, 5]);
    assert_eq!(ir.default_options(), 1 | (1 << 1) | (1 << 4) | (1 << 5));

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
