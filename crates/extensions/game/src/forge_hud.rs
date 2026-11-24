//! The monitor HUD's runtime (docs/game/forge-firefight-ui.md, "The
//! monitor HUD", slice U3): what the session shows each tick as the HUD
//! kit's `forge` HUD, drawn by the game's own HUD.
//!
//! - [`Published`] is what the editor publishes each tick ([`publish`]):
//!   the kit's [`View`] and the notice the session is saying. Cleared, the
//!   monitor is off and every element is hidden.
//! - [`buttons`] reads the controls the prompts name from the pad's table
//!   (`forge_controls::bindings::BINDINGS`), as the font's glyphs, so the
//!   HUD and the Controls page cannot disagree.
//! - [`Teaching`] picks the training box: the first time a state with a hint
//!   is reached, for [`TEACH_TICKS`], once for each hint.
//! - [`notice_line`] is the line a notice sends: the notification list
//!   keeps a line whole, so a longer sentence sends its first sentence, and
//!   one with no short first sentence none.
//! - [`hook`] registers the HUD with the HUD kit and feeds it every tick:
//!   the values when they change, the notice once each. It is part of the
//!   `forge` feature, which has no switch of its own for it.
//!
//! It is presentation: it reads the session and never writes it. The
//! monitor's reticle is still the compositor's (`forge_glue::page`), and so
//! are the gizmo and the markers (slices U6 and U7). The retail widgets are
//! not hidden in a monitor yet (slice K1).

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

use forge::tool::Action;
use hud_kit_core::model::{Values, message_fault};
use hud_kit_core::views::forge::{self as kit, Buttons, Cue, Forge, Teach, View};
use input::host::HostButton;

use crate::forge_controls::bindings::{self, Does, Gesture, Held, Pad, Side};
use crate::hud_kit::{self, Handle};

/// How long a hint stays up, in game ticks (seven seconds at 30).
pub const TEACH_TICKS: u32 = 210;
/// A hint left before it has been up this long is not taught: it was not
/// read (one second).
pub const TEACH_MIN: u32 = 30;
/// The local user the monitor's HUD and notices are for.
pub const USER: u32 = 0;

/// What the editor publishes each tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Published {
    pub view: View,
    /// The session's notice and the tick it was said at: a new stamp is a
    /// new line, the same one is not sent again.
    pub notice: Option<(String, u32)>,
}

/// A button's glyph in the fonts (docs/input/button-icons.md). The host
/// pad's bumpers are the game's White (left) and Black (right).
pub const fn glyph(b: HostButton) -> char {
    match b {
        HostButton::South => '\u{e100}',
        HostButton::East => '\u{e101}',
        HostButton::West => '\u{e102}',
        HostButton::North => '\u{e103}',
        HostButton::RightBumper => '\u{e104}',
        HostButton::LeftBumper => '\u{e105}',
        HostButton::DpadUp => '\u{e108}',
        HostButton::DpadDown => '\u{e109}',
        HostButton::DpadLeft => '\u{e10a}',
        HostButton::DpadRight => '\u{e10b}',
        HostButton::Start => '\u{e10c}',
        HostButton::Back => '\u{e10d}',
        HostButton::LeftStick => '\u{e110}',
        HostButton::RightStick => '\u{e111}',
    }
}

/// How the prompt names the control of `does`: the glyph of its pad
/// button, and whether it is held.
pub fn cue(does: Does) -> Cue {
    let (glyph, hold) = match bindings::binding(does).pad {
        Pad::Tap(b) | Pad::Down(b) => (glyph(b), false),
        Pad::Hold(b) => (glyph(b), true),
        Pad::Trigger(Side::Left) => ('\u{e106}', true),
        Pad::Trigger(Side::Right) => ('\u{e107}', true),
        Pad::Stick(Side::Left) => ('\u{e10e}', false),
        Pad::Stick(Side::Right) => ('\u{e10f}', false),
    };
    Cue { glyph, hold }
}

