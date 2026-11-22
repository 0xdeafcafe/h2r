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
            takeoff: None,
            landing: None,
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
    assert_eq!(
        p.steps,
        vec![Step::Nav {
            to: [20.0, 0.0, 0.0]
        }]
    );
    assert!(p.site.is_none());
    // No spots at all.
    let p = plan(
        &ask([18.0, 0.0, 6.0], false, Mode::Help),
        &Known::default(),
        &ledge_walk,
    );
    assert_eq!(p.why, Why::Plain);
    // A ledge the launch cannot clear.
    let weak = known(vec![site(10.0, 0.0, 6.0)]);
    let p = plan(
        &ask([18.0, 0.0, 6.0], false, Mode::Help),
        &weak,
        &ledge_walk,
    );
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
    let p = plan(
        &ask([18.0, 0.0, 6.0], false, Mode::Help),
        &known(vec![s]),
        &ledge_walk,
    );
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
    let prime = p
        .steps
        .iter()
        .position(|s| matches!(s, Step::Prime(_)))
        .unwrap();
    let bounce = p
        .steps
        .iter()
        .position(|s| matches!(s, Step::Bounce(_)))
        .unwrap();
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
    assert_eq!(
        plan(&ask([20.0, 0.0, 0.0], false, Mode::Fun), &far, &flat_walk).why,
        Why::Plain
    );
}

