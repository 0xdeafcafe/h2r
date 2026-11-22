//! The menu at the input layer: an [`input::PadGate`] that sees every
//! pad before the game does.
//!
//! * **Opening, any pad.** Back (View) and D-pad down held together for
//!   [`HOLD`] open it. The game has no use for that pair in any layout
//!   (docs/input/controller-layouts.md); the pair stays in the game's
//!   hands until the menu opens, and is hidden from it after.
//! * **Opening, touchpad pads.** A touchpad click on a DualSense or DualShock 4 opens the
//!   menu (the game has no use for the click). Touchpad click and Options
//!   (GameController's menu button, which this crate's mapping makes
//!   Start) held together open or close it. Pressed "together"
//!   is loose: on a pad with a touchpad, Start alone is held back for
//!   [`CHORD`] in case the click follows; if it doesn't, the game gets it
//!   then (a press shorter than that still reaches it, one sync late).
//! * **Navigating** while open: touchpad swipes move and change, the click
//!   selects, a two-finger tap goes back ([`input::touchpad`]); the D-pad
//!   moves and changes (held, it repeats: [`REPEAT_DELAY`], then
//!   [`REPEAT_EVERY`]), Cross/A selects, Circle/B goes back, the bumpers
//!   page and Triangle/Y stars, on any pad.
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
use crate::model::{Device, Nav};

/// How long Start waits for the touchpad click on a pad with a touchpad.
pub const CHORD: Duration = Duration::from_millis(80);

/// Back and D-pad down are held this long to open the menu on any pad.
pub const HOLD: Duration = Duration::from_millis(500);
/// A held D-pad direction repeats after this.
pub const REPEAT_DELAY: Duration = Duration::from_millis(350);
/// Then it repeats this often.
pub const REPEAT_EVERY: Duration = Duration::from_millis(80);

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
    /// The touchpad click last sync.
    click: bool,
    /// This press of the click opened the menu: it neither selects nor,
    /// with Options, closes it again until it is let go.
    opened_by_press: bool,
    start: StartHold,
    /// The D-pad direction repeating: which, since when, last fired.
    /// `Duration::MAX` marks one held from before it counted (a press that
    /// opened the menu), which never repeats.
    repeat: Option<(HostButton, Duration, Duration)>,
    /// Back and D-pad down are down together since then.
    hold: Option<Duration>,
    /// The hold opened the menu: its buttons do nothing until let go.
    hold_opened: bool,
}

/// One pad as the gate last saw it, for `h2 agent hostpads`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadReport {
    /// The input manager's key: backend index and the pad's id in it.
    pub key: (usize, u64),
    /// The pad came with a touchpad (so it can open the menu, and its
    /// Start waits [`CHORD`] for the click).
    pub touchpad: bool,
    /// The touchpad click is down.
    pub click: bool,
    /// Touchpad click and Start are both down.
    pub chord: bool,
    /// Start is held back, waiting for the click.
    pub chord_pending: bool,
    /// The click that opened the menu is still down.
    pub opened_by_press: bool,
    /// Buttons hidden from the game until they are let go.
    pub latched: HostButtons,
}

