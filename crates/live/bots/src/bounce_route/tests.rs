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
/// when it is within 0.1 of it, comes in faster than 4, and `bounces`.
/// The body moves at [`RUN_SPEED`] a unit of stick, on the ground and in
/// the air, and a jump on the ground leaves at [`JUMP_VZ`].
fn play_drop(bounces: bool) -> (Option<Result<(), &'static str>>, OrderRun) {
    play_drop_from([-0.4, 0.0, 2.4], bounces).0
}

/// [`play_drop`] from the launch point `from`, with the motions it asked.
fn play_drop_from(
    from: Vec3,
    bounces: bool,
) -> ((Option<Result<(), &'static str>>, OrderRun), Vec<(Vec3, RouteMotion)>) {
    let map = ledge_map();
    let s = drop_site(1.0, 12.3);
    let mut seen = Vec::new();
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
            return ((o.done, run), seen);
        }
        let m = o.motion;
        seen.push((at, m));
        let step = RUN_SPEED * m.speed / 30.0;
        at[0] += m.dir[0] * step;
        at[1] += m.dir[1] * step;
        let floor = if at[0] < 0.0 { 2.4 } else { 0.0 };
        if m.jump && at[2] <= floor && vz <= 0.0 {
            vz = JUMP_VZ;
        }
        if at[2] > floor || vz > 0.0 {
            vz -= FALL_GRAVITY / 30.0;
            at[2] += vz / 30.0;
            if at[2] <= floor {
                at[2] = floor;
                vz = if bounces && (at[0] - 1.0).abs() < 0.1 && vz < -4.0 { 12.0 } else { 0.0 };
            }
        }
    }
    ((None, run), seen)
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
fn a_drop_that_needs_more_than_a_walk_off_runs_up_and_jumps_at_its_launch_point() {
    // From 2.4 a walk-off lands at sqrt(2 g 2.4) = 4.46, under the 4.5 the
    // site needs: the drop jumps.
    let s = drop_site(1.0, 12.3);
    let from = [-0.4, 0.0, 2.4];
    assert!(drop_jumps(from, &s));
    let ((_, run), seen) = play_drop_from(from, true);
    let jumps: Vec<_> = seen.iter().filter(|(_, m)| m.jump).collect();
    assert!(!jumps.is_empty(), "no jump");
    // Pressed at the launch point, moving at the target, after a run-up
    // from behind it.
    let (at, m) = jumps[0];
    assert!((at[0] - from[0]).abs() < 0.15 && at[2] == 2.4, "{at:?}");
    assert!(m.dir[0] > 0.9 && m.speed >= MIN_STICK, "{m:?}");
    // The walk ends at the launch point; the drop backs off to the run-up.
    let arrived = seen.iter().position(|(a, _)| (a[0] - from[0]).abs() < 0.3).unwrap();
    assert!(seen[arrived..].iter().any(|(a, _)| a[0] < from[0] - RUN_UP + 0.2), "no run-up");
    let t = run.status().landing.expect("a touchdown");
    assert!(t.bounced() && t.miss < 0.1 && t.speed_in > 4.5, "{t:?}");
}

#[test]
fn the_measured_ascension_spot_is_the_recipes_running_jump_from_its_takeoff() {
    // docs/game/superbounce.md, "The forced crouch leaves state": a
    // running jump from (-8.4, 2.5, -0.76) heading (-0.8, 0.6), a 2.43
    // drop landing at -5.4. A standing jump lands at -3.
    let site = crate::spots::Site::measured(crate::glitches::SPOTS[0]);
    assert!(!site.standing());
    let takeoff = [-8.4, 2.5, -0.76];
    assert_eq!(launch_from(&site, &|_| None), Some(takeoff));
    assert_eq!(approach(&site, &|_| None), Some(takeoff));
    assert!(drop_jumps(takeoff, &site));
    // The jump lands at the need: (3.08^2 + 2 g 2.43)^0.5 = 5.44.
    let vz_in = (JUMP_VZ * JUMP_VZ + 2.0 * FALL_GRAVITY * 2.43).sqrt();
    assert!((vz_in - crate::spots::MEASURED_NEED).abs() < 0.1, "{vz_in}");
    // About 1.3 units/s over the ground; the player ran at about 1.6 and
    // eased off in the air.
    let v = takeoff_speed(takeoff, site.target());
    assert!((1.1..1.6).contains(&v), "{v}");
    // A primed route by it is the walk to the takeoff and the drop.
    let k = Known {
        sites: vec![site],
        crouches: vec![],
    };
    let p = plan_site([-6.0, 2.0, -0.76], true, site, &k, &|_, _| Some(3.0), &|_| None)
        .expect("a route");
    let names: Vec<_> = p.steps.iter().map(|s| s.name()).collect();
    assert_eq!(names, ["walk", "drop", "fly"]);
    assert!(matches!(p.steps[1], Step::Drop { from, .. } if from == takeoff));
}