#[test]
fn a_plan_makes_a_bounded_number_of_path_queries_however_many_spots_there_are() {
    let sites: Vec<Site> = (0..400)
        .map(|i| site(5.0 + (i % 40) as f32 * 0.5, (i / 40) as f32, 12.3))
        .collect();
    let crouches: Vec<Crouch> = (0..500)
        .map(|i| Crouch::at([(i % 50) as f32 * 0.4, (i / 50) as f32, 0.0]))
        .collect();
    let k = Known { sites, crouches };
    for mode in [Mode::Help, Mode::Fun] {
        for primed in [false, true] {
            let n = std::cell::Cell::new(0u32);
            let walk = |a: Vec3, b: Vec3| {
                n.set(n.get() + 1);
                ledge_walk(a, b)
            };
            let p = plan(&ask([18.0, 0.0, 6.0], primed, mode), &k, &walk);
            assert!(
                n.get() <= MAX_QUERIES,
                "{mode:?} primed {primed}: {} queries",
                n.get()
            );
            assert!(n.get() > 0);
            assert!(!p.steps.is_empty());
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
    let primed = play(
        &mut run,
        &map,
        &mut at,
        &mut tick,
        Some(PRIMED_WORD),
        "walk",
        100,
    );
    assert!((primed.len() as u32) < CROUCH_TICKS + FORCE_TICKS);
    assert!(run.primed());
    assert!(run.status().read);
    // Walking on, the word reads 0: it was lost.
    play(
        &mut run,
        &map,
        &mut at,
        &mut tick,
        Some(PRIMED_WORD),
        "bounce",
        3,
    );
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
    let p = plan(
        &ask([30.0, 0.0, 0.0], false, Mode::Help),
        &Known::default(),
        &flat_walk,
    );
    let mut run = OrderRun::new(p, false, 0, 1);
    let o = run.step(&map, [0.0; 3], ORDER_TICKS + 1, None);
    assert_eq!(o.done, Some(Err("timeout")));
}

#[test]
fn a_plain_order_walks_to_the_goal_and_finishes() {
    let map = flat_map();
    let p = plan(
        &ask([6.0, 0.0, 0.0], false, Mode::Help),
        &Known::default(),
        &flat_walk,
    );
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
        ..site(x, 0.0, launch)
    }
}

#[test]
fn a_site_that_needs_a_drop_is_planned_from_its_launch_floor() {
    let s = drop_site(10.0, 12.3);
    let k = known(vec![s]);
    // No launch floor: not used.
    let p = plan_with(
        &ask([18.0, 0.0, 6.0], false, Mode::Help),
        &k,
        &ledge_walk,
        &|_| None,
    );
    assert_eq!(p.why, Why::Plain);
    // A floor 2.4 up, a unit back.
    let floor = [9.0, 0.0, 2.4];
    let walk = |a: Vec3, b: Vec3| Some(dist3(a, b));
    let p = plan_with(&ask([18.0, 0.0, 6.0], false, Mode::Fun), &k, &walk, &|_| {
        Some(floor)
    });
    let names: Vec<_> = p.steps.iter().map(Step::name).collect();
    assert_eq!(
        names,
        ["walk_crouched", "prime", "walk", "drop", "fly", "nav"]
    );
    assert_eq!(
        p.steps[2],
        Step::Walk {
            to: floor,
            crouch: false
        }
    );
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
    assert_eq!(
        p.steps[0],
        Step::Walk {
            to: start,
            crouch: false
        }
    );
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
        steps: vec![
            Step::Prime(c),
            Step::Nav {
                to: [3.0, 5.0, 0.0],
            },
        ],
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
    assert!(
        log.contains(&("prime", "start")) && log.contains(&("prime", "done")),
        "{log:?}"
    );
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
    let presses = seen
        .windows(2)
        .filter(|w| !w[0].crouch && w[1].crouch)
        .count()
        + usize::from(seen[0].crouch);
    assert_eq!(presses as u32, crate::glitches::prime::WALK_TRIES);
    // Each try crouches first, standing, then walks in; the second comes
    // in turned.
    let presses: Vec<usize> = (0..seen.len())
        .filter(|&i| seen[i].crouch && (i == 0 || !seen[i - 1].crouch))
        .collect();
    for &p in &presses {
        let first = &seen[p..p + crate::glitches::prime::CROUCH_FIRST as usize];
        assert!(
            first.iter().all(|m| m.crouch && m.speed == 0.0),
            "{first:?}"
        );
    }
    let ins: Vec<[f32; 2]> = presses
        .iter()
        .filter_map(|&p| {
            seen[p..]
                .iter()
                .find(|m| m.crouch && m.speed > 0.0)
                .map(|m| m.dir)
        })
        .collect();
    assert!(
        ins.len() >= 3 && (ins[1][0] - ins[2][0]).abs() > 0.1,
        "{ins:?}"
    );
}

/// A ledge at z 2.4 for x under 0, a floor at 0 past it (not linked).
fn ledge_map() -> MapData {
    let ledge = Poly::new(vec![
        [-10.0, -5.0, 2.4],
        [0.0, -5.0, 2.4],
        [0.0, 5.0, 2.4],
        [-10.0, 5.0, 2.4],
    ]);
    let floor = Poly::new(vec![
        [0.0, -5.0, 0.0],
        [10.0, -5.0, 0.0],
        [10.0, 5.0, 0.0],
        [0.0, 5.0, 0.0],
    ]);
    MapData {
        nav: NavMesh::new(vec![ledge, floor], vec![]),
        ..MapData::default()
    }
}

/// Where a run off the ledge at full stick lands on its slot: 0.05 back
/// from the edge, the slot 1.57 out, inside the 1.54 to 1.6 the air stick
/// gives a 2.4 fall from 2 units/s; the site needs 4, which the run-off's
/// 4.46 makes.
const RUN_OFF_FROM: Vec3 = [-0.05, 0.0, 2.4];

fn run_off_site() -> Site {
    Site {
        need: 4.0,
        ..drop_site(1.52, 12.3)
    }
}

/// Where only a running jump lands on its slot: the site needs 4.5, over
/// the run-off's 4.46, and the slot is 2.3 out from 0.3 back from the
/// edge, inside the 1.9 to 2.76 a full-stick jump's flight reaches.
const JUMP_FROM: Vec3 = [-0.3, 0.0, 2.4];

fn jump_site() -> Site {
    drop_site(2.0, 12.3)
}

/// Plays the run off the ledge onto its slot ([`RUN_OFF_FROM`],
/// [`run_off_site`]).
fn play_drop(bounces: bool) -> (Option<Result<(), &'static str>>, OrderRun) {
    play_drop_from(RUN_OFF_FROM, run_off_site(), bounces).0
}

/// Plays a drop off the ledge from the launch point `from` onto `s`, with
/// the motions it asked; the landing launches when it is within 0.1 of
/// the slot, comes in faster than 4, and `bounces`. The body is the drill
/// runs' (docs/game/notes/2026-11-21-bot-bounce-sim.md): on the ground it
/// moves at [`ground_speed`] of the stick (a crawl at least, so a walk's
/// slowing end arrives); a jump leaves at [`JUMP_VZ`] [`INPUT_LAG`] ticks
/// after the press, if the body is still on the floor; in the air the
/// stick drives the sideways speed toward the stick times [`AIR_TOP`] at
/// [`AIR_ACCEL`], so letting go brakes too.
fn play_drop_from(
    from: Vec3,
    s: Site,
    bounces: bool,
) -> (
    (Option<Result<(), &'static str>>, OrderRun),
    Vec<(Vec3, RouteMotion)>,
) {
    let map = ledge_map();
    let slot = s.target();
    let mut seen = Vec::new();
    let plan = Plan {
        steps: vec![
            Step::Walk {
                to: from,
                crouch: false,
            },
            Step::Drop { from, site: s },
            Step::Fly {
                to: [3.0, 0.0, 0.0],
            },
        ],
        why: Why::Fun,
        site: Some(s),
        cost: None,
    };
    let floor_at = |x: f32| if x < 0.0 { 2.4 } else { 0.0 };
    let mut run = OrderRun::new(plan, true, 0, 1);
    let (mut at, mut vz, mut side) = ([-1.0f32, 0.0, 2.4], 0.0f32, [0.0f32; 2]);
    let mut press: Option<u32> = None;
    for tick in 0..900 {
        let o = run.step(&map, at, tick, Some(PRIMED_WORD));
        if o.done.is_some() {
            return ((o.done, run), seen);
        }
        let m = o.motion;
        seen.push((at, m));
        if at[2] <= floor_at(at[0]) && vz <= 0.0 {
            let v = if m.speed > 0.0 {
                ground_speed(m.speed).max(0.2)
            } else {
                0.0
            };
            side = [m.dir[0] * v, m.dir[1] * v];
        } else {
            let want = [m.dir[0] * m.speed * AIR_TOP, m.dir[1] * m.speed * AIR_TOP];
            let (d, gap) = norm([want[0] - side[0], want[1] - side[1]]);
            let step = gap.min(AIR_ACCEL / 30.0);
            side[0] += d[0] * step;
            side[1] += d[1] * step;
        }
        at[0] += side[0] / 30.0;
        at[1] += side[1] / 30.0;
        let floor = floor_at(at[0]);
        let on = at[2] <= floor && vz <= 0.0;
        if m.jump && on && press.is_none() {
            press = Some(tick);
        }
        if press.is_some_and(|p| tick >= p + INPUT_LAG) {
            if on {
                vz = JUMP_VZ;
            }
            press = None;
        }
        if at[2] > floor || vz > 0.0 {
            vz -= FALL_GRAVITY / 30.0;
            at[2] += vz / 30.0;
            if at[2] <= floor {
                at[2] = floor;
                let on_slot = dist_xy(at, slot) < 0.1;
                vz = if bounces && on_slot && vz < -4.0 {
                    12.0
                } else {
                    0.0
                };
            }
        }
    }
    ((None, run), seen)
}

#[test]
fn a_drop_lines_up_runs_off_steers_onto_the_slot_and_rides_the_launch() {
    let ((_, run), seen) = play_drop_from(RUN_OFF_FROM, run_off_site(), true);
    let log: Vec<_> = run.log().iter().map(|l| (l.step, l.what)).collect();
    assert!(log.contains(&("drop", "done")), "{log:?}");
    // A run off the edge: lined up behind the launch point, at full
    // stick, no jump pressed.
    assert!(!seen.iter().any(|(_, m)| m.jump), "a jump");
    assert!(
        seen.iter()
            .any(|(a, _)| a[0] < RUN_OFF_FROM[0] - RUN_UP + 0.2),
        "no run-up"
    );
    assert!(
        seen.iter()
            .any(|(a, m)| a[2] == 2.4 && a[0] > RUN_OFF_FROM[0] - 0.3 && m.speed == 1.0),
        "not at full stick"
    );
    let t = run.status().landing.expect("a touchdown");
    assert!(t.bounced() && t.miss < 0.1 && t.speed_in > 4.0, "{t:?}");
    assert_eq!(t.result(4.0), "bounce");
    assert!(
        log.iter()
            .any(|l| *l == ("fly", "bounce") || *l == ("drop", "bounce")),
        "{log:?}"
    );
    // The flight left the floor falling, not rising.
    let f = *run.flights().last().expect("a flight");
    assert!(f.vz < 0.5 && f.speed > 1.5, "{f:?}");
}

#[test]
fn a_drop_that_does_not_launch_walks_back_up_for_another() {
    let (done, run) = play_drop(false);
    let log: Vec<_> = run.log().iter().map(|l| (l.step, l.what)).collect();
    assert!(log.contains(&("drop", "retry")), "{log:?}");
    assert!(
        log.iter().any(|l| l.0 == "drop" && l.1 == "no_bounce"),
        "{log:?}"
    );
    // The floor is not linked to the ledge here: the walk back fails.
    assert_eq!(done, Some(Err("no_path")));
}

#[test]
fn a_drop_a_run_off_lands_too_slow_for_runs_up_and_jumps_before_its_launch_point() {
    // From 2.4 a run-off lands at sqrt(2 g 2.4) = 4.46, under the 4.5 the
    // site needs: the drop jumps, at full stick.
    let s = jump_site();
    let how = drop_takeoff(JUMP_FROM, &s, RUN_OFF_SPEED);
    assert!(how.jump && how.stick == 1.0 && how.reaches(), "{how:?}");
    let ((_, run), seen) = play_drop_from(JUMP_FROM, s, true);
    let jumps: Vec<_> = seen.iter().filter(|(_, m)| m.jump).collect();
    assert!(!jumps.is_empty(), "no jump");
    // Pressed short of the launch point by the ground the body covers
    // before it acts, at full stick, moving at the target.
    let (at, m) = jumps[0];
    let lead = TAKEOFF_EARLY + RUN_SPEED * INPUT_LAG as f32 / 30.0;
    assert!(
        at[0] < JUMP_FROM[0] && at[0] > JUMP_FROM[0] - lead - 0.15 && at[2] == 2.4,
        "{at:?}"
    );
    assert!(m.dir[0] > 0.9 && m.speed == 1.0, "{m:?}");
    // The walk ends at the launch point; the drop backs off to the run-up.
    let arrived = seen
        .iter()
        .position(|(a, _)| (a[0] - JUMP_FROM[0]).abs() < 0.3)
        .unwrap();
    assert!(
        seen[arrived..]
            .iter()
            .any(|(a, _)| a[0] < JUMP_FROM[0] - RUN_UP + 0.2),
        "no run-up"
    );
    // The body left the floor rising, on the ledge, before the edge.
    let lift = seen
        .windows(2)
        .find(|w| w[1].0[2] > w[0].0[2] + 0.01 && w[0].0[2] >= 2.4)
        .map(|w| w[0].0)
        .expect("a lift-off");
    assert!(lift[0] < 0.0, "{lift:?}");
    // In the air the stick brakes: a full-stick run carries 4 units in
    // the flight's 2 s, the slot is 2.3 out.
    let vx: Vec<f32> = seen
        .windows(2)
        .filter(|w| w[0].0[2] > 2.45)
        .map(|w| (w[1].0[0] - w[0].0[0]) * 30.0)
        .collect();
    assert!(vx[0] > 1.8 && *vx.last().unwrap() < 1.5, "{vx:?}");
    let t = run.status().landing.expect("a touchdown");
    assert!(t.bounced() && t.miss < 0.1 && t.speed_in > 4.5, "{t:?}");
    // The try's flight: from the press at the jump's speed, the word
    // read, the stick past the dead zone, and the landing that bounced.
    let f = *run.flights().last().expect("a flight");
    assert!(
        (f.takeoff[0] - at[0]).abs() < 0.01 && f.takeoff[2] == 2.4,
        "{f:?}"
    );
    assert!((f.vz - JUMP_VZ).abs() < 0.3 && f.speed > 1.5, "{f:?}");
    assert_eq!(f.word, Some(PRIMED_WORD));
    assert!(f.ticks > 30 && f.stick_max >= MIN_STICK, "{f:?}");
    assert_eq!((f.landing, f.end), (Some(t), Some("bounce")));
    let line = f.describe();
    assert!(
        line.contains("word -3") && line.ends_with(": bounce"),
        "{line}"
    );
}

#[test]
fn every_drop_of_a_try_that_does_not_launch_is_a_flight_that_lands() {
    let (_, run) = play_drop(false);
    let flights = run.flights();
    assert!(!flights.is_empty());
    for f in &flights {
        let t = f.landing.expect("a touchdown");
        assert_eq!(f.end, Some(t.result(4.0)), "{f:?}");
        assert!(!t.bounced(), "{t:?}");
    }
}

#[test]
fn the_takeoff_is_a_run_off_where_one_gets_there_fast_enough_else_a_jump() {
    let from = RUN_OFF_FROM;
    // A run off at full stick reaches the slot and lands fast enough.
    let how = drop_takeoff(from, &run_off_site(), RUN_OFF_SPEED);
    assert!(!how.jump && how.stick == 1.0 && how.reaches(), "{how:?}");
    assert!((how.speed_in - (2.0 * FALL_GRAVITY * 2.4f32).sqrt()).abs() < 1e-3);
    // Nearer than a full-stick run-off can brake to: the slower run.
    let near = Site {
        need: 4.0,
        ..drop_site(1.0, 12.3)
    };
    let how = drop_takeoff(from, &near, RUN_OFF_SPEED);
    assert!(
        !how.jump && how.stick == RUN_OFF_SPEED && how.reaches(),
        "{how:?}"
    );
    // The same slot needing more than a run-off lands at: a jump, and of
    // the jumps the slower run, whose flight brakes onto it.
    let how = drop_takeoff(from, &drop_site(1.0, 12.3), RUN_OFF_SPEED);
    assert!(how.jump && how.reaches(), "{how:?}");
    // Out of reach of everything: the nearest of those fast enough.
    let far = drop_site(9.0, 12.3);
    let how = drop_takeoff(from, &far, RUN_OFF_SPEED);
    assert!(how.jump && !how.reaches() && how.off() > 0.0, "{how:?}");
    assert!(how.fast_enough(far.need));
}

#[test]
fn the_air_stick_brakes_pushes_and_holds() {
    // Landing 1 past the target with 1 s left, moving at 2: the full stick
    // back (past AIR_PAST).
    let (dir, speed) = air_stick([2.0, 0.0], [-1.0, 0.0], 1.0, 1.0, AIR_DEAD);
    assert!(dir[0] < -0.99 && speed == 1.0, "{dir:?} {speed}");
    // A little past: a speed to end at under the one now, still forward.
    let (dir, speed) = air_stick([1.0, 0.0], [-0.1, 0.0], 1.0, 1.0, AIR_DEAD);
    assert!(
        dir[0] > 0.99 && speed * AIR_TOP < 1.0 && speed > 0.0,
        "{dir:?} {speed}"
    );
    // Short: a speed to end at over the one now.
    let (dir, speed) = air_stick([0.5, 0.0], [0.2, 0.0], 1.0, 1.0, AIR_DEAD);
    assert!(dir[0] > 0.99 && speed * AIR_TOP > 0.5, "{dir:?} {speed}");
    // On it: the speed held.
    let (dir, speed) = air_stick([0.6, 0.0], [0.01, 0.0], 1.0, 1.0, AIR_DEAD);
    assert!(
        dir[0] > 0.99 && (speed * AIR_TOP - 0.6).abs() < 1e-4,
        "{dir:?} {speed}"
    );
    // Short by more than the stick can make up in the time: all of it.
    let (_, speed) = air_stick([0.0, 0.0], [2.0, 0.0], 0.5, 1.0, AIR_DEAD);
    assert_eq!(speed, 1.0);
}

#[test]
fn the_air_reach_brakes_to_a_stop_and_pushes_toward_the_top() {
    // Slow enough to stop in the time: u^2 / 2a.
    let (near, far) = air_reach(1.0, 2.0);
    assert!((near - 1.0 / (2.0 * AIR_ACCEL)).abs() < 1e-4);
    // Pushed from 1 to the top, then held.
    let t1 = (AIR_TOP - 1.0) / AIR_ACCEL;
    let want = t1 + 0.5 * AIR_ACCEL * t1 * t1 + AIR_TOP * (2.0 - t1);
    assert!((far - want).abs() < 1e-4, "{far} {want}");
    // Faster than the top: even pushing it slows toward it.
    let (near, far) = air_reach(2.0, 1.0);
    assert!(far < 2.0 && near < far && near > 0.0, "{near} {far}");
}

#[test]
fn the_measured_ascension_spot_is_a_full_stick_running_jump_from_its_takeoff() {
    // docs/game/superbounce.md, "The forced crouch leaves state": a
    // running jump from (-8.4, 2.5, -0.76) heading (-0.8, 0.6), a 2.43
    // drop landing at -5.4. A standing jump lands at -3.
    let site = crate::spots::Site::measured(crate::glitches::SPOTS[0]);
    assert!(!site.standing());
    let from = [-8.4, 2.5, -0.76];
    assert_eq!(launch_from(&site, &|_| None), Some(from));
    assert_eq!(approach(&site, &|_| None), Some(from));
    // A run-off lands at (2 g 2.43)^0.5 = 4.49, under the 5.4 every traced
    // bounce came in at; the jump at (3.08^2 + 2 g 2.43)^0.5 = 5.44.
    let off = takeoff(from, &site, 1.0, false).unwrap();
    assert!(!off.fast_enough(site.need), "{off:?}");
    let how = drop_takeoff(from, &site, RUN_OFF_SPEED);
    assert!(how.jump && how.stick == 1.0 && how.reaches(), "{how:?}");
    assert!(
        (how.speed_in - crate::spots::MEASURED_NEED).abs() < 0.1,
        "{how:?}"
    );
    // The drill's working tries: 2 units/s off the floor, braked in the
    // air to land 2.5 out (ground-truth 7).
    assert!(
        (how.speed - 2.0).abs() < 0.1 && how.near < how.gap - 0.3,
        "{how:?}"
    );
    // It aims at the middle of the bounces' landings, which the takeoff
    // heads at along the recipe's (-0.8, 0.6), within 3 degrees.
    let aim = site.target();
    assert!(
        (-10.7..-10.2).contains(&aim[0]) && (3.93..4.04).contains(&aim[1]),
        "{aim:?}"
    );
    let (h, _) = norm([aim[0] - from[0], aim[1] - from[1]]);
    assert!(
        h[0] * -0.8 + h[1] * 0.6 > 3.0f32.to_radians().cos(),
        "{h:?}"
    );
    // A primed route by it is the walk to the takeoff and the drop.
    let k = Known {
        sites: vec![site],
        crouches: vec![],
    };
    let p = plan_site(
        [-6.0, 2.0, -0.76],
        true,
        site,
        &k,
        &|_, _| Some(3.0),
        &|_| None,
    )
    .expect("a route");
    let names: Vec<_> = p.steps.iter().map(|s| s.name()).collect();
    assert_eq!(names, ["walk", "drop", "fly"]);
    assert!(matches!(p.steps[1], Step::Drop { from: f, .. } if f == from));
}

#[test]
fn the_fall_time_follows_the_jump_and_the_ground_speed_the_measured_map() {
    // A standing jump comes back to its floor in 2 vz / g.
    let t = fall_time(0.0, JUMP_VZ).unwrap();
    assert!((t - 2.0 * JUMP_VZ / FALL_GRAVITY).abs() < 1e-4);
    assert!(fall_time(-10.0, 1.0).is_none());
    // The drill runs' points: nothing in the dead zone, a walk at 0.6 to
    // 0.85, a run at 1.
    assert_eq!(ground_speed(0.1), 0.0);
    assert!((ground_speed(0.6) - 1.05).abs() < 1e-4);
    assert!((ground_speed(0.85) - 1.1).abs() < 1e-4);
    assert_eq!(ground_speed(1.0), RUN_SPEED);
    assert!(ground_speed(0.7) < ground_speed(0.8) && ground_speed(0.9) < RUN_SPEED);
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
    assert_eq!(t.result(4.0), "no_bounce");
    assert_eq!(Touchdown { vz_out: 12.0, ..t }.result(4.0), "bounce");
    assert_eq!(
        Touchdown { word: Some(0), ..t }.result(4.0),
        "not_primed"
    );
    assert_eq!(Touchdown { miss: 0.2, ..t }.result(4.0), "missed");
    assert_eq!(
        Touchdown { speed_in: 3.0, ..t }.result(4.0),
        "too_slow"
    );
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
    assert_eq!(
        p.steps[2],
        Step::Walk {
            to: far.bounce.from(),
            crouch: false
        }
    );
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

// ---- spots off the nav mesh, and pairing a site with a crouch spot ----

/// A floor from x -10 to `edge`, y -5 to 5, at z 0: past `edge` is off
/// the mesh (a seam, an edge the scan stands on), but the collision's
/// floor goes on 1.2 further.
fn edge_map(edge: f32) -> MapData {
    let collision = [
        [-10.0, -5.0, 0.0],
        [edge + 1.2, -5.0, 0.0],
        [edge + 1.2, 5.0, 0.0],
        [-10.0, 5.0, 0.0],
    ];
    MapData {
        walls: crate::los::Walls::from_polygons([&collision[..]]),
        ..cliff_map(edge)
    }
}

/// The same floor with nothing past `edge`: a ledge, a sill.
fn cliff_map(edge: f32) -> MapData {
    let floor = Poly::new(vec![
        [-10.0, -5.0, 0.0],
        [edge, -5.0, 0.0],
        [edge, 5.0, 0.0],
        [-10.0, 5.0, 0.0],
    ]);
    MapData {
        nav: NavMesh::new(vec![floor], vec![]),
        ..MapData::default()
    }
}

/// Plays a one-step plan from `at` until it is over (or `ticks`).
fn play_steps(
    map: &MapData,
    steps: Vec<Step>,
    mut at: Vec3,
    ticks: u32,
) -> (Option<Result<(), &'static str>>, Vec3) {
    let plan = Plan {
        steps,
        why: Why::Asked,
        site: None,
        cost: None,
    };
    let mut run = OrderRun::new(plan, false, 0, 1);
    for tick in 0..ticks {
        let o = run.step(map, at, tick, None);
        if o.done.is_some() {
            return (o.done, at);
        }
        let step = 5.0 * o.motion.speed / 30.0;
        at[0] += o.motion.dir[0] * step;
        at[1] += o.motion.dir[1] * step;
    }
    (None, at)
}

#[test]
fn a_spot_just_off_the_mesh_is_walked_to_by_a_last_straight_stretch() {
    let map = edge_map(5.0);
    // 0.6 past the floor's edge, a little under it (a crease): the path's
    // end is pulled inside the edge, so it ends about 0.9 short.
    let goal = [5.6, 0.0, -0.1];
    assert!(path_length(&map, [0.0; 3], goal).is_some());
    for steps in [
        vec![Step::Walk {
            to: goal,
            crouch: false,
        }],
        vec![Step::Nav { to: goal }],
    ] {
        let (done, at) = play_steps(&map, steps, [0.0, 0.0, 0.0], 600);
        assert_eq!(done, Some(Ok(())), "ended at {at:?}");
        assert!(dist_xy(at, goal) <= GOAL, "{at:?}");
    }
    // The walk to a bounce's approach off the mesh: walked, then the
    // bounce step steers onto the stand itself.
    let s = site(6.4, 0.0, 12.3);
    let approach = s.bounce.from();
    assert!(approach[0] > 5.0, "off the mesh: {approach:?}");
    let (done, at) = play_steps(
        &map,
        vec![Step::Walk {
            to: approach,
            crouch: false,
        }],
        [0.0; 3],
        600,
    );
    assert_eq!(done, Some(Ok(())));
    assert!(dist_xy(at, approach) <= ARRIVED + 0.01, "{at:?}");
}

#[test]
fn a_spot_far_off_the_mesh_has_no_walk_and_is_not_planned() {
    let map = edge_map(5.0);
    // 2.5 past the edge: the mesh snaps to it, but no last stretch reaches.
    let goal = [7.5, 0.0, 0.0];
    assert!(
        map.nav.path([0.0; 3], goal).is_some(),
        "the mesh snaps within SNAP"
    );
    assert_eq!(path_length(&map, [0.0; 3], goal), None);
    let (done, _) = play_steps(
        &map,
        vec![Step::Walk {
            to: goal,
            crouch: false,
        }],
        [0.0; 3],
        60,
    );
    assert_eq!(done, Some(Err("goal_off_mesh")));
    // A site that far off pairs with no crouch spot, and plans no route.
    let s = site(8.3, 0.0, 12.3);
    let k = known(vec![s]);
    let walk = |a: Vec3, b: Vec3| path_length(&map, a, b);
    let p = pair_site(&s, &k, &walk, &|_| None, &|_| true).unwrap();
    assert_eq!((p.crouch, p.walk), (None, None));
    assert!(plan_site([0.0; 3], false, s, &k, &walk, &|_| None).is_none());
}

#[test]
fn a_last_stretch_into_a_wall_ends_a_walk_where_it_got_and_fails_a_nav() {
    let map = edge_map(5.0);
    let goal = [5.6, 0.0, 0.0];
    // A body that stops moving short of the goal past the edge.
    let play_blocked = |steps: Vec<Step>| {
        let plan = Plan {
            steps,
            why: Why::Asked,
            site: None,
            cost: None,
        };
        let mut run = OrderRun::new(plan, false, 0, 1);
        let mut at = [0.0f32, 0.0, 0.0];
        for tick in 0..900 {
            let o = run.step(&map, at, tick, None);
            if o.done.is_some() {
                return o.done;
            }
            let step = 5.0 * o.motion.speed / 30.0;
            at[0] = (at[0] + o.motion.dir[0] * step).min(4.9);
            at[1] += o.motion.dir[1] * step;
        }
        None
    };
    assert_eq!(
        play_blocked(vec![Step::Walk {
            to: goal,
            crouch: false
        }]),
        Some(Ok(()))
    );
    assert_eq!(
        play_blocked(vec![Step::Nav { to: goal }]),
        Some(Err("tail_stuck"))
    );
}

#[test]
fn a_site_pairs_with_the_crouch_spot_a_walk_reaches_its_approach_from() {
    let s = site(10.0, 0.0, 12.3);
    let near = Crouch::at([9.0, 1.0, 0.0]);
    let reachable = Crouch::at([4.0, 0.0, 0.0]);
    let far = Crouch::at([200.0, 0.0, 0.0]);
    let k = Known {
        sites: vec![s],
        crouches: vec![near, far, reachable],
    };
    // Nothing walks from the nearest crouch spot.
    let walk = |a: Vec3, b: Vec3| (dist_xy(a, near.stand) > 0.01).then(|| dist3(a, b));
    let p = pair_site(&s, &k, &walk, &|_| None, &|_| true).unwrap();
    assert_eq!(p.approach, s.bounce.from());
    assert_eq!(p.crouch, Some(reachable));
    assert!((p.walk.unwrap() - dist3(reachable.stand, s.bounce.from())).abs() < 1e-4);
    // Out of PAIR_REACH: not paired however it walks.
    let lone = Known {
        sites: vec![s],
        crouches: vec![far],
    };
    assert_eq!(
        pair_site(&s, &lone, &flat_walk, &|_| None, &|_| true)
            .unwrap()
            .crouch,
        None
    );
    // A drop with no launch floor, or a launch that goes nowhere: no route.
    assert_eq!(
        pair_site(&drop_site(10.0, 12.3), &k, &flat_walk, &|_| None, &|_| true),
        Err("no_approach")
    );
    assert_eq!(
        pair_site(&site(10.0, 0.0, 0.0), &k, &flat_walk, &|_| None, &|_| true),
        Err("no_approach")
    );
}

// ---- a lost walk: its check and leg, planned again; the prime kept ----

/// Plays `steps` with a body that never moves; the route's end and log.
fn play_stuck(
    steps: Vec<Step>,
    ticks: u32,
) -> (Option<Result<(), &'static str>>, Vec<&'static str>) {
    let map = flat_map();
    let plan = Plan {
        steps,
        why: Why::Asked,
        site: None,
        cost: None,
    };
    let mut run = OrderRun::new(plan, false, 0, 1);
    for tick in 0..ticks {
        let o = run.step(&map, [0.0; 3], tick, None);
        if o.done.is_some() {
            return (o.done, run.log().iter().map(|l| l.what).collect());
        }
    }
    (None, run.log().iter().map(|l| l.what).collect())
}

#[test]
fn a_lost_walk_is_planned_again_then_fails_naming_the_check_and_the_leg() {
    let to = [5.0, 0.0, 0.0];
    let c = Crouch::at(to);
    let s = site(6.0, 0.0, 12.3);
    let cases = [
        (
            vec![Step::Walk { to, crouch: false }, Step::Prime(c)],
            "path_lost_stuck_crouch",
        ),
        (
            vec![Step::Walk { to, crouch: false }, Step::Bounce(s)],
            "path_lost_stuck_approach",
        ),
        (vec![Step::Nav { to }], "path_lost_stuck_nav"),
    ];
    for (steps, why) in cases {
        let (done, log) = play_stuck(steps, 2000);
        assert_eq!(done, Some(Err(why)));
        let replans = log.iter().filter(|w| **w == "path_lost_stuck").count() as u32;
        assert_eq!(replans, WALK_REPLANS, "{log:?}");
        assert!(why.starts_with("path_lost"));
    }
    assert_eq!(
        lost_on("path_lost_fell", Leg::Approach),
        "path_lost_fell_approach"
    );
    assert_eq!(
        lost_on("path_lost_link", Leg::Crouch),
        "path_lost_link_crouch"
    );
    assert_eq!(lost_on("path_lost_ended", Leg::Nav), "path_lost_ended_nav");
}

#[test]
fn a_walk_below_its_path_is_lost_by_the_fall_check() {
    let map = ledge_map();
    let goal = [-1.0, 0.0, 2.4];
    let mut w = RouteWalk::new(&map, [-5.0, 0.0, 2.4], goal, false, 0).unwrap();
    // Knocked off the ledge, half a unit from the goal sideways.
    assert_eq!(
        w.step(&map, [-1.5, 0.0, 0.0], 1, ARRIVED, 1.0).err(),
        Some("path_lost_fell")
    );
}

#[test]
fn a_walk_ends_at_a_lip_without_stopping_for_headroom_past_it() {
    // A low ceiling (z 0.5) from x 5 on: the standing capsule does not fit
    // under it. The walk's goal is at its edge, as a crouch spot's start.
    let lip = [
        [5.0, -1.0, 0.5],
        [7.0, -1.0, 0.5],
        [7.0, 1.0, 0.5],
        [5.0, 1.0, 0.5],
    ];
    let map = MapData {
        walls: crate::los::Walls::from_polygons([&lip[..]]),
        ..flat_map()
    };
    let to = [5.0, 0.0, 0.0];
    let plan = Plan {
        steps: vec![Step::Walk { to, crouch: false }],
        why: Why::Asked,
        site: None,
        cost: None,
    };
    let mut run = OrderRun::new(plan, false, 0, 1);
    let mut at = [0.0f32, 0.0, 0.0];
    let (mut standing, mut most) = (0, 0);
    let mut done = None;
    for tick in 0..300 {
        let o = run.step(&map, at, tick, None);
        if o.done.is_some() {
            done = o.done;
            break;
        }
        assert!(!o.motion.jump, "tick {tick}");
        standing = if o.motion.speed == 0.0 {
            standing + 1
        } else {
            0
        };
        most = most.max(standing);
        let step = 5.0 * o.motion.speed / 30.0;
        at[0] += o.motion.dir[0] * step;
        at[1] += o.motion.dir[1] * step;
    }
    assert_eq!(done, Some(Ok(())), "at {at:?}");
    assert!(most < 5, "stood {most} ticks on the way");
}

#[test]
fn a_word_lost_while_settling_on_the_stand_primes_again_rather_than_jumping() {
    let map = flat_map();
    let c = Crouch::at([3.0, 0.0, 0.0]);
    let s = site(10.0, 0.0, 12.3);
    let plan = Plan {
        steps: vec![
            Step::Walk {
                to: c.stand,
                crouch: true,
            },
            Step::Prime(c),
            Step::Walk {
                to: s.bounce.from(),
                crouch: false,
            },
            Step::Bounce(s),
            Step::Fly { to: s.target() },
        ],
        why: Why::Asked,
        site: Some(s),
        cost: None,
    };
    let mut run = OrderRun::new(plan, false, 0, 1);
    let (mut at, mut tick) = ([0.0, 0.0, 0.0], 0);
    play(
        &mut run,
        &map,
        &mut at,
        &mut tick,
        Some(PRIMED_WORD),
        "bounce",
        900,
    );
    assert_eq!(run.status().what, "bounce");
    assert!(run.primed());
    // The body slept on the stand: the pair came back at 0.
    let o = run.step(&map, at, tick, Some(0));
    assert!(!o.motion.jump);
    assert!(!run.primed());
    assert_eq!(run.status().what, "walk_crouched");
    assert!(
        run.log()
            .iter()
            .any(|l| (l.step, l.what) == ("bounce", "reprime"))
    );
}

#[test]
fn a_spot_just_past_a_ledge_is_not_walked_off_to() {
    // The same 0.6 past the edge, with no floor there: off a sill.
    let map = cliff_map(5.0);
    let goal = [5.6, 0.0, 0.0];
    assert_eq!(path_length(&map, [0.0; 3], goal), None);
    let (done, at) = play_steps(
        &map,
        vec![Step::Walk {
            to: goal,
            crouch: false,
        }],
        [0.0; 3],
        60,
    );
    assert_eq!(done, Some(Err("goal_off_mesh")));
    assert!(at[0] <= 5.0, "{at:?}");
}

#[test]
fn a_goal_under_the_ramp_beside_it_is_walked_to_on_the_ramp() {
    // A ramp from x 0 (z 0) up to x 4 (z 2): a point at x 2 with the
    // crease's height (0), as a bounce's approach behind a crease keeps.
    let ramp = Poly::new(vec![
        [0.0, -5.0, 0.0],
        [4.0, -5.0, 2.0],
        [4.0, 5.0, 2.0],
        [0.0, 5.0, 0.0],
    ]);
    let flat = Poly::new(vec![
        [-10.0, -5.0, 0.0],
        [0.0, -5.0, 0.0],
        [0.0, 5.0, 0.0],
        [-10.0, 5.0, 0.0],
    ]);
    let (a, b) = ([0.0, -5.0, 0.0], [0.0, 5.0, 0.0]);
    let walk = |from, to| crate::nav::Edge {
        from,
        to,
        a,
        b,
        kind: crate::nav::Kind::Walk,
    };
    let map = MapData {
        nav: NavMesh::new(vec![flat, ramp], vec![walk(0, 1), walk(1, 0)]),
        ..MapData::default()
    };
    let goal = [1.2, 0.0, 0.0];
    assert!((crate::nav::floor_goal(&map, goal)[2] - 0.6).abs() < 1e-4);
    assert!(path_length(&map, [-3.0, 0.0, 0.0], goal).is_some());
    // The body walks up the ramp; its height follows the floor.
    let mut w = RouteWalk::new(&map, [-3.0, 0.0, 0.0], goal, false, 0).unwrap();
    let mut at = [-3.0f32, 0.0, 0.0];
    for tick in 0..300 {
        match w.step(&map, at, tick, ARRIVED, 1.0) {
            Ok(None) => return,
            Ok(Some(m)) => {
                at[0] += m.dir[0] * 5.0 * m.speed / 30.0;
                at[1] += m.dir[1] * 5.0 * m.speed / 30.0;
                at[2] = (at[0] * 0.5).max(0.0);
            }
            Err(e) => panic!("{e} at {at:?}"),
        }
    }
    panic!("never arrived: {at:?}");
}

// ---- learned knobs (docs/game/bounce-search.md) ----

/// The fields of two [`Flown`]s that differ, by name.
fn flown_diff(a: &Flown, b: &Flown) -> Vec<&'static str> {
    let mut d = Vec::new();
    let mut ne = |name, same: bool| {
        if !same {
            d.push(name);
        }
    };
    ne("how", a.how == b.how);
    ne("target", a.target == b.target);
    ne("yaw", a.yaw == b.yaw);
    ne("heading", a.heading == b.heading);
    ne("back", a.back == b.back);
    ne("stick", a.stick == b.stick);
    ne("early", a.early == b.early);
    ne("gain", a.gain == b.gain);
    ne("letgo", a.letgo == b.letgo);
    ne("drift", a.drift == b.drift);
    ne("still", a.still == b.still);
    d
}

#[test]
fn no_knobs_fly_as_the_route_does() {
    let tune = crate::tuning::BOUNCE_DEFAULT;
    for (from, s) in [(RUN_OFF_FROM, run_off_site()), (JUMP_FROM, jump_site())] {
        assert!(s.knobs.is_default());
        let f = flown(&s, from, &tune);
        // Today's expressions, field by field.
        let model = drop_takeoff(from, &s, tune.run_off_speed);
        let target = s.target();
        assert_eq!(f.jump(), model.jump);
        assert_eq!(f.how, if model.jump { How::Jump } else { How::Runoff });
        assert_eq!(f.stick, model.stick);
        assert_eq!(f.target, target);
        assert_eq!(f.target, s.own_target());
        assert_eq!(
            f.heading,
            norm([target[0] - from[0], target[1] - from[1]]).0
        );
        assert_eq!(f.yaw, 0.0);
        assert_eq!(f.back, RUN_UP);
        assert_eq!(f.early, TAKEOFF_EARLY);
        assert_eq!(f.gain, tune.air_gain);
        assert_eq!(f.letgo, AIR_DEAD);
        assert_eq!(f.drift, tune.drift);
        assert_eq!(f.still, tune.still_ticks);
        assert_eq!(f.run_up(from), [from[0] - RUN_UP, from[1], from[2]]);
    }
    // A standing site: a standing jump at its own target.
    let s = site(10.0, 0.0, 12.3);
    let f = flown(&s, s.bounce.from(), &tune);
    assert_eq!((f.how, f.target), (How::Stand, s.bounce.stand));
    assert_eq!((f.drift, f.still), (tune.drift, tune.still_ticks));
    // The launch point and the plan are the route's own.
    assert_eq!(
        launch_from(&run_off_site(), &|_| Some(RUN_OFF_FROM)),
        Some(RUN_OFF_FROM)
    );
}

#[test]
fn a_knob_moves_only_its_own_number() {
    let tune = crate::tuning::BOUNCE_DEFAULT;
    let s = run_off_site();
    let base = flown(&s, RUN_OFF_FROM, &tune);
    let one = |k: Knobs| {
        let with = Site { knobs: k, ..s };
        flown_diff(&base, &flown(&with, RUN_OFF_FROM, &tune))
    };
    let k = Knobs::default;
    assert_eq!(
        one(Knobs {
            yaw: Some(10.0),
            ..k()
        }),
        ["yaw", "heading"]
    );
    assert_eq!(
        one(Knobs {
            back: Some(1.5),
            ..k()
        }),
        ["back"]
    );
    assert_eq!(
        one(Knobs {
            run: Some(0.4),
            ..k()
        }),
        ["stick"]
    );
    assert_eq!(
        one(Knobs {
            early: Some(0.2),
            ..k()
        }),
        ["early"]
    );
    assert_eq!(
        one(Knobs {
            gain: Some(2.0),
            ..k()
        }),
        ["gain"]
    );
    assert_eq!(
        one(Knobs {
            letgo: Some(0.1),
            ..k()
        }),
        ["letgo"]
    );
    assert_eq!(
        one(Knobs {
            drift: Some(0.9),
            ..k()
        }),
        ["drift"]
    );
    assert_eq!(
        one(Knobs {
            still: Some(20),
            ..k()
        }),
        ["still"]
    );
    // The aim along the run's own line: the target moves, the heading and
    // the takeoff do not.
    assert_eq!(
        one(Knobs {
            aim: Some([1.53, 0.0]),
            ..k()
        }),
        ["target"]
    );
    // A running jump where the route runs off.
    let d = one(Knobs {
        how: Some(How::Jump),
        ..k()
    });
    assert_eq!(d.first(), Some(&"how"));
    assert!(d.iter().all(|n| *n == "how" || *n == "stick"), "{d:?}");
}

#[test]
fn learned_knobs_change_the_plan_aim_launch_back_run_and_how() {
    let s = run_off_site();
    // The aim: the target moves, the site's own target does not.
    let aimed = Site {
        knobs: Knobs {
            aim: Some([1.6, 0.1]),
            ..Knobs::default()
        },
        ..s
    };
    assert_eq!(aimed.target(), [1.6, 0.1, 0.0]);
    assert_eq!(aimed.own_target(), s.target());
    // The launch point: ahead of the spot's takeoff and the launch floor,
    // and the plan drops from it.
    let from = [-0.2, 0.3, 2.4];
    let launched = Site {
        knobs: Knobs {
            from: Some(from),
            ..Knobs::default()
        },
        ..s
    };
    assert_eq!(launch_from(&launched, &|_| Some(RUN_OFF_FROM)), Some(from));
    let walk = |a: Vec3, b: Vec3| Some(dist3(a, b));
    let plan = plan_site(
        [-3.0, 0.0, 2.4],
        true,
        launched,
        &known(vec![launched]),
        &walk,
        &|_| Some(RUN_OFF_FROM),
    )
    .expect("a plan");
    assert!(
        plan.steps
            .iter()
            .any(|st| matches!(st, Step::Drop { from: f, site } if *f == from && site.knobs == launched.knobs)),
        "{:?}",
        plan.steps
    );
    // `how`: a stand makes the drop site a standing jump, planned as one.
    let stand = Site {
        knobs: Knobs {
            how: Some(How::Stand),
            ..Knobs::default()
        },
        ..s
    };
    assert!(!s.standing() && stand.standing());
    let plan = plan_site(
        [-3.0, 0.0, 0.0],
        true,
        stand,
        &known(vec![stand]),
        &walk,
        &|_| Some(RUN_OFF_FROM),
    )
    .expect("a plan");
    assert!(
        plan.steps.iter().any(|st| matches!(st, Step::Bounce(_))),
        "{:?}",
        plan.steps
    );
    // `back`: after the walk to the launch point, the run-up starts 1.2
    // behind it.
    let backed = Site {
        knobs: Knobs {
            back: Some(1.2),
            ..Knobs::default()
        },
        ..s
    };
    let (_, seen) = play_drop_from(RUN_OFF_FROM, backed, true);
    let arrived = seen
        .iter()
        .position(|(a, _)| (a[0] - RUN_OFF_FROM[0]).abs() < 0.3)
        .expect("at the launch point");
    assert!(
        seen[arrived..]
            .iter()
            .any(|(a, _)| a[0] < RUN_OFF_FROM[0] - 1.2 + 0.2),
        "no long run-up"
    );
    // `run`: the run at its stick, not the takeoff's full one: it leaves
    // the floor at the walk's speed.
    let slow = Site {
        knobs: Knobs {
            run: Some(0.6),
            ..Knobs::default()
        },
        ..s
    };
    let ((_, run), _) = play_drop_from(RUN_OFF_FROM, slow, true);
    let f = *run.flights().first().expect("a flight");
    assert!(f.speed < 1.5, "{f:?}");
    let ((_, run), _) = play_drop_from(RUN_OFF_FROM, s, true);
    let f = *run.flights().first().expect("a flight");
    assert!(f.speed > 1.5, "{f:?}");
    // `how`: a running jump where the route runs off presses jump.
    let jumped = Site {
        knobs: Knobs {
            how: Some(How::Jump),
            ..Knobs::default()
        },
        ..s
    };
    let (_, seen) = play_drop_from(RUN_OFF_FROM, jumped, true);
    assert!(seen.iter().any(|(_, m)| m.jump), "no jump");
}

#[test]
fn a_training_loop_flies_the_boards_knobs_folds_its_tries_and_writes_the_file() {
    use crate::learn::{Board, SiteIn, Train, Try};
    let tune = crate::tuning::BOUNCE_DEFAULT;
    let s = run_off_site();
    let own = flown(&s, RUN_OFF_FROM, &tune);
    let other = if own.jump() { How::Runoff } else { How::Jump };
    let map = "train_fake_body";
    let site_in = SiteIn {
        at: s.bounce.stand,
        hows: vec![own.how, other],
        base: own.knobs(Some(RUN_OFF_FROM)),
        run_off: tune.run_off_speed,
        need: s.need,
        control: false,
    };
    let train = Train {
        shard: 0,
        shards: 1,
        budget: 60,
        seed: 1,
    };
    let mut board = Board::new(map, train, vec![site_in], &[]);
    let dir = std::env::temp_dir().join(format!("h2-train-fake-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut rounds = 0;
    while rounds < 4 {
        let Some(i) = board.pick(1, RUN_OFF_FROM) else {
            break;
        };
        let before = board.sites()[i].search.entry().tries;
        let mut done: Vec<Try> = Vec::new();
        while let Some(k) = board.next(i, &done) {
            // The try flies the board's knobs over the site's.
            let site = Site {
                knobs: k.or(s.knobs),
                ..s
            };
            let from = launch_from(&site, &|_| Some(RUN_OFF_FROM)).unwrap();
            let ((_, run), _) = play_drop_from(from, site, true);
            done.push(Try::from_leap(
                run.flights().first(),
                site.own_target(),
                false,
            ));
            assert!(done.len() < 64, "a round that never ends");
        }
        assert!(!done.is_empty());
        board.fold(i, &done).expect("a fold");
        rounds += 1;
        // Between rounds the board's lines go to the file.
        let (_, all) = crate::spots::write_learned(&dir, map, &board.entries()).unwrap();
        let back = crate::spots::read_learned(&dir, map).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].tries, all[0].tries);
        let scored = done.iter().filter(|t| t.scored()).count() as u32;
        assert_eq!(back[0].tries, before + scored, "{:?}", back[0]);
    }
    assert!(rounds >= 1);
    let line = &crate::spots::read_learned(&dir, map).unwrap()[0];
    // The route's own run-off bounces on this body: the arms round saw it.
    assert!(line.hits > 0, "{line:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- the approach to a crouch spot ----

/// A square floor polygon at z 0.
fn quad(x0: f32, y0: f32, x1: f32, y1: f32) -> Poly {
    Poly::new(vec![
        [x0, y0, 0.0],
        [x1, y0, 0.0],
        [x1, y1, 0.0],
        [x0, y1, 0.0],
    ])
}

/// Plays `steps` from `at` with a body that walks as asked, except that
/// it never crosses x 4 between y 0 and 4 (a rail the mesh lacks); the
/// route's end, where the body got to, and the log.
fn play_railed(
    map: &MapData,
    steps: Vec<Step>,
    mut at: Vec3,
    ticks: u32,
) -> (Option<Result<(), &'static str>>, Vec3, Vec<&'static str>) {
    let plan = Plan {
        steps,
        why: Why::Asked,
        site: None,
        cost: None,
    };
    let mut run = OrderRun::new(plan, false, 0, 1);
    for tick in 0..ticks {
        let o = run.step(map, at, tick, None);
        if o.done.is_some() {
            return (o.done, at, run.log().iter().map(|l| l.what).collect());
        }
        let step = 5.0 * o.motion.speed / 30.0;
        let to = [
            at[0] + o.motion.dir[0] * step,
            at[1] + o.motion.dir[1] * step,
        ];
        let crosses = (at[0] <= 4.0) != (to[0] <= 4.0);
        if !(crosses && (0.0..=4.0).contains(&to[1])) {
            at = [to[0], to[1], 0.0];
        }
    }
    (None, at, run.log().iter().map(|l| l.what).collect())
}

#[test]
fn a_walk_to_a_crouch_spot_goes_round_a_drop_it_cannot_make() {
    // A floor (A) with a drop link across a gap to the goal's (B), and the
    // long way round by C, D and E. Ascension's ramp: a drop link over a
    // rail the mesh lacks, which the bot pushed at for the rest of the
    // match, every try ending path_lost_jump_crouch.
    use crate::nav::{Edge, Kind};
    let polys = vec![
        quad(0.0, 0.0, 4.0, 4.0),  // A
        quad(6.0, 0.0, 10.0, 4.0), // B
        quad(0.0, 4.0, 4.0, 8.0),  // C
        quad(4.0, 6.0, 10.0, 8.0), // D
        quad(6.0, 4.0, 10.0, 6.0), // E
    ];
    let link = |from: u32, to: u32, a: [f32; 2], b: [f32; 2], kind| Edge {
        from,
        to,
        a: [a[0], a[1], 0.0],
        b: [b[0], b[1], 0.0],
        kind,
    };
    let both = |x: u32, y: u32, a, b| [link(x, y, a, b, Kind::Walk), link(y, x, a, b, Kind::Walk)];
    let mut edges = vec![link(0, 1, [4.0, 0.0], [4.0, 4.0], Kind::Drop)];
    edges.extend(both(0, 2, [0.0, 4.0], [4.0, 4.0]));
    edges.extend(both(2, 3, [4.0, 6.0], [4.0, 8.0]));
    edges.extend(both(3, 4, [6.0, 6.0], [10.0, 6.0]));
    edges.extend(both(4, 1, [6.0, 4.0], [10.0, 4.0]));
    let map = MapData {
        nav: NavMesh::new(polys, edges),
        ..MapData::default()
    };
    let to = [8.0, 2.0, 0.0];
    // The short way is the drop.
    let first = RouteWalk::new(&map, [1.0, 2.0, 0.0], to, false, 0).unwrap();
    assert!(
        first.follower().path.iter().any(|w| w.kind == Kind::Drop),
        "{:?}",
        first.follower().path
    );
    let steps = vec![
        Step::Walk { to, crouch: false },
        Step::Prime(Crouch::at(to)),
    ];
    let (done, at, log) = play_railed(&map, steps, [1.0, 2.0, 0.0], 3000);
    assert_eq!(done, Some(Ok(())), "at {at:?}: {log:?}");
    // Lost once at the drop, then planned round it.
    let lost: Vec<_> = log.iter().filter(|w| w.starts_with("path_lost")).collect();
    assert_eq!(lost, [&"path_lost_jump"], "{log:?}");
}

/// A low ceiling (z 0.6, under the standing capsule's 0.725) over x 3 to
/// 5 of a flat floor.
fn low_ceiling_map() -> MapData {
    let ceiling = [
        [3.0, -2.0, 0.6],
        [5.0, -2.0, 0.6],
        [5.0, 2.0, 0.6],
        [3.0, 2.0, 0.6],
    ];
    MapData {
        walls: crate::los::Walls::from_polygons([&ceiling[..]]),
        ..flat_map()
    }
}

/// Plays `steps` from the origin with a body that walks as asked; the
/// route's end and every motion of its first step (a walk).
fn play_walk(
    map: &MapData,
    steps: Vec<Step>,
    ticks: u32,
) -> (Option<Result<(), &'static str>>, Vec<RouteMotion>) {
    let plan = Plan {
        steps,
        why: Why::Asked,
        site: None,
        cost: None,
    };
    let mut run = OrderRun::new(plan, false, 0, 1);
    let mut at = [0.0f32; 3];
    let mut walk = Vec::new();
    for tick in 0..ticks {
        let o = run.step(map, at, tick, None);
        if o.done.is_some() {
            return (o.done, walk);
        }
        if run.status().step == 0 {
            walk.push(o.motion);
        }
        let step = 5.0 * o.motion.speed / 30.0;
        at[0] += o.motion.dir[0] * step;
        at[1] += o.motion.dir[1] * step;
    }
    (None, walk)
}

#[test]
fn a_walk_to_a_crouch_spot_ducks_under_a_low_ceiling_rather_than_stopping() {
    // Midship and Lockout: the walk to a crouch spot's start stood still
    // at a low ceiling (the headroom check), jumped, and was lost:
    // path_lost_stuck_crouch.
    let map = low_ceiling_map();
    let to = [8.0, 0.0, 0.0];
    let (done, walk) = play_walk(
        &map,
        vec![
            Step::Walk { to, crouch: false },
            Step::Prime(Crouch::at(to)),
        ],
        600,
    );
    assert_eq!(done, Some(Ok(())));
    assert!(walk.iter().any(|m| m.crouch), "crouched under the ceiling");
    assert!(
        !walk[0].crouch && !walk.last().unwrap().crouch,
        "stood up past it"
    );
    let still = walk.iter().filter(|m| m.speed == 0.0).count();
    assert_eq!(still, 0, "never stood still on the way");
    // A walk to a bounce's approach never crouches: past the prime a
    // crouch loses the word.
    let s = site(8.8, 0.0, 12.3);
    let (_, walk) = play_walk(
        &map,
        vec![Step::Walk { to, crouch: false }, Step::Bounce(s)],
        200,
    );
    assert!(!walk.is_empty() && walk.iter().all(|m| !m.crouch));
}

/// Zanzibar's window as the bots meet it: the crouch spot on the sill with
/// its way out back into the room, and the pane at x 5.30, a low piece
/// (index 3) in the walk in and the lip's piece (index 4) above it.
fn window() -> (MapData, Crouch) {
    let pane = |y0: f32, y1: f32, z0: f32, z1: f32, index: u8| crate::glass::Pane {
        index,
        verts: vec![[5.3, y0, z0], [5.3, y1, z0], [5.3, y1, z1], [5.3, y0, z1]],
    };
    let map = MapData {
        walls: crate::los::Walls::new(Vec::new()).with_glass(vec![
            pane(-26.6, -26.0, 5.7, 6.3, 4),
            pane(-26.6, -26.0, 5.1, 5.6, 3),
        ]),
        ..MapData::default()
    };
    let p = superbounce::proven("zanzibar")
        .into_iter()
        .find(|p| p.note.contains("break the glass"))
        .expect("the window row");
    let c = Crouch {
        stand: p.at,
        out: p.out,
        status: superbounce::Status::Proven,
        start: None,
    };
    (map, c)
}

fn whole(indices: &[u8]) -> crate::glass::Mask {
    let mut m = [0u32; 8];
    for &i in indices {
        m[usize::from(i >> 5)] |= 1 << (i & 31);
    }
    m
}

#[test]
fn the_window_primes_from_the_room_floor_not_from_the_air() {
    let (_, c) = window();
    // The room's floor is x 5.40 on; the sill x 5.20 to 5.40; past it the
    // ground is 3 units down.
    let s = prime_start(&c);
    assert!(s[0] > 5.4, "{s:?}");
    assert!((s[1] - c.stand[1]).abs() < 1e-4, "{s:?}");
}

#[test]
fn a_whole_pane_in_the_walk_in_is_shot_out_before_the_prime() {
    let (map, c) = window();
    let p = Plan {
        steps: vec![Step::Prime(c)],
        why: Why::Asked,
        site: None,
        cost: None,
    };
    let at = prime_start(&c);
    let mut run = OrderRun::new(p, false, 0, 1);
    // Only the low piece is in the way: the lip's stays.
    assert_eq!(glass_in_the_way(&map, &c, Some(&whole(&[3, 4]))).len(), 1);
    assert!(glass_in_the_way(&map, &c, Some(&whole(&[4]))).is_empty());
    assert!(glass_in_the_way(&map, &c, None).is_empty());
    run.see_glass(Some(whole(&[3, 4])));
    let o = run.step(&map, at, 1, None);
    assert!(o.done.is_none());
    assert_eq!(run.status().phase, "break");
    let aim = run.shooting().expect("shooting the low piece");
    assert!((aim[0] - 5.3).abs() < 1e-4 && aim[2] < 5.6, "{aim:?}");
    // Facing it, standing still, not crouched.
    let d = [aim[0] - at[0], aim[1] - at[1]];
    assert!((o.motion.yaw.unwrap() - d[1].atan2(d[0])).abs() < 1e-4);
    assert_eq!(o.motion.speed, 0.0);
    assert!(!o.motion.crouch);
    // The piece breaks: the prime starts, crouching first.
    run.see_glass(Some(whole(&[4])));
    run.step(&map, at, 2, None);
    assert!(run.shooting().is_none());
    let o = run.step(&map, at, 3, None);
    assert_ne!(run.status().phase, "break");
    assert!(o.motion.crouch, "{:?}", run.status());
    assert!(run.log().iter().any(|l| l.what == "glass_broken"));
}

#[test]
fn glass_that_never_breaks_fails_the_route_as_glass_whole() {
    let (map, c) = window();
    let p = Plan {
        steps: vec![Step::Prime(c)],
        why: Why::Asked,
        site: None,
        cost: None,
    };
    let at = prime_start(&c);
    let mut run = OrderRun::new(p, false, 0, 1);
    let mut done = None;
    for tick in 1..BREAK_TICKS + 10 {
        run.see_glass(Some(whole(&[3, 4])));
        let o = run.step(&map, at, tick, None);
        if o.done.is_some() {
            done = o.done;
            break;
        }
    }
    assert_eq!(done, Some(Err("glass_whole")));
}
