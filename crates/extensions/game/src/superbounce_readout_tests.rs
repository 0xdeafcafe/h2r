//! The adapter from `superbounce_mode::State` to the readout's view, the
//! miss line's numbers and its time on screen (docs/game/hud-kit.md, "The
//! superbounce readout").

use std::f32::consts::{FRAC_PI_2, PI};

use hud_kit_core::views::superbounce_readout::{
    FALL, INFO, SPEED, STEER, TAG, TARGET_1, TARGET_2, arrow,
};
use superbounce::{Kind, Vec3};

use super::*;
use crate::superbounce_mode::{Course, Predicted, ScanState};
use crate::superbounce_options::TargetMode;
use crate::superbounce_second::Ranked;
use crate::superbounce_targets::{Mark, Marks};
use guest_layout::GuestMemory as _;

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
        // The seam has no drift and its stand is on the edge: the point
        // to come down on is the edge's nearest.
        aim: [10.0, at[1].clamp(-1.0, 1.0)],
        drift: [0.0; 2],
        across: 0.0,
        verdict: Course::Fixable,
        speed,
        need: s.need,
        edge: s.edge,
        stand: s.stand,
        fall: s.fall,
        steer: [0.0; 2],
    }
}

fn landing(result: LandingResult, tick: u32, at: Vec3) -> Landing {
    Landing { result, tick, at }
}

