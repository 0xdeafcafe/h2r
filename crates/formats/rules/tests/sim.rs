//! Generated test matches (`rules::sim`): a seed gives one match, the
//! setup is respected, a match holds only what its script can notice,
//! every built-in reaches an end, a match plays again as a scenario, and a
//! batch sums up. Also the fake game's world the generator plays in:
//! zones, objectives and the record's limits (`rules::workbench`).

use std::collections::BTreeMap;

use rules::builtin;
use rules::ir::Event;
use rules::sim::{self, End, Setup, Winner};
use rules::vm::Input;
use rules::scenario::ScenarioLabel;
use rules::workbench::{self, Limit, Scenario, Seat, Step};

const ALL: [(&str, &str); 12] = [
    ("slayer", builtin::SLAYER),
    ("oddball", builtin::ODDBALL),
    ("koth", builtin::KING),
    ("juggernaut", builtin::JUGGERNAUT),
    ("territories", builtin::TERRITORIES),
    ("ctf", builtin::CTF),
    ("infection", builtin::INFECTION),
    ("gun_game", builtin::GUN_GAME),
    ("vip", builtin::VIP),
    ("siege", builtin::SIEGE),
    ("race", builtin::RACE),
    ("bounce_practice", builtin::BOUNCE_PRACTICE),
];

fn setup(players: u8, teams: u8, seed: u64) -> Setup {
    Setup {
        players,
        teams,
        seed,
        ..Setup::default()
    }
}

#[test]
fn the_same_seed_gives_the_same_match() {
    for (name, src) in [ALL[0], ALL[2], ALL[6]] {
        let a = sim::simulate(src, &setup(8, 0, 7)).unwrap();
        let b = sim::simulate(src, &setup(8, 0, 7)).unwrap();
        assert_eq!(a, b, "{name}");
    }
    let a = sim::simulate(builtin::SLAYER, &setup(8, 0, 1)).unwrap();
    let b = sim::simulate(builtin::SLAYER, &setup(8, 0, 2)).unwrap();
    assert_ne!(a.scenario.steps, b.scenario.steps);
    assert_eq!((a.seed, a.scenario.seed, b.seed), (1, 1, 2));
}

#[test]
fn a_match_seats_the_players_and_teams_asked_for() {
    let m = sim::simulate(builtin::SLAYER, &setup(6, 2, 3)).unwrap();
    let teams: Vec<u8> = m.scenario.players.iter().map(|p| p.team).collect();
    assert_eq!(teams, [0, 1, 0, 1, 0, 1]);
    assert_eq!(m.scenario.teams, [0, 1]);
    assert_eq!(m.scenario.variant["team_play"], 1);
    assert_eq!(m.teams, 2);
    assert!(matches!(m.winner, Winner::Team(_) | Winner::Draw));

    let m = sim::simulate(builtin::SLAYER, &setup(5, 0, 3)).unwrap();
    assert_eq!(m.scenario.players.len(), 5);
    assert!(m.scenario.players.iter().all(|p| p.team == 0));
    assert_eq!(m.scenario.variant["team_play"], 0);
    assert_eq!(m.teams, 0);

    let m = sim::simulate(builtin::SLAYER, &setup(16, 4, 3)).unwrap();
    assert_eq!(m.scenario.teams, [0, 1, 2, 3]);
    assert_eq!(m.scenario.players[7].team, 3);

    // A script that declares teams, and a carrier that is nothing without
    // them, play with teams whatever was asked.
    for src in [builtin::VIP, builtin::CTF, builtin::INFECTION] {
        let m = sim::simulate(src, &setup(8, 0, 3)).unwrap();
        assert_eq!(m.teams, 2);
        assert_eq!(m.scenario.variant["team_play"], 1);
    }
    // Every slot named is a seated one.
    let m = sim::simulate(builtin::SLAYER, &setup(2, 0, 9)).unwrap();
    for s in &m.scenario.steps {
        match s.input {
            Input::Kill { victim, killer } => assert!(victim < 2 && killer.is_none_or(|k| k < 2)),
            Input::Spawn(p) => assert!(p < 2),
            _ => {}
        }
    }
}

