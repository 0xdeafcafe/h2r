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
/// A label sits this far under its mark's point: below the box, not
/// over the ring and chevron the game's navpoint draws above the seam.
pub const LABEL_DROP: f32 = 0.25;
/// Marks whose points lie within this of each other (world units) are
/// one seam's: only the first of them with a label keeps it.
pub const SEAM_RADIUS: f32 = 0.75;

pub use gfx::world_lines::{Block, HUD_BLOCKS, label_under_hud};
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

/// The game's colour of each team (`0..world::TEAMS`), `None` for a team
/// it gives none (docs/game/hud-kit.md, "Colours").
pub type Teams = [Option<[f32; 3]>; world::TEAMS as usize];

/// A mark's colour and alpha for its style on the HUD colour `hud`:
/// `Normal` and a flash as it is, `Dim` at [`DIM_ALPHA`], `Inactive` grey
/// at [`INACTIVE_ALPHA`], `Go` the go green (`world::GO_GREEN`), `Axis(n)` axis `n`'s colour
/// (`world::AXIS_COLOURS`), `Team(n)`
/// team `n`'s colour in `teams`, or the HUD colour where there is none.
pub fn colour(style: Style, hud: [f32; 3], teams: &Teams) -> ([f32; 3], f32) {
    match style {
        Style::Normal | Style::Flash(_) => (hud, 1.0),
        Style::Dim => (hud, DIM_ALPHA),
        Style::Inactive => {
            let luma = 0.299 * hud[0] + 0.587 * hud[1] + 0.114 * hud[2];
            ([luma; 3], INACTIVE_ALPHA)
        }
        Style::Go => (world::GO_GREEN, 1.0),
        Style::Axis(n) => (
            world::AXIS_COLOURS[usize::from(n) % world::AXIS_COLOURS.len()],
            1.0,
        ),
        Style::Team(n) => (
            teams.get(usize::from(n)).copied().flatten().unwrap_or(hud),
            1.0,
        ),
    }
}

