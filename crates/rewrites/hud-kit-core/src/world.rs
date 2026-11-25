//! World marks (docs/game/hud-kit.md, "World marks"): elements of a HUD
//! that lie on the level's geometry, not on the screen. A
//! [`Kind::WorldMark`](crate::model::Kind::WorldMark) element has a
//! [`Shape`]; where it lies, which way it is turned and how big it is are
//! its [`Pose`], which the runtime gives each tick as it gives a navpoint
//! its point, and the preview takes from the HUD's samples
//! (`Describe::world`).
//!
//! [`edges`] turns a shape at a pose into line segments in world space.
//! The game's runtime hands them to the window's world-line pass and the
//! preview projects them onto its canvas, so both draw the same lines.
//! Everything here is flat on the floor, [`LIFT`] above the pose's point.

/// A world point: X, Y, and Z up.
pub type Vec3 = [f32; 3];
/// One line segment.
pub type Edge = [Vec3; 2];

/// Marks sit this far above their point, clear of the floor they lie on.
pub const LIFT: f32 = 0.02;
/// Segments of a pipper's ring.
pub const PIPPER_SEGMENTS: usize = 12;
/// A pipper's centre cross: each arm this fraction of the ring's radius.
pub const PIPPER_CROSS: f32 = 0.3;
/// A bracket's corner arms: this fraction of the box's shorter half.
pub const BRACKET_ARM: f32 = 0.5;
/// How many closure ticks a [`Shape::Ticks`] draws.
pub const TICKS: usize = 3;
/// The drawn part of each tick's slot of the run; the rest is the gap to
/// the next.
pub const TICK_DUTY: f32 = 0.55;
/// A bar's band ends: this fraction of the bar's length.
pub const BAR_END: f32 = 0.5;
/// The go green: the one colour a world mark has beside the user's HUD
/// colour, for "this fall will bounce" (docs/game/superbounce.md, "The
/// approach guide"). [`Style::Go`](crate::model::Style::Go) draws in it.
pub const GO_GREEN: [f32; 3] = [0.35, 0.95, 0.45];

/// How many axes a gizmo has: the axes [`Style::Axis`](crate::model::Style::Axis)
/// can name (`0..AXES`), X, Y and Z.
pub const AXES: u8 = 3;
/// The colour of each axis, X, Y and Z: the red, green and blue every
/// editor draws them in. S: ours, not a colour the game's HUD holds.
pub const AXIS_COLOURS: [[f32; 3]; AXES as usize] = [
    [0.95, 0.30, 0.25],
    [0.35, 0.90, 0.35],
    [0.35, 0.55, 0.98],
];

/// How many teams a game has: the teams [`Style::Team`](crate::model::Style::Team)
/// can name (`0..TEAMS`), the rows of the team colours the game reads
/// (docs/game/hud-kit.md, "Colours").
pub const TEAMS: u8 = 8;

/// What a world mark draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    /// An impact marker: a small ring of radius `half[0]` round the point
    /// with a cross at its centre.
    Pipper,
    /// The four corner brackets of a box round the point, `half[0]` along
    /// the pose's heading and `half[1]` across it.
    Bracket,
    /// An arrow centred on the point, pointing along the heading,
    /// `half[0]` each way; its head's wings are `half[1]` out.
    Arrow,
    /// [`TICKS`] short dashes from the point along the heading, over a
    /// run of `half[0]`: closure cues, shorter as the run closes.
    Ticks,
    /// A line on the floor across the heading, `half[1]` either side of
    /// the point; with `half[0]` over 0, the two ends of a band
    /// `half[0]` either side of it along the heading, as shorter lines
    /// ([`BAR_END`] of its length).
    Bar,
    /// One segment, centred on the point, `half[0]` each way along the
    /// heading: an edge of an outline, as a hill's or a territory's.
    Line,
    /// One segment between two world points, [`Pose::at`] and
    /// [`Pose::to`], at any slope: an axis of a gizmo, drawn where it
    /// lies and not [`LIFT`] above a floor. It has no heading and no size.
    Segment,
}

