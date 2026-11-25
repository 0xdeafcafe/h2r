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
//! - [`Points`] are the world points of the HUD's navpoints (the held or
//!   aimed thing, a link's ends, the other monitors) and the gizmo's axes:
//!   [`Points::resolved`] picks the nearest monitors and measures the
//!   distances, which the view shows as the navpoints' numbers.
//!
//! It is presentation: it reads the session and never writes it. The
//! monitor has no reticle until the kit's `forge.reticle` is built. While
//! the monitor is up the user's retail widgets are off but the motion
//! tracker (`hud_kit::Handle::retail`, slice K1).

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

use forge::tool::Action;
use hud_kit_core::model::{Values, message_fault};
use hud_kit_core::views::forge::{self as kit, Buttons, Cue, Forge, Marks, Teach, View};
use hud_kit_core::world::{Pose, Vec3};
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Published {
    pub view: View,
    /// The session's notice and the tick it was said at: a new stamp is a
    /// new line, the same one is not sent again.
    pub notice: Option<(String, u32)>,
    /// Where the navpoints and the gizmo are in the world.
    pub points: Points,
}

/// Which of the monitor's drawings over the world are on: the markers (the
/// held, aimed and linked things' navpoints), the rings (the other
/// monitors' navpoints) and the gizmo. All are on until the player turns
/// one off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Show {
    pub markers: bool,
    pub rings: bool,
    pub gizmo: bool,
}

impl Default for Show {
    fn default() -> Self {
        Show {
            markers: true,
            rings: true,
            gizmo: true,
        }
    }
}

/// A navpoint's place and its distance from the monitor in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub at: Vec3,
    pub metres: u32,
}

/// The points [`Points`] resolves to: what each element shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Resolved {
    pub target: Option<Point>,
    pub from: Option<Point>,
    pub monitors: [Option<Point>; kit::MONITORS],
    /// The gizmo's three segments, origin and end.
    pub gizmo: Option<[(Vec3, Vec3); 3]>,
}

/// Where the monitor's marks are in the world, as the session has them
/// this tick, with the drawings that are turned off ([`Show`]) left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Points {
    /// The monitor's eye, which the distances are from.
    pub eye: Vec3,
    /// The held, aimed or link's second end.
    pub target: Option<Vec3>,
    /// The link's first end.
    pub from: Option<Vec3>,
    /// Every other seat's monitor.
    pub monitors: Vec<Vec3>,
    /// The gizmo's axes.
    pub axes: Option<crate::forge_glue::gizmo::Axes>,
}

/// World units to metres: a world unit is ten feet.
const METRES_PER_UNIT: f32 = 3.048;

impl Points {
    fn point(&self, at: Vec3) -> Option<Point> {
        let d = (0..3)
            .map(|i| at[i] - self.eye[i])
            .map(|d| d * d)
            .sum::<f32>();
        let metres = d.sqrt() * METRES_PER_UNIT;
        (at.iter().all(|v| v.is_finite()) && metres.is_finite()).then(|| Point {
            at,
            // Whole metres, rounded; a navpoint shows at most 999.
            metres: metres.round().min(999.0) as u32,
        })
    }

    /// What each element shows: the nearest monitors first, the rest left
    /// out; a point that is not a number is left out.
    pub fn resolved(&self) -> Resolved {
        let mut monitors: Vec<Point> = self
            .monitors
            .iter()
            .filter_map(|m| self.point(*m))
            .collect();
        monitors.sort_by_key(|p| p.metres);
        Resolved {
            target: self.target.and_then(|a| self.point(a)),
            from: self.from.and_then(|a| self.point(a)),
            monitors: std::array::from_fn(|i| monitors.get(i).copied()),
            gizmo: self
                .axes
                .map(|a| std::array::from_fn(|i| (a.origin, a.ends[i]))),
        }
    }
}

impl Resolved {
    /// The marks the view shows.
    pub fn marks(&self) -> Marks {
        Marks {
            target: self.target.map(|p| p.metres),
            from: self.from.map(|p| p.metres),
            monitors: self.monitors.map(|m| m.map(|p| p.metres)),
            gizmo: self.gizmo.is_some(),
        }
    }
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
    view.marks = p.points.resolved().marks();
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
    /// The points and poses last given to the kit.
    last_marks: Option<Resolved>,
    teaching: Teaching,
    sent: Option<(String, u32)>,
}

impl Live {
    fn tick(&mut self) {
        let p = lock(&PUBLISHED).clone().unwrap_or_default();
        let (values, notice) = frame(&p, &mut self.teaching, &mut self.sent, &buttons());
        // A monitor has no shield, weapon or grenades: the retail widgets
        // are off while it is up, the motion tracker kept.
        self.handle.retail(USER, !p.view.monitor);
        if self.last.as_ref() != Some(&values) {
            self.handle.publish(values.clone());
            self.last = Some(values);
        }
        if let Some(line) = notice {
            self.handle.notify(USER, &line);
        }
        // The marks follow the session every tick: they move with the
        // monitor's eye and the held thing.
        let marks = (p.view.monitor)
            .then(|| p.points.resolved())
            .unwrap_or_default();
        if self.last_marks != Some(marks) {
            give(&self.handle, &marks);
            self.last_marks = Some(marks);
        }
    }
}

/// Gives the kit the points of the navpoints and the poses of the gizmo's
/// segments; an element with none has its point or pose taken away.
pub fn give(handle: &Handle, marks: &Resolved) {
    let at = |p: &Option<Point>| p.map(|p| p.at);
    handle.navpoint_at(kit::NAV_TARGET, at(&marks.target));
    handle.navpoint_at(kit::NAV_FROM, at(&marks.from));
    for (id, m) in kit::NAV_MONITORS.into_iter().zip(&marks.monitors) {
        handle.navpoint_at(id, at(m));
    }
    for (i, id) in kit::GIZMO.into_iter().enumerate() {
        let pose: Option<Pose> = marks.gizmo.map(|g| kit::gizmo_pose(g[i].0, g[i].1));
        handle.world_mark(id, pose);
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
        last_marks: None,
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
