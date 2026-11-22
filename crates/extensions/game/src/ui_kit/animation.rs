//! Tick-driven transition samples for the UI builder. The game plays the
//! native pane and list tracks selected by the same page preference, held
//! to the same open and close lengths ([`OPEN_TICKS`], [`CLOSE_TICKS`]).

use std::fmt;

use serde::Serialize;

use crate::ui_kit::view::{Layer, View};

/// How the class chooses its animation tracks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionStyle {
    /// The source pane, list and skin supply the native tracks.
    #[default]
    Default,
    /// Disable the page transition. Native list selection still uses its skin.
    Off,
    /// Use these 1-based native animation slots for the pane and list.
    /// Zero disables that track. The builder samples representative motion
    /// for enabled tracks.
    Retail { pane: u16, rows: u16 },
}

impl TransitionStyle {
    pub fn is_default(&self) -> bool {
        *self == Self::Default
    }
}

/// A page's position on the builder's transition timeline, in UI ticks
/// since it started opening or closing. At rest the layout is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    In(u32),
    Rest,
    Out(u32),
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::In(tick) => write!(f, "in-{tick}"),
            Self::Rest => f.write_str("rest"),
            Self::Out(tick) => write!(f, "out-{tick}"),
        }
    }
}

/// Frames that show both directions and the settled page.
pub const SAMPLES: [Phase; 5] = [
    Phase::In(0),
    Phase::In(6),
    Phase::Rest,
    Phase::Out(4),
    Phase::Out(14),
];

/// How long a page of ours takes to open and to close, in the builder's
/// ticks: the kit's budgets. The game holds a screen of ours to the same
/// time ([`Transition::millis`], `crate::ui_kit::lifecycle::cap_transition`),
/// so the editor's animation player ends where the game does. Closing is
/// quicker than opening: a page leaves as soon as it is dismissed.
pub const OPEN_TICKS: u32 = 10;
pub const CLOSE_TICKS: u32 = 6;

// The parts of each: the pane's fade and slide, and each row's after a
// stagger of one tick per visible slot (capped), the selection a tick
// after its row coming in and a tick before it going out.
const ENTER_TICKS: u32 = OPEN_TICKS;
const ROW_ENTER_TICKS: u32 = 6;
const STAGGER_ENTER: usize = 3;
const LEAVE_TICKS: u32 = CLOSE_TICKS;
const ROW_LEAVE_TICKS: u32 = 4;
const STAGGER_LEAVE: usize = 2;
const _: () = assert!(STAGGER_ENTER as u32 + ROW_ENTER_TICKS < OPEN_TICKS);
const _: () = assert!(STAGGER_LEAVE as u32 + ROW_LEAVE_TICKS <= CLOSE_TICKS);

/// The builder's ticks per second, which [`Transition::millis`] turns the
/// budgets into the game's milliseconds with.
pub const TICKS_PER_SECOND: u32 = 30;

/// A transition the builder samples: a page opening ([`Phase::In`]) or
/// closing ([`Phase::Out`]). Focus moves and page changes play the skin's
/// native tracks in game and are not sampled here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    Open,
    Close,
}

impl Transition {
    pub const ALL: [Transition; 2] = [Transition::Open, Transition::Close];

    /// `open`, `close`: the agent API's names.
    pub fn name(self) -> &'static str {
        match self {
            Transition::Open => "open",
            Transition::Close => "close",
        }
    }

    /// The transition named `name` (`open`/`in`, `close`/`out`).
    pub fn named(name: &str) -> Option<Transition> {
        match name.to_ascii_lowercase().as_str() {
            "open" | "in" => Some(Transition::Open),
            "close" | "out" => Some(Transition::Close),
            _ => None,
        }
    }

    /// Ticks from its start until every part has settled: the pane's
    /// track, or the last staggered row and the selection after it,
    /// whichever is longer ([`OPEN_TICKS`], [`CLOSE_TICKS`]).
    pub fn ticks(self) -> u32 {
        match self {
            Transition::Open => ENTER_TICKS.max(STAGGER_ENTER as u32 + ROW_ENTER_TICKS + 1),
            Transition::Close => LEAVE_TICKS.max(STAGGER_LEAVE as u32 + ROW_LEAVE_TICKS),
        }
    }

    /// Its length in the game, in milliseconds: the most a screen of ours
    /// spends coming in or going out (`crate::ui_kit::lifecycle::cap_transition`
    /// holds the screen's delay and length to it). The same time as
    /// [`Transition::ticks`] at [`TICKS_PER_SECOND`].
    pub fn millis(self) -> u32 {
        self.ticks() * 1000 / TICKS_PER_SECOND
    }

    /// Its pose `tick` ticks in (past the end it stays settled).
    pub fn phase(self, tick: u32) -> Phase {
        let tick = tick.min(self.ticks());
        match self {
            Transition::Open => Phase::In(tick),
            Transition::Close => Phase::Out(tick),
        }
    }

    /// `n` ticks spread evenly from its start to its end, both included
    /// (one frame is the start).
    pub fn frames(self, n: usize) -> Vec<u32> {
        let end = self.ticks();
        match n {
            0 => Vec::new(),
            1 => vec![0],
            n => (0..n)
                .map(|i| (i as u32 * end + (n as u32 - 1) / 2) / (n as u32 - 1))
                .collect(),
        }
    }

    /// Seconds at `tick`.
    pub fn seconds(tick: u32) -> f32 {
        tick as f32 / TICKS_PER_SECOND as f32
    }
}

