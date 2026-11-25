use super::*;
use crate::los::Walls;
use crate::nav::NavMesh;

fn ray(yaw: f32) -> Vec3 {
    [yaw.cos(), yaw.sin(), 0.0]
}

#[test]
fn the_response_has_a_deadzone_a_slow_half_stick_and_a_slower_pitch() {
    assert_eq!(turn_rate((0.1, 0.1)), (0.0, 0.0));
    for (table, axis) in [(RESPONSE.yaw, 0), (RESPONSE.pitch, 1)] {
        let rate = |s: f32| {
            let stick = if axis == 0 { (s, 0.0) } else { (0.0, s) };
            let (x, y) = turn_rate(stick);
            if axis == 0 { x } else { y }
        };
        let mut prev = rate(DEADZONE);
        assert_eq!(prev, 0.0);
        for i in 14..=100 {
            let r = rate(i as f32 / 100.0);
            assert!(r > prev, "{axis} {i}");
            prev = r;
        }
        assert!((prev - table[table.len() - 1].1).abs() < 1e-5);
        // Odd: the other way is the same turn the other way.
        assert_eq!(rate(-0.7), -rate(0.7));
        // The inverse lands on the asked-for rate, past the quiet band.
        for wanted in [0.02, 0.1, 0.157, 0.5, 1.2, prev] {
            let rates = if axis == 0 {
                [wanted, 0.0]
            } else {
                [0.0, wanted]
            };
            let [x, y] = RESPONSE.stick(rates);
            assert!(
                (rate(if axis == 0 { x } else { y }) - wanted).abs() < 1e-4,
                "{axis} {wanted}"
            );
        }
        assert_eq!(RESPONSE.stick([0.5 * QUIET, -0.5 * QUIET]), [0.0, 0.0]);
    }
    // The measured half-stick point: about 9°/s in yaw, 5°/s in pitch,
    // and pitch slower than yaw at every deflection.
    assert!((turn_rate((0.5, 0.0)).0 - 9f32.to_radians()).abs() < 1e-4);
    assert!((turn_rate((0.0, 0.5)).1 - 5f32.to_radians()).abs() < 1e-4);
    for i in 14..=100 {
        let s = i as f32 / 100.0;
        assert!(turn_rate((0.0, s)).1 < turn_rate((s, 0.0)).0);
    }
    // Each axis keeps its own curve; the stick stays in its circle.
    let (x, y) = turn_rate((1.0, 1.0));
    assert!(x > y && x < TURN_RATE);
    let [x, y] = RESPONSE.stick([10.0, 10.0]);
    assert!((x.hypot(y) - 1.0).abs() < 1e-5);
}

/// The test plant: a scene a tick at a time (the eye, the aim point and
/// its velocity), the game turning by the response table, and the hand's
/// view as late as the skill's perception and motor delays. For each tick
/// it gives the true yaw and pitch error once that tick's turn is made,
/// against the scene as it stands then, the distance, and the stick.
struct Plant {
    errors: Vec<[f32; 2]>,
    distances: Vec<f32>,
    sticks: Vec<(f32, f32)>,
}

type Scene<'a> = &'a dyn Fn(u32) -> (Vec3, Vec3, Vec3);

/// One run of the code aim against `scene`, leading the aim point by the
/// hand's own delay when `lead`, with the skill's spread or `error`.
fn plant(
    skill: Skill,
    seed: u64,
    ticks: u32,
    scene: Scene<'_>,
    lead: bool,
    error: Option<f32>,
) -> Plant {
    let tuning = crate::tuning::Tuning {
        aim_model: Some(AimModel::Code),
        error,
        ..Default::default()
    };
    crate::tuning::with(Some(tuning), || {
        let late = latency(skill);
        let mut hand = Hand::new(seed);
        let (mut yaw, mut pitch) = (0.0f32, 0.0f32);
        let mut views = Vec::new();
        let mut out = Plant {
            errors: Vec::new(),
            distances: Vec::new(),
            sticks: Vec::new(),
        };
        for tick in 0..ticks {
            let (eye, at, vel) = scene(tick);
            views.push(((yaw, pitch), eye, at, vel));
            let (aim, eye, at, vel) = views[views.len().saturating_sub(late + 1)];
            let raw = sub(at, eye);
            let ahead = if lead {
                lead_time(skill, len(raw), 0.0)
            } else {
                0.0
            };
            let led = std::array::from_fn(|i| raw[i] + vel[i] * ahead);
            let error = hand.track(skill, 1, tick, aim, [raw, led], 0.0);
            let stick = hand.steer(skill, Some(error), 1);
            assert!(stick.0.hypot(stick.1) <= 1.000_001);
            let (x, y) = turn_rate(stick);
            yaw -= x / TICKS;
            pitch += y / TICKS;
            let (eye, at, _) = scene(tick + 1);
            let to = sub(at, eye);
            out.errors.push(angle_error((yaw, pitch), angles(to)));
            out.distances.push(len(to));
            out.sticks.push(stick);
        }
        out
    })
}

