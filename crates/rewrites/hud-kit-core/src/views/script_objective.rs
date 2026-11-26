//! The scripted objective HUD (docs/game/hud-kit.md, "The scripted
//! objective HUD"): what a script's `objective`, `objective_mark`,
//! `meter` and `countdown` (capability `hud.objective`) show of the
//! viewer's team.
//!
//! - `so.banner`: the objective line as the message routine's timed
//!   centred line, which the kit starts again when the text changes: a
//!   phase change.
//! - `so.line`: the same text as a resting line, top right under the clock.
//! - `so.clock`: `m:ss`, flashing with the help flash under
//!   [`LAST_SECONDS`].
//! - `so.meter`: the retail strip under the grenade row, filled by the
//!   meter's value of its whole.
//! - `so.nav.1` to `so.nav.3`: the navpoints over the marked zones, each
//!   with its distance in metres.
//!
//! The banner and the clock show in split screen; the rest are
//! fullscreen only. A viewer with nothing set sees none of it.
//!
//! The data is `content/modes/huds/script-objective.hud.yaml`. [`View`] is
//! what the HUD shows; the game (`game::objective_hud`) makes it from the
//! VM's state, so every machine that runs the script draws the same.

use crate::model::{Describe, Hud, Pulse, Style, Value, Values, banner_fault};
use crate::views::objective::{MARK_MAX, clock};

pub const NAME: &str = "script-objective";

pub const BANNER: &str = "so.banner";
pub const CLOCK: &str = "so.clock";
pub const LINE: &str = "so.line";
pub const METER: &str = "so.meter";
pub const NAVS: [&str; 3] = ["so.nav.1", "so.nav.2", "so.nav.3"];

/// The elements' ids, in draw order.
pub const IDS: [&str; 7] = [BANNER, CLOCK, LINE, METER, NAVS[0], NAVS[1], NAVS[2]];

/// The clock flashes with this many seconds or fewer left.
pub const LAST_SECONDS: u32 = 30;

/// What a marked zone asks of the viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Attack,
    Defend,
}

/// A marked zone as the viewer sees it: its kind and how far it is, in
/// metres, when the viewer has a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub kind: MarkKind,
    pub metres: Option<u16>,
}

/// What the HUD shows, for one viewer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// The objective line.
    pub text: Option<String>,
    /// Whole seconds left on the clock, rounded up.
    pub seconds: Option<u32>,
    /// The gauge's value and what it is of.
    pub meter: Option<(i32, i32)>,
    /// The marked zones, the first three drawn.
    pub marks: Vec<Mark>,
}

impl View {
    /// Nothing is set: no HUD is drawn.
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.seconds.is_none() && self.meter.is_none() && self.marks.is_empty()
    }
}

/// Whole seconds a clock of `ticks` shows: rounded up, so it reaches 0:00
/// with the round.
pub fn seconds_of(ticks: u32, ticks_per_second: u32) -> u32 {
    ticks.div_ceil(ticks_per_second.max(1))
}

/// The elements, in draw order ([`IDS`]).
pub fn hud() -> Hud {
    crate::data::builtin(NAME)
}

/// Every element hidden.
pub fn hidden() -> Values {
    let mut out = Values::default();
    for id in IDS {
        out.set(id, Value::hidden());
    }
    out
}

/// The gauge's fill: its value of its whole, 0 to 1.
pub fn fill_of(value: i32, of: i32) -> f32 {
    if of <= 0 {
        return 0.0;
    }
    (f64::from(value.clamp(0, of)) / f64::from(of)) as f32
}

/// The values for `v`. A text the banner cannot take is left out of it.
pub fn values(v: &View) -> Values {
    let mut out = hidden();
    if let Some(t) = &v.text {
        out.set(LINE, Value::text(t.clone()));
        if banner_fault(t).is_none() {
            out.set(BANNER, Value::text(t.clone()));
        }
    }
    if let Some(s) = v.seconds {
        let style = if s <= LAST_SECONDS { Style::Flash(Pulse::Help) } else { Style::Normal };
        out.set(CLOCK, Value::text(clock(s)).styled(style));
    }
    if let Some((value, of)) = v.meter.filter(|&(_, of)| of > 0) {
        out.set(METER, Value::fill(fill_of(value, of)));
    }
    for (id, m) in NAVS.into_iter().zip(&v.marks) {
        if let Some(d) = m.metres {
            out.set(id, Value::text(d.min(MARK_MAX).to_string()));
        }
    }
    out
}

