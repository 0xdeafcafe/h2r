//! Bounce spots: a narrow strip of walkable floor between two parallel
//! floor joints, at least one of them a concave crease, and the single
//! creases the sim's training tries (docs/game/superbounce.md, "Spots in
//! the geometry", "Bounce").
//!
//! At Ascension the touchdown tick's two contacts (normals `(0, −1, 0)` at
//! y 4.105 and `(0, 0.9993, 0.0372)` at y 3.939) are Havok's contacts with
//! floor edges, not walls. The strip y 3.8..4.0 has two joints: the crease
//! where the ramp (normal z 0.8396) meets the floor at y 3.800, and a flat
//! seam with the next floor piece at y 4.000. The contacts straddle
//! y 4.02, on the seam: the pinch sits on the seam, and a spot's target
//! is that collision edge, its two end vertices as the map stores them
//! ([`strip`]).
//!
//! A single concave crease (a flat floor at the foot of a ramp) is a
//! candidate of its own ([`Shape::Crease`]): the Zanzibar window landing is
//! one, a flat floor meeting a 17.6° ramp along y −26.70, with no second
//! joint (docs/game/superbounce.md, "Single creases"). Two ramps meeting
//! at the bottom, a V ([`Shape::Vee`], Sanctuary's), are single creases
//! too, ranked high, with the groove's bottom line as the target.
//! Candidates are for the sim's training to test; play shows them only
//! once proved. A seam with structure over it nearer than a launch climbs
//! ([`covered`]) is no spot at all. A floor vertex where many edges meet
//! is a busy point (`busy.rs`), a candidate on the vertex itself.
use crate::geom::{
    Grid, V2, dot, dot2, inside2, len, len2, plane_z, point_line, sub, sub2, unit, xy_ccw,
};
use crate::scan::{Face, ORIGIN_LIFT, WALKABLE, grid_of};
use crate::{BounceExplain, CreaseNote, Kind, RADIUS, Shape, Spot, Vec3};

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
pub(crate) const LAUNCH_PER_CREASE: f32 = 17.85 / (1.0 - 0.8396);
/// A strip that launches slower than this is a landing, not a bounce.
pub(crate) const MIN_LAUNCH: f32 = 3.0;
/// Launch estimates past this are capped and scored down: the line is
/// fitted to one crease.
pub(crate) const MAX_LAUNCH: f32 = 60.0;
/// Bounce spots kept at most, slots and candidates together, best score
/// first. A sanity bound, not a ranking cut: a map's creases and slots
/// come to a few hundred, so a high-scoring candidate (the Zanzibar
/// window, 0.53) is never dropped for lower ones.
pub(crate) const MAX_BOUNCES: usize = 4096;
/// Ascension's crease, `1 − cos(dihedral)` with its ramp at z 0.8396: a
/// single crease's score counts its own against this.
const ASCENSION_CREASE: f32 = 1.0 - 0.8396;
/// A V's two floors each tilt at least this from flat (their normals' z
/// under cos 5°): a flat floor at a ramp's foot is no V.
const VEE_TILT: f32 = 0.9962;
/// The cover test samples the target line this often (world units), and
/// both ends.
const COVER_STEP: f32 = 0.25;
/// Faces less than this over the floor are the floor itself, not cover.
const COVER_MIN: f32 = 0.05;
/// A "monster bounce" (Halopedia: a V of converging lines): at least this
/// many other creases end on a spot's target line, crossing it
/// ([`converging`]).
const MONSTER_CONVERGING: usize = 2;
/// A monster's score at [`MONSTER_CONVERGING`] creases, 0.05 more for each
/// further one to 0.95: ranked with the Vs. Unmeasured, like the Vs'.
const MONSTER_SCORE: f32 = 0.85;

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

/// Every joint between two walkable polygons.
fn floor_joints(faces: &[Face]) -> Vec<Joint> {
    let floors: Vec<usize> = (0..faces.len())
        .filter(|&i| faces[i].poly.normal[2] > WALKABLE)
        .collect();
    joints(faces, &floors)
}