/// A still target 8 units away, 0.95 rad to the left.
fn flick_scene(_: u32) -> (Vec3, Vec3, Vec3) {
    ([0.0; 3], ray(0.95).map(|c| c * 8.0), [0.0; 3])
}

fn flick(skill: Skill, seed: u64) -> Vec<f32> {
    plant(skill, seed, 90, &flick_scene, true, None)
        .errors
        .iter()
        .map(|e| e[0])
        .collect()
}

#[test]
fn flick_error_falls_overshoots_a_little_then_settles_and_rests() {
    let curves: Vec<_> = (0..32).map(|seed| flick(Skill::Legendary, seed)).collect();
    assert_eq!(curves[0], flick(Skill::Legendary, 0));
    let mut final_legendary = 0.0;
    let mut final_easy = 0.0;
    let mut past = 0;
    for (seed, curve) in curves.iter().enumerate() {
        assert!((0.9..0.95).contains(&curve[0]), "{curve:?}");
        assert!(curve[8] < 0.6, "{curve:?}");
        past += usize::from(curve[8..35].iter().any(|&e| e < -0.02));
        // Within the body's half width at 8 units once settled.
        let settled = &curve[55..];
        assert!(settled.iter().any(|&e| e.abs() < 0.045), "{curve:?}");
        final_legendary += settled.iter().map(|e| e.abs()).sum::<f32>();
        final_easy += flick(Skill::Easy, seed as u64)[55..]
            .iter()
            .map(|e| e.abs())
            .sum::<f32>();
    }
    // Most flicks carry a little past before they come back.
    assert!(past >= 24, "{past} of 32");
    assert!(final_legendary / (32.0 * 35.0) < 0.035, "{final_legendary}");
    assert!(
        final_easy > final_legendary * 2.0,
        "{final_easy}, {final_legendary}"
    );
}

/// The share of ticks after the first second that the stick turns the
/// view not at all, over seeds.
fn resting(skill: Skill, scene: Scene<'_>) -> f32 {
    let (mut still, mut all) = (0, 0);
    for seed in 0..24 {
        let run = plant(skill, seed, 150, scene, true, None);
        for &stick in &run.sticks[30..] {
            still += usize::from(turn_rate(stick) == (0.0, 0.0));
            all += 1;
        }
    }
    still as f32 / all as f32
}

#[test]
fn on_a_still_target_the_thumb_rests_and_on_a_moving_one_it_tracks() {
    // Already on a still target 8 units ahead: a steady hand lets the
    // stick go, a shaky one keeps correcting. (A player's median turn
    // rate in a match is 0.)
    let ahead = |_: u32| ([0.0; 3], [8.0, 0.0, 0.0], [0.0; 3]);
    let still: Vec<f32> = Skill::ALL.iter().map(|&s| resting(s, &ahead)).collect();
    assert!(still[3] > 0.85 && still[2] > 0.7, "{still:?}");
    assert!(still[0] < still[3], "{still:?}");
    // Crossing at 2.25 units a second, 6 or 12 units out: no rest.
    for distance in [6.0, 12.0] {
        let across = move |tick: u32| {
            let y = -3.0 + 2.25 * tick as f32 / TICKS;
            ([0.0; 3], [distance, y, 0.0], [0.0, 2.25, 0.0])
        };
        for skill in Skill::ALL {
            let moving = resting(skill, &across);
            assert!(moving < 0.1, "{skill:?} at {distance}: {moving}");
        }
    }
}