#[test]
fn the_fall_time_and_takeoff_speed_follow_the_jump() {
    // A standing jump comes back to its floor in 2 vz / g.
    let t = fall_time(0.0, JUMP_VZ).unwrap();
    assert!((t - 2.0 * JUMP_VZ / FALL_GRAVITY).abs() < 1e-4);
    assert!(fall_time(-10.0, 1.0).is_none());
    // No gap, no speed; twice the gap, twice the speed.
    let a = takeoff_speed([0.0, 0.0, 2.0], [0.0, 0.0, 0.0]);
    let b = takeoff_speed([0.0, 0.0, 2.0], [1.0, 0.0, 0.0]);
    let c = takeoff_speed([0.0, 0.0, 2.0], [2.0, 0.0, 0.0]);
    assert!(a == 0.0 && (c - 2.0 * b).abs() < 1e-4);
    // A walk-off is enough when the floor is high over the strip.
    let s = drop_site(1.0, 12.3);
    assert!(!drop_jumps([0.0, 0.0, 4.0], &s));
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

// ---- spots off the nav mesh, and pairing a site with a crouch spot ----

/// A floor from x -10 to `edge`, y -5 to 5, at z 0: past `edge` is off
/// the mesh (a seam, an edge the scan stands on), but the collision's
/// floor goes on 1.2 further.
fn edge_map(edge: f32) -> MapData {
    let collision = [[-10.0, -5.0, 0.0], [edge + 1.2, -5.0, 0.0], [edge + 1.2, 5.0, 0.0], [-10.0, 5.0, 0.0]];
    MapData {
        walls: crate::los::Walls::from_polygons([&collision[..]]),
        ..cliff_map(edge)
    }
}

/// The same floor with nothing past `edge`: a ledge, a sill.
fn cliff_map(edge: f32) -> MapData {
    let floor = Poly::new(vec![[-10.0, -5.0, 0.0], [edge, -5.0, 0.0], [edge, 5.0, 0.0], [-10.0, 5.0, 0.0]]);
    MapData {
        nav: NavMesh::new(vec![floor], vec![]),
        ..MapData::default()
    }
}

/// Plays a one-step plan from `at` until it is over (or `ticks`).
fn play_steps(map: &MapData, steps: Vec<Step>, mut at: Vec3, ticks: u32) -> (Option<Result<(), &'static str>>, Vec3) {
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
        vec![Step::Walk { to: goal, crouch: false }],
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
    let (done, at) = play_steps(&map, vec![Step::Walk { to: approach, crouch: false }], [0.0; 3], 600);
    assert_eq!(done, Some(Ok(())));
    assert!(dist_xy(at, approach) <= ARRIVED + 0.01, "{at:?}");
}

#[test]
fn a_spot_far_off_the_mesh_has_no_walk_and_is_not_planned() {
    let map = edge_map(5.0);
    // 2.5 past the edge: the mesh snaps to it, but no last stretch reaches.
    let goal = [7.5, 0.0, 0.0];
    assert!(map.nav.path([0.0; 3], goal).is_some(), "the mesh snaps within SNAP");
    assert_eq!(path_length(&map, [0.0; 3], goal), None);
    let (done, _) = play_steps(&map, vec![Step::Walk { to: goal, crouch: false }], [0.0; 3], 60);
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
    assert_eq!(play_blocked(vec![Step::Walk { to: goal, crouch: false }]), Some(Ok(())));
    assert_eq!(play_blocked(vec![Step::Nav { to: goal }]), Some(Err("tail_stuck")));
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
    assert_eq!(pair_site(&s, &lone, &flat_walk, &|_| None, &|_| true).unwrap().crouch, None);
    // A drop with no launch floor, or a launch that goes nowhere: no route.
    assert_eq!(pair_site(&drop_site(10.0, 12.3), &k, &flat_walk, &|_| None, &|_| true), Err("no_approach"));
    assert_eq!(pair_site(&site(10.0, 0.0, 0.0), &k, &flat_walk, &|_| None, &|_| true), Err("no_approach"));
}

// ---- a lost walk: its check and leg, planned again; the prime kept ----

/// Plays `steps` with a body that never moves; the route's end and log.
fn play_stuck(steps: Vec<Step>, ticks: u32) -> (Option<Result<(), &'static str>>, Vec<&'static str>) {
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
        (vec![Step::Walk { to, crouch: false }, Step::Prime(c)], "path_lost_stuck_crouch"),
        (vec![Step::Walk { to, crouch: false }, Step::Bounce(s)], "path_lost_stuck_approach"),
        (vec![Step::Nav { to }], "path_lost_stuck_nav"),
    ];
    for (steps, why) in cases {
        let (done, log) = play_stuck(steps, 2000);
        assert_eq!(done, Some(Err(why)));
        let replans = log.iter().filter(|w| **w == "path_lost_stuck").count() as u32;
        assert_eq!(replans, WALK_REPLANS, "{log:?}");
        assert!(why.starts_with("path_lost"));
    }
    assert_eq!(lost_on("path_lost_fell", Leg::Approach), "path_lost_fell_approach");
    assert_eq!(lost_on("path_lost_link", Leg::Crouch), "path_lost_link_crouch");
    assert_eq!(lost_on("path_lost_ended", Leg::Nav), "path_lost_ended_nav");
}

