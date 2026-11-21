//! The geometric heuristics behind [`crate::scan`]
//! (docs/game/superbounce.md, "Spots in the geometry"). Every threshold is
//! named here with where it comes from.
use crate::geom::{
    Bounds, Grid, Tagged, V2, area2, centroid2, clip, dist2_to_poly, dot, dot2, inside2, left_of,
    len2, plane_z, sub, sub2, xy_ccw,
};
use crate::{CROUCHED, Kind, Poly, RADIUS, STANDING, Spot, Vec3};

/// Grid cell (world units) for bucketing polygons on the ground plane.
const CELL: f32 = 2.0;

// --- Bounce ---------------------------------------------------------------

/// A wall: a face with `|nz|` under this. The Ascension pair is at 0 and
/// 0.0372; two faces that oppose to [`OPPOSE`] can't lean more than 9°.
const WALL_NZ: f32 = 0.3;
/// The two faces' normals oppose: their dot is below this.
const OPPOSE: f32 = -0.95;
/// Faces closer than this are one face split in two, not a slot.
const MIN_GAP: f32 = 0.02;
/// The faces overlap along the slot by at least this.
const MIN_OVERLAP: f32 = 0.05;
/// The slot is at least this deep.
const MIN_DEPTH: f32 = 0.02;
/// How far below the floor the slot's top may be: the deepest traced
/// touchdown penetration was 0.23 (docs/game/superbounce.md, "The
/// touchdown tick"); a little more for margin.
const MAX_SINK: f32 = 0.3;
/// Floor counts as at the slot's top within this much above it.
const FLOOR_ABOVE_TOP: f32 = 0.05;
/// Launch per unit of the two normals' summed z: calibrated so the
/// Ascension slot (normals `(0, -1, 0)` and `(0, 0.9993, 0.0372)`) gives the
/// 17.85 units/s the body gained on the touchdown tick. The solver's two
/// opposing impulses are about equal (77,576 and 77,566), so what is left
/// is their upward part, proportional to the summed normal z.
pub(crate) const LAUNCH_PER_NZ: f32 = 17.85 / 0.0372;
/// A summed tilt that launches slower than this is a landing, not a bounce.
const MIN_LAUNCH: f32 = 3.0;
/// Launch estimates past this are capped and scored down: the linear
/// calibration is from one slot with a 2.1° tilt.
const MAX_LAUNCH: f32 = 60.0;
/// The shallowest traced bounce sank 0.070 below the floor into a slot
/// whose walls the capsule first touches at 0.021: about 0.05 of pinch on
/// top of first touch (docs/game/superbounce.md, "Traces").
const PINCH_DEPTH: f32 = 0.05;
/// Ticks a second the bounce was measured at (the lab runs at 30).
const TICKS: f32 = 30.0;
/// Vertical speed a standing jump lands at on the floor it left (units/s):
/// "a standing jump at the patch lands at −3".
const JUMP_LAND: f32 = 3.0;
/// Gravity (units/s²), fitted from the recipe: a standing jump lands at 3,
/// the jump off the upper level (z −0.76) onto the patch (z −3.19, a 2.43
/// drop) lands at 5.4: (5.4² − 3²) / (2 × 2.43) ≈ 4.15.
const GRAVITY: f32 = 4.15;
/// How far behind the tilted wall the stand point is (the measured spot
/// stands at y 3.88, 0.06 short of the tilted wall at y 3.94).
const STAND_BACK: f32 = 0.06;
/// A unit's origin standing sits this far above the floor (−3.19 on the
/// −3.2 floor).
const ORIGIN_LIFT: f32 = 0.01;

// --- Crouch ---------------------------------------------------------------

/// Walkable floor: normal z over this.
const WALKABLE: f32 = 0.7;
/// A ceiling: any face looking down more steeply than this.
const CEILING_NZ: f32 = -0.3;
/// Pieces of low ceiling smaller than this are ignored.
const MIN_AREA: f32 = 0.01;
/// How far out past a low ceiling's edge the scan looks for standing room.
const STEP_OUT: f32 = 0.1;
/// Floor across a ceiling's edge counts as the same floor within this.
const STEP_Z: f32 = 0.15;

/// Spots of each kind at most, best first.
const MAX_SPOTS: usize = 512;