#[derive(Clone, Copy, Debug)]
enum Part {
    Pane,
    Row(usize),
    Selection(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pose {
    alpha: u8,
    shift: [i16; 2],
}

struct Frame {
    phase: Phase,
    stagger: bool,
    pane: Pose,
    rows: usize,
}

impl Frame {
    fn new(style: TransitionStyle, modal: bool, rows: usize, phase: Phase) -> Self {
        let alpha = match style {
            TransitionStyle::Off | TransitionStyle::Retail { pane: 0, .. } => 255,
            _ => visibility(phase, 0, ENTER_TICKS, LEAVE_TICKS),
        };
        let hidden = i16::from(255 - alpha);
        let shift = if modal {
            [0, -6 * hidden / 255]
        } else {
            [-20 * hidden / 255, 0]
        };
        Self {
            phase,
            stagger: !matches!(
                style,
                TransitionStyle::Off | TransitionStyle::Retail { rows: 0, .. }
            ),
            pane: Pose { alpha, shift },
            rows,
        }
    }

    fn pose(&self, part: Part) -> Pose {
        let (slot, selected) = match part {
            Part::Pane => return self.pane,
            Part::Row(slot) => (slot, false),
            Part::Selection(slot) => (slot, true),
        };
        if !self.stagger {
            return self.pane;
        }
        // Each item moves with its pane, while its own alpha depends on
        // its visible slot. The selection art settles just after its row.
        let ordinal = match self.phase {
            Phase::In(_) | Phase::Rest => slot,
            Phase::Out(_) => self.rows.saturating_sub(1).saturating_sub(slot),
        };
        let stagger = match self.phase {
            Phase::In(_) | Phase::Rest => STAGGER_ENTER,
            Phase::Out(_) => STAGGER_LEAVE,
        };
        let delay = ordinal.min(stagger) as u32;
        let delay = match self.phase {
            Phase::In(_) | Phase::Rest => delay + u32::from(selected),
            Phase::Out(_) => delay.saturating_sub(u32::from(selected)),
        };
        let row = visibility(self.phase, delay, ROW_ENTER_TICKS, ROW_LEAVE_TICKS);
        Pose {
            alpha: mix_alpha(self.pane.alpha, row),
            ..self.pane
        }
    }
}

// Smoothstep evaluated entirely in integer arithmetic. The same tick
// always yields the same opacity and the exact end points.
fn visibility(phase: Phase, delay: u32, enter: u32, leave: u32) -> u8 {
    let (tick, duration, closing) = match phase {
        Phase::In(tick) => (tick, enter, false),
        Phase::Out(tick) => (tick, leave, true),
        Phase::Rest => return 255,
    };
    let elapsed = tick.saturating_sub(delay).min(duration);
    let t = elapsed * 255 / duration;
    let progress = (t * t * (3 * 255 - 2 * t) / (255 * 255)) as u8;
    if closing { 255 - progress } else { progress }
}

fn mix_alpha(base: u8, fade: u8) -> u8 {
    ((u16::from(base) * u16::from(fade) + 127) / 255) as u8
}

#[derive(Clone, Copy)]
struct RestWidget {
    rect: [i16; 4],
    alpha: u8,
    part: Part,
}

/// An animated copy of a page's geometry. Capture source positions and
/// classify widgets once. Seeking in either direction rewrites the same
/// view and never allocates or mutates the opener's view.
pub struct AnimatedView {
    view: View,
    rest: Vec<RestWidget>,
    style: TransitionStyle,
    modal: bool,
    rows: usize,
}

fn row_index(id: &str) -> Option<usize> {
    id.strip_prefix("row.")?.split('.').next()?.parse().ok()
}

impl AnimatedView {
    pub fn new(view: View, style: TransitionStyle, modal: bool) -> Self {
        let rows = || {
            view.widgets
                .iter()
                .filter_map(|widget| row_index(&widget.id))
        };
        let first = rows().min().unwrap_or(0);
        let count = rows()
            .max()
            .map_or(0, |last| (last - first).saturating_add(1));
        let focus = view.focus.as_deref().and_then(row_index);
        let rest = view
            .widgets
            .iter()
            .map(|widget| {
                let part = match row_index(&widget.id) {
                    Some(i) if Some(i) == focus && widget.layer == Layer::Under => {
                        Part::Selection(i - first)
                    }
                    Some(i) => Part::Row(i - first),
                    None => Part::Pane,
                };
                RestWidget {
                    rect: widget.rect,
                    alpha: widget.alpha,
                    part,
                }
            })
            .collect();
        Self {
            view,
            rest,
            style,
            modal,
            rows: count,
        }
    }

    /// Sample `phase` and return the same view with its new pose.
    pub fn seek(&mut self, phase: Phase) -> &View {
        let frame = Frame::new(self.style, self.modal, self.rows, phase);
        for (widget, rest) in self.view.widgets.iter_mut().zip(&self.rest) {
            let Pose {
                alpha,
                shift: [x, y],
            } = frame.pose(rest.part);
            let [top, left, bottom, right] = rest.rect;
            widget.rect = [
                top.saturating_add(y),
                left.saturating_add(x),
                bottom.saturating_add(y),
                right.saturating_add(x),
            ];
            widget.alpha = mix_alpha(rest.alpha, alpha);
        }
        &self.view
    }

    pub fn into_view(self) -> View {
        self.view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::{preview::From, screens};

    #[test]
    fn pane_rows_and_selection_reach_their_endpoints_in_order() {
        let style = TransitionStyle::Default;
        let opening = Frame::new(style, false, 3, Phase::In(4));
        let pane = opening.pose(Part::Pane);
        let first = opening.pose(Part::Row(0));
        let last = opening.pose(Part::Row(2));
        let selection = opening.pose(Part::Selection(0));
        assert!(first.alpha > last.alpha && last.alpha > 0);
        assert!(selection.alpha < first.alpha);
        assert_eq!(first.shift, pane.shift);
        assert_eq!(last.shift, pane.shift);
        assert_eq!(selection.shift, first.shift);
        assert!(pane.shift[0] < 0);

        let closing = Frame::new(style, false, 3, Phase::Out(4));
        assert!(closing.pose(Part::Row(0)).alpha > closing.pose(Part::Row(2)).alpha);
        for (phase, alpha) in [
            (Phase::In(0), 0),
            (Phase::In(14), 255),
            (Phase::Out(0), 255),
            (Phase::Out(14), 0),
        ] {
            let frame = Frame::new(style, false, 12, phase);
            assert_eq!(frame.pose(Part::Pane).alpha, alpha);
            assert_eq!(frame.pose(Part::Selection(11)).alpha, alpha);
        }
    }

    #[test]
    fn a_modal_moves_its_panel_and_text_without_moving_the_parent() {
        let mut spec = screens::sample("kit.dialog.large").unwrap();
        spec.underlay = Some(Box::new(screens::sample("kit.example").unwrap()));
        let mut animated = From::Synthetic.animated_view(&spec, "sample").unwrap();
        let resting = animated.seek(Phase::Rest).clone();
        let storage = animated.seek(Phase::Rest).widgets.as_ptr();

        let opening = animated.seek(Phase::In(6));
        assert_eq!(opening.underlay, resting.underlay);
        assert_eq!(opening.widgets.as_ptr(), storage);
        let panel = |view: &View| view.widget("panel.0").unwrap().rect[0];
        let title = |view: &View| view.widget("title").unwrap().rect[0];
        assert!(panel(opening) < panel(&resting));
        assert_eq!(
            title(opening) - title(&resting),
            panel(opening) - panel(&resting)
        );
        let opening = opening.clone();

        let closed = animated.seek(Phase::Out(14));
        assert!(closed.widgets.iter().all(|w| w.alpha == 0));
        assert_eq!(closed.underlay, resting.underlay);
        assert_eq!(animated.seek(Phase::In(6)), &opening);
        assert_eq!(animated.seek(Phase::Rest), &resting);
    }

    #[test]
    fn page_preferences_can_disable_the_pane_and_row_tracks() {
        let off = Frame::new(TransitionStyle::Off, false, 3, Phase::In(0));
        assert_eq!(
            off.pose(Part::Pane),
            Pose {
                alpha: 255,
                shift: [0, 0],
            }
        );
        assert_eq!(off.pose(Part::Selection(2)), off.pose(Part::Pane));

        let rows_only = Frame::new(
            TransitionStyle::Retail { pane: 0, rows: 1 },
            false,
            3,
            Phase::In(0),
        );
        assert_eq!(rows_only.pose(Part::Pane), off.pose(Part::Pane));
        assert_eq!(rows_only.pose(Part::Row(0)).alpha, 0);
    }

    #[test]
    fn playback_overlay_samples_include_the_transport() {
        let spec = screens::sample(crate::theater_ui::OVERLAY).unwrap();
        let mut animated = From::Synthetic.animated_view(&spec, "playing").unwrap();
        let opacity = |view: &View| view.widget("transport.time").unwrap().alpha;
        let opening = opacity(animated.seek(Phase::In(6)));
        let rest = opacity(animated.seek(Phase::Rest));
        assert!(opening > 0 && opening < rest);
        assert_eq!(opacity(animated.seek(Phase::Out(14))), 0);
    }

    #[test]
    fn a_transition_spans_until_every_part_settles() {
        assert_eq!(Transition::named("in"), Some(Transition::Open));
        assert_eq!(Transition::named("Close"), Some(Transition::Close));
        assert_eq!(Transition::named("focus"), None);
        for t in Transition::ALL {
            assert_eq!(Transition::named(t.name()), Some(t));
            let frames = t.frames(5);
            assert_eq!(frames.len(), 5);
            assert_eq!(frames[0], 0);
            assert_eq!(*frames.last().unwrap(), t.ticks());
            assert!(frames.windows(2).all(|w| w[0] < w[1]), "{frames:?}");
            assert_eq!(t.phase(t.ticks() + 50), t.phase(t.ticks()));
            // The last staggered row and its selection are settled at the end.
            let end = Frame::new(TransitionStyle::Default, false, 12, t.phase(t.ticks()));
            let settled = if t == Transition::Open { 255 } else { 0 };
            assert_eq!(end.pose(Part::Selection(11)).alpha, settled);
            assert_eq!(end.pose(Part::Row(0)).alpha, settled);
        }
        assert_eq!(Transition::Open.frames(1), vec![0]);
        assert!(Transition::Close.frames(0).is_empty());
        assert_eq!(Transition::seconds(TICKS_PER_SECOND), 1.0);
    }

    /// The budgets the game holds a screen of ours to: closing quicker
    /// than opening, and both quick (a third of a second to open, a fifth
    /// to close). The editor's player plays the same lengths.
    #[test]
    fn closing_is_quicker_than_opening_and_both_are_the_games_lengths() {
        assert_eq!(Transition::Open.ticks(), OPEN_TICKS);
        assert_eq!(Transition::Close.ticks(), CLOSE_TICKS);
        assert_eq!(Transition::Open.millis(), 333);
        assert_eq!(Transition::Close.millis(), 200);
        assert!(Transition::Close.millis() * 3 <= Transition::Open.millis() * 2);
        for t in Transition::ALL {
            let seconds = Transition::seconds(t.ticks());
            assert!((seconds * 1000.0 - t.millis() as f32).abs() < 1.0, "{t:?}");
        }
        // Gone at the end of the close: nothing of the page still drawn.
        let end = Frame::new(TransitionStyle::Default, true, 11, Phase::Out(CLOSE_TICKS));
        for part in [Part::Pane, Part::Row(0), Part::Row(10), Part::Selection(5)] {
            assert_eq!(end.pose(part).alpha, 0, "{part:?}");
        }
    }

    #[test]
    fn stagger_uses_visible_slots_in_a_scrolled_list() {
        let mut spec = screens::sample("kit.example").unwrap();
        let row = spec.rows[0].clone();
        spec.rows.resize(30, row);
        spec.focus = spec.rows.len() - 1;
        let mut animated = From::Synthetic.animated_view(&spec, "scrolled").unwrap();
        let first_visible_alpha = |view: &View| {
            view.widgets
                .iter()
                .find(|w| w.id.starts_with("row.") && w.id.ends_with(".label"))
                .unwrap()
                .alpha
        };
        assert!(first_visible_alpha(animated.seek(Phase::In(4))) > 0);
    }
}
