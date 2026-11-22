use std::path::Path;

use super::report::{fit, pooled};
use super::*;
use crate::config::{BotParams, parse_match, resolve};

fn no_files(_: &Path) -> Result<BotParams, String> {
    Ok(BotParams::default())
}

const MATCH: &str = r#"
map = "ascension"
seed = 3

[drops]
heights = [1.0, 3.5]
along = 2
across = { from = -0.04, to = 0.04, step = 0.02 }

[[drops.targets]]
id = "slot"
shape = "slot"
edge = [[-11.10, 4.0, -3.2], [-9.98, 4.0, -3.2]]
drift = [0.0, 1.0]
stand = [-10.54, 4.0, -3.19]

[[drops.targets]]
point = [3.9, -26.7, 2.1]
"#;

fn table(text: &str) -> DropsConfig {
    parse_match(text, false).unwrap().drops.unwrap()
}

fn edge_spec() -> TargetSpec {
    TargetSpec {
        id: Some("e".into()),
        shape: Some("crease".into()),
        edge: Some([[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]]),
        drift: Some([0.0, -1.0]),
        ..TargetSpec::default()
    }
}

fn bare(targets: Vec<TargetSpec>) -> DropsConfig {
    DropsConfig {
        no_control: true,
        heights: vec![1.0],
        across: Some(Across::List(vec![0.0])),
        along: Some(1),
        targets: Targets::List(targets),
        ..DropsConfig::default()
    }
}

// ---- the table ----

#[test]
fn a_drops_table_reads_with_its_defaults_filled_in() {
    let d = table(MATCH);
    let s = d.settings("ascension").unwrap();
    assert_eq!(s.heights, vec![1.0, 3.5]);
    assert_eq!(s.across, vec![-0.04, -0.02, 0.0, 0.02, 0.04]);
    assert_eq!(s.along, 2);
    assert_eq!(s.seats, SEATS);
    assert_eq!(s.sides, vec![Side::None]);
    assert_eq!(s.bounce_vz, BOUNCE_VZ);
    assert_eq!(s.repeats, 1);
    // Ascension's control without a [drops.control].
    assert_eq!(s.control, Some(ControlSpec::ascension()));
    // The defaults: the task's heights and across range.
    let d = DropsConfig::default();
    let s = d.settings("ascension").unwrap();
    assert_eq!(s.heights, HEIGHTS.to_vec());
    assert_eq!(s.across.len(), 13);
    assert_eq!(s.across[0], -0.12);
    assert_eq!(s.across[12], 0.12);
    assert!(s.across.contains(&0.0));
    assert!(d.targets.is_scan());
}

#[test]
fn the_tables_problems_are_all_listed() {
    let mut d = DropsConfig {
        heights: vec![0.1, 30.0],
        across: Some(Across::Range {
            from: 0.1,
            to: -0.1,
            step: 0.02,
        }),
        along: Some(0),
        repeats: Some(0),
        seats: Some(16),
        shard: 2,
        shards: 2,
        targets: Targets::Word("everything".into()),
        ..DropsConfig::default()
    };
    let p = d.problems("lockout", true, true, false).join("\n");
    for want in [
        "heights: 0.1 is outside",
        "across: to is under from",
        "along is 1 to 64",
        "repeats is 1 to 20",
        "seats is 1 to 15",
        "shard 2 is not under shards 2",
        "targets = \"everything\"",
        "no known control drop on lockout",
        "leave superbounce out",
        "a [drill] or a [train]",
    ] {
        assert!(p.contains(want), "{want:?} not in:\n{p}");
    }
    // A control on another map, or none, is fine.
    d = bare(vec![edge_spec()]);
    assert!(d.problems("lockout", false, false, false).is_empty());
    d.no_control = true;
    d.control = Some(ControlSpec::ascension());
    assert!(d.problems("lockout", false, false, false)[0].contains("use one"));
    // A target with neither or both.
    let both = TargetSpec {
        point: Some([0.0; 3]),
        ..edge_spec()
    };
    let p = bare(vec![TargetSpec::default(), both]).problems("x", false, false, false);
    assert!(p[0].contains("target 1: an edge or a point"), "{p:?}");
    assert!(p[1].contains("target 2: an edge or a point, not both"), "{p:?}");
    // The map twice, different.
    let d = DropsConfig {
        map: Some("lockout".into()),
        ..bare(vec![edge_spec()])
    };
    assert!(d.problems("ascension", false, false, false)[0].contains("but the match's map"));
}

