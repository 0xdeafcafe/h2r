//! Busy points: one floor vertex where many collision edges meet
//! (docs/game/superbounce.md, "Busy points"). Coplanar triangulation
//! edges count as much as creases: the stressed, busy points of a floor's
//! mesh are where the capsule finds the most edges at once. The target is
//! the vertex itself, to the float. Candidates for the sim's training,
//! like single creases ([`crate::Spot::candidate`]).
use std::collections::HashMap;

use crate::bounce::{LAUNCH_PER_CREASE, MAX_LAUNCH, MIN_LAUNCH};
use crate::geom::{dot, len, sub};
use crate::scan::{Face, ORIGIN_LIFT, WALKABLE};
use crate::{Kind, Shape, Spot, Vec3};

/// A busy point has at least this many distinct floor edges at its
/// vertex: a plain grid corner's four.
pub(crate) const MIN_VALENCE: usize = 4;
/// Two floors at a vertex are one plane: their normals' dot at least this
/// (the seam test's, `bounce::SEAM`).
const PLANE: f32 = 0.999;
/// Vertices this close (world units, each axis) are one: a millimetre.
/// The kept coordinate is the first floor's own vertex, unrounded.
const GRAIN: f32 = 1e-3;
/// Edges shorter than this are slivers, not counted.
const MIN_EDGE: f32 = 0.02;
/// Each edge past [`MIN_VALENCE`] is a point, to [`MAX_EDGE_POINTS`]; each
/// plane past the first is a point, to [`MAX_PLANE_POINTS`].
const MAX_EDGE_POINTS: usize = 6;
const MAX_PLANE_POINTS: usize = 3;
/// The floor: a busy point with fewer points than this is no spot. Two:
/// six flat edges (a triangulated grid's vertex), five over two planes, or
/// four over three. A plain four-edge grid corner, flat, has none and is
/// left out, so a floor's every vertex does not flood the list.
// TODO(sim): unmeasured. Raise it if a map's scan lists too many to train.
pub(crate) const MIN_POINTS: usize = 2;
/// A busy point's score: this, plus [`POINT_SCORE`] a point. 0.1 to 0.55,
/// under every slot and most creases.
const BASE_SCORE: f32 = 0.1;
const POINT_SCORE: f32 = 0.05;

/// One floor vertex and what meets there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fan {
    /// The vertex, as the first walkable polygon with it stores it.
    pub at: Vec3,
    /// Distinct walkable-floor edges ending at it, coplanar ones too.
    pub valence: usize,
    /// Distinct planes among the walkable floors that have it.
    pub planes: usize,
    /// The sharpest `1 − n_a · n_b` between two of those floors.
    pub crease: f32,
}

type Key = [i64; 3];

fn key(p: Vec3) -> Key {
    p.map(|v| (v / GRAIN).round() as i64)
}

/// A busy point's points: edges past [`MIN_VALENCE`] and planes past one,
/// each capped.
pub(crate) fn points(valence: usize, planes: usize) -> usize {
    valence.saturating_sub(MIN_VALENCE).min(MAX_EDGE_POINTS)
        + planes.saturating_sub(1).min(MAX_PLANE_POINTS)
}

/// A busy point's score from its edges and planes: more of either is more.
pub(crate) fn score(valence: usize, planes: usize) -> f32 {
    BASE_SCORE + POINT_SCORE * points(valence, planes) as f32
}

/// Every walkable-floor vertex with at least [`MIN_VALENCE`] edges, in
/// vertex order (deterministic, whatever the hash map's).
pub(crate) fn fans(faces: &[Face]) -> Vec<Fan> {
    struct Acc {
        at: Vec3,
        first: usize,
        ends: Vec<Key>,
        normals: Vec<Vec3>,
    }
    let mut by: HashMap<Key, Acc> = HashMap::new();
    let mut order = 0;
    for f in faces.iter().filter(|f| f.poly.normal[2] > WALKABLE) {
        let v = &f.poly.verts;
        let n = v.len();
        for (k, &p) in v.iter().enumerate() {
            let here = key(p);
            let acc = by.entry(here).or_insert_with(|| {
                order += 1;
                Acc {
                    at: p,
                    first: order,
                    ends: Vec::new(),
                    normals: Vec::new(),
                }
            });
            for e in [v[(k + n - 1) % n], v[(k + 1) % n]] {
                let end = key(e);
                if end != here && len(sub(e, p)) >= MIN_EDGE && !acc.ends.contains(&end) {
                    acc.ends.push(end);
                }
            }
            acc.normals.push(f.poly.normal);
        }
    }
    let mut out: Vec<(usize, Fan)> = by
        .into_values()
        .filter(|a| a.ends.len() >= MIN_VALENCE)
        .map(|a| {
            let mut planes: Vec<Vec3> = Vec::new();
            for &n in &a.normals {
                if !planes.iter().any(|&q| dot(q, n) >= PLANE) {
                    planes.push(n);
                }
            }
            let crease = planes
                .iter()
                .enumerate()
                .flat_map(|(i, &p)| planes[i + 1..].iter().map(move |&q| 1.0 - dot(p, q)))
                .fold(0.0f32, f32::max);
            (
                a.first,
                Fan {
                    at: a.at,
                    valence: a.ends.len(),
                    planes: planes.len(),
                    crease,
                },
            )
        })
        .collect();
    out.sort_by_key(|(first, _)| *first);
    out.into_iter().map(|(_, f)| f).collect()
}

