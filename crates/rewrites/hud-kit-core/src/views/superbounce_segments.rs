//! The charge segments (docs/game/hud-kit.md, "The charge segments"):
//! nine arcs round the motion tracker's left, bottom and right sides,
//! three a side, concentric with its rim and [`GAP`] clear of it. They are
//! the priming indicator: they fill from the top of the left side, round
//! the bottom and up the right as the recipe's steps are done, and wipe
//! back the other way as the charge runs out.
//!
//! ```text
//!        .---.
//!  1 (  /     \  ) 9
//!  2 ( | track | ) 8
//!  3  \ \     / /  7
//!      \ '---' /
//!       ' ___ '
//!       4  5  6
//! ```
//!
//! They are elements of the readout HUD ([`super::superbounce_readout`]),
//! fullscreen only as it is. Each is a gauge (`Kind::Gauge`): a text
//! widget whose one character is the arc's picture for its fill, drawn in
//! code and served in the fonts (`font::ring`), in the HUD's own text
//! colour. A fill has `font::ring::STEPS` steps, so the arc at the edge
//! of the fill is part lit and the wipe moves along it.
//!
//! [`Stage`] names what they show and [`segments`] maps the mode's charge
//! (`superbounce_charge::ChargePhase`, as [`super::superbounce::Phase`])
//! onto it:
//!
//! | Stage | Charge | Lit |
//! |---|---|---|
//! | `Idle` | none: not primed, and not by a crouch spot | none |
//! | `Near` | `Near`: crouched by a crouch spot, the capsule fits | none |
//! | `Under` | `Under`: crouched under the lip, crouch held | the left side, 3 |
//! | `Held` | `Held`: crouch let go, the forced crouch | the left and the bottom, 6; the right side flashes, the next to light |
//! | `Primed` | `Primed`, over half the charge left | all 9 less the still ticks over 38, wiped from the right side's top |
//! | `Warning` | `Primed`, half the charge or less (19 still ticks) | what is left, flashing the timer's warning |
//! | `Lost` | `Lost`: the word left -3 | none; all nine blink and go |

use font::ring;

use crate::model::{Glyphs, Pulse, Style, Value, Values};
use crate::motion::Motion;

use super::superbounce::{Charge, LOW_CHARGE, Phase};

/// How many segments, and how many a side.
pub const SEGMENTS: usize = ring::SEGMENTS;
pub const SIDE: usize = ring::SIDE;
/// The segments, in fill order: down the left side, along the bottom
/// left to right, up the right side.
pub const IDS: [&str; SEGMENTS] = [
    "sbr.seg.1",
    "sbr.seg.2",
    "sbr.seg.3",
    "sbr.seg.4",
    "sbr.seg.5",
    "sbr.seg.6",
    "sbr.seg.7",
    "sbr.seg.8",
    "sbr.seg.9",
];

/// The motion tracker's rim fullscreen, screen pixels: left, top, right,
/// bottom (`hud_kit::rects::TRACKER`, which a test holds this to). The
/// arcs' geometry is in this rectangle's pixels (`font::ring`), whose
/// rim is as wide.
pub const TRACKER: [i16; 4] = [47, 360, 130, 443];
/// Pixels between the rim and an arc's inner edge: clearly apart from the
/// ring, and off the tracker's blips (`font::ring::GAP`).
pub const GAP: i16 = ring::GAP as i16;
/// The fullscreen frame's bottom-left corner, the segments' anchor
/// (`hud_kit::rects::BOTTOM_LEFT`).
pub const CORNER: (i16, i16) = (48, 444);

/// Each segment's picture's top-left corner fullscreen, screen pixels, in
/// fill order: its box in the rim's pixels (`font::ring::BOXES`) from the
/// rim's corner.
pub const AT: [(i16, i16); SEGMENTS] = {
    let mut at = [(0, 0); SEGMENTS];
    let mut i = 0;
    while i < SEGMENTS {
        at[i] = (TRACKER[0] + ring::BOXES[i][0], TRACKER[1] + ring::BOXES[i][1]);
        i += 1;
    }
    at
};
/// Each segment's picture's size, wide and tall.
pub const SIZE: [(i16, i16); SEGMENTS] = {
    let mut size = [(0, 0); SEGMENTS];
    let mut i = 0;
    while i < SEGMENTS {
        let (w, h) = ring::size(i);
        size[i] = (w as i16, h as i16);
        i += 1;
    }
    size
};

