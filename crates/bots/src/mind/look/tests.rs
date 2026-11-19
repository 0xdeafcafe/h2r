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
                0.0,
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
            0.0,
        );
        hand.track(
            Skill::Legendary,
            1,
            1,
            (0.4, 0.0),
            [ray(0.4), ray(0.4)],
            0.0,
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
fn temperament_aim_noise_changes_only_the_tracking_error() {
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
        let extra = behavior.aim_noise_extra();
        let mut calm = Hand::new(7);
        let mut upset = Hand::new(7);
        for tick in 0..120 {
            // No lag, lead or flick in this sightline: only tracking noise.
            let target = [ray(0.0); 2];
            let base = calm.track(skill, 1, tick, (0.0, 0.0), target, 0.0);
            let changed = upset.track(skill, 1, tick, (0.0, 0.0), target, extra);
            assert_eq!(changed, crate::human_input::tracking_noise(base, extra));
        }
    }
}

// ---- the aim net against the code aim (docs/game/bots.md §3.4.1) ----

/// One chase in the nominal stick model, with the view as late as the
/// skill's perception and motor delays make it in a game.
struct Chase {
    /// The true yaw error after each tick, radians.
    errors: Vec<f32>,
    sticks: Vec<(f32, f32)>,
}

fn chase(
    model: AimModel,
    skill: Skill,
    seed: u64,
    ticks: u32,
    target: impl Fn(u32) -> f32,
) -> Chase {
    let tuning = crate::tuning::Tuning {
        aim_model: Some(model),
        ..Default::default()
    };
    crate::tuning::with(Some(tuning), || {
        let limits = tuning.input_limits(skill);
        let late = (limits.perception_ticks + limits.motor_ticks) as usize;
        let mut hand = Hand::new(seed);
        let (mut yaw, mut pitch) = (0.0f32, 0.0f32);
        let mut stick = (0.0f32, 0.0f32);
        let mut views = Vec::new();
        let mut out = Chase {
            errors: Vec::new(),
            sticks: Vec::new(),
        };
        for tick in 0..ticks {
            let at = target(tick);
            views.push((yaw, pitch, at));
            let (seen_yaw, seen_pitch, seen_at) = views[views.len().saturating_sub(late + 1)];
            let to = ray(seen_at).map(|c| c * 8.0);
            let error = hand.track(skill, 1, tick, (seen_yaw, seen_pitch), [to; 2], 0.0);
            let next = hand.steer(skill, Some(error), 1);
            // Both models move the same thumb: in the stick's circle, at
            // most the skill's travel a tick.
            assert!(next.0.hypot(next.1) <= 1.000_001, "{model:?}");
            assert!(
                (next.0 - stick.0).hypot(next.1 - stick.1) <= Handling::of(skill).engage + 1e-6,
                "{model:?}"
            );
            stick = next;
            let (x, y) = turn_rate(stick);
            yaw -= x / TICKS;
            pitch += y / TICKS;
            out.errors.push(at - yaw);
            out.sticks.push(stick);
        }
        out
    })
}

/// The aim-motion numbers the sim reports, on one chase: how far past the
/// target the first approach went (a fraction of the flick), the tick it
/// settled inside 0.05 rad for six ticks, the mean stick travel a tick,
/// and the mean error once the flick is over.
#[derive(Debug)]
struct Motion {
    overshoot: f32,
    settle: f32,
    jitter: f32,
    tail: f32,
}

fn motion(c: &Chase) -> Motion {
    let first = c.errors[0];
    let past = c.errors[..60]
        .iter()
        .map(|e| -e * first.signum())
        .fold(0.0, f32::max);
    let settle = (0..c.errors.len())
        .find(|&i| i + 6 <= c.errors.len() && c.errors[i..i + 6].iter().all(|e| e.abs() < 0.05))
        .unwrap_or(c.errors.len());
    let travel: f32 = c.sticks.windows(2).map(|w| (w[1].0 - w[0].0).abs()).sum();
    let tail = &c.errors[60..];
    Motion {
        overshoot: past / first.abs(),
        settle: settle as f32,
        jitter: travel / (c.sticks.len() - 1) as f32,
        tail: tail.iter().map(|e| e.abs()).sum::<f32>() / tail.len() as f32,
    }
}

