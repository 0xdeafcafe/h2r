use super::*;
use crate::glitches::Bounce;
use crate::nav::{NavMesh, Poly};

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

fn site(x: f32, y: f32) -> Site {
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
        launch: 12.3,
        ..Site::default()
    }
}

fn known() -> Known {
    Known {
        sites: vec![site(10.0, 0.0)],
        crouches: vec![Crouch::at([3.0, 0.0, 0.0])],
    }
}

fn names(p: &Plan) -> Vec<&'static str> {
    p.steps.iter().map(Step::name).collect()
}

/// Moves the body as the motion asks, at the run speed (5 units a second).
fn walk(at: &mut Vec3, m: &RouteMotion) {
    let step = 5.0 * m.speed / 30.0;
    at[0] += m.dir[0] * step;
    at[1] += m.dir[1] * step;
}

/// Plays `g` from `at` until it ends or `ticks` pass; the motions it gave.
fn play(
    g: &mut Goto,
    map: &MapData,
    at: &mut Vec3,
    tick: &mut u32,
    pair: Option<u32>,
    ticks: u32,
) -> Vec<RouteMotion> {
    let k = known();
    let mut seen = Vec::new();
    for _ in 0..ticks {
        let o = g.step(map, &k, *at, *tick, pair);
        *tick += 1;
        if o.done.is_some() {
            break;
        }
        walk(at, &o.motion);
        seen.push(o.motion);
    }
    seen
}

// ---- the plan ----

#[test]
fn each_target_and_then_has_its_plan() {
    let map = flat_map();
    let k = known();
    let from = [0.0, 0.0, 0.0];
    let plan_of =
        |t: Target, then: Then, primed: bool| plan(&Ask::new(t, then), from, primed, &k, &map);

    let p = plan_of(Target::Point([5.0, 1.0, 0.0]), Then::Stop, false).unwrap();
    assert_eq!(
        p.steps,
        vec![Step::Nav {
            to: [5.0, 1.0, 0.0]
        }]
    );
    assert_eq!(p.why, Why::Plain);

    // A crouch spot with no way out known: walked into crouched.
    let c = Crouch::at([3.0, 0.0, 0.0]);
    let p = plan_of(Target::Crouch(c), Then::Stop, false).unwrap();
    assert_eq!(names(&p), ["walk_crouched"]);
    let p = plan_of(Target::Crouch(c), Then::CrouchRecipe, false).unwrap();
    assert_eq!(names(&p), ["walk_crouched", "prime"]);
    assert_eq!(p.why, Why::Asked);

    // A bounce site: its approach, behind it along the drift.
    let s = site(10.0, 0.0);
    let p = plan_of(Target::Site(s), Then::Stop, false).unwrap();
    assert_eq!(
        p.steps,
        vec![Step::Walk {
            to: s.bounce.from(),
            crouch: false
        }]
    );
    // Its whole route: the prime, the bounce, the ride down; no walk after.
    let p = plan_of(Target::Site(s), Then::Bounce, false).unwrap();
    assert_eq!(
        names(&p),
        ["walk_crouched", "prime", "walk", "bounce", "fly"]
    );
    assert_eq!(p.site, Some(s));
    // The recipe alone stops at the approach.
    let p = plan_of(Target::Site(s), Then::CrouchRecipe, false).unwrap();
    assert_eq!(names(&p), ["walk_crouched", "prime", "walk"]);
    // Primed already: straight to the bounce.
    let p = plan_of(Target::Site(s), Then::Bounce, true).unwrap();
    assert_eq!(names(&p), ["walk", "bounce", "fly"]);
}

#[test]
fn a_then_that_does_not_fit_the_target_is_refused() {
    let map = flat_map();
    let k = known();
    let at = [0.0; 3];
    let refused = |t: Target, then: Then| plan(&Ask::new(t, then), at, false, &k, &map).is_err();
    assert!(refused(Target::Point([1.0, 0.0, 0.0]), Then::CrouchRecipe));
    assert!(refused(Target::Point([1.0, 0.0, 0.0]), Then::Bounce));
    assert!(refused(
        Target::Crouch(Crouch::at([3.0, 0.0, 0.0])),
        Then::Bounce
    ));
    // A bounce with nowhere to prime.
    let none = Known {
        sites: vec![site(10.0, 0.0)],
        crouches: Vec::new(),
    };
    let p = plan(
        &Ask::new(Target::Site(site(10.0, 0.0)), Then::Bounce),
        at,
        false,
        &none,
        &map,
    );
    assert_eq!(p, Err("no_route"));
    // A drop with no launch floor on the flat.
    let mut d = site(10.0, 0.0);
    d.fall = 2.4;
    let p = plan(&Ask::new(Target::Site(d), Then::Stop), at, false, &k, &map);
    assert_eq!(p, Err("no_launch_floor"));
    for t in [Then::Stop, Then::CrouchRecipe, Then::Bounce] {
        assert_eq!(Then::parse(t.name()), Some(t));
    }
    assert_eq!(Then::parse("dance"), None);
}

