//! **From render**: collision polygons from a render section's triangles
//! (docs/editor/modeling-targets.md, "Collision and physics models").
//!
//! Positions are welded; triangles left degenerate, and repeats of a
//! triangle already taken, are dropped. A triangle whose three vertices
//! an earlier one has in the other winding (a sheet drawn from both
//! sides) is dropped too, and the earlier one becomes two-sided. Then
//! neighbours join: two polygons sharing an edge in opposite directions,
//! of one material and the same flags, with normals within
//! [`FromRender::angle`] and every vertex of each within
//! [`FromRender::planar`] of the other's plane, become one when the result
//! is convex and has at most eight vertices. Polygons are visited in
//! order and each joins its lowest-numbered partner first, until nothing
//! joins, so the result depends only on the input.

use std::collections::HashMap;

use crate::geom::{self, V3};
use crate::prep::{self, Welder};
use crate::{DEFAULT_WELD, MAX_RING, NO_BREAKABLE, Polygon, TWO_SIDED};

/// How triangles weld and join.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FromRender {
    pub weld: f32,
    /// Largest angle between two neighbours' normals that may join, in
    /// degrees.
    pub angle: f32,
    /// Largest distance a vertex may lie off the joined plane.
    pub planar: f32,
}

impl Default for FromRender {
    fn default() -> Self {
        Self {
            weld: DEFAULT_WELD,
            angle: 0.5,
            planar: 1e-3,
        }
    }
}

struct Face {
    ring: Vec<u32>,
    material: u16,
    flags: u8,
    alive: bool,
}

/// Collision polygons from `triangles` over `positions`. `materials` is
/// per triangle (empty: all 0), already in the collision tag's material
/// list.
pub fn from_render(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    materials: &[u16],
    opts: &FromRender,
) -> Vec<Polygon> {
    let mut welder = Welder::new(opts.weld);
    let ids: Vec<u32> = positions
        .iter()
        .map(|&p| if p.iter().all(|c| c.is_finite()) { welder.add(p) } else { u32::MAX })
        .collect();
    let pts: Vec<V3> = welder.points.iter().map(|&p| geom::wide(p)).collect();
    let mut faces: Vec<Face> = Vec::new();
    // Sorted vertices to (face, the triangle's winding parity).
    let mut taken: HashMap<[u32; 3], (usize, bool)> = HashMap::new();
    for (t, tri) in triangles.iter().enumerate() {
        let Some(ring) = tri
            .iter()
            .map(|&i| ids.get(i as usize).copied().filter(|&v| v != u32::MAX))
            .collect::<Option<Vec<u32>>>()
        else {
            continue;
        };
        let Some(ring) = prep::tidy_ring(&ring) else {
            continue;
        };
        let p3: Vec<V3> = ring.iter().map(|&v| pts[v as usize]).collect();
        if geom::area(&p3) <= geom::AREA_EPS {
            continue;
        }
        let mut key = [ring[0], ring[1], ring[2]];
        key.sort_unstable();
        // The winding: whether the ring is an even rotation of `key`.
        let at = ring.iter().position(|&v| v == key[0]).unwrap_or(0);
        let even = ring[(at + 1) % 3] == key[1];
        if let Some(&(face, was)) = taken.get(&key) {
            if was != even {
                faces[face].flags |= TWO_SIDED;
            }
            continue;
        }
        taken.insert(key, (faces.len(), even));
        faces.push(Face {
            ring,
            material: materials.get(t).copied().unwrap_or(0),
            flags: 0,
            alive: true,
        });
    }

    let cos = f64::from(opts.angle.to_radians().cos());
    let planar = f64::from(opts.planar);
    loop {
        let mut joined = false;
        let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, f) in faces.iter().enumerate().filter(|(_, f)| f.alive) {
            for k in 0..f.ring.len() {
                let e = (f.ring[k], f.ring[(k + 1) % f.ring.len()]);
                by_edge.entry(e).or_default().push(i);
            }
        }
        for i in 0..faces.len() {
            if !faces[i].alive {
                continue;
            }
            let ring = faces[i].ring.clone();
            let mut partners: Vec<usize> = (0..ring.len())
                .flat_map(|k| {
                    let back = (ring[(k + 1) % ring.len()], ring[k]);
                    by_edge.get(&back).cloned().unwrap_or_default()
                })
                .filter(|&j| {
                    j != i
                        && faces[j].alive
                        && faces[j].material == faces[i].material
                        && faces[j].flags == faces[i].flags
                })
                .collect();
            partners.sort_unstable();
            partners.dedup();
            for j in partners {
                if let Some(merged) = join(&pts, &faces[i].ring, &faces[j].ring, cos, planar) {
                    faces[i].ring = merged;
                    faces[j].alive = false;
                    joined = true;
                    break;
                }
            }
        }
        if !joined {
            break;
        }
    }
    faces
        .into_iter()
        .filter(|f| f.alive)
        .map(|f| Polygon {
            points: f.ring.iter().map(|&v| welder.points[v as usize]).collect(),
            material: f.material,
            flags: f.flags,
            breakable_surface: NO_BREAKABLE,
        })
        .collect()
}

