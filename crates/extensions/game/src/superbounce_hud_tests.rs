//! The adapter from `superbounce_mode::State` to the HUD's view, and the
//! values each state gives (docs/game/hud-kit.md, "States").

use hud_kit_core::huds::superbounce::{
    self as sb, CHARGE, Landed, MESSAGE, NAVPOINT, Note, PILL, PROMPT, PROMPT_HELD, PROMPT_NEAR,
    PROMPT_UNDER, Phase, Scan, Target, Verb,
};
use hud_kit_core::model::Style;

use super::*;
use crate::superbounce_mode::{
    Charge, ChargePhase, Landing, LandingResult, NavKind, NavTarget, Near, PRIMED_WORD,
};

fn ready() -> State {
    State {
        active: true,
        scan: ScanState::Ready,
        bounce: 3,
        crouch: 2,
        ..State::default()
    }
}

fn charge(phase: ChargePhase, fill: f32) -> Option<Charge> {
    Some(Charge { phase, fill })
}

fn crouch_spot(distance: f32) -> Option<NavTarget> {
    Some(NavTarget {
        at: [1.0, 2.0, 3.0],
        kind: NavKind::Crouch,
        distance,
    })
}

#[test]
fn the_scan_maps_to_the_pills_states() {
    let mut s = ready();
    assert_eq!(view(&s, None).scan, Scan::Ready);
    s.bounce = 0;
    s.crouch = 0;
    assert_eq!(view(&s, None).scan, Scan::NoStrips);
    s.scan = ScanState::Scanning;
    assert_eq!(view(&s, None).scan, Scan::Scanning);
    s.scan = ScanState::Idle;
    assert_eq!(view(&s, None).scan, Scan::Scanning);
    s.scan = ScanState::Failed("x".into());
    assert_eq!(view(&s, None).scan, Scan::Failed);
}

#[test]
fn the_recipe_steps_give_their_verbs_and_fills() {
    let near = State {
        charge: charge(ChargePhase::Near, 0.0),
        nav_target: crouch_spot(0.5),
        ..ready()
    };
    let v = view(&near, None);
    assert_eq!(
        v.verb,
        Some(Verb::HoldAndWalkUnder),
        "near, no verb of the mode's"
    );
    assert_eq!(
        v.charge.map(|c| (c.phase, c.fill)),
        Some((Phase::Near, 0.0))
    );
    let (target, m) = v.nav.unwrap();
    assert_eq!(target, Target::Crouch);
    assert!(
        (m - 0.5 * METRES_PER_UNIT).abs() < 1e-4,
        "world units to metres"
    );
    let values = sb::values(&v);
    assert_eq!(values.get(PROMPT).unwrap().text, PROMPT_NEAR);
    assert!(values.get(NAVPOINT).unwrap().shown);

    let under = State {
        charge: charge(ChargePhase::Under, 1.0 / 3.0),
        verb: Some(mode::Verb::LetGoOfCrouch),
        ..near.clone()
    };
    assert_eq!(
        sb::values(&view(&under, None)).get(PROMPT).unwrap().text,
        PROMPT_UNDER
    );
    let held = State {
        charge: charge(ChargePhase::Held, 2.0 / 3.0),
        verb: Some(mode::Verb::WalkOutToStand),
        ..near.clone()
    };
    let values = sb::values(&view(&held, None));
    assert_eq!(values.get(PROMPT).unwrap().text, PROMPT_HELD);
    assert_eq!(values.get(CHARGE).unwrap().fill, 2.0 / 3.0);
    // Not primed: the pill is inactive, which the runtime does not draw
    // yet; the charge meter is drawn.
    assert_eq!(values.get(PILL).unwrap().style, Style::Inactive);
    assert!(values.get(CHARGE).unwrap().shown);
}

#[test]
fn primed_lights_the_pill_and_drains_the_meter() {
    let s = State {
        word: Some(PRIMED_WORD as u32),
        charge: charge(ChargePhase::Primed, 0.25),
        nav_target: Some(NavTarget {
            at: [0.0; 3],
            kind: NavKind::Strip,
            distance: 4.0,
        }),
        ..ready()
    };
    let v = view(&s, None);
    assert!(v.primed);
    assert_eq!(v.verb, None);
    assert_eq!(v.nav.map(|n| n.0), Some(Target::Strip));
    let values = sb::values(&v);
    assert_eq!(values.get(PILL).unwrap().style, Style::Normal);
    assert_eq!(values.get(CHARGE).unwrap().fill, 0.25);
    assert!(!values.get(PROMPT).unwrap().shown);
}

#[test]
fn a_landing_is_a_message_at_its_tick() {
    for (result, landed) in [
        (LandingResult::Bounce, Landed::Bounce { height_m: None }),
        (LandingResult::NotPrimed, Landed::NotPrimed),
        (LandingResult::Missed, Landed::Missed),
        (LandingResult::TooSlow, Landed::TooSlow),
        (LandingResult::NoBounce, Landed::NoBounce),
    ] {
        let s = State {
            last_landing: Some(Landing {
                result,
                tick: 77,
                at: [0.0; 3],
            }),
            ..ready()
        };
        let v = view(&s, None);
        assert_eq!(v.note, Some((Note::Landed(landed), 77)));
        let m = sb::values(&v).get(MESSAGE).cloned().unwrap();
        assert_eq!((m.shown, m.since_tick), (true, 77));
    }
}

#[test]
fn the_tutorial_watches_primed_strips_and_new_bounces() {
    let mut s = State {
        word: Some(PRIMED_WORD as u32),
        nearest: Some(Near {
            distance: 2.0,
            launch: 1.0,
            fall: 0.0,
            edge: [[0.0; 3]; 2],
        }),
        last_landing: Some(Landing {
            result: LandingResult::Bounce,
            tick: 5,
            at: [0.0; 3],
        }),
        ..ready()
    };
    let w = watch(&s, None);
    assert!(w.primed && w.bounced);
    assert!((w.strip_m.unwrap() - 2.0 * METRES_PER_UNIT).abs() < 1e-4);
    assert!(!watch(&s, Some(5)).bounced, "the same landing twice");
    s.last_landing.as_mut().unwrap().result = LandingResult::Missed;
    assert!(!watch(&s, None).bounced, "not a bounce");
}

#[test]
fn off_hides_everything() {
    assert!(hidden().0.iter().all(|(_, v)| !v.shown));
    assert_eq!(hidden().0.len(), sb::hud().elements.len());
}
