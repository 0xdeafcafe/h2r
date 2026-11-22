//! The step chain: what each block says for a state, where the blocks go,
//! and what runs on the wall clock (docs/game/superbounce.md, "The step
//! chain").

use std::time::Duration;

use super::*;
use crate::superbounce_mode::{Charge, Landing, NavTarget, PRIMED_WORD, Predicted};

fn playing() -> State {
    State {
        active: true,
        position: Some([0.0, 0.0, 0.0]),
        ..State::default()
    }
}

fn primed() -> State {
    State {
        word: Some(PRIMED_WORD as u32),
        nav_target: Some(NavTarget {
            at: [3.0, 0.0, -2.0],
            kind: NavKind::Strip,
            distance: 3.6,
            fall: 1.1,
        }),
        ..playing()
    }
}

fn charge(phase: ChargePhase, fill: f32) -> Option<Charge> {
    Some(Charge {
        phase,
        fill,
        lost: None,
    })
}

/// A fall toward a strip that needs 4.28: touching down at `speed`, the
/// coasting landing `off` from the crease.
fn fall(speed: f32, off: f32, verdict: Course) -> Option<Predicted> {
    Some(Predicted {
        at: [off, 0.0, 0.0],
        verdict,
        speed,
        need: 4.28,
        edge: [[0.0, -1.0, 0.0], [0.0, 1.0, 0.0]],
        stand: [0.0, 0.0, 0.0],
        fall: 1.1,
    })
}

fn landed(result: LandingResult, tick: u32) -> Option<Landing> {
    Some(Landing {
        result,
        tick,
        at: [0.0; 3],
    })
}

fn steps(blocks: &[Block]) -> Vec<(Step, Status)> {
    blocks.iter().map(|b| (b.step, b.status)).collect()
}

/// Seven pixels a character, lines of 13: near the HUD font's.
fn metrics() -> Metrics<'static> {
    fn width(text: &str) -> f32 {
        7.0 * text.chars().count() as f32
    }
    Metrics {
        width: &width,
        line: 13.0,
    }
}

// ---- the rule ----

#[test]
fn not_primed_the_chain_is_the_charge_alone_with_the_recipes_step() {
    let s = playing();
    let c = chain(&s, None);
    assert_eq!(steps(&c), [(Step::Prime, Status::Current)]);
    assert_eq!(
        (c[0].title.as_str(), c[0].detail.as_str()),
        ("Prime", "Crouch under a lip")
    );
    assert_eq!(c[0].fill, None);
    assert_eq!(current(&c), Some(Step::Prime));
    let says = |phase, fill| {
        let c = chain(
            &State {
                charge: charge(phase, fill),
                ..playing()
            },
            None,
        );
        (c[0].detail.clone(), c[0].fill)
    };
    assert_eq!(
        says(ChargePhase::Near, 0.0),
        ("Hold crouch, walk under".into(), Some(0.0))
    );
    assert_eq!(
        says(ChargePhase::Under, 1.0 / 3.0),
        ("Let go of crouch".into(), Some(1.0 / 3.0))
    );
    assert_eq!(
        says(ChargePhase::Held, 2.0 / 3.0),
        ("Walk out to stand up".into(), Some(2.0 / 3.0))
    );
    assert_eq!(
        says(ChargePhase::Lost, 0.0),
        ("Charge lost. Crouch again".into(), None)
    );
    // The mode off, or between lives: nothing.
    assert!(chain(&State::default(), None).is_empty());
    let dead = State {
        position: None,
        ..playing()
    };
    assert!(chain(&dead, None).is_empty());
}

