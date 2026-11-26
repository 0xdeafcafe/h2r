//! The ground a hill, a territory or a site covers
//! (docs/game/gametype-markers.md, "Areas"): a radius round the centre
//! marker, a box on the marker's axes, a ball round it, or a path of
//! points dropped round it. All but the ball lie between two heights.
//!
//! A path's points are offsets from the centre marker across the floor,
//! so moving the marker moves its area. They are joined in the order
//! they were dropped and closed back to the first; the outline may be
//! concave and may not cross itself.
use std::fmt;

use crate::{Mesh, Vec3, shapes::{circle, line, path}};

/// The fewest points a path has.
pub const MIN_POINTS: usize = 3;
/// The most points a tool stores in a path.
pub const MAX_POINTS: usize = 32;
/// The smallest radius, in world units.
pub const MIN_RADIUS: f32 = 0.25;
/// The largest radius, and the farthest a path's point is from the
/// centre marker, in world units.
pub const MAX_REACH: f32 = 64.0;
/// The shortest edge of a path, in world units.
pub const MIN_EDGE: f32 = 0.1;
/// The smallest floor a path covers, in square world units.
pub const MIN_FLOOR: f32 = 0.05;
/// The least an area reaches from its lowest to its highest, and the
/// most it reaches under or over its centre marker, in world units.
pub const MIN_HEIGHT: f32 = 0.25;
pub const MAX_HEIGHT: f32 = 32.0;

/// The radius of an area nobody has shaped: the cylinder a zone round
/// one flag is (`game::rules::world::zones::RADIUS`).
pub const DEFAULT_RADIUS: f32 = 1.0;
/// How far an unshaped area reaches under its marker (`zones::BELOW`).
pub const DEFAULT_BELOW: f32 = 0.5;
/// How far an unshaped area reaches over its marker (`zones::ABOVE`).
pub const DEFAULT_ABOVE: f32 = 1.5;
/// The points a radius's outline is drawn with.
pub const SEGMENTS: usize = 32;
/// How far past a turned box's edge a point still counts as on it, in
/// world units, so a corner survives the turn's rounding.
pub const EDGE_SLACK: f32 = 1e-4;

/// An area's outline across the floor.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// A circle of this radius round the centre marker.
    Radius(f32),
    /// A box on the marker's own axes: these half lengths along x (the
    /// way the marker faces) and y (its left).
    Box { half: [f32; 2] },
    /// A ball of this radius round the centre marker. It has no height:
    /// `below` and `above` of its [`Area`] are ignored and kept at zero.
    Sphere(f32),
    /// The polygon through these offsets from the centre marker, in the
    /// order they were dropped.
    Path(Vec<[f32; 2]>),
}

/// An area: its outline, and how far it reaches under and over its
/// centre marker.
#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub shape: Shape,
    pub below: f32,
    pub above: f32,
}

/// Why an area is refused.
#[derive(Clone, Debug, PartialEq)]
pub enum AreaError {
    /// A number is not finite.
    NotFinite,
    /// The radius is outside [`MIN_RADIUS`] to [`MAX_REACH`].
    Radius(f32),
    /// A box's half lengths are not both within [`MIN_RADIUS`] to
    /// [`MAX_REACH`].
    Half([f32; 2]),
    /// A height is negative or over [`MAX_HEIGHT`], or the two together
    /// are under [`MIN_HEIGHT`].
    Height { below: f32, above: f32 },
    /// The path has fewer than [`MIN_POINTS`] or more than
    /// [`MAX_POINTS`] points.
    Points(usize),
    /// The point at this index is farther than [`MAX_REACH`] from the
    /// centre marker.
    Reach(usize),
    /// The edge from the point at this index to the next is shorter than
    /// [`MIN_EDGE`].
    Edge(usize),
    /// The path covers less than [`MIN_FLOOR`]: its points are in a line.
    Flat,
    /// The edges starting at these two indices cross or touch.
    Crosses(usize, usize),
}

impl fmt::Display for AreaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFinite => write!(f, "An area's numbers must be finite."),
            Self::Radius(r) => write!(
                f,
                "An area's radius is {MIN_RADIUS} to {MAX_REACH} world units; this one is {r}."
            ),
            Self::Half([x, y]) => write!(
                f,
                "A box's half lengths are {MIN_RADIUS} to {MAX_REACH} world units; this one is {x} by {y}."
            ),
            Self::Height { below, above } => write!(
                f,
                "An area reaches 0 to {MAX_HEIGHT} world units under and over its marker and at least {MIN_HEIGHT} in all; this one reaches {below} under and {above} over."
            ),
            Self::Points(n) => write!(
                f,
                "An area's path has {MIN_POINTS} to {MAX_POINTS} points; this one has {n}."
            ),
            Self::Reach(i) => write!(
                f,
                "Point {} of the path is more than {MAX_REACH} world units from the marker.",
                i + 1
            ),
            Self::Edge(i) => write!(
                f,
                "Point {} of the path is less than {MIN_EDGE} world units from the next.",
                i + 1
            ),
            Self::Flat => write!(f, "The path's points are in a line: it covers no floor."),
            Self::Crosses(a, b) => write!(
                f,
                "The path crosses itself: the edge after point {} meets the edge after point {}.",
                a + 1,
                b + 1
            ),
        }
    }
}