/// Two-joint slots and single creases, each merged along its line, and
/// busy points, best score first, at most [`MAX_BOUNCES`]. Creases and
/// busy points are ranked with the slots by score alone: no cap of their
/// own drops a high-scoring one (busy points have a score floor instead,
/// `busy::MIN_POINTS`).
pub(crate) fn bounces(faces: &[Face]) -> Vec<Spot> {
    let joints = floor_joints(faces);
    let creases: Vec<Spot> = joints
        .iter()
        .filter_map(|j| crease(faces, j).ok())
        .collect();
    let mut out = merge_bounces(slots(faces, &joints));
    out.extend(merge_bounces(creases));
    out.extend(crate::busy::busy(faces));
    let grid = grid_of(faces, &(0..faces.len()).collect::<Vec<_>>());
    out.retain(|s| covered(faces, &grid, s.edge).is_none());
    for s in &mut out {
        let n = converging(&joints, s.edge);
        if n >= MONSTER_CONVERGING {
            let more = (n.min(MONSTER_CONVERGING + 2) - MONSTER_CONVERGING) as f32;
            s.score = s.score.max(MONSTER_SCORE + 0.05 * more);
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.truncate(MAX_BOUNCES);
    out
}

/// How many concave creases other than `line`'s own run into it: an end
/// on the line (to [`JOIN_TOL`]), not parallel to it. Lines converging on
/// a seam, a monster bounce's shape. Seams are not counted: a floor's
/// triangulation ends coplanar seams on every joint.
fn converging(joints: &[Joint], line: [Vec3; 2]) -> usize {
    let Some(u) = unit(sub(line[1], line[0])) else {
        return 0;
    };
    let l = len(sub(line[1], line[0]));
    joints
        .iter()
        .filter(|j| matches!(j.join, Join::Crease(_)))
        .filter(|j| {
            let Some(d) = unit(sub(j.b, j.a)) else {
                return false;
            };
            dot(u, d).abs() < PARALLEL
                && [j.a, j.b].into_iter().any(|p| {
                    let t = dot(sub(p, line[0]), u);
                    (-JOIN_TOL..=l + JOIN_TOL).contains(&t)
                        && point_line(p, line[0], line[1]) <= JOIN_TOL
                })
        })
        .count()
}

/// The bounce side of `--explain`: every concave joint within `r` of `at`
/// judged as a single crease, and the bounce spots near it.
pub(crate) fn explain(faces: &[Face], at: Vec3, r: f32) -> BounceExplain {
    let joints = floor_joints(faces);
    let grid = grid_of(faces, &(0..faces.len()).collect::<Vec<_>>());
    let creases = joints
        .iter()
        .filter_map(|j| {
            let Join::Crease(c) = j.join else {
                return None;
            };
            if crate::dist_to_edge([j.a, j.b], at) > r {
                return None;
            }
            let dihedral = (1.0 - c).clamp(-1.0, 1.0).acos().to_degrees();
            let (spot, rejected) = match crease(faces, j) {
                Ok(s) => match covered(faces, &grid, s.edge) {
                    Some(h) => (
                        None,
                        Some(format!(
                            "covered: structure {h:.2} over it, under the {:.2} a launch needs",
                            crate::speed::cover()
                        )),
                    ),
                    None => (Some(s), None),
                },
                Err(why) => (None, Some(why)),
            };
            Some(CreaseNote {
                edge: [j.a, j.b],
                dihedral,
                spot,
                rejected,
            })
        })
        .collect();
    let spots = bounces(faces)
        .into_iter()
        .filter(|s| crate::dist_to_edge(s.edge, at) <= r)
        .collect();
    BounceExplain { creases, spots }
}

/// The single-crease candidate on joint `j`, or why it is none.
///
/// - **edge**: the crease itself, the collision edge's two end vertices:
///   the joint is the target.
/// - **launch**: [`LAUNCH_PER_CREASE`] × its own `1 − cos(dihedral)`, the
///   Ascension calibration with no second joint: 5.2 units/s for
///   Zanzibar's 17.6° ramp, the "mini bounce" scale.
/// - **drift**: across the crease onto the flatter floor, the side
///   landed on.
/// - **stand**: the crease's middle, on the line, at its height plus the
///   origin's lift.
/// - **need**, **fall**: none. Nothing has measured what a crease needs,
///   so the spot carries no speed and no drop.
/// - **score**: 0.3, plus 0.2 × its length over a diameter (to 1), plus
///   0.1 × its crease over Ascension's (to 1): at most 0.6, so a slot of
///   the traced shape outranks every crease.
///
/// Two ramps meeting at the bottom, each tilted over 5° and rising away
/// from the joint, are a V ([`Shape::Vee`]): the groove's bottom line is
/// the target, and it scores 0.5, plus 0.25 × its length fit, plus 0.2 ×
/// its crease fit, at most 0.95, over every other single crease (the
/// user's Sanctuary bounces are Vs).
fn crease(faces: &[Face], j: &Joint) -> Result<Spot, String> {
    let Join::Crease(c) = j.join else {
        return Err("a seam".into());
    };
    let u = hdir(j).ok_or("not horizontal")?;
    let length = len(sub(j.b, j.a));
    if length < MIN_OVERLAP {
        return Err(format!("{length:.3} long, under {MIN_OVERLAP}"));
    }
    let launch_raw = LAUNCH_PER_CREASE * c;
    if launch_raw < MIN_LAUNCH {
        return Err(format!(
            "launch {launch_raw:.1} units/s, under {MIN_LAUNCH}"
        ));
    }
    let launch = launch_raw.min(MAX_LAUNCH);
    // The flatter floor is the one landed on.
    let [p, q] = j.polys;
    let flat = if faces[p].poly.normal[2] >= faces[q].poly.normal[2] {
        p
    } else {
        q
    };
    let perp = [-u[1], u[0]];
    let toward = sub2(xy(faces[flat].b.centroid), xy(j.a));
    let drift = if dot2(perp, toward) >= 0.0 {
        perp
    } else {
        [-perp[0], -perp[1]]
    };
    let m = mid([j.a, j.b]);
    let stand = [m[0], m[1], m[2] + ORIGIN_LIFT];
    let (na, nb) = (faces[p].poly.normal, faces[q].poly.normal);
    // Each floor's horizontal normal points across the joint toward it
    // when the floor rises away from it: opposite sides for a V.
    let (sa, sb) = (dot2(perp, [na[0], na[1]]), dot2(perp, [nb[0], nb[1]]));
    let vee = na[2] < VEE_TILT && nb[2] < VEE_TILT && sa * sb < 0.0;
    let (length_fit, crease_fit) = (
        (length / (2.0 * RADIUS)).min(1.0),
        (c / ASCENSION_CREASE).min(1.0),
    );
    let mut score = if vee {
        0.5 + 0.25 * length_fit + 0.2 * crease_fit
    } else {
        0.3 + 0.2 * length_fit + 0.1 * crease_fit
    };
    if launch_raw > MAX_LAUNCH {
        score *= 0.5;
    }
    Ok(Spot {
        kind: Kind::Bounce,
        edge: [j.a, j.b],
        stand,
        launch,
        drift,
        score,
        shape: if vee { Shape::Vee } else { Shape::Crease },
        ..Spot::default()
    })
}

/// How high (world units) the lowest structure over `line` is, when at
/// least half the points along it (each [`COVER_STEP`], and both ends)
/// have a face over them within [`crate::speed::cover`] of their floor:
/// a seam under a low ceiling or indoors, which can't launch anyone.
/// `None` for open air. A column cast up from each point: any face that
/// isn't vertical counts, an upper floor as much as a ceiling.
fn covered(faces: &[Face], grid: &Grid, line: [Vec3; 2]) -> Option<f32> {
    let reach = crate::speed::cover();
    let n = ((len(sub(line[1], line[0])) / COVER_STEP).ceil() as usize).max(1);
    let mut near = Vec::new();
    let mut hits = 0;
    let mut lowest = f32::INFINITY;
    for k in 0..=n {
        let t = k as f32 / n as f32;
        let p = [
            line[0][0] + (line[1][0] - line[0][0]) * t,
            line[0][1] + (line[1][1] - line[0][1]) * t,
            line[0][2] + (line[1][2] - line[0][2]) * t,
        ];
        let at = [p[0], p[1]];
        grid.query(at, at, &mut near);
        let over = near
            .iter()
            .map(|&i| &faces[i as usize])
            .filter(|f| f.b.max[2] > p[2] + COVER_MIN && f.b.min[2] <= p[2] + reach)
            .filter_map(|f| {
                let h = plane_z(f.poly, at)? - p[2];
                (h > COVER_MIN && h <= reach && inside2(&xy_ccw(&f.poly.verts), at, 1e-4))
                    .then_some(h)
            })
            .reduce(f32::min);
        if let Some(h) = over {
            hits += 1;
            lowest = lowest.min(h);
        }
    }
    (2 * hits > n).then_some(lowest)
}

/// Every strip between two joints that makes a slot.
fn slots(faces: &[Face], joints: &[Joint]) -> Vec<Spot> {
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
    found
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
            // The overlap's ends are vertices, to the float: `e`'s own
            // where `f` runs past it, else `f`'s (on `e`'s line to
            // JOIN_TOL), never a point worked out along the line.
            let (f_lo, f_hi) = if t0 <= t1 {
                ((f.a, t0), (f.b, t1))
            } else {
                ((f.b, t1), (f.a, t0))
            };
            let (a, lo) = if f_lo.1 > 0.0 { f_lo } else { (e.a, 0.0) };
            let (b, hi) = if f_hi.1 < l { f_hi } else { (e.b, l) };
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
            out.push(Joint {
                a,
                b,
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

/// The point of joint `j`'s horizontal line nearest `p`.
fn foot(j: &Joint, p: V2) -> V2 {
    let a = xy(j.a);
    let d = sub2(xy(j.b), a);
    let t = dot2(sub2(p, a), d) / dot2(d, d).max(1e-12);
    [a[0] + d[0] * t, a[1] + d[1] * t]
}

/// The bounce spot on the strip between joints `p` and `q`, if they make
/// one.
///
/// Its target (`edge`) is the joint the pinch sits on: that collision
/// edge itself, its two end vertices to the float, and `stand` is on it
/// across the middle of the two joints' overlap.
/// Which joint: Ascension's slot has two, the concave crease at y 3.800
/// (ramp meets strip) and the flat seam at y 4.000 (strip meets the main
/// floor piece). The touchdown tick's two contacts, y 4.105 with normal
/// `(0, −1, 0)` and y 3.939 with normal `(0, 0.9993, 0.0372)`, straddle
/// y 4.022, and the stepped run's origin was y 4.04: the capsule sank
/// into the seam between the edges of the two coplanar pieces. A contact
/// with the ramp would carry the ramp's normal (z 0.84), and none did. The
/// bots' one learned Ascension site settled its aim at y 3.985, on the
/// seam too. So with a seam the pinch is on the seam. Two creases (a
/// trough) have no seam and nothing measured: the sharper crease is
/// taken.
///
/// The community guide's Ascension spot says "jump onto the line where
/// the ramp meets the ground", the crease. From the capsule: centred at
/// y 4.02 the lower sphere (radius 0.175) reaches y 3.845, short of the
/// crease at 3.800; however deep it sinks its centre stays over 0.22 from
/// the crease line, the ramp polygon's nearest edge, so neither is
/// touched, and the 3.939 contact is 0.14 from the crease. The
/// guide names the line a player sees: the seam between coplanar pieces
/// is not drawn, and the two are 0.2 apart (about 60 cm), one landing's
/// width. Both lines stay targets for the sim to settle: the slot on the
/// seam, and the crease's own candidate on y 3.800 ([`crease`]).
///
/// No band: how far off the edge a landing still counts is the one catch,
/// [`crate::CATCH`], for the verdict only.
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
    if !(MIN_GAP..2.0 * RADIUS).contains(&gap) {
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
    // The joint the pinch sits on: the seam when there is one.
    let on_seam = o.join == Join::Seam;
    // The target line's point across from the crease at `t`, at floor
    // height on its joint.
    let target_at = |t: f32| -> Option<Vec3> {
        let cxy = [c.a[0] + u[0] * t, c.a[1] + u[1] * t];
        let oxy = foot(o, cxy);
        let (cz, oz) = (line_z(c, cxy), line_z(o, oxy));
        // The floor between rises at most 45°.
        let across = len2(sub2(oxy, cxy));
        ((cz - oz).abs() <= across + JOIN_TOL).then(|| {
            if on_seam {
                [oxy[0], oxy[1], oz]
            } else {
                [cxy[0], cxy[1], cz]
            }
        })
    };
    // The floor between rises at most 45° along the whole overlap.
    target_at(lo)?;
    target_at(hi)?;
    let target = if on_seam { o } else { c };

    let launch_raw = LAUNCH_PER_CREASE * (c.join.crease() + o.join.crease());
    if launch_raw < MIN_LAUNCH {
        return None;
    }
    let launch = launch_raw.min(MAX_LAUNCH);

    let m = target_at((lo + hi) / 2.0)?;
    let stand = [m[0], m[1], m[2] + ORIGIN_LIFT];
    let overlap_fit = ((hi - lo) / (2.0 * RADIUS)).min(1.0);
    // The traced shape is one crease and one seam.
    let shape_fit = if on_seam { 1.0 } else { 0.5 };
    let mut score = 0.4 + 0.3 * overlap_fit + 0.3 * shape_fit;
    if launch_raw > MAX_LAUNCH {
        score *= 0.5;
    }
    // No `need` and no `fall`: nothing has measured the touchdown speed a
    // slot needs, so the spot carries none.
    Some(Spot {
        kind: Kind::Bounce,
        edge: [target.a, target.b],
        stand,
        launch,
        drift,
        score,
        ..Spot::default()
    })
}

/// Bounce spots on one line: the same drift (to 5°), the same target
/// line across (to [`JOIN_TOL`]) and height (to 10 cm), and sharing an end
/// vertex or overlapping along it (to [`JOIN_TOL`]) are one spot. The
/// merged edge runs from the first's start vertex to the last's end
/// vertex, so it stays a collision edge's ends; no gap is bridged. Its
/// aim is the better-scored spot's.
fn merge_bounces(spots: Vec<Spot>) -> Vec<Spot> {
    const ANGLE: f32 = 5.0 * std::f32::consts::PI / 180.0;
    const ACROSS: f32 = JOIN_TOL;
    const HEIGHT: f32 = 0.1;
    const JOIN: f32 = JOIN_TOL;
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
            if s1 > l1 {
                last.edge[1] = s.edge[1];
            }
            if s.score > last.score {
                last.stand = s.stand;
            }
            last.launch = (last.launch * wl + s.launch * ws) / (wl + ws);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn crease(a: Vec3, b: Vec3) -> Joint {
        Joint {
            a,
            b,
            polys: [0, 1],
            join: Join::Crease(0.1),
        }
    }

    #[test]
    fn creases_converging_on_a_line_are_counted_and_seams_and_parallels_are_not() {
        let line = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0]];
        let joints = [
            // Its own crease, along it: parallel, not counted.
            crease([0.0, 0.0, 0.0], [4.0, 0.0, 0.0]),
            // Two creases ending on it, one at its end.
            crease([1.0, 0.0, 0.0], [1.5, 1.0, 0.1]),
            crease([4.0, 0.0, 0.0], [5.0, -1.0, 0.2]),
            // A seam ending on it: a floor's triangulation.
            Joint {
                join: Join::Seam,
                ..crease([2.0, 0.0, 0.0], [2.0, 1.0, 0.0])
            },
            // A crease that stops short of it.
            crease([3.0, 0.2, 0.0], [3.0, 1.0, 0.0]),
        ];
        assert_eq!(converging(&joints, line), 2);
        assert_eq!(converging(&joints[..2], line), 1);
    }
}