#[test]
fn a_drops_match_resolves_its_seats_map_and_limits() {
    let text = r#"
[drops]
map = "ascension"
seats = 4
"#;
    let m = resolve(parse_match(text, false).unwrap(), &no_files).unwrap();
    assert_eq!(m.config.map, "ascension");
    assert_eq!(m.bots.len(), 4);
    assert_eq!(m.bots[3].name, "Dropper 4");
    // No time limit, a training report.
    assert!(m.variant.contains(&(crate::config::TIME_LIMIT_SETTING, 0)));
    assert_eq!(m.config.detail(), crate::config::Detail::Training);
    // Teams, so the free-for-all tick-0 end doesn't hit; seats alternate.
    assert!(m.team_play);
    assert_eq!((m.bots[0].team, m.bots[1].team), (Some(0), Some(1)));
    // The table's problems reach the match's.
    let bad = "map = \"lockout\"\n[drops]\nheights = [99.0]\n";
    let e = resolve(parse_match(bad, false).unwrap(), &no_files).unwrap_err();
    assert!(e.contains("heights: 99 is outside"), "{e}");
    assert!(e.contains("no known control drop on lockout"), "{e}");
    // A drops table round-trips through the TOML a batch writes.
    let m = resolve(parse_match(MATCH, false).unwrap(), &no_files).unwrap();
    let back = parse_match(&toml::to_string(&m.config).unwrap(), false).unwrap();
    assert_eq!(back.drops, m.config.drops);
}

// ---- the plan ----

#[test]
fn the_plan_covers_each_target_and_puts_the_control_first() {
    let d = table(MATCH);
    let s = d.settings("ascension").unwrap();
    let job = Job::new(&d, "ascension", 15).unwrap();
    let t = job.targets();
    assert_eq!(t[0].id, CONTROL_ID);
    assert!(t[0].control);
    assert_eq!(t[1].id, "slot");
    assert_eq!(t[2].id, "t001");
    assert!(t[2].is_point());
    let drops = plan(&s, t);
    // 2 control heights; slot 2 along × 2 heights × 5 across; the point 1 × 2 × 5.
    assert_eq!(drops.len(), 2 + 20 + 10);
    assert_eq!(drops_per_target(&t[1], &s), 20);
    assert_eq!(drops_per_target(&t[2], &s), 10);
    assert!(drops[..2].iter().all(|d| d.control));
    // The control drops at its point exactly, 0.04 across the seam, on
    // the seam's floor.
    let c = drops[0];
    assert_eq!([c.point[0], c.point[1]], [-10.54, 4.04]);
    assert!((c.across - 0.04).abs() < 1e-4);
    assert!((c.point[2] + 3.2).abs() < 1e-4);
    assert!((c.start[2] - (-3.2 + 3.6)).abs() < 1e-4);
    // The ids are the plan's order; the slot's points are across +y.
    for (i, d) in drops.iter().enumerate() {
        assert_eq!(d.id as usize, i);
    }
    let on = drops.iter().find(|d| d.target == 1 && d.across == 0.04).unwrap();
    assert!((on.point[1] - 4.04).abs() < 1e-4);
    // Along points sit in the middle of their halves of the edge.
    let alongs: Vec<f32> = t[1].alongs(2);
    assert_eq!(alongs, vec![0.25, 0.75]);
    // Rest points sit over the floor by the lift and the offset.
    assert!((on.rest[2] - (-3.2 + REST_LIFT + 0.04)).abs() < 1e-4);
}

#[test]
fn the_across_direction_follows_the_drift_and_an_approach_pushes_along_it() {
    let t = Target::from_spec(0, &edge_spec()).unwrap();
    // Edge along +x, drift -y: across is -y.
    assert_eq!(t.normal, [0.0, -1.0]);
    assert!((t.across_of([1.0, -0.1]) - 0.1).abs() < 1e-6);
    assert!((t.miss([3.0, 0.0]) - 1.0).abs() < 1e-6);
    let mut d = bare(vec![edge_spec()]);
    d.approach = Some(Approach {
        stick: 0.5,
        lead: Some(0.2),
        from: vec![],
    });
    let s = d.settings("x").unwrap();
    assert_eq!(s.sides, vec![Side::Plus, Side::Minus]);
    let drops = plan(&s, &[t.clone()]);
    assert_eq!(drops.len(), 2);
    let plus = drops.iter().find(|d| d.side == Side::Plus).unwrap();
    // Starts back against the push: +y by the lead, pushed toward -y.
    assert!((plus.start[1] - 0.2).abs() < 1e-5);
    let run = DropRun::new(*plus, &t, &s, 0);
    assert_eq!(run.walk, [0.0, -0.5]);
}

