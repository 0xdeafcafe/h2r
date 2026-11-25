//! The seams of the targeting set on the map (docs/game/superbounce.md,
//! "Seam lines" and "The faced seam"): which seam of the set the player
//! is facing, and the line drawn along each seam's collision edge.
//!
//! - **The faced seam** ([`faced`]): of the seams the targets' mode shows
//!   (`superbounce_targets::current`), the one the player looks and moves
//!   toward. While primed on the ground it is the chosen strip, so the
//!   navpoint, the takeoff, the approach guide and the fall assist follow
//!   where the player is going, not the nearest strip.
//! - **Seam lines** ([`lines`], [`segments`]): every seam of the set gets
//!   a line on its collision edge, vertex to vertex, where the bounce
//!   happens; the chosen seam's is bold with a tick across each end.
//!
//! All of it is pure; `superbounce_mode` runs [`faced`] each tick and
//! `superbounce_world` draws [`lines`].

use superbounce::Vec3;

use crate::superbounce_targets::{Target, faces};

/// A seam further than this from the player (world units, to its edge,
/// X and Y) is not the faced one.
pub const FACED_RANGE: f32 = 25.0;
/// A seam is faced within this angle of the heading: 50 degrees.
pub const FACED_CONE: f32 = 50.0 * std::f32::consts::PI / 180.0;
/// What a world unit of distance costs against the angle (radians): of
/// two seams ahead, one 10 units nearer wins over one 11 degrees nearer
/// the heading.
pub const FACED_PER_UNIT: f32 = 0.02;
/// What the seam already faced is ahead by (radians, 8.6 degrees), so the
/// choice does not flicker between two seams as the aim crosses the
/// middle.
pub const FACED_KEEP: f32 = 0.15;
/// A seam whose edge is this near (world units, X and Y) is underfoot:
/// it is faced whatever the heading.
pub const UNDERFOOT: f32 = 0.5;
/// Moving faster than this over the ground (units/s), the heading is
/// the aim and the way of travel together; slower, the aim alone.
pub const MOVING: f32 = 0.5;

/// A seam line is drawn this far over its edge, clear of the floor.
pub const LIFT: f32 = 0.02;
/// Seam lines start to fade this far from the player (world units) and
/// are gone at [`FADE_FAR`], as the mode's other lines
/// (`superbounce_mode::FADE_NEAR`).
pub const FADE_NEAR: f32 = crate::superbounce_mode::FADE_NEAR;
pub const FADE_FAR: f32 = crate::superbounce_mode::FADE_FAR;
/// A seam of the set that is not the chosen one draws at this alpha.
pub const OTHER_ALPHA: f32 = 0.6;
/// The chosen seam's end ticks: half their length across the edge, the
/// catch (`superbounce::CATCH`), so the ticks span what a landing may be
/// off by.
pub const END_TICK: f32 = superbounce::CATCH;
/// A seam with no length (a busy point, a V's tip) is a cross this big
/// each way.
pub const POINT_HALF: f32 = 0.1;
/// Two landing points this close on each axis are one seam's.
pub const SAME: f32 = 1e-3;

fn same(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= SAME)
}