const SEEDS: u64 = 24;

fn mean(f: impl Fn(u64) -> f32) -> f32 {
    (0..SEEDS).map(f).sum::<f32>() / SEEDS as f32
}

fn flick_at(model: AimModel, skill: Skill, seed: u64) -> Chase {
    chase(model, skill, seed, 150, |_| 0.95)
}

#[test]
fn the_net_is_deterministic_and_leaves_the_code_aim_alone() {
    for model in [AimModel::Code, AimModel::Net] {
        let a = flick_at(model, Skill::Heroic, 9);
        let b = flick_at(model, Skill::Heroic, 9);
        assert_eq!(a.errors, b.errors, "{model:?}");
        assert_eq!(a.sticks, b.sticks, "{model:?}");
        let other = flick_at(model, Skill::Heroic, 10);
        assert_ne!(a.sticks, other.sticks, "{model:?}");
    }
    assert_ne!(
        flick_at(AimModel::Code, Skill::Heroic, 9).sticks,
        flick_at(AimModel::Net, Skill::Heroic, 9).sticks
    );
    // With the code model named, the hand is the one it always was: no
    // net state, and the same stick as a hand with no tuning at all
    // (unless H2_BOT_AIM_MODEL makes the net this process's default).
    if crate::tuning::aim_model() != AimModel::Code {
        return;
    }
    let code = crate::tuning::Tuning {
        aim_model: Some(AimModel::Code),
        ..Default::default()
    };
    let mut named = Hand::new(3);
    let mut plain = Hand::new(3);
    let mut yaw = 0.0;
    for tick in 0..60 {
        let to = [ray(0.8); 2];
        let a = crate::tuning::with(Some(code), || {
            let e = named.track(Skill::Normal, 1, tick, (yaw, 0.0), to, 0.0);
            named.steer(Skill::Normal, Some(e), 1)
        });
        let b = {
            let e = plain.track(Skill::Normal, 1, tick, (yaw, 0.0), to, 0.0);
            plain.steer(Skill::Normal, Some(e), 1)
        };
        assert_eq!(a, b);
        assert!(named.net.is_none() && named.turns == [[0.0; 2]; REMEMBERED]);
        yaw -= turn_rate(a).0 / TICKS;
    }
}

#[test]
fn with_no_error_the_net_rests_and_a_plain_look_is_the_codes() {
    let net = crate::tuning::Tuning {
        aim_model: Some(AimModel::Net),
        error: Some(0.0),
        ..Default::default()
    };
    crate::tuning::with(Some(net), || {
        // Dead on a still target with a steady hand: no stick at all. The
        // misread is the one error a zero spread keeps, so the thumb only
        // ever nudges.
        let mut hand = Hand::new(5);
        let mut moved = 0;
        for tick in 0..200 {
            let to = ray(0.3).map(|c| c * 8.0);
            let e = hand.track(Skill::Legendary, 1, tick, (0.3, 0.0), [to; 2], 0.0);
            let stick = hand.steer(Skill::Legendary, Some(e), 1);
            assert!(stick.0.hypot(stick.1) < 0.3, "{stick:?}");
            if stick != (0.0, 0.0) {
                moved += 1;
            }
        }
        assert!(moved < 60, "{moved}");
        // A look with no tracked target (travel, a glance) goes through
        // the code's curve under either model.
        let mut look = Hand::new(5);
        let stick = look.steer(Skill::Legendary, Some([3.13, 0.0]), 1);
        assert!((-0.26..-0.24).contains(&stick.0), "{stick:?}");
        // So does an error that is not the one `track` just gave.
        let to = ray(0.5).map(|c| c * 8.0);
        let mut a = Hand::new(6);
        let mut b = Hand::new(6);
        let e = a.track(Skill::Legendary, 1, 0, (0.0, 0.0), [to; 2], 0.0);
        b.track(Skill::Legendary, 1, 0, (0.0, 0.0), [to; 2], 0.0);
        let other = [e[0] + 0.25, e[1]];
        assert_eq!(
            b.steer(Skill::Legendary, Some(other), 1),
            Hand::new(6).steer(Skill::Legendary, Some(other), 1)
        );
        assert!(a.steer(Skill::Legendary, Some(e), 1).0 < 0.0);
    });
}

