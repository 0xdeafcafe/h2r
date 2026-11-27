use std::sync::atomic::{AtomicU8, AtomicU32};

use input::gamepad::{AnalogButton, Buttons};
use input::host::HostPadState;
use xapi::tap::{InputTap, PACKET};

use super::*;
use crate::superbounce_assist::AssistTap;

/// The seat state is process-wide: one test at a time drives it.
static SERIAL: Mutex<()> = Mutex::new(());

/// What the stand-in view says: 0 a game at [`TICK`], 1 no game, 2 the
/// seat can't be played.
static MODE: AtomicU8 = AtomicU8::new(0);
static TICK: AtomicU32 = AtomicU32::new(100);

fn fake_view(_: &dyn GuestMemory) -> Result<Option<View>, String> {
    match MODE.load(Ordering::Relaxed) {
        0 => Ok(Some(View {
            tick: TICK.load(Ordering::Relaxed),
            machine: [1, 2, 3, 4, 5, 6],
            name: "Chief".into(),
            map: None,
        })),
        1 => Ok(None),
        _ => Err("split screen".into()),
    }
}

/// A source whose pad says which sample it is: the right stick's x is a
/// tenth of the frame, and it fires.
fn script_pad(frame: u64) -> HostPadState {
    HostPadState {
        left_stick: (0.0, 1.0),
        right_stick: (frame as f32 / 10.0, 0.0),
        right_trigger: 1.0,
        ..HostPadState::default()
    }
}

fn scripted(_: Ask, _: &str) -> Box<dyn PadSource> {
    Box::new(|f: &Frame<'_>| script_pad(f.frame))
}

const STAND_IN: Hooks = Hooks {
    make: scripted,
    view: fake_view,
};

/// Guest memory that reads as zeros: the stand-in view never looks.
struct Zeros;

impl xapi::GuestMemory for Zeros {
    fn check(&self, _: u32, _: usize, _: bool) -> xapi::Result<()> {
        Ok(())
    }
    fn read(&self, _: u32, bytes: &mut [u8]) -> xapi::Result<()> {
        bytes.fill(0);
        Ok(())
    }
    fn write(&mut self, _: u32, _: &[u8]) -> xapi::Result<()> {
        Ok(())
    }
}

const HANDLE: u32 = 7;

fn tap() -> AssistTap {
    let mut t = AssistTap::new(None);
    t.opened(HANDLE, PORT);
    t
}

/// One read of the first controller through the tap, the player holding
/// `real`: the pad the guest gets.
fn read(t: &mut AssistTap, real: Gamepad) -> Gamepad {
    let mut packet = [0u8; PACKET];
    packet[4..].copy_from_slice(&real.to_bytes());
    let mut status = 0;
    t.read_with_memory(HANDLE, &mut status, &mut packet, Some(&Zeros));
    let raw: &[u8; Gamepad::SIZE] = (&packet[4..]).try_into().unwrap();
    Gamepad::from_bytes(raw)
}

fn game_pad(frame: u64) -> Gamepad {
    script_pad(frame).to_gamepad(&Default::default(), false)
}

fn fresh() -> MutexGuard<'static, ()> {
    let g = lock(&SERIAL);
    turn_off("test");
    MODE.store(0, Ordering::Relaxed);
    TICK.store(100, Ordering::Relaxed);
    g
}

#[test]
fn params_turn_it_on_or_off_or_only_read() {
    let p = |v: Value| serde_json::from_value::<BotParams>(v).unwrap();
    assert_eq!(want(&p(serde_json::json!({}))), Ok(Want::Read));
    assert_eq!(
        want(&p(serde_json::json!({"on": true}))),
        Ok(Want::On(Ask {
            skill: Skill::Normal,
            style: Style::Normal
        }))
    );
    assert_eq!(
        want(&p(
            serde_json::json!({"on": true, "difficulty": "Legendary", "style": "wild"})
        )),
        Ok(Want::On(Ask {
            skill: Skill::Legendary,
            style: Style::Wild
        }))
    );
    assert_eq!(want(&p(serde_json::json!({"on": false}))), Ok(Want::Off));
    assert!(want(&p(serde_json::json!({"on": true, "difficulty": "mixed"}))).is_err());
    assert!(want(&p(serde_json::json!({"on": true, "style": "twitchy"}))).is_err());
    assert!(want(&p(serde_json::json!({"on": false, "style": "calm"}))).is_err());
    assert!(want(&p(serde_json::json!({"difficulty": "easy"}))).is_err());
}

#[test]
fn the_status_says_what_plays_and_why_it_stopped() {
    let _g = fresh();
    let s = status();
    assert!(!s.on && !s.driving);
    turn_on_with(
        Ask {
            skill: Skill::Heroic,
            style: Style::Calm,
        },
        STAND_IN,
    );
    let s = status();
    assert!(s.on && !s.driving, "on, but no pad read yet");
    assert_eq!(
        (s.difficulty.as_str(), s.style.as_str()),
        ("heroic", "calm")
    );
    assert_eq!(s.off, None);
    turn_off("agent");
    let s = status();
    assert!(!s.on);
    assert_eq!(s.off.as_deref(), Some("agent"));
    assert_eq!(s.difficulty, "heroic", "the last ask, once off");
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["off"], "agent");
}