struct Face<'a> {
    poly: &'a Poly,
    b: Bounds,
}

pub(crate) fn scan(polys: &[Poly]) -> Vec<Spot> {
    let faces: Vec<Face> = polys
        .iter()
        .filter(|p| p.verts.len() >= 3)
        .map(|poly| Face {
            poly,
            b: Bounds::of(&poly.verts),
        })
        .collect();
    let mut walls = Vec::new();
    let mut floors = Vec::new();
    let mut ceilings = Vec::new();
    for (i, f) in faces.iter().enumerate() {
        let nz = f.poly.normal[2];
        if nz.abs() < WALL_NZ {
            walls.push(i);
        }
        if nz > WALKABLE {
            floors.push(i);
        }
        if nz < CEILING_NZ {
            ceilings.push(i);
        }
    }
    let grid = |set: &[usize]| {
        let mut g = Grid::new(CELL);
        for &i in set {
            let b = faces[i].b;
            g.insert(i as u32, [b.min[0], b.min[1]], [b.max[0], b.max[1]]);
        }
        g
    };
    let world = World {
        faces: &faces,
        wall_grid: grid(&walls),
        floor_grid: grid(&floors),
        ceiling_grid: grid(&ceilings),
    };
    let mut out = world.bounces(&walls);
    out.extend(world.crouches(&floors));
    out
}

struct World<'a> {
    faces: &'a [Face<'a>],
    wall_grid: Grid,
    floor_grid: Grid,
    ceiling_grid: Grid,
}

fn h(n: Vec3) -> Option<V2> {
    let l = len2([n[0], n[1]]);
    (l > 0.5).then(|| [n[0] / l, n[1] / l])
}