#[test]
fn the_net_turns_the_right_way_on_each_axis() {
    // A steady hand, so the other axis stays still.
    let net = crate::tuning::Tuning {
        aim_model: Some(AimModel::Net),
        error: Some(0.0),
        ..Default::default()
    };
    crate::tuning::with(Some(net), || {
        for (to, want) in [
            ([8.0, 4.0, 0.0], (-1.0, 0.0)),
            ([8.0, -4.0, 0.0], (1.0, 0.0)),
            ([8.0, 0.0, 4.0], (0.0, 1.0)),
            ([8.0, 0.0, -4.0], (0.0, -1.0)),
        ] {
            let mut hand = Hand::new(2);
            let mut stick = (0.0, 0.0);
            for tick in 0..5 {
                let e = hand.track(Skill::Heroic, 1, tick, (0.0, 0.0), [to; 2], 0.0);
                stick = hand.steer(Skill::Heroic, Some(e), 1);
            }
            // Left of the aim is a negative stick x, above it a positive y,
            // as the code aim has it.
            assert!(
                stick.0 * want.0 + stick.1 * want.1 > 0.8,
                "{to:?}: {stick:?}"
            );
            assert!((stick.0 * want.1).abs() + (stick.1 * want.0).abs() < 0.3);
            let mut code = Hand::new(2);
            let by_code = code.steer(
                Skill::Heroic,
                Some([to[1].atan2(to[0]), to[2].atan2(8.0)]),
                1,
            );
            assert!(by_code.0 * want.0 + by_code.1 * want.1 > 0.0, "{by_code:?}");
        }
    });
}

#[test]
fn the_net_flick_lands_settles_and_improves_with_skill() {
    let mut tails = Vec::new();
    let mut settles = Vec::new();
    for skill in Skill::ALL {
        for seed in 0..SEEDS {
            let c = flick_at(AimModel::Net, skill, seed);
            assert!((0.93..0.951).contains(&c.errors[0]), "{}", c.errors[0]);
            assert!(c.sticks[0].0 < 0.0);
            // It reaches the target within two seconds and does not swing
            // far past it.
            let nearest = c.errors[..60].iter().map(|e| e.abs()).fold(1.0, f32::min);
            assert!(nearest < 0.05, "{skill:?} {seed}: {nearest}");
            let m = motion(&c);
            assert!(m.overshoot < 0.45, "{skill:?} {seed}: {m:?}");
        }
        tails.push(mean(|s| motion(&flick_at(AimModel::Net, skill, s)).tail));
        settles.push(mean(|s| motion(&flick_at(AimModel::Net, skill, s)).settle));
    }
    // Easy to legendary: each holds the target closer than the one before,
    // and none takes longer to get there.
    assert!(tails.windows(2).all(|w| w[1] < 0.9 * w[0]), "{tails:?}");
    assert!(tails[0] < 0.15 && tails[3] < 0.035, "{tails:?}");
    assert!(settles.windows(2).all(|w| w[1] < w[0] + 2.0), "{settles:?}");
    assert!(settles[0] > settles[3] + 8.0, "{settles:?}");
    assert!(settles[3] < 20.0, "{settles:?}");
    let legendary = mean(|s| motion(&flick_at(AimModel::Net, Skill::Legendary, s)).overshoot);
    assert!(legendary < 0.1, "{legendary}");
}