/// How far outside the HUD frame the segments sit (`Element::outside`):
/// the left side is left of the frame's left edge (x 48) and the bottom
/// under its foot (y 444), where the tracker's rim is on both. They are
/// in the screen's margin, as nothing of the retail HUD is.
pub const OUTSIDE: i16 = {
    let mut out = 0;
    let mut i = 0;
    while i < SEGMENTS {
        let left = CORNER.0 - AT[i].0;
        let under = AT[i].1 + SIZE[i].1 - CORNER.1;
        if left > out {
            out = left;
        }
        if under > out {
            out = under;
        }
        i += 1;
    }
    out
};
/// The segments' right edge fullscreen, screen pixels: the readout's
/// rows start right of it.
pub const RIGHT_EDGE: i16 = {
    let mut edge = 0;
    let mut i = 0;
    while i < SEGMENTS {
        let right = AT[i].0 + SIZE[i].0;
        if right > edge {
            edge = right;
        }
        i += 1;
    }
    edge
};

/// Segment `index`'s pictures: the arc's run of `font::ring::STEPS` + 1
/// characters, the empty track first.
pub const fn glyphs(index: usize) -> Glyphs {
    Glyphs {
        first: ring::unit(index, 0),
        steps: ring::STEPS,
        size: SIZE[index % SEGMENTS],
    }
}

/// What the segments show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// No charge and no recipe step: the empty ring.
    Idle,
    /// Crouched by a crouch spot: no step done yet.
    Near,
    /// Under the lip with crouch held: the first step done.
    Under,
    /// Crouch let go, forced to stay crouched: the second step done.
    Held,
    /// Primed, with more than [`LOW_CHARGE`] of the charge left.
    Primed,
    /// Primed and about to lose it: [`LOW_CHARGE`] or less left.
    Warning,
    /// The charge just went.
    Lost,
}

/// The segments at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segments {
    pub stage: Stage,
    /// How many segments are lit, 0 to [`SEGMENTS`]; a fraction is the
    /// edge segment part lit.
    pub lit: f32,
}

/// The segments for the mode's charge (`None`: no charge shows).
pub fn segments(charge: Option<Charge>) -> Segments {
    let at = |stage, lit| Segments { stage, lit };
    let Some(c) = charge else {
        return at(Stage::Idle, 0.0);
    };
    match c.phase {
        Phase::Near => at(Stage::Near, 0.0),
        Phase::Under => at(Stage::Under, SIDE as f32),
        Phase::Held => at(Stage::Held, 2.0 * SIDE as f32),
        Phase::Primed => {
            let fill = if c.fill.is_finite() { c.fill.clamp(0.0, 1.0) } else { 0.0 };
            let stage = if fill <= LOW_CHARGE { Stage::Warning } else { Stage::Primed };
            at(stage, fill * SEGMENTS as f32)
        }
        Phase::Lost => at(Stage::Lost, 0.0),
    }
}

/// Segment `index`'s fill for `lit` segments lit: 1 under the edge, 0
/// past it, the fraction on it (drawn as the nearest of the arc's steps,
/// `Glyphs::step`).
pub fn fill_of(index: usize, lit: f32) -> f32 {
    (lit - index as f32).clamp(0.0, 1.0)
}