#[test]
fn primed_the_speed_block_comes_with_the_live_speed_and_what_the_strip_needs() {
    // On the ground: nothing falling, and the drop to come from.
    let c = chain(&primed(), None);
    assert_eq!(
        steps(&c),
        [(Step::Prime, Status::Done), (Step::Speed, Status::Current)]
    );
    assert_eq!(c[0].title, "Primed");
    assert!(c[0].detail.is_empty() && c[0].fill.is_none());
    assert_eq!(c[1].title, "Speed 0.0");
    // A fall of 1.1 units is 3 m; it needs sqrt(9 + 2 * 4.15 * 1.1) = 4.26
    // units/s, 13.0 m/s.
    assert_eq!(c[1].detail, "Fall 3 m for 13.0");
    assert_eq!(c[1].fill, Some(0.0));
    assert_eq!(current(&c), Some(Step::Speed));
    // A strip with no drop needs the one threshold every seam shares, and
    // says so: no "a jump is enough".
    let mut flat = primed();
    flat.nav_target.as_mut().unwrap().fall = 0.0;
    assert_eq!(need(&flat), Some(superbounce::speed::BOUNCE_SPEED));
    assert!(chain(&flat, None)[1].detail.starts_with("Need "));
    // Standing still too long.
    let still = State {
        verb: Some(Verb::KeepMoving),
        ..primed()
    };
    assert_eq!(chain(&still, None)[0].title, "Primed: keep moving");
    // No strip to go to: no speed block.
    let lone = State {
        nav_target: None,
        ..primed()
    };
    assert_eq!(steps(&chain(&lone, None)), [(Step::Prime, Status::Done)]);
}

#[test]
fn in_a_fall_the_speed_is_live_and_done_once_the_touchdown_is_fast_enough() {
    // Falling at 2 units/s now, touching down at 3.5: under the 4.28.
    let slow = State {
        airborne: true,
        vz: Some(-2.0),
        landing: fall(3.5, 0.0, Course::OnSlot),
        ..primed()
    };
    let c = chain(&slow, None);
    assert_eq!(c[1].status, Status::Current);
    assert_eq!(c[1].title, "Speed 6.1");
    assert_eq!(c[1].detail, "Need 13.0 m/s");
    let fill = c[1].fill.unwrap();
    assert!((fill - 3.5 / 4.28).abs() < 1e-5, "{fill}");
    // The same moment of a higher fall: it will touch down at 5.
    let fast = State {
        landing: fall(5.0, 0.0, Course::OnSlot),
        ..slow.clone()
    };
    let c = chain(&fast, None);
    assert_eq!(
        (c[1].status, c[1].detail.as_str()),
        (Status::Done, "Fast enough")
    );
    assert_eq!(c[1].title, "Speed 6.1");
    assert_eq!(c[1].fill, Some(1.0));
    // Rising, the speed down is 0.
    let rising = State {
        airborne: true,
        vz: Some(6.0),
        ..primed()
    };
    assert_eq!(falling_speed(&rising), 0.0);
    assert_eq!(chain(&rising, None)[1].title, "Speed 0.0");
    assert_eq!(chain(&rising, None)[1].detail, "Need 13.0 m/s");
    // The need is the predicted strip's when there is one.
    assert_eq!(need(&slow), Some(4.28));
    assert_eq!(need(&playing()), None);
}

#[test]
fn in_a_fall_the_landing_block_says_on_the_strip_or_how_far_to_steer() {
    let falling = |off, verdict| State {
        airborne: true,
        vz: Some(-3.0),
        landing: fall(5.0, off, verdict),
        ..primed()
    };
    let on = chain(&falling(0.05, Course::OnSlot), None);
    assert_eq!(
        steps(&on),
        [
            (Step::Prime, Status::Done),
            (Step::Speed, Status::Done),
            (Step::Land, Status::Done)
        ]
    );
    assert_eq!(on[2].title, "On the strip");
    assert_eq!(current(&on), None);
    // Half a unit off the crease: 0.4 outside the 0.2 slot, 1.2 m.
    let off = chain(&falling(0.5, Course::Fixable), None);
    assert_eq!(off[2].status, Status::Current);
    assert_eq!(
        (off[2].title.as_str(), off[2].detail.as_str()),
        ("Steer 1.2 m", "Onto the strip")
    );
    assert_eq!(current(&off), Some(Step::Land));
    let far = chain(&falling(2.6, Course::Miss), None);
    assert_eq!(far[2].detail, "Too far to reach");
    // Not primed, a fall toward a strip is no step of the chain.
    let unprimed = State {
        word: None,
        ..falling(0.0, Course::OnSlot)
    };
    assert_eq!(
        steps(&chain(&unprimed, None)),
        [(Step::Prime, Status::Current)]
    );
}