#[test]
fn targets_split_over_runs_by_their_drops_and_each_lands_once() {
    let ts = vec!["a", "b", "c", "d"];
    let runs = shard_targets(&ts, &[10, 1, 1, 8], 2);
    assert_eq!(runs, vec![vec!["a"], vec!["b", "c", "d"]]);
    // More runs than targets: a run a target.
    assert_eq!(shard_targets(&ts, &[1, 1, 1, 1], 9).len(), 4);
    // No target: one empty run.
    assert_eq!(shard_targets::<&str>(&[], &[], 3), vec![Vec::<&str>::new()]);
    let many: Vec<u32> = (0..50).collect();
    let w: Vec<usize> = (0..50).map(|i| 1 + i % 7).collect();
    let runs = shard_targets(&many, &w, 4);
    let mut all: Vec<u32> = runs.iter().flatten().copied().collect();
    all.sort();
    assert_eq!(all, many);
    let loads: Vec<usize> = runs
        .iter()
        .map(|r| r.iter().map(|&i| w[i as usize]).sum())
        .collect();
    let (lo, hi) = (loads.iter().min().unwrap(), loads.iter().max().unwrap());
    assert!(hi - lo <= 7, "{loads:?}");
}

#[test]
fn the_queue_hands_out_the_first_drop_clear_of_the_others() {
    let t = Target::from_spec(0, &edge_spec()).unwrap();
    let d = |x: f32| Drop {
        id: 0,
        target: 0,
        along: 0.0,
        across: 0.0,
        height: 1.0,
        side: Side::None,
        repeat: 0,
        point: [x, 0.0, 0.0],
        start: [x, 0.0, 1.0],
        rest: [x, 0.0, 0.05],
        control: false,
    };
    let _ = t;
    let drops = vec![d(0.0), d(0.5), d(5.0)];
    let mut q = Queue::new([0, 1, 2]);
    // A seat at 0.2: the first two are too near.
    assert_eq!(q.take(&drops, &[[0.2, 0.0]], CLEAR), Some(2));
    assert_eq!(q.take(&drops, &[[0.2, 0.0]], CLEAR), None);
    assert_eq!(q.len(), 2);
    assert_eq!(q.take(&drops, &[], CLEAR), Some(0));
    q.put_back(0);
    assert_eq!(q.take_any(), Some(1));
    assert_eq!(q.take_any(), Some(0));
    assert!(q.is_empty());
}

// ---- one drop, on a made-up body ----

/// What a body's level pairs do through a drop.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pairs {
    /// One pair, kept: the word written holds.
    Hold,
    /// No pair while high in the air; a new one at the touchdown.
    NoneInAir,
    /// The pair is rebuilt (a new agent, word widened to 1) on the
    /// touchdown tick.
    Rebuild,
    /// A second pair is made on the touchdown tick at 0, widened to 1.
    NewPair,
    /// The same agent comes out of the touchdown tick at 1.
    Reset,
}

/// A body falling under gravity onto a floor at z 0, the HUD's way: a
/// primed touchdown launches at `launch`, an unprimed one stops.
struct Body {
    pos: [f32; 3],
    vel: [f32; 3],
    airborne: bool,
    pairs: Vec<PairSense>,
    teleport: Option<[f32; 3]>,
    answer: Option<Result<(), String>>,
    plan: Pairs,
    launch: f32,
    refuse: bool,
}

impl Body {
    fn new(at: [f32; 3], plan: Pairs, launch: f32) -> Body {
        Body {
            pos: at,
            vel: [0.0; 3],
            airborne: false,
            pairs: vec![PairSense { agent: 1, word: 0 }],
            teleport: None,
            answer: None,
            plan,
            launch,
            refuse: false,
        }
    }