impl Shape {
    /// The shapes that lie flat on the floor at a point, which share
    /// their reach and lift; [`Shape::Segment`] is the one that does not.
    pub const ALL: [Shape; 6] = [
        Shape::Pipper,
        Shape::Bracket,
        Shape::Arrow,
        Shape::Ticks,
        Shape::Bar,
        Shape::Line,
    ];

    /// The name the JSON and the findings use.
    pub fn name(self) -> &'static str {
        match self {
            Shape::Pipper => "pipper",
            Shape::Bracket => "bracket",
            Shape::Arrow => "arrow",
            Shape::Ticks => "ticks",
            Shape::Bar => "bar",
            Shape::Line => "line",
            Shape::Segment => "segment",
        }
    }
}

/// Where a world mark lies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// The point on the geometry.
    pub at: Vec3,
    /// The heading, flat (X and Y): a bracket's long side, an arrow's and
    /// the ticks' direction. It need not be a unit; no direction is +X.
    pub along: [f32; 2],
    /// The size, world units: what each [`Shape`] says of it.
    pub half: [f32; 2],
    /// 0 to 1: how strongly it draws (its lines' alpha), for a mark that
    /// fades in and out.
    pub strength: f32,
    /// The far end of a [`Shape::Segment`], whose near end is `at`; no
    /// other shape reads it. `None` is a segment of no length.
    pub to: Option<Vec3>,
}

impl Pose {
    /// At `at`, heading +X, full strength.
    pub const fn at(at: Vec3, half: [f32; 2]) -> Pose {
        Pose {
            at,
            along: [1.0, 0.0],
            half,
            strength: 1.0,
            to: None,
        }
    }
    /// The far end of a [`Shape::Segment`].
    pub const fn to(mut self, to: Vec3) -> Pose {
        self.to = Some(to);
        self
    }
    pub const fn heading(mut self, along: [f32; 2]) -> Pose {
        self.along = along;
        self
    }
    pub const fn strength(mut self, strength: f32) -> Pose {
        self.strength = strength;
        self
    }

    /// Why the pose cannot be drawn, if it cannot: a number that is not
    /// finite, a negative size, a strength outside 0 to 1.
    pub fn fault(&self) -> Option<String> {
        let finite = self
            .at
            .iter()
            .chain(&self.along)
            .chain(&self.half)
            .chain(self.to.iter().flatten())
            .all(|v| v.is_finite());
        if !finite || !self.strength.is_finite() {
            Some("a number that is not finite".into())
        } else if self.half.iter().any(|h| *h < 0.0) {
            Some(format!("a negative size {:?}", self.half))
        } else if !(0.0..=1.0).contains(&self.strength) {
            Some(format!("strength {} outside 0 to 1", self.strength))
        } else {
            None
        }
    }
}

/// `v` as a unit; +X for no direction.
pub fn unit(v: [f32; 2]) -> [f32; 2] {
    let len = v[0].hypot(v[1]);
    if len > 1e-6 && len.is_finite() {
        [v[0] / len, v[1] / len]
    } else {
        [1.0, 0.0]
    }
}

fn offset(p: Vec3, d: [f32; 2], by: f32) -> Vec3 {
    [p[0] + d[0] * by, p[1] + d[1] * by, p[2]]
}

