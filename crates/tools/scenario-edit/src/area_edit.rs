//! Editing an area with handles (docs/game/gametype-markers.md,
//! "Studio"): where the handles of a marker's area are, and the area a
//! drag of one makes. Pure: the viewport finds the handle under the
//! cursor and the point it was dragged to, and the session stores what
//! this returns when `Area::validate` passes.
//!
//! A radius has one handle on its rim, a box one on each corner and one
//! on the middle of each side (both in the marker's own axes, so they
//! turn with the marker), a sphere one on its equator and one on its
//! pole, and a path has a handle on each point and one on the middle of
//! each edge (dragging it adds a point there). The top and the bottom of
//! the wall each have a handle for the heights of everything but a
//! sphere.
use game_markers::area::{MAX_POINTS, MIN_POINTS};
use game_markers::{Area, AreaError, Shape};

use crate::geometry::Vec3;

/// One handle of an area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// A radius's rim, or a sphere's equator.
    Rim,
    /// Corner `i` of a box, counter-clockwise from the one at +x, +y of
    /// the marker: a drag sets both half lengths.
    Corner(usize),
    /// The middle of side `i` of a box (+x, +y, -x, -y of the marker): a
    /// drag sets that axis's half length.
    Side(usize),
    /// A sphere's pole, straight over the marker: a drag sets the
    /// radius.
    Pole,
    /// Point `i` of a path.
    Point(usize),
    /// The middle of the edge from point `i` to the next: a drag adds a
    /// point after `i`.
    Edge(usize),
    /// The top of the wall: `above`.
    Top,
    /// The bottom of the wall: `below`.
    Bottom,
}

impl Handle {
    /// Whether the handle moves across the floor (else it moves up and
    /// down).
    pub fn flat(self) -> bool {
        !matches!(self, Handle::Top | Handle::Bottom | Handle::Pole)
    }
}

/// Why a handle cannot be used.
#[derive(Clone, Debug, PartialEq)]
pub enum Refused {
    /// The area refuses the result (the sentence is `AreaError`'s).
    Area(AreaError),
    /// The handle is not one of this area's.
    NoHandle,
    /// A path keeps at least [`MIN_POINTS`].
    TooFew,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Area(e) => e.fmt(f),
            Refused::NoHandle => write!(f, "That is not a handle of this area."),
            Refused::TooFew => write!(f, "A path keeps at least {MIN_POINTS} points."),
        }
    }
}

impl From<AreaError> for Refused {
    fn from(e: AreaError) -> Self {
        Refused::Area(e)
    }
}

/// A floor offset turned `yaw` radians counter-clockwise.
fn turn(v: [f32; 2], yaw: f32) -> [f32; 2] {
    let (s, c) = yaw.sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c]
}

/// The signs of a box's corners, in [`Handle::Corner`]'s order.
const CORNERS: [[f32; 2]; 4] = [[1.0, 1.0], [-1.0, 1.0], [-1.0, -1.0], [1.0, -1.0]];
/// The directions of a box's sides, in [`Handle::Side`]'s order.
const SIDES: [[f32; 2]; 4] = [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]];

/// The yaw of a marker whose forward axis is `forward`: radians
/// counter-clockwise from +x.
pub fn yaw_of(forward: Vec3) -> f32 {
    forward[1].atan2(forward[0])
}