fn plane(pts: &[V3], ring: &[u32]) -> (V3, f64) {
    let p: Vec<V3> = ring.iter().map(|&v| pts[v as usize]).collect();
    let nv = geom::newell(&p);
    let n = nv.map(|c| c / geom::len(nv));
    (n, geom::dot(n, geom::centroid(&p)))
}

/// `a` and `b` as one polygon, when they share an edge in opposite
/// directions and the union is flat, convex and small enough.
fn join(pts: &[V3], a: &[u32], b: &[u32], cos: f64, planar: f64) -> Option<Vec<u32>> {
    let (na, da) = plane(pts, a);
    let (nb, db) = plane(pts, b);
    if geom::dot(na, nb) < cos {
        return None;
    }
    let off = |ring: &[u32], n: V3, d: f64| {
        ring.iter()
            .all(|&v| (geom::dot(n, pts[v as usize]) - d).abs() <= planar)
    };
    if !off(b, na, da) || !off(a, nb, db) {
        return None;
    }
    let (la, lb) = (a.len(), b.len());
    let (i, j) = (0..la).find_map(|i| {
        let (x, y) = (a[i], a[(i + 1) % la]);
        (0..lb)
            .find(|&j| b[j] == y && b[(j + 1) % lb] == x)
            .map(|j| (i, j))
    })?;
    // a from its edge's end round to its start, then b's other corners.
    let mut ring: Vec<u32> = (0..la).map(|k| a[(i + 1 + k) % la]).collect();
    ring.extend((0..lb - 2).map(|k| b[(j + 2 + k) % lb]));
    let ring = prep::tidy_ring(&ring)?;
    let ring = drop_collinear(pts, ring);
    if ring.len() > MAX_RING || ring.len() < 3 {
        return None;
    }
    let p: Vec<V3> = ring.iter().map(|&v| pts[v as usize]).collect();
    prep::convex(&p, na).then_some(ring)
}

/// A ring without corners that lie on the line of their neighbours.
fn drop_collinear(pts: &[V3], mut ring: Vec<u32>) -> Vec<u32> {
    let mut k = 0;
    while ring.len() > 3 && k < ring.len() {
        let n = ring.len();
        let (a, b, c) = (
            pts[ring[(k + n - 1) % n] as usize],
            pts[ring[k] as usize],
            pts[ring[(k + 1) % n] as usize],
        );
        let (e1, e2) = (geom::sub(b, a), geom::sub(c, b));
        let s = geom::len(geom::cross(e1, e2));
        if s <= 1e-6 * geom::len(e1) * geom::len(e2) && geom::dot(e1, e2) > 0.0 {
            ring.remove(k);
        } else {
            k += 1;
        }
    }
    ring
}
