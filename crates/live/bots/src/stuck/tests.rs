use super::*;

const EAST: [f32; 2] = [1.0, 0.0];
const WALL: Vec3 = [5.0, 0.0, 0.0];
const GOAL: Vec3 = [20.0, 0.0, 0.0];

/// Steps a bot that stays at `at` from tick `from` to `to` (exclusive),
/// wanting to go east; the unsticks it got.
fn stay(s: &mut Stuck, at: Vec3, from: u32, to: u32) -> Vec<(u32, Unstick)> {
    (from..to)
        .map(|t| (t, s.step(t, at, Some(EAST), Some(WALL), Some(GOAL))))
        .collect()
}

#[test]
fn moving_freely_is_never_stuck() {
    let mut s = Stuck::new();
    for t in 0..600u32 {
        let at = [t as f32 * 0.05, 0.0, 0.0];
        let u = s.step(t, at, Some(EAST), Some(WALL), Some([40.0, 0.0, 0.0]));
        assert_eq!(u, Unstick::default(), "tick {t}");
    }
    assert_eq!(s.level, Level::Moving);
    assert_eq!(s.repaths, 0);
}

#[test]
fn standing_still_on_purpose_is_not_stuck() {
    let mut s = Stuck::new();
    for t in 0..600 {
        assert_eq!(s.step(t, [0.0; 3], None, None, None), Unstick::default());
    }
    assert_eq!(s.level, Level::Moving);
}

#[test]
fn it_escalates_one_step_a_window() {
    let mut s = Stuck::new();
    let at = [0.0; 3];
    let us = stay(&mut s, at, 0, WINDOW);
    assert!(us.iter().all(|(_, u)| *u == Unstick::default()));

    // Sidestep: a strafe, backing off a little; no buttons.
    let u = s.step(WINDOW, at, Some(EAST), Some(WALL), Some(GOAL));
    assert_eq!(s.level, Level::Sidestep);
    let d = u.dir.expect("a sidestep");
    assert!(d[1].abs() > 0.8, "sideways: {d:?}");
    assert!(d[0] < 0.0, "backs off: {d:?}");
    assert!(!u.jump && !u.crouch && !u.repath);
    stay(&mut s, at, WINDOW + 1, 2 * WINDOW);

    // Jump: pressed for the first ticks, then let go.
    let u = s.step(2 * WINDOW, at, Some(EAST), Some(WALL), Some(GOAL));
    assert_eq!(s.level, Level::Jump);
    assert!(u.jump && !u.crouch && u.dir.is_none());
    let later = stay(&mut s, at, 2 * WINDOW + 1, 3 * WINDOW);
    assert!(later.iter().filter(|(_, u)| u.jump).count() as u32 == JUMP_TICKS - 1);

    // Crouch-jump: jump, then crouch held.
    let u = s.step(3 * WINDOW, at, Some(EAST), Some(WALL), Some(GOAL));
    assert_eq!(s.level, Level::CrouchJump);
    assert!(u.jump && !u.crouch);
    let later = stay(&mut s, at, 3 * WINDOW + 1, 4 * WINDOW);
    assert!(later.last().unwrap().1.crouch);
    assert!(!later.last().unwrap().1.jump);

    // Repath: once, with the waypoint blacklisted.
    assert!(!s.blocked(WALL, 4 * WINDOW));
    let u = s.step(4 * WINDOW, at, Some(EAST), Some(WALL), Some(GOAL));
    assert!(u.repath);
    assert_eq!(s.repaths, 1);
    assert_eq!(s.level, Level::Moving);
    assert!(s.blocked(WALL, 4 * WINDOW));
    assert!(
        s.blocked([5.3, 0.2, 9.0], 4 * WINDOW),
        "near it, any height"
    );
    assert!(!s.blocked(GOAL, 4 * WINDOW), "not the goal");
}

