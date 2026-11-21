//! Tick-driven transition samples for the UI builder. The game uses the
//! native pane and list tracks selected by the same page preference.

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

const ENTER_TICKS: u32 = 12;
const LEAVE_TICKS: u32 = 9;
const ROW_TICKS: u32 = 7;
const STAGGER_TICKS: usize = 6;

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
        let delay = ordinal.min(STAGGER_TICKS) as u32;
        let delay = match self.phase {
            Phase::In(_) | Phase::Rest => delay + u32::from(selected),
            Phase::Out(_) => delay.saturating_sub(u32::from(selected)),
        };
        let row = visibility(self.phase, delay, ROW_TICKS, ROW_TICKS);
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
        let opacity = |view: &View| view.widget("transport.0").unwrap().alpha;
        let opening = opacity(animated.seek(Phase::In(6)));
        let rest = opacity(animated.seek(Phase::Rest));
        assert!(opening > 0 && opening < rest);
        assert_eq!(opacity(animated.seek(Phase::Out(14))), 0);
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