    fn sense(&mut self) -> Sense {
        Sense {
            pos: Some(self.pos),
            vel: Some(self.vel),
            airborne: self.airborne,
            pairs: self.pairs.clone(),
            asleep: false,
            teleported: self.answer.take(),
        }
    }

    fn act(&mut self, a: &Act) {
        if let Some(w) = a.word {
            for p in &mut self.pairs {
                p.word = w;
            }
        }
        if a.stop {
            self.vel = [0.0; 3];
        }
        if let Some((to, _)) = a.teleport {
            self.teleport = Some(to);
        }
    }

    /// One game tick.
    fn tick(&mut self) {
        if let Some(to) = self.teleport.take() {
            if self.refuse {
                self.answer = Some(Err("refused".into()));
                return;
            }
            self.pos = to;
            self.airborne = to[2] > 0.2;
            self.answer = Some(Ok(()));
            return;
        }
        if !self.airborne {
            self.vel = [0.0; 3];
            return;
        }
        self.vel[2] -= GRAVITY / 30.0;
        self.pos[2] += self.vel[2] / 30.0;
        if self.plan == Pairs::NoneInAir && self.pos[2] > 0.3 {
            self.pairs.clear();
        }
        if self.pos[2] > 0.0 {
            return;
        }
        // The touchdown tick: the pass runs with the words as they are.
        self.pos[2] = 0.0;
        match self.plan {
            Pairs::Hold => {}
            Pairs::NoneInAir => self.pairs = vec![PairSense { agent: 9, word: 1 }],
            Pairs::Rebuild => self.pairs = vec![PairSense { agent: 5, word: 1 }],
            Pairs::NewPair => self.pairs.push(PairSense { agent: 6, word: 1 }),
            Pairs::Reset => self.pairs[0].word = 1,
        }
        let primed = self.plan == Pairs::Hold && self.pairs.iter().all(|p| p.word == PRIMED);
        self.vel[2] = if primed { self.launch } else { 0.0 };
        self.airborne = self.vel[2] > 0.0;
    }
}

fn settings() -> Settings {
    bare(vec![edge_spec()]).settings("x").unwrap()
}

fn drop_at(height: f32) -> (Drop, Target) {
    let s = settings();
    let t = Target::from_spec(0, &edge_spec()).unwrap();
    let mut d = plan(&s, &[t.clone()])[0];
    d.height = height;
    d.start[2] = height;
    (d, t)
}

/// Runs one drop on a body to its end: the record and the acts.
fn fly(plan: Pairs, launch: f32, height: f32) -> (Record, Vec<Act>) {
    let (d, t) = drop_at(height);
    let mut run = DropRun::new(d, &t, &settings(), 3);
    let mut body = Body::new([1.0, 0.0, 0.0], plan, launch);
    let mut acts = Vec::new();
    for tick in 100..2000u64 {
        let sense = body.sense();
        let act = run.step(tick, &sense);
        if act.contacts {
            run.set_contacts(vec![ContactPoint {
                normal: [0.0, 0.0, 1.0],
                distance: -0.047,
                ..ContactPoint::default()
            }]);
        }
        body.act(&act);
        acts.push(act);
        if run.done() {
            return (run.record(true), acts);
        }
        body.tick();
    }
    panic!("the drop never ended");
}

#[test]
fn a_held_prime_that_launches_is_a_bounce_with_its_launch_and_firsts() {
    let (r, acts) = fly(Pairs::Hold, 12.0, 2.0);
    assert_eq!(r.outcome, Outcome::Bounced, "{r:#?}");
    assert_eq!(r.prime_held, Some(true));
    assert_eq!(r.seat, 3);
    let l = r.launch.unwrap();
    assert!((l.speed - 12.0).abs() < 0.5, "{l:?}");
    // 12 units/s climbs 17 units; the top is past the watch.
    assert!(l.estimated);
    assert!((l.height - 144.0 / (2.0 * GRAVITY)).abs() < 1.5, "{l:?}");
    let t = r.touchdown.as_ref().unwrap();
    // From 2 up: √(2 × 4.15 × 2) ≈ 4.07 in.
    assert!((t.speed - 4.07).abs() < 0.3, "{t:?}");
    assert!(t.miss < 1e-4);
    assert_eq!(t.contacts.len(), 1);
    assert_eq!(r.first.touchdown, Some(t.t));
    assert_eq!(r.first.launch, Some(t.t + 1));
    assert_eq!(r.first.lost, None);
    assert!((r.start_z.unwrap() - 2.0).abs() < 1e-4);
    // The word: -3 on every fall row, one pair.
    assert!(r.words.iter().all(|w| w.word == Some(-3) && w.pairs == 1), "{:?}", r.words);
    // Settled with 0, armed with -3 and lifted on one row, then -3 each
    // fall row.
    let arm = acts.iter().position(|a| a.word == Some(PRIMED)).unwrap();
    assert!(acts[..arm].iter().all(|a| a.word.is_none() || a.word == Some(0)));
    assert!(acts[arm].teleport.is_some());
    assert!(acts[0].teleport.is_some());
    assert_eq!(r.rows[0].phase, "arm");
    assert!(r.rows.iter().any(|row| row.phase == "touchdown"));
}

