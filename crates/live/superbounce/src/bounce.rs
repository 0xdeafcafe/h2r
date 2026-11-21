//! Bounce spots: a narrow strip of walkable floor between two parallel
//! floor joints, at least one of them a concave crease
//! (docs/game/superbounce.md, "Spots in the geometry", "Bounce").
//!
//! At Ascension the touchdown tick's two contacts (normals `(0, −1, 0)` at
//! y 4.105 and `(0, 0.9993, 0.0372)` at y 3.939) are Havok's contacts with
//! floor edges, not walls: the strip y 3.8..4.0 between the crease where
//! the ramp (normal z 0.8396) meets the floor and a flat seam with the
//! next floor piece. A capsule (radius 0.175) not caught by the floor
//! sinks onto both edges at once; the crease's tilt is what is left
//! upward.
use crate::geom::{Grid, V2, dot, dot2, len, len2, point_line, scale, sub, sub2, unit};
use crate::scan::{Face, ORIGIN_LIFT, WALKABLE, best};
use crate::{Kind, RADIUS, Spot, Vec3};

/// Two polygon edges are one joint when both ends of one are this close
/// to the other's line.
const JOIN_TOL: f32 = 0.01;
/// Edges and joint overlaps shorter than this are ignored.
const MIN_EDGE: f32 = 0.02;
/// Collinear edges: their directions' dot at least this.
const COLLINEAR: f32 = 0.999;
/// A seam: the two polygons' normals' dot at least this (dihedral ~0).
const SEAM: f32 = 0.999;
/// A concave crease: the neighbour's centroid is at least this far in
/// front of the polygon's plane (the neighbour rises away from the edge).
const CONCAVE: f32 = 1e-3;
/// The two joints are parallel: their horizontal directions' dot at least
/// this.
const PARALLEL: f32 = 0.98;
/// Joints closer than this are one joint.
const MIN_GAP: f32 = 0.02;
/// The two joints overlap along their length by at least this.
const MIN_OVERLAP: f32 = 0.05;
/// Launch per unit of crease `1 − cos(dihedral)`, from one point:
/// Ascension's crease (ramp normal z 0.8396 against flat floor) gives the
/// 17.85 units/s the body gained on the touchdown tick
/// (docs/game/superbounce.md, "The touchdown tick").
const LAUNCH_PER_CREASE: f32 = 17.85 / (1.0 - 0.8396);
/// A strip that launches slower than this is a landing, not a bounce.
const MIN_LAUNCH: f32 = 3.0;
/// Launch estimates past this are capped and scored down: the line is
/// fitted to one crease.
const MAX_LAUNCH: f32 = 60.0;
/// Pinch past first touch: the shallowest traced bounce sank 0.070 below
/// the floor, and on the 0.2 strip the capsule's bottom first touches both
/// edges at 0.031 (docs/game/superbounce.md, "Traces").
const PINCH_DEPTH: f32 = 0.04;
/// Ticks a second the bounce was measured at (the lab runs at 30).
const TICKS: f32 = 30.0;
/// Vertical speed a standing jump lands at on the floor it left (units/s):
/// "a standing jump at the patch lands at −3".
const JUMP_LAND: f32 = 3.0;
/// Gravity (units/s²), fitted from the recipe: a standing jump lands at 3,
/// the jump off the upper level (z −0.76) onto the patch (z −3.19, a 2.43
/// drop) lands at 5.4: (5.4² − 3²) / (2 × 2.43) ≈ 4.15.
const GRAVITY: f32 = 4.15;
/// The stand point is this far from the strip's midline toward the
/// crease, so drifting across carries the landing onto the midline: the
/// measured spot stands at y 3.88 on the strip y 3.8..4.0.
const STAND_BACK: f32 = 0.02;

/// How two walkable polygons meet along a shared edge.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Join {
    /// Coplanar.
    Seam,
    /// Concave, with `1 − n_a · n_b`.
    Crease(f32),
}

impl Join {
    fn crease(self) -> f32 {
        match self {
            Join::Seam => 0.0,
            Join::Crease(c) => c,
        }
    }
}

/// Where two walkable polygons share a stretch of edge.
#[derive(Clone, Copy, Debug)]
struct Joint {
    a: Vec3,
    b: Vec3,
    polys: [usize; 2],
    join: Join,
}

fn xy(p: Vec3) -> V2 {
    [p[0], p[1]]
}

/// A joint's horizontal direction, unit.
fn hdir(j: &Joint) -> Option<V2> {
    let d = sub2(xy(j.b), xy(j.a));
    let l = len2(d);
    (l > 1e-6).then(|| [d[0] / l, d[1] / l])
}