#[test]
fn the_sidesteps_alternate() {
    let mut s = Stuck::new();
    let at = [0.0; 3];
    stay(&mut s, at, 0, WINDOW);
    let a = s
        .step(WINDOW, at, Some(EAST), Some(WALL), Some(GOAL))
        .dir
        .unwrap();
    // Through to the repath, and stuck again: the other side.
    stay(&mut s, at, WINDOW + 1, 4 * WINDOW + 1);
    assert_eq!(s.repaths, 1);
    let t = 4 * WINDOW + 1;
    stay(&mut s, at, t, t + WINDOW);
    let b = s
        .step(t + WINDOW, at, Some(EAST), Some(WALL), Some(GOAL))
        .dir
        .unwrap();
    assert!(a[1] * b[1] < 0.0, "{a:?} then {b:?}");
}

#[test]
fn moving_forward_is_recovery_and_a_relapse_carries_on() {
    let mut s = Stuck::new();
    stay(&mut s, [0.0; 3], 0, 2 * WINDOW + 1);
    assert_eq!(s.level, Level::Jump);
    // The jump got it over: forward progress.
    let t = 2 * WINDOW + 1;
    s.step(
        t,
        [PROGRESS + 0.05, 0.0, 0.0],
        Some(EAST),
        Some(WALL),
        Some(GOAL),
    );
    assert_eq!(s.level, Level::Moving);
    // Stuck again at once: it carries on from the jump.
    let at = [PROGRESS + 0.05, 0.0, 0.0];
    stay(&mut s, at, t + 1, t + WINDOW);
    s.step(t + WINDOW, at, Some(EAST), Some(WALL), Some(GOAL));
    assert_eq!(s.level, Level::CrouchJump);
}

#[test]
fn a_relapse_long_after_starts_over() {
    let mut s = Stuck::new();
    stay(&mut s, [0.0; 3], 0, 2 * WINDOW + 1);
    assert_eq!(s.level, Level::Jump);
    // Free, and walking well for a while.
    let mut t = 2 * WINDOW + 1;
    let mut x = 0.0;
    while t < 2 * WINDOW + 1 + RELAPSE_TICKS + 10 {
        x += 0.05;
        s.step(t, [x, 0.0, 0.0], Some(EAST), Some(GOAL), Some(GOAL));
        t += 1;
    }
    assert_eq!(s.level, Level::Moving);
    let at = [x, 0.0, 0.0];
    stay(&mut s, at, t, t + WINDOW);
    s.step(t + WINDOW, at, Some(EAST), Some(WALL), Some(GOAL));
    assert_eq!(s.level, Level::Sidestep);
}

#[test]
fn sidestepping_is_not_progress() {
    let mut s = Stuck::new();
    stay(&mut s, [0.0; 3], 0, WINDOW + 1);
    assert_eq!(s.level, Level::Sidestep);
    // It moves, but sideways and back: still stuck, so it jumps next.
    for k in 1..WINDOW {
        let at = [-0.02 * k as f32, 0.05 * k as f32, 0.0];
        s.step(WINDOW + k, at, Some(EAST), Some(WALL), Some(GOAL));
    }
    let at = [-0.4, 1.0, 0.0];
    s.step(2 * WINDOW, at, Some(EAST), Some(WALL), Some(GOAL));
    assert_eq!(s.level, Level::Jump);
}

#[test]
fn the_blacklist_expires() {
    let mut s = Stuck::new();
    s.blacklist([1.0, 1.0, 0.0], 1000);
    assert!(s.blocked([1.0, 1.0, 0.0], 1000));
    assert!(s.blocked([1.0, 1.0, 0.0], 1000 + BLACKLIST_TICKS - 1));
    assert!(!s.blocked([1.0, 1.0, 0.0], 1000 + BLACKLIST_TICKS));
    assert!(!s.blocked([1.0 + BLACKLIST_RADIUS, 1.0, 0.0], 1000));
    assert_eq!(s.blacklisted(1000).count(), 1);
    assert_eq!(s.blacklisted(1000 + BLACKLIST_TICKS).count(), 0);
    // Across the tick counter's wrap.
    let mut w = Stuck::new();
    w.blacklist([0.0; 3], u32::MAX - 10);
    assert!(w.blocked([0.0; 3], 5));
    assert!(!w.blocked([0.0; 3], BLACKLIST_TICKS));
}