#[test]
fn a_held_prime_that_stays_down_is_no_bounce() {
    let (r, _) = fly(Pairs::Hold, 0.4, 1.0);
    assert_eq!(r.outcome, Outcome::NoBounce, "{r:#?}");
    assert_eq!(r.prime_held, Some(true));
    assert_eq!(r.why, None);
}

#[test]
fn a_prime_that_doesnt_reach_the_touchdown_is_prime_lost_never_no_bounce() {
    for (plan, why) in [
        (Pairs::NoneInAir, "no_pair"),
        (Pairs::Rebuild, "rebuilt"),
        (Pairs::NewPair, "new_pair (word 1)"),
        (Pairs::Reset, "reset (word 1)"),
    ] {
        let (r, _) = fly(plan, 12.0, 3.5);
        assert_eq!(r.outcome, Outcome::PrimeLost, "{plan:?}: {r:#?}");
        assert_eq!(r.why.as_deref(), Some(why), "{plan:?}");
        assert_eq!(r.prime_held, Some(false));
        // The landing itself: stopped, the unprimed signature.
        assert!(r.launch.unwrap().speed < 0.5);
    }
    // With no pair in the air, the trace shows it the first row it happens.
    let (r, _) = fly(Pairs::NoneInAir, 12.0, 3.5);
    assert!(r.first.lost.is_some());
    assert_eq!(r.first.lost_why.as_deref(), Some("no_pair"));
    assert!(r.words.iter().any(|w| w.word.is_none() && w.pairs == 0), "{:?}", r.words);
}

#[test]
fn the_prime_check_reads_the_touchdown_ticks_pairs() {
    let p = |agent, word| PairSense { agent, word };
    assert_eq!(prime_check(&[p(1, PRIMED)], &[p(1, PRIMED)]), Ok(()));
    // A new pair that is itself -3 is held too.
    assert_eq!(prime_check(&[p(1, PRIMED)], &[p(1, PRIMED), p(2, PRIMED)]), Ok(()));
    assert_eq!(prime_check(&[], &[p(1, PRIMED)]).unwrap_err(), "no_pair");
    assert_eq!(prime_check(&[p(1, PRIMED)], &[]).unwrap_err(), "rebuilt");
    assert_eq!(prime_check(&[p(1, PRIMED)], &[p(1, 1)]).unwrap_err(), "reset (word 1)");
    assert_eq!(
        prime_check(&[p(1, PRIMED)], &[p(1, PRIMED), p(2, 0)]).unwrap_err(),
        "new_pair (word 0)"
    );
}