/// The line segments of `shape` at `pose`, in world space, flat and
/// [`LIFT`] above the pose's point. None for a pose with a
/// [`Pose::fault`], or with no size.
///
/// - [`Shape::Pipper`]: [`PIPPER_SEGMENTS`] ring edges, then the cross's
///   two.
/// - [`Shape::Bracket`]: eight edges, two arms at each corner running
///   toward the middle, [`BRACKET_ARM`] of the shorter half long.
/// - [`Shape::Arrow`]: the shaft, then the head's two wings, each from
///   its wing to the tip.
/// - [`Shape::Ticks`]: [`TICKS`] dashes, the first starting at the point,
///   each [`TICK_DUTY`] of a third of the run.
/// - [`Shape::Bar`]: the line across the heading through the point, then
///   with a band its near and far ends, each [`BAR_END`] as long.
/// - [`Shape::Line`]: one edge, from `half[0]` behind the point to
///   `half[0]` ahead of it.
/// - [`Shape::Segment`]: one edge from the point to [`Pose::to`], as
///   given: not lifted, none where `to` is unset or is the point.
pub fn edges(shape: Shape, pose: &Pose) -> Vec<Edge> {
    if pose.fault().is_some() {
        return Vec::new();
    }
    let c = [pose.at[0], pose.at[1], pose.at[2] + LIFT];
    let u = unit(pose.along);
    let n = [-u[1], u[0]];
    let [a, b] = pose.half;
    match shape {
        Shape::Pipper => {
            if a <= 0.0 {
                return Vec::new();
            }
            let point = |i: usize| {
                let t = std::f32::consts::TAU * (i % PIPPER_SEGMENTS) as f32
                    / PIPPER_SEGMENTS as f32;
                [c[0] + a * t.cos(), c[1] + a * t.sin(), c[2]]
            };
            let mut out: Vec<Edge> = (0..PIPPER_SEGMENTS)
                .map(|i| [point(i), point(i + 1)])
                .collect();
            let arm = a * PIPPER_CROSS;
            out.push([offset(c, u, -arm), offset(c, u, arm)]);
            out.push([offset(c, n, -arm), offset(c, n, arm)]);
            out
        }
        Shape::Bracket => {
            if a <= 0.0 || b <= 0.0 {
                return Vec::new();
            }
            let arm = BRACKET_ARM * a.min(b);
            let mut out = Vec::with_capacity(8);
            for su in [-1.0f32, 1.0] {
                for sn in [-1.0f32, 1.0] {
                    let corner = offset(offset(c, u, su * a), n, sn * b);
                    out.push([corner, offset(corner, u, -su * arm)]);
                    out.push([corner, offset(corner, n, -sn * arm)]);
                }
            }
            out
        }
        Shape::Arrow => {
            if a <= 0.0 {
                return Vec::new();
            }
            let tip = offset(c, u, a);
            let tail = offset(c, u, -a);
            // The head is as long as its wings are out, at most the shaft.
            let neck = offset(tip, u, -b.min(2.0 * a));
            vec![
                [tail, tip],
                [offset(neck, n, b), tip],
                [offset(neck, n, -b), tip],
            ]
        }
        Shape::Ticks => {
            if a <= 0.0 {
                return Vec::new();
            }
            let slot = a / TICKS as f32;
            (0..TICKS)
                .map(|i| {
                    let from = slot * i as f32;
                    [offset(c, u, from), offset(c, u, from + slot * TICK_DUTY)]
                })
                .collect()
        }
        Shape::Bar => {
            if b <= 0.0 {
                return Vec::new();
            }
            let across = |at: Vec3, half: f32| [offset(at, n, -half), offset(at, n, half)];
            let mut out = vec![across(c, b)];
            if a > 0.0 {
                out.push(across(offset(c, u, -a), b * BAR_END));
                out.push(across(offset(c, u, a), b * BAR_END));
            }
            out
        }
        Shape::Line => {
            if a <= 0.0 {
                return Vec::new();
            }
            vec![[offset(c, u, -a), offset(c, u, a)]]
        }
        Shape::Segment => match pose.to {
            Some(to) if to != pose.at => vec![[pose.at, to]],
            _ => Vec::new(),
        },
    }
}

