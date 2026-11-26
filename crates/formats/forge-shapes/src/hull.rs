//! The convex hull of integer points by exact arithmetic
//! (docs/game/forge-blockout.md, "The natural set").
//!
//! Every test is an integer sign, so the hull of a point set is the same on
//! every host and a face is exactly planar: its vertices are the points of
//! one supporting plane. Coordinates are expected within 2^20 so no product
//! passes an `i128`. The search is every triple of points (a few thousand for
//! the 34 points of a rock), kept for being plain and not for speed.

use std::collections::{BTreeMap, BTreeSet};

/// A point on the integer grid.
pub type IPoint = [i64; 3];

/// A convex polyhedron: points, and faces as indices into them wound
/// counter-clockwise seen from outside. A face holds only its corners: a
/// point along an edge or inside a face is not one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Polyhedron {
    pub points: Vec<IPoint>,
    pub faces: Vec<Vec<u32>>,
}

type Wide = [i128; 3];

fn wide(p: IPoint) -> Wide {
    [i128::from(p[0]), i128::from(p[1]), i128::from(p[2])]
}

fn diff(a: IPoint, b: IPoint) -> Wide {
    [
        i128::from(a[0]) - i128::from(b[0]),
        i128::from(a[1]) - i128::from(b[1]),
        i128::from(a[2]) - i128::from(b[2]),
    ]
}

fn cross(a: Wide, b: Wide) -> Wide {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: Wide, b: Wide) -> i128 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The sign of the volume of the tetrahedron `a b c d`: positive when `d` is
/// on the side of the plane `a b c` that its counter-clockwise normal points
/// to, zero when the four are coplanar.
pub fn orientation(a: IPoint, b: IPoint, c: IPoint, d: IPoint) -> i128 {
    dot(cross(diff(b, a), diff(c, a)), diff(d, a)).signum()
}

/// The convex hull of the points, or `None` when they all lie in one plane
/// (or there are fewer than four distinct).
pub fn convex_hull(input: &[IPoint]) -> Option<Polyhedron> {
    let mut pts = input.to_vec();
    pts.sort_unstable();
    pts.dedup();
    let n = pts.len();

    // Every plane through three points that has all the others on or behind
    // it, as an outward normal in lowest terms and the offset `normal . p`.
    let mut planes: BTreeSet<(Wide, i128)> = BTreeSet::new();
    for i in 0..n {
        for j in i + 1..n {
            for k in j + 1..n {
                let normal = cross(diff(pts[j], pts[i]), diff(pts[k], pts[i]));
                if normal == [0; 3] {
                    continue;
                }
                let (mut above, mut below) = (false, false);
                for &p in &pts {
                    match dot(normal, diff(p, pts[i])).signum() {
                        1 => above = true,
                        -1 => below = true,
                        _ => {}
                    }
                    if above && below {
                        break;
                    }
                }
                if above && below {
                    continue;
                }
                if !above && !below {
                    return None;
                }
                let normal = if above {
                    [-normal[0], -normal[1], -normal[2]]
                } else {
                    normal
                };
                let g = gcd(gcd(normal[0], normal[1]), normal[2]);
                let normal = [normal[0] / g, normal[1] / g, normal[2] / g];
                planes.insert((normal, dot(normal, wide(pts[i]))));
            }
        }
    }
    if planes.len() < 4 {
        return None;
    }

    let rings: Vec<Vec<IPoint>> = planes
        .iter()
        .map(|&(normal, offset)| face_ring(&pts, normal, offset))
        .collect();
    let corners: BTreeSet<IPoint> = rings.iter().flatten().copied().collect();
    let points: Vec<IPoint> = corners.into_iter().collect();
    let index: BTreeMap<IPoint, u32> = points.iter().zip(0u32..).map(|(&p, i)| (p, i)).collect();
    let faces = rings
        .iter()
        .map(|ring| ring.iter().map(|p| index[p]).collect())
        .collect();
    Some(Polyhedron { points, faces })
}

/// The corners of the face on a plane, counter-clockwise about its normal: the
/// two-dimensional hull (Andrew's chain, strict turns only) of the points on
/// the plane, seen along the axis the normal is most along.
fn face_ring(pts: &[IPoint], normal: Wide, offset: i128) -> Vec<IPoint> {
    let on_plane: Vec<IPoint> = pts
        .iter()
        .copied()
        .filter(|&p| dot(normal, wide(p)) == offset)
        .collect();
    let axis = (0..3).max_by_key(|&a| normal[a].abs()).unwrap_or(2);
    let (iu, iv) = ((axis + 1) % 3, (axis + 2) % 3);
    let flat = |p: IPoint| (i128::from(p[iu]), i128::from(p[iv]));
    let turn = |o: IPoint, a: IPoint, b: IPoint| {
        let (o, a, b) = (flat(o), flat(a), flat(b));
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };

    let mut order = on_plane;
    order.sort_unstable_by_key(|&p| flat(p));
    let mut ring: Vec<IPoint> = Vec::with_capacity(order.len() * 2);
    for &p in &order {
        while ring.len() >= 2 && turn(ring[ring.len() - 2], ring[ring.len() - 1], p) <= 0 {
            ring.pop();
        }
        ring.push(p);
    }
    let floor = ring.len() + 1;
    for &p in order.iter().rev().skip(1) {
        while ring.len() >= floor && turn(ring[ring.len() - 2], ring[ring.len() - 1], p) <= 0 {
            ring.pop();
        }
        ring.push(p);
    }
    ring.pop();
    // The axes (u, v) are cyclic, so counter-clockwise in them is
    // counter-clockwise about +axis; a normal along -axis wants the reverse.
    if normal[axis] < 0 {
        ring.reverse();
    }
    ring
}