#[test]
fn a_drop_that_never_drops_is_setup_failed() {
    // A refused teleport.
    let (d, t) = drop_at(1.0);
    let mut run = DropRun::new(d, &t, &settings(), 0);
    let mut body = Body::new([1.0, 0.0, 0.0], Pairs::Hold, 12.0);
    body.refuse = true;
    for tick in 0..50 {
        let s = body.sense();
        let a = run.step(tick, &s);
        body.act(&a);
        if run.done() {
            break;
        }
        body.tick();
    }
    assert_eq!(
        run.outcome(),
        (Outcome::SetupFailed, Some("teleport: refused".into()))
    );
    // A body that never settles (sliding forever).
    let mut run = DropRun::new(d, &t, &settings(), 0);
    let mut s = Sense {
        pos: Some([0.0; 3]),
        vel: Some([0.5, 0.0, 0.0]),
        ..Sense::default()
    };
    run.step(0, &s);
    s.teleported = Some(Ok(()));
    for tick in 1..1000 {
        run.step(tick, &s);
        s.teleported = None;
        if run.done() {
            break;
        }
    }
    let (o, why) = run.outcome();
    assert_eq!(o, Outcome::SetupFailed);
    assert!(why.unwrap().starts_with("did not settle"));
    // The seat dying in the fall.
    let mut run = DropRun::new(d, &t, &settings(), 0);
    let mut body = Body::new([1.0, 0.0, 0.0], Pairs::Hold, 12.0);
    let mut lifted = false;
    for tick in 0..200 {
        let mut s = body.sense();
        if lifted && run.phase() == "fall" {
            s.pos = None;
        }
        let a = run.step(tick, &s);
        lifted |= run.phase() == "fall";
        body.act(&a);
        if run.done() {
            break;
        }
        body.tick();
    }
    assert_eq!(run.outcome(), (Outcome::SetupFailed, Some("died".into())));
}

// ---- the gate and the job ----

#[test]
fn the_control_gate_passes_on_one_bounce_and_names_what_failed() {
    let b = (Outcome::Bounced, None);
    let lost = (Outcome::PrimeLost, Some("rebuilt".to_string()));
    let flat = (Outcome::NoBounce, None);
    assert_eq!(gate(&[], true), Gate::Running);
    assert_eq!(gate(&[lost.clone()], true), Gate::Running);
    assert_eq!(gate(&[lost.clone(), b], true), Gate::Passed);
    assert_eq!(
        gate(&[lost.clone(), flat.clone(), lost], false),
        Gate::Failed("control did not bounce: prime_lost (rebuilt), no_bounce".into())
    );
    assert_eq!(
        gate(&[flat], false),
        Gate::Failed("control did not bounce: no_bounce".into())
    );
}

/// A match of `n` seats, each a [`Body`], spawned far apart.
fn play(job: &mut Job, n: usize, plan: Pairs, launch: f32) -> u64 {
    let mut bodies: Vec<Body> = (0..n)
        .map(|i| Body::new([100.0 + 10.0 * i as f32, 0.0, 0.0], plan, launch))
        .collect();
    for tick in 0..200_000u64 {
        let senses: Vec<SeatSense> = bodies
            .iter_mut()
            .enumerate()
            .map(|(i, b)| SeatSense {
                seat: i as u8,
                sense: b.sense(),
            })
            .collect();
        for a in job.tick(tick, &senses) {
            bodies[a.seat as usize].act(&a.act);
        }
        if job.finished() {
            return tick;
        }
        for b in &mut bodies {
            b.tick();
        }
    }
    panic!("the job never finished");
}

fn small_job() -> Job {
    let mut d = bare(vec![
        edge_spec(),
        TargetSpec {
            id: Some("far".into()),
            point: Some([20.0, 0.0, 0.0]),
            ..TargetSpec::default()
        },
    ]);
    d.no_control = false;
    d.heights = vec![1.0, 2.0];
    d.control = Some(ControlSpec {
        point: [-20.0, 0.0, 0.0],
        heights: vec![2.0, 3.0],
        edge: None,
        drift: None,
    });
    Job::new(&d, "testmap", 3).unwrap()
}

#[test]
fn a_control_that_does_not_bounce_stops_the_run_before_the_sweep() {
    let mut job = small_job();
    assert_eq!(job.gate(), &Gate::Running);
    play(&mut job, 3, Pairs::Reset, 12.0);
    assert_eq!(
        job.error(),
        Some("control did not bounce: prime_lost (reset (word 1))")
    );
    // Only the two control drops ran, one at a time.
    let r = job.records();
    assert_eq!(r.len(), 2);
    assert!(r.iter().all(|x| x.control && !x.rows.is_empty()));
    let rep = job.report();
    assert_eq!(rep.control, "failed");
    assert_eq!(rep.error.as_deref(), job.error());
    assert_eq!(rep.planned, 2 + 4);
    assert!(job.proven().is_empty());
}