/// The height of a joint's line over the point of it nearest `p`.
fn line_z(j: &Joint, p: V2) -> f32 {
    let d = sub2(xy(j.b), xy(j.a));
    let l2 = dot2(d, d).max(1e-12);
    let t = dot2(sub2(p, xy(j.a)), d) / l2;
    j.a[2] + t * (j.b[2] - j.a[2])
}

pub(crate) fn bounces(faces: &[Face]) -> Vec<Spot> {
    let floors: Vec<usize> = (0..faces.len())
        .filter(|&i| faces[i].poly.normal[2] > WALKABLE)
        .collect();
    let joints = joints(faces, &floors);
    let mut g = Grid::new(crate::scan::CELL);
    for (i, j) in joints.iter().enumerate() {
        g.insert(
            i as u32,
            [j.a[0].min(j.b[0]), j.a[1].min(j.b[1])],
            [j.a[0].max(j.b[0]), j.a[1].max(j.b[1])],
        );
    }
    let mut found = Vec::new();
    let mut near = Vec::new();
    let d = 2.0 * RADIUS;
    for (i, p) in joints.iter().enumerate() {
        g.query(
            [p.a[0].min(p.b[0]) - d, p.a[1].min(p.b[1]) - d],
            [p.a[0].max(p.b[0]) + d, p.a[1].max(p.b[1]) + d],
            &mut near,
        );
        for &k in &near {
            if k as usize > i
                && let Some(spot) = strip(faces, p, &joints[k as usize])
            {
                found.push(spot);
            }
        }
    }
    best(merge_bounces(found))
}

/// Every stretch where two walkable polygons share an edge, as a seam or
/// a concave crease (convex joints are left out: they tilt nothing up).
fn joints(faces: &[Face], floors: &[usize]) -> Vec<Joint> {
    struct Edge {
        poly: usize,
        a: Vec3,
        b: Vec3,
    }
    let mut edges = Vec::new();
    for &i in floors {
        let v = &faces[i].poly.verts;
        for k in 0..v.len() {
            let (a, b) = (v[k], v[(k + 1) % v.len()]);
            if len(sub(b, a)) >= MIN_EDGE {
                edges.push(Edge { poly: i, a, b });
            }
        }
    }
    let mut g = Grid::new(crate::scan::CELL);
    for (i, e) in edges.iter().enumerate() {
        g.insert(
            i as u32,
            [e.a[0].min(e.b[0]) - JOIN_TOL, e.a[1].min(e.b[1]) - JOIN_TOL],
            [e.a[0].max(e.b[0]) + JOIN_TOL, e.a[1].max(e.b[1]) + JOIN_TOL],
        );
    }
    let mut out = Vec::new();
    let mut near = Vec::new();
    for (i, e) in edges.iter().enumerate() {
        g.query(
            [e.a[0].min(e.b[0]), e.a[1].min(e.b[1])],
            [e.a[0].max(e.b[0]), e.a[1].max(e.b[1])],
            &mut near,
        );
        let Some(dir) = unit(sub(e.b, e.a)) else {
            continue;
        };
        let l = len(sub(e.b, e.a));
        for &k in &near {
            let f = &edges[k as usize];
            if (k as usize) <= i || f.poly == e.poly {
                continue;
            }
            let Some(fd) = unit(sub(f.b, f.a)) else {
                continue;
            };
            if dot(dir, fd).abs() < COLLINEAR
                || point_line(f.a, e.a, e.b) > JOIN_TOL
                || point_line(f.b, e.a, e.b) > JOIN_TOL
            {
                continue;
            }
            let (t0, t1) = (dot(sub(f.a, e.a), dir), dot(sub(f.b, e.a), dir));
            let (lo, hi) = (t0.min(t1).max(0.0), t0.max(t1).min(l));
            if hi - lo < MIN_EDGE {
                continue;
            }
            let (na, nb) = (faces[e.poly].poly.normal, faces[f.poly].poly.normal);
            let join = if dot(na, nb) >= SEAM {
                Join::Seam
            } else if dot(na, sub(faces[f.poly].b.centroid, e.a)) > CONCAVE {
                Join::Crease(1.0 - dot(na, nb))
            } else {
                continue;
            };
            let at = |t: f32| {
                let s = scale(dir, t);
                [e.a[0] + s[0], e.a[1] + s[1], e.a[2] + s[2]]
            };
            out.push(Joint {
                a: at(lo),
                b: at(hi),
                polys: [e.poly, f.poly],
                join,
            });
        }
    }
    out
}

/// Two polygons are one floor: the same, or coplanar.
fn same_floor(faces: &[Face], i: usize, k: usize) -> bool {
    let (a, b) = (faces[i].poly, faces[k].poly);
    i == k
        || (dot(a.normal, b.normal) >= SEAM
            && dot(a.normal, sub(b.verts[0], a.verts[0])).abs() <= JOIN_TOL)
}

