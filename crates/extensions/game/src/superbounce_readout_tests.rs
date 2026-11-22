//! The adapter from `superbounce_mode::State` to the readout's view, the
//! miss line's numbers and its time on screen (docs/game/hud-kit.md, "The
//! superbounce readout").

use std::f32::consts::{FRAC_PI_2, PI};

use hud_kit_core::huds::superbounce_readout::{
    FALL, INFO, SPEED, STEER, TAG, TARGET_1, TARGET_2, arrow,
};
use superbounce::{Kind, Vec3};

use super::*;
use crate::superbounce_mode::{Course, Predicted, ScanState};
use crate::superbounce_options::TargetMode;
use crate::superbounce_second::Ranked;
use crate::superbounce_targets::{Mark, Marks};

const M: f32 = METRES_PER_UNIT;

fn on() -> State {
    State {
        active: true,
        scan: ScanState::Ready,
        yaw: Some(0.0),
        ..State::default()
    }
}

fn seam() -> Spot {
    Spot {
        kind: Kind::Bounce,
        edge: [[10.0, -1.0, 2.0], [10.0, 1.0, 2.0]],
        stand: [10.0, 0.0, 2.0],
        fall: 2.43,
        need: 5.4,
        ..Spot::default()
    }
}

fn predicted(at: Vec3, speed: f32) -> Predicted {
    let s = seam();
    Predicted {
        at,
        verdict: Course::Fixable,
        speed,
        need: s.need,
        edge: s.edge,
        stand: s.stand,
        fall: s.fall,
    }
}