/// Every pad the gate saw at its last filter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GateReport {
    /// Filters since the gate was made.
    pub syncs: u64,
    pub pads: Vec<PadReport>,
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
            let click_edge = click && !t.click;
            if chord && !t.chord && !t.opened_by_press {
                let was_open = m.is_open();
                m.set_device(Device::Touchpad);
                m.toggle(now);
                t.opened_by_press = !was_open;
                t.start = StartHold::Idle;
            } else if click_edge && !m.is_open() {
                // The click alone opens it too: the game has no use for the
                // touchpad click, so nothing is taken from it.
                m.set_device(Device::Touchpad);
                m.toggle(now);
                t.opened_by_press = true;
                t.start = StartHold::Idle;
            } else if m.is_open() && !t.opened_by_press && !t.hold_opened {
                let mut navs: Vec<(Nav, Device)> = gestures
                    .iter()
                    .filter(|g| !(chord && **g == Gesture::Click))
                    .map(|g| {
                        let n = match g {
                            Gesture::Up => Nav::Up,
                            Gesture::Down => Nav::Down,
                            Gesture::Left => Nav::Left,
                            Gesture::Right => Nav::Right,
                            Gesture::Click => Nav::Select,
                            Gesture::TwoFingerTap => Nav::Back,
                        };
                        (n, Device::Touchpad)
                    })
                    .collect();
                for (b, n) in [
                    (HostButton::South, Nav::Select),
                    (HostButton::East, Nav::Back),
                    (HostButton::North, Nav::Favourite),
                    (HostButton::LeftBumper, Nav::PageUp),
                    (HostButton::RightBumper, Nav::PageDown),
                ] {
                    if raw.contains(b) && !t.held.contains(b) {
                        navs.push((n, Device::Pad));
                    }
                }
                if let Some(n) = repeat(t, raw, now) {
                    navs.push((n, Device::Pad));
                }
                for (n, d) in navs {
                    m.set_device(d);
                    m.nav(n, now);
                }
            } else {
                t.repeat = held_dir(raw).map(|(b, _)| (b, Duration::MAX, Duration::MAX));
            }
            // Back and D-pad down, held: opens the menu on any pad.
            if raw.contains(HostButton::Back) && raw.contains(HostButton::DpadDown) {
                let since = *t.hold.get_or_insert(now);
                if !m.is_open() && !t.hold_opened && now.saturating_sub(since) >= HOLD {
                    m.set_device(Device::Pad);
                    m.toggle(now);
                    t.hold_opened = true;
                    t.start = StartHold::Idle;
                }
            } else {
                t.hold = None;
            }
            if !raw.contains(HostButton::Back) && !raw.contains(HostButton::DpadDown) {
                t.hold_opened = false;
            }
            t.chord = chord;
            t.click = click;
            if !click {
                t.opened_by_press = false;
            }
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
        let report = &mut inner.gate;
        report.syncs += 1;
        report.pads.clear();
        report.pads.extend(pads.iter().map(|g| {
            let t = self.tracks.get(&g.key).copied().unwrap_or_default();
            PadReport {
                key: (g.key.0, g.key.1.0),
                touchpad: g.pad.state.touchpad.is_some(),
                click: t.click,
                chord: t.chord,
                chord_pending: matches!(t.start, StartHold::Waiting(_)),
                opened_by_press: t.opened_by_press,
                latched: t.latched,
            }
        }));
    }
}

impl PadGate for Gate {
    fn filter(&mut self, pads: &mut [GatedPad]) {
        let now = self.menu.now();
        self.filter_at(pads, now);
    }
}

/// The D-pad direction held, first of up, down, left, right.
fn held_dir(raw: HostButtons) -> Option<(HostButton, Nav)> {
    [
        (HostButton::DpadUp, Nav::Up),
        (HostButton::DpadDown, Nav::Down),
        (HostButton::DpadLeft, Nav::Left),
        (HostButton::DpadRight, Nav::Right),
    ]
    .into_iter()
    .find(|(b, _)| raw.contains(*b))
}

/// The step a held D-pad direction makes now: one at the press, then
/// every [`REPEAT_EVERY`] after [`REPEAT_DELAY`].
fn repeat(t: &mut Track, raw: HostButtons, now: Duration) -> Option<Nav> {
    let Some((b, n)) = held_dir(raw) else {
        t.repeat = None;
        return None;
    };
    match t.repeat {
        Some((rb, since, last)) if rb == b => {
            if now.saturating_sub(since) >= REPEAT_DELAY
                && now.saturating_sub(last) >= REPEAT_EVERY
            {
                t.repeat = Some((b, since, now));
                Some(n)
            } else {
                None
            }
        }
        _ => {
            t.repeat = Some((b, now, now));
            Some(n)
        }
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