/// The named states: nothing set, a phase's start, a site being held, the
/// last seconds, two marks, and the widest.
pub fn views() -> Vec<(&'static str, View)> {
    let line = "Phase 1 of 3: take a site";
    let mark = |kind, metres| Mark { kind, metres: Some(metres) };
    vec![
        ("idle", View::default()),
        (
            "phase",
            View {
                text: Some(line.into()),
                seconds: Some(300),
                marks: vec![mark(MarkKind::Attack, 120), mark(MarkKind::Attack, 85)],
                ..View::default()
            },
        ),
        (
            "capturing",
            View {
                text: Some(line.into()),
                seconds: Some(212),
                meter: Some((14, 40)),
                marks: vec![mark(MarkKind::Attack, 4)],
            },
        ),
        (
            "defending",
            View {
                text: Some("Hold the sites".into()),
                seconds: Some(130),
                meter: Some((33, 40)),
                marks: vec![mark(MarkKind::Defend, 18)],
            },
        ),
        (
            "last-30",
            View {
                text: Some("Phase 3 of 3: take the final site".into()),
                seconds: Some(29),
                meter: Some((38, 40)),
                marks: vec![mark(MarkKind::Attack, 9)],
            },
        ),
        (
            "widest",
            View {
                text: Some("Phase 3 of 3: take the final site".into()),
                seconds: Some(99 * 60 + 59),
                meter: Some((40, 40)),
                marks: vec![
                    mark(MarkKind::Attack, 999),
                    mark(MarkKind::Defend, 999),
                    mark(MarkKind::Attack, 999),
                ],
            },
        ),
    ]
}

/// The HUD's description.
pub struct ScriptObjective;

impl Describe for ScriptObjective {
    fn hud(&self) -> Hud {
        hud()
    }

    fn states(&self) -> Vec<(&'static str, Values)> {
        views().into_iter().map(|(name, v)| (name, values(&v))).collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Some("a script's objective runs on multiplayer maps; a campaign player HUD may have no grenade row")
    }

