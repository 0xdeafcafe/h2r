//! Input to surfaces: weld, drop the degenerate, triangulate what is not
//! convex and planar, split rings over eight, and gather planes.

use std::collections::HashMap;

use crate::geom::{self, V3};
use crate::{Error, MAX_RING, Polygon, Report, Result};

/// Largest distance a corner may lie off its polygon's plane before the
/// polygon is triangulated (world units).
pub(crate) const PLANAR_EPS: f64 = 1e-3;
/// Two planes are one when their normals' cosine is past this, their
/// distances differ by less than [`SAME_DISTANCE`], and every corner of
/// the new surface lies within [`PLANAR_EPS`] of the plane found.
const SAME_NORMAL: f64 = 1.0 - 1e-6;
const SAME_DISTANCE: f64 = 1e-3;

/// One surface to be: a ring of vertex indices and its fields.
#[derive(Clone, Debug)]
pub(crate) struct PrepSurface {
    pub ring: Vec<u32>,
    pub source: usize,
    pub material: u16,
    pub flags: u8,
    pub breakable_surface: u8,
    pub plane: usize,
    /// The surface faces the plane's back.
    pub flip: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    pub points: Vec<[f32; 3]>,
    pub surfaces: Vec<PrepSurface>,
    pub planes: Vec<[f32; 4]>,
    pub report: Report,
}

impl Prepared {
    pub(crate) fn ring_points(&self, s: usize) -> Vec<V3> {
        self.surfaces[s]
            .ring
            .iter()
            .map(|&v| geom::wide(self.points[v as usize]))
            .collect()
    }

    pub(crate) fn plane64(&self, p: usize) -> (V3, f64) {
        let [x, y, z, d] = self.planes[p];
        (geom::wide([x, y, z]), f64::from(d))
    }

    /// The surface's open-side normal.
    pub(crate) fn open_normal(&self, s: usize) -> V3 {
        let (n, _) = self.plane64(self.surfaces[s].plane);
        if self.surfaces[s].flip {
            n.map(|c| -c)
        } else {
            n
        }
    }
}

/// Points welded within a distance, lowest input index first.
pub(crate) struct Welder {
    weld: f64,
    cells: HashMap<[i64; 3], Vec<u32>>,
    pub points: Vec<[f32; 3]>,
    pub merged: usize,
}

impl Welder {
    pub(crate) fn new(weld: f32) -> Self {
        Self {
            weld: f64::from(weld.max(0.0)),
            cells: HashMap::new(),
            points: Vec::new(),
            merged: 0,
        }
    }

    fn cell(&self, p: [f32; 3]) -> [i64; 3] {
        if self.weld == 0.0 {
            return p.map(|c| i64::from(c.to_bits()));
        }
        p.map(|c| (f64::from(c) / self.weld).floor() as i64)
    }

    pub(crate) fn add(&mut self, p: [f32; 3]) -> u32 {
        let c = self.cell(p);
        let reach = if self.weld == 0.0 { 0 } else { 1 };
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let Some(list) = self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) else {
                        continue;
                    };
                    for &i in list {
                        let q = self.points[i as usize];
                        let near = if self.weld == 0.0 {
                            q.map(f32::to_bits) == p.map(f32::to_bits)
                        } else {
                            geom::len(geom::sub(geom::wide(p), geom::wide(q))) <= self.weld
                        };
                        if near {
                            self.merged += 1;
                            return i;
                        }
                    }
                }
            }
        }
        let i = self.points.len() as u32;
        self.points.push(p);
        self.cells.entry(c).or_default().push(i);
        i
    }
}

/// Whether a ring (positions) is convex about `n`.
pub(crate) fn convex(points: &[V3], n: V3) -> bool {
    let k = points.len();
    (0..k).all(|i| {
        let (a, b, c) = (points[i], points[(i + 1) % k], points[(i + 2) % k]);
        let (e1, e2) = (geom::sub(b, a), geom::sub(c, b));
        let s = geom::dot(geom::cross(e1, e2), n);
        s >= -1e-6 * geom::len(e1) * geom::len(e2)
    })
}