#[test]
fn a_match_holds_only_events_the_script_can_notice() {
    for (name, src) in ALL {
        let ir = rules::compile(src).unwrap();
        let allowed = sim::emits(&ir);
        let m = sim::simulate(src, &setup(8, 0, 11)).unwrap();
        for s in &m.scenario.steps {
            assert!(
                allowed.contains(&s.input.event()),
                "{name}: {} at tick {}",
                s.input.event().name(),
                s.tick
            );
        }
    }
    let events = |src: &str| sim::emits(&rules::compile(src).unwrap());
    assert_eq!(events(builtin::SLAYER), [Event::Kill, Event::Spawn]);
    assert_eq!(events(builtin::BOUNCE_PRACTICE), [Event::Bounce]);
    assert_eq!(events(builtin::RACE), [Event::Enter, Event::Leave]);
    assert!(events(builtin::KING).contains(&Event::Enter));
    assert!(events(builtin::ODDBALL).contains(&Event::Pickup));
    assert!(!events(builtin::SLAYER).contains(&Event::Bounce));

    let m = sim::simulate(builtin::BOUNCE_PRACTICE, &setup(4, 0, 1)).unwrap();
    assert!(m.scenario.steps.iter().any(|s| matches!(s.input, Input::Bounce(..))));
    let m = sim::simulate(builtin::SIEGE, &setup(8, 0, 1)).unwrap();
    assert!(m.scenario.steps.iter().any(|s| matches!(s.input, Input::Death(_))));
}

#[test]
fn every_builtin_reaches_an_end() {
    for (name, src) in ALL {
        let m = sim::simulate(src, &setup(8, 0, 1)).unwrap();
        assert!(
            matches!(m.end, End::Script | End::ScoreLimit | End::TimeLimit),
            "{name}: {:?} ({:?}) at tick {} after {} ops",
            m.end,
            m.why,
            m.trace.last_tick,
            m.trace.ops
        );
        assert_eq!(m.why, None, "{name}");
        assert_eq!(m.scenario.ticks, m.trace.last_tick, "{name}");
        assert!(m.trace.worst_tick_ops <= rules::vm::TICK_BUDGET, "{name}");
    }
    let end = |src: &str, players: u8, teams: u8| sim::simulate(src, &setup(players, teams, 1)).unwrap();
    // Sixteen players in two teams reach 50 kills well inside five minutes.
    let m = end(builtin::SLAYER, 16, 2);
    assert_eq!(m.end, End::ScoreLimit);
    assert!(m.top_score >= 50);
    // These end themselves: the top rung, the last human, the phase clock.
    assert_eq!(end(builtin::GUN_GAME, 8, 0).end, End::Script);
    let m = end(builtin::INFECTION, 8, 0);
    assert_eq!(m.end, End::Script);
    assert!(matches!(m.winner, Winner::Team(_)));
    let m = end(builtin::SIEGE, 8, 0);
    assert_eq!((m.end, m.winner), (End::Script, Winner::Team(1)));
    // Somebody scores in the modes that score.
    for src in [builtin::ODDBALL, builtin::KING, builtin::TERRITORIES, builtin::CTF, builtin::RACE] {
        let ir = rules::compile(src).unwrap();
        assert!(end(src, 8, 0).top_score > 0, "{}", ir.header.name);
    }
}

#[test]
fn a_generated_match_plays_again_as_a_scenario() {
    for (name, src) in ALL {
        let m = sim::simulate(src, &setup(6, 0, 5)).unwrap();
        let t = workbench::run(src, &m.scenario).unwrap();
        assert_eq!(t.scores, m.trace.scores, "{name}");
        assert_eq!(t.last_tick, m.trace.last_tick, "{name}");
        assert_eq!(t.round_ended, m.trace.round_ended, "{name}");
        assert_eq!(t.limit, m.trace.limit, "{name}");
        assert_eq!(t.ops, m.trace.ops, "{name}");
        assert_eq!(t.error, m.trace.error, "{name}");
        assert_eq!(t.handlers_ran, m.trace.handlers_ran, "{name}");
    }
}

#[test]
fn a_length_and_a_score_limit_are_the_records() {
    let s = Setup {
        length: Some(20),
        score_limit: Some(1000),
        ..setup(4, 0, 1)
    };
    let m = sim::simulate(builtin::SLAYER, &s).unwrap();
    assert_eq!(m.end, End::TimeLimit);
    assert_eq!(m.trace.last_tick, 600);
    assert_eq!(m.scenario.round_time, 600);
    let s = Setup {
        score_limit: Some(2),
        ..setup(8, 0, 1)
    };
    let m = sim::simulate(builtin::SLAYER, &s).unwrap();
    assert_eq!((m.end, m.top_score), (End::ScoreLimit, 2));
}