#[test]
fn a_bouncing_control_opens_the_queue_and_every_drop_runs() {
    let mut job = small_job();
    play(&mut job, 3, Pairs::Hold, 12.0);
    assert_eq!(job.error(), None);
    assert_eq!(job.gate(), &Gate::Passed);
    let r = job.records();
    // The control stops at its first bounce; then the four drops.
    assert_eq!(r.iter().filter(|x| x.control).count(), 1);
    assert_eq!(r.iter().filter(|x| !x.control).count(), 4);
    assert!(r.iter().all(|x| x.outcome == Outcome::Bounced));
    // Rows only for the control and the first of each outcome.
    assert_eq!(r.iter().filter(|x| !x.rows.is_empty()).count(), 1);
    // More than one seat took drops.
    let mut seats: Vec<u8> = r.iter().map(|x| x.seat).collect();
    seats.dedup();
    assert!(seats.len() > 1, "{seats:?}");
    let p = job.proven();
    assert_eq!(p.len(), 2);
    assert_eq!(p[0].target, "e");
    assert_eq!(p[0].ok, 2);
    assert_eq!(p[0].trials, 2);
    assert_eq!(job.report().done, 5);
}

#[test]
fn setup_failures_are_tried_again_then_recorded() {
    let mut job = Job::new(&bare(vec![edge_spec()]), "x", 1).unwrap();
    let mut body = Body::new([50.0, 0.0, 0.0], Pairs::Hold, 12.0);
    body.refuse = true;
    for tick in 0..5000u64 {
        let s = vec![SeatSense {
            seat: 0,
            sense: body.sense(),
        }];
        for a in job.tick(tick, &s) {
            body.act(&a.act);
        }
        if job.finished() {
            break;
        }
        body.tick();
    }
    assert!(job.finished());
    let r = job.records();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].outcome, Outcome::SetupFailed);
    assert_eq!(r[0].attempts, 1 + RETRIES);
}

#[test]
fn a_run_without_explicit_targets_is_refused() {
    let e = Job::new(&DropsConfig::default(), "ascension", 15).unwrap_err();
    assert!(e.contains("resolved by `h2 sim drops`"), "{e}");
}

// ---- the report ----

fn rec(target: &str, outcome: Outcome, speed: f32, miss: f32, launch: f32) -> Record {
    Record {
        target: target.into(),
        shape: "slot".into(),
        height: 2.0,
        across: miss,
        outcome,
        touchdown: Some(Touchdown {
            speed,
            miss,
            pos: [miss, 0.0, 0.0],
            ..Touchdown::default()
        }),
        launch: Some(Launch {
            speed: launch,
            height: launch * launch / (2.0 * GRAVITY),
            estimated: false,
        }),
        ..Record::default()
    }
}

fn run_of(records: Vec<Record>, shard: u32) -> DropsRun {
    let t = |id: &str| Target {
        id: id.into(),
        shape: "slot".into(),
        edge: [[0.0; 3], [1.0, 0.0, 0.0]],
        normal: [0.0, 1.0],
        stand: [0.5, 0.0, 0.01],
        control: false,
    };
    DropsRun {
        format: FORMAT.into(),
        map: "ascension".into(),
        shard,
        shards: 2,
        seats: 15,
        planned: records.len() as u32,
        done: records.len() as u32,
        control: "passed".into(),
        targets: vec![t("a"), t("b")],
        records,
        ..DropsRun::default()
    }
}

/// a: launch = 3 + 2 × speed; b: launch = 10 + 2 × speed.
fn a_recs() -> Vec<Record> {
    vec![
        rec("a", Outcome::Bounced, 5.5, 0.02, 14.0),
        rec("a", Outcome::Bounced, 6.0, 0.08, 15.0),
        rec("a", Outcome::NoBounce, 5.2, 0.03, 0.2),
        // Outside the catch: doesn't bracket the threshold.
        rec("a", Outcome::NoBounce, 5.9, 0.3, 0.1),
        rec("a", Outcome::PrimeLost, 7.0, 0.0, 0.0),
    ]
}

fn b_recs() -> Vec<Record> {
    vec![
        rec("b", Outcome::Bounced, 7.0, 0.05, 24.0),
        rec("b", Outcome::Bounced, 8.0, 0.11, 26.0),
        rec("b", Outcome::SetupFailed, 0.0, 0.0, 0.0),
    ]
}