#[test]
fn a_walk_below_its_path_is_lost_by_the_fall_check() {
    let map = ledge_map();
    let goal = [-1.0, 0.0, 2.4];
    let mut w = RouteWalk::new(&map, [-5.0, 0.0, 2.4], goal, false, 0).unwrap();
    // Knocked off the ledge, half a unit from the goal sideways.
    assert_eq!(w.step(&map, [-1.5, 0.0, 0.0], 1, ARRIVED, 1.0).err(), Some("path_lost_fell"));
}

#[test]
fn a_walk_ends_at_a_lip_without_stopping_for_headroom_past_it() {
    // A low ceiling (z 0.5) from x 5 on: the standing capsule does not fit
    // under it. The walk's goal is at its edge, as a crouch spot's start.
    let lip = [[5.0, -1.0, 0.5], [7.0, -1.0, 0.5], [7.0, 1.0, 0.5], [5.0, 1.0, 0.5]];
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
        standing = if o.motion.speed == 0.0 { standing + 1 } else { 0 };
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
            Step::Walk { to: c.stand, crouch: true },
            Step::Prime(c),
            Step::Walk { to: s.bounce.from(), crouch: false },
            Step::Bounce(s),
            Step::Fly { to: s.target() },
        ],
        why: Why::Asked,
        site: Some(s),
        cost: None,
    };
    let mut run = OrderRun::new(plan, false, 0, 1);
    let (mut at, mut tick) = ([0.0, 0.0, 0.0], 0);
    play(&mut run, &map, &mut at, &mut tick, Some(PRIMED_WORD), "bounce", 900);
    assert_eq!(run.status().what, "bounce");
    assert!(run.primed());
    // The body slept on the stand: the pair came back at 0.
    let o = run.step(&map, at, tick, Some(0));
    assert!(!o.motion.jump);
    assert!(!run.primed());
    assert_eq!(run.status().what, "walk_crouched");
    assert!(run.log().iter().any(|l| (l.step, l.what) == ("bounce", "reprime")));
}

#[test]
fn a_spot_just_past_a_ledge_is_not_walked_off_to() {
    // The same 0.6 past the edge, with no floor there: off a sill.
    let map = cliff_map(5.0);
    let goal = [5.6, 0.0, 0.0];
    assert_eq!(path_length(&map, [0.0; 3], goal), None);
    let (done, at) = play_steps(&map, vec![Step::Walk { to: goal, crouch: false }], [0.0; 3], 60);
    assert_eq!(done, Some(Err("goal_off_mesh")));
    assert!(at[0] <= 5.0, "{at:?}");
}

#[test]
fn a_goal_under_the_ramp_beside_it_is_walked_to_on_the_ramp() {
    // A ramp from x 0 (z 0) up to x 4 (z 2): a point at x 2 with the
    // crease's height (0), as a bounce's approach behind a crease keeps.
    let ramp = Poly::new(vec![[0.0, -5.0, 0.0], [4.0, -5.0, 2.0], [4.0, 5.0, 2.0], [0.0, 5.0, 0.0]]);
    let flat = Poly::new(vec![[-10.0, -5.0, 0.0], [0.0, -5.0, 0.0], [0.0, 5.0, 0.0], [-10.0, 5.0, 0.0]]);
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
