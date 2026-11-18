//! The menu at the input layer: an [`input::PadGate`] that sees every
//! pad before the game does.
//!
//! * **Opening.** Touchpad click and Options (GameController's menu
//!   button, which this crate's mapping makes Start) held together, on a
//!   DualSense or DualShock 4, open or close the menu. Pressed "together"
//!   is loose: on a pad with a touchpad, Start alone is held back for
//!   [`CHORD`] in case the click follows; if it doesn't, the game gets it
//!   then (a press shorter than that still reaches it, one sync late).
//! * **Navigating** while open: touchpad swipes move and change, the click
//!   selects, a two-finger tap goes back ([`input::touchpad`]); the D-pad
//!   moves and changes, Cross/A selects and Circle/B goes back, on any pad.
//! * **Swallowing.** While open, every pad the game sees is neutral (no
//!   buttons, sticks centred, triggers released), so opening releases
//!   whatever was held. On closing, whatever is still held stays hidden
//!   until it is let go: the Circle that closed the menu never melees, the
//!   Options of the chord never pauses.

use std::collections::HashMap;
use std::time::Duration;

use input::host::{HostButton, HostButtons, HostPadId, HostPadState};
use input::touchpad::{Gesture, Gestures};
use input::{GatedPad, PadGate};

use crate::Shared;
use crate::model::Nav;

/// How long Start waits for the touchpad click on a pad with a touchpad.
pub const CHORD: Duration = Duration::from_millis(80);

/// A trigger counts as held from this pull.
const TRIGGER_HELD: f32 = 0.1;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum StartHold {
    #[default]
    Idle,
    /// Down since then, hidden while the click may follow.
    Waiting(Duration),
    /// Held past the chord window: the game has it.
    Passing,
    /// Let go inside the window: shown this one sync.
    Replay,
}

/// One pad's state between syncs.
#[derive(Clone, Copy, Debug, Default)]
struct Track {
    gestures: Gestures,
    /// The raw buttons last sync.
    held: HostButtons,
    /// Hidden from the game until released.
    latched: HostButtons,
    /// Triggers hidden until released (left, right).
    latched_triggers: (bool, bool),
    chord: bool,
    start: StartHold,
}

/// The gate. One per [`input::InputManager`]; [`Shared::gate`] makes it.
pub struct Gate {
    menu: Shared,
    tracks: HashMap<(usize, HostPadId), Track>,
    last_open: bool,
}

impl Gate {
    pub fn new(menu: Shared) -> Self {
        Gate {
            menu,
            tracks: HashMap::new(),
            last_open: false,
        }
    }

    /// [`PadGate::filter`] at `now` (time since the menu was made).
    pub fn filter_at(&mut self, pads: &mut [GatedPad], now: Duration) {
        let mut inner = self.menu.lock();
        inner.pump(now);
        let m = &mut inner.menu;
        self.tracks.retain(|k, _| pads.iter().any(|p| p.key == *k));
        // Pass 1: chords and navigation.
        for g in pads.iter() {
            let t = self.tracks.entry(g.key).or_default();
            let s = &g.pad.state;
            let raw = s.buttons;
            let click = s.touchpad.is_some_and(|t| t.click);
            let chord = click && raw.contains(HostButton::Start);
            let gestures = t.gestures.update(s.touchpad.as_ref());
            if chord && !t.chord {
                m.toggle(now);
                t.start = StartHold::Idle;
            } else if m.is_open() {
                let mut navs: Vec<Nav> = gestures
                    .iter()
                    .filter(|g| !(chord && **g == Gesture::Click))
                    .map(|g| match g {
                        Gesture::Up => Nav::Up,
                        Gesture::Down => Nav::Down,
                        Gesture::Left => Nav::Left,
                        Gesture::Right => Nav::Right,
                        Gesture::Click => Nav::Select,
                        Gesture::TwoFingerTap => Nav::Back,
                    })
                    .collect();
                for (b, n) in [
                    (HostButton::DpadUp, Nav::Up),
                    (HostButton::DpadDown, Nav::Down),
                    (HostButton::DpadLeft, Nav::Left),
                    (HostButton::DpadRight, Nav::Right),
                    (HostButton::South, Nav::Select),
                    (HostButton::East, Nav::Back),
                ] {
                    if raw.contains(b) && !t.held.contains(b) {
                        navs.push(n);
                    }
                }
                for n in navs {
                    m.nav(n, now);
                }
            }
            t.chord = chord;
        }
        // Pass 2: what the game gets.
        let open = m.is_open();
        let closed_now = self.last_open && !open;
        self.last_open = open;
        for g in pads.iter_mut() {
            let t = self.tracks.entry(g.key).or_default();
            let s = &mut g.pad.state;
            let raw = s.buttons;
            if closed_now {
                t.latched = raw;
                t.latched_triggers = (
                    s.left_trigger >= TRIGGER_HELD,
                    s.right_trigger >= TRIGGER_HELD,
                );
                t.start = StartHold::Idle;
            }
            if open {
                neutral(s);
                t.start = StartHold::Idle;
            } else {
                hide_latched(t, s);
                if s.touchpad.is_some() {
                    chord_wait(t, s, raw, now);
                }
            }
            t.held = raw;
        }
    }
}