/// A ring with consecutive repeats removed; `None` when a vertex repeats
/// elsewhere or fewer than three are left.
pub(crate) fn tidy_ring(ring: &[u32]) -> Option<Vec<u32>> {
    let mut out: Vec<u32> = Vec::with_capacity(ring.len());
    for &v in ring {
        if out.last() != Some(&v) {
            out.push(v);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    let mut seen = out.clone();
    seen.sort_unstable();
    seen.dedup();
    (out.len() >= 3 && seen.len() == out.len()).then_some(out)
}

/// A convex ring in fans of at most [`MAX_RING`] vertices, all from its
/// first corner.
pub(crate) fn fan_split(ring: &[u32]) -> Vec<Vec<u32>> {
    if ring.len() <= MAX_RING {
        return vec![ring.to_vec()];
    }
    let mut out = Vec::new();
    let mut k = 1;
    while k < ring.len() - 1 {
        let end = (k + MAX_RING - 2).min(ring.len() - 1);
        let mut piece = vec![ring[0]];
        piece.extend_from_slice(&ring[k..=end]);
        out.push(piece);
        k = end;
    }
    out
}

pub(crate) fn prepare(polygons: &[Polygon], weld: f32) -> Result<Prepared> {
    let mut welder = Welder::new(weld);
    let mut report = Report::default();
    // (ring in welded indices, source polygon)
    let mut pieces: Vec<(Vec<u32>, usize)> = Vec::new();
    for (i, poly) in polygons.iter().enumerate() {
        if !poly.points.iter().flatten().all(|c| c.is_finite()) {
            return Err(Error::NonFinite(i));
        }
        let ring: Vec<u32> = poly.points.iter().map(|&p| welder.add(p)).collect();
        let Some(ring) = tidy_ring(&ring) else {
            report.dropped.push(i);
            continue;
        };
        let pts: Vec<V3> = ring
            .iter()
            .map(|&v| geom::wide(welder.points[v as usize]))
            .collect();
        let nv = geom::newell(&pts);
        let l = geom::len(nv);
        if l / 2.0 <= geom::AREA_EPS {
            report.dropped.push(i);
            continue;
        }
        let n = nv.map(|c| c / l);
        let d = geom::dot(n, geom::centroid(&pts));
        let (lo, hi) = geom::extent(&pts, n, d);
        let planar = lo >= -PLANAR_EPS && hi <= PLANAR_EPS;
        if !planar || !convex(&pts, n) {
            report.triangulated.push(i);
            let flat: Vec<[f32; 3]> = ring.iter().map(|&v| welder.points[v as usize]).collect();
            for t in mesh_edit::triangulate(&flat) {
                let tri: Vec<u32> = t.iter().map(|&k| ring[k]).collect();
                let tp: Vec<V3> = tri
                    .iter()
                    .map(|&v| geom::wide(welder.points[v as usize]))
                    .collect();
                if tidy_ring(&tri).is_some() && geom::area(&tp) > geom::AREA_EPS {
                    pieces.push((tri, i));
                }
            }
            continue;
        }
        if ring.len() > MAX_RING {
            report.split.push(i);
        }
        for piece in fan_split(&ring) {
            pieces.push((piece, i));
        }
    }
    report.welded = welder.merged;

    // Only vertices a surface uses, in first-use order.
    let mut remap = vec![u32::MAX; welder.points.len()];
    let mut points = Vec::new();
    for (ring, _) in &mut pieces {
        for v in ring.iter_mut() {
            let r = &mut remap[*v as usize];
            if *r == u32::MAX {
                *r = points.len() as u32;
                points.push(welder.points[*v as usize]);
            }
            *v = *r;
        }
    }

    let mut planes: Vec<[f32; 4]> = Vec::new();
    let mut buckets: HashMap<[i64; 4], Vec<usize>> = HashMap::new();
    let mut surfaces = Vec::with_capacity(pieces.len());
    for (ring, source) in pieces {
        let pts: Vec<V3> = ring.iter().map(|&v| geom::wide(points[v as usize])).collect();
        let nv = geom::newell(&pts);
        let n = nv.map(|c| c / geom::len(nv));
        let d = geom::dot(n, geom::centroid(&pts));
        let (plane, flip) = find_plane(&mut planes, &mut buckets, &pts, n, d);
        let poly = &polygons[source];
        surfaces.push(PrepSurface {
            ring,
            source,
            material: poly.material,
            flags: poly.flags,
            breakable_surface: poly.breakable_surface,
            plane,
            flip,
        });
    }
    Ok(Prepared {
        points,
        surfaces,
        planes,
        report,
    })
}

/// The plane `n . p = d`, found among the planes so far (either facing)
/// or added.
fn find_plane(
    planes: &mut Vec<[f32; 4]>,
    buckets: &mut HashMap<[i64; 4], Vec<usize>>,
    pts: &[V3],
    n: V3,
    d: f64,
) -> (usize, bool) {
    // Bucket by the normal and distance with the dominant component made
    // positive, so both facings of a plane meet. A plane near a bucket's
    // edge may be added twice; nothing depends on one index per plane.
    let k = (0..3)
        .max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
        .unwrap_or(2);
    let s = if n[k] < 0.0 { -1.0 } else { 1.0 };
    let key = [
        (n[0] * s * 1e3).round() as i64,
        (n[1] * s * 1e3).round() as i64,
        (n[2] * s * 1e3).round() as i64,
        (d * s * 1e2).round() as i64,
    ];
    let list = buckets.entry(key).or_default();
    for &j in list.iter() {
        let [x, y, z, pd] = planes[j];
        let m = geom::wide([x, y, z]);
        let c = geom::dot(n, m);
        let on = || {
            let (lo, hi) = geom::extent(pts, m, f64::from(pd));
            lo >= -PLANAR_EPS && hi <= PLANAR_EPS
        };
        if c >= SAME_NORMAL && (d - f64::from(pd)).abs() <= SAME_DISTANCE && on() {
            return (j, false);
        }
        if c <= -SAME_NORMAL && (d + f64::from(pd)).abs() <= SAME_DISTANCE && on() {
            return (j, true);
        }
    }
    let j = planes.len();
    planes.push([n[0] as f32, n[1] as f32, n[2] as f32, d as f32]);
    list.push(j);
    (j, false)
}