#[test]
fn the_brains_pad_reaches_the_tap_once_a_tick() {
    let _g = fresh();
    let mut t = tap();
    // Off: the player's pad goes through.
    assert_eq!(read(&mut t, Gamepad::default()), Gamepad::default());
    turn_on_with(Ask::default(), STAND_IN);
    assert_eq!(read(&mut t, Gamepad::default()), game_pad(1));
    // Another read in the same tick: the same pad, no second sample.
    assert_eq!(read(&mut t, Gamepad::default()), game_pad(1));
    let s = status();
    assert!(s.on && s.driving);
    assert_eq!((s.ticks, s.name.as_deref()), (1, Some("Chief")));
    TICK.store(101, Ordering::Relaxed);
    let pad = read(&mut t, Gamepad::default());
    assert_eq!(pad, game_pad(2));
    assert_eq!(pad.analog(AnalogButton::RightTrigger), 255, "it fires");
    assert!(pad.thumb_ly > i16::MAX / 2, "and walks");
    assert_eq!(status().ticks, 2);
    turn_off("test");
}

#[test]
fn a_real_press_hands_the_seat_back() {
    let _g = fresh();
    let mut t = tap();
    turn_on_with(Ask::default(), STAND_IN);
    // A stick resting a little off centre is not a press.
    let drift = Gamepad {
        thumb_lx: 3000,
        ..Gamepad::default()
    };
    assert_eq!(read(&mut t, drift), game_pad(1));
    let mut press = Gamepad::default();
    press.analog[AnalogButton::A.index()] = 255;
    TICK.store(101, Ordering::Relaxed);
    assert_eq!(
        read(&mut t, press),
        press,
        "the press itself goes to the game"
    );
    let s = status();
    assert!(!s.on && !active());
    assert_eq!(s.off.as_deref(), Some("pad"));
    // From then on the player's pad is the player's.
    TICK.store(102, Ordering::Relaxed);
    assert_eq!(read(&mut t, Gamepad::default()), Gamepad::default());
}

#[test]
fn outside_a_game_the_real_pad_goes_through_and_the_brain_waits() {
    let _g = fresh();
    let mut t = tap();
    turn_on_with(Ask::default(), STAND_IN);
    MODE.store(1, Ordering::Relaxed);
    let mut press = Gamepad::default();
    press.buttons.0 = Buttons::START.0;
    assert_eq!(read(&mut t, press), press, "a menu press");
    assert!(active(), "a press in the menus does not hand the seat back");
    MODE.store(0, Ordering::Relaxed);
    assert_eq!(read(&mut t, Gamepad::default()), game_pad(1));
    // A seat that can't be played turns it off, saying why.
    MODE.store(2, Ordering::Relaxed);
    TICK.store(101, Ordering::Relaxed);
    assert_eq!(read(&mut t, Gamepad::default()), Gamepad::default());
    assert_eq!(status().off.as_deref(), Some("split screen"));
    assert!(!active());
}

#[test]
fn presses_are_buttons_triggers_and_sticks_well_off_centre() {
    assert!(!pressed(&Gamepad::default()));
    let mut g = Gamepad::default();
    g.analog[AnalogButton::LeftTrigger.index()] = BUTTON_PRESS - 1;
    assert!(!pressed(&g));
    g.analog[AnalogButton::LeftTrigger.index()] = BUTTON_PRESS;
    assert!(pressed(&g));
    let g = Gamepad {
        thumb_ry: -STICK_PRESS,
        ..Gamepad::default()
    };
    assert!(pressed(&g));
    let g = Gamepad {
        thumb_rx: STICK_PRESS - 1,
        ..Gamepad::default()
    };
    assert!(!pressed(&g));
    let mut g = Gamepad::default();
    g.buttons.0 = Buttons::DPAD_UP.0;
    assert!(pressed(&g));
}

#[test]
fn the_brain_is_a_phantoms_with_the_personality_of_the_seats_name() {
    let ask = Ask {
        skill: Skill::Heroic,
        style: Style::Wild,
    };
    let b = brain(ask, "Chief");
    let v = b.brain_view().expect("a brain");
    assert_eq!(v.skill, Skill::Heroic);
    assert_eq!(
        v.profile,
        Some(bots::name_profile::NameProfile::for_name("Chief"))
    );
    // The same name plays the same way; another name does not.
    let again = brain(ask, "Chief").brain_view().unwrap();
    assert_eq!(again.personality, v.personality);
    assert_ne!(
        brain(ask, "Arbiter").brain_view().unwrap().profile,
        v.profile
    );
}

#[test]
fn the_setting_is_a_diagnostic_one_that_starts_off() {
    use crate::setting_defs::{ALL, PLAYER_BOT};
    assert!(ALL.iter().any(|d| d.key == PLAYER_BOT.key));
    assert_eq!(PLAYER_BOT.key, "player.bot");
    assert_eq!(PLAYER_BOT.default, settings::Value::Bool(false));
    assert_eq!(PLAYER_BOT.tier, settings::Tier::Diagnostic);
    assert!(PLAYER_BOT.menu.is_none());
}