/// Segment `index`'s value for `s`.
///
/// - Every stage but `Lost`: the fill, eased to (`Motion::Ease`), so a
///   step lights its side and the charge wipes at the same rate at any
///   frame rate.
/// - `Held`: the right side, the next to light, flashes the help record.
/// - `Warning`: the lit segments flash the timer's warning record.
/// - `Lost`: empty, in the timer's expired record, blinking and gone
///   (`Motion::Blink`).
pub fn value(index: usize, s: &Segments) -> Value {
    let fill = fill_of(index, s.lit);
    let eased = Value::fill(fill).moving(Motion::Ease);
    match s.stage {
        Stage::Lost => Value::fill(0.0)
            .styled(Style::Flash(Pulse::TimeExpired))
            .moving(Motion::Blink),
        Stage::Held if index >= 2 * SIDE => eased.styled(Style::Flash(Pulse::Help)),
        Stage::Warning if fill > 0.0 => eased.styled(Style::Flash(Pulse::TimeWarning)),
        _ => eased,
    }
}

/// Sets the nine segments' values for `s` (`None`: all hidden).
pub fn set_values(out: &mut Values, s: Option<&Segments>) {
    for (i, id) in IDS.iter().enumerate() {
        out.set(id, s.map_or_else(Value::hidden, |s| value(i, s)));
    }
}