#[test]
fn the_net_tracks_a_moving_target() {
    // A strafe: 0.3 rad/s one way, then the other, turning every 1.2 s.
    let strafe = |tick: u32| {
        let leg = tick / 36;
        let into = (tick % 36) as f32;
        let before = if leg % 2 == 0 { 0.0 } else { 36.0 };
        let now = if leg % 2 == 0 { into } else { -into };
        0.2 + 0.3 * (before + now) / TICKS
    };
    // A crossing: 0.8 rad/s the whole way.
    let cross = |tick: u32| 0.2 + 0.8 * tick as f32 / TICKS;
    let held = |skill, target: &dyn Fn(u32) -> f32| {
        mean(|s| {
            let c = chase(AimModel::Net, skill, s, 180, target);
            c.errors[30..].iter().map(|e| e.abs()).sum::<f32>() / 150.0
        })
    };
    assert!(held(Skill::Legendary, &strafe) < 0.05);
    assert!(held(Skill::Legendary, &cross) < 0.06);
    assert!(held(Skill::Heroic, &strafe) < 0.07);
    assert!(held(Skill::Heroic, &cross) < 0.07);
    for skill in Skill::ALL {
        assert!(held(skill, &strafe) < 0.2, "{skill:?}");
        assert!(held(skill, &cross) < 0.25, "{skill:?}");
    }
    assert!(held(Skill::Easy, &cross) > held(Skill::Legendary, &cross) * 1.5);
}

#[test]
fn both_models_run_the_same_chases() {
    for skill in Skill::ALL {
        let mut jitter = [0.0f32; 2];
        for (i, model) in [AimModel::Code, AimModel::Net].into_iter().enumerate() {
            for seed in 0..SEEDS {
                let c = flick_at(model, skill, seed);
                // The same start, the same first move, and both get there.
                assert!((0.93..0.951).contains(&c.errors[0]), "{model:?}");
                assert!(
                    c.sticks[0].0 < 0.0 && c.sticks[0].1.abs() < 0.1,
                    "{model:?}"
                );
                let nearest = c.errors[..60].iter().map(|e| e.abs()).fold(1.0, f32::min);
                assert!(nearest < 0.1, "{model:?} {skill:?} {seed}: {nearest}");
                let m = motion(&c);
                assert!(
                    m.overshoot.is_finite() && m.overshoot < 1.0,
                    "{model:?}: {m:?}"
                );
                assert!(m.jitter > 0.0 && m.jitter < 0.5, "{model:?}: {m:?}");
                assert!(m.tail < 0.5, "{model:?}: {m:?}");
                jitter[i] += m.jitter / SEEDS as f32;
            }
        }
        // The net's thumb is no busier than the code's.
        assert!(jitter[1] < 1.5 * jitter[0], "{skill:?}: {jitter:?}");
    }
}

#[test]
fn temperament_scales_the_nets_error_as_it_does_the_codes() {
    use crate::decisions::{Behavior, MoodView, Personality};

    let net = crate::tuning::Tuning {
        aim_model: Some(AimModel::Net),
        ..Default::default()
    };
    crate::tuning::with(Some(net), || {
        for skill in Skill::ALL {
            let behavior = Behavior {
                personality: Personality::for_skill(7, skill),
                mood: MoodView {
                    tilt: 1.0,
                    fear: 1.0,
                    ..MoodView::default()
                },
            };
            let extra = behavior.aim_noise_extra();
            let mut calm = Hand::new(7);
            let mut upset = Hand::new(7);
            for tick in 0..120 {
                let target = [ray(0.0); 2];
                let base = calm.track(skill, 1, tick, (0.0, 0.0), target, 0.0);
                let changed = upset.track(skill, 1, tick, (0.0, 0.0), target, extra);
                assert_eq!(changed, crate::human_input::tracking_noise(base, extra));
                assert_eq!(calm.trace().unwrap().error, [0.0, 0.0]);
                assert_eq!(calm.trace().unwrap().tick, tick);
            }
        }
    });
}
