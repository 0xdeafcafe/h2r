use super::*;
use crate::los::Walls;
use crate::nav::NavMesh;

fn ray(yaw: f32) -> Vec3 {
    [yaw.cos(), yaw.sin(), 0.0]
}

#[test]
fn the_stick_curve_has_a_deadzone_and_a_radial_limit() {
    assert_eq!(turn_rate((0.1, 0.0)), (0.0, 0.0));
    let (_, mut prev) = turn_rate((0.0, DEADZONE));
    for i in 14..=100 {
        let stick = i as f32 / 100.0;
        let (_, rate) = turn_rate((0.0, stick));
        assert!(rate > prev);
        prev = rate;
    }
    assert_eq!(prev, TURN_RATE);
    let (x, y) = turn_rate((1.0, 1.0));
    assert!((x.hypot(y) - TURN_RATE).abs() < 1e-5);
    for fraction in [0.01, 0.1, 0.3, 0.6, 1.0] {
        let [x, y] = deflection([fraction, 0.0]);
        assert!((turn_rate((x, y)).0 / TURN_RATE - fraction).abs() < 1e-5);
    }
}

fn flick(skill: Skill, seed: u64) -> Vec<f32> {
    let target = 0.95;
    let mut hand = Hand::new(seed);
    let mut yaw = 0.0;
    let mut stick = (0.0f32, 0.0f32);
    let mut pitch = 0.0;
    (0..90)
        .map(|tick| {
            let error = hand.track(
                skill,
                1,
                tick,
                (yaw, pitch),
                [ray(target), ray(target)],
                1.0,
            );
            let next = hand.steer(skill, Some(error), 1);
            assert!(next.0.hypot(next.1) <= 1.000_001);
            assert!(
                (next.0 - stick.0).hypot(next.1 - stick.1) <= Handling::of(skill).engage + 1e-6
            );
            stick = next;
            let (x, y) = turn_rate(stick);
            yaw -= x / TICKS;
            pitch += y / TICKS;
            target - yaw
        })
        .collect()
}

#[test]
fn flick_error_falls_overshoots_then_settles_with_micro_jitter() {
    let curves: Vec<_> = (0..32).map(|seed| flick(Skill::Legendary, seed)).collect();
    assert_eq!(curves[0], flick(Skill::Legendary, 0));
    let mut final_legendary = 0.0;
    let mut final_easy = 0.0;
    for (seed, curve) in curves.iter().enumerate() {
        assert!((0.92..0.95).contains(&curve[0]), "{curve:?}");
        assert!(curve[8] < 0.6, "{curve:?}");
        assert!(curve[8..35].iter().any(|&e| e < -0.02), "{curve:?}");
        let settled = &curve[55..];
        assert!(settled.iter().any(|&e| e.abs() < 0.03), "{curve:?}");
        assert!(settled.windows(2).any(|w| w[0] != w[1]));
        final_legendary += settled.iter().map(|e| e.abs()).sum::<f32>();
        final_easy += flick(Skill::Easy, seed as u64)[55..]
            .iter()
            .map(|e| e.abs())
            .sum::<f32>();
    }
    assert!(final_legendary / (32.0 * 35.0) < 0.035, "{final_legendary}");
    assert!(
        final_easy > final_legendary * 2.0,
        "{final_easy}, {final_legendary}"
    );
}

#[test]
fn tracking_lags_faster_bearings_more_and_crosses_the_wrap() {
    let error = |before| {
        let mut hand = Hand::new(17);
        hand.track(
            Skill::Legendary,
            1,
            0,
            (0.4, 0.0),
            [ray(before), ray(before)],
            1.0,
        );
        hand.track(
            Skill::Legendary,
            1,
            1,
            (0.4, 0.0),
            [ray(0.4), ray(0.4)],
            1.0,
        )[0]
    };
    assert!(error(0.25) < error(0.38) - 0.14);
    let d = angle_error((3.13, 0.0), (-3.13, 0.0));
    assert!((0.01..0.04).contains(&d[0]), "{d:?}");
}