#[test]
fn a_large_flick_takes_the_thumb_to_full_stick_at_once() {
    for skill in Skill::ALL {
        let mut halves = 0;
        for seed in 0..32 {
            let run = plant(skill, seed, 60, &flick_scene, true, None);
            let mut last = (0.0f32, 0.0f32);
            for &stick in &run.sticks {
                let travel = (stick.0 - last.0).hypot(stick.1 - last.1);
                assert!(travel <= FLICK_STEP + 1e-6, "{skill:?} {seed}: {travel}");
                last = stick;
            }
            let full = run.sticks.iter().position(|s| s.0.hypot(s.1) >= 0.99);
            assert!(full.is_some_and(|t| t <= 3), "{skill:?} {seed}: {full:?}");
            let first = run.errors[0][0];
            let half = run.errors.iter().position(|e| e[0] < 0.5 * first).unwrap();
            assert!(half <= 8, "{skill:?} {seed}: {half}");
            halves += half;
        }
        // Halfway in under a quarter of a second on average, Easy too.
        assert!(halves as f32 / 32.0 < 7.5, "{skill:?}: {halves}");
    }
}

#[test]
fn a_flick_on_a_late_view_does_not_swing_past() {
    for skill in Skill::ALL {
        for seed in 0..24 {
            // The skill's own wandering hand: no swing past by a quarter.
            let c = flick(skill, seed);
            let first = c[0];
            let past = c[..60]
                .iter()
                .map(|e| -e * first.signum())
                .fold(0.0, f32::max);
            assert!(past < 0.25 * first.abs(), "{skill:?} {seed}: {past} {c:?}");
            // A steady hand settles inside 0.05 rad by tick 45 and stays.
            let steady = plant(skill, seed, 120, &flick_scene, true, Some(0.0));
            let yaw: Vec<f32> = steady.errors.iter().map(|e| e[0]).collect();
            assert!(
                yaw[45..].iter().all(|e| e.abs() < 0.05),
                "{skill:?} {seed}: {yaw:?}"
            );
        }
    }
}

/// A target `height` above the eye 6 units ahead, the bot strafing at
/// 2.25 units a second, one way then the other every 1.2 s.
fn strafing_eye(height: f32) -> impl Fn(u32) -> (Vec3, Vec3, Vec3) {
    move |tick| {
        let leg = tick / 36;
        let into = (tick % 36) as f32;
        let side = if leg % 2 == 0 { into } else { 36.0 - into };
        (
            [0.0, 2.25 * side / TICKS, 0.0],
            [6.0, 0.0, height],
            [0.0; 3],
        )
    }
}

#[test]
fn pitch_tracks_as_well_as_yaw_on_the_retail_curve() {
    for skill in Skill::ALL {
        let mean_pitch = |height: f32| {
            let scene = strafing_eye(height);
            (0..24)
                .map(|seed| {
                    let run = plant(skill, seed, 180, &scene, true, None);
                    run.errors[30..].iter().map(|e| e[1].abs()).sum::<f32>() / 150.0
                })
                .sum::<f32>()
                / 24.0
        };
        let level = mean_pitch(0.0);
        let above = mean_pitch(1.5);
        assert!(above < 1.5 * level, "{skill:?}: {above} against {level}");
    }
}

#[test]
fn the_error_shrinks_with_range() {
    // Dead on a still target, `track` returns the hand's own error alone.
    let rms = |skill: Skill, distance: f32| {
        let mut hand = Hand::new(21);
        let sum: f32 = (0..300)
            .map(|tick| {
                let to = [distance, 0.0, 0.0];
                let e = hand.track(skill, 1, tick, (0.0, 0.0), [to, to], 0.0);
                e[0] * e[0] + e[1] * e[1]
            })
            .sum();
        (sum / 300.0).sqrt()
    };
    let code = crate::tuning::Tuning {
        aim_model: Some(AimModel::Code),
        ..Default::default()
    };
    crate::tuning::with(Some(code), || {
        for skill in Skill::ALL {
            let (near, far) = (rms(skill, SPREAD_AT), rms(skill, 20.0));
            assert!(near >= 2.49 * far, "{skill:?}: {near} {far}");
            // Closer than the spread's distance it is the whole spread.
            assert_eq!(rms(skill, 2.0), near);
        }
        for distance in [SPREAD_AT, 20.0] {
            assert!(rms(Skill::Easy, distance) > 2.0 * rms(Skill::Legendary, distance));
        }
    });
}

