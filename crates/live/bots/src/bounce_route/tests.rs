use super::*;
use crate::glitches::Bounce;
use crate::glitches::prime::{CROUCH_TICKS, FORCE_TICKS, PRIMED_WORD};
use crate::nav::{NavMesh, Poly};

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
        crouches: vec![[3.0, 0.0, 0.0]],
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
    assert!(!p.steps.iter().any(|s| matches!(s, Step::Prime { .. })));
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
    assert_eq!(p.steps[1], Step::Prime { at: [3.0, 0.0, 0.0] });
    // From the prime to the bounce: no crouching walk.
    let prime = p.steps.iter().position(|s| matches!(s, Step::Prime { .. })).unwrap();
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
    assert!(p.steps.iter().any(|s| matches!(s, Step::Prime { .. })));
    // None in reach: the plain walk.
    let far = known(vec![site(500.0, 0.0, 20.0)]);
    assert_eq!(plan(&ask([20.0, 0.0, 0.0], false, Mode::Fun), &far, &flat_walk).why, Why::Plain);
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
    assert!(p.steps.iter().any(|s| matches!(s, Step::Prime { .. })));
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