#[test]
fn the_report_measures_the_catch_the_threshold_and_the_gain() {
    let (a, b) = (a_recs(), b_recs());
    let r = summarize(&[run_of(a, 0), run_of(b, 1)]);
    assert_eq!(r.drops, 8);
    assert_eq!(r.outcomes["bounced"], 4);
    assert_eq!(r.outcomes["prime_lost"], 1);
    let c = r.catch.clone().unwrap();
    assert!((c.half_width - 0.11).abs() < 1e-6);
    assert_eq!(c.target, "b");
    let s = r.bounce_speed.unwrap();
    assert_eq!(s.min_bounced, 5.5);
    assert_eq!(s.max_unbounced, Some(5.2));
    let g = r.gain.unwrap();
    assert!((g.slope - 2.0).abs() < 1e-4, "{g:?}");
    assert_eq!(g.n, 4);
    assert_eq!(g.base, None);
    // Each target's own line, its base at the measured threshold.
    let ta = r.targets.iter().find(|t| t.id == "a").unwrap();
    let ga = ta.gain.unwrap();
    assert!((ga.slope - 2.0).abs() < 1e-4);
    assert!((ga.base.unwrap() - 14.0).abs() < 1e-3);
    assert_eq!(ta.rate, Some(0.5));
    assert_eq!(ta.prime_lost, 1);
    // A prime lost or a failed setup measures nothing: not in the rate.
    let tb = r.targets.iter().find(|t| t.id == "b").unwrap();
    assert_eq!(tb.rate, Some(1.0));
    assert_eq!(tb.setup_failed, 1);
    // Cells by height and across.
    assert!(ta.cells.iter().all(|c| c.height == 2.0));
    let md = markdown(&r);
    for want in [
        "catch half-width: 0.110",
        "BOUNCE_SPEED: 5.50",
        "EXCESS_GAIN: 2.000",
        "| a | slot |",
        "### b (slot",
    ] {
        assert!(md.contains(want), "{want:?} not in:\n{md}");
    }
    // The proven targets: both, each at its best bounce.
    let all: Vec<Record> = [a_recs(), b_recs()].concat();
    let p = proven(&run_of(vec![], 0).targets, &all);
    assert_eq!(p.len(), 2);
    assert_eq!((p[0].target.as_str(), p[0].trials, p[0].ok), ("a", 4, 2));
    assert_eq!(p[0].best, 15.0);
    assert!((p[0].offset[0] - (0.08 - 0.5)).abs() < 1e-5 && p[0].offset[1] == 0.0);
    assert_eq!(p[0].height, 2.0);
    assert_eq!((p[1].target.as_str(), p[1].best), ("b", 26.0));
}

#[test]
fn fits_need_two_speeds() {
    assert_eq!(fit(&[(5.0, 10.0)], 5.0), None);
    assert_eq!(fit(&[(5.0, 10.0), (5.0, 12.0)], 5.0), None);
    let g = fit(&[(5.0, 10.0), (6.0, 11.5)], 5.0).unwrap();
    assert!((g.slope - 1.5).abs() < 1e-5 && (g.base.unwrap() - 10.0).abs() < 1e-5);
    assert_eq!(pooled(&[vec![(5.0, 1.0)], vec![]], 5.0), None);
    let none = summarize(&[run_of(vec![rec("a", Outcome::NoBounce, 5.0, 0.0, 0.0)], 0)]);
    assert!(none.catch.is_none() && none.bounce_speed.is_none() && none.gain.is_none());
    assert!(markdown(&none).contains("catch half-width: no bounce"));
}

#[test]
fn the_errors_and_the_first_of_each_outcome_reach_the_report() {
    let mut a = run_of(vec![rec("a", Outcome::NoBounce, 5.0, 0.0, 0.0)], 0);
    a.records[0].tick = 900;
    a.error = Some("control did not bounce: no_bounce".into());
    a.control = "failed".into();
    let mut b = run_of(vec![rec("b", Outcome::NoBounce, 5.0, 0.0, 0.0)], 1);
    b.records[0].tick = 400;
    b.records[0].drop = 7;
    let r = summarize(&[a, b]);
    assert_eq!(r.errors, vec!["shard 0: control did not bounce: no_bounce"]);
    let f = &r.first["no_bounce"];
    assert_eq!((f.shard, f.drop, f.tick), (1, 7, 400));
    assert!(markdown(&r).contains("**shard 0: control did not bounce: no_bounce**"));
}