impl PadGate for Gate {
    fn filter(&mut self, pads: &mut [GatedPad]) {
        let now = self.menu.now();
        self.filter_at(pads, now);
    }
}

/// What a pad reports to the game while the menu is open.
fn neutral(s: &mut HostPadState) {
    let keep = (s.prefiltered, s.touchpad);
    *s = HostPadState {
        prefiltered: keep.0,
        touchpad: keep.1,
        ..Default::default()
    };
}

/// Hides what was held when the menu closed, until it is let go.
fn hide_latched(t: &mut Track, s: &mut HostPadState) {
    let still = HostButtons::from_bits(t.latched.bits() & s.buttons.bits());
    t.latched = still;
    s.buttons = HostButtons::from_bits(s.buttons.bits() & !still.bits());
    if let Some(a) = s.analog_buttons.as_mut() {
        // The pressure of a hidden face button goes too.
        for (i, b) in [
            HostButton::South,
            HostButton::East,
            HostButton::West,
            HostButton::North,
            HostButton::RightBumper,
            HostButton::LeftBumper,
        ]
        .into_iter()
        .enumerate()
        {
            if still.contains(b) {
                a[i] = 0.0;
            }
        }
    }
    if t.latched_triggers.0 {
        if s.left_trigger < TRIGGER_HELD {
            t.latched_triggers.0 = false;
        } else {
            s.left_trigger = 0.0;
        }
    }
    if t.latched_triggers.1 {
        if s.right_trigger < TRIGGER_HELD {
            t.latched_triggers.1 = false;
        } else {
            s.right_trigger = 0.0;
        }
    }
}

/// Start on a pad with a touchpad, while the menu is closed: held back
/// for [`CHORD`] in case the click joins it.
fn chord_wait(t: &mut Track, s: &mut HostPadState, raw: HostButtons, now: Duration) {
    let down = raw.contains(HostButton::Start) && !t.latched.contains(HostButton::Start);
    let hide = |s: &mut HostPadState| s.buttons.set(HostButton::Start, false);
    t.start = match t.start {
        StartHold::Idle | StartHold::Replay if down && !t.held.contains(HostButton::Start) => {
            hide(s);
            StartHold::Waiting(now)
        }
        StartHold::Waiting(since) if down => {
            if now.saturating_sub(since) >= CHORD {
                StartHold::Passing
            } else {
                hide(s);
                StartHold::Waiting(since)
            }
        }
        StartHold::Waiting(_) => {
            // Let go inside the window: the game still gets the press.
            s.buttons.set(HostButton::Start, true);
            StartHold::Replay
        }
        StartHold::Replay => StartHold::Idle,
        StartHold::Passing if !down => StartHold::Idle,
        other => other,
    };
}