#[test]
fn the_blacklist_is_bounded_and_blocks_segments() {
    let mut s = Stuck::new();
    for i in 0..(BLACKLIST_MAX as u32 + 4) {
        s.blacklist([10.0 * i as f32, 0.0, 0.0], 0);
    }
    assert_eq!(s.blacklisted(0).count(), BLACKLIST_MAX);
    assert!(!s.blocked([0.0; 3], 0), "the oldest went first");
    let mut s = Stuck::new();
    s.blacklist([5.0, 0.3, 0.0], 0);
    assert!(s.blocks_segment([5.0, -2.0, 0.0], [5.0, 2.0, 0.0], 0));
    assert!(!s.blocks_segment([8.0, -2.0, 0.0], [8.0, 2.0, 0.0], 0));
}

#[test]
fn penned_in_between_two_waypoints_gives_up_the_goal() {
    let mut s = Stuck::new();
    // It walks back and forth between two points 0.8 apart: always
    // "progress" for the window, never anywhere.
    let mut repath_at = None;
    for t in 0..(PEN_TICKS + 40) {
        let phase = (t / 16) % 2;
        let x = if phase == 0 {
            (t % 16) as f32 * 0.05
        } else {
            0.8 - (t % 16) as f32 * 0.05
        };
        let want = if phase == 0 { EAST } else { [-1.0, 0.0] };
        let u = s.step(
            t,
            [x, 0.0, 0.0],
            Some(want),
            Some([0.8, 0.0, 0.0]),
            Some(GOAL),
        );
        if u.repath && repath_at.is_none() {
            repath_at = Some(t);
        }
    }
    let t = repath_at.expect("it gave up");
    assert!(t >= PEN_TICKS && t <= PEN_TICKS + 2, "{t}");
    assert!(s.blocked(GOAL, t), "the goal is blacklisted");
    assert!(s.blocked([0.8, 0.0, 0.0], t), "and the waypoint");
}

#[test]
fn a_goal_that_gets_no_closer_is_given_up() {
    let mut s = Stuck::new();
    // Circling the goal at a distance: moving well, never closer.
    let mut gave_up = None;
    for t in 0..(GOAL_TICKS + 10) {
        let a = t as f32 * 0.02;
        let at = [GOAL[0] + 5.0 * a.cos(), 5.0 * a.sin(), 0.0];
        let want = [-a.sin(), a.cos()];
        if s.step(t, at, Some(want), None, Some(GOAL)).repath && gave_up.is_none() {
            gave_up = Some(t);
        }
    }
    assert_eq!(gave_up, Some(GOAL_TICKS));
    assert!(s.blocked(GOAL, GOAL_TICKS));
}

#[test]
fn bodies_in_the_way_are_walked_around() {
    let at = [0.0; 3];
    // Someone just ahead, a little to the left: go right.
    let d = around_bodies(at, EAST, &[[0.5, 0.1, 0.0]]);
    assert!(d[1] < -0.5 && d[0] > 0.0, "{d:?}");
    // A little to the right: go left.
    let d = around_bodies(at, EAST, &[[0.5, -0.1, 0.0]]);
    assert!(d[1] > 0.5, "{d:?}");
    // The speed is kept.
    let d = around_bodies(at, [0.5, 0.0], &[[0.5, 0.0, 0.0]]);
    assert!(((d[0] * d[0] + d[1] * d[1]).sqrt() - 0.5).abs() < 1e-5);
    // Behind, far, off the line, or on another floor: no change.
    for b in [
        [-0.5, 0.0, 0.0],
        [3.0, 0.0, 0.0],
        [0.5, 1.0, 0.0],
        [0.5, 0.0, 2.0],
    ] {
        assert_eq!(around_bodies(at, EAST, &[b]), EAST, "{b:?}");
    }
}