#[test]
fn a_batch_summarises_its_matches() {
    let s = setup(8, 0, 40);
    let b = sim::batch(builtin::SLAYER, &s, 10).unwrap();
    let sum = &b.summary;
    assert_eq!((sum.asked, sum.ran, b.rows.len()), (10, 10, 10));
    assert_eq!(sum.stopped, None);
    let seeds: Vec<u64> = b.rows.iter().map(|r| r.seed).collect();
    assert_eq!(seeds, (40..50).collect::<Vec<u64>>());
    assert_eq!(sum.wins.iter().map(|w| w.matches).sum::<u32>(), 10);
    assert_eq!(sum.ends.iter().map(|e| e.matches).sum::<u32>(), 10);
    assert!(sum.wins.windows(2).all(|w| w[0].matches >= w[1].matches));
    assert_eq!(sum.budget, rules::vm::TICK_BUDGET);
    assert!(sum.worst_tick_ops > 0 && sum.worst_tick_ops <= sum.budget);
    assert!(b.rows.iter().any(|r| r.seed == sum.worst_tick_seed));
    assert_eq!(sum.max_score, b.rows.iter().map(|r| r.top_score).max().unwrap());
    assert!(sum.avg_score > 0.0 && sum.avg_score <= f64::from(sum.max_score));
    assert!(sum.avg_ticks <= sum.max_ticks);
    assert!(sum.errors.is_empty());
    assert!(!sum.flags.iter().any(|f| f.kind == "never_ran" || f.kind == "never_ends"));

    // A row is the match its seed gives on its own.
    let one = sim::simulate(builtin::SLAYER, &Setup { seed: 43, ..s.clone() }).unwrap();
    let row = &b.rows[3];
    assert_eq!(
        (row.end, row.winner, row.ticks, row.top_score, row.ops),
        (one.end, one.winner, one.trace.last_tick, one.top_score, one.trace.ops)
    );
    // The same batch again.
    assert_eq!(sim::batch(builtin::SLAYER, &s, 10).unwrap(), b);
}

#[test]
fn a_batch_flags_what_looks_wrong() {
    // Nothing scores, the only way out is a timer nothing starts.
    let idle = "rules \"Idle\" 1\ncarrier slayer\nglobal var t: timer\non timer(t) { end_round(none) }\n";
    let s = Setup {
        length: Some(5),
        ..setup(4, 0, 1)
    };
    let b = sim::batch(idle, &s, 3).unwrap();
    let kinds: Vec<&str> = b.summary.flags.iter().map(|f| f.kind).collect();
    assert_eq!(kinds, ["nobody_scores", "never_ran", "clock"]);
    assert_eq!(b.summary.flags[0].text, "nobody scored in every match");
    assert_eq!((b.summary.flags[0].seed, b.summary.flags[0].matches), (1, 3));
    assert_eq!(b.summary.flags[1].text, "on timer(t) never ran");
    assert_eq!(b.summary.ends.len(), 1);
    assert_eq!((b.summary.ends[0].end, b.summary.ends[0].matches), (End::TimeLimit, 3));
    assert!(b.rows.iter().all(|r| r.winner == Winner::Draw));

    // Every death costs a point.
    let cruel = "rules \"Cruel\" 1\ncarrier slayer\non kill(victim, killer) { score(victim, -1) }\n";
    let b = sim::batch(cruel, &setup(8, 0, 1), 2).unwrap();
    let f = b.summary.flags.iter().find(|f| f.kind == "negative_score").unwrap();
    assert_eq!((f.seed, f.matches), (1, 2));
}

#[test]
fn the_scenario_file_holds_the_match() {
    let m = sim::simulate(builtin::SLAYER, &setup(4, 2, 2)).unwrap();
    let (file, left) = sim::to_file(&m.scenario);
    assert_eq!(file.seed, 2);
    assert_eq!(file.players.len(), 4);
    assert_eq!(file.players[1].team, 1);
    assert_eq!(file.teams, Some(vec![0, 1]));
    assert_eq!(file.ticks.len() as u32, m.scenario.ticks + 1);
    let events: usize = file.ticks.iter().map(|t| t.events.len()).sum();
    assert_eq!(events, m.scenario.steps.len());
    assert!(left.is_empty(), "{left:?}");
    // The record's fields, the units and the round clock come along.
    assert_eq!(file.variant, m.scenario.variant);
    assert_eq!(file.players[1].unit, Some(workbench::unit_of(1)));
    assert_eq!(file.round_time_left, m.scenario.round_time);
    let text = serde_json::to_string(&file).unwrap();
    let back: rules::scenario::Scenario = serde_json::from_str(&text).unwrap();
    assert_eq!(back, file);
    // The file's own runner plays it to the same scores: Slayer makes no
    // objective, so the two runners differ in nothing but where they stop.
    let ran = rules::scenario::run(rules::build(builtin::SLAYER).unwrap().0, &file).unwrap();
    let scores: Vec<(u8, i32)> = ran.game.players.iter().map(|(&s, p)| (s, p.score)).collect();
    assert_eq!(scores, m.trace.scores);

    // Labelled objects with their teams, and the events past the engine's.
    let m = sim::simulate(builtin::CTF, &setup(4, 2, 2)).unwrap();
    let (file, left) = sim::to_file(&m.scenario);
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(file.labelled, m.scenario.labelled);
    assert!(file.labelled.iter().any(|l| l.team == Some(1)));
    let m = sim::simulate(builtin::BOUNCE_PRACTICE, &setup(4, 0, 2)).unwrap();
    let (file, left) = sim::to_file(&m.scenario);
    assert!(left.is_empty(), "{left:?}");
    let bounces = file
        .ticks
        .iter()
        .flat_map(|t| &t.events)
        .filter(|e| matches!(e, rules::scenario::ScenarioEvent::Bounce { .. }))
        .count();
    assert!(bounces > 0);
}