/// Every handle of `area` for a marker at `centre` facing `yaw`, with its
/// place in the world (a flat handle at the marker's height). Only a box
/// turns with the marker.
pub fn handles(centre: Vec3, yaw: f32, area: &Area) -> Vec<(Handle, Vec3)> {
    let mut out = Vec::new();
    let flat = |v: [f32; 2]| [centre[0] + v[0], centre[1] + v[1], centre[2]];
    match &area.shape {
        Shape::Radius(r) => out.push((Handle::Rim, flat([*r, 0.0]))),
        Shape::Box { half } => {
            for (i, s) in CORNERS.iter().enumerate() {
                out.push((Handle::Corner(i), flat(turn([s[0] * half[0], s[1] * half[1]], yaw))));
            }
            for (i, s) in SIDES.iter().enumerate() {
                out.push((Handle::Side(i), flat(turn([s[0] * half[0], s[1] * half[1]], yaw))));
            }
        }
        Shape::Sphere(r) => {
            out.push((Handle::Rim, flat([*r, 0.0])));
            out.push((Handle::Pole, [centre[0], centre[1], centre[2] + r]));
        }
        Shape::Path(points) => {
            let n = points.len();
            for (i, p) in points.iter().enumerate() {
                out.push((Handle::Point(i), flat(*p)));
            }
            for i in 0..n {
                let (a, b) = (points[i], points[(i + 1) % n]);
                out.push((Handle::Edge(i), flat([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0])));
            }
        }
    }
    if !matches!(area.shape, Shape::Sphere(_)) {
        out.push((Handle::Top, [centre[0], centre[1], centre[2] + area.above]));
        out.push((Handle::Bottom, [centre[0], centre[1], centre[2] - area.below]));
    }
    out
}

/// The area after handle `h` is dragged to the world point `to`, for a
/// marker at `centre` facing `yaw`, or why not. The result passes
/// `Area::validate`; a refused drag leaves `area` as it was (the caller
/// keeps it).
pub fn drag(area: &Area, h: Handle, centre: Vec3, yaw: f32, to: Vec3) -> Result<Area, Refused> {
    let offset = [to[0] - centre[0], to[1] - centre[1]];
    // The same offset in the marker's own axes.
    let local = turn(offset, -yaw);
    let mut out = area.clone();
    match (h, &mut out.shape) {
        (Handle::Rim, Shape::Radius(r) | Shape::Sphere(r)) => *r = offset[0].hypot(offset[1]),
        (Handle::Pole, Shape::Sphere(r)) => *r = to[2] - centre[2],
        (Handle::Corner(i), Shape::Box { half }) if i < CORNERS.len() => {
            *half = [local[0].abs(), local[1].abs()];
        }
        (Handle::Side(i), Shape::Box { half }) if i < SIDES.len() => {
            // Sides 0 and 2 are on the x axis, 1 and 3 on the y.
            half[i % 2] = local[i % 2].abs();
        }
        (Handle::Point(i), Shape::Path(points)) => {
            *points.get_mut(i).ok_or(Refused::NoHandle)? = offset;
        }
        (Handle::Edge(i), Shape::Path(points)) => {
            if i >= points.len() {
                return Err(Refused::NoHandle);
            }
            if points.len() >= MAX_POINTS {
                return Err(AreaError::Points(points.len() + 1).into());
            }
            points.insert(i + 1, offset);
        }
        (Handle::Top | Handle::Bottom, Shape::Sphere(_)) => return Err(Refused::NoHandle),
        (Handle::Top, _) => out.above = to[2] - centre[2],
        (Handle::Bottom, _) => out.below = centre[2] - to[2],
        _ => return Err(Refused::NoHandle),
    }
    out.validate()?;
    Ok(out)
}

/// The path without point `i` (Delete on a point handle).
pub fn remove_point(area: &Area, i: usize) -> Result<Area, Refused> {
    let Shape::Path(points) = &area.shape else {
        return Err(Refused::NoHandle);
    };
    if i >= points.len() {
        return Err(Refused::NoHandle);
    }
    if points.len() <= MIN_POINTS {
        return Err(Refused::TooFew);
    }
    let mut out = area.clone();
    if let Shape::Path(p) = &mut out.shape {
        p.remove(i);
    }
    out.validate()?;
    Ok(out)
}

/// The four shapes, for the inspector's **Shape** row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Radius,
    Box,
    Sphere,
    Path,
}

