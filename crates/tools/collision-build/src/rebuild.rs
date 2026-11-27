//! A level's collision rebuilt from its edited render surface
//! (docs/editor/modeling.md, "Sculpting"; docs/editor/modeling-targets.md,
//! "Structure collision", step 3), as plain data.
//!
//! An edit names, per edited section, its render triangles before and
//! after. [`Level::rebuild`]:
//!
//! 1. Keeps every collision surface that does not lie on the edited
//!    sections' render triangles (walls with no render face, other
//!    clusters, invisible walls) and every one that lies on them away from
//!    the change.
//! 2. Removes the surfaces on the render surface the edit changed: a
//!    surface whose centroid is on a changed triangle, or that holds a
//!    changed triangle's centroid.
//! 3. Adds the changed sections' new triangles, joined by
//!    [`crate::from_render`], with the collision material, flags and
//!    breakable byte of the surfaces their render material sat on before.
//!    A render material that had no collision under it gets none. Unchanged
//!    triangles under a removed surface are added again with its
//!    attributes, so a large surface the change only crossed stays whole.
//! 4. Compiles the result whole ([`crate::build_with`]), and keeps each
//!    plane a caller asks for (a portal's) resolvable: [`Rebuilt::planes`]
//!    gives its new index, the plane appended when no surface uses it.
//! 5. Gives each new leaf the per-leaf element of the old leaf its cell's
//!    centre is in, or, for space the edit opened, of a leaf of the edited
//!    section's cluster nearest it.
//! 6. Rebuilds the structure MOPP ([`crate::level::rebuild_mopp`]).

use std::collections::{BTreeMap, HashSet};

use tags::collision::{Bsp, Plane};

use crate::from_render::{FromRender, from_render};
use crate::level::{LEAF_SIZE, Level, rebuild_mopp};
use crate::{Error, Options, Polygon, Result, build_with, query, verify};

type Vec3 = [f32; 3];

/// How far a point may be from a triangle and still lie on it.
pub const ON: f32 = 1e-2;

/// A render triangle and its render material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tri {
    pub points: [Vec3; 3],
    pub material: u16,
}

/// One edited render section of the level: its cluster and its triangles
/// before and after the edit.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionEdit {
    pub cluster: i16,
    pub before: Vec<Tri>,
    pub after: Vec<Tri>,
}

impl SectionEdit {
    /// The triangles the edit took away and the ones it made: a triangle
    /// with the same three points before and after is unchanged.
    pub fn changes(&self) -> (Vec<Tri>, Vec<Tri>) {
        let was: HashSet<_> = self.before.iter().map(key).collect();
        let now: HashSet<_> = self.after.iter().map(key).collect();
        (
            self.before.iter().filter(|t| !now.contains(&key(t))).copied().collect(),
            self.after.iter().filter(|t| !was.contains(&key(t))).copied().collect(),
        )
    }
}

