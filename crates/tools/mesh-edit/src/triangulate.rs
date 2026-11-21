//! Polygon triangulation by ear clipping.

use crate::math::{self, Vec3};

/// Splits a polygon (its corners in winding order) into `n - 2` triangles
/// of corner indices, each wound like the polygon.
///
/// Ear clipping in the plane that drops the normal's largest axis, so
/// concave polygons come out right. A polygon with no ear left (degenerate,
/// or self-intersecting) has its remainder fanned.
pub fn triangulate(points: &[Vec3]) -> Vec<[usize; 3]> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }
    let normal = math::newell(points);
    let axis = (0..3)
        .max_by(|&a, &b| normal[a].abs().total_cmp(&normal[b].abs()))
        .unwrap_or(2);
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    let flat: Vec<[f32; 2]> = points.iter().map(|p| [p[u], p[v]]).collect();
    // The projection keeps a counter-clockwise polygon counter-clockwise
    // when the dropped axis's normal component is positive.
    let sign = if normal[axis] >= 0.0 { 1.0 } else { -1.0 };
    let cross = |a: usize, b: usize, c: usize| -> f32 {
        let (pa, pb, pc) = (flat[a], flat[b], flat[c]);
        ((pb[0] - pa[0]) * (pc[1] - pa[1]) - (pb[1] - pa[1]) * (pc[0] - pa[0])) * sign
    };

    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    while idx.len() > 3 {
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            let blocked = idx.iter().any(|&p| {
                p != a
                    && p != b
                    && p != c
                    && cross(a, b, p) >= 0.0
                    && cross(b, c, p) >= 0.0
                    && cross(c, a, p) >= 0.0
            });
            if blocked {
                continue;
            }
            out.push([a, b, c]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            for k in 1..idx.len() - 1 {
                out.push([idx[0], idx[k], idx[k + 1]]);
            }
            return out;
        }
    }
    out.push([idx[0], idx[1], idx[2]]);
    out
}

#[cfg(test)]
mod tests {
    use super::triangulate;

    fn area(points: &[[f32; 3]], t: [usize; 3]) -> f32 {
        let (a, b, c) = (points[t[0]], points[t[1]], points[t[2]]);
        ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) * 0.5
    }

    #[test]
    fn concave_l_shape_has_only_positive_triangles() {
        let l = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 2.0, 0.0],
            [0.0, 2.0, 0.0],
        ];
        let tris = triangulate(&l);
        assert_eq!(tris.len(), 4);
        let total: f32 = tris.iter().map(|&t| area(&l, t)).sum();
        assert!((total - 3.0).abs() < 1e-5, "area {total}");
        for &t in &tris {
            assert!(area(&l, t) > 0.0, "{t:?} is wound backwards or flat");
        }
    }

    #[test]
    fn clockwise_polygon_keeps_its_winding() {
        let square = [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
        for t in triangulate(&square) {
            assert!(area(&square, t) < 0.0);
        }
    }

    #[test]
    fn collinear_corners_still_give_n_minus_two() {
        // A quad with a vertex inserted on two sides, as a loop cut leaves it.
        let hex = [
            [0.0, 0.0, 0.0],
            [0.5, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.5, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        assert_eq!(triangulate(&hex).len(), 4);
    }
}