/// How far (X and Y) the lines of `shape` at `pose` reach from the pose's
/// point: the radius the check holds [`edges`] to.
pub fn reach(shape: Shape, pose: &Pose) -> f32 {
    let [a, b] = pose.half;
    match shape {
        Shape::Pipper | Shape::Ticks | Shape::Line => a,
        Shape::Bracket | Shape::Arrow | Shape::Bar => a.hypot(b),
        Shape::Segment => pose
            .to
            .map_or(0.0, |to| (to[0] - pose.at[0]).hypot(to[1] - pose.at[1])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn length(e: &Edge) -> f32 {
        (e[1][0] - e[0][0]).hypot(e[1][1] - e[0][1])
    }

    /// A pipper is a closed ring of its radius round the point, lifted
    /// off the floor, with a cross at its centre.
    #[test]
    fn a_pipper_is_a_ring_with_a_cross() {
        let pose = Pose::at([2.0, 3.0, 1.0], [0.06, 0.0]);
        let e = edges(Shape::Pipper, &pose);
        assert_eq!(e.len(), PIPPER_SEGMENTS + 2);
        for (i, seg) in e[..PIPPER_SEGMENTS].iter().enumerate() {
            for p in seg {
                assert!(close((p[0] - 2.0).hypot(p[1] - 3.0), 0.06), "{p:?}");
                assert!(close(p[2], 1.0 + LIFT));
            }
            assert_eq!(seg[1], e[(i + 1) % PIPPER_SEGMENTS][0], "the ring closes");
        }
        for seg in &e[PIPPER_SEGMENTS..] {
            assert!(close(length(seg), 2.0 * 0.06 * PIPPER_CROSS));
            let mid = [(seg[0][0] + seg[1][0]) / 2.0, (seg[0][1] + seg[1][1]) / 2.0];
            assert!(close(mid[0], 2.0) && close(mid[1], 3.0));
        }
    }

    /// A bracket's corners are the box's, turned with the heading: along
    /// +Y the long side runs in Y.
    #[test]
    fn a_bracket_turns_with_its_heading() {
        let pose = Pose::at([0.0, 0.0, 0.0], [0.3, 0.1]).heading([0.0, 2.0]);
        let e = edges(Shape::Bracket, &pose);
        assert_eq!(e.len(), 8);
        let arm = BRACKET_ARM * 0.1;
        for pair in e.chunks(2) {
            let corner = pair[0][0];
            assert_eq!(corner, pair[1][0]);
            assert!(close(corner[0].abs(), 0.1), "across is X: {corner:?}");
            assert!(close(corner[1].abs(), 0.3), "along is Y: {corner:?}");
            for seg in pair {
                assert!(close(length(seg), arm));
                // Every arm runs toward the middle.
                assert!(seg[1][0].abs() <= corner[0].abs() + 1e-6);
                assert!(seg[1][1].abs() <= corner[1].abs() + 1e-6);
            }
        }
        // Along +X the box is the other way round.
        let flat = edges(Shape::Bracket, &Pose::at([0.0; 3], [0.3, 0.1]));
        assert!(close(flat[0][0][0].abs(), 0.3) && close(flat[0][0][1].abs(), 0.1));
    }

    /// An arrow's shaft runs through the point along the heading and its
    /// wings meet at the tip.
    #[test]
    fn an_arrow_points_along_its_heading() {
        let pose = Pose::at([1.0, 1.0, 0.0], [0.1, 0.04]).heading([0.0, -1.0]);
        let e = edges(Shape::Arrow, &pose);
        assert_eq!(e.len(), 3);
        let tip = e[0][1];
        assert!(close(tip[0], 1.0) && close(tip[1], 0.9), "{tip:?}");
        assert!(close(e[0][0][1], 1.1), "the tail is behind the point");
        for wing in &e[1..] {
            assert_eq!(wing[1], tip);
            assert!(close((wing[0][0] - 1.0).abs(), 0.04));
            assert!(close(wing[0][1], 0.94), "the head is as long as its wings are out");
        }
    }

    /// The ticks start at the point, run toward the heading and end
    /// inside the run; a shorter run gives shorter ticks, and none at 0.
    #[test]
    fn the_ticks_shrink_with_the_run() {
        let at = |run: f32| Pose::at([0.0; 3], [run, 0.0]).heading([1.0, 0.0]);
        let far = edges(Shape::Ticks, &at(0.6));
        assert_eq!(far.len(), TICKS);
        assert!(close(far[0][0][0], 0.0));
        for (i, t) in far.iter().enumerate() {
            assert!(close(t[0][0], 0.2 * i as f32));
            assert!(close(length(t), 0.2 * TICK_DUTY));
            assert!(t[1][0] <= 0.6);
        }
        let near = edges(Shape::Ticks, &at(0.15));
        assert!(length(&near[0]) < length(&far[0]) / 3.9);
        assert!(edges(Shape::Ticks, &at(0.0)).is_empty());
    }

    /// A bar lies across its heading through the point, and a band adds
    /// its two ends, shorter, either side along the heading.
    #[test]
    fn a_bar_lies_across_its_heading() {
        let pose = Pose::at([1.0, 2.0, 0.0], [0.0, 0.25]).heading([0.0, 1.0]);
        let e = edges(Shape::Bar, &pose);
        assert_eq!(e.len(), 1);
        assert!(close(e[0][0][1], 2.0) && close(e[0][1][1], 2.0), "across +Y is X: {e:?}");
        assert!(close(length(&e[0]), 0.5));
        let band = edges(Shape::Bar, &Pose { half: [0.03, 0.25], ..pose });
        assert_eq!(band.len(), 3);
        assert!(close(band[1][0][1], 1.97) && close(band[2][0][1], 2.03), "{band:?}");
        for end in &band[1..] {
            assert!(close(length(end), 0.5 * BAR_END));
            assert!(close((end[0][0] + end[1][0]) / 2.0, 1.0), "centred on the point");
        }
        assert!(edges(Shape::Bar, &Pose { half: [0.03, 0.0], ..pose }).is_empty());
    }

    /// A line is one segment through the point along the heading, `half[0]`
    /// each way, and none without a length.
    #[test]
    fn a_line_is_one_segment_along_its_heading() {
        let pose = Pose::at([1.0, 2.0, 0.5], [0.5, 0.0]).heading([0.0, 3.0]);
        let e = edges(Shape::Line, &pose);
        assert_eq!(e.len(), 1);
        assert!(close(e[0][0][0], 1.0) && close(e[0][0][1], 1.5), "{:?}", e[0][0]);
        assert!(close(e[0][1][0], 1.0) && close(e[0][1][1], 2.5), "{:?}", e[0][1]);
        assert!(close(e[0][0][2], 0.5 + LIFT) && close(e[0][1][2], 0.5 + LIFT));
        assert!(close(length(&e[0]), 1.0));
        assert!(edges(Shape::Line, &Pose::at([0.0; 3], [0.0, 0.0])).is_empty());
        assert_eq!(Shape::Line.name(), "line");
    }

    /// A segment runs from its point to its far end as given, up and
    /// down too, and is nothing without a far end or with the point as it.
    #[test]
    fn a_segment_runs_between_its_two_points_unlifted() {
        let up = Pose::at([1.0, 2.0, 3.0], [0.0, 0.0]).to([1.0, 2.0, 4.5]);
        assert_eq!(edges(Shape::Segment, &up), vec![[[1.0, 2.0, 3.0], [1.0, 2.0, 4.5]]]);
        assert!(close(reach(Shape::Segment, &up), 0.0));
        let slant = Pose::at([0.0; 3], [0.0; 2]).to([3.0, 4.0, -1.0]);
        assert!(close(reach(Shape::Segment, &slant), 5.0));
        assert!(edges(Shape::Segment, &Pose::at([0.0; 3], [0.0; 2])).is_empty());
        assert!(edges(Shape::Segment, &Pose::at([1.0; 3], [0.0; 2]).to([1.0; 3])).is_empty());
        let bad = Pose::at([0.0; 3], [0.0; 2]).to([f32::NAN, 0.0, 0.0]);
        assert!(bad.fault().is_some());
        assert!(edges(Shape::Segment, &bad).is_empty());
        assert_eq!(Shape::Segment.name(), "segment");
        assert!(!Shape::ALL.contains(&Shape::Segment));
    }

    /// Every shape's lines stay within its reach of the point, and a
    /// pose with a fault draws nothing.
    #[test]
    fn every_shape_stays_within_its_reach() {
        let pose = Pose::at([5.0, -2.0, 0.5], [0.3, 0.12]).heading([0.6, 0.8]);
        for shape in Shape::ALL {
            let e = edges(shape, &pose);
            assert!(!e.is_empty(), "{shape:?}");
            let r = reach(shape, &pose);
            for p in e.iter().flatten() {
                let d = (p[0] - 5.0).hypot(p[1] + 2.0);
                assert!(d <= r + 1e-4, "{shape:?}: {d} over {r}");
                assert!(close(p[2], 0.5 + LIFT));
            }
            for bad in [
                Pose { at: [f32::NAN, 0.0, 0.0], ..pose },
                Pose { half: [-0.1, 0.1], ..pose },
                Pose { strength: 1.5, ..pose },
            ] {
                assert!(bad.fault().is_some());
                assert!(edges(shape, &bad).is_empty(), "{shape:?}");
            }
        }
        assert_eq!(unit([0.0, 0.0]), [1.0, 0.0]);
    }
}