#[test]
fn a_strafing_target_is_led_for_the_hands_own_delay() {
    // Strafing across at 2.25 units a second, one way.
    let across = |distance: f32| {
        move |tick: u32| {
            let y = -3.0 + 2.25 * tick as f32 / TICKS;
            ([0.0; 3], [distance, y, 0.0], [0.0, 2.25, 0.0])
        }
    };
    // The mean miss along the strafe (units, positive behind it) and the
    // mean size of the miss, once tracking, over seeds.
    let miss = |skill: Skill, distance: f32, lead: bool| {
        let scene = across(distance);
        let (mut behind, mut size) = (0.0, 0.0);
        for seed in 0..24 {
            let run = plant(skill, seed, 240, &scene, lead, None);
            for (e, d) in run.errors[90..].iter().zip(&run.distances[90..]) {
                behind += e[0] * d;
                size += e[0].abs() * d;
            }
        }
        let n = 24.0 * 150.0;
        (behind / n, size / n)
    };
    for distance in [6.0, 18.0] {
        for (skill, most) in [(Skill::Legendary, 0.25), (Skill::Normal, 0.5)] {
            let (behind, size) = miss(skill, distance, true);
            assert!(size < most, "{skill:?} at {distance}: {size}");
            assert!(behind.abs() < 0.12, "{skill:?} at {distance}: {behind}");
        }
    }
    // Without the lead for its own delay it trails the target.
    for skill in [Skill::Normal, Skill::Heroic, Skill::Legendary] {
        let (led, _) = miss(skill, 18.0, true);
        let (trailing, _) = miss(skill, 18.0, false);
        assert!(
            trailing > 0.15 && trailing > led.abs() + 0.1,
            "{skill:?}: {trailing} {led}"
        );
    }
}

#[test]
fn tracking_aims_where_it_sees_with_no_lag_and_crosses_the_wrap() {
    // A bearing that moved and one that did not: the same error once it is
    // there. The lead is the aim point's job (lead_time), not the hand's.
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
    assert_eq!(error(0.25), error(0.4));
    let d = angle_error((3.13, 0.0), (-3.13, 0.0));
    assert!((0.01..0.04).contains(&d[0]), "{d:?}");
}

#[test]
fn it_counts_the_turn_the_view_does_not_show_yet() {
    let code = crate::tuning::Tuning {
        aim_model: Some(AimModel::Code),
        ..Default::default()
    };
    crate::tuning::with(Some(code), || {
        let skill = Skill::Normal;
        let mut hand = Hand::new(4);
        let (yaw, pitch) = hand.ahead(skill, (0.3, 0.1));
        assert!((yaw - 0.3).abs() < 1e-6 && pitch == 0.1);
        let mut turned = 0.0;
        let mut sticks = Vec::new();
        for _ in 0..12 {
            let stick = hand.steer(skill, Some([0.5, 0.0]), 1);
            sticks.push(stick);
        }
        // The last `latency` reports' turn, by the response table.
        for stick in sticks.iter().rev().take(latency(skill)) {
            turned -= turn_rate(*stick).0 / TICKS;
        }
        let (yaw, pitch) = hand.ahead(skill, (0.0, 0.0));
        assert!(
            (yaw - turned).abs() < 1e-5 && turned > 0.1,
            "{yaw} {turned}"
        );
        assert_eq!(pitch, 0.0);
        // Gone with the hand.
        hand.release();
        let (yaw, pitch) = hand.ahead(skill, (0.0, 0.0));
        assert!(yaw.abs() < 1e-6 && pitch == 0.0);
    });
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
        places: Default::default(),
        nav: NavMesh::default(),
        walls: Walls::from_polygons([&wall[..]]),
    };
    let mut hand = Hand::new(1);
    assert_eq!(
        hand.travel([0.0, 0.0, 0.6], [1.0, 0.0], 0, Some(&map), None, None),
        (0.45, 0.0)
    );
    assert_eq!(
        hand.travel([0.0, 0.0, 0.6], [1.0, 0.0], 10, None, None, None),
        (0.0, 0.0)
    );
}