#[test]
fn a_landing_shows_what_it_came_to() {
    let says = |r| {
        let c = chain(&primed(), Some(r));
        let b = c.last().unwrap().clone();
        assert_eq!(b.step, Step::Land);
        (b.status, b.title)
    };
    assert_eq!(
        says(LandingResult::Bounce),
        (Status::Done, "Bounce!".into())
    );
    assert_eq!(
        says(LandingResult::Missed),
        (Status::Failed, "Missed the strip".into())
    );
    assert_eq!(
        says(LandingResult::TooSlow),
        (Status::Failed, "Too slow".into())
    );
    assert_eq!(
        says(LandingResult::NoBounce),
        (Status::Failed, "No bounce".into())
    );
    // A landing on a strip with no charge, beside the charge's own block.
    let c = chain(&playing(), Some(LandingResult::NotPrimed));
    assert_eq!(
        steps(&c),
        [(Step::Prime, Status::Current), (Step::Land, Status::Failed)]
    );
    assert_eq!(c[1].title, "Not primed");
}

// ---- the wall clock ----

#[test]
fn a_result_stays_for_its_seconds_or_until_the_next_fall() {
    let t0 = Instant::now();
    let at = |s: f32| t0 + Duration::from_secs_f32(s);
    let mut track = Track::default();
    // The landing the state held when the chain first saw it is not news.
    let old = State {
        last_landing: landed(LandingResult::Missed, 100),
        ..primed()
    };
    assert_eq!(track.result(&old, at(0.0)), None);
    assert_eq!(track.result(&old, at(0.1)), None);
    // A new landing shows from the tick it is filed.
    let new = State {
        last_landing: landed(LandingResult::Bounce, 400),
        ..primed()
    };
    assert_eq!(track.result(&new, at(1.0)), Some(LandingResult::Bounce));
    assert_eq!(
        track.result(&new, at(1.0 + RESULT_SECONDS - 0.1)),
        Some(LandingResult::Bounce)
    );
    assert_eq!(track.result(&new, at(1.0 + RESULT_SECONDS + 0.1)), None);
    assert_eq!(track.result(&new, at(9.0)), None, "once gone it stays gone");
    // The next fall toward a strip takes the block at once.
    let next = State {
        last_landing: landed(LandingResult::TooSlow, 700),
        ..primed()
    };
    assert_eq!(track.result(&next, at(10.0)), Some(LandingResult::TooSlow));
    let falling = State {
        airborne: true,
        landing: fall(5.0, 0.0, Course::OnSlot),
        ..next.clone()
    };
    assert_eq!(track.result(&falling, at(10.2)), None);
    assert_eq!(track.result(&next, at(10.3)), None);
    // Cleared (the mode off), the first state is not news again.
    track.clear();
    assert_eq!(track.result(&next, at(11.0)), None);
}

