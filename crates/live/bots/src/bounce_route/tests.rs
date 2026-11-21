use super::*;
use crate::glitches::Bounce;
use crate::glitches::prime::{CROUCH_TICKS, FORCE_TICKS, PRIMED_WORD};
use crate::nav::{NavMesh, Poly};
use crate::spots::Crouch;

fn site(x: f32, y: f32, launch: f32) -> Site {
    Site {
        bounce: Bounce {
            map: "",
            name: "test",
            stand: [x, y, 0.0],
            spread: 0.03,
            drift: [1.0, 0.0],
            source: "scan",
        },
        launch,
        fall: 0.0,
        ..Site::default()
    }
}

fn dist3(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// A map where nothing above 2 units over the start can be walked to.
fn ledge_walk(a: Vec3, b: Vec3) -> Option<f32> {
    ((a[2] - b[2]).abs() < 2.0).then(|| dist3(a, b))
}

fn flat_walk(a: Vec3, b: Vec3) -> Option<f32> {
    Some(dist3(a, b))
}

fn ask(to: Vec3, primed: bool, mode: Mode) -> Ask {
    Ask {
        from: [0.0, 0.0, 0.0],
        to,
        primed,
        mode,
    }
}

fn known(sites: Vec<Site>) -> Known {
    Known {
        sites,
        crouches: vec![Crouch::at([3.0, 0.0, 0.0])],
    }
}

#[test]
fn the_flight_comes_from_the_launch_and_gravity() {
    let s = site(0.0, 0.0, 12.3);
    let f = flight(&s, 0.0).unwrap();
    assert!((f.apex - 12.3 * 12.3 / (2.0 * GRAVITY)).abs() < 1e-4);
    // Up and back down to the start's height.
    assert!((f.time - 2.0 * 12.3 / GRAVITY).abs() < 1e-3);
    assert!((f.reach - AIR_SPEED * f.time).abs() < 1e-4);
    // Higher goal, less time.
    assert!(flight(&s, 6.0).unwrap().time < f.time);
    // Too high for the launch.
    assert!(flight(&s, f.apex).is_none());
    assert!(flight(&site(0.0, 0.0, 0.0), -1.0).is_none());
}

#[test]
fn a_goal_up_a_ledge_walking_cannot_reach_is_bounced_to() {
    let k = known(vec![
        // Too weak to clear the ledge.
        site(8.0, 0.0, 6.0),
        // Strong, but too far from the ledge's edge to reach it in the air.
        site(-30.0, 0.0, 12.3),
        // The one.
        site(10.0, 0.0, 12.3),
    ]);
    let p = plan(&ask([18.0, 0.0, 6.0], false, Mode::Help), &k, &ledge_walk);
    assert_eq!(p.why, Why::Helps);
    assert_eq!(p.site.unwrap().bounce.stand, [10.0, 0.0, 0.0]);
    assert!(matches!(p.steps.last(), Some(Step::Nav { to }) if *to == [18.0, 0.0, 6.0]));
}

#[test]
fn the_cheaper_bounce_wins_when_several_help() {
    let k = known(vec![site(4.0, 0.0, 12.3), site(10.0, 0.0, 12.3)]);
    // Both reach it; the one nearer the crouch spot is less walking.
    let p = plan(&ask([14.0, 0.0, 6.0], false, Mode::Help), &k, &ledge_walk);
    assert_eq!(p.site.unwrap().bounce.stand[0], 4.0);
}

#[test]
fn with_no_bounce_that_helps_it_is_the_plain_walk() {
    let k = known(vec![site(10.0, 0.0, 12.3)]);
    // On the flat: the detour to prime is dearer than walking.
    let p = plan(&ask([20.0, 0.0, 0.0], false, Mode::Help), &k, &flat_walk);
    assert_eq!(p.why, Why::Plain);
    assert_eq!(p.steps, vec![Step::Nav { to: [20.0, 0.0, 0.0] }]);
    assert!(p.site.is_none());
    // No spots at all.
    let p = plan(&ask([18.0, 0.0, 6.0], false, Mode::Help), &Known::default(), &ledge_walk);
    assert_eq!(p.why, Why::Plain);
    // A ledge the launch cannot clear.
    let weak = known(vec![site(10.0, 0.0, 6.0)]);
    let p = plan(&ask([18.0, 0.0, 6.0], false, Mode::Help), &weak, &ledge_walk);
    assert_eq!(p.why, Why::Plain);
}

#[test]
fn without_a_crouch_spot_to_prime_at_there_is_no_bounce_route() {
    let k = Known {
        sites: vec![site(10.0, 0.0, 12.3)],
        crouches: Vec::new(),
    };
    let p = plan(&ask([18.0, 0.0, 6.0], false, Mode::Help), &k, &ledge_walk);
    assert_eq!(p.why, Why::Plain);
    // Already primed: no crouch spot needed.
    let p = plan(&ask([18.0, 0.0, 6.0], true, Mode::Help), &k, &ledge_walk);
    assert_eq!(p.why, Why::Helps);
    assert!(!p.steps.iter().any(|s| matches!(s, Step::Prime(_))));
}

#[test]
fn a_site_that_needs_a_drop_is_not_planned() {
    let mut s = site(10.0, 0.0, 12.3);
    s.fall = 3.0;
    let p = plan(&ask([18.0, 0.0, 6.0], false, Mode::Help), &known(vec![s]), &ledge_walk);
    assert_eq!(p.why, Why::Plain);
}

#[test]
fn the_route_primes_first_and_never_crouches_between_the_prime_and_the_bounce() {
    let k = known(vec![site(10.0, 0.0, 12.3)]);
    let p = plan(&ask([18.0, 0.0, 6.0], false, Mode::Help), &k, &ledge_walk);
    let names: Vec<_> = p.steps.iter().map(Step::name).collect();
    assert_eq!(
        names,
        ["walk_crouched", "prime", "walk", "bounce", "fly", "nav"]
    );
    assert_eq!(
        p.steps[0],
        Step::Walk {
            to: [3.0, 0.0, 0.0],
            crouch: true
        }
    );
    assert_eq!(p.steps[1], Step::Prime(Crouch::at([3.0, 0.0, 0.0])));
    // From the prime to the bounce: no crouching walk.
    let prime = p.steps.iter().position(|s| matches!(s, Step::Prime(_))).unwrap();
    let bounce = p.steps.iter().position(|s| matches!(s, Step::Bounce(_))).unwrap();
    for s in &p.steps[prime + 1..bounce] {
        assert!(matches!(s, Step::Walk { crouch: false, .. }), "{s:?}");
    }
    // The walk ends behind the spot.
    assert_eq!(
        p.steps[prime + 1],
        Step::Walk {
            to: k.sites[0].bounce.from(),
            crouch: false
        }
    );
}

#[test]
fn a_primed_bot_goes_straight_to_the_bounce() {
    let k = known(vec![site(10.0, 0.0, 12.3)]);
    let p = plan(&ask([18.0, 0.0, 6.0], true, Mode::Help), &k, &ledge_walk);
    let names: Vec<_> = p.steps.iter().map(Step::name).collect();
    assert_eq!(names, ["walk", "bounce", "fly", "nav"]);
}

#[test]
fn for_fun_the_highest_launch_within_reach_is_taken_even_on_the_flat() {
    let k = known(vec![
        site(10.0, 0.0, 10.0),
        site(15.0, 0.0, 14.0),
        // The highest, but out of reach.
        site(500.0, 0.0, 20.0),
    ]);
    let p = plan(&ask([20.0, 0.0, 0.0], false, Mode::Fun), &k, &flat_walk);
    assert_eq!(p.why, Why::Fun);
    assert_eq!(p.site.unwrap().launch, 14.0);
    assert!(p.steps.iter().any(|s| matches!(s, Step::Prime(_))));
    // None in reach: the plain walk.
    let far = known(vec![site(500.0, 0.0, 20.0)]);
    assert_eq!(plan(&ask([20.0, 0.0, 0.0], false, Mode::Fun), &far, &flat_walk).why, Why::Plain);
}

#[test]
fn a_plan_makes_a_bounded_number_of_path_queries_however_many_spots_there_are() {
    let sites: Vec<Site> = (0..400)
        .map(|i| site(5.0 + (i % 40) as f32 * 0.5, (i / 40) as f32, 12.3))
        .collect();
    let crouches: Vec<Crouch> =
        (0..500).map(|i| Crouch::at([(i % 50) as f32 * 0.4, (i / 50) as f32, 0.0])).collect();
    let k = Known { sites, crouches };
    for mode in [Mode::Help, Mode::Fun] {
        for primed in [false, true] {
            let n = std::cell::Cell::new(0u32);
            let walk = |a: Vec3, b: Vec3| {
                n.set(n.get() + 1);
                ledge_walk(a, b)
            };
            let p = plan(&ask([18.0, 0.0, 6.0], primed, mode), &k, &walk);
            assert!(n.get() <= MAX_QUERIES, "{mode:?} primed {primed}: {} queries", n.get());
            assert!(n.get() > 0);
            assert!(p.steps.len() >= 1);
        }
    }
}

#[test]
fn spots_farther_than_the_reach_cost_no_query() {
    let k = Known {
        sites: vec![site(900.0, 0.0, 12.3)],
        crouches: vec![Crouch::at([800.0, 0.0, 0.0])],
    };
    let n = std::cell::Cell::new(0u32);
    let walk = |a: Vec3, b: Vec3| {
        n.set(n.get() + 1);
        flat_walk(a, b)
    };
    let p = plan(&ask([20.0, 0.0, 0.0], false, Mode::Fun), &k, &walk);
    assert_eq!(p.why, Why::Plain);
    assert_eq!(n.get(), 0);
}

#[test]
fn the_mode_names_round_trip() {
    for m in [Mode::Help, Mode::Fun] {
        assert_eq!(Mode::parse(m.name()), Some(m));
    }
    assert_eq!(Mode::parse("loud"), None);
}

// ---- playing a plan ----

fn flat_map() -> MapData {
    let floor = Poly::new(vec![
        [-40.0, -40.0, 0.0],
        [40.0, -40.0, 0.0],
        [40.0, 40.0, 0.0],
        [-40.0, 40.0, 0.0],
    ]);
    MapData {
        nav: NavMesh::new(vec![floor], vec![]),
        ..MapData::default()
    }
}

/// Plays `run` with a bot that walks as asked at the run speed, returning
/// the motions it gave until `ticks` or the step named `until` is reached.
fn play(
    run: &mut OrderRun,
    map: &MapData,
    at: &mut Vec3,
    tick: &mut u32,
    pair: Option<u32>,
    until: &str,
    ticks: u32,
) -> Vec<RouteMotion> {
    let mut seen = Vec::new();
    for _ in 0..ticks {
        if run.status().what == until {
            break;
        }
        let o = run.step(map, *at, *tick, pair);
        if o.done.is_some() {
            break;
        }
        let m = o.motion;
        let step = 5.0 * m.speed / 30.0;
        at[0] += m.dir[0] * step;
        at[1] += m.dir[1] * step;
        seen.push(m);
        *tick += 1;
    }
    seen
}

#[test]
fn the_run_walks_to_the_crouch_spot_primes_then_walks_without_crouching() {
    let map = flat_map();
    let k = known(vec![site(10.0, 0.0, 12.3)]);
    let p = plan(&ask([18.0, 0.0, 0.0], false, Mode::Fun), &k, &flat_walk);
    assert!(p.steps.iter().any(|s| matches!(s, Step::Prime(_))));
    let mut run = OrderRun::new(p, false, 0, 1);
    let (mut at, mut tick) = ([0.0, 0.0, 0.0], 0);

    // To the crouch spot: crouched, not yet primed.
    let walked = play(&mut run, &map, &mut at, &mut tick, None, "prime", 400);
    assert!(!walked.is_empty());
    assert!(walked.iter().all(|m| m.crouch));
    assert_eq!(run.status().what, "prime");
    assert!(!run.primed());

    // Priming: crouch held, then let go, then primed.
    let primed = play(&mut run, &map, &mut at, &mut tick, None, "walk", 100);
    // (The first crouch tick went with the walk's last.)
    let held = primed.iter().take_while(|m| m.crouch).count() as u32;
    assert_eq!(held + 1, CROUCH_TICKS);
    assert!(primed.iter().skip(held as usize).all(|m| !m.crouch));
    assert!(primed.len() as u32 >= FORCE_TICKS);
    assert!(run.primed());
    assert!(!run.status().read);

    // To the bounce: primed the whole way, the crouch never pressed.
    let after = play(&mut run, &map, &mut at, &mut tick, None, "bounce", 400);
    assert!(!after.is_empty());
    assert!(after.iter().all(|m| !m.crouch));
    assert!(run.primed());
    assert_eq!(run.status().what, "bounce");
}

#[test]
fn a_read_word_primes_at_once_and_a_lost_word_sends_the_bot_back() {
    let map = flat_map();
    let k = known(vec![site(10.0, 0.0, 12.3)]);
    let p = plan(&ask([18.0, 0.0, 0.0], false, Mode::Fun), &k, &flat_walk);
    let mut run = OrderRun::new(p, false, 0, 1);
    let (mut at, mut tick) = ([0.0, 0.0, 0.0], 0);
    play(&mut run, &map, &mut at, &mut tick, None, "prime", 400);
    let primed = play(&mut run, &map, &mut at, &mut tick, Some(PRIMED_WORD), "walk", 100);
    assert!((primed.len() as u32) < CROUCH_TICKS + FORCE_TICKS);
    assert!(run.primed());
    assert!(run.status().read);
    // Walking on, the word reads 0: it was lost.
    play(&mut run, &map, &mut at, &mut tick, Some(PRIMED_WORD), "bounce", 3);
    run.step(&map, at, tick, Some(0));
    assert!(!run.primed());
    // Back at the crouch spot (or already at it).
    assert!(matches!(run.status().what, "walk_crouched" | "prime"));
}

#[test]
fn a_run_that_never_gets_forced_fails_and_lets_the_crouch_go() {
    let map = flat_map();
    let k = known(vec![site(10.0, 0.0, 12.3)]);
    let p = plan(&ask([18.0, 0.0, 0.0], false, Mode::Fun), &k, &flat_walk);
    let mut run = OrderRun::new(p, false, 0, 1);
    let (mut at, mut tick) = ([0.0, 0.0, 0.0], 0);
    play(&mut run, &map, &mut at, &mut tick, None, "prime", 400);
    // The word reads, and is never -3.
    let mut done = None;
    for _ in 0..300 {
        let o = run.step(&map, at, tick, Some(0));
        tick += 1;
        if o.done.is_some() {
            done = o.done;
            break;
        }
    }
    assert_eq!(done, Some(Err("prime_failed")));
    assert!(!run.primed());
}

#[test]
fn a_run_gives_up_after_its_time() {
    let map = flat_map();
    let p = plan(&ask([30.0, 0.0, 0.0], false, Mode::Help), &Known::default(), &flat_walk);
    let mut run = OrderRun::new(p, false, 0, 1);
    let o = run.step(&map, [0.0; 3], ORDER_TICKS + 1, None);
    assert_eq!(o.done, Some(Err("timeout")));
}

#[test]
fn a_plain_order_walks_to_the_goal_and_finishes() {
    let map = flat_map();
    let p = plan(&ask([6.0, 0.0, 0.0], false, Mode::Help), &Known::default(), &flat_walk);
    let mut run = OrderRun::new(p, false, 0, 1);
    let (mut at, mut tick) = ([0.0, 0.0, 0.0], 0);
    let mut done = None;
    for _ in 0..600 {
        let o = run.step(&map, at, tick, None);
        if o.done.is_some() {
            done = o.done;
            break;
        }
        let step = 5.0 * o.motion.speed / 30.0;
        at[0] += o.motion.dir[0] * step;
        at[1] += o.motion.dir[1] * step;
        tick += 1;
    }
    assert_eq!(done, Some(Ok(())));
}

// ---- proven spots, drops and the walking prime ----

fn drop_site(x: f32, launch: f32) -> Site {
    Site {
        fall: 2.4,
        need: 4.5,
        width: 0.2,
        ..site(x, 0.0, launch)
    }
}

#[test]
fn a_site_that_needs_a_drop_is_planned_from_its_launch_floor() {
    let s = drop_site(10.0, 12.3);
    let k = known(vec![s]);
    // No launch floor: not used.
    let p = plan_with(&ask([18.0, 0.0, 6.0], false, Mode::Help), &k, &ledge_walk, &|_| None);
    assert_eq!(p.why, Why::Plain);
    // A floor 2.4 up, a unit back.
    let floor = [9.0, 0.0, 2.4];
    let walk = |a: Vec3, b: Vec3| Some(dist3(a, b));
    let p = plan_with(&ask([18.0, 0.0, 6.0], false, Mode::Fun), &k, &walk, &|_| Some(floor));
    let names: Vec<_> = p.steps.iter().map(Step::name).collect();
    assert_eq!(names, ["walk_crouched", "prime", "walk", "drop", "fly", "nav"]);
    assert_eq!(p.steps[2], Step::Walk { to: floor, crouch: false });
    assert!(matches!(p.steps[3], Step::Drop { from, .. } if from == floor));
}

#[test]
fn proven_sites_and_crouch_spots_come_first() {
    let mut proven = site(14.0, 0.0, 10.0);
    proven.status = superbounce::Status::Proven;
    let k = Known {
        sites: vec![site(10.0, 0.0, 14.0), proven],
        crouches: vec![
            Crouch::at([1.0, 0.0, 0.0]),
            Crouch {
                status: superbounce::Status::Proven,
                ..Crouch::at([5.0, 0.0, 0.0])
            },
        ],
    };
    // For fun the higher launch would win; the proven one does.
    let p = plan(&ask([20.0, 0.0, 0.0], false, Mode::Fun), &k, &flat_walk);
    assert_eq!(p.site.unwrap().bounce.stand[0], 14.0);
    assert!(matches!(p.steps[1], Step::Prime(c) if c.stand[0] == 5.0));
}

#[test]
fn with_the_way_out_known_the_prime_starts_out_along_it_and_walks_there_standing() {
    let c = Crouch {
        out: [0.0, 1.0],
        ..Crouch::at([3.0, 0.0, 0.0])
    };
    let k = Known {
        sites: vec![site(10.0, 0.0, 12.3)],
        crouches: vec![c],
    };
    let p = plan(&ask([18.0, 0.0, 6.0], false, Mode::Help), &k, &ledge_walk);
    let start = prime_start(&c);
    assert!((start[1] - crate::glitches::prime::START_OUT).abs() < 1e-5 && start[0] == 3.0);
    assert_eq!(p.steps[0], Step::Walk { to: start, crouch: false });
    assert_eq!(p.steps[0].name(), "walk");
}

/// Plays a walking prime at a crouch spot whose lip lets go `release_at`
/// units out along the way out; the word turns -3 there when `primes`.
fn walk_prime(primes: bool) -> (Option<Result<(), &'static str>>, OrderRun, Vec<RouteMotion>) {
    let map = flat_map();
    let c = Crouch {
        out: [0.0, 1.0],
        ..Crouch::at([3.0, 0.0, 0.0])
    };
    let plan = Plan {
        steps: vec![Step::Prime(c), Step::Nav { to: [3.0, 5.0, 0.0] }],
        why: Why::Fun,
        site: None,
        cost: None,
    };
    let mut run = OrderRun::new(plan, false, 0, 1);
    let mut at = [3.0, 0.4, 0.0];
    let mut seen = Vec::new();
    let mut out_since: Option<u32> = None;
    for tick in 0..2000 {
        // The word: 0 until the bot walks out 0.06 past the stand after
        // a crouch, then -3 for good (the game keeps it until a crouch or
        // a landing).
        let crouched = seen.iter().any(|m: &RouteMotion| m.crouch);
        let word = if out_since.is_some()
            || (primes && crouched && at[1] > 0.06 && run.status().phase == "out")
        {
            out_since.get_or_insert(tick);
            PRIMED_WORD
        } else {
            0
        };
        let o = run.step(&map, at, tick, Some(word));
        if o.done.is_some() {
            return (o.done, run, seen);
        }
        let m = o.motion;
        let step = 5.0 * m.speed / 30.0;
        at[0] += m.dir[0] * step;
        // The lip stops it 2 cm past the stand.
        at[1] = (at[1] + m.dir[1] * step).max(-0.02);
        seen.push(m);
    }
    (None, run, seen)
}

#[test]
fn a_walking_prime_crouch_walks_in_lets_go_and_walks_out_until_the_word_turns() {
    let (done, run, seen) = walk_prime(true);
    assert_eq!(done, Some(Ok(())));
    let log: Vec<_> = run.log().iter().map(|l| (l.step, l.what)).collect();
    assert!(log.contains(&("prime", "start")) && log.contains(&("prime", "done")), "{log:?}");
    // Crouch, walk in crouched toward -y, release, then out standing
    // toward +y, never crouching after it let go.
    let first_crouch = seen.iter().position(|m| m.crouch).unwrap();
    assert_eq!(seen[first_crouch].speed, 0.0, "crouch before walking in");
    let let_go = first_crouch + seen[first_crouch..].iter().position(|m| !m.crouch).unwrap();
    assert!(seen[first_crouch..let_go].iter().all(|m| m.dir[1] <= 0.0));
    assert!(seen[let_go..].iter().all(|m| !m.crouch));
    assert!(seen[let_go..].iter().any(|m| m.dir[1] > 0.9));
}

#[test]
fn a_walking_prime_that_never_turns_the_word_tries_other_angles_then_fails() {
    let (done, _, seen) = walk_prime(false);
    assert_eq!(done, Some(Err("prime_failed")));
    let presses = seen.windows(2).filter(|w| !w[0].crouch && w[1].crouch).count()
        + usize::from(seen[0].crouch);
    assert_eq!(presses as u32, crate::glitches::prime::WALK_TRIES);
    // Each try crouches first, standing, then walks in; the second comes
    // in turned.
    let presses: Vec<usize> = (0..seen.len())
        .filter(|&i| seen[i].crouch && (i == 0 || !seen[i - 1].crouch))
        .collect();
    for &p in &presses {
        let first = &seen[p..p + crate::glitches::prime::CROUCH_FIRST as usize];
        assert!(first.iter().all(|m| m.crouch && m.speed == 0.0), "{first:?}");
    }
    let ins: Vec<[f32; 2]> = presses
        .iter()
        .filter_map(|&p| seen[p..].iter().find(|m| m.crouch && m.speed > 0.0).map(|m| m.dir))
        .collect();
    assert!(ins.len() >= 3 && (ins[1][0] - ins[2][0]).abs() > 0.1, "{ins:?}");
}

/// A ledge at z 2.4 for x under 0, a floor at 0 past it (not linked).
fn ledge_map() -> MapData {
    let ledge = Poly::new(vec![[-10.0, -5.0, 2.4], [0.0, -5.0, 2.4], [0.0, 5.0, 2.4], [-10.0, 5.0, 2.4]]);
    let floor = Poly::new(vec![[0.0, -5.0, 0.0], [10.0, -5.0, 0.0], [10.0, 5.0, 0.0], [0.0, 5.0, 0.0]]);
    MapData {
        nav: NavMesh::new(vec![ledge, floor], vec![]),
        ..MapData::default()
    }
}

/// Plays a drop off the ledge onto a slot at x 1; the landing launches
/// when it is within 0.1 of it and `bounces`.
fn play_drop(bounces: bool) -> (Option<Result<(), &'static str>>, OrderRun) {
    let map = ledge_map();
    let s = drop_site(1.0, 12.3);
    let from = [-0.4, 0.0, 2.4];
    let plan = Plan {
        steps: vec![
            Step::Walk { to: from, crouch: false },
            Step::Drop { from, site: s },
            Step::Fly { to: [3.0, 0.0, 0.0] },
        ],
        why: Why::Fun,
        site: Some(s),
        cost: None,
    };
    let mut run = OrderRun::new(plan, true, 0, 1);
    let (mut at, mut vz) = ([-1.0f32, 0.0, 2.4], 0.0f32);
    for tick in 0..900 {
        let o = run.step(&map, at, tick, Some(PRIMED_WORD));
        if o.done.is_some() {
            return (o.done, run);
        }
        let m = o.motion;
        let step = 5.0 * m.speed / 30.0;
        at[0] += m.dir[0] * step;
        at[1] += m.dir[1] * step;
        let floor = if at[0] < 0.0 { 2.4 } else { 0.0 };
        if at[2] > floor || vz > 0.0 {
            vz -= GRAVITY / 30.0;
            at[2] += vz / 30.0;
            if at[2] <= floor {
                at[2] = floor;
                vz = if bounces && (at[0] - 1.0).abs() < 0.1 && vz < -4.0 { 12.0 } else { 0.0 };
            }
        }
    }
    (None, run)
}

#[test]
fn a_drop_lines_up_runs_off_steers_onto_the_slot_and_rides_the_launch() {
    let (_, run) = play_drop(true);
    let log: Vec<_> = run.log().iter().map(|l| (l.step, l.what)).collect();
    assert!(log.contains(&("drop", "done")), "{log:?}");
    let t = run.status().landing.expect("a touchdown");
    assert!(t.bounced() && t.miss < 0.1 && t.speed_in > 4.0, "{t:?}");
    assert_eq!(t.result(0.2, 4.5), "bounce");
    assert!(log.iter().any(|l| *l == ("fly", "bounce") || *l == ("drop", "bounce")), "{log:?}");
}

#[test]
fn a_drop_that_does_not_launch_walks_back_up_for_another() {
    let (done, run) = play_drop(false);
    let log: Vec<_> = run.log().iter().map(|l| (l.step, l.what)).collect();
    assert!(log.contains(&("drop", "retry")), "{log:?}");
    assert!(log.iter().any(|l| l.0 == "drop" && l.1 == "no_bounce"), "{log:?}");
    // The floor is not linked to the ledge here: the walk back fails.
    assert_eq!(done, Some(Err("no_path")));
}

#[test]
fn a_touchdown_is_named_by_its_first_failed_test() {
    let t = Touchdown {
        tick: 1,
        at: [0.0; 3],
        speed_in: 5.0,
        vz_out: 0.0,
        miss: 0.05,
        word: Some(PRIMED_WORD),
    };
    assert_eq!(t.result(0.2, 4.0), "no_bounce");
    assert_eq!(Touchdown { vz_out: 12.0, ..t }.result(0.2, 4.0), "bounce");
    assert_eq!(Touchdown { word: Some(0), ..t }.result(0.2, 4.0), "not_primed");
    assert_eq!(Touchdown { miss: 0.2, ..t }.result(0.2, 4.0), "missed");
    assert_eq!(Touchdown { speed_in: 3.0, ..t }.result(0.2, 4.0), "too_slow");
}

// ---- a named site ----

#[test]
fn a_named_site_is_planned_by_itself_with_no_reach_limit() {
    // Far past FUN_REACH, and the only site: the route still goes there.
    let far = site(200.0, 0.0, 12.3);
    let k = known(vec![site(10.0, 0.0, 14.0), far]);
    let p = plan_site([0.0; 3], false, far, &k, &flat_walk, &|_| None).unwrap();
    assert_eq!(p.why, Why::Asked);
    assert_eq!(p.site, Some(far));
    let names: Vec<_> = p.steps.iter().map(Step::name).collect();
    assert_eq!(names, ["walk_crouched", "prime", "walk", "bounce", "fly"]);
    assert_eq!(p.steps[2], Step::Walk { to: far.bounce.from(), crouch: false });
    // Primed: no prime. A drop with no floor, or nowhere to prime: none.
    let p = plan_site([0.0; 3], true, far, &k, &flat_walk, &|_| None).unwrap();
    assert!(!p.steps.iter().any(|s| matches!(s, Step::Prime(_))));
    let s = drop_site(10.0, 12.3);
    assert!(plan_site([0.0; 3], false, s, &k, &flat_walk, &|_| None).is_none());
    let floor = [9.0, 0.0, 2.4];
    let p = plan_site([0.0; 3], false, s, &k, &flat_walk, &|_| Some(floor)).unwrap();
    assert!(matches!(p.steps[3], Step::Drop { from, .. } if from == floor));
    let bare = Known {
        sites: vec![far],
        crouches: Vec::new(),
    };
    assert!(plan_site([0.0; 3], false, far, &bare, &flat_walk, &|_| None).is_none());
    // The approach: behind a standing site, the floor of a drop.
    assert_eq!(approach(&far, &|_| None), Some(far.bounce.from()));
    assert_eq!(approach(&s, &|_| Some(floor)), Some(floor));
    assert_eq!(approach(&s, &|_| None), None);
}