/// The busy points: each fan with at least [`MIN_POINTS`], as a candidate
/// spot on its vertex.
///
/// - **edge**: the vertex twice, to the float.
/// - **stand**: on the vertex, the origin's lift over it.
/// - **launch**: unmeasured; the sharpest crease's estimate
///   (`bounce::LAUNCH_PER_CREASE`), at least `bounce::MIN_LAUNCH` for a
///   flat one.
/// - **drift**: none; a point has no across.
/// - **score**: [`score`].
pub(crate) fn busy(faces: &[Face]) -> Vec<Spot> {
    fans(faces)
        .into_iter()
        .filter(|f| points(f.valence, f.planes) >= MIN_POINTS)
        .map(|f| Spot {
            kind: Kind::Bounce,
            shape: Shape::Busy,
            edge: [f.at, f.at],
            stand: [f.at[0], f.at[1], f.at[2] + ORIGIN_LIFT],
            launch: (LAUNCH_PER_CREASE * f.crease).clamp(MIN_LAUNCH, MAX_LAUNCH),
            score: score(f.valence, f.planes),
            ..Spot::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Poly;
    use crate::geom::Bounds;

    /// The busy vertex every fan shares: not a round number, so an exact
    /// match is the vertex's own float.
    const AT: Vec3 = [1.237, -2.519, 0.733];

    /// `n` walkable triangles round [`AT`], a unit out, each `dip` higher
    /// at the rim than at the middle (0: flat, one plane).
    fn fan(n: usize, dip: f32) -> Vec<Poly> {
        let rim = |i: usize| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            [AT[0] + a.cos(), AT[1] + a.sin(), AT[2] + dip]
        };
        (0..n)
            .map(|i| {
                let verts = vec![AT, rim(i), rim(i + 1)];
                let u = sub(verts[1], verts[0]);
                let v = sub(verts[2], verts[0]);
                let c = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let l = len(c);
                Poly {
                    verts,
                    normal: c.map(|x| x / l),
                }
            })
            .collect()
    }

    fn faces(polys: &[Poly]) -> Vec<Face<'_>> {
        polys
            .iter()
            .map(|poly| Face {
                poly,
                b: Bounds::of(&poly.verts),
            })
            .collect()
    }

    fn the_fan(polys: &[Poly]) -> Fan {
        let fans = fans(&faces(polys));
        assert_eq!(fans.len(), 1, "{fans:?}");
        fans[0]
    }

    #[test]
    fn a_vertex_shared_by_six_floor_triangles_is_a_busy_point_on_it() {
        let polys = fan(6, 0.0);
        let f = the_fan(&polys);
        assert_eq!((f.valence, f.planes), (6, 1));
        // Through the whole scan: one spot, a candidate, on the vertex to
        // the float.
        let spots = crate::scan(&polys);
        assert_eq!(spots.len(), 1, "{spots:?}");
        let s = spots[0];
        assert_eq!((s.kind, s.shape), (Kind::Bounce, Shape::Busy));
        assert_eq!(s.edge, [AT, AT]);
        assert_eq!(s.stand, [AT[0], AT[1], AT[2] + ORIGIN_LIFT]);
        assert!(s.candidate());
        assert_eq!((s.need, s.fall, s.drift), (0.0, 0.0, [0.0, 0.0]));
        assert_eq!(crate::catch_miss(s.edge, [AT[0], AT[1]]), 0.0);
        assert!(crate::catch_miss(s.edge, [AT[0] + 0.2, AT[1]]) > 0.0);
    }

    #[test]
    fn a_plain_four_triangle_corner_scores_under_six_and_eight_edge_ones() {
        let score_of = |n: usize, dip: f32| {
            let f = the_fan(&fan(n, dip));
            assert_eq!(f.valence, n);
            score(f.valence, f.planes)
        };
        let (four, six, eight) = (score_of(4, 0.0), score_of(6, 0.0), score_of(8, 0.0));
        assert!(four < six && six < eight, "{four} {six} {eight}");
        // The plain corner is under the floor and left out of the scan;
        // the six and eight are kept.
        assert!(points(4, 1) < MIN_POINTS);
        assert!(crate::scan(&fan(4, 0.0)).is_empty());
        assert_eq!(crate::scan(&fan(8, 0.0)).len(), 1);
        // More planes meeting is busier: the same six edges in a dip, each
        // triangle its own plane, outrank the flat six.
        let dipped = the_fan(&fan(6, 0.05));
        assert_eq!(dipped.planes, 6);
        assert!(score(dipped.valence, dipped.planes) > six);
        // Four edges over four planes clear the floor too.
        assert!(points(4, 4) >= MIN_POINTS);
    }

    #[test]
    fn slivers_and_repeated_edges_are_not_counted() {
        // Each edge at the vertex is two triangles' and counts once; a
        // 1 cm sliver adds none.
        let mut polys = fan(6, 0.0);
        polys.push(Poly {
            verts: vec![
                AT,
                [AT[0] + 0.01, AT[1], AT[2]],
                [AT[0], AT[1] + 0.01, AT[2]],
            ],
            normal: [0.0, 0.0, 1.0],
        });
        assert_eq!(the_fan(&polys).valence, 6);
    }
}