#[test]
fn a_block_slides_in_from_the_instant_it_first_shows() {
    let t0 = Instant::now();
    let at = |s: f32| t0 + Duration::from_secs_f32(s);
    let mut track = Track::default();
    let alone = chain(&playing(), None);
    track.show(&alone, at(0.0));
    assert_eq!(track.since(Step::Prime), Some(at(0.0)));
    assert_eq!(track.since(Step::Speed), None);
    // Primed a second later: the speed block starts then, the first keeps
    // its instant.
    let two = chain(&primed(), None);
    track.show(&two, at(1.0));
    track.show(&two, at(1.1));
    assert_eq!(track.since(Step::Prime), Some(at(0.0)));
    assert_eq!(track.since(Step::Speed), Some(at(1.0)));
    // The charge lost, the speed block goes; primed again it slides again.
    track.show(&alone, at(2.0));
    assert_eq!(track.since(Step::Speed), None);
    track.show(&two, at(3.0));
    assert_eq!(track.since(Step::Speed), Some(at(3.0)));
    // The level: 0 as it comes, half way at half the slide, then in place.
    assert_eq!(level(at(1.0), at(1.0)), 0.0);
    assert!((level(at(1.0), at(1.0 + SLIDE / 2.0)) - 0.5).abs() < 1e-3);
    assert_eq!(level(at(1.0), at(1.0 + SLIDE)), 1.0);
    assert_eq!(level(at(1.0), at(9.0)), 1.0);
    assert_eq!(level(at(1.0), at(0.5)), 0.0, "a clock that ran back");
}

// ---- the layout ----

#[test]
fn the_blocks_sit_in_one_centred_row_under_the_reticle_without_meeting() {
    let m = metrics();
    let s = State {
        airborne: true,
        vz: Some(-3.0),
        landing: fall(3.5, 0.5, Course::Fixable),
        ..primed()
    };
    let blocks = chain(&s, None);
    assert_eq!(blocks.len(), 3);
    let centre = [320.0, 240.0];
    let rects = place(&blocks, &m, centre);
    assert_eq!(rects.len(), 3);
    for pair in rects.windows(2) {
        assert!((pair[1].x - pair[0].right() - GAP).abs() < 1e-3, "{pair:?}");
    }
    // Centred on the view, inside the 640 frame's title-safe width.
    let (left, right) = (rects[0].x, rects[2].right());
    assert!((left + right - 2.0 * centre[0]).abs() < 1e-3);
    assert!(left >= 48.0 && right <= 592.0, "{left}..{right}");
    // Under the reticle's 64 by 64 box, over the arming bar at 110 down.
    let top = rects.iter().map(|r| r.y).fold(f32::MAX, f32::min);
    let bottom = rects.iter().map(|r| r.bottom()).fold(f32::MIN, f32::max);
    assert!(top >= centre[1] + 32.0, "{top}");
    assert!(bottom <= centre[1] + 110.0 - 8.0, "{bottom}");
    // Each block on the row's middle line; a one-line block is shorter.
    let mid = |r: &Rect| r.y + r.h / 2.0;
    assert!((mid(&rects[0]) - mid(&rects[1])).abs() < 1e-3);
    assert!(rects[0].h < rects[1].h);
    // Wide enough for both lines and the mark.
    for (b, r) in blocks.iter().zip(&rects) {
        let [w, h] = size(b, &m);
        assert_eq!((r.w, r.h), (w, h));
        assert!(w >= (m.width)(&b.detail) + 2.0 * PAD_X);
        assert!(w >= (m.width)(&b.title) + 2.0 * PAD_X);
    }
    // One block alone is centred too.
    let one = place(&chain(&playing(), None), &m, centre);
    assert!((one[0].x + one[0].w / 2.0 - centre[0]).abs() < 1e-3);
    assert!(place(&[], &m, centre).is_empty());
}

#[test]
fn the_speed_blocks_width_does_not_change_with_its_digits() {
    let m = metrics();
    let at = |vz: f32| {
        let s = State {
            airborne: true,
            vz: Some(vz),
            ..primed()
        };
        let c = chain(&s, None);
        (c[1].title.clone(), size(&c[1], &m))
    };
    let (slow, a) = at(-0.5);
    let (fast, b) = at(-4.0);
    assert_eq!((slow.as_str(), fast.as_str()), ("Speed 1.5", "Speed 12.2"));
    assert_eq!(a, b);
}

