//! The adapter from `superbounce_mode::State` to the HUD's view, and the
//! values each state gives (docs/game/hud-kit.md, "States").

use hud_kit_core::huds::superbounce::{
    self as sb, ARM, CHARGE, LOW_CHARGE, Landed, Lost, MESSAGE, MESSAGE_LOST_CROUCHED,
    MESSAGE_LOST_STOOD_STILL, Medal, NAVPOINT, Note, PILL, PILL_ICON, PROMPT, PROMPT_CHARGE_AGAIN,
    PROMPT_HELD, PROMPT_KEEP_MOVING, PROMPT_NEAR, PROMPT_UNDER, Phase, Scan, Target, Verb,
};
use hud_kit_core::model::{Pulse, Style};
use hud_kit_core::motion::Motion;

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
        fall: 0.0,
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
    // The recipe is on the arming bar, eased to each step's fill; the
    // strip under the pill waits for primed.
    let arm = values.get(ARM).unwrap();
    assert_eq!(
        (arm.shown, arm.fill, arm.motion),
        (true, 2.0 / 3.0, Motion::Ease)
    );
    assert!(!values.get(CHARGE).unwrap().shown);
    // Not primed: the pill is inactive, which the runtime does not draw
    // yet.
    assert_eq!(values.get(PILL).unwrap().style, Style::Inactive);
}

/// Tick to tick through the view: the steps ease, leaving early runs
/// out, primed plays the ending, a loss blinks.
#[test]
fn the_arming_bar_follows_the_charge_tick_to_tick() {
    let mut track = ArmTrack::default();
    let mut bar = |c: Option<Charge>| {
        let base = view(
            &State {
                charge: c,
                ..ready()
            },
            None,
        );
        let v = View {
            arm: track.update(base.charge.map(|c| c.phase)),
            ..base
        };
        let a = sb::values(&v).get(ARM).unwrap().clone();
        a.shown.then_some((a.fill, a.motion))
    };
    assert_eq!(bar(None), None);
    assert_eq!(
        bar(charge(ChargePhase::Near, 0.0)),
        Some((0.0, Motion::Ease))
    );
    assert_eq!(
        bar(charge(ChargePhase::Under, 0.3)),
        Some((1.0 / 3.0, Motion::Ease))
    );
    assert_eq!(bar(None), Some((0.0, Motion::RunOut)), "left early");
    assert_eq!(
        bar(charge(ChargePhase::Held, 0.6)),
        Some((2.0 / 3.0, Motion::Ease))
    );
    assert_eq!(
        bar(charge(ChargePhase::Primed, 1.0)),
        Some((1.0, Motion::Ending))
    );
    assert_eq!(
        bar(charge(ChargePhase::Lost, 0.0)),
        Some((0.0, Motion::Blink))
    );
    assert_eq!(bar(None), Some((0.0, Motion::Blink)));
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
            fall: 1.0,
        }),
        ..ready()
    };
    let v = view(&s, None);
    assert!(v.primed);
    assert_eq!(v.verb, None);
    // Over a strip the number is the drop, not the distance.
    let (target, m) = v.nav.unwrap();
    assert_eq!(target, Target::Strip);
    assert!((m - METRES_PER_UNIT).abs() < 1e-4, "{m}");
    assert_eq!(sb::values(&v).get(NAVPOINT).unwrap().text, "3");
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
        spot: None,
        strips: None,
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

    // A recipe step begins: "Charging...", once; the next step is the
    // same charge.
    s.charge = charge(ChargePhase::Near, 0.0);
    assert_eq!(
        sent.notes(Some(&s), None),
        vec![(0, "Charging...".to_string())]
    );
    s.charge = charge(ChargePhase::Under, 0.3);
    assert!(sent.notes(Some(&s), None).is_empty(), "the same charge");

    s.word = Some(PRIMED_WORD as u32);
    s.charge = charge(ChargePhase::Primed, 1.0);
    assert_eq!(sent.notes(Some(&s), None), vec![(0, "Primed!".to_string())]);
    assert!(sent.notes(Some(&s), None).is_empty(), "primed again");
    s.charge = None;

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

/// The lock holds primed, in the air, falling onto the slot fast enough:
/// the rule the world's brackets follow.
#[test]
fn the_reticle_locks_with_the_landing() {
    use crate::superbounce_charge::{Course, Predicted};
    let landing = |verdict, speed| {
        Some(Predicted {
            at: [0.0; 3],
            verdict,
            speed,
            need: 5.0,
            edge: [[0.0, -1.0, 0.0], [0.0, 1.0, 0.0]],
            stand: [0.0; 3],
            width: 0.2,
            fall: 1.0,
        })
    };
    let s = State {
        word: Some(PRIMED_WORD as u32),
        airborne: true,
        landing: landing(Course::OnSlot, 6.0),
        ..ready()
    };
    assert!(reticle_locked(&s));
    assert!(!reticle_locked(&State {
        landing: landing(Course::Fixable, 6.0),
        ..s.clone()
    }));
    assert!(!reticle_locked(&State {
        landing: landing(Course::OnSlot, 4.0),
        ..s.clone()
    }));
    assert!(!reticle_locked(&State {
        word: Some(0),
        ..s.clone()
    }));
    assert!(!reticle_locked(&State {
        airborne: false,
        ..s
    }));
}