impl ShapeKind {
    /// In the order the row shows them.
    pub const ALL: [ShapeKind; 4] = [
        ShapeKind::Radius,
        ShapeKind::Box,
        ShapeKind::Sphere,
        ShapeKind::Path,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ShapeKind::Radius => "Radius",
            ShapeKind::Box => "Box",
            ShapeKind::Sphere => "Sphere",
            ShapeKind::Path => "Path",
        }
    }

    /// The kind of `shape`.
    pub fn of(shape: &Shape) -> ShapeKind {
        match shape {
            Shape::Radius(_) => ShapeKind::Radius,
            Shape::Box { .. } => ShapeKind::Box,
            Shape::Sphere(_) => ShapeKind::Sphere,
            Shape::Path(_) => ShapeKind::Path,
        }
    }
}

/// How far the shape's farthest point is from its marker across the floor.
fn reach(shape: &Shape) -> f32 {
    match shape {
        Shape::Radius(r) | Shape::Sphere(r) => *r,
        Shape::Box { half } => half[0].hypot(half[1]),
        Shape::Path(points) => points.iter().map(|p| p[0].hypot(p[1])).fold(0.0_f32, f32::max),
    }
}

/// The inspector's **Shape** row: the area as `to`, for a marker facing
/// `yaw`, holding what the old shape held. A radius, a sphere and a path
/// keep their farthest reach; a box has the half lengths of the tightest
/// box in the marker's axes round the old floor; a path of a box or a
/// radius is its outline. The heights stay, but a sphere has none: out of
/// one, the height is its radius each way, and into one the heights are
/// zero.
pub fn with_shape(area: &Area, to: ShapeKind, yaw: f32) -> Result<Area, Refused> {
    if ShapeKind::of(&area.shape) == to {
        return Ok(area.clone());
    }
    let mut out = area.clone();
    out.shape = match to {
        ShapeKind::Radius => Shape::Radius(reach(&area.shape)),
        ShapeKind::Sphere => Shape::Sphere(reach(&area.shape)),
        ShapeKind::Box => Shape::Box {
            half: match &area.shape {
                Shape::Radius(r) | Shape::Sphere(r) => [*r, *r],
                Shape::Box { half } => *half,
                Shape::Path(points) => points.iter().fold([0.0_f32; 2], |h, p| {
                    let l = turn(*p, -yaw);
                    [h[0].max(l[0].abs()), h[1].max(l[1].abs())]
                }),
            },
        },
        ShapeKind::Path => Shape::Path(match &area.shape {
            Shape::Box { .. } => area.outline().into_iter().map(|p| turn(p, yaw)).collect(),
            _ => area.outline(),
        }),
    };
    if let Shape::Sphere(_) = out.shape {
        out.below = 0.0;
        out.above = 0.0;
    } else if let Shape::Sphere(r) = area.shape {
        let h = r.min(game_markers::area::MAX_HEIGHT);
        out.below = h;
        out.above = h;
    }
    out.validate()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: Vec3 = [10.0, 5.0, 1.0];

    fn triangle() -> Area {
        Area::path(vec![[-2.0, -2.0], [2.0, -2.0], [0.0, 2.0]])
    }

    #[test]
    fn a_radius_has_a_rim_handle_and_two_heights() {
        let h = handles(AT, 0.0, &Area::radius(3.0));
        assert_eq!(
            h,
            vec![
                (Handle::Rim, [13.0, 5.0, 1.0]),
                (Handle::Top, [10.0, 5.0, 2.5]),
                (Handle::Bottom, [10.0, 5.0, 0.5]),
            ]
        );
    }

    #[test]
    fn a_path_has_a_handle_a_point_and_one_on_each_edge() {
        let h = handles(AT, 0.0, &triangle());
        assert_eq!(h.len(), 3 + 3 + 2);
        assert_eq!(h[0], (Handle::Point(0), [8.0, 3.0, 1.0]));
        assert_eq!(h[3], (Handle::Edge(0), [10.0, 3.0, 1.0]));
        assert_eq!(h[5], (Handle::Edge(2), [9.0, 5.0, 1.0]));
    }

    #[test]
    fn a_rim_drag_sets_the_radius_and_a_refused_one_changes_nothing() {
        let area = Area::radius(3.0);
        let wide = drag(&area, Handle::Rim, AT, 0.0, [16.0, 5.0, 1.0]).unwrap();
        assert_eq!(wide.shape, Shape::Radius(6.0));
        assert_eq!(area.shape, Shape::Radius(3.0));
        // Too small, then too large: the radius limits' sentence.
        let e = drag(&area, Handle::Rim, AT, 0.0, [10.05, 5.0, 1.0]).unwrap_err();
        assert!(matches!(e, Refused::Area(AreaError::Radius(_))), "{e}");
        let e = drag(&area, Handle::Rim, AT, 0.0, [200.0, 5.0, 1.0]).unwrap_err();
        assert!(matches!(e, Refused::Area(AreaError::Radius(_))), "{e}");
    }

    #[test]
    fn an_edge_handle_adds_a_point_after_its_edges_start() {
        let area = triangle();
        let out = drag(&area, Handle::Edge(0), AT, 0.0, [10.0, 2.0, 1.0]).unwrap();
        let Shape::Path(p) = &out.shape else { panic!() };
        assert_eq!(p.len(), 4);
        assert_eq!(p[1], [0.0, -3.0]);
        assert_eq!(p[2], [2.0, -2.0]);
    }

    #[test]
    fn a_point_drag_that_crosses_the_path_is_refused() {
        let square = Area::path(vec![[-2.0, -2.0], [2.0, -2.0], [2.0, 2.0], [-2.0, 2.0]]);
        // Point 2 dragged across the left side makes a bow tie.
        let e = drag(&square, Handle::Point(2), AT, 0.0, [7.0, 5.0, 1.0]).unwrap_err();
        assert!(matches!(e, Refused::Area(AreaError::Crosses(..))), "{e}");
        let ok = drag(&square, Handle::Point(0), AT, 0.0, [7.0, 2.0, 1.0]).unwrap();
        let Shape::Path(p) = &ok.shape else { panic!() };
        assert_eq!(p[0], [-3.0, -3.0]);
    }

    #[test]
    fn the_wall_handles_set_the_heights() {
        let area = Area::radius(3.0);
        let tall = drag(&area, Handle::Top, AT, 0.0, [10.0, 5.0, 5.0]).unwrap();
        assert_eq!((tall.above, tall.below), (4.0, 0.5));
        let deep = drag(&area, Handle::Bottom, AT, 0.0, [10.0, 5.0, -1.0]).unwrap();
        assert_eq!(deep.below, 2.0);
        // Under the marker's floor is a negative reach: refused.
        assert!(drag(&area, Handle::Top, AT, 0.0, [10.0, 5.0, 0.0]).is_err());
    }

    #[test]
    fn a_handle_of_the_other_shape_is_not_one() {
        assert_eq!(
            drag(&Area::radius(3.0), Handle::Point(0), AT, 0.0, AT),
            Err(Refused::NoHandle)
        );
        assert_eq!(drag(&triangle(), Handle::Rim, AT, 0.0, AT), Err(Refused::NoHandle));
        assert_eq!(drag(&triangle(), Handle::Point(9), AT, 0.0, AT), Err(Refused::NoHandle));
    }

    #[test]
    fn delete_removes_a_point_down_to_three() {
        let square = Area::path(vec![[-2.0, -2.0], [2.0, -2.0], [2.0, 2.0], [-2.0, 2.0]]);
        let tri = remove_point(&square, 3).unwrap();
        let Shape::Path(p) = &tri.shape else { panic!() };
        assert_eq!(p.len(), 3);
        assert_eq!(remove_point(&tri, 0), Err(Refused::TooFew));
        assert_eq!(remove_point(&Area::radius(2.0), 0), Err(Refused::NoHandle));
    }

    #[test]
    fn the_shape_row_swaps_a_radius_for_its_outline_and_back() {
        let r = Area::radius(3.0);
        let path = with_shape(&r, ShapeKind::Path, 0.0).unwrap();
        let Shape::Path(p) = &path.shape else { panic!() };
        assert_eq!(p.len(), game_markers::area::SEGMENTS);
        let back = with_shape(&path, ShapeKind::Radius, 0.0).unwrap();
        let Shape::Radius(r) = back.shape else { panic!() };
        assert!((r - 3.0).abs() < 1e-3);
        assert_eq!(with_shape(&path, ShapeKind::Path, 0.0).unwrap(), path);
    }

    /// A quarter turn: the marker's x is the world's y.
    const QUARTER: f32 = std::f32::consts::FRAC_PI_2;

    fn near(a: Vec3, b: Vec3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
    }

    #[test]
    fn a_box_has_four_corners_four_sides_and_two_heights_in_the_markers_axes() {
        let area = Area::boxed([2.0, 1.0]);
        let h = handles(AT, 0.0, &area);
        assert_eq!(h.len(), 4 + 4 + 2);
        assert_eq!(h[0], (Handle::Corner(0), [12.0, 6.0, 1.0]));
        assert_eq!(h[2], (Handle::Corner(2), [8.0, 4.0, 1.0]));
        assert_eq!(h[4], (Handle::Side(0), [12.0, 5.0, 1.0]));
        assert_eq!(h[5], (Handle::Side(1), [10.0, 6.0, 1.0]));
        // Turned a quarter, the +x side is the world's +y one.
        let turned = handles(AT, QUARTER, &area);
        assert!(near(turned[4].1, [10.0, 7.0, 1.0]), "{:?}", turned[4]);
        assert!(near(turned[5].1, [9.0, 5.0, 1.0]), "{:?}", turned[5]);
    }

    #[test]
    fn a_corner_drag_writes_one_box_in_the_markers_axes() {
        let area = Area::boxed([2.0, 1.0]);
        let wide = drag(&area, Handle::Corner(0), AT, 0.0, [14.0, 8.0, 1.0]).unwrap();
        assert_eq!(wide.shape, Shape::Box { half: [4.0, 3.0] });
        // Any corner gives the same box for the same point's offsets, and
        // the heights stay.
        let back = drag(&area, Handle::Corner(2), AT, 0.0, [6.0, 2.0, 1.0]).unwrap();
        assert_eq!(back.shape, Shape::Box { half: [4.0, 3.0] });
        assert_eq!((wide.below, wide.above), (area.below, area.above));
        // Turned a quarter, a point 3 along the world's y is 3 along the
        // marker's x.
        let turned = drag(&area, Handle::Corner(0), AT, QUARTER, [8.0, 8.0, 1.0]).unwrap();
        let Shape::Box { half } = turned.shape else { panic!() };
        assert!((half[0] - 3.0).abs() < 1e-4 && (half[1] - 2.0).abs() < 1e-4, "{half:?}");
    }

    #[test]
    fn a_side_drag_changes_one_half_length() {
        let area = Area::boxed([2.0, 1.0]);
        let long = drag(&area, Handle::Side(0), AT, 0.0, [15.0, 9.0, 1.0]).unwrap();
        assert_eq!(long.shape, Shape::Box { half: [5.0, 1.0] });
        let tall = drag(&area, Handle::Side(3), AT, 0.0, [11.0, 1.0, 1.0]).unwrap();
        assert_eq!(tall.shape, Shape::Box { half: [2.0, 4.0] });
        assert_eq!(drag(&area, Handle::Side(4), AT, 0.0, AT), Err(Refused::NoHandle));
        assert_eq!(drag(&area, Handle::Corner(4), AT, 0.0, AT), Err(Refused::NoHandle));
    }

    #[test]
    fn a_box_drag_below_the_least_half_length_is_refused_and_changes_nothing() {
        let area = Area::boxed([2.0, 1.0]);
        let e = drag(&area, Handle::Corner(0), AT, 0.0, [10.05, 5.0, 1.0]).unwrap_err();
        assert!(matches!(e, Refused::Area(AreaError::Half(_))), "{e}");
        assert_eq!(area.shape, Shape::Box { half: [2.0, 1.0] });
        let e = drag(&area, Handle::Side(0), AT, 0.0, [200.0, 5.0, 1.0]).unwrap_err();
        assert!(matches!(e, Refused::Area(AreaError::Half(_))), "{e}");
    }

    #[test]
    fn a_sphere_has_an_equator_and_a_pole_and_no_heights() {
        let area = Area::sphere(3.0);
        assert_eq!(
            handles(AT, 0.0, &area),
            vec![
                (Handle::Rim, [13.0, 5.0, 1.0]),
                (Handle::Pole, [10.0, 5.0, 4.0]),
            ]
        );
        assert!(!Handle::Pole.flat() && Handle::Rim.flat());
        let wide = drag(&area, Handle::Rim, AT, 0.0, [16.0, 5.0, 1.0]).unwrap();
        assert_eq!(wide, Area::sphere(6.0));
        let tall = drag(&area, Handle::Pole, AT, 0.0, [10.0, 5.0, 6.0]).unwrap();
        assert_eq!(tall, Area::sphere(5.0));
        // Under the marker, and the wall handles: no radius, no handle.
        let e = drag(&area, Handle::Pole, AT, 0.0, [10.0, 5.0, 0.0]).unwrap_err();
        assert!(matches!(e, Refused::Area(AreaError::Radius(_))), "{e}");
        assert_eq!(drag(&area, Handle::Top, AT, 0.0, AT), Err(Refused::NoHandle));
    }

    #[test]
    fn the_shape_row_goes_between_all_four_shapes() {
        let radius = Area::radius(3.0);
        let boxed = with_shape(&radius, ShapeKind::Box, 0.0).unwrap();
        assert_eq!(boxed.shape, Shape::Box { half: [3.0, 3.0] });
        assert_eq!((boxed.below, boxed.above), (radius.below, radius.above));
        let ball = with_shape(&boxed, ShapeKind::Sphere, 0.0).unwrap();
        let Shape::Sphere(r) = ball.shape else { panic!() };
        assert!((r - 18.0_f32.sqrt()).abs() < 1e-4);
        assert_eq!((ball.below, ball.above), (0.0, 0.0));
        // Out of a sphere, the heights are its radius each way.
        let cylinder = with_shape(&Area::sphere(3.0), ShapeKind::Radius, 0.0).unwrap();
        assert_eq!(cylinder.shape, Shape::Radius(3.0));
        assert_eq!((cylinder.below, cylinder.above), (3.0, 3.0));
        // A box as a path is its corners turned to the marker.
        let long = Area::boxed([2.0, 1.0]);
        let Shape::Path(p) = with_shape(&long, ShapeKind::Path, QUARTER).unwrap().shape else {
            panic!()
        };
        assert_eq!(p.len(), 4);
        assert!((p[1][0] - 1.0).abs() < 1e-4 && (p[1][1] - 2.0).abs() < 1e-4, "{p:?}");
        // And back: the tightest box in the marker's axes.
        let back = with_shape(&with_shape(&long, ShapeKind::Path, QUARTER).unwrap(), ShapeKind::Box, QUARTER)
            .unwrap();
        let Shape::Box { half } = back.shape else { panic!() };
        assert!((half[0] - 2.0).abs() < 1e-4 && (half[1] - 1.0).abs() < 1e-4, "{half:?}");
        // The same kind is the same area.
        assert_eq!(with_shape(&long, ShapeKind::Box, 0.0).unwrap(), long);
        // A reach past the limit is refused by name.
        let far = Area::boxed([50.0, 50.0]);
        assert!(matches!(
            with_shape(&far, ShapeKind::Radius, 0.0),
            Err(Refused::Area(AreaError::Radius(_)))
        ));
    }

    #[test]
    fn the_yaw_of_a_forward_axis_is_counter_clockwise_from_x() {
        assert!((yaw_of([0.0, 1.0, 0.0]) - QUARTER).abs() < 1e-6);
        assert_eq!(yaw_of([1.0, 0.0, 0.0]), 0.0);
    }
}