/// The controls the HUD names, from the table.
pub fn buttons() -> Buttons {
    Buttons {
        accept: cue(Does::Act(Gesture::Do(Action::Accept))),
        objects: cue(Does::Act(Gesture::ObjectsOrProperties)),
        delete: cue(Does::Act(Gesture::Do(Action::Delete))),
        turn: cue(Does::Hold(Held::Manipulate)),
        menu: cue(Does::Game),
    }
}

/// The hint to show, one at a time, each once.
#[derive(Clone, Debug, Default)]
pub struct Teaching {
    taught: BTreeSet<u8>,
    /// The hint up and the ticks it has left.
    showing: Option<(Teach, u32)>,
}

impl Teaching {
    /// One tick, with the hint the state has (`want`): the hint to show.
    /// A hint is shown the first time its state is reached, until its time
    /// is up or the state is left, and is then taught; one left within
    /// [`TEACH_MIN`] ticks is shown again the next time.
    pub fn step(&mut self, want: Option<Teach>) -> Option<Teach> {
        if let Some((t, left)) = self.showing {
            let up = TEACH_TICKS - left;
            if want != Some(t) || left <= 1 {
                if left <= 1 || up >= TEACH_MIN {
                    self.taught.insert(t as u8);
                }
                self.showing = None;
            } else {
                self.showing = Some((t, left - 1));
                return Some(t);
            }
        }
        let t = want.filter(|t| !self.taught.contains(&(*t as u8)))?;
        self.showing = Some((t, TEACH_TICKS));
        Some(t)
    }
}

/// The line `text` sends to the notification list: itself when the list
/// keeps it whole, else its first sentence when that is, else none.
pub fn notice_line(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if message_fault(text).is_none() {
        return Some(text.to_owned());
    }
    let first = text.split_once(". ").map(|(f, _)| format!("{f}."))?;
    message_fault(&first).is_none().then_some(first)
}

static PUBLISHED: Mutex<Option<Published>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the HUD shows from the next tick on.
pub fn publish(p: Published) {
    *lock(&PUBLISHED) = Some(p);
}

/// The HUD shows nothing from the next tick on.
pub fn clear() {
    *lock(&PUBLISHED) = None;
}

/// The values for what is published, and the notice to send (one not sent
/// before). `teaching` and `sent` are the feed's memory.
pub fn frame(
    p: &Published,
    teaching: &mut Teaching,
    sent: &mut Option<(String, u32)>,
    b: &Buttons,
) -> (Values, Option<String>) {
    let mut view = p.view.clone();
    view.teach = teaching.step(
        Some(&view)
            .filter(|v| v.monitor)
            .and_then(|v| Teach::of(v.tool, v.hand)),
    );
    let values = kit::values(&view, b);
    let notice = match &p.notice {
        Some(n) if view.monitor && sent.as_ref() != Some(n) => {
            *sent = Some(n.clone());
            notice_line(&n.0)
        }
        Some(_) => None,
        None => {
            *sent = None;
            None
        }
    };
    (values, notice)
}

struct Live {
    handle: Handle,
    last: Option<Values>,
    teaching: Teaching,
    sent: Option<(String, u32)>,
}

impl Live {
    fn tick(&mut self) {
        let p = lock(&PUBLISHED).clone().unwrap_or_default();
        let (values, notice) = frame(&p, &mut self.teaching, &mut self.sent, &buttons());
        if self.last.as_ref() != Some(&values) {
            self.handle.publish(values.clone());
            self.last = Some(values);
        }
        if let Some(line) = notice {
            self.handle.notify(USER, &line);
        }
    }
}

/// Registers the HUD with the HUD kit and feeds it each tick. The `forge`
/// feature's build calls it.
pub fn hook(b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
    static HANDLE: OnceLock<Handle> = OnceLock::new();
    let handle = *HANDLE.get_or_init(|| {
        let handle = hud_kit::register(std::sync::Arc::new(Forge));
        handle.publish(kit::values(&View::default(), &Buttons::sample()));
        handle
    });
    let live = Mutex::new(Live {
        handle,
        last: None,
        teaching: Teaching::default(),
        sent: None,
    });
    b.frame()
        .on_tick("forge hud: values", move |_| lock(&live).tick());
    Ok(())
}

#[cfg(test)]
#[path = "forge_hud_tests.rs"]
mod tests;