fn texts(shapes: &[Shape]) -> Vec<(String, f32)> {
    shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Text { text, alpha, .. } => Some((text.clone(), *alpha)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_step_to_do_is_at_full_strength_done_ones_dimmed_and_a_failure_red() {
    let m = metrics();
    let colour = [0.4, 0.7, 1.0];
    let blocks = chain(&primed(), Some(LandingResult::Missed));
    assert_eq!(
        steps(&blocks),
        [
            (Step::Prime, Status::Done),
            (Step::Speed, Status::Current),
            (Step::Land, Status::Failed)
        ]
    );
    let out = shapes(&blocks, &[1.0, 1.0, 1.0], &m, [320.0, 240.0], colour);
    let t = texts(&out);
    let alpha = |text: &str| t.iter().find(|(s, _)| s == text).unwrap().1;
    assert_eq!(alpha("Primed"), DONE_ALPHA);
    assert_eq!(alpha("Speed 0.0"), 1.0);
    assert!(alpha("Missed the strip") > DONE_ALPHA);
    // The failed block's ink is the lock's red; the others the HUD's.
    let ink = |text: &str| {
        out.iter()
            .find_map(|s| match s {
                Shape::Text { text: t, color, .. } if t == text => Some(*color),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(ink("Missed the strip"), RED);
    assert_eq!(ink("Primed"), colour);
    // Three panels, each a dark fill of the HUD colour with cut corners.
    let panels: Vec<_> = out
        .iter()
        .filter(|s| matches!(s, Shape::Fill { points, .. } if points.len() == 8))
        .collect();
    assert_eq!(panels.len(), 3);
    // Two chevrons between three blocks: open three-point paths.
    let chevrons = out
        .iter()
        .filter(|s| {
            matches!(s, Shape::Path { points, closed: false, alpha, .. }
                if points.len() == 3 && *alpha == CHEVRON_ALPHA)
        })
        .count();
    assert_eq!(chevrons, 2);
}

#[test]
fn a_block_coming_in_is_fainter_and_left_of_its_place() {
    let m = metrics();
    let blocks = chain(&primed(), None);
    let centre = [320.0, 240.0];
    let colour = [0.4, 0.7, 1.0];
    let left_of = |shapes: &[Shape], text: &str| {
        shapes
            .iter()
            .find_map(|s| match s {
                Shape::Text {
                    text: t, at, alpha, ..
                } if t == text => Some((at[0], *alpha)),
                _ => None,
            })
            .unwrap()
    };
    let there = shapes(&blocks, &[1.0, 1.0], &m, centre, colour);
    let coming = shapes(&blocks, &[1.0, 0.5], &m, centre, colour);
    let (x1, a1) = left_of(&there, "Speed 0.0");
    let (x0, a0) = left_of(&coming, "Speed 0.0");
    assert!((x1 - x0 - SLIDE_FROM / 2.0).abs() < 1e-3, "{x0} {x1}");
    assert!((a0 - a1 / 2.0).abs() < 1e-5);
    // The block before it does not move.
    assert_eq!(left_of(&there, "Primed"), left_of(&coming, "Primed"));
    // Not yet come: not drawn, and no chevron toward it.
    let before = shapes(&blocks, &[1.0, 0.0], &m, centre, colour);
    assert!(texts(&before).iter().all(|(t, _)| t != "Speed 0.0"));
    assert_eq!(
        before.len(),
        shapes(&blocks[..1], &[1.0], &m, centre, colour).len()
    );
    // A missing level reads as in place.
    assert_eq!(shapes(&blocks, &[], &m, centre, colour), there);
}

#[test]
fn the_panel_is_a_box_with_cut_corners() {
    let r = Rect {
        x: 10.0,
        y: 20.0,
        w: 100.0,
        h: 30.0,
    };
    let p = panel(&r);
    assert_eq!(p.len(), 8);
    assert_eq!(p[0], [10.0 + CUT, 20.0]);
    assert_eq!(p[2], [110.0, 20.0 + CUT]);
    assert_eq!(p[5], [10.0 + CUT, 50.0]);
    assert!(
        p.iter()
            .all(|q| (10.0..=110.0).contains(&q[0]) && (20.0..=50.0).contains(&q[1]))
    );
}