// ---- the path ----

fn wp(x: f32, y: f32) -> Waypoint {
    Waypoint {
        at: [x, y, 0.0],
        kind: Kind::Walk,
    }
}

#[test]
fn the_distance_left_runs_along_the_waypoints() {
    let ahead = [wp(3.0, 0.0), wp(3.0, 4.0)];
    assert!((remaining([0.0; 3], &ahead) - 7.0).abs() < 1e-5);
    assert_eq!(remaining([1.0, 1.0, 0.0], &[]), 0.0);
}

#[test]
fn off_the_path_is_measured_from_the_leg_being_walked() {
    let ahead = [wp(10.0, 0.0)];
    // From the walk's start to the next waypoint.
    assert!((off_path([5.0, 2.0, 0.0], None, [0.0; 3], &ahead) - 2.0).abs() < 1e-5);
    // From the waypoint passed.
    let behind = Some(wp(10.0, -10.0));
    assert!((off_path([10.0, -5.0, 0.0], behind, [0.0; 3], &ahead)).abs() < 1e-5);
    // Past the leg's end it is the distance to the end.
    assert!((off_path([13.0, 4.0, 0.0], None, [0.0; 3], &ahead) - 5.0).abs() < 1e-5);
    // A jump has its own line; nothing ahead is no leg.
    let jump = [Waypoint {
        kind: Kind::Jump,
        ..wp(10.0, 0.0)
    }];
    assert_eq!(off_path([5.0, 9.0, 0.0], None, [0.0; 3], &jump), 0.0);
    assert_eq!(off_path([5.0, 9.0, 0.0], None, [0.0; 3], &[]), 0.0);
}

// ---- playing it ----

#[test]
fn a_point_is_walked_to_and_reached_within_the_asked_distance() {
    let map = flat_map();
    let to = [6.0, 2.0, 0.0];
    let mut ask = Ask::new(Target::Point(to), Then::Stop);
    ask.within = 0.1;
    let mut g = Goto::new(ask, 0, 1);
    let (mut at, mut tick) = ([0.0; 3], 0);
    let moved = play(&mut g, &map, &mut at, &mut tick, None, 900);
    assert!(!moved.is_empty());
    assert_eq!(g.ended(), Some(Ok(())));
    assert!(dist3(at, to) <= 0.1 + 1e-4, "{at:?}");
    assert_eq!(g.status(Some(at)).state, "done");
    assert_eq!(g.status(Some(at)).replans, 0);
    // Never crouched on a plain walk.
    assert!(moved.iter().all(|m| !m.crouch));
}

#[test]
fn the_status_gives_the_path_ahead_and_the_distance_along_it() {
    let map = flat_map();
    let to = [8.0, 0.0, 0.0];
    let mut g = Goto::new(Ask::new(Target::Point(to), Then::Stop), 0, 1);
    let (mut at, mut tick) = ([0.0; 3], 0);
    play(&mut g, &map, &mut at, &mut tick, None, 6);
    let s = g.status(Some(at));
    assert_eq!(s.state, "running");
    assert_eq!(s.steps, ["nav"]);
    assert_eq!(s.phase, "walking");
    assert_eq!(s.path.first(), Some(&at));
    assert!(s.path.last().is_some_and(|p| dist3(*p, to) < 0.05));
    let d = s.distance.unwrap();
    assert!((d - dist3(at, to)).abs() < 0.05, "{d}");
}