fn phase(drop: f32) -> crate::superbounce_guide::Phase {
    crate::superbounce_guide::Phase {
        verdict: superbounce::rule::Verdict {
            outcome: superbounce::rule::Outcome::Landing,
            tick: 0,
            row: [0.0; 3],
            speed: 0.0,
            gap: 0.0,
            pierce: None,
        },
        drop: Some(drop),
        edge: Some(0.02),
        apex: drop,
        crouching: false,
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// The readout's arrows are the font's: the same first code point, the
/// same count, the same step for an angle.
#[test]
fn the_readouts_arrows_are_the_fonts() {
    use hud_kit_core::views::superbounce_readout::{ARROW_UP, ARROWS};
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
fn the_fall_row_is_the_drop_window_and_the_way_to_the_seam() {
    let mut s = on();
    s.airborne = true;
    // Coasting down 0.5 short of the seam (which is ahead along +X), with
    // the apex-to-floor drop a quarter of the fade below the window.
    s.phase = Some(phase(3.1));
    s.landing = Some(predicted([9.5, 0.0, 2.0], 2.7));
    let f = fall(&s).unwrap();
    assert!(close(f.window, 0.75));
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
    // Within the catch and the drop window: on the seam, the bar full.
    s.phase = Some(phase(3.45));
    s.landing = Some(predicted([10.02, 0.3, 2.0], 6.0));
    let f = fall(&s).unwrap();
    assert_eq!(f.steer, Steer::OnSeam);
    assert_eq!(f.window, 1.0);
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
fn a_primed_fall_shows_its_drop_and_nearest_edge() {
    let mut s = on();
    s.word = Some(0xffff_fffd);
    s.airborne = true;
    s.phase = Some(phase(3.45));
    let a = air(&s).unwrap();
    assert!(close(a.drop, 3.45 * M) && close(a.edge.unwrap(), 0.02 * M));
    let values = rd::values(&view(&s, Options::DEFAULT, Some(Miss::NotPrimed)));
    assert_eq!(values.get(INFO).unwrap().text, "Drop 10.5 m  Edge 0.06 m");
    // Outside the window the same numbers, the bar short of full.
    s.phase = Some(phase(3.1));
    assert!(close(
        fall(&State {
            landing: Some(predicted([9.5, 0.0, 2.0], 2.7)),
            ..s.clone()
        })
        .unwrap()
        .window,
        0.75
    ));
    // On the ground, or with the alignment pieces off, there is no air row.
    s.airborne = false;
    assert_eq!(air(&s), None);
    s.airborne = true;
    let off = Options {
        alignment: false,
        ..Options::DEFAULT
    };
    assert_eq!(view(&s, off, None).air, None);
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
    // A fall through: the speed in, as a no-bounce's.
    let Some(Miss::Through { had }) = miss(&at(LandingResult::Through), &spots, Some(-6.0)) else {
        panic!();
    };
    assert!(had > 6.0 * M);
    assert_eq!(
        rd::miss_text(&Miss::Through { had: 17.2 }),
        "Fell through at 17.2 m/s"
    );
    // Without the speed, or by no seam, there is no line to give.
    assert_eq!(miss(&at(LandingResult::TooSlow), &spots, None), None);
    assert_eq!(miss(&at(LandingResult::NoBounce), &spots, None), None);
    assert_eq!(miss(&at(LandingResult::Through), &spots, None), None);
    assert_eq!(miss(&off, &[], None), None);
    assert_eq!(
        miss(&at(LandingResult::NotPrimed), &[], None),
        Some(Miss::NotPrimed)
    );
}

/// The mode's state with a body: the miss line's clock runs only while
/// the player is alive.
fn alive() -> State {
    State {
        position: Some([0.0; 3]),
        ..on()
    }
}

fn missed_line(l: &Landing) -> Option<Miss> {
    match l.result {
        LandingResult::Bounce => None,
        _ => Some(Miss::Missed { by: 0.3 }),
    }
}

const MISSED: Option<Miss> = Some(Miss::Missed { by: 0.3 });

/// A state and an `Info` with the miss line up, at the readout's tick 2.
fn with_line() -> (State, Info) {
    let mut s = alive();
    let mut info = Info::default();
    assert_eq!(info.step(&s, 1, missed_line), None, "the first state");
    s.last_landing = Some(landing(LandingResult::Missed, 100, [0.0; 3]));
    assert_eq!(info.step(&s, 2, missed_line), MISSED);
    (s, info)
}

#[test]
fn the_miss_line_stays_five_seconds_at_most() {
    let mut s = alive();
    s.last_landing = Some(landing(LandingResult::Missed, 50, [0.0; 3]));
    let mut info = Info::default();
    // The landing the first state held is not news.
    assert_eq!(info.step(&s, 1, missed_line), None);
    assert_eq!(info.step(&s, 2, missed_line), None);
    // A new one puts its line up for five seconds.
    s.last_landing = Some(landing(LandingResult::Missed, 90, [0.0; 3]));
    assert_eq!(info.step(&s, 3, missed_line), MISSED);
    assert_eq!(INFO_TICKS, 150);
    assert_eq!(info.step(&s, 3 + INFO_TICKS - 1, missed_line), MISSED);
    assert_eq!(info.step(&s, 3 + INFO_TICKS, missed_line), None);
}

/// The rebound hop after the failed landing, and a jump with a landing
/// predicted while rising, are no new fall: the line holds through them,
/// and after the hold it goes only on a real fall toward a seam.
#[test]
fn the_miss_line_survives_a_rebound_hop_and_a_rising_jump() {
    let (mut s, mut info) = with_line();
    // The rebound: up off the seam's foot, a landing predicted.
    s.airborne = true;
    s.landing = Some(predicted([9.5, 0.0, 2.0], 2.7));
    s.vz = Some(3.0);
    assert_eq!(info.step(&s, 4, missed_line), MISSED);
    // Coming down from it, inside the hold.
    s.vz = Some(-4.0);
    assert_eq!(info.step(&s, 20, missed_line), MISSED);
    // Past the hold: a jump rising with a landing predicted is no fall.
    s.vz = Some(5.0);
    assert_eq!(info.step(&s, 2 + MISS_HOLD_TICKS + 5, missed_line), MISSED);
    // Nor is drifting down slower than a touchdown.
    s.vz = Some(-0.5);
    assert_eq!(info.step(&s, 2 + MISS_HOLD_TICKS + 6, missed_line), MISSED);
    // Nor a fall toward no seam.
    s.vz = Some(-6.0);
    s.landing = None;
    assert_eq!(info.step(&s, 2 + MISS_HOLD_TICKS + 7, missed_line), MISSED);
    // A real fall toward a seam takes it down.
    s.landing = Some(predicted([9.5, 0.0, 2.0], 2.7));
    s.vz = Some(TOUCHDOWN_VZ);
    assert_eq!(info.step(&s, 2 + MISS_HOLD_TICKS + 8, missed_line), None);
}

/// A real fall inside the hold leaves the line; the tick the hold ends it
/// goes.
#[test]
fn a_real_fall_takes_the_line_down_after_the_hold() {
    let (mut s, mut info) = with_line();
    s.airborne = true;
    s.vz = Some(-6.0);
    s.landing = Some(predicted([9.5, 0.0, 2.0], 2.7));
    assert_eq!(info.step(&s, 2 + MISS_HOLD_TICKS - 1, missed_line), MISSED);
    assert_eq!(info.step(&s, 2 + MISS_HOLD_TICKS, missed_line), None);
    // A fall on the tick a landing is filed does not hide that landing.
    s.last_landing = Some(landing(LandingResult::Missed, 410, [0.0; 3]));
    assert_eq!(info.step(&s, 2 + MISS_HOLD_TICKS + 1, missed_line), MISSED);
}

/// A bounce takes the line down at once, inside the hold too: its line is
/// its medal.
#[test]
fn a_bounce_takes_the_line_down_at_once() {
    let (mut s, mut info) = with_line();
    s.last_landing = Some(landing(LandingResult::Bounce, 130, [0.0; 3]));
    assert_eq!(info.step(&s, 3, missed_line), None);
}

/// A landing the line cannot be worked out for (no seam by it, no fall
/// speed in the history) leaves the line shown as it is.
#[test]
fn a_landing_with_no_line_leaves_the_line() {
    let spots = [seam()];
    let (mut s, mut info) = with_line();
    // No speed history for a no-bounce by the seam.
    s.last_landing = Some(landing(LandingResult::NoBounce, 130, [10.0, 0.0, 2.1]));
    assert_eq!(info.step(&s, 10, |l| miss(l, &spots, None)), MISSED);
    // A miss by no seam.
    s.last_landing = Some(landing(LandingResult::Missed, 160, [50.0, 50.0, 0.0]));
    assert_eq!(info.step(&s, 20, |l| miss(l, &spots, None)), MISSED);
}

/// Under a second with no state keeps the line; longer is the mode off.
/// A new game (the state's landing gone) starts again, and the landing
/// after it is news.
#[test]
fn the_miss_line_survives_a_short_gap_in_the_state() {
    let (s, mut info) = with_line();
    for _ in 0..20 {
        assert!(!info.absent());
    }
    assert_eq!(info.step(&s, 23, missed_line), MISSED);
    for _ in 0..GAP_TICKS {
        assert!(!info.absent());
    }
    assert!(info.absent(), "over a second: the mode is off");
    // The landing the next state holds is not news.
    assert_eq!(info.step(&s, 100, missed_line), None);
    // A new game: no landing, then one at the new game's ticks.
    let mut fresh = alive();
    assert_eq!(info.step(&fresh, 101, missed_line), None);
    fresh.last_landing = Some(landing(LandingResult::Missed, 5, [0.0; 3]));
    assert_eq!(info.step(&fresh, 102, missed_line), MISSED);
}

/// Between lives the line's clock stops: it stays through the respawn
/// countdown, then has the rest of its time.
#[test]
fn the_miss_line_stays_through_a_death() {
    let (mut s, mut info) = with_line();
    s.position = None;
    assert_eq!(info.step(&s, 502, missed_line), MISSED);
    s.position = Some([0.0; 3]);
    assert_eq!(info.step(&s, 502 + INFO_TICKS - 1, missed_line), MISSED);
    assert_eq!(info.step(&s, 502 + INFO_TICKS, missed_line), None);
}

/// A fall through the seam's floor outranks every other line, replaces
/// one inside the hold whatever its speed, and only a bounce replaces it.
#[test]
fn a_fall_through_outranks_and_replaces() {
    let line = |l: &Landing| match l.result {
        LandingResult::Bounce => None,
        LandingResult::Through => Some(Miss::Through { had: l.at[2] }),
        _ => Some(Miss::TooSlow {
            had: l.at[2],
            need: 16.5,
        }),
    };
    let mut s = alive();
    let mut info = Info::default();
    assert_eq!(info.step(&s, 1, line), None);
    s.last_landing = Some(landing(LandingResult::TooSlow, 100, [0.0, 0.0, 20.0]));
    assert!(matches!(info.step(&s, 2, line), Some(Miss::TooSlow { .. })));
    // Slower, and a fall through: it replaces the line.
    s.last_landing = Some(landing(LandingResult::Through, 100, [0.0, 0.0, 5.0]));
    assert_eq!(info.step(&s, 3, line), Some(Miss::Through { had: 5.0 }));
    // A faster miss does not replace it, in the hold or after.
    s.last_landing = Some(landing(LandingResult::TooSlow, 140, [0.0, 0.0, 30.0]));
    assert_eq!(info.step(&s, 4, line), Some(Miss::Through { had: 5.0 }));
    s.last_landing = Some(landing(LandingResult::TooSlow, 300, [0.0, 0.0, 30.0]));
    assert_eq!(
        info.step(&s, 3 + MISS_HOLD_TICKS + 1, line),
        Some(Miss::Through { had: 5.0 })
    );
    // A bounce does.
    s.last_landing = Some(landing(LandingResult::Bounce, 320, [0.0; 3]));
    assert_eq!(info.step(&s, 3 + MISS_HOLD_TICKS + 2, line), None);
    // The rule itself.
    let through = Miss::Through { had: 1.0 };
    let slow = Miss::TooSlow {
        had: 9.0,
        need: 16.5,
    };
    assert!(replaces(&through, &slow, 0));
    assert!(!replaces(&slow, &through, 0));
    assert!(!replaces(&slow, &through, INFO_TICKS));
    assert!(replaces(&Miss::Through { had: 2.0 }, &through, 0));
    assert!(!replaces(&Miss::NotPrimed, &slow, 0));
    assert!(replaces(&Miss::NotPrimed, &slow, MISS_HOLD_TICKS));
}

fn ranked(at: Vec3, steer: [f32; 2], caught: bool) -> Ranked {
    Ranked {
        at,
        distance: (at[0] * at[0] + at[1] * at[1] + at[2] * at[2]).sqrt(),
        time: 2.0,
        steer,
        caught,
        drift: [0.0; 2],
        across: 0.0,
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
    s.phase = Some(phase(3.45));
    let v = view(&s, Options::DEFAULT, Some(Miss::NotPrimed));
    let f = v.fall.unwrap();
    assert_eq!(f.window, 1.0);
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
        line_up(&ranked([10.0, 0.0, 0.0], [0.0, 0.0], true), 0.0, Some(3.45)).steer,
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

/// The targets row is the kinds in effect and their count, with the
/// layout's flashlight glyph, whenever the map is scanned and has a spot;
/// the view of the state alone leaves it to the runtime, and a switch
/// no longer takes the miss line.
#[test]
fn the_targets_row_shows_the_kinds_in_effect_and_their_count() {
    let s = State { bounce: 3, ..on() };
    let row = kinds_row(&s, TargetMode::Likely, 4, rd::FLASHLIGHT, None).unwrap();
    assert_eq!(
        row,
        rd::KindsRow {
            kinds: rd::Kinds::Likely,
            count: 4,
            button: rd::FLASHLIGHT,
            switched: None,
        }
    );
    let v = View {
        kinds: Some(row),
        ..view(&s, Options::DEFAULT, Some(Miss::NotPrimed))
    };
    let values = rd::values(&v);
    assert_eq!(
        values.get(rd::KINDS).unwrap().text,
        "Targets: Likely (4)  \u{e105}\u{e105}"
    );
    assert_eq!(values.get(INFO).unwrap().text, "Not primed");
    // A switch in the state does not take the miss line.
    let switched = State {
        switched: Some(TargetMode::Common),
        ..s.clone()
    };
    let v = view(&switched, Options::DEFAULT, Some(Miss::NotPrimed));
    assert_eq!(v.kinds, None);
    assert_eq!(rd::values(&v).get(INFO).unwrap().text, "Not primed");
    // Common on a map with none proven: the row says 0.
    let none = kinds_row(&s, TargetMode::Common, 0, rd::FLASHLIGHT, None).unwrap();
    assert_eq!(
        rd::kinds_text(&none),
        "Targets: Common (0)  \u{e105}\u{e105}"
    );
    // Before the scan is ready, and on a map with no spot, no row.
    let scanning = State {
        scan: ScanState::Scanning,
        ..s.clone()
    };
    assert_eq!(
        kinds_row(&scanning, TargetMode::All, 4, rd::FLASHLIGHT, None),
        None
    );
    let empty = State {
        bounce: 0,
        crouch: 0,
        ..s.clone()
    };
    assert_eq!(
        kinds_row(&empty, TargetMode::All, 4, rd::FLASHLIGHT, None),
        None
    );
    // The flashlight is X on Bumper Jumper, White on every other layout.
    use crate::superbounce_ghost::{BUTTON, BUTTON_BUMPER_JUMPER};
    assert_eq!(flashlight_glyph(BUTTON), rd::FLASHLIGHT);
    assert_eq!(
        flashlight_glyph(BUTTON_BUMPER_JUMPER),
        rd::FLASHLIGHT_BUMPER_JUMPER
    );
    assert_eq!(
        [TargetMode::Common, TargetMode::Likely, TargetMode::All].map(kinds),
        rd::Kinds::EVERY
    );
}

/// A switch flashes the row from the tick it first shows in the state; a
/// switch to another mode starts the flash again; it ends with the
/// state's switch.
#[test]
fn the_targets_row_flashes_from_the_switch() {
    use hud_kit_core::model::{Pulse, Style};
    let mut f = SwitchFlash::default();
    assert_eq!(f.step(None, 10), None);
    assert_eq!(f.step(Some(TargetMode::All), 11), Some(11));
    assert_eq!(f.step(Some(TargetMode::All), 40), Some(11));
    assert_eq!(f.step(Some(TargetMode::Common), 50), Some(50));
    assert_eq!(f.step(None, 111), None);
    assert_eq!(f.step(Some(TargetMode::Common), 200), Some(200));
    let s = State { bounce: 3, ..on() };
    let row = kinds_row(&s, TargetMode::All, 23, rd::FLASHLIGHT, Some(11)).unwrap();
    let x = rd::kinds_value(&row);
    assert_eq!(x.text, "Targets: All (23)  \u{e105}\u{e105}");
    assert_eq!((x.style, x.since_tick), (Style::Flash(Pulse::Help), 11));
}

/// Off the seam and out of the assist's reach the fall row says so, with
/// no arrow and the bar kept; off it and fixable it keeps its arrow; a
/// caught landing is on the seam whatever the verdict.
#[test]
fn out_of_reach_shows_for_a_miss_and_not_for_a_fixable_landing() {
    let mut s = on();
    s.airborne = true;
    s.phase = Some(phase(3.1));
    let mut p = predicted([9.5, 0.0, 2.0], 2.7);
    s.landing = Some(p);
    assert!(
        matches!(fall(&s).unwrap().steer, Steer::Off { .. }),
        "fixable"
    );
    p.verdict = Course::Miss;
    s.landing = Some(p);
    let f = fall(&s).unwrap();
    assert_eq!(f.steer, Steer::OutOfReach);
    assert!(close(f.window, 0.75));
    let values = rd::values(&view(&s, Options::DEFAULT, None));
    assert_eq!(values.get(STEER).unwrap().text, rd::OUT_OF_REACH);
    assert!(values.get(FALL).unwrap().shown);
    // Caught: on the seam.
    let mut caught = predicted([10.02, 0.3, 2.0], 6.0);
    caught.verdict = Course::Miss;
    s.landing = Some(caught);
    assert_eq!(fall(&s).unwrap().steer, Steer::OnSeam);
}

/// The line-up points at the seam's landing point, not the edge's nearest
/// point: off the seam the steer is to `Predicted::aim`; caught, a
/// landing coming down still or against the seam's drift is told the way
/// to drift, and one drifting with it is on the seam.
#[test]
fn the_line_up_is_to_the_landing_point_and_wants_the_seams_drift() {
    let mut s = on();
    s.airborne = true;
    // The seam runs along Y at x 10 and bounces drifting +X; its measured
    // landing is 0.02 before it. Coasting down 0.5 short.
    let mut p = predicted([9.5, 0.0, 2.0], 6.0);
    p.aim = [9.98, 0.4];
    p.drift = [1.0, 0.0];
    s.landing = Some(p);
    let Steer::Off { heading, metres } = fall(&s).unwrap().steer else {
        panic!("off the seam");
    };
    assert!(close(metres, 0.48f32.hypot(0.4) * M));
    assert!(close(heading, against_aim(0.48, 0.4, 0.0)));
    // Caught (within the catch of the edge), coming straight down: drift
    // +X, which is ahead; aiming along +Y it is to the right.
    p.at = [9.98, 0.0, 2.0];
    s.landing = Some(p);
    assert_eq!(fall(&s).unwrap().steer, Steer::Drift { heading: 0.0 });
    s.yaw = Some(FRAC_PI_2);
    let Steer::Drift { heading } = fall(&s).unwrap().steer else {
        panic!("caught, no drift");
    };
    assert!(close(heading, FRAC_PI_2));
    // Drifting against it is no better; with it, the landing is on.
    p.across = -0.5;
    s.landing = Some(p);
    assert!(matches!(fall(&s).unwrap().steer, Steer::Drift { .. }));
    p.across = 0.5;
    s.landing = Some(p);
    assert_eq!(fall(&s).unwrap().steer, Steer::OnSeam);
    // The second bounce's line-up reads the same.
    let mut r = ranked([10.0, 0.0, 0.0], [0.0; 2], true);
    assert_eq!(line_up(&r, 0.0, Some(3.45)).steer, Steer::OnSeam);
    r.drift = [0.0, 1.0];
    let Steer::Drift { heading } = line_up(&r, 0.0, Some(3.45)).steer else {
        panic!("caught, no drift");
    };
    assert!(close(heading, -FRAC_PI_2));
    r.across = 0.8;
    assert_eq!(line_up(&r, 0.0, Some(3.45)).steer, Steer::OnSeam);
    r.caught = false;
    assert!(matches!(
        line_up(&r, 0.0, Some(3.45)).steer,
        Steer::Off { .. }
    ));
}

/// The approach guide's marks are the state's guide, with the variant's
/// option: what they show in the view, and where each lies.
#[test]
fn the_approach_guide_is_the_states_with_its_option() {
    use crate::superbounce_guide::{Body, guide};
    use hud_kit_core::views::superbounce_guide::{BOX, Call, DRIFT, MARKS, PIPPER, TAKEOFF, TICKS};
    use hud_kit_core::model::Style;
    let spot = Spot {
        drift: [1.0, 0.0],
        ..seam()
    };
    let mut s = on();
    assert_eq!(view(&s, Options::DEFAULT, None).guide, None);
    assert!(
        guide_poses(&s, Options::DEFAULT)
            .iter()
            .all(|(_, p)| p.is_none())
    );
    // Primed on the ground a unit before the seam: the box on its landing
    // point, the arrow unlit; the floor underfoot is not the seam's, so
    // no pipper and no ticks (`superbounce_guide::guide_from`).
    let body = Body {
        position: [9.0, 0.0, 2.03],
        velocity: [0.0; 3],
        airborne: false,
        primed: true,
        // The centre on the origin: the heights below are the depth
        // rule's own.
        lift: 0.0,
        stick: [0.0; 2],
    };
    s.guide = guide(&body, &spot);
    let v = view(&s, Options::DEFAULT, None).guide.unwrap();
    assert_eq!(
        (v.call, v.pipper, v.ticks, v.drift),
        (Call::OffLine, false, false, Some(false))
    );
    assert_eq!((v.cue, v.takeoff), (None, false));
    let values = rd::values(&view(&s, Options::DEFAULT, None));
    assert_eq!(
        MARKS.map(|id| values.get(id).unwrap().shown),
        [true, false, false, true, false]
    );
    assert!(!values.get(PIPPER).unwrap().shown && !values.get(TICKS).unwrap().shown);
    assert_eq!(values.get(BOX).unwrap().style, Style::Dim);
    assert_eq!(values.get(BOX).unwrap().text, "");
    let poses = guide_poses(&s, Options::DEFAULT);
    assert_eq!(poses.map(|(id, _)| id), MARKS);
    assert!(
        poses
            .iter()
            .all(|(id, p)| p.is_some() == (*id == BOX || *id == DRIFT))
    );
    let at = |id: &str| poses.iter().find(|(i, _)| *i == id).unwrap().1.unwrap().at;
    assert_eq!(at(BOX), [10.0, 0.0, 2.0]);
    assert!(at(DRIFT)[0] > 10.0);
    // Standing on a ledge 3.45 over the seam with its takeoff: the box
    // says `Walk to line`; the takeoff is the world's bar, not a mark.
    let ledge = Body {
        position: [7.0, 0.0, 5.45],
        ..body
    };
    let takeoff = crate::superbounce_guide::Takeoff {
        at: [8.0, 0.0, 5.45 - crate::superbounce_guide::CLEARANCE],
        heading: [1.0, 0.0],
        half: 0.03,
        speed: 0.8,
        leave: crate::superbounce_guide::Leave::Jump,
        checked: true,
    };
    s.guide = crate::superbounce_guide::guide_from(&ledge, &spot, Some(takeoff));
    let values = rd::values(&view(&s, Options::DEFAULT, None));
    assert_eq!(values.get(BOX).unwrap().text, "Walk to line");
    assert!(!values.get(TAKEOFF).unwrap().shown);
    assert!(
        guide_poses(&s, Options::DEFAULT)
            .iter()
            .all(|(id, p)| *id != TAKEOFF || p.is_none())
    );
    // The variant's option takes all of it away.
    let off = Options {
        approach_guide: false,
        ..Options::DEFAULT
    };
    assert_eq!(view(&s, off, None).guide, None);
    assert!(guide_poses(&s, off).iter().all(|(_, p)| p.is_none()));
    let values = rd::values(&view(&s, off, None));
    assert_eq!(MARKS.map(|id| values.get(id).unwrap().shown), [false; 5]);
}

/// In a primed fall the box shows the guide's call alone and the fall
/// row the steer line alone, with no percentage; the variant's option
/// hides the box.
#[test]
fn the_guides_verdict_has_no_percentage() {
    use crate::superbounce_guide::{Body, guide};
    use hud_kit_core::views::superbounce_guide::{BOX, Call};
    let scan = Spot {
        drift: [1.0, 0.0],
        ..seam()
    };
    let measured = Spot {
        landed: true,
        ..scan
    };
    // One tick over the seam, 0.05 before its edge, 0.0375 under the cut.
    let body = Body {
        position: [9.95, 0.0, 2.37],
        velocity: [0.3, 0.0, -5.3],
        airborne: true,
        primed: true,
        lift: 0.0,
        stick: crate::superbounce_flight::keep([0.3, 0.0]),
    };
    let mut s = on();
    s.airborne = true;
    s.landing = Some(predicted([9.95, 0.0, 2.0], 6.0));
    let texts = |s: &State, o: Options| {
        let values = rd::values(&view(s, o, None));
        (
            values.get(BOX).unwrap().text.clone(),
            values.get(STEER).unwrap().text.clone(),
        )
    };
    s.guide = guide(&body, &measured);
    assert_eq!(s.guide.unwrap().call, Call::WillBounce);
    let (on_box, row) = texts(&s, Options::DEFAULT);
    assert_eq!(
        (on_box.as_str(), row.as_str()),
        ("Will bounce", rd::ON_SEAM)
    );
    assert!(!on_box.contains('%'));
    // The scan's seam has the same label: no capped chance is shown.
    s.guide = guide(&body, &scan);
    let (on_box, row) = texts(&s, Options::DEFAULT);
    assert_eq!(
        (on_box.as_str(), row.as_str()),
        ("Will bounce", rd::ON_SEAM)
    );
    // Too shallow.
    s.guide = guide(
        &Body {
            position: [9.95, 0.0, 2.43],
            ..body
        },
        &measured,
    );
    let (on_box, row) = texts(&s, Options::DEFAULT);
    assert_eq!(
        (on_box.as_str(), row.as_str()),
        ("Too shallow", rd::ON_SEAM)
    );
    // With the guide off the row is as it was, and the box is hidden.
    let off = Options {
        approach_guide: false,
        ..Options::DEFAULT
    };
    let (on_box, row) = texts(&s, off);
    assert_eq!((on_box.as_str(), row.as_str()), ("", rd::ON_SEAM));
}

/// The charge segments are the mode's charge, stage for stage, once the
/// map is scanned and has a spot, with the variant's option.
#[test]
fn the_charge_segments_are_the_modes_charge() {
    use crate::superbounce_mode::{Charge, ChargePhase};
    use hud_kit_core::views::superbounce_segments::{IDS, Stage};
    let charge = |phase, fill| {
        Some(Charge {
            phase,
            fill,
            lost: None,
        })
    };
    let mut s = State { bounce: 2, ..on() };
    // Not primed, by no crouch spot: the empty ring.
    let ring = segments(&s).unwrap();
    assert_eq!((ring.stage, ring.lit), (Stage::Idle, 0.0));
    for (phase, fill, stage, lit) in [
        (ChargePhase::Near, 0.0, Stage::Near, 0.0),
        (ChargePhase::Under, 1.0 / 3.0, Stage::Under, 3.0),
        (ChargePhase::Held, 2.0 / 3.0, Stage::Held, 6.0),
        (ChargePhase::Primed, 1.0, Stage::Primed, 9.0),
        (ChargePhase::Primed, 0.5, Stage::Warning, 4.5),
        (ChargePhase::Lost, 0.0, Stage::Lost, 0.0),
    ] {
        s.charge = charge(phase, fill);
        let g = segments(&s).unwrap();
        assert_eq!((g.stage, g.lit), (stage, lit), "{phase:?} {fill}");
    }
    // In the view, and in the values: all nine, lit as the stage says.
    s.charge = charge(ChargePhase::Held, 2.0 / 3.0);
    let v = view(&s, Options::DEFAULT, None);
    assert_eq!(v.segments.map(|g| g.stage), Some(Stage::Held));
    let values = rd::values(&v);
    let fills = IDS.map(|id| {
        let x = values.get(id).unwrap();
        (x.shown, x.fill)
    });
    assert_eq!(fills[..6], [(true, 1.0); 6]);
    assert_eq!(fills[6..], [(true, 0.0); 3]);
    // The variant's option hides them.
    let off = Options {
        charge_segments: false,
        ..Options::DEFAULT
    };
    assert_eq!(view(&s, off, None).segments, None);
    let values = rd::values(&view(&s, off, None));
    assert!(IDS.iter().all(|id| !values.get(id).unwrap().shown));
    // Before the scan is ready, and on a map with no spot, nothing.
    let none = State {
        bounce: 0,
        crouch: 0,
        ..s.clone()
    };
    assert_eq!(segments(&none), None);
    let scanning = State {
        scan: ScanState::Scanning,
        ..s
    };
    assert_eq!(segments(&scanning), None);
}

fn miss_line(l: &Landing) -> Option<Miss> {
    match l.result {
        LandingResult::Bounce => None,
        LandingResult::TooSlow => Some(Miss::TooSlow {
            had: l.at[2],
            need: 16.5,
        }),
        _ => Some(Miss::Missed { by: 0.3 }),
    }
}

/// A hop at a seam's foot files a landing too; within the hold of the
/// real miss it does not replace it, unless it came in faster.
#[test]
fn a_new_miss_does_not_replace_a_younger_one_unless_it_came_in_faster() {
    let slow = |had: f32, tick| landing(LandingResult::TooSlow, tick, [0.0, 0.0, had]);
    let too_slow = |had| Some(Miss::TooSlow { had, need: 16.5 });
    let mut s = alive();
    let mut info = Info::default();
    s.last_landing = Some(slow(0.0, 1));
    assert_eq!(info.step(&s, 1, miss_line), None, "the first state");
    // The real landing, then a hop at 6.3 m/s a second later.
    s.last_landing = Some(slow(12.0, 100));
    assert_eq!(info.step(&s, 10, miss_line), too_slow(12.0));
    s.last_landing = Some(slow(6.3, 130));
    assert_eq!(info.step(&s, 40, miss_line), too_slow(12.0));
    // The hop did not restart the clock: the real line still goes five
    // seconds after it came up.
    assert_eq!(info.step(&s, 10 + INFO_TICKS, miss_line), None);
    // A faster one replaces it.
    s.last_landing = Some(slow(14.0, 200));
    assert_eq!(info.step(&s, 200, miss_line), too_slow(14.0));
    s.last_landing = Some(slow(15.0, 210));
    assert_eq!(info.step(&s, 210, miss_line), too_slow(15.0));
    // A miss with no speed in is not higher than one with.
    s.last_landing = Some(landing(LandingResult::Missed, 220, [0.0; 3]));
    assert_eq!(info.step(&s, 220, miss_line), too_slow(15.0));
    // After MISS_HOLD_TICKS any miss replaces it.
    s.last_landing = Some(slow(3.0, 300));
    assert_eq!(
        info.step(&s, 210 + MISS_HOLD_TICKS, miss_line),
        too_slow(3.0)
    );
}

/// Only the targets within 40 degrees of the aim keep their navpoint; the
/// game would clamp the others onto the tracker and the readout.
#[test]
fn the_targets_outside_the_aim_cone_are_cleared() {
    let mark = |at: Vec3| Some(Mark { at, distance: 10.0 });
    let polar = |deg: f32| {
        let a = deg.to_radians();
        mark([10.0 * a.cos(), 10.0 * a.sin(), 0.0])
    };
    let mut s = on();
    s.position = Some([0.0; 3]);
    // Straight ahead (+X) and 39 degrees to the left.
    s.marks.targets = [polar(0.0), polar(39.0)];
    assert_eq!(ahead(&s), s.marks.targets);
    // 41 degrees to the right, and behind.
    s.marks.targets = [polar(-41.0), polar(180.0)];
    assert_eq!(ahead(&s), [None, None]);
    // The aim turns with the yaw: the one behind is now ahead.
    s.yaw = Some(PI);
    assert_eq!(ahead(&s), [None, s.marks.targets[1]]);
    // The view's numbers follow.
    let v = view(&s, Options::DEFAULT, None);
    assert_eq!(v.targets, [None, Some(10.0 * M)]);
    // With no position known, none is cleared.
    s.position = None;
    assert_eq!(ahead(&s), s.marks.targets);
}

/// The waypoint area's bottom is raised with the mode on and put back
/// with it off, and never lowered below the map's own.
#[test]
fn the_waypoint_bottom_inset_is_raised_while_the_mode_is_on() {
    use guest_layout::testing::FakeGuest;
    assert_eq!(waypoint_inset(10.0, false), 10.0);
    assert_eq!(waypoint_inset(10.0, true), WAYPOINT_BOTTOM_INSET);
    assert_eq!(waypoint_inset(120.0, true), 120.0);
    // The offset is the hudg definition's `waypoint_bottom_inset`.
    let def = tags::defs::Defs::builtin()
        .group(map::FourCC(*b"hudg"))
        .unwrap();
    assert_eq!(
        def.field("waypoint_bottom_inset").unwrap().offset,
        HUDG_WAYPOINT_BOTTOM_INSET
    );

    const HUDG: u32 = 0x7e00_0000;
    let pointer = guest_layout::points::HUD_GLOBALS_TAG.base;
    let at = HUDG + HUDG_WAYPOINT_BOTTOM_INSET;
    let mut g = FakeGuest::new();
    g.map(HUDG, 0x1000).map(pointer, 4);
    g.put_u32(pointer, HUDG).put_f32(at, 4.0);
    keep_waypoint_inset(&mut g, true);
    assert_eq!(g.f32(at), Ok(WAYPOINT_BOTTOM_INSET));
    keep_waypoint_inset(&mut g, true);
    assert_eq!(g.f32(at), Ok(WAYPOINT_BOTTOM_INSET));
    keep_waypoint_inset(&mut g, false);
    assert_eq!(g.f32(at), Ok(4.0), "the map's own is put back");
}