fn landing(result: LandingResult, tick: u32, at: Vec3) -> Landing {
    Landing { result, tick, at }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// The readout's arrows are the font's: the same first code point, the
/// same count, the same step for an angle.
#[test]
fn the_readouts_arrows_are_the_fonts() {
    use hud_kit_core::huds::superbounce_readout::{ARROW_UP, ARROWS};
    assert_eq!(ARROW_UP as u32, u32::from(font::arrows::FIRST));
    assert_eq!(ARROWS, u32::from(font::arrows::COUNT));
    for k in -16..=16 {
        let angle = k as f32 * PI / 8.0 + 0.01;
        assert_eq!(arrow(angle), font::arrows::glyph_for(angle), "{angle}");
    }
}

#[test]
fn a_direction_is_clockwise_from_the_aim() {
    // Aiming along +X: +X is ahead, -Y is to the right, +Y to the left.
    assert!(close(against_aim(1.0, 0.0, 0.0), 0.0));
    assert!(close(against_aim(0.0, -1.0, 0.0), FRAC_PI_2));
    assert!(close(against_aim(0.0, 1.0, 0.0), -FRAC_PI_2));
    assert!(close(against_aim(-1.0, 0.0, 0.0).abs(), PI));
    // Aiming along +Y: +Y is ahead, +X to the right.
    assert!(close(against_aim(0.0, 1.0, FRAC_PI_2), 0.0));
    assert!(close(against_aim(1.0, 0.0, FRAC_PI_2), FRAC_PI_2));
}

#[test]
fn the_speed_line_is_the_bodys_motion_against_the_aim() {
    let mut s = on();
    assert_eq!(speed(&s), None, "no velocity read");
    // Running to the right of the aim at 2 units a second.
    s.velocity = Some([0.0, -2.0, 0.0]);
    let got = speed(&s).unwrap();
    assert!(close(got.heading, FRAC_PI_2) && close(got.across, 2.0 * M));
    assert_eq!(got.down, None);
    // On the ground a downward speed is not a fall.
    s.velocity = Some([0.0, -2.0, -1.0]);
    assert_eq!(speed(&s).unwrap().down, None);
    // Falling: the fall speed beside it; rising: none.
    s.airborne = true;
    s.velocity = Some([1.0, 0.0, -4.0]);
    let got = speed(&s).unwrap();
    assert!(close(got.heading, 0.0) && close(got.across, M));
    assert!(close(got.down.unwrap(), 4.0 * M));
    s.velocity = Some([1.0, 0.0, 3.0]);
    assert_eq!(speed(&s).unwrap().down, None);
    // Standing still the line is hidden.
    s.airborne = false;
    s.velocity = Some([0.0, 0.0, 0.0]);
    let v = view(&s, Options::DEFAULT, None);
    assert!(!rd::values(&v).get(SPEED).unwrap().shown);
    s.velocity = Some([0.0, -2.0, 0.0]);
    let v = view(&s, Options::DEFAULT, None);
    let line = rd::values(&v).get(SPEED).cloned().unwrap();
    assert!(line.shown);
    assert_eq!(
        line.text,
        format!("{} {:.1} m/s", arrow(FRAC_PI_2), 2.0 * M)
    );
}

#[test]
fn the_fall_row_is_the_speed_over_the_need_and_the_way_to_the_seam() {
    let mut s = on();
    s.airborne = true;
    // Coasting down 0.5 short of the seam (which is ahead along +X) at
    // half the speed it needs.
    s.landing = Some(predicted([9.5, 0.0, 2.0], 2.7));
    let f = fall(&s).unwrap();
    assert!(close(f.speed, 0.5));
    let Steer::Off { heading, metres } = f.steer else {
        panic!("{f:?}");
    };
    assert!(close(heading, 0.0) && close(metres, 0.5 * M));
    // Aiming along +Y, the seam at +X is to the right.
    s.yaw = Some(FRAC_PI_2);
    let Some(Fall {
        steer: Steer::Off { heading, .. },
        ..
    }) = fall(&s)
    else {
        panic!();
    };
    assert!(close(heading, FRAC_PI_2));
    // Within the catch, fast enough: on the seam, the bar full.
    s.landing = Some(predicted([10.02, 0.3, 2.0], 6.0));
    let f = fall(&s).unwrap();
    assert_eq!(f.steer, Steer::OnSeam);
    assert!(f.speed > 1.0);
    let values = rd::values(&view(&s, Options::DEFAULT, None));
    assert_eq!(values.get(FALL).unwrap().fill, 1.0);
    assert_eq!(values.get(STEER).unwrap().text, rd::ON_SEAM);
    // On the ground the prediction is not shown.
    s.airborne = false;
    assert_eq!(fall(&s), None);
    s.landing = None;
    s.airborne = true;
    assert_eq!(fall(&s), None);
}

#[test]
fn a_piece_the_variant_turned_off_is_left_out() {
    let mut s = on();
    s.airborne = true;
    s.velocity = Some([1.0, 0.0, -4.0]);
    s.landing = Some(predicted([9.5, 0.0, 2.0], 2.7));
    s.marks = Marks {
        tag: Some(Mark {
            at: [40.0, 0.0, 0.0],
            distance: 10.0,
        }),
        targets: [
            Some(Mark {
                at: [10.0, 0.0, 2.0],
                distance: 3.0,
            }),
            None,
        ],
        tagged: Some([40.0, 0.0, 0.0]),
    };
    let info = Some(Miss::NotPrimed);
    let all = view(&s, Options::DEFAULT, info);
    assert!(all.speed.is_some() && all.fall.is_some() && all.info.is_some());
    assert!(close(all.tag.unwrap(), 10.0 * M));
    assert!(close(all.targets[0].unwrap(), 3.0 * M));
    assert_eq!(all.targets[1], None);
    let shown = |v: &View| {
        let values = rd::values(v);
        [SPEED, FALL, STEER, INFO, TAG, TARGET_1, TARGET_2].map(|id| values.get(id).unwrap().shown)
    };
    assert_eq!(shown(&all), [true, true, true, true, true, true, false]);
    let without = |o: Options| shown(&view(&s, o, info));
    let d = Options::DEFAULT;
    assert_eq!(
        without(Options {
            velocity: false,
            ..d
        }),
        [false, true, true, true, true, true, false]
    );
    assert_eq!(
        without(Options {
            alignment: false,
            ..d
        }),
        [true, false, false, true, true, true, false]
    );
    assert_eq!(
        without(Options {
            miss_info: false,
            ..d
        }),
        [true, true, true, false, true, true, false]
    );
    // The medals and the targets' mode are not the view's: the mode's
    // marks are already the option's.
    assert_eq!(
        without(Options {
            medals: false,
            targets: TargetMode::Common,
            ..d
        }),
        shown(&all)
    );
    // Off, everything is hidden.
    assert!(hidden().0.iter().all(|(_, v)| !v.shown));
    assert_eq!(hidden().0.len(), rd::hud().elements.len());
}

#[test]
fn the_history_answers_the_tick_before_touchdown() {
    let mut h = History::default();
    assert_eq!(h.before(10), None);
    for t in 0..10 {
        h.push(100 + t, -(t as f32));
    }
    assert_eq!(h.before(105), Some(-4.0));
    assert_eq!(h.before(100), None);
    assert_eq!(h.before(0), None);
    // Only the last ticks are kept.
    for t in 10..200 {
        h.push(100 + t, -1.0);
    }
    assert_eq!(h.before(105), None);
    assert_eq!(h.before(299), Some(-1.0));
    // A tick that goes back is a new game.
    h.push(5, -2.0);
    assert_eq!(h.before(299), None);
    assert_eq!(h.before(6), Some(-2.0));
}

#[test]
fn a_miss_carries_the_numbers_of_its_first_failed_test() {
    let spots = [seam()];
    let by_seam = [10.0, 0.0, 2.1];
    let at = |r| landing(r, 500, by_seam);
    assert_eq!(miss(&at(LandingResult::Bounce), &spots, Some(-9.0)), None);
    assert_eq!(
        miss(&at(LandingResult::NotPrimed), &spots, None),
        Some(Miss::NotPrimed)
    );
    // Missed: the origin's distance to the seam's edge, in metres.
    let off = landing(LandingResult::Missed, 500, [10.3, 0.0, 2.1]);
    let Some(Miss::Missed { by }) = miss(&off, &spots, None) else {
        panic!();
    };
    assert!(close(by, 0.3 * M), "{by}");
    // Too slow: the fall speed carried down to the seam, and its need.
    let Some(Miss::TooSlow { had, need }) = miss(&at(LandingResult::TooSlow), &spots, Some(-4.0))
    else {
        panic!();
    };
    let want = (16.0f32 + 2.0 * GRAVITY * 0.1).sqrt();
    assert!(close(had, want * M), "{had}");
    assert!(close(need, 5.4 * M));
    assert!(close(speed_in(-4.0, 2.1, 2.0), want));
    // A seam above the origin adds nothing.
    assert!(close(speed_in(-4.0, 1.9, 2.0), 4.0));
    let Some(Miss::NoBounce { had }) = miss(&at(LandingResult::NoBounce), &spots, Some(-6.0))
    else {
        panic!();
    };
    assert!(had > 6.0 * M);
    // Without the speed, or by no seam, there is no line to give.
    assert_eq!(miss(&at(LandingResult::TooSlow), &spots, None), None);
    assert_eq!(miss(&at(LandingResult::NoBounce), &spots, None), None);
    assert_eq!(miss(&off, &[], None), None);
    assert_eq!(
        miss(&at(LandingResult::NotPrimed), &[], None),
        Some(Miss::NotPrimed)
    );
}

#[test]
fn the_miss_line_stays_four_seconds_or_until_the_next_fall() {
    let line = |l: &Landing| match l.result {
        LandingResult::Bounce => None,
        _ => Some(Miss::Missed { by: 0.3 }),
    };
    let missed = Some(Miss::Missed { by: 0.3 });
    let mut s = on();
    s.last_landing = Some(landing(LandingResult::Missed, 50, [0.0; 3]));
    let mut info = Info::default();
    // The landing the first state held is not news.
    assert_eq!(info.step(&s, 1, line), None);
    assert_eq!(info.step(&s, 2, line), None);
    // A new one puts its line up for four seconds.
    s.last_landing = Some(landing(LandingResult::Missed, 90, [0.0; 3]));
    assert_eq!(info.step(&s, 3, line), missed);
    assert_eq!(info.step(&s, 3 + INFO_TICKS - 1, line), missed);
    assert_eq!(info.step(&s, 3 + INFO_TICKS, line), None);
    // The next fall toward a seam takes it down early.
    s.last_landing = Some(landing(LandingResult::Missed, 300, [0.0; 3]));
    assert_eq!(info.step(&s, 400, line), missed);
    s.airborne = true;
    assert_eq!(
        info.step(&s, 401, line),
        missed,
        "in the air, toward no seam"
    );
    s.landing = Some(predicted([9.5, 0.0, 2.0], 2.7));
    assert_eq!(info.step(&s, 402, line), None);
    // A fall on the tick a landing is filed does not hide that landing.
    s.last_landing = Some(landing(LandingResult::Missed, 410, [0.0; 3]));
    assert_eq!(info.step(&s, 412, line), missed);
    // A bounce takes the line down: its line is its medal.
    s.airborne = false;
    s.landing = None;
    s.last_landing = Some(landing(LandingResult::Bounce, 500, [0.0; 3]));
    assert_eq!(info.step(&s, 420, line), None);
}

fn ranked(at: Vec3, steer: [f32; 2], caught: bool) -> Ranked {
    Ranked {
        at,
        distance: (at[0] * at[0] + at[1] * at[1] + at[2] * at[2]).sqrt(),
        time: 2.0,
        steer,
        caught,
        reach: 6.0,
        speed: 8.1,
        need: 5.4,
        cost: 0.4,
    }
}

/// In the flight after a bounce the fall row is the best seam's line-up,
/// the ranked line lists the seams by where they are against the aim, and
/// `alignment` off leaves the row out and keeps the line.
#[test]
fn the_second_bounce_takes_the_fall_row_and_the_line() {
    let mut s = on();
    s.airborne = true;
    s.position = Some([0.0; 3]);
    // Rising: the assist predicts no landing yet.
    s.velocity = Some([1.0, 0.0, 6.0]);
    assert_eq!(view(&s, Options::DEFAULT, None).fall, None);
    assert_eq!(view(&s, Options::DEFAULT, None).next, [None, None]);
    // The best seam ahead, 2 units to steer to the right; the second to
    // the left.
    s.second = [
        Some(ranked([10.0, 0.0, 0.0], [0.0, -2.0], false)),
        Some(ranked([0.0, 20.0, 0.0], [0.0, 3.0], false)),
    ];
    let v = view(&s, Options::DEFAULT, Some(Miss::NotPrimed));
    let f = v.fall.unwrap();
    assert!(close(f.speed, 8.1 / 5.4));
    let Steer::Off { heading, metres } = f.steer else {
        panic!("{f:?}");
    };
    assert!(close(heading, FRAC_PI_2) && close(metres, 2.0 * M));
    let [Some(a), Some(b)] = v.next else {
        panic!("{:?}", v.next);
    };
    assert!(close(a.heading, 0.0) && close(a.metres, 10.0 * M));
    assert!(close(b.heading, -FRAC_PI_2) && close(b.metres, 20.0 * M));
    // The line is the ranked seams', over a miss's.
    let values = rd::values(&v);
    assert_eq!(
        values.get(INFO).unwrap().text,
        format!("1 {} 30 m  2 {} 61 m", arrow(0.0), arrow(-FRAC_PI_2))
    );
    assert!(values.get(STEER).unwrap().text.ends_with("6.10 m"));
    // Caught coasting: on the seam.
    assert_eq!(
        line_up(&ranked([10.0, 0.0, 0.0], [0.0, 0.0], true), 0.0).steer,
        Steer::OnSeam
    );
    // The best seam's row is over the assist's prediction.
    s.velocity = Some([1.0, 0.0, -4.0]);
    s.landing = Some(predicted([10.02, 0.3, 2.0], 6.0));
    assert_ne!(
        view(&s, Options::DEFAULT, None).fall.unwrap().steer,
        Steer::OnSeam
    );
    let off = view(
        &s,
        Options {
            alignment: false,
            ..Options::DEFAULT
        },
        None,
    );
    assert_eq!(off.fall, None);
    assert!(off.next[0].is_some());
    // With none ranked the row is the assist's again.
    s.second = [None, None];
    assert_eq!(
        view(&s, Options::DEFAULT, None).fall.unwrap().steer,
        Steer::OnSeam
    );
}
