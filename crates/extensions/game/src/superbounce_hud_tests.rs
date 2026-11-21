//! The adapter from `superbounce_mode::State` to the HUD's view, and the
//! values each state gives (docs/game/hud-kit.md, "States").

use hud_kit_core::huds::superbounce::{
    self as sb, CHARGE, LOW_CHARGE, Landed, Lost, MESSAGE, MESSAGE_LOST_CROUCHED,
    MESSAGE_LOST_STOOD_STILL, Medal, NAVPOINT, Note, PILL, PILL_ICON, PROMPT, PROMPT_CHARGE_AGAIN,
    PROMPT_HELD, PROMPT_KEEP_MOVING, PROMPT_NEAR, PROMPT_UNDER, Phase, Scan, Target, Verb,
};
use hud_kit_core::model::{Pulse, Style};

use super::*;
use crate::superbounce_chain::Award;
use crate::superbounce_charge::{SLEEP_TICKS, WARN_STILL_TICKS};
use crate::superbounce_mode::{
    Charge, ChargePhase, Landing, LandingResult, LostReason, NavKind, NavTarget, Near, PRIMED_WORD,
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
    Some(Charge {
        phase,
        fill,
        lost: None,
    })
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

/// The prompt line's warning starts at the tick the icon starts to flash:
/// 19 still ticks, half the fill.
#[test]
fn the_warning_lines_up_with_the_icons_flash() {
    assert_eq!(
        1.0 - WARN_STILL_TICKS as f32 / SLEEP_TICKS as f32,
        LOW_CHARGE
    );
    let s = State {
        word: Some(PRIMED_WORD as u32),
        charge: charge(ChargePhase::Primed, LOW_CHARGE),
        verb: Some(mode::Verb::KeepMoving),
        still_ticks: WARN_STILL_TICKS,
        ..ready()
    };
    let v = view(&s, None);
    assert_eq!(v.verb, Some(Verb::KeepMoving));
    let values = sb::values(&v);
    assert_eq!(values.get(PROMPT).unwrap().text, PROMPT_KEEP_MOVING);
    assert_eq!(
        values.get(PILL_ICON).unwrap().style,
        Style::Flash(Pulse::TimeWarning)
    );
    // A tick before: no warning, and no flash.
    let before = State {
        charge: charge(
            ChargePhase::Primed,
            1.0 - (WARN_STILL_TICKS - 1) as f32 / SLEEP_TICKS as f32,
        ),
        verb: None,
        ..s
    };
    let values = sb::values(&view(&before, None));
    assert!(!values.get(PROMPT).unwrap().shown);
    assert_eq!(values.get(PILL_ICON).unwrap().style, Style::Normal);
}

/// A loss says why in the message, and the prompt line asks for a new
/// charge.
#[test]
fn a_loss_says_why_and_asks_for_a_new_charge() {
    for (reason, why, text) in [
        (
            LostReason::StoodStill,
            Lost::StoodStill,
            MESSAGE_LOST_STOOD_STILL,
        ),
        (LostReason::Crouched, Lost::Crouched, MESSAGE_LOST_CROUCHED),
        (LostReason::Unknown, Lost::Unknown, sb::MESSAGE_LOST),
    ] {
        let lost = mode::Lost { reason, tick: 90 };
        let s = State {
            charge: Some(Charge {
                phase: ChargePhase::Lost,
                fill: 0.0,
                lost: Some(lost),
            }),
            verb: Some(mode::Verb::ChargeAgain(lost)),
            ..ready()
        };
        let v = view(&s, None);
        assert_eq!(v.note, Some((Note::ChargeLost(why), 90)));
        assert_eq!(v.verb, Some(Verb::ChargeAgain(why)));
        let values = sb::values(&v);
        let m = values.get(MESSAGE).unwrap();
        assert_eq!((m.text.as_str(), m.since_tick), (text, 90));
        // The reason is the notification's; the prompt line asks for a
        // new charge.
        assert_eq!(values.get(PROMPT).unwrap().text, PROMPT_CHARGE_AGAIN);
        assert_eq!(values.get(CHARGE).unwrap().fill, 0.0);
    }
}

/// The loss and the last landing: the newer one is the note, and a loss
/// the mode no longer carries keeps its place over an older landing.
#[test]
fn the_newer_of_the_loss_and_the_landing_is_the_note() {
    let landing = |tick| {
        Some(Landing {
            result: LandingResult::Missed,
            tick,
            at: [0.0; 3],
        })
    };
    let lost = mode::Lost {
        reason: LostReason::StoodStill,
        tick: 100,
    };
    let after_landing = State {
        last_landing: landing(50),
        verb: Some(mode::Verb::ChargeAgain(lost)),
        ..ready()
    };
    assert_eq!(
        view(&after_landing, None).note,
        Some((Note::ChargeLost(Lost::StoodStill), 100))
    );
    let landed_since = State {
        last_landing: landing(120),
        ..after_landing.clone()
    };
    assert_eq!(
        view(&landed_since, None).note,
        Some((Note::Landed(Landed::Missed), 120))
    );
    // Past the hint the state has no loss; the HUD's kept one still wins.
    let later = State {
        verb: None,
        ..after_landing
    };
    assert_eq!(
        view(&later, None).note,
        Some((Note::Landed(Landed::Missed), 50))
    );
    assert_eq!(
        note(&later, Some(lost)),
        Some((Note::ChargeLost(Lost::StoodStill), 100))
    );
}

/// A landing that did not bounce is a message at its tick; a bounce's
/// line is its medal, so it has none.
#[test]
fn a_landing_is_a_message_at_its_tick() {
    let bounce = State {
        last_landing: Some(Landing {
            result: LandingResult::Bounce,
            tick: 77,
            at: [0.0; 3],
        }),
        ..ready()
    };
    assert_eq!(view(&bounce, None).note, None);
    for (result, landed) in [
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

fn award(seq: u64, user: Option<u8>, tick: u32, medals: Vec<Medal>) -> Award {
    Award {
        seq,
        slot: user.unwrap_or(5),
        user,
        tick,
        chain: 0,
        medals,
    }
}

/// The medal line is a local player's last award's highest medal; a
/// bot's or a remote player's award is not shown.
#[test]
fn the_medal_is_the_last_local_awards_highest() {
    let mut s = ready();
    assert_eq!(view(&s, None).medal, None);
    s.medals.recent = vec![
        award(
            1,
            Some(0),
            10,
            vec![Medal::Superbounce, Medal::DoubleBounce],
        ),
        award(2, None, 12, vec![Medal::Superbounce, Medal::TripleBounce]),
    ];
    assert_eq!(view(&s, None).medal, Some((Medal::DoubleBounce, 10)));
    let v = view(&s, None);
    let m = sb::values(&v).get(sb::MEDAL).cloned().unwrap();
    assert_eq!((m.text, m.since_tick), (Medal::DoubleBounce.line(), 10));
}

/// Each notification is sent once: primed on its rise, a loss and a
/// landing that did not bounce by their ticks, each new local award's
/// highest medal to its user. What the first state already held is not
/// news, and the mode going off forgets all.
#[test]
fn each_notification_is_sent_once() {
    let mut sent = Sent::default();
    let mut s = ready();
    s.last_landing = Some(Landing {
        result: LandingResult::Missed,
        tick: 3,
        at: [0.0; 3],
    });
    s.medals.recent = vec![award(1, Some(0), 3, vec![Medal::Superbounce])];
    assert!(sent.notes(Some(&s), None).is_empty(), "the first state");

    s.word = Some(PRIMED_WORD as u32);
    assert_eq!(sent.notes(Some(&s), None), vec![(0, Note::Primed.text())]);
    assert!(sent.notes(Some(&s), None).is_empty(), "primed again");

    let lost = mode::Lost {
        reason: LostReason::Crouched,
        tick: 20,
    };
    s.word = None;
    assert_eq!(
        sent.notes(Some(&s), Some(lost)),
        vec![(0, MESSAGE_LOST_CROUCHED.to_string())]
    );
    assert!(sent.notes(Some(&s), Some(lost)).is_empty(), "the same loss");

    s.last_landing = Some(Landing {
        result: LandingResult::Bounce,
        tick: 30,
        at: [0.0; 3],
    });
    s.medals.recent.push(award(
        2,
        Some(1),
        30,
        vec![Medal::Superbounce, Medal::DoubleBounce],
    ));
    s.medals
        .recent
        .push(award(3, None, 30, vec![Medal::Superbounce]));
    assert_eq!(
        sent.notes(Some(&s), None),
        vec![(1, Medal::DoubleBounce.line())],
        "a bounce has no landing line; a bot's award is not sent"
    );
    assert!(sent.notes(Some(&s), None).is_empty());

    s.last_landing = Some(Landing {
        result: LandingResult::TooSlow,
        tick: 40,
        at: [0.0; 3],
    });
    assert_eq!(
        sent.notes(Some(&s), None),
        vec![(0, Note::Landed(Landed::TooSlow).text())]
    );

    // A new game: the awards count from 1 again.
    s.medals.recent = vec![award(1, Some(0), 2, vec![Medal::Superbounce])];
    assert_eq!(
        sent.notes(Some(&s), None),
        vec![(0, Medal::Superbounce.line())]
    );

    assert!(sent.notes(None, None).is_empty());
    assert_eq!(sent, Sent::default());
}

#[test]
fn off_hides_everything() {
    assert!(hidden().0.iter().all(|(_, v)| !v.shown));
    assert_eq!(hidden().0.len(), sb::hud().elements.len());
}