/// The bounce spot on the strip between joints `p` and `q`, if they make
/// one.
fn strip(faces: &[Face], p: &Joint, q: &Joint) -> Option<Spot> {
    // `c` is the crease (the sharper, for two); `o` the other joint.
    let (c, o) = match (p.join, q.join) {
        (Join::Seam, Join::Seam) => return None,
        (Join::Crease(x), Join::Crease(y)) if y > x => (q, p),
        (Join::Seam, Join::Crease(_)) => (q, p),
        _ => (p, q),
    };
    // One floor between them: a polygon on each joint that is the same
    // floor.
    if !c
        .polys
        .iter()
        .any(|&i| o.polys.iter().any(|&k| same_floor(faces, i, k)))
    {
        return None;
    }
    let (u, ou) = (hdir(c)?, hdir(o)?);
    if dot2(u, ou).abs() < PARALLEL {
        return None;
    }
    let perp = [-u[1], u[0]];
    let om = [(o.a[0] + o.b[0]) / 2.0, (o.a[1] + o.b[1]) / 2.0];
    let signed = dot2(perp, sub2(om, xy(c.a)));
    let gap = signed.abs();
    if !(gap >= MIN_GAP && gap < 2.0 * RADIUS) {
        return None;
    }
    // From the crease toward the other joint.
    let drift = if signed >= 0.0 {
        perp
    } else {
        [-perp[0], -perp[1]]
    };
    // Overlap along the crease.
    let lc = dot2(u, sub2(xy(c.b), xy(c.a)));
    let (t0, t1) = (
        dot2(u, sub2(xy(o.a), xy(c.a))),
        dot2(u, sub2(xy(o.b), xy(c.a))),
    );
    let (lo, hi) = (t0.min(t1).max(0.0), t0.max(t1).min(lc));
    if hi - lo < MIN_OVERLAP {
        return None;
    }
    // The midline between them at floor height, at both ends.
    let mid_at = |t: f32| -> Option<Vec3> {
        let cxy = [c.a[0] + u[0] * t, c.a[1] + u[1] * t];
        let oxy = [cxy[0] + drift[0] * gap, cxy[1] + drift[1] * gap];
        let (cz, oz) = (line_z(c, cxy), line_z(o, oxy));
        // The floor between rises at most 45°.
        ((cz - oz).abs() <= gap + JOIN_TOL).then(|| {
            [
                cxy[0] + drift[0] * gap / 2.0,
                cxy[1] + drift[1] * gap / 2.0,
                (cz + oz) / 2.0,
            ]
        })
    };
    let (e0, e1) = (mid_at(lo)?, mid_at(hi)?);

    let launch_raw = LAUNCH_PER_CREASE * (c.join.crease() + o.join.crease());
    if launch_raw < MIN_LAUNCH {
        return None;
    }
    let launch = launch_raw.min(MAX_LAUNCH);

    // fall: the touchdown tick moves the capsule v / 30 down; started
    // anywhere in the tick, half the landings sink at least v / 60. With
    // the floor not caught, the capsule's bottom reaches both edges
    // r − √(r² − (gap/2)²) below the floor, and a bounce needs PINCH_DEPTH
    // more.
    let half = gap / 2.0;
    let first_touch = RADIUS - (RADIUS * RADIUS - half * half).sqrt();
    let land = 2.0 * TICKS * (first_touch + PINCH_DEPTH);
    let fall = ((land * land - JUMP_LAND * JUMP_LAND) / (2.0 * GRAVITY)).max(0.0);

    let m = [
        (e0[0] + e1[0]) / 2.0,
        (e0[1] + e1[1]) / 2.0,
        (e0[2] + e1[2]) / 2.0,
    ];
    let stand = [
        m[0] - drift[0] * STAND_BACK,
        m[1] - drift[1] * STAND_BACK,
        m[2] + ORIGIN_LIFT,
    ];
    let overlap_fit = ((hi - lo) / (2.0 * RADIUS)).min(1.0);
    // The traced shape is one crease and one seam.
    let shape_fit = if o.join == Join::Seam { 1.0 } else { 0.5 };
    let mut score = 0.4 + 0.3 * overlap_fit + 0.3 * shape_fit;
    if launch_raw > MAX_LAUNCH {
        score *= 0.5;
    }
    Some(Spot {
        kind: Kind::Bounce,
        edge: [e0, e1],
        stand,
        fall,
        launch,
        drift,
        score,
        width: gap,
        need: land,
        ..Spot::default()
    })
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
            // The easiest landing along the merged slot, with the width
            // and touchdown speed it is worked from.
            if s.fall < last.fall {
                last.fall = s.fall;
                last.need = s.need;
                last.width = s.width;
            }
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