    /// The gauge sits in the grenade row's next place, as the other
    /// games' pills do, so it never meets theirs.
    fn never_with(&self) -> &'static [&'static str] {
        &[
            super::firefight::NAME,
            super::forge::NAME,
            super::forge_readout::NAME,
            super::superbounce::NAME,
            super::superbounce_readout::NAME,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mode;
    use crate::model::Origin;

    fn shown(v: &Values, id: &str) -> Option<String> {
        v.get(id).filter(|x| x.shown).map(|x| x.text.clone())
    }

    fn state(name: &str) -> View {
        views().into_iter().find(|(n, _)| *n == name).unwrap().1
    }

    /// Nothing set draws nothing.
    #[test]
    fn idle_shows_nothing() {
        let v = values(&state("idle"));
        assert!(state("idle").is_empty());
        for id in IDS {
            assert!(!v.get(id).unwrap().shown, "{id}");
        }
        assert_eq!(v, hidden());
    }

    /// A phase's start: the line twice (banner and resting), the clock
    /// steady, a navpoint for each mark and no gauge.
    #[test]
    fn a_phase_shows_the_line_the_clock_and_the_marks() {
        let v = values(&state("phase"));
        assert_eq!(shown(&v, BANNER).as_deref(), Some("Phase 1 of 3: take a site"));
        assert_eq!(shown(&v, LINE).as_deref(), Some("Phase 1 of 3: take a site"));
        assert_eq!(shown(&v, CLOCK).as_deref(), Some("5:00"));
        assert_eq!(v.get(CLOCK).unwrap().style, Style::Normal);
        assert_eq!(shown(&v, NAVS[0]).as_deref(), Some("120"));
        assert_eq!(shown(&v, NAVS[1]).as_deref(), Some("85"));
        assert!(!v.get(NAVS[2]).unwrap().shown);
        assert!(!v.get(METER).unwrap().shown);
    }

    /// A site being held fills the gauge by value of whole; a whole of 0
    /// or less lifts it; a value past either end is clamped.
    #[test]
    fn capturing_fills_the_gauge() {
        let v = values(&state("capturing"));
        let m = v.get(METER).unwrap();
        assert!(m.shown);
        assert!((m.fill - 0.35).abs() < 1e-6, "{}", m.fill);
        assert_eq!(shown(&v, NAVS[0]).as_deref(), Some("4"));
        assert_eq!(fill_of(50, 40), 1.0);
        assert_eq!(fill_of(-3, 40), 0.0);
        let off = values(&View { meter: Some((5, 0)), ..View::default() });
        assert!(!off.get(METER).unwrap().shown);
    }

    /// The clock flashes with the help flash from 30 s down and not
    /// above; whole seconds round up so the clock ends with the round.
    #[test]
    fn the_last_30_seconds_flash() {
        let flash = Style::Flash(Pulse::Help);
        let at = |s| {
            let v = values(&View { seconds: Some(s), ..View::default() });
            (shown(&v, CLOCK).unwrap(), v.get(CLOCK).unwrap().style)
        };
        assert_eq!(at(31), ("0:31".to_owned(), Style::Normal));
        assert_eq!(at(30), ("0:30".to_owned(), flash));
        assert_eq!(at(0), ("0:00".to_owned(), flash));
        assert_eq!(values(&state("last-30")).get(CLOCK).unwrap().style, flash);
        assert_eq!(seconds_of(900, 30), 30);
        assert_eq!(seconds_of(901, 30), 31);
        assert_eq!(seconds_of(0, 30), 0);
        assert_eq!(shown(&values(&state("widest")), CLOCK).as_deref(), Some("99:59"));
    }

    /// Split screen keeps the banner and the clock; the line, gauge and
    /// navpoints are fullscreen only. Every state is the one layout.
    #[test]
    fn split_screen_keeps_the_banner_and_the_clock() {
        let h = hud();
        assert_eq!(h.problems(), Vec::<String>::new());
        assert_eq!(h.elements.iter().map(|e| e.id).collect::<Vec<_>>(), IDS);
        for id in IDS {
            let e = h.element(id).unwrap();
            let split = matches!(id, BANNER | CLOCK);
            for m in [Mode::Half, Mode::Quarter] {
                assert_eq!(e.at.offset(m).is_some(), split, "{id} {m:?}");
            }
            assert!(e.at.offset(Mode::Full).is_some(), "{id}");
        }
        assert_eq!(h.element(METER).unwrap().at.origin, Origin::NextGrenadePill);
        assert!(h.uses_grenade_row() && h.has_text());
        for (name, v) in ScriptObjective.states() {
            assert_eq!(v.layout_problems(&h), Vec::<String>::new(), "{name}");
        }
        assert_eq!(hidden().layout_problems(&h), Vec::<String>::new());
        assert!(ScriptObjective.multiplayer_only().is_some());
    }

    /// A line the banner cannot take is a resting line alone; the
    /// distances stop at the kit's widest.
    #[test]
    fn a_long_line_is_not_a_banner() {
        let long = "x".repeat(crate::model::BANNER_UNITS + 1);
        let v = values(&View { text: Some(long.clone()), ..View::default() });
        assert!(!v.get(BANNER).unwrap().shown);
        assert_eq!(shown(&v, LINE), Some(long));
        let far = values(&View {
            marks: vec![Mark { kind: MarkKind::Defend, metres: Some(u16::MAX) }],
            ..View::default()
        });
        assert_eq!(shown(&far, NAVS[0]).as_deref(), Some("999"));
        // A mark with no unit to measure from has no navpoint.
        let none = values(&View {
            marks: vec![Mark { kind: MarkKind::Attack, metres: None }],
            ..View::default()
        });
        assert!(!none.get(NAVS[0]).unwrap().shown);
    }
}