/// The point of `edge` nearest `xy`, X and Y.
pub fn edge_point([a, b]: [Vec3; 2], xy: [f32; 2]) -> Vec3 {
    let e = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len2 = e[0] * e[0] + e[1] * e[1];
    let t = if len2 > 1e-12 && len2.is_finite() {
        (((xy[0] - a[0]) * e[0] + (xy[1] - a[1]) * e[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    [a[0] + t * e[0], a[1] + t * e[1], a[2] + t * e[2]]
}

fn unit(v: [f32; 2]) -> Option<[f32; 2]> {
    let n = v[0].hypot(v[1]);
    (n > 1e-6 && n.is_finite()).then(|| [v[0] / n, v[1] / n])
}

/// The way the player is going, flat: the aim's yaw, and above
/// [`MOVING`] the aim and the way of travel together (their sum's
/// direction). `None` when neither is known.
pub fn heading(yaw: Option<f32>, velocity: Option<Vec3>) -> Option<[f32; 2]> {
    let aim = yaw.filter(|y| y.is_finite()).map(|y| [y.cos(), y.sin()]);
    let travel = velocity
        .map(|v| [v[0], v[1]])
        .filter(|v| v[0].hypot(v[1]) > MOVING)
        .and_then(unit);
    match (aim, travel) {
        (Some(a), Some(t)) => unit([a[0] + t[0], a[1] + t[1]]).or(Some(a)),
        (a, t) => a.or(t),
    }
}

/// What facing `t` from `at` along `heading` costs, lower first: the
/// angle off the heading to the nearest point of its edge (radians) plus
/// [`FACED_PER_UNIT`] a unit of distance. `None` when it is not faced:
/// a V's arm from its own side, further than [`FACED_RANGE`], or outside
/// [`FACED_CONE`]. A seam underfoot ([`UNDERFOOT`]) costs its distance
/// alone.
pub fn facing_cost(t: &Target, at: Vec3, heading: [f32; 2]) -> Option<f32> {
    if !faces(t, at) {
        return None;
    }
    let q = edge_point(t.spot.edge, [at[0], at[1]]);
    let d = [q[0] - at[0], q[1] - at[1]];
    let far = d[0].hypot(d[1]);
    if !far.is_finite() || far > FACED_RANGE {
        return None;
    }
    if far <= UNDERFOOT {
        return Some(far * FACED_PER_UNIT);
    }
    let cos = (d[0] * heading[0] + d[1] * heading[1]) / far;
    let angle = cos.clamp(-1.0, 1.0).acos();
    (angle <= FACED_CONE).then_some(angle + far * FACED_PER_UNIT)
}

/// The seam of `targets` a player at `at` is facing, by its landing
/// point (`Target::at`): the lowest [`facing_cost`] along [`heading`],
/// the seam at `held` (the one faced the tick before) ahead by
/// [`FACED_KEEP`]. `None` with no heading or no seam in the cone.
pub fn faced(
    targets: &[Target],
    at: Vec3,
    yaw: Option<f32>,
    velocity: Option<Vec3>,
    held: Option<Vec3>,
) -> Option<Vec3> {
    let h = heading(yaw, velocity)?;
    targets
        .iter()
        .filter_map(|t| {
            let keep = if held.is_some_and(|p| same(p, t.at())) {
                FACED_KEEP
            } else {
                0.0
            };
            Some((t.at(), facing_cost(t, at, h)? - keep))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| p)
}

/// The faced seam from tick to tick: on the ground it follows
/// [`faced`]; in the air it stays the one the jump left for, while that
/// seam is still in the set, so the target does not change in a flight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facing {
    held: Option<Vec3>,
}

impl Facing {
    pub const fn new() -> Facing {
        Facing { held: None }
    }

    /// The seam held.
    pub fn held(&self) -> Option<Vec3> {
        self.held
    }

    /// One tick. With no seam faced on the ground the last one is kept
    /// while it is still in `targets`: looking away to find the way up
    /// does not drop the seam being worked on.
    pub fn step(
        &mut self,
        targets: &[Target],
        at: Vec3,
        yaw: Option<f32>,
        velocity: Option<Vec3>,
        airborne: bool,
    ) -> Option<Vec3> {
        let still = self
            .held
            .filter(|p| targets.iter().any(|t| same(t.at(), *p)));
        self.held = if airborne {
            still
        } else {
            faced(targets, at, yaw, velocity, still).or(still)
        };
        self.held
    }
}

/// One seam's line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeamLine {
    /// The collision edge, vertex to vertex, as the map stores it.
    pub edge: [Vec3; 2],
    /// The seam's landing point.
    pub stand: Vec3,
    /// It is the chosen seam's.
    pub chosen: bool,
    /// 0 to 1: the distance fade, times [`OTHER_ALPHA`] for a seam that
    /// is not the chosen one.
    pub alpha: f32,
}

/// The fade of a line `distance` from the player: 1 to [`FADE_NEAR`],
/// 0 from [`FADE_FAR`]. The chosen seam's line does not fade.
pub fn fade(distance: f32) -> f32 {
    if !distance.is_finite() {
        return 0.0;
    }
    ((FADE_FAR - distance) / (FADE_FAR - FADE_NEAR)).clamp(0.0, 1.0)
}

/// The lines of the set: one for each seam of `targets` a player at `at`
/// is given ([`faces`]), the one whose landing point is `chosen` marked
/// so and drawn last. A seam faded out is left out.
pub fn lines(targets: &[Target], at: Vec3, chosen: Option<Vec3>) -> Vec<SeamLine> {
    let mut out: Vec<SeamLine> = targets
        .iter()
        .filter(|t| faces(t, at))
        .filter_map(|t| {
            let is_chosen = chosen.is_some_and(|c| same(c, t.at()));
            let q = edge_point(t.spot.edge, [at[0], at[1]]);
            let far = ((q[0] - at[0]).powi(2) + (q[1] - at[1]).powi(2) + (q[2] - at[2]).powi(2))
                .sqrt();
            let alpha = if is_chosen {
                1.0
            } else {
                OTHER_ALPHA * fade(far)
            };
            (alpha > 0.0).then_some(SeamLine {
                edge: t.spot.edge,
                stand: t.at(),
                chosen: is_chosen,
                alpha,
            })
        })
        .collect();
    out.sort_by_key(|l| l.chosen);
    out
}

/// The segments of a seam line, [`LIFT`] over the edge: the edge itself
/// from vertex to vertex, and for the chosen seam a tick across each
/// end, [`END_TICK`] either side. An edge with no length is a cross on
/// its vertex ([`POINT_HALF`]).
pub fn segments(l: &SeamLine) -> Vec<[Vec3; 2]> {
    let up = |p: Vec3| [p[0], p[1], p[2] + LIFT];
    let [a, b] = [up(l.edge[0]), up(l.edge[1])];
    let Some(u) = unit([b[0] - a[0], b[1] - a[1]]) else {
        let h = POINT_HALF;
        return vec![
            [[a[0] - h, a[1], a[2]], [a[0] + h, a[1], a[2]]],
            [[a[0], a[1] - h, a[2]], [a[0], a[1] + h, a[2]]],
        ];
    };
    let mut out = vec![[a, b]];
    if l.chosen {
        let n = [-u[1] * END_TICK, u[0] * END_TICK];
        for p in [a, b] {
            out.push([
                [p[0] - n[0], p[1] - n[1], p[2]],
                [p[0] + n[0], p[1] + n[1], p[2]],
            ]);
        }
    }
    out
}

/// Where a seam line's label and its sight test sit: the point of the
/// edge nearest the seam's landing point, lifted.
pub fn anchor(l: &SeamLine) -> Vec3 {
    let q = edge_point(l.edge, [l.stand[0], l.stand[1]]);
    [q[0], q[1], q[2] + LIFT]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::superbounce_targets::Class;
    use superbounce::{Kind, Spot};

    /// A seam along +X through `(x, y)`, one unit long each way, its
    /// landing point 0.02 before the edge.
    fn seam(x: f32, y: f32) -> Target {
        Target {
            spot: Spot {
                kind: Kind::Bounce,
                edge: [[x - 1.0, y, 0.0], [x + 1.0, y, 0.0]],
                stand: [x, y - 0.02, 0.01],
                drift: [0.0, 1.0],
                ..Spot::default()
            },
            class: Class::Slot,
        }
    }

    const NORTH: f32 = std::f32::consts::FRAC_PI_2;

    #[test]
    fn the_seam_looked_at_is_faced_not_the_nearest() {
        // One seam 4 units behind, one 12 ahead: the nearest strip rule
        // picks the one behind, the facing rule the one ahead.
        let set = [seam(0.0, -4.0), seam(0.0, 12.0)];
        let at = [0.0, 0.0, 2.0];
        assert_eq!(faced(&set, at, Some(NORTH), None, None), Some(set[1].at()));
        assert_eq!(faced(&set, at, Some(-NORTH), None, None), Some(set[0].at()));
        // Looking along +X, neither is within 50 degrees.
        assert_eq!(faced(&set, at, Some(0.0), None, None), None);
    }

    #[test]
    fn of_two_seams_ahead_the_one_nearer_the_heading_wins_unless_much_further() {
        let at = [0.0, 0.0, 2.0];
        // 10 units out, its edge dead ahead, against 10 units out and
        // 21.8 degrees right.
        let set = [seam(-1.0, 10.0), seam(5.0, 10.0)];
        assert_eq!(faced(&set, at, Some(NORTH), None, None), Some(set[0].at()));
        // Dead ahead at 24 units (cost 0.48) against 17 degrees off at 5
        // (0.29 + 0.10): the near one.
        let set = [seam(0.0, 24.0), Target {
            spot: Spot {
                edge: [[1.4, 4.8, 0.0], [1.6, 4.8, 0.0]],
                stand: [1.5, 4.78, 0.01],
                ..seam(0.0, 0.0).spot
            },
            class: Class::Slot,
        }];
        assert_eq!(faced(&set, at, Some(NORTH), None, None), Some(set[1].at()));
    }

    #[test]
    fn the_distance_is_to_the_edge_not_the_landing_point() {
        // A long seam whose landing point is 20 units to the side: the
        // edge passes 3 units ahead.
        let long = Target {
            spot: Spot {
                edge: [[-30.0, 3.0, 0.0], [30.0, 3.0, 0.0]],
                stand: [20.0, 2.98, 0.01],
                ..seam(0.0, 0.0).spot
            },
            class: Class::Slot,
        };
        let c = facing_cost(&long, [0.0, 0.0, 1.0], [0.0, 1.0]).unwrap();
        assert!((c - 3.0 * FACED_PER_UNIT).abs() < 1e-5, "{c}");
        assert_eq!(edge_point(long.spot.edge, [0.0, 0.0]), [0.0, 3.0, 0.0]);
    }

    #[test]
    fn a_run_bends_the_heading_and_a_stroll_does_not() {
        // Looking north, running east at 2: the heading is north-east.
        let h = heading(Some(NORTH), Some([2.0, 0.0, 0.0])).unwrap();
        assert!((h[0] - h[1]).abs() < 1e-5 && h[0] > 0.7, "{h:?}");
        let h = heading(Some(NORTH), Some([0.3, 0.0, 0.0])).unwrap();
        assert!(h[0].abs() < 1e-5 && (h[1] - 1.0).abs() < 1e-5, "{h:?}");
        // Backing straight away from the aim cancels: the aim stands.
        let h = heading(Some(NORTH), Some([0.0, -2.0, 0.0])).unwrap();
        assert!((h[1] - 1.0).abs() < 1e-5, "{h:?}");
        assert_eq!(heading(None, None), None);
        assert_eq!(heading(Some(f32::NAN), Some([0.0; 3])), None);
    }

    #[test]
    fn the_faced_seam_is_kept_across_the_middle_and_in_the_air() {
        let at = [0.0, 0.0, 2.0];
        let set = [seam(-3.0, 10.0), seam(3.0, 10.0)];
        let mut f = Facing::new();
        // Aimed a little left: the left seam.
        let left = NORTH + 0.1;
        assert_eq!(f.step(&set, at, Some(left), None, false), Some(set[0].at()));
        // A little right of the middle, inside the keep: still the left.
        let right = NORTH - 0.05;
        assert_eq!(f.step(&set, at, Some(right), None, false), Some(set[0].at()));
        // Well right: the right seam.
        let right = NORTH - 0.3;
        assert_eq!(f.step(&set, at, Some(right), None, false), Some(set[1].at()));
        // In the air, looking back at the left one changes nothing.
        assert_eq!(f.step(&set, at, Some(left), None, true), Some(set[1].at()));
        // Looking away on the ground keeps it; the seam leaving the set
        // (a switch of the targets) drops it.
        assert_eq!(f.step(&set, at, Some(-NORTH), None, false), Some(set[1].at()));
        assert_eq!(f.step(&set[..1], at, Some(-NORTH), None, false), None);
    }

    #[test]
    fn a_seam_underfoot_is_faced_whatever_the_aim() {
        let set = [seam(0.0, 0.3)];
        assert_eq!(
            faced(&set, [0.0, 0.0, 0.01], Some(-NORTH), None, None),
            Some(set[0].at())
        );
    }

    #[test]
    fn every_seam_of_the_set_has_a_line_on_its_edge() {
        let set = [seam(0.0, 5.0), seam(8.0, 5.0), seam(0.0, 100.0)];
        let at = [0.0, 0.0, 2.0];
        let l = lines(&set, at, Some(set[0].at()));
        // The far one is faded out; the chosen one is last.
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].edge, l[0].chosen), (set[1].spot.edge, false));
        assert!((l[0].alpha - OTHER_ALPHA).abs() < 1e-6);
        assert_eq!((l[1].edge, l[1].chosen, l[1].alpha), (set[0].spot.edge, true, 1.0));
        // The chosen seam's line shows from any distance.
        let l = lines(&set, at, Some(set[2].at()));
        assert!(l.iter().any(|l| l.chosen && l.edge == set[2].spot.edge));
    }

    #[test]
    fn a_line_runs_vertex_to_vertex_on_the_edge() {
        let t = seam(2.0, 5.0);
        let other = SeamLine {
            edge: t.spot.edge,
            stand: t.at(),
            chosen: false,
            alpha: 1.0,
        };
        // The line is the edge itself, lifted: it touches the seam.
        assert_eq!(
            segments(&other),
            vec![[[1.0, 5.0, LIFT], [3.0, 5.0, LIFT]]]
        );
        // The chosen seam's has a tick across each end, the catch wide.
        let s = segments(&SeamLine { chosen: true, ..other });
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], [[1.0, 5.0, LIFT], [3.0, 5.0, LIFT]]);
        for (tick, x) in s[1..].iter().zip([1.0, 3.0]) {
            assert_eq!(tick[0], [x, 5.0 - END_TICK, LIFT]);
            assert_eq!(tick[1], [x, 5.0 + END_TICK, LIFT]);
        }
        // The anchor is on the edge by the landing point, not on it.
        assert_eq!(anchor(&other), [2.0, 5.0, LIFT]);
    }

    #[test]
    fn a_seam_with_no_length_is_a_cross() {
        let l = SeamLine {
            edge: [[1.0, 2.0, 3.0]; 2],
            stand: [1.0, 2.0, 3.01],
            chosen: true,
            alpha: 1.0,
        };
        let s = segments(&l);
        assert_eq!(s.len(), 2);
        assert_eq!(
            s[0],
            [[1.0 - POINT_HALF, 2.0, 3.0 + LIFT], [1.0 + POINT_HALF, 2.0, 3.0 + LIFT]]
        );
    }

    #[test]
    fn lines_fade_between_the_two_ranges() {
        assert_eq!(fade(0.0), 1.0);
        assert_eq!(fade(FADE_NEAR), 1.0);
        assert!((fade((FADE_NEAR + FADE_FAR) / 2.0) - 0.5).abs() < 1e-6);
        assert_eq!(fade(FADE_FAR), 0.0);
        assert_eq!(fade(f32::NAN), 0.0);
    }
}