#[test]
fn a_body_pushed_off_its_corridor_plans_again_and_still_arrives() {
    let map = flat_map();
    let to = [20.0, 0.0, 0.0];
    let mut g = Goto::new(Ask::new(Target::Point(to), Then::Stop), 0, 1);
    let (mut at, mut tick) = ([0.0; 3], 0);
    play(&mut g, &map, &mut at, &mut tick, None, 20);
    assert_eq!(g.status(Some(at)).replans, 0);
    // Knocked sideways, well off the line it was walking.
    at[1] += 2.0 * STRAY;
    let k = known();
    let o = g.step(&map, &k, at, tick, None);
    tick += 1;
    assert!(o.done.is_none());
    let s = g.status(Some(at));
    assert_eq!((s.replans, s.replanned), (1, Some("strayed")));
    play(&mut g, &map, &mut at, &mut tick, None, 900);
    assert_eq!(g.ended(), Some(Ok(())));
}

#[test]
fn a_body_that_never_moves_plans_again_then_gives_up() {
    let map = flat_map();
    let k = known();
    let mut g = Goto::new(Ask::new(Target::Point([10.0, 0.0, 0.0]), Then::Stop), 0, 1);
    let at = [0.0; 3];
    let mut last_jump = false;
    let mut end = None;
    for tick in 0..3_000 {
        let o = g.step(&map, &k, at, tick, None);
        if o.done.is_some() {
            end = o.done;
            break;
        }
        // A jump is a fresh press, never held.
        assert!(!(last_jump && o.motion.jump), "tick {tick}");
        last_jump = o.motion.jump;
    }
    let s = g.status(Some(at));
    assert!(end.is_some_and(|e| e.is_err()), "{end:?}");
    assert_eq!(s.replans, REPLANS);
    assert!(s.replanned.is_some());
    assert_eq!(s.state, "failed");
}

#[test]
fn a_walk_that_only_creeps_is_stuck() {
    let map = flat_map();
    let k = known();
    let mut g = Goto::new(Ask::new(Target::Point([30.0, 0.0, 0.0]), Then::Stop), 0, 1);
    let mut at = [0.0f32; 3];
    // Nearer by a little every 25 ticks: enough for the follower, which
    // wants 0.02 in 30 ticks, never STUCK_GAIN in STUCK_TICKS.
    let mut why = None;
    for tick in 0..2_000 {
        let o = g.step(&map, &k, at, tick, None);
        assert!(o.done.is_none());
        if tick % 25 == 0 {
            at[0] += 0.025;
        }
        if let Some(w) = g.status(Some(at)).replanned {
            why = Some((w, tick));
            break;
        }
    }
    let (w, tick) = why.unwrap();
    assert_eq!(w, "stuck");
    assert!(tick > STUCK_TICKS && tick < STUCK_TICKS + 10, "{tick}");
}

#[test]
fn a_goto_gives_up_after_its_time() {
    let map = flat_map();
    let k = known();
    let mut ask = Ask::new(Target::Point([30.0, 0.0, 0.0]), Then::Stop);
    ask.timeout = 10;
    let mut g = Goto::new(ask, 100, 1);
    assert!(g.step(&map, &k, [0.0; 3], 105, None).done.is_none());
    assert_eq!(
        g.step(&map, &k, [0.0; 3], 111, None).done,
        Some(Err("timeout"))
    );
    // Over is over.
    assert_eq!(
        g.step(&map, &k, [0.0; 3], 112, None).done,
        Some(Err("timeout"))
    );
}

#[test]
fn a_bounce_spot_is_reached_by_way_of_the_prime() {
    let map = flat_map();
    let mut g = Goto::new(Ask::new(Target::Site(site(10.0, 0.0)), Then::Bounce), 0, 1);
    let (mut at, mut tick) = ([0.0; 3], 0);
    // To the crouch spot and through the prime, the word read at -3.
    for _ in 0..1_200 {
        let s = g.status(Some(at));
        if s.steps.get(s.step) == Some(&"bounce") {
            break;
        }
        // No word to read: the prime is taken as done once walked out.
        let o = g.step(&map, &known(), at, tick, None);
        tick += 1;
        assert!(o.done.is_none(), "{:?}", g.status(Some(at)));
        walk(&mut at, &o.motion);
    }
    let s = g.status(Some(at));
    assert_eq!(s.steps, ["walk_crouched", "prime", "walk", "bounce", "fly"]);
    assert_eq!(s.steps[s.step], "bounce");
    assert!(s.primed);
    assert!(dist_xy(at, site(10.0, 0.0).bounce.from()) < 0.5, "{at:?}");
}