impl World<'_> {
    // ----- Bounce ---------------------------------------------------------

    fn bounces(&self, walls: &[usize]) -> Vec<Spot> {
        let mut found = Vec::new();
        let mut near = Vec::new();
        let d = 2.0 * RADIUS;
        for &a in walls {
            let fa = &self.faces[a];
            let b = fa.b;
            self.wall_grid.query(
                [b.min[0] - d, b.min[1] - d],
                [b.max[0] + d, b.max[1] + d],
                &mut near,
            );
            for &j in &near {
                let j = j as usize;
                if j > a
                    && let Some(spot) = self.slot(fa, &self.faces[j])
                {
                    found.push(spot);
                }
            }
        }
        best(merge_bounces(found))
    }

    /// The bounce spot between walls `a` and `b`, if they make a slot.
    fn slot(&self, a: &Face, b: &Face) -> Option<Spot> {
        let (na, nb) = (a.poly.normal, b.poly.normal);
        let along = dot(na, nb);
        if along >= OPPOSE {
            return None;
        }
        let (ha, hb) = (h(na)?, h(nb)?);
        // Signed horizontal distance of each face's centroid in front of
        // the other's plane: both positive means they face each other.
        let ahead = |from: &Face, p: Vec3| {
            let (n, o) = (from.poly.normal, from.poly.verts[0]);
            dot(n, sub(p, o)) / len2([n[0], n[1]])
        };
        let gap = ahead(a, b.b.centroid);
        let back = ahead(b, a.b.centroid);
        if !(gap > MIN_GAP && back > MIN_GAP && gap < 2.0 * RADIUS) {
            return None;
        }
        // Overlap along the slot, on a's tangent.
        let t = [-ha[1], ha[0]];
        let span = |f: &Face| {
            f.poly
                .verts
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                    let s = dot2(t, [v[0], v[1]]);
                    (lo.min(s), hi.max(s))
                })
        };
        let ((a0, a1), (b0, b1)) = (span(a), span(b));
        let (lo, hi) = (a0.max(b0), a1.min(b1));
        if hi - lo < MIN_OVERLAP {
            return None;
        }
        let top = a.b.max[2].min(b.b.max[2]);
        let bottom = a.b.min[2].max(b.b.min[2]);
        if top - bottom < MIN_DEPTH {
            return None;
        }
        // The slot's centre line on the ground plane.
        let s = dot2(ha, [a.b.centroid[0], a.b.centroid[1]]) + gap / 2.0;
        let at = |u: f32| [ha[0] * s + t[0] * u, ha[1] * s + t[1] * u];
        let mid = at((lo + hi) / 2.0);
        let floor_z = self.floor_at_slot(mid, gap / 2.0 + RADIUS, top)?;

        let launch_raw = LAUNCH_PER_NZ * (na[2] + nb[2]);
        if launch_raw < MIN_LAUNCH {
            return None;
        }
        let launch = launch_raw.min(MAX_LAUNCH);
        // Drift along the tilted wall's normal: into the slot from its side.
        let drift = if na[2] >= nb[2] { ha } else { hb };

        // fall: the touchdown tick moves the capsule v / 30 down; started
        // anywhere in the tick, half the landings sink at least v / 60. The
        // capsule (radius r) first touches both walls' top edges when its
        // bottom is r - sqrt(r² - (gap/2)²) below the slot top; a bounce
        // needs PINCH_DEPTH more, plus however far the slot's top is below
        // the floor.
        let half = (gap / 2.0).min(RADIUS);
        let first_touch = RADIUS - (RADIUS * RADIUS - half * half).sqrt();
        let sink = (floor_z - top).max(0.0) + first_touch + PINCH_DEPTH;
        let land = 2.0 * TICKS * sink;
        let fall = ((land * land - JUMP_LAND * JUMP_LAND) / (2.0 * GRAVITY)).max(0.0);

        let back_by = gap / 2.0 + STAND_BACK;
        let stand_xy = [mid[0] - drift[0] * back_by, mid[1] - drift[1] * back_by];
        let stand_z = self.floor_near(stand_xy, floor_z).unwrap_or(floor_z) + ORIGIN_LIFT;

        let overlap_fit = ((hi - lo) / (2.0 * RADIUS)).min(1.0);
        let oppose_fit = ((-along + OPPOSE) / (1.0 + OPPOSE)).clamp(0.0, 1.0);
        let mut score = 0.4 + 0.3 * overlap_fit + 0.3 * oppose_fit;
        if launch_raw > MAX_LAUNCH {
            score *= 0.5;
        }
        let [e0, e1] = [at(lo), at(hi)];
        Some(Spot {
            kind: Kind::Bounce,
            edge: [[e0[0], e0[1], floor_z], [e1[0], e1[1], floor_z]],
            stand: [stand_xy[0], stand_xy[1], stand_z],
            fall,
            launch,
            drift,
            score,
        })
    }

    /// Walkable floor reaching within `reach` of `p` whose height there is
    /// between the slot's top and MAX_SINK above it: the highest one.
    fn floor_at_slot(&self, p: V2, reach: f32, top: f32) -> Option<f32> {
        let mut near = Vec::new();
        self.floor_grid.query(
            [p[0] - reach, p[1] - reach],
            [p[0] + reach, p[1] + reach],
            &mut near,
        );
        near.iter()
            .filter_map(|&i| {
                let f = &self.faces[i as usize];
                let z = plane_z(f.poly, p)?;
                let ok = z >= top - FLOOR_ABOVE_TOP
                    && z <= top + MAX_SINK
                    && dist2_to_poly(&xy_ccw(&f.poly.verts), p) <= reach;
                ok.then_some(z)
            })
            .reduce(f32::max)
    }

    /// The walkable floor under `p` nearest in height to `z` (within
    /// [`STEP_Z`]).
    fn floor_near(&self, p: V2, z: f32) -> Option<f32> {
        let mut near = Vec::new();
        self.floor_grid.query(p, p, &mut near);
        near.iter()
            .filter_map(|&i| {
                let f = &self.faces[i as usize];
                let fz = plane_z(f.poly, p)?;
                ((fz - z).abs() <= STEP_Z && inside2(&xy_ccw(&f.poly.verts), p, 1e-3)).then_some(fz)
            })
            .min_by(|a, b| (a - z).abs().total_cmp(&(b - z).abs()))
    }

    // ----- Crouch ---------------------------------------------------------

    fn crouches(&self, floors: &[usize]) -> Vec<Spot> {
        let mut regions = Vec::new();
        let mut near = Vec::new();
        for &fi in floors {
            let f = &self.faces[fi];
            self.ceiling_grid.query(
                [f.b.min[0], f.b.min[1]],
                [f.b.max[0], f.b.max[1]],
                &mut near,
            );
            for &ci in &near {
                let c = &self.faces[ci as usize];
                if c.b.max[2] < f.b.min[2] + CROUCHED - 1e-3 || c.b.min[2] > f.b.max[2] + STANDING {
                    continue;
                }
                if let Some(r) = self.low_ceiling(f, c) {
                    regions.push(r);
                }
            }
        }
        best(cluster_crouches(regions))
    }

    /// The part of floor `f` under ceiling `c` with crouched but not
    /// standing room, as a counter-clockwise polygon on the ground plane.
    fn low_ceiling(&self, f: &Face, c: &Face) -> Option<Region> {
        let (fp, cp) = (f.poly, c.poly);
        let start: Tagged<Side> = xy_ccw(&fp.verts)
            .into_iter()
            .map(|p| (p, Side::Floor))
            .collect();
        let ceiling = xy_ccw(&cp.verts);
        let mut r = start;
        for k in 0..ceiling.len() {
            let (a, b) = (ceiling[k], ceiling[(k + 1) % ceiling.len()]);
            if len2(sub2(b, a)) < 1e-6 {
                continue;
            }
            r = clip(&r, left_of(a, b, 0.0), Side::Ceiling);
            if r.len() < 3 {
                return None;
            }
        }
        // Clearance is linear over the ground plane (two planes), so the
        // band [CROUCHED, STANDING) is two more half-planes.
        let clearance = |p: V2| -> f32 {
            plane_z(cp, p).unwrap_or(f32::NAN) - plane_z(fp, p).unwrap_or(f32::NAN)
        };
        r = clip(&r, |p| clearance(p) - CROUCHED, Side::Low);
        r = clip(&r, |p| STANDING - 1e-4 - clearance(p), Side::High);
        if r.len() < 3 {
            return None;
        }
        let poly: Vec<V2> = r.iter().map(|v| v.0).collect();
        let area = area2(&poly);
        if area.is_nan() || area < MIN_AREA {
            return None;
        }
        let centre = centroid2(&poly);
        let floor_z = plane_z(fp, centre)?;
        let ceiling_z = plane_z(cp, centre)?;
        // Nothing else between this floor and this ceiling at the centre:
        // a lower ceiling, or a higher floor, means these two don't face
        // each other there.
        if self.between(centre, floor_z, ceiling_z) {
            return None;
        }
        // The capsule's footprint fits when the region shrunk by the
        // radius is not empty (the region is convex).
        let mut inner: Tagged<()> = poly.iter().map(|&p| (p, ())).collect();
        for k in 0..poly.len() {
            let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
            if len2(sub2(b, a)) < 1e-6 {
                continue;
            }
            inner = clip(&inner, left_of(a, b, RADIUS), ());
            if inner.len() < 3 {
                break;
            }
        }
        let inner: Vec<V2> = inner.into_iter().map(|v| v.0).collect();
        let fits = (inner.len() >= 3 && area2(&inner) > 1e-6).then(|| centroid2(&inner));
        let exits = (0..r.len())
            .filter(|&k| matches!(r[k].1, Side::Ceiling | Side::High))
            .filter_map(|k| {
                let (a, b) = (r[k].0, r[(k + 1) % r.len()].0);
                let za = plane_z(fp, a)?;
                let zb = plane_z(fp, b)?;
                let l = len2(sub2(b, a));
                (l > 1e-3 && self.can_stand_beyond(a, b, (za + zb) / 2.0))
                    .then_some(([[a[0], a[1], za], [b[0], b[1], zb]], l))
            })
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .map(|x| x.0);
        let longest = (0..r.len())
            .map(|k| {
                let (a, b) = (r[k].0, r[(k + 1) % r.len()].0);
                (a, b, len2(sub2(b, a)))
            })
            .max_by(|x, y| x.2.total_cmp(&y.2))
            .map(|(a, b, _)| {
                let z = |p: V2| plane_z(fp, p).unwrap_or(floor_z);
                [[a[0], a[1], z(a)], [b[0], b[1], z(b)]]
            })?;
        let (min, max) = poly.iter().fold(
            ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
            |(lo, hi), p| {
                (
                    [lo[0].min(p[0]), lo[1].min(p[1])],
                    [hi[0].max(p[0]), hi[1].max(p[1])],
                )
            },
        );
        let stand_xy = fits.unwrap_or(centre);
        Some(Region {
            min,
            max,
            area,
            floor_z,
            stand: [
                stand_xy[0],
                stand_xy[1],
                plane_z(fp, stand_xy).unwrap_or(floor_z) + ORIGIN_LIFT,
            ],
            fits: fits.is_some(),
            exit: exits,
            longest,
        })
    }

    /// Another floor or ceiling strictly between `lo` and `hi` over `p`.
    fn between(&self, p: V2, lo: f32, hi: f32) -> bool {
        let mut near = Vec::new();
        [&self.ceiling_grid, &self.floor_grid].iter().any(|g| {
            g.query(p, p, &mut near);
            near.iter().any(|&i| {
                let f = &self.faces[i as usize];
                plane_z(f.poly, p).is_some_and(|z| z > lo + 0.02 && z < hi - 0.02)
                    && inside2(&xy_ccw(&f.poly.verts), p, -1e-3)
            })
        })
    }

    /// Standing room just outside the edge `a -> b` of a counter-clockwise
    /// region: floor at about `z`, and nothing within standing height over
    /// it.
    fn can_stand_beyond(&self, a: V2, b: V2, z: f32) -> bool {
        let e = sub2(b, a);
        let l = len2(e);
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        // Outward of a counter-clockwise polygon is the right of the edge.
        let out = [mid[0] + e[1] / l * STEP_OUT, mid[1] - e[0] / l * STEP_OUT];
        let Some(fz) = self.floor_near(out, z) else {
            return false;
        };
        let mut near = Vec::new();
        self.ceiling_grid.query(out, out, &mut near);
        !near.iter().any(|&i| {
            let c = &self.faces[i as usize];
            plane_z(c.poly, out).is_some_and(|cz| cz > fz + 0.02 && cz < fz + STANDING)
                && inside2(&xy_ccw(&c.poly.verts), out, 1e-3)
        })
    }
}