#[test]
fn an_about_face_takes_multiple_ticks_of_thumb_motion() {
    let mut hand = Hand::new(42);
    let mut yaw = 0.0;
    for tick in 0..10 {
        let e = angle_error((yaw, 0.0), (3.13, 0.0));
        let stick = hand.steer(Skill::Legendary, Some(e), 1);
        if tick == 0 {
            assert!((-0.26..-0.24).contains(&stick.0));
        }
        yaw -= turn_rate(stick).0 / TICKS;
    }
    assert!((0.5..1.0).contains(&yaw), "{yaw}");
}

#[test]
fn the_hand_checks_the_opening_as_it_passes_a_doorway() {
    let wall = [
        [-2.0, 0.5, 0.0],
        [0.25, 0.5, 0.0],
        [0.25, 0.5, 2.0],
        [-2.0, 0.5, 2.0],
    ];
    let map = MapData {
        nav: NavMesh::default(),
        walls: Walls::from_polygons([&wall[..]]),
    };
    let mut hand = Hand::new(1);
    assert_eq!(
        hand.travel([0.0, 0.0, 0.6], [1.0, 0.0], 0, Some(&map), None),
        (0.45, 0.0)
    );
    assert_eq!(
        hand.travel([0.0, 0.0, 0.6], [1.0, 0.0], 10, None, None),
        (0.0, 0.0)
    );
}

#[test]
fn a_muffled_shot_draws_a_delayed_bearing_and_a_distant_one_does_not() {
    let wall = [
        [-1.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
        [1.0, 1.0, 2.0],
        [-1.0, 1.0, 2.0],
    ];
    let map = MapData {
        nav: NavMesh::default(),
        walls: Walls::from_polygons([&wall[..]]),
    };
    let eye = [0.0, 0.0, 0.6];
    let mut hand = Hand::new(2);
    assert_eq!(
        hand.sound(
            eye,
            100,
            Some(&map),
            &[ShotSound {
                at: [0.0, 9.0, 0.6],
                tick: 100
            }]
        ),
        None
    );
    let shot = [ShotSound {
        at: [0.0, 4.0, 0.6],
        tick: 104,
    }];
    assert_eq!(hand.sound(eye, 104, Some(&map), &shot), None);
    for tick in 105..109 {
        assert_eq!(hand.sound(eye, tick, Some(&map), &[]), None);
    }
    let yaw = hand
        .sound(eye, 112, Some(&map), &[])
        .expect("it heard the closer report");
    assert!((1.24..1.9).contains(&yaw), "{yaw}");
    assert_eq!(hand.sound(eye, 140, Some(&map), &[]), None);
}

#[test]
fn temperament_aim_noise_stays_bounded_and_uses_the_same_thumb_limits() {
    use crate::decisions::{Behavior, MoodView, Personality};

    for skill in Skill::ALL {
        let behavior = Behavior {
            personality: Personality {
                caution: 1.0,
                ..Personality::for_skill(7, skill)
            },
            mood: MoodView {
                tilt: 1.0,
                fear: 1.0,
                ..MoodView::default()
            },
        };
        let scale = behavior.aim_noise();
        assert!((1.0..=1.10).contains(&scale));
        let mut calm = Hand::new(7);
        let mut upset = Hand::new(7);
        let mut previous = (0.0f32, 0.0f32);
        for tick in 0..120 {
            // No lag, lead or flick in this sightline: only tracking noise.
            let target = [ray(0.0); 2];
            let base = calm.track(skill, 1, tick, (0.0, 0.0), target, 1.0);
            let changed = upset.track(skill, 1, tick, (0.0, 0.0), target, scale);
            assert_eq!(changed, base.map(|e| e * scale));
            // Also exercise the full input cap under a large turn request.
            let error = [changed[0] + 2.0, changed[1] + 1.0];
            let stick = upset.steer(skill, Some(error), 1);
            assert!(stick.0.hypot(stick.1) <= 1.000_001);
            let slew = (stick.0 - previous.0).hypot(stick.1 - previous.1);
            assert!(slew <= Handling::of(skill).engage + 1e-6);
            let (yaw, pitch) = turn_rate(stick);
            assert!(yaw.hypot(pitch) <= TURN_RATE + 1e-6);
            previous = stick;
        }
    }
}