/// The lines of `marks` in the HUD colour `hud` and the team colours
/// `teams`, labels in `face`. A mark
/// whose shape gives no lines at its pose, or whose strength is 0, is
/// left out. A seam has one label: of the marks within [`SEAM_RADIUS`]
/// of each other only the first with a label keeps it, below its point
/// ([`LABEL_DROP`]).
pub fn lines(marks: &[MarkNow], hud: [f32; 3], teams: &Teams, face: LabelFace) -> Vec<MarkLines> {
    let mut labelled: Vec<Vec3> = Vec::new();
    marks
        .iter()
        .filter_map(|m| {
            let edges = world::edges(m.shape, &m.pose);
            let strength = m.pose.strength.clamp(0.0, 1.0);
            if edges.is_empty() || strength <= 0.0 {
                return None;
            }
            let (color, alpha) = colour(m.style, hud, teams);
            let at = m.pose.at;
            let mut label = Some(m.label.clone()).filter(|l| !l.is_empty());
            if label.is_some() {
                if labelled.iter().any(|p| near(*p, at)) {
                    label = None;
                } else {
                    labelled.push(at);
                }
            }
            Some(MarkLines {
                id: m.id,
                edges,
                anchor: [at[0], at[1], at[2] - LABEL_DROP],
                color,
                label,
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

/// Whether two points are one seam's.
fn near(a: Vec3, b: Vec3) -> bool {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= SEAM_RADIUS * SEAM_RADIUS
}

/// Registers the kit's world-line source before boot (windowed hosts).
#[cfg(feature = "gpu")]
pub use gpu::register;

#[cfg(feature = "gpu")]
mod gpu {
    use std::sync::Arc;

    use guest_layout::GuestMemory;
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
            // The map's team colours are read only for a mark that has one.
            let mut teams: Teams = [None; world::TEAMS as usize];
            for m in &marks {
                if let Style::Team(n) = m.style
                    && let Some(slot) = teams.get_mut(usize::from(n))
                {
                    *slot = crate::pad_lightbar::team_rgb(guest, n)
                        .map(|c| c.map(|v| f32::from(v) / 255.0));
                }
            }
            let lines = lines(&marks, look.colour, &teams, look.face);
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
    use hud_kit_core::world::{GO_GREEN, Pose, Shape};

    const HUD: [f32; 3] = [0.3, 0.6, 0.9];
    const NO_TEAMS: Teams = [None; world::TEAMS as usize];

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
                mark(Shape::Bracket, Style::Go, "Will bounce", pose.strength(0.5)),
                mark(Shape::Arrow, Style::Dim, "", pose),
            ],
            HUD,
            &NO_TEAMS,
            LabelFace::Hud,
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].edges, world::edges(Shape::Bracket, &pose));
        assert_eq!((out[0].color, out[0].style.alpha, out[0].label.as_deref()), (HUD, 1.0, None));
        assert_eq!(out[0].style.label_face, LabelFace::Hud);
        assert!(out[0].style.fade.is_none(), "a mark shows from any distance");
        assert_eq!(out[1].color, GO_GREEN);
        assert_eq!(out[1].style.alpha, 0.5);
        assert_eq!(out[1].label.as_deref(), Some("Will bounce"));
        assert_eq!(out[1].anchor, [1.0, 2.0, 3.0 - LABEL_DROP]);
        assert_eq!((out[2].color, out[2].style.alpha), (HUD, DIM_ALPHA));
        assert_eq!(out[2].edges.len(), 3);
    }

    /// A gizmo's three axes are segments between their two world points,
    /// each in its axis' colour; one with no far end is left out.
    #[test]
    fn a_gizmo_axis_is_a_segment_in_its_colour() {
        let from = [1.0, 2.0, 3.0];
        let seg = |axis: u8, to: [f32; 3]| {
            mark(
                Shape::Segment,
                Style::Axis(axis),
                "",
                Pose::at(from, [0.0, 0.0]).to(to),
            )
        };
        let out = lines(
            &[
                seg(0, [1.5, 2.0, 3.0]),
                seg(1, [1.0, 2.5, 3.0]),
                seg(2, [1.0, 2.0, 3.5]),
                mark(Shape::Segment, Style::Axis(0), "", Pose::at(from, [0.0, 0.0])),
            ],
            HUD,
            &NO_TEAMS,
            LabelFace::Hud,
        );
        assert_eq!(out.len(), 3);
        for (i, m) in out.iter().enumerate() {
            assert_eq!(m.color, world::AXIS_COLOURS[i]);
            assert_eq!(m.edges.len(), 1);
            assert_eq!(m.edges[0][0], from, "the lines are not lifted");
        }
        assert_eq!(out[2].edges[0][1], [1.0, 2.0, 3.5]);
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
            &NO_TEAMS,
            LabelFace::Hud,
        );
        assert!(none.is_empty());
        let (grey, alpha) = colour(Style::Inactive, HUD, &NO_TEAMS);
        assert!(grey[0] == grey[1] && grey[1] == grey[2] && alpha == INACTIVE_ALPHA);
    }

    /// A seam has one label, below its point: of marks on one seam the
    /// first with a label keeps it, the others' are left off; a mark
    /// on another seam keeps its own.
    #[test]
    fn a_seam_has_one_label_below_the_box() {
        let seam = Pose::at([0.0, 0.0, 1.0], [0.3, 0.1]).heading([1.0, 0.0]);
        let near_by = Pose::at([0.2, 0.1, 1.0], [0.06, 0.0]);
        let far = Pose::at([5.0, 0.0, 1.0], [0.3, 0.1]).heading([1.0, 0.0]);
        let out = lines(
            &[
                mark(Shape::Bracket, Style::Normal, "Jump now", seam),
                mark(Shape::Pipper, Style::Normal, "Land, 7 m", near_by),
                mark(Shape::Bracket, Style::Normal, "Too shallow", far),
            ],
            HUD,
            &NO_TEAMS,
            LabelFace::Hud,
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].label.as_deref(), Some("Jump now"));
        assert_eq!(out[0].anchor[2], 1.0 - LABEL_DROP, "below the box");
        assert_eq!(out[1].label, None, "the seam's second label is left off");
        assert_eq!(out[2].label.as_deref(), Some("Too shallow"));
        // A labelless mark does not take the seam's label.
        let out = lines(
            &[
                mark(Shape::Pipper, Style::Normal, "", near_by),
                mark(Shape::Bracket, Style::Normal, "Walk to line", seam),
            ],
            HUD,
            &NO_TEAMS,
            LabelFace::Hud,
        );
        assert_eq!(out[1].label.as_deref(), Some("Walk to line"));
    }

    /// A label that projects into the HUD's top-left or bottom-left block
    /// is left out; the middle, the right and a label off the frame are
    /// not under it.
    #[test]
    fn a_label_under_the_huds_corners_is_left_out() {
        assert!(label_under_hud([0.1, 0.05]), "top left");
        assert!(label_under_hud([0.3, 0.9]), "bottom left");
        assert!(!label_under_hud([0.5, 0.5]));
        assert!(!label_under_hud([0.9, 0.9]), "bottom right");
        assert!(!label_under_hud([0.9, 0.05]), "top right");
        assert!(!label_under_hud([0.1, 0.45]), "left, between the blocks");
        assert!(!label_under_hud([-0.5, 0.9]), "off the frame");
        assert!(!label_under_hud([f32::NAN, 0.9]));
    }

    // ---- K5: team colours and lines ----

    /// A `Team(n)` mark is in the game's colour of team `n`, in full
    /// alpha; a team the game gives no colour is in the HUD colour, and a
    /// team past the game's too. A line is one segment.
    #[test]
    fn a_team_mark_is_in_its_teams_colour() {
        let pose = Pose::at([0.0; 3], [0.5, 0.0]);
        let mut teams: Teams = [None; world::TEAMS as usize];
        teams[1] = Some([0.9, 0.1, 0.2]);
        let out = lines(
            &[
                mark(Shape::Line, Style::Team(1), "Spawn", pose),
                mark(Shape::Line, Style::Team(2), "", pose),
                mark(Shape::Line, Style::Team(200), "", pose),
            ],
            HUD,
            &teams,
            LabelFace::Hud,
        );
        assert_eq!(out.len(), 3);
        assert_eq!((out[0].color, out[0].style.alpha), ([0.9, 0.1, 0.2], 1.0));
        assert_eq!(out[0].edges.len(), 1);
        assert_eq!(out[0].label.as_deref(), Some("Spawn"));
        assert_eq!((out[1].color, out[2].color), (HUD, HUD));
    }
}
