//! The HUD kit's world marks in the game (docs/game/hud-kit.md, "World
//! marks"): the marks of the registered HUDs that draw now
//! ([`hud_kit::world_marks_now`]) as lines for the window's world-line
//! pass, which draws them under the HUD with no depth test, from each
//! view's camera.
//!
//! [`lines`] is pure: each mark's shape at its pose
//! (`hud_kit_core::world::edges`, the same segments the preview
//! projects), in the user's HUD colour at the value's style and the
//! pose's strength, with the value's text as its label. The source
//! ([`register`], windowed hosts) captures them once a present.

use gfx::world_lines::{Glow, LabelFace, LineStyle};
use hud_kit_core::model::Style;
use hud_kit_core::world::{self, Edge, Vec3};

use crate::hud_kit::MarkNow;

/// A mark's line width, pixels at 1080p, and its glow.
pub const WIDTH: f32 = 2.5;
pub const GLOW: Glow = Glow {
    width: 3.0,
    alpha: 0.35,
};
/// A `Dim` mark's alpha, as a fraction of a `Normal` one's, and an
/// `Inactive` one's: the preview's (`hud_kit::preview::DIM_ALPHA`,
/// `INACTIVE_ALPHA`).
pub const DIM_ALPHA: f32 = 0.5;
pub const INACTIVE_ALPHA: f32 = 0.7;
/// A label sits this far over its mark's point.
pub const LABEL_RAISE: f32 = 0.25;
/// The labels' size: a font pixel is a HUD pixel (`LabelFace::Hud` at 2).
pub const LABEL_SCALE: f32 = 2.0;

/// One mark's lines.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkLines {
    pub id: &'static str,
    pub edges: Vec<Edge>,
    /// Where its label is centred.
    pub anchor: Vec3,
    pub color: [f32; 3],
    pub label: Option<String>,
    pub style: LineStyle,
}

/// A mark's colour and alpha for its style on the HUD colour `hud`:
/// `Normal` and a flash as it is, `Dim` at [`DIM_ALPHA`], `Inactive` grey
/// at [`INACTIVE_ALPHA`], `Lock` the lock's red (`world::LOCK_RED`).
pub fn colour(style: Style, hud: [f32; 3]) -> ([f32; 3], f32) {
    match style {
        Style::Normal | Style::Flash(_) => (hud, 1.0),
        Style::Dim => (hud, DIM_ALPHA),
        Style::Inactive => {
            let luma = 0.299 * hud[0] + 0.587 * hud[1] + 0.114 * hud[2];
            ([luma; 3], INACTIVE_ALPHA)
        }
        Style::Lock => (world::LOCK_RED, 1.0),
    }
}

/// The lines of `marks` in the HUD colour `hud`, labels in `face`. A mark
/// whose shape gives no lines at its pose, or whose strength is 0, is
/// left out.
pub fn lines(marks: &[MarkNow], hud: [f32; 3], face: LabelFace) -> Vec<MarkLines> {
    marks
        .iter()
        .filter_map(|m| {
            let edges = world::edges(m.shape, &m.pose);
            let strength = m.pose.strength.clamp(0.0, 1.0);
            if edges.is_empty() || strength <= 0.0 {
                return None;
            }
            let (color, alpha) = colour(m.style, hud);
            let at = m.pose.at;
            Some(MarkLines {
                id: m.id,
                edges,
                anchor: [at[0], at[1], at[2] + LABEL_RAISE],
                color,
                label: Some(m.label.clone()).filter(|l| !l.is_empty()),
                style: LineStyle {
                    width: WIDTH,
                    alpha: alpha * strength,
                    glow: Some(GLOW),
                    label_scale: LABEL_SCALE,
                    label_centered: true,
                    label_face: face,
                    ..LineStyle::default()
                },
            })
        })
        .collect()
}

/// Registers the kit's world-line source before boot (windowed hosts).
#[cfg(feature = "gpu")]
pub use gpu::register;