/// The preview's moments of the segments: a name and the charge it
/// shows.
pub fn samples() -> Vec<(&'static str, Option<Charge>)> {
    let charge = |phase, fill| Some(Charge { phase, fill });
    vec![
        ("segments-idle", None),
        ("segments-near", charge(Phase::Near, 0.0)),
        ("segments-under", charge(Phase::Under, 1.0 / 3.0)),
        ("segments-held", charge(Phase::Held, 2.0 / 3.0)),
        ("segments-primed", charge(Phase::Primed, 1.0)),
        ("segments-draining", charge(Phase::Primed, 0.75)),
        ("segments-warning", charge(Phase::Primed, LOW_CHARGE)),
        ("segments-lost", charge(Phase::Lost, 0.0)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Anchor, Kind};

    fn fills(s: &Segments) -> [f32; SEGMENTS] {
        std::array::from_fn(|i| value(i, s).fill)
    }

    fn charge(phase: Phase, fill: f32) -> Option<Charge> {
        Some(Charge { phase, fill })
    }

    /// The recipe's steps light a side each, left then bottom then
    /// right; primed is all nine.
    #[test]
    fn the_steps_light_the_left_the_bottom_then_the_right() {
        let lit = |c| segments(c).lit;
        assert_eq!(segments(None), Segments { stage: Stage::Idle, lit: 0.0 });
        assert_eq!(segments(charge(Phase::Near, 0.0)).stage, Stage::Near);
        assert_eq!(lit(charge(Phase::Near, 0.0)), 0.0);
        assert_eq!(lit(charge(Phase::Under, 1.0 / 3.0)), 3.0);
        assert_eq!(lit(charge(Phase::Held, 2.0 / 3.0)), 6.0);
        assert_eq!(lit(charge(Phase::Primed, 1.0)), 9.0);
        let one = |n: usize| -> [f32; SEGMENTS] { std::array::from_fn(|i| f32::from(i < n)) };
        assert_eq!(fills(&segments(charge(Phase::Under, 1.0 / 3.0))), one(3));
        assert_eq!(fills(&segments(charge(Phase::Held, 2.0 / 3.0))), one(6));
        assert_eq!(fills(&segments(charge(Phase::Primed, 1.0))), one(9));
        assert_eq!(fills(&segments(None)), one(0));
    }

    /// The charge wipes back from the right side's top: the edge segment
    /// is part lit, those past it empty.
    #[test]
    fn the_charge_wipes_back_from_the_right() {
        // 29 of 38 ticks left.
        let s = segments(charge(Phase::Primed, 29.0 / 38.0));
        assert_eq!(s.stage, Stage::Primed);
        let f = fills(&s);
        assert_eq!(f[..6], [1.0; 6]);
        assert!(f[6] > 0.8 && f[6] < 0.9, "{f:?}");
        assert_eq!(f[7..], [0.0; 2]);
        // Each still tick takes the same off the edge.
        let a = segments(charge(Phase::Primed, 30.0 / 38.0)).lit;
        assert!((a - s.lit - 9.0 / 38.0).abs() < 1e-5);
        // Nothing left: the empty ring, still the warning's.
        let none = segments(charge(Phase::Primed, 0.0));
        assert_eq!((none.stage, none.lit), (Stage::Warning, 0.0));
        // A fill that is not a number is none.
        assert_eq!(segments(charge(Phase::Primed, f32::NAN)).lit, 0.0);
        assert_eq!(segments(charge(Phase::Primed, 7.0)).lit, 9.0);
    }

    /// The warning flashes what is left, the held step the side to come,
    /// and a loss blinks all nine empty.
    #[test]
    fn the_warning_and_the_loss_flash() {
        let style = |s: &Segments, i| value(i, s).style;
        let warn = segments(charge(Phase::Primed, LOW_CHARGE));
        assert_eq!(warn.stage, Stage::Warning);
        assert_eq!(style(&warn, 0), Style::Flash(Pulse::TimeWarning));
        assert_eq!(style(&warn, 4), Style::Flash(Pulse::TimeWarning));
        assert_eq!(style(&warn, 5), Style::Normal, "empty: not lit, not flashing");
        let over = segments(charge(Phase::Primed, LOW_CHARGE + 0.01));
        assert_eq!((over.stage, style(&over, 0)), (Stage::Primed, Style::Normal));
        let held = segments(charge(Phase::Held, 2.0 / 3.0));
        assert_eq!(style(&held, 5), Style::Normal);
        assert_eq!(style(&held, 6), Style::Flash(Pulse::Help));
        assert_eq!(style(&held, 8), Style::Flash(Pulse::Help));
        let lost = segments(charge(Phase::Lost, 0.0));
        for i in 0..SEGMENTS {
            let v = value(i, &lost);
            assert_eq!(
                (v.shown, v.fill, v.style, v.motion),
                (true, 0.0, Style::Flash(Pulse::TimeExpired), Motion::Blink)
            );
        }
        // Every other value eases.
        assert_eq!(value(0, &warn).motion, Motion::Ease);
        assert_eq!(value(8, &segments(None)).motion, Motion::Ease);
    }

    /// Three arcs a side round the tracker's left, bottom and right, each
    /// concentric with the rim and `GAP` clear of it, their boxes apart,
    /// none over the rim; the same from the left and from the right.
    #[test]
    fn the_segments_are_round_the_tracker_and_clear_of_it() {
        let [l, t, r, b] = TRACKER;
        // The rim the arcs are drawn round is the tracker's: as wide, and
        // the arcs' pixels are counted from its corner.
        assert_eq!(f32::from(r - l), 2.0 * ring::RIM);
        assert_eq!(f32::from(b - t), 2.0 * ring::RIM);
        assert_eq!((GAP, SEGMENTS, SIDE), (4, 9, 3));
        assert_eq!(
            AT,
            [
                (40, 384),
                (40, 404),
                (45, 422),
                (62, 439),
                (80, 446),
                (100, 439),
                (120, 422),
                (130, 404),
                (131, 384)
            ]
        );
        let centre = (f32::from(l + r) / 2.0, f32::from(t + b) / 2.0);
        for (i, ((x, y), (w, h))) in AT.into_iter().zip(SIZE).enumerate() {
            assert!(w > 0 && h > 0, "segment {}", i + 1);
            assert!(y >= t + 24, "segment {}: 13 under the shield meter's band (y 371)", i + 1);
            assert!(x >= 0 && x + w <= 640 && y + h <= 480, "on the screen");
            // Its mirror about the tracker's middle is a segment too, in
            // the mirrored order.
            let (m, ms) = (AT[SEGMENTS - 1 - i], SIZE[SEGMENTS - 1 - i]);
            assert_eq!(((l + r - (x + w), y), (w, h)), (m, ms), "segment {}", i + 1);
            // No ink of the full picture is on the rim or inside it: every
            // inked pixel is GAP from the rim, less the edge's one pixel.
            let p = glyphs(i).picture(1.0).unwrap();
            assert_eq!((i32::from(p.width), i32::from(p.height)), (i32::from(w), i32::from(h)));
            for (n, px) in p.pixels.iter().enumerate() {
                if px >> 12 == 0 {
                    continue;
                }
                let px_x = f32::from(x) + (n % usize::from(p.width)) as f32 + 0.5;
                let px_y = f32::from(y) + (n / usize::from(p.width)) as f32 + 0.5;
                let from_centre = (px_x - centre.0).hypot(px_y - centre.1);
                assert!(
                    from_centre > ring::RIM + ring::GAP - 1.0,
                    "segment {}: ink {from_centre} from the centre",
                    i + 1
                );
                assert!(from_centre < ring::OUTER + 1.0, "segment {}", i + 1);
            }
        }
        for i in 0..SEGMENTS {
            for j in i + 1..SEGMENTS {
                let ((ax, ay), (aw, ah)) = (AT[i], SIZE[i]);
                let ((cx, cy), (cw, ch)) = (AT[j], SIZE[j]);
                let apart = ax + aw <= cx || cx + cw <= ax || ay + ah <= cy || cy + ch <= ay;
                assert!(apart, "{} and {}", i + 1, j + 1);
            }
        }
        assert_eq!((OUTSIDE, RIGHT_EDGE), (8, 137));
        // The elements (the readout's YAML): gauges of the arcs' pictures,
        // from the frame's bottom-left corner, fullscreen only, let outside
        // the frame.
        let e: Vec<_> = crate::views::superbounce_readout::hud()
            .elements
            .into_iter()
            .filter(|e| e.id.starts_with("sbr.seg."))
            .collect();
        assert_eq!(e.iter().map(|e| e.id).collect::<Vec<_>>(), IDS);
        for (i, (e, (x, y))) in e.iter().zip(AT).enumerate() {
            assert_eq!(e.at.anchor, Anchor::BottomLeft);
            assert_eq!(e.at.full, (x - 48, y - 444));
            assert_eq!((e.at.half, e.at.quarter, e.outside), (None, None, OUTSIDE));
            let g = glyphs(i);
            assert_eq!(e.kind, Kind::Gauge { glyphs: g });
            assert_eq!((g.first, g.steps, g.size), (ring::unit(i, 0), ring::STEPS, SIZE[i]));
            // A fill is the arc's nearest step: empty, half, full.
            assert_eq!(g.unit(0.0), ring::unit(i, 0));
            assert_eq!(g.unit(0.5), ring::unit(i, ring::STEPS / 2));
            assert_eq!(g.unit(1.0), ring::unit(i, ring::STEPS));
            assert_eq!(g.unit(f32::NAN), ring::unit(i, 0));
            assert_eq!(g.text(1.0).chars().count(), 1);
            assert_eq!(g.units().count(), usize::from(ring::STEPS) + 1);
            assert_eq!(g.picture(0.5).unwrap().pixels, ring::draw(i, ring::STEPS / 2));
        }
        // The lowest is 6 under the frame's foot and the leftmost 8 left of
        // it, inside what is let.
        assert_eq!(AT[4].1 + SIZE[4].1 - CORNER.1, 6);
        assert_eq!(CORNER.0 - AT[0].0, 8);
    }

    /// The preview's moments: one for each stage, and a wipe.
    #[test]
    fn the_samples_are_every_stage() {
        let stages: Vec<_> = samples().iter().map(|(_, c)| segments(*c).stage).collect();
        assert_eq!(
            stages,
            [
                Stage::Idle,
                Stage::Near,
                Stage::Under,
                Stage::Held,
                Stage::Primed,
                Stage::Primed,
                Stage::Warning,
                Stage::Lost
            ]
        );
        let mut out = Values::default();
        set_values(&mut out, None);
        assert!(out.0.len() == SEGMENTS && out.0.iter().all(|(_, v)| !v.shown));
        set_values(&mut out, Some(&segments(None)));
        assert!(out.0.len() == SEGMENTS && out.0.iter().all(|(_, v)| v.shown));
    }
}
