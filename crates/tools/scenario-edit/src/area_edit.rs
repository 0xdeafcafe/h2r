//! Editing an area with handles (docs/game/gametype-markers.md,
//! "Studio"): where the handles of a marker's area are, and the area a
//! drag of one makes. Pure: the viewport finds the handle under the
//! cursor and the point it was dragged to, and the session stores what
//! this returns when `Area::validate` passes.
//!
//! A radius has one handle on its rim, and a path has a handle on each
//! point and one on the middle of each edge (dragging it adds a point
//! there). The top and the bottom of the wall each have a handle for the
//! heights.
use game_markers::area::{MAX_POINTS, MIN_POINTS};
use game_markers::{Area, AreaError, Shape};

use crate::geometry::Vec3;

/// One handle of an area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// A radius's rim.
    Rim,
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
        !matches!(self, Handle::Top | Handle::Bottom)
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

/// Every handle of `area` for a marker at `centre`, with its place in
/// the world (a flat handle at the marker's height).
pub fn handles(centre: Vec3, area: &Area) -> Vec<(Handle, Vec3)> {
    let mut out = Vec::new();
    match &area.shape {
        Shape::Radius(r) => out.push((Handle::Rim, [centre[0] + r, centre[1], centre[2]])),
        // A box and a sphere have no handle but their heights until their
        // editor lands (a box's, not a sphere's).
        Shape::Box { .. } | Shape::Sphere(_) => {}
        Shape::Path(points) => {
            let n = points.len();
            for (i, p) in points.iter().enumerate() {
                out.push((Handle::Point(i), [centre[0] + p[0], centre[1] + p[1], centre[2]]));
            }
            for i in 0..n {
                let (a, b) = (points[i], points[(i + 1) % n]);
                out.push((
                    Handle::Edge(i),
                    [
                        centre[0] + (a[0] + b[0]) / 2.0,
                        centre[1] + (a[1] + b[1]) / 2.0,
                        centre[2],
                    ],
                ));
            }
        }
    }
    if !matches!(area.shape, Shape::Sphere(_)) {
        out.push((Handle::Top, [centre[0], centre[1], centre[2] + area.above]));
        out.push((Handle::Bottom, [centre[0], centre[1], centre[2] - area.below]));
    }
    out
}

/// The area after handle `h` is dragged to the world point `to`, or why
/// not. The result passes `Area::validate`; a refused drag leaves `area`
/// as it was (the caller keeps it).
pub fn drag(area: &Area, h: Handle, centre: Vec3, to: Vec3) -> Result<Area, Refused> {
    let offset = [to[0] - centre[0], to[1] - centre[1]];
    let mut out = area.clone();
    match (h, &mut out.shape) {
        (Handle::Rim, Shape::Radius(r)) => *r = offset[0].hypot(offset[1]),
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

/// The inspector's **Shape** row: a radius as the path of its outline, or
/// a path as the radius that reaches its farthest point from the
/// marker. The heights stay.
pub fn with_shape(area: &Area, path: bool) -> Result<Area, Refused> {
    let mut out = area.clone();
    out.shape = match (&area.shape, path) {
        (Shape::Radius(_), true) => Shape::Path(area.outline()),
        (Shape::Path(points), false) => Shape::Radius(
            points
                .iter()
                .map(|p| p[0].hypot(p[1]))
                .fold(0.0_f32, f32::max),
        ),
        _ => return Ok(out),
    };
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
        let h = handles(AT, &Area::radius(3.0));
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
        let h = handles(AT, &triangle());
        assert_eq!(h.len(), 3 + 3 + 2);
        assert_eq!(h[0], (Handle::Point(0), [8.0, 3.0, 1.0]));
        assert_eq!(h[3], (Handle::Edge(0), [10.0, 3.0, 1.0]));
        assert_eq!(h[5], (Handle::Edge(2), [9.0, 5.0, 1.0]));
    }

    #[test]
    fn a_rim_drag_sets_the_radius_and_a_refused_one_changes_nothing() {
        let area = Area::radius(3.0);
        let wide = drag(&area, Handle::Rim, AT, [16.0, 5.0, 1.0]).unwrap();
        assert_eq!(wide.shape, Shape::Radius(6.0));
        assert_eq!(area.shape, Shape::Radius(3.0));
        // Too small, then too large: the radius limits' sentence.
        let e = drag(&area, Handle::Rim, AT, [10.05, 5.0, 1.0]).unwrap_err();
        assert!(matches!(e, Refused::Area(AreaError::Radius(_))), "{e}");
        let e = drag(&area, Handle::Rim, AT, [200.0, 5.0, 1.0]).unwrap_err();
        assert!(matches!(e, Refused::Area(AreaError::Radius(_))), "{e}");
    }

    #[test]
    fn an_edge_handle_adds_a_point_after_its_edges_start() {
        let area = triangle();
        let out = drag(&area, Handle::Edge(0), AT, [10.0, 2.0, 1.0]).unwrap();
        let Shape::Path(p) = &out.shape else { panic!() };
        assert_eq!(p.len(), 4);
        assert_eq!(p[1], [0.0, -3.0]);
        assert_eq!(p[2], [2.0, -2.0]);
    }

    #[test]
    fn a_point_drag_that_crosses_the_path_is_refused() {
        let square = Area::path(vec![[-2.0, -2.0], [2.0, -2.0], [2.0, 2.0], [-2.0, 2.0]]);
        // Point 2 dragged across the left side makes a bow tie.
        let e = drag(&square, Handle::Point(2), AT, [7.0, 5.0, 1.0]).unwrap_err();
        assert!(matches!(e, Refused::Area(AreaError::Crosses(..))), "{e}");
        let ok = drag(&square, Handle::Point(0), AT, [7.0, 2.0, 1.0]).unwrap();
        let Shape::Path(p) = &ok.shape else { panic!() };
        assert_eq!(p[0], [-3.0, -3.0]);
    }

    #[test]
    fn the_wall_handles_set_the_heights() {
        let area = Area::radius(3.0);
        let tall = drag(&area, Handle::Top, AT, [10.0, 5.0, 5.0]).unwrap();
        assert_eq!((tall.above, tall.below), (4.0, 0.5));
        let deep = drag(&area, Handle::Bottom, AT, [10.0, 5.0, -1.0]).unwrap();
        assert_eq!(deep.below, 2.0);
        // Under the marker's floor is a negative reach: refused.
        assert!(drag(&area, Handle::Top, AT, [10.0, 5.0, 0.0]).is_err());
    }

    #[test]
    fn a_handle_of_the_other_shape_is_not_one() {
        assert_eq!(
            drag(&Area::radius(3.0), Handle::Point(0), AT, AT),
            Err(Refused::NoHandle)
        );
        assert_eq!(drag(&triangle(), Handle::Rim, AT, AT), Err(Refused::NoHandle));
        assert_eq!(drag(&triangle(), Handle::Point(9), AT, AT), Err(Refused::NoHandle));
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
        let path = with_shape(&r, true).unwrap();
        let Shape::Path(p) = &path.shape else { panic!() };
        assert_eq!(p.len(), game_markers::area::SEGMENTS);
        let back = with_shape(&path, false).unwrap();
        let Shape::Radius(r) = back.shape else { panic!() };
        assert!((r - 3.0).abs() < 1e-3);
        assert_eq!(with_shape(&path, true).unwrap(), path);
    }
}