#[cfg(feature = "gpu")]
mod gpu {
    use std::sync::Arc;

    use engine::GuestMemory;
    use hud_kit_core::Mode;

    use super::*;
    use crate::hud_kit;
    use crate::world_lines::{ImageMarks, WorldLine, WorldLineSource};

    /// Draws the registered HUDs' world marks.
    pub struct Source;

    pub fn register(host: &mut crate::Host) {
        host.add_world_line_source(Arc::new(Source));
    }

    impl WorldLineSource for Source {
        fn active(&self) -> bool {
            hud_kit::any_world_marks()
        }

        fn capture(&self, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let mode = hud_kit::mode(guest).unwrap_or(Mode::Full);
            let marks = hud_kit::world_marks_now(mode);
            if marks.is_empty() {
                return None;
            }
            let look = crate::superbounce_font::look(guest);
            let lines = lines(&marks, look.colour, look.face);
            (!lines.is_empty()).then(|| Arc::new(Sample(lines)) as Arc<dyn ImageMarks>)
        }
    }

    #[derive(Debug)]
    struct Sample(Vec<MarkLines>);

    impl ImageMarks for Sample {
        fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
            for m in &self.0 {
                visitor(WorldLine {
                    edges: &m.edges,
                    anchor: m.anchor,
                    color: m.color,
                    dashed: false,
                    label: m.label.as_deref(),
                    style: m.style,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hud_kit_core::world::{LOCK_RED, Pose, Shape};

    const HUD: [f32; 3] = [0.3, 0.6, 0.9];

    fn mark(shape: Shape, style: Style, label: &str, pose: Pose) -> MarkNow {
        MarkNow {
            id: "m",
            shape,
            pose,
            style,
            label: label.into(),
        }
    }

    /// A mark's lines are its shape's at its pose, in the HUD colour, with
    /// its label over its point; the style and the pose's strength set
    /// the colour and the alpha.
    #[test]
    fn a_marks_lines_are_its_shapes_in_the_hud_colour() {
        let pose = Pose::at([1.0, 2.0, 3.0], [0.3, 0.1]).heading([0.0, 1.0]);
        let out = lines(
            &[
                mark(Shape::Bracket, Style::Normal, "", pose),
                mark(Shape::Bracket, Style::Lock, "Will bounce", pose.strength(0.5)),
                mark(Shape::Arrow, Style::Dim, "", pose),
            ],
            HUD,
            LabelFace::Hud,
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].edges, world::edges(Shape::Bracket, &pose));
        assert_eq!((out[0].color, out[0].style.alpha, out[0].label.as_deref()), (HUD, 1.0, None));
        assert_eq!(out[0].style.label_face, LabelFace::Hud);
        assert!(out[0].style.fade.is_none(), "a mark shows from any distance");
        assert_eq!(out[1].color, LOCK_RED);
        assert_eq!(out[1].style.alpha, 0.5);
        assert_eq!(out[1].label.as_deref(), Some("Will bounce"));
        assert_eq!(out[1].anchor, [1.0, 2.0, 3.0 + LABEL_RAISE]);
        assert_eq!((out[2].color, out[2].style.alpha), (HUD, DIM_ALPHA));
        assert_eq!(out[2].edges.len(), 3);
    }

    /// A mark with nothing to draw is left out: no size, no strength.
    #[test]
    fn a_mark_with_nothing_to_draw_is_left_out() {
        let pose = Pose::at([0.0; 3], [0.3, 0.1]);
        let none = lines(
            &[
                mark(Shape::Ticks, Style::Normal, "", Pose::at([0.0; 3], [0.0, 0.0])),
                mark(Shape::Bracket, Style::Normal, "", pose.strength(0.0)),
            ],
            HUD,
            LabelFace::Hud,
        );
        assert!(none.is_empty());
        let (grey, alpha) = colour(Style::Inactive, HUD);
        assert!(grey[0] == grey[1] && grey[1] == grey[2] && alpha == INACTIVE_ALPHA);
    }
}