/// A level rebuilt.
#[derive(Clone, Debug, PartialEq)]
pub struct Rebuilt {
    pub level: Level,
    /// Per plane asked to be kept, its index in the rebuilt BSP.
    pub planes: Vec<u32>,
    /// Collision surfaces taken out, and polygons put in.
    pub removed: usize,
    pub added: usize,
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn mix(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn mean(points: &[Vec3]) -> Vec3 {
    let n = points.len().max(1) as f32;
    let s = points.iter().fold([0.0; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
    [s[0] / n, s[1] / n, s[2] / n]
}

/// Squared distance from `p` to the triangle `abc` (Ericson, "Real-Time
/// Collision Detection", 5.1.5).
pub fn triangle_distance2(p: Vec3, [a, b, c]: [Vec3; 3]) -> f32 {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let at = |q: Vec3| dot(sub(p, q), sub(p, q));
    if d1 <= 0.0 && d2 <= 0.0 {
        return at(a);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return at(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return at(mix(a, b, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return at(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return at(mix(a, c, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return at(mix(b, c, (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    at([
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ])
}

/// Whether `p` lies on the triangle.
pub fn on_triangle(p: Vec3, t: [Vec3; 3]) -> bool {
    triangle_distance2(p, t) < ON * ON
}

/// Whether `p` lies on the convex polygon `ring`.
fn on_polygon(p: Vec3, ring: &[Vec3]) -> bool {
    (1..ring.len().saturating_sub(1)).any(|k| on_triangle(p, [ring[0], ring[k], ring[k + 1]]))
}

fn key(t: &Tri) -> [[u32; 3]; 3] {
    let mut k = t.points.map(|p| p.map(f32::to_bits));
    k.sort_unstable();
    k
}

/// A box, grown by [`ON`].
#[derive(Clone, Copy)]
struct Bounds {
    lo: Vec3,
    hi: Vec3,
}

impl Bounds {
    fn of(points: &[Vec3]) -> Bounds {
        let mut b = Bounds { lo: [f32::INFINITY; 3], hi: [f32::NEG_INFINITY; 3] };
        for p in points {
            for k in 0..3 {
                b.lo[k] = b.lo[k].min(p[k] - ON);
                b.hi[k] = b.hi[k].max(p[k] + ON);
            }
        }
        b
    }
    fn holds(&self, p: Vec3) -> bool {
        (0..3).all(|k| p[k] >= self.lo[k] && p[k] <= self.hi[k])
    }
    fn meets(&self, o: &Bounds) -> bool {
        (0..3).all(|k| self.lo[k] <= o.hi[k] && o.lo[k] <= self.hi[k])
    }
}

/// Collision surface attributes: material, flags, breakable byte.
type Attrs = (u16, u8, u8);

impl Level {
    /// Rebuilds this level after `sections` changed (see the module docs),
    /// keeping the planes `keep_planes` (old plane indices) resolvable.
    /// Refused, with nothing changed, when a plane asked for is not in the
    /// level, a limit is passed, a leaf of space the edit opened has no
    /// leaf of its cluster to copy, or the result fails `Bsp::validate`.
    pub fn rebuild(&self, sections: &[SectionEdit], keep_planes: &[usize], opts: &Options) -> Result<Rebuilt> {
        self.check()?;
        let bsp = &self.bsp;
        let rings: Vec<Vec<Vec3>> = (0..bsp.surfaces.len())
            .map(|s| bsp.surface_ring(s).iter().map(|&v| bsp.vertices[v].point).collect())
            .collect();
        let bounds: Vec<Bounds> = rings.iter().map(|r| Bounds::of(r)).collect();
        let before: Vec<Tri> = sections.iter().flat_map(|s| s.before.iter().copied()).collect();
        let tri_bounds: Vec<Bounds> = before.iter().map(|t| Bounds::of(&t.points)).collect();
        let mut changed_before = Vec::new();
        let mut changed_after: Vec<(Tri, usize)> = Vec::new();
        let mut unchanged_after = Vec::new();
        for (k, s) in sections.iter().enumerate() {
            let was: HashSet<_> = s.before.iter().map(key).collect();
            let now: HashSet<_> = s.after.iter().map(key).collect();
            changed_before.extend(s.before.iter().filter(|t| !now.contains(&key(t))).copied());
            for t in &s.after {
                if was.contains(&key(t)) {
                    unchanged_after.push(*t);
                } else {
                    changed_after.push((*t, k));
                }
            }
        }
        let planes_now = |bsp: &Bsp| -> Result<Vec<u32>> {
            keep_planes
                .iter()
                .map(|&p| {
                    (p < bsp.planes.len())
                        .then_some(p as u32)
                        .ok_or_else(|| Error::Invalid(format!("plane {p} is not in the level")))
                })
                .collect()
        };
        if changed_before.is_empty() && changed_after.is_empty() {
            return Ok(Rebuilt { level: self.clone(), planes: planes_now(bsp)?, removed: 0, added: 0 });
        }

        // 1 and 2: which surfaces lie on the render surface, and which of
        // those the edit changed.
        let on_render: Vec<bool> = rings
            .iter()
            .enumerate()
            .map(|(s, ring)| {
                ring.iter().all(|&p| {
                    before
                        .iter()
                        .zip(&tri_bounds)
                        .any(|(t, b)| b.meets(&bounds[s]) && b.holds(p) && on_triangle(p, t.points))
                })
            })
            .collect();
        let removed: Vec<bool> = rings
            .iter()
            .enumerate()
            .map(|(s, ring)| {
                on_render[s] && {
                    let c = mean(ring);
                    changed_before
                        .iter()
                        .any(|t| on_triangle(c, t.points) || on_polygon(mean(&t.points), ring))
                }
            })
            .collect();

        // 3: the attributes each render material had, by vote.
        let mut votes: BTreeMap<u16, BTreeMap<Attrs, usize>> = BTreeMap::new();
        for t in &before {
            let c = mean(&t.points);
            if let Some(s) = (0..rings.len()).find(|&s| on_render[s] && bounds[s].holds(c) && on_polygon(c, &rings[s])) {
                let x = &bsp.surfaces[s];
                *votes.entry(t.material).or_default().entry((x.material, x.flags, x.breakable_surface)).or_default() += 1;
            }
        }
        let attrs_of: BTreeMap<u16, Attrs> = votes
            .into_iter()
            .filter_map(|(m, v)| v.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(a, _)| (m, a)))
            .collect();
        let mut fresh: BTreeMap<(u8, u8), Vec<(Tri, u16)>> = BTreeMap::new();
        for (t, _) in &changed_after {
            if let Some(&(m, f, b)) = attrs_of.get(&t.material) {
                fresh.entry((f, b)).or_default().push((*t, m));
            }
        }
        for t in &unchanged_after {
            let c = mean(&t.points);
            if let Some(s) = (0..rings.len()).find(|&s| removed[s] && bounds[s].holds(c) && on_polygon(c, &rings[s])) {
                let x = &bsp.surfaces[s];
                fresh.entry((x.flags, x.breakable_surface)).or_default().push((*t, x.material));
            }
        }
        let mut polygons: Vec<Polygon> = Vec::new();
        for (s, ring) in rings.iter().enumerate() {
            if !removed[s] {
                let x = &bsp.surfaces[s];
                polygons.push(Polygon {
                    points: ring.clone(),
                    material: x.material,
                    flags: x.flags,
                    breakable_surface: x.breakable_surface,
                });
            }
        }
        let kept = polygons.len();
        for ((flags, breakable), tris) in &fresh {
            let positions: Vec<Vec3> = tris.iter().flat_map(|(t, _)| t.points).collect();
            let triangles: Vec<[u32; 3]> = (0..tris.len() as u32).map(|k| [3 * k, 3 * k + 1, 3 * k + 2]).collect();
            let materials: Vec<u16> = tris.iter().map(|&(_, m)| m).collect();
            for mut p in from_render(&positions, &triangles, &materials, &FromRender::default()) {
                p.flags |= flags;
                p.breakable_surface = *breakable;
                polygons.push(p);
            }
        }
        if let Some((surface, p)) =
            polygons.iter().enumerate().find(|(_, p)| usize::from(p.material) >= self.materials)
        {
            return Err(Error::Material { surface, material: p.material, materials: self.materials });
        }

        // 4: compile, and keep the planes asked for.
        let mut new = build_with(&polygons, opts)?.bsp;
        let mut planes = Vec::with_capacity(keep_planes.len());
        for &p in keep_planes {
            let old = bsp.planes.get(p).ok_or_else(|| Error::Invalid(format!("plane {p} is not in the level")))?.0;
            let same = |q: &Plane| {
                let n = [q.0[0], q.0[1], q.0[2]];
                dot(n, [old[0], old[1], old[2]]) > 1.0 - 1e-5 && (q.0[3] - old[3]).abs() < 1e-3
            };
            let at = match new.planes.iter().position(same) {
                Some(j) => j,
                None => {
                    new.planes.push(Plane(old));
                    new.planes.len() - 1
                }
            };
            planes.push(at as u32);
        }
        opts.limits.check("plane", new.planes.len())?;
        if let Some(problem) = new.validate(self.materials).into_iter().next() {
            return Err(Error::Invalid(format!("the rebuilt level does not validate: {problem}")));
        }

        // 5: the per-leaf block.
        let all: Vec<Vec3> = new.vertices.iter().map(|v| v.point).chain(bsp.vertices.iter().map(|v| v.point)).collect();
        let world = Bounds::of(&all);
        let paths = verify::leaf_paths(&new);
        let mut leaves = Vec::with_capacity(new.leaves.len() * LEAF_SIZE);
        for l in 0..new.leaves.len() {
            let path = paths.get(l).and_then(|p| p.first()).cloned().unwrap_or_default();
            let candidates = cell_points(&new, &path, world);
            let old = candidates.iter().find_map(|&p| query::locate(bsp, p));
            let element = match old {
                Some(o) => self.element(o)?,
                None => {
                    let at = candidates.first().copied().unwrap_or_else(|| mean(&[world.lo, world.hi]));
                    let cluster = nearest_cluster(sections, &changed_after, at)
                        .ok_or_else(|| Error::Invalid(format!("leaf {l} is in no old leaf and no edited section")))?;
                    self.cluster_element(cluster).ok_or_else(|| {
                        Error::Invalid(format!("leaf {l} opened in cluster {cluster}, which has no leaf to copy"))
                    })?
                }
            };
            leaves.extend_from_slice(&element);
        }

        // 6: the MOPP.
        let mopp = rebuild_mopp(&new, &self.mopp)?;
        Ok(Rebuilt {
            level: Level { bsp: new, leaves, mopp, materials: self.materials },
            planes,
            removed: removed.iter().filter(|&&r| r).count(),
            added: polygons.len() - kept,
        })
    }

    fn element(&self, leaf: usize) -> Result<[u8; LEAF_SIZE]> {
        self.leaves
            .get(leaf * LEAF_SIZE..(leaf + 1) * LEAF_SIZE)
            .and_then(|e| e.try_into().ok())
            .ok_or_else(|| Error::Invalid(format!("no per-leaf element for leaf {leaf}")))
    }

    /// The element of the first old leaf in `cluster` (the `i16` at `+0`).
    fn cluster_element(&self, cluster: i16) -> Option<[u8; LEAF_SIZE]> {
        self.leaves
            .chunks_exact(LEAF_SIZE)
            .find(|e| i16::from_le_bytes([e[0], e[1]]) == cluster)
            .and_then(|e| e.try_into().ok())
    }
}

/// The cluster of the edited section whose changed triangles come
/// nearest `p`.
fn nearest_cluster(sections: &[SectionEdit], changed: &[(Tri, usize)], p: Vec3) -> Option<i16> {
    changed
        .iter()
        .map(|(t, k)| (triangle_distance2(p, t.points), *k))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, k)| sections[k].cluster)
        .or_else(|| sections.first().map(|s| s.cluster))
}

/// Points inside a leaf's cell: the cell (the world box cut by each plane
/// on the leaf's path, on the path's side) has its centre first, then each
/// corner pulled a quarter of the way in.
fn cell_points(bsp: &Bsp, path: &[(usize, bool)], world: Bounds) -> Vec<Vec3> {
    let (lo, hi) = (world.lo.map(|v| v - 1.0), world.hi.map(|v| v + 1.0));
    let c = |i: usize| [if i & 1 != 0 { hi[0] } else { lo[0] }, if i & 2 != 0 { hi[1] } else { lo[1] }, if i & 4 != 0 { hi[2] } else { lo[2] }];
    let mut faces: Vec<Vec<Vec3>> = [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]]
        .iter()
        .map(|f| f.iter().map(|&i| c(i)).collect())
        .collect();
    for &(node, front) in path {
        let Some(n) = bsp.nodes.get(node) else { continue };
        let Some(Plane(p)) = bsp.planes.get(usize::from(n.plane)).copied() else { continue };
        let sign = if front { 1.0 } else { -1.0 };
        let normal = [p[0], p[1], p[2]];
        let side = |q: Vec3| sign * (dot(normal, q) - p[3]);
        let mut cap = Vec::new();
        let mut next = Vec::new();
        for f in &faces {
            let mut out = Vec::new();
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                let (sa, sb) = (side(a), side(b));
                if sa >= 0.0 {
                    out.push(a);
                }
                if (sa >= 0.0) != (sb >= 0.0) {
                    let x = mix(a, b, sa / (sa - sb));
                    out.push(x);
                    cap.push(x);
                }
            }
            if out.len() >= 3 {
                next.push(out);
            }
        }
        if cap.len() >= 3 {
            let centre = mean(&cap);
            let u = sub(cap[0], centre);
            let v = cross(normal, u);
            cap.sort_by(|a, b| {
                let (da, db) = (sub(*a, centre), sub(*b, centre));
                dot(da, v).atan2(dot(da, u)).total_cmp(&dot(db, v).atan2(dot(db, u)))
            });
            next.push(cap);
        }
        faces = next;
        if faces.is_empty() {
            return Vec::new();
        }
    }
    let corners: Vec<Vec3> = faces.into_iter().flatten().collect();
    let centre = mean(&corners);
    let mut out = vec![centre];
    out.extend(corners.iter().take(16).map(|&p| mix(p, centre, 0.25)));
    out
}