// ---------- the fake game's world ----------

fn seats(n: u8) -> Vec<Seat> {
    (0..n)
        .map(|slot| Seat { slot, team: slot % 2, lives: 0, unit: Some(workbench::unit_of(slot)) })
        .collect()
}

#[test]
fn a_seated_score_limit_ends_the_run() {
    let kills = |tick| Step {
        tick,
        input: Input::Kill { victim: 1, killer: Some(0) },
    };
    let s = Scenario {
        players: seats(2),
        variant: BTreeMap::from([("score_limit".to_string(), 2)]),
        ticks: 100,
        start: true,
        steps: vec![kills(3), kills(9), kills(20)],
        ..Scenario::default()
    };
    let t = workbench::run(builtin::SLAYER, &s).unwrap();
    assert_eq!((t.limit, t.last_tick), (Some(Limit::Score), 9));
    assert_eq!(t.scores, [(0, 2), (1, 0)]);
    assert_eq!(t.handlers_ran, ["kill"]);
    assert!(t.worst_tick_ops > 0);

    let s = Scenario {
        players: seats(2),
        variant: BTreeMap::from([("time_limit".to_string(), 2)]),
        ticks: 100,
        ..Scenario::default()
    };
    let t = workbench::run(builtin::SLAYER, &s).unwrap();
    assert_eq!((t.limit, t.last_tick), (Some(Limit::Time), 60));
    // With no limit seated a run goes to its last tick.
    let s = Scenario { ticks: 100, ..Scenario::default() };
    let t = workbench::run(builtin::SLAYER, &s).unwrap();
    assert_eq!((t.limit, t.last_tick), (None, 100));
}

#[test]
fn a_player_entering_a_labelled_object_is_inside_it() {
    let src = "rules \"Pad\" 1\ncarrier slayer\non tick {\n    for each objects labelled \"pad\" z {\n        for each players p where inside(p, z) { score(p, 1) }\n    }\n}\n";
    let zone = |tick, enter: bool, slot: u8| Step {
        tick,
        input: if enter {
            Input::Enter { zone: 7, object: workbench::unit_of(slot) }
        } else {
            Input::Leave { zone: 7, object: workbench::unit_of(slot) }
        },
    };
    let s = Scenario {
        players: seats(2),
        labelled: vec![ScenarioLabel { label: "pad".into(), object: 7, number: 0, placement: None, team: None }],
        ticks: 10,
        start: true,
        // Player 0 stands on it for ticks 1 to 3; player 1 dies on it.
        steps: vec![
            zone(1, true, 0),
            zone(4, false, 0),
            zone(5, true, 1),
            Step { tick: 7, input: Input::Kill { victim: 1, killer: None } },
        ],
        ..Scenario::default()
    };
    let t = workbench::run(src, &s).unwrap();
    assert_eq!(t.scores, [(0, 3), (1, 2)]);
}

#[test]
fn a_spawned_objective_is_carried_by_who_picks_it_up() {
    let ball = workbench::SPAWNED_BASE;
    let s = Scenario {
        players: seats(2),
        labelled: vec![ScenarioLabel { label: "oddball_spawn".into(), object: 40, number: 0, placement: None, team: None }],
        variant: BTreeMap::from([("oddball_ball_count".to_string(), 1)]),
        ticks: 100,
        start: true,
        // The ball is made at tick 1, held from tick 2 and dropped at 40.
        steps: vec![
            Step { tick: 2, input: Input::Pickup(1, ball) },
            Step { tick: 40, input: Input::Drop(1, ball) },
        ],
        ..Scenario::default()
    };
    let t = workbench::run(builtin::ODDBALL, &s).unwrap();
    assert_eq!(t.scores, [(0, 0), (1, 1)]);
}