/// While locked, a retail bitmap widget of the first local player that
/// reads the autoaimed input gets 1 in that slot after the game's own
/// answer; nothing else is touched.
#[test]
fn the_locked_reticle_reads_autoaimed() {
    use engine::memory::FlatMemory;
    const SP: u32 = 0x0010_0000;
    const INPUTS: u32 = 0x0010_0100;
    const OUT: u32 = 0x0010_0200;
    let mut m = FlatMemory::new(SP, 0x1000);
    m.set_u32(SP, BITMAP_INPUTS_RETURN).unwrap();
    m.set_u32(SP + 4, 0).unwrap();
    m.set_u32(SP + 8, INPUTS).unwrap();
    m.set_u32(SP + 12, OUT).unwrap();
    m.write(INPUTS, &[5, AUTOAIMED, 0, AUTOAIMED]).unwrap();
    assert_eq!(autoaim_slots([5, AUTOAIMED, 0, AUTOAIMED]), 0b1010);
    assert_eq!(autoaim_slots([16, 17, 20, 21]), 0);
    let a = autoaim_entry(&m, SP, true).unwrap().unwrap();
    assert_eq!(
        a,
        Autoaim {
            sp_after: SP + 16,
            out: OUT,
            slots: 0b1010
        }
    );
    // The game's answer, then ours on the way out.
    for k in 0..4 {
        m.set_f32(OUT + 4 * k, 0.25).unwrap();
    }
    assert!(!autoaim_exit(&mut m, SP + 4, a).unwrap(), "another stack");
    assert!(autoaim_exit(&mut m, SP + 16, a).unwrap());
    let out: Vec<f32> = (0..4).map(|k| m.f32(OUT + 4 * k).unwrap()).collect();
    assert_eq!(out, [0.25, 1.0, 0.25, 1.0]);
    // Not locked, a text widget, another player, or no autoaimed input:
    // left alone.
    assert_eq!(autoaim_entry(&m, SP, false).unwrap(), None);
    m.set_u32(SP, 0x0013_a673).unwrap();
    assert_eq!(autoaim_entry(&m, SP, true).unwrap(), None);
    m.set_u32(SP, BITMAP_INPUTS_RETURN).unwrap();
    m.set_u32(SP + 4, 1).unwrap();
    assert_eq!(autoaim_entry(&m, SP, true).unwrap(), None);
    m.set_u32(SP + 4, 0).unwrap();
    m.write(INPUTS, &[16, 17, 20, 21]).unwrap();
    assert_eq!(autoaim_entry(&m, SP, true).unwrap(), None);
}

#[test]
fn the_tutorial_counts_game_ticks_not_presents_or_wall_time() {
    use hud_kit_core::huds::superbounce::{Change, STEP3_SECS, Tutorial, Watch};
    assert_eq!(tutorial_seconds(0), 0.0);
    assert_eq!(tutorial_seconds(30), 1.0);
    // Step 3 gives way after 600 ticks without a bounce, whatever the
    // display rate (a tick is a tick at 60 and 120 fps) and however slow
    // the wall clock runs below 30 fps.
    let near = Watch {
        primed: true,
        strip_m: Some(1.0),
        bounced: false,
    };
    let mut t = Tutorial::new(true);
    let mut tick = 0;
    for _ in 0..3 {
        t.update(Some(near), tutorial_seconds(tick));
        tick += 1;
    }
    assert_eq!(t.step(), Some(3));
    let limit = (STEP3_SECS * 30.0) as u64;
    let shown = tick - 1;
    while t.update(Some(near), tutorial_seconds(tick)) == Change::None {
        tick += 1;
        assert!(tick < shown + 2 * limit, "step 3 never gave way");
    }
    // To a tick (the float sum of 1/30s).
    let after = tick - shown;
    assert!((limit..=limit + 1).contains(&after), "after {after} ticks");
    assert_eq!(t.step(), Some(4));
}

#[test]
fn in_the_air_the_hud_keeps_the_pill_and_the_lines_and_drops_the_rest() {
    let full = View {
        primed: true,
        charge: Some(sb::Charge {
            phase: Phase::Primed,
            fill: 1.0,
        }),
        arm: arm_for(Some(Phase::Held)),
        verb: Some(Verb::KeepMoving),
        nav: Some((Target::Strip, 3.0)),
        note: Some((Note::Primed, 7)),
        medal: None,
        tutorial: Some(2),
        ..View::idle()
    };
    assert!(full.arm.is_some());
    let v = in_air(full.clone());
    assert_eq!(
        (v.charge, v.arm, v.verb, v.nav, v.tutorial),
        (None, None, None, None, None)
    );
    assert!(v.primed);
    assert_eq!(v.note, full.note);
    assert_eq!(v.scan, full.scan);
}