impl std::error::Error for AreaError {}

impl Default for Area {
    /// The area of a marker nobody has shaped.
    fn default() -> Self {
        Self::radius(DEFAULT_RADIUS)
    }
}

/// `(b - a) x (c - a)`: positive when `c` is left of `a -> b`.
fn cross(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn dot(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[0] - a[0]) + (b[1] - a[1]) * (c[1] - a[1])
}

/// Whether `p`, on the line through `a` and `b`, is between them.
fn between(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> bool {
    p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

/// Whether the segments `a b` and `c d` cross or touch.
fn meet(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    let apart = |x: f32, y: f32| (x > 0.0 && y < 0.0) || (x < 0.0 && y > 0.0);
    (apart(d1, d2) && apart(d3, d4))
        || (d1 == 0.0 && between(c, d, a))
        || (d2 == 0.0 && between(c, d, b))
        || (d3 == 0.0 && between(a, b, c))
        || (d4 == 0.0 && between(a, b, d))
}

/// Twice the signed floor of the polygon: positive counter-clockwise.
fn doubled_floor(points: &[[f32; 2]]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// The convex hull of `points`, counter-clockwise (Andrew's monotone
/// chain). Fewer than three points, or all in a line, give fewer than
/// three corners.
pub fn hull(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut p: Vec<[f32; 2]> = points.to_vec();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let mut chain: Vec<[f32; 2]> = Vec::with_capacity(p.len() + 1);
    let back: Vec<[f32; 2]> = p.iter().rev().copied().collect();
    // The lower side left to right, then the upper side back.
    for side in [&p, &back] {
        let start = chain.len();
        for &q in side {
            while chain.len() >= start + 2
                && cross(chain[chain.len() - 2], chain[chain.len() - 1], q) <= 0.0
            {
                chain.pop();
            }
            chain.push(q);
        }
        // Each side's last point is the other side's first.
        chain.pop();
    }
    chain
}

impl Area {
    /// A circle of `radius` at the default heights.
    pub fn radius(radius: f32) -> Self {
        Self {
            shape: Shape::Radius(radius),
            below: DEFAULT_BELOW,
            above: DEFAULT_ABOVE,
        }
    }

    /// A box of these half lengths at the default heights.
    pub fn boxed(half: [f32; 2]) -> Self {
        Self {
            shape: Shape::Box { half },
            below: DEFAULT_BELOW,
            above: DEFAULT_ABOVE,
        }
    }

    /// A ball of `radius`: no height, so both heights are zero.
    pub fn sphere(radius: f32) -> Self {
        Self {
            shape: Shape::Sphere(radius),
            below: 0.0,
            above: 0.0,
        }
    }

    /// The polygon through `points` at the default heights.
    pub fn path(points: Vec<[f32; 2]>) -> Self {
        Self {
            shape: Shape::Path(points),
            below: DEFAULT_BELOW,
            above: DEFAULT_ABOVE,
        }
    }

    /// The area of a map's hill corners, and its centre marker's place:
    /// the convex hull of the corners across the floor round their mean,
    /// from [`DEFAULT_BELOW`] under the lowest corner to
    /// [`DEFAULT_ABOVE`] over the highest, which is the zone the host
    /// makes of them (`game::rules::world::zones`). `None` when the
    /// corners make no polygon. It is read from a map and not stored, so
    /// it may have more than [`MAX_POINTS`] points.
    pub fn of_corners(corners: &[[f32; 3]]) -> Option<([f32; 3], Area)> {
        let flat: Vec<[f32; 2]> = corners.iter().map(|c| [c[0], c[1]]).collect();
        let hull = hull(&flat);
        if hull.len() < MIN_POINTS {
            return None;
        }
        let n = hull.len() as f32;
        let x = hull.iter().map(|p| p[0]).sum::<f32>() / n;
        let y = hull.iter().map(|p| p[1]).sum::<f32>() / n;
        let low = corners.iter().map(|c| c[2]).fold(f32::INFINITY, f32::min);
        let high = corners
            .iter()
            .map(|c| c[2])
            .fold(f32::NEG_INFINITY, f32::max);
        let area = Area {
            shape: Shape::Path(hull.iter().map(|p| [p[0] - x, p[1] - y]).collect()),
            below: DEFAULT_BELOW,
            above: high - low + DEFAULT_ABOVE,
        };
        Some(([x, y, low], area))
    }

    /// Refuses an area the tools must not store. A stored area has
    /// passed this, so [`Area::contains`] and [`Area::outline`] need no
    /// checks of their own.
    pub fn validate(&self) -> Result<(), AreaError> {
        self.validate_with(false)
    }

    /// Check an edit in progress, including its first two points.
    /// Three or more points must already make a valid closed path.
    pub fn validate_draft(&self) -> Result<(), AreaError> {
        self.validate_with(true)
    }

    fn validate_with(&self, draft: bool) -> Result<(), AreaError> {
        let shape_finite = match &self.shape {
            Shape::Radius(r) | Shape::Sphere(r) => r.is_finite(),
            Shape::Box { half } => half.iter().all(|h| h.is_finite()),
            Shape::Path(points) => points.iter().flatten().all(|c| c.is_finite()),
        };
        if !(shape_finite && self.below.is_finite() && self.above.is_finite()) {
            return Err(AreaError::NotFinite);
        }
        let height = 0.0..=MAX_HEIGHT;
        if !matches!(self.shape, Shape::Sphere(_))
            && (!height.contains(&self.below)
                || !height.contains(&self.above)
                || self.below + self.above < MIN_HEIGHT)
        {
            return Err(AreaError::Height {
                below: self.below,
                above: self.above,
            });
        }
        match &self.shape {
            Shape::Radius(r) | Shape::Sphere(r) => {
                if !(MIN_RADIUS..=MAX_REACH).contains(r) {
                    return Err(AreaError::Radius(*r));
                }
            }
            Shape::Box { half } => {
                if !half.iter().all(|h| (MIN_RADIUS..=MAX_REACH).contains(h)) {
                    return Err(AreaError::Half(*half));
                }
            }
            Shape::Path(points) => validate_path(points, draft)?,
        }
        Ok(())
    }

    /// The radius of a circle covering the same floor as this shape.
    /// An unfinished path has no floor.
    pub fn equal_floor_radius(&self) -> Option<f32> {
        match &self.shape {
            Shape::Radius(radius) | Shape::Sphere(radius) => Some(*radius),
            Shape::Box { half } => Some((4.0 * half[0] * half[1] / std::f32::consts::PI).sqrt()),
            Shape::Path(points) if points.len() >= MIN_POINTS => {
                Some((doubled_floor(points).abs() / (2.0 * std::f32::consts::PI)).sqrt())
            }
            Shape::Path(_) => None,
        }
    }

    /// The outline across the floor, counter-clockwise, as offsets from
    /// the centre marker: a radius as [`SEGMENTS`] points, a path as its
    /// own points. A box is its four corners and a sphere its equator;
    /// both are in the marker's own axes, which a caller turns to the
    /// marker's facing.
    pub fn outline(&self) -> Vec<[f32; 2]> {
        match &self.shape {
            Shape::Radius(r) | Shape::Sphere(r) => circle(*r, SEGMENTS),
            Shape::Box { half: [x, y] } => vec![[-x, -y], [*x, -y], [*x, *y], [-x, *y]],
            Shape::Path(points) => {
                let mut points = points.clone();
                if doubled_floor(&points) < 0.0 {
                    points.reverse();
                }
                points
            }
        }
    }

    /// The lines of an unfinished path, with a dashed edge back to the
    /// first point. Uses drop order so the closing edge stays the same
    /// for either winding.
    pub fn path_preview(&self, colour: [f32; 3]) -> Mesh {
        let mut mesh = Mesh::default();
        let Shape::Path(points) = &self.shape else { return mesh };
        let level = |z| points.iter().map(|&[x, y]| [x, y, z]).collect::<Vec<_>>();
        let bottom = level(-self.below);
        let top = level(self.above);
        for (&a, &b) in bottom.iter().zip(&top) {
            line(&mut mesh, a, b, colour);
        }
        for edge in [&bottom, &top] {
            path(&mut mesh, edge, false, colour);
            if edge.len() >= MIN_POINTS
                && let (Some(&first), Some(&last)) = (edge.first(), edge.last())
            {
                dashed(&mut mesh, last, first, colour);
            }
        }
        mesh
    }

    /// Whether the world point `p` is in the area of a marker at
    /// `centre`, edges included.
    pub fn contains(&self, centre: [f32; 3], p: [f32; 3]) -> bool {
        self.contains_turned(centre, 0.0, p)
    }

    /// [`Area::contains`] for a marker facing `yaw` radians (counter-
    /// clockwise from +x): a box turns with the marker, every other shape
    /// is the same at any facing.
    pub fn contains_turned(&self, centre: [f32; 3], yaw: f32, p: [f32; 3]) -> bool {
        let q = [p[0] - centre[0], p[1] - centre[1]];
        if let Shape::Sphere(r) = &self.shape {
            let dz = p[2] - centre[2];
            return q[0] * q[0] + q[1] * q[1] + dz * dz <= r * r;
        }
        if p[2] < centre[2] - self.below || p[2] > centre[2] + self.above {
            return false;
        }
        match &self.shape {
            Shape::Radius(r) => q[0] * q[0] + q[1] * q[1] <= r * r,
            Shape::Box { half } => {
                let (s, c) = yaw.sin_cos();
                let local = [q[0] * c + q[1] * s, q[1] * c - q[0] * s];
                local[0].abs() <= half[0] + EDGE_SLACK && local[1].abs() <= half[1] + EDGE_SLACK
            }
            Shape::Sphere(_) => false,
            Shape::Path(points) => inside_path(points, q),
        }
    }

    /// The middle of the area of a marker at `centre`: the marker across
    /// the floor for a radius or a box, the marker itself for a sphere, the mean of a path's points, halfway
    /// between the heights.
    pub fn middle(&self, centre: [f32; 3]) -> [f32; 3] {
        let z = centre[2] + (self.above - self.below) / 2.0;
        match &self.shape {
            Shape::Radius(_) | Shape::Box { .. } => [centre[0], centre[1], z],
            Shape::Sphere(_) => centre,
            Shape::Path(points) => {
                let n = points.len().max(1) as f32;
                let x = points.iter().map(|p| p[0]).sum::<f32>() / n;
                let y = points.iter().map(|p| p[1]).sum::<f32>() / n;
                [centre[0] + x, centre[1] + y, z]
            }
        }
    }
}

fn dashed(mesh: &mut Mesh, from: Vec3, to: Vec3, colour: [f32; 3]) {
    let length = (to[0] - from[0]).hypot(to[1] - from[1]);
    let count = (length / 0.4).ceil().clamp(2., 64.) as u32;
    let at = |t: f32| std::array::from_fn(|i| from[i] + (to[i] - from[i]) * t / count as f32);
    for i in 0..count {
        line(mesh, at(i as f32), at(i as f32 + 0.5), colour);
    }
}

fn validate_path(points: &[[f32; 2]], draft: bool) -> Result<(), AreaError> {
    let n = points.len();
    if n > MAX_POINTS || (!draft && n < MIN_POINTS) {
        return Err(AreaError::Points(n));
    }
    if let Some(i) = points
        .iter()
        .position(|p| p[0] * p[0] + p[1] * p[1] > MAX_REACH * MAX_REACH)
    {
        return Err(AreaError::Reach(i));
    }
    if n < MIN_POINTS {
        if let [a, b] = points
            && dot(*a, *b, *b) < MIN_EDGE * MIN_EDGE
        {
            return Err(AreaError::Edge(0));
        }
        return Ok(());
    }
    let next = |i: usize| points[(i + 1) % n];
    for i in 0..n {
        let (a, b) = (points[i], next(i));
        if dot(a, b, b) < MIN_EDGE * MIN_EDGE {
            return Err(AreaError::Edge(i));
        }
    }
    if doubled_floor(points).abs() < 2.0 * MIN_FLOOR {
        return Err(AreaError::Flat);
    }
    for i in 0..n {
        for j in i + 1..n {
            let (a, b, c, d) = (points[i], next(i), points[j], next(j));
            let crosses = if j == i + 1 {
                // Neighbours share `b`: they meet only when one folds
                // back along the other.
                cross(b, a, d) == 0.0 && dot(b, a, d) > 0.0
            } else if i == 0 && j == n - 1 {
                // Neighbours round the join: they share `a`.
                cross(a, b, c) == 0.0 && dot(a, b, c) > 0.0
            } else {
                meet(a, b, c, d)
            };
            if crosses {
                return Err(AreaError::Crosses(i, j));
            }
        }
    }
    Ok(())
}

/// Whether `q` is in the polygon `points` or on its edge: an edge test,
/// then the crossings of a ray to +x.
fn inside_path(points: &[[f32; 2]], q: [f32; 2]) -> bool {
    let n = points.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        if cross(a, b, q) == 0.0 && between(a, b, q) {
            return true;
        }
        if (a[1] > q[1]) != (b[1] > q[1]) {
            let x = a[0] + (q[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if q[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

#[cfg(test)]
#[path = "area_shape_tests.rs"]
mod shape_tests;