/// Which clip made a region edge: the floor's own edge, the ceiling's
/// edge, or where the clearance reaches crouch (`Low`) or standing
/// (`High`) height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Floor,
    Ceiling,
    Low,
    High,
}

/// One floor/ceiling pair's low-ceiling piece.
struct Region {
    min: V2,
    max: V2,
    area: f32,
    floor_z: f32,
    stand: Vec3,
    fits: bool,
    /// Its longest edge with standing room beyond it.
    exit: Option<[Vec3; 2]>,
    longest: [Vec3; 2],
}

/// Pieces that touch (bounds within 2 cm, floors within [`STEP_Z`]) are
/// one place: a crouch spot each, when the capsule fits in one piece or
/// the place is at least a capsule wide and a footprint in area.
fn cluster_crouches(regions: Vec<Region>) -> Vec<Spot> {
    let n = regions.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut g = Grid::new(CELL);
    for (i, r) in regions.iter().enumerate() {
        g.insert(i as u32, r.min, r.max);
    }
    let mut near = Vec::new();
    const TOUCH: f32 = 0.02;
    for (i, r) in regions.iter().enumerate() {
        g.query(
            [r.min[0] - TOUCH, r.min[1] - TOUCH],
            [r.max[0] + TOUCH, r.max[1] + TOUCH],
            &mut near,
        );
        for &j in &near {
            let (j, o) = (j as usize, &regions[j as usize]);
            let touch = o.min[0] <= r.max[0] + TOUCH
                && o.max[0] >= r.min[0] - TOUCH
                && o.min[1] <= r.max[1] + TOUCH
                && o.max[1] >= r.min[1] - TOUCH
                && (o.floor_z - r.floor_z).abs() <= STEP_Z;
            if j > i && touch {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in 0..n {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let footprint = std::f32::consts::PI * RADIUS * RADIUS;
    groups
        .into_values()
        .filter_map(|members| {
            let rs: Vec<&Region> = members.iter().map(|&i| &regions[i]).collect();
            let one_fits = rs.iter().any(|r| r.fits);
            let (min, max) = rs.iter().fold(
                ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
                |(lo, hi), r| {
                    (
                        [lo[0].min(r.min[0]), lo[1].min(r.min[1])],
                        [hi[0].max(r.max[0]), hi[1].max(r.max[1])],
                    )
                },
            );
            let area: f32 = rs.iter().map(|r| r.area).sum();
            let wide = (max[0] - min[0]).min(max[1] - min[1]) >= 2.0 * RADIUS;
            if !one_fits && !(wide && area >= footprint) {
                return None;
            }
            let edge_len = |e: &[Vec3; 2]| len2([e[1][0] - e[0][0], e[1][1] - e[0][1]]);
            let exit = rs
                .iter()
                .filter_map(|r| r.exit)
                .max_by(|a, b| edge_len(a).total_cmp(&edge_len(b)));
            let host = rs
                .iter()
                .max_by(|a, b| {
                    (a.fits, a.area)
                        .partial_cmp(&(b.fits, b.area))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .copied()?;
            let exit_bonus = if exit.is_some() { 0.3 } else { 0.0 };
            let fit_bonus = if one_fits { 0.3 } else { 0.0 };
            let score = 0.4 + exit_bonus + fit_bonus;
            Some(Spot {
                kind: Kind::Crouch,
                edge: exit.unwrap_or(host.longest),
                stand: host.stand,
                fall: 0.0,
                launch: 0.0,
                drift: [0.0, 0.0],
                score,
            })
        })
        .collect()
}

/// Bounce spots on one line: the same drift (to 5°), the same crease
/// position across the slot (to 5 cm) and height (to 10 cm), and
/// touching or overlapping along it (within 10 cm) are one spot.
fn merge_bounces(spots: Vec<Spot>) -> Vec<Spot> {
    const ANGLE: f32 = 5.0 * std::f32::consts::PI / 180.0;
    const ACROSS: f32 = 0.05;
    const HEIGHT: f32 = 0.1;
    const JOIN: f32 = 0.1;
    // Along the crease: the drift turned a quarter.
    let along = |s: &Spot, p: Vec3| dot2([-s.drift[1], s.drift[0]], [p[0], p[1]]);
    let mut keyed: Vec<_> = spots
        .into_iter()
        .map(|mut s| {
            // Orient each edge low to high along the crease.
            if along(&s, s.edge[0]) > along(&s, s.edge[1]) {
                s.edge.swap(0, 1);
            }
            let (d, m) = (s.drift, mid(s.edge));
            let key = (
                (d[1].atan2(d[0]) / ANGLE).round() as i64,
                (dot2(d, [m[0], m[1]]) / ACROSS).round() as i64,
                (m[2] / HEIGHT).round() as i64,
            );
            (key, along(&s, s.edge[0]), s)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut out: Vec<((i64, i64, i64), Spot)> = Vec::new();
    for (key, _, s) in keyed {
        if let Some((last_key, last)) = out.last_mut()
            && *last_key == key
            && along(&*last, s.edge[0]) <= along(&*last, last.edge[1]) + JOIN
        {
            let (l0, l1) = (along(&*last, last.edge[0]), along(&*last, last.edge[1]));
            let (s0, s1) = (along(&*last, s.edge[0]), along(&*last, s.edge[1]));
            let (wl, ws) = ((l1 - l0).max(1e-3), (s1 - s0).max(1e-3));
            let before = mid(last.edge);
            let offset = [
                last.stand[0] - before[0],
                last.stand[1] - before[1],
                last.stand[2] - before[2],
            ];
            if s1 > l1 {
                last.edge[1] = s.edge[1];
            }
            let m = mid(last.edge);
            last.stand = [m[0] + offset[0], m[1] + offset[1], m[2] + offset[2]];
            last.launch = (last.launch * wl + s.launch * ws) / (wl + ws);
            last.fall = last.fall.min(s.fall);
            last.score = last.score.max(s.score);
            continue;
        }
        out.push((key, s));
    }
    out.into_iter().map(|(_, s)| s).collect()
}

fn mid([a, b]: [Vec3; 2]) -> Vec3 {
    [
        (a[0] + b[0]) / 2.0,
        (a[1] + b[1]) / 2.0,
        (a[2] + b[2]) / 2.0,
    ]
}

/// At most [`MAX_SPOTS`], best score first.
fn best(mut spots: Vec<Spot>) -> Vec<Spot> {
    spots.sort_by(|a, b| b.score.total_cmp(&a.score));
    spots.truncate(MAX_SPOTS);
    spots
}