#[test]
fn on_a_ramp_the_look_pitches_toward_the_next_waypoint() {
    let mut hand = Hand::new(1);
    let eye = [0.0, 0.0, 0.6];
    // Level ground: level eyes.
    let (_, level) = hand.travel(eye, [1.0, 0.0], 0, None, None, Some([4.0, 0.0, 0.0]));
    assert!(level.abs() < 1e-6, "{level}");
    // Up a ramp: 1 up over 4 along, the slope's pitch.
    let (yaw, up) = hand.travel(eye, [1.0, 0.0], 0, None, None, Some([4.0, 0.0, 1.0]));
    assert!(yaw.abs() < 1e-6);
    assert!((up - 0.25f32.atan()).abs() < 1e-3, "{up}");
    // Down one, and a steep one capped.
    let (_, down) = hand.travel(eye, [1.0, 0.0], 0, None, None, Some([4.0, 0.0, -1.0]));
    assert!(down < -0.2, "{down}");
    let (_, steep) = hand.travel(eye, [1.0, 0.0], 0, None, None, Some([1.0, 0.0, 5.0]));
    assert_eq!(steep, SLOPE_PITCH);
    // Standing on the waypoint gives no slope.
    let (_, close) = hand.travel(eye, [1.0, 0.0], 0, None, None, Some([0.2, 0.0, 1.0]));
    assert_eq!(close, 0.0);
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
        places: Default::default(),
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
                tick: 100,
                shooter: None,
            }]
        ),
        None
    );
    let shot = [ShotSound {
        at: [0.0, 4.0, 0.6],
        tick: 104,
        shooter: None,
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

/// One chase with the view as late as the skill's perception and motor
/// delays make it in a game: the code aim on the response table, the net
/// on the nominal curve its weights were designed against.
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
        let game = match model {
            AimModel::Net => nominal_turn_rate,
            _ => turn_rate,
        };
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
            let step = match model {
                AimModel::Net => Handling::of(skill).engage,
                _ => Handling::of(skill).engage.max(FLICK_STEP),
            };
            assert!(
                (next.0 - stick.0).hypot(next.1 - stick.1) <= step + 1e-6,
                "{model:?}"
            );
            stick = next;
            let (x, y) = game(stick);
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
        assert!(named.net.is_none());
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
                    c.sticks[0].0 < 0.0 && c.sticks[0].1.abs() < 0.25,
                    "{model:?} {skill:?} {seed}: {:?}",
                    c.sticks[0]
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
        // The code's thumb, which rests on a still target, is no busier
        // than the net's, and the net's is not wild.
        assert!(jitter[0] < 1.25 * jitter[1], "{skill:?}: {jitter:?}");
        assert!(jitter[1] < 3.0 * jitter[0], "{skill:?}: {jitter:?}");
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

#[test]
fn a_twitchy_hand_flicks_faster_with_the_selected_skill_accuracy() {
    for skill in Skill::ALL {
        let steady = Handling::of(skill);
        let t = crate::tuning::Tuning {
            twitchy: true,
            ..Default::default()
        };
        let twitchy = crate::tuning::with(Some(t), || Handling::of(skill));
        assert!(twitchy.engage > steady.engage);
        assert!(twitchy.overshoot > steady.overshoot);
        assert_eq!(twitchy.misread, steady.misread);
        assert_eq!(t.aim(skill.base_aim()), skill.base_aim());
    }
}

// ---- a camouflaged target (docs/game/bots.md §3.1, "Visual range") ----

#[test]
fn a_camouflaged_target_is_tracked_with_more_error_at_every_skill() {
    for skill in Skill::ALL {
        let mut plain = Hand::new(11);
        let mut cloaked = Hand::new(11);
        let (mut a, mut b) = (0.0, 0.0);
        for tick in 0..240 {
            let target = [ray(0.0).map(|c| c * 6.0); 2];
            plain.obscure(skill, false);
            cloaked.obscure(skill, true);
            let e = plain.track(skill, 1, tick, (0.0, 0.0), target, 0.0);
            let f = cloaked.track(skill, 1, tick, (0.0, 0.0), target, 0.0);
            a += e[0].hypot(e[1]);
            b += f[0].hypot(f[1]);
        }
        assert!(b > 1.3 * a, "{skill:?}: {a} vs {b}");
    }
    // The better hand loses less to the shimmer.
    assert!(camo_aim(Skill::Legendary) < camo_aim(Skill::Easy));
}

#[test]
fn a_camouflaged_target_is_locked_onto_more_slowly() {
    let skill = Skill::Normal;
    let mut plain = Hand::new(3);
    let mut cloaked = Hand::new(3);
    plain.obscure(skill, false);
    cloaked.obscure(skill, true);
    let fast = plain.steer(skill, Some([0.5, 0.0]), 1);
    let slow = cloaked.steer(skill, Some([0.5, 0.0]), 1);
    assert!(slow.0.abs() < fast.0.abs(), "{slow:?} vs {fast:?}");
    assert!((slow.0.abs() - CAMO_ENGAGE * fast.0.abs()).abs() < 1e-4);
    // Plain again once the target shows.
    cloaked.obscure(skill, false);
    let mut again = Hand::new(3);
    again.obscure(skill, false);
    cloaked.release();
    assert_eq!(
        cloaked.steer(skill, Some([0.5, 0.0]), 1),
        again.steer(skill, Some([0.5, 0.0]), 1)
    );
}
