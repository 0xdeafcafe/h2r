//! Forced-crouch spots: places on walkable floor where the standing
//! capsule hits the geometry and the crouched one doesn't
//! (docs/game/superbounce.md, "Spots in the geometry", "Crouch").
//!
//! The measured spot on Ascension is a corner, not a low ceiling: the
//! player at (−10.91, 0.72, −1.16) is beside a small prop whose
//! overhanging lip (underside normal `(0, 0.7071, −0.7071)`, y 0.56..0.61,
//! z −0.73..−0.68) comes within 0.142 of the capsule's axis. Standing, the
//! capsule's top reaches into the lip; crouched, it stays under it.
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::geom::{V2, area2, dot2, inside2, len2, plane_z, seg_poly, sub2, xy_ccw};
use crate::scan::{Face, ORIGIN_LIFT, WALKABLE, best, grid_of};
use crate::{CROUCHED, Kind, RADIUS, STANDING, Spot, Vec3};

/// Spacing (world units) of the candidate origins, on a world-aligned
/// lattice. Under the Ascension lip the forced-crouch band is about 5 cm
/// wide, so the lattice is finer than that.
const SAMPLE: f32 = 0.04;
/// A capsule hits a polygon when it is in deeper than this.
const HIT_EPS: f32 = 0.005;
/// A face looking up more than this whose nearest point is under the
/// capsule's lower sphere centre is support (a slope at the feet), not
/// an obstacle.
const SUPPORT_NZ: f32 = 0.3;
/// Neighbouring samples are on the same floor within this height.
const STEP_Z: f32 = 0.15;
/// A region narrower than this is a strip, cut into spots this long, so
/// every stand point is within a capsule's width of its whole spot.
const CHUNK: f32 = 2.0 * RADIUS;
/// Samples tested at most, over the whole map.
const MAX_SAMPLES: usize = 8_000_000;

/// One candidate origin and what the two capsules hit there.
struct Sample {
    ij: (i32, i32),
    floor_z: f32,
    origin: Vec3,
    stand_hit: bool,
    crouch_hit: bool,
}

impl Sample {
    fn forced(&self) -> bool {
        self.stand_hit && !self.crouch_hit
    }
}

fn lattice(v: f32) -> i32 {
    (v / SAMPLE).round() as i32
}

fn at(ij: (i32, i32)) -> V2 {
    [ij.0 as f32 * SAMPLE, ij.1 as f32 * SAMPLE]
}

pub(crate) fn crouches(faces: &[Face]) -> Vec<Spot> {
    let floors: Vec<usize> = (0..faces.len())
        .filter(|&i| faces[i].poly.normal[2] > WALKABLE)
        .collect();
    // Everything that isn't walkable floor can stop a capsule.
    let obstacles: Vec<usize> = (0..faces.len())
        .filter(|&i| faces[i].poly.normal[2] <= WALKABLE)
        .collect();
    let grid = grid_of(faces, &obstacles);
    let mut samples = Vec::new();
    let mut near = Vec::new();
    let mut hits = Vec::new();
    for &fi in &floors {
        let f = &faces[fi];
        let (lo, hi) = (
            [f.b.min[0] - RADIUS, f.b.min[1] - RADIUS],
            [f.b.max[0] + RADIUS, f.b.max[1] + RADIUS],
        );
        grid.query(lo, hi, &mut near);
        // Only where some geometry is within a standing capsule over the
        // floor.
        let mut cells = HashSet::new();
        for &oi in &near {
            let o = &faces[oi as usize];
            if o.b.max[2] > f.b.min[2] + 0.02 && o.b.min[2] < f.b.max[2] + STANDING + RADIUS {
                cells_near(o, lo, hi, &mut cells);
            }
        }
        let outline = xy_ccw(&f.poly.verts);
        let mut cells: Vec<_> = cells.into_iter().collect();
        cells.sort_unstable();
        for ij in cells {
            let p = at(ij);
            if !inside2(&outline, p, 1e-4) {
                continue;
            }
            let Some(fz) = plane_z(f.poly, p) else {
                continue;
            };
            // The lower sphere resting on the slope: its centre r above
            // the plane, so the origin is r / nz − r over the floor point.
            let nz = f.poly.normal[2];
            let origin = [p[0], p[1], fz + RADIUS / nz - RADIUS + ORIGIN_LIFT];
            grid.query(
                [p[0] - RADIUS, p[1] - RADIUS],
                [p[0] + RADIUS, p[1] + RADIUS],
                &mut hits,
            );
            let stand_hit = capsule_hits(faces, &hits, origin, STANDING);
            let crouch_hit = stand_hit && capsule_hits(faces, &hits, origin, CROUCHED);
            samples.push(Sample {
                ij,
                floor_z: fz,
                origin,
                stand_hit,
                crouch_hit,
            });
        }
        if samples.len() > MAX_SAMPLES {
            break;
        }
    }
    best(regions(&samples))
}

/// Lattice cells whose origin could put a capsule within reach of `o`
/// horizontally: inside its outline on the ground plane, or within a
/// radius of its edges. Limited to the box `lo`..`hi`.
fn cells_near(o: &Face, lo: V2, hi: V2, cells: &mut HashSet<(i32, i32)>) {
    let outline = xy_ccw(&o.poly.verts);
    let within = |p: V2| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1];
    // A face with real extent on the ground plane (a ceiling, a slope):
    // the cells under it.
    if area2(&outline) > 1e-4 {
        let (x0, x1) = (
            lattice(o.b.min[0].max(lo[0])),
            lattice(o.b.max[0].min(hi[0])),
        );
        let (y0, y1) = (
            lattice(o.b.min[1].max(lo[1])),
            lattice(o.b.max[1].min(hi[1])),
        );
        for i in x0..=x1 {
            for j in y0..=y1 {
                if inside2(&outline, at((i, j)), 0.0) {
                    cells.insert((i, j));
                }
            }
        }
    }
    // Within a radius of its edges (a wall's outline is a line).
    let reach = RADIUS + SAMPLE;
    let k = (reach / SAMPLE).ceil() as i32;
    for e in 0..outline.len() {
        let (a, b) = (outline[e], outline[(e + 1) % outline.len()]);
        let d = sub2(b, a);
        let steps = ((len2(d) / SAMPLE).ceil() as i32).max(1);
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let q = [a[0] + d[0] * t, a[1] + d[1] * t];
            if !(q[0] >= lo[0] - reach
                && q[0] <= hi[0] + reach
                && q[1] >= lo[1] - reach
                && q[1] <= hi[1] + reach)
            {
                continue;
            }
            let (ci, cj) = (lattice(q[0]), lattice(q[1]));
            for i in ci - k..=ci + k {
                for j in cj - k..=cj + k {
                    let p = at((i, j));
                    if within(p) && len2(sub2(p, q)) <= reach {
                        cells.insert((i, j));
                    }
                }
            }
        }
    }
}

/// Whether a capsule of `height` (bottom at `origin`, so its segment runs
/// from origin + r to origin + height − r, as the engine builds it:
/// docs/game/superbounce.md, "The crouch height term") is in any of the
/// faces `near` deeper than [`HIT_EPS`].
fn capsule_hits(faces: &[Face], near: &[u32], origin: Vec3, height: f32) -> bool {
    let z0 = origin[2] + RADIUS;
    let z1 = origin[2] + height - RADIUS;
    let (p0, p1) = ([origin[0], origin[1], z0], [origin[0], origin[1], z1]);
    near.iter().any(|&i| {
        let o = &faces[i as usize];
        let b = o.b;
        if b.min[0] > origin[0] + RADIUS
            || b.max[0] < origin[0] - RADIUS
            || b.min[1] > origin[1] + RADIUS
            || b.max[1] < origin[1] - RADIUS
            || b.min[2] > z1 + RADIUS
            || b.max[2] < z0 - RADIUS
        {
            return false;
        }
        let (d, nearest) = seg_poly(p0, p1, &o.poly.verts);
        d < RADIUS - HIT_EPS && !(o.poly.normal[2] > SUPPORT_NZ && nearest[2] < z0 + 1e-3)
    })
}

/// Forced samples that neighbour each other (8-neighbours on one floor)
/// are one region; each region, or each piece of a long strip, is a spot.
fn regions(samples: &[Sample]) -> Vec<Spot> {
    let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, s) in samples.iter().enumerate() {
        cells.entry(s.ij).or_default().push(i);
    }
    let by_cell = &cells;
    let neighbour = move |i: usize, di: i32, dj: i32| {
        let s = &samples[i];
        by_cell
            .get(&(s.ij.0 + di, s.ij.1 + dj))
            .into_iter()
            .flatten()
            .copied()
            .filter(move |&k| (samples[k].floor_z - s.floor_z).abs() <= STEP_Z)
    };
    let forced: Vec<usize> = (0..samples.len())
        .filter(|&i| samples[i].forced())
        .collect();
    let mut parent: HashMap<usize, usize> = forced.iter().map(|&i| (i, i)).collect();
    fn root(p: &mut HashMap<usize, usize>, mut i: usize) -> usize {
        while p[&i] != i {
            let up = p[&p[&i]];
            p.insert(i, up);
            i = up;
        }
        i
    }
    for &i in &forced {
        for (di, dj) in [(1, 0), (0, 1), (1, 1), (1, -1)] {
            for k in neighbour(i, di, dj).filter(|&k| samples[k].forced()) {
                let (a, b) = (root(&mut parent, i), root(&mut parent, k));
                if a != b {
                    parent.insert(a.max(b), a.min(b));
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &i in &forced {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    // Where standing is clear next door: the direction out to it.
    let exit = |i: usize| -> Option<(i32, i32)> {
        [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .find(|&(di, dj)| neighbour(i, di, dj).any(|k| !samples[k].stand_hit))
    };
    let mut out = Vec::new();
    for members in groups.into_values() {
        let pts: Vec<V2> = members.iter().map(|&i| at(samples[i].ij)).collect();
        let (axis, minor) = axes(&pts);
        let proj = |p: V2, d: V2| dot2(p, d);
        let range = |d: V2| {
            pts.iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &p| {
                    (lo.min(proj(p, d)), hi.max(proj(p, d)))
                })
        };
        let (a0, a1) = range(axis);
        let (m0, m1) = range(minor);
        let length = a1 - a0;
        let pieces = if m1 - m0 < CHUNK && length > CHUNK {
            (length / CHUNK).ceil() as usize
        } else {
            1
        };
        let mut split = vec![Vec::new(); pieces];
        for (&i, &p) in members.iter().zip(&pts) {
            let k = if length > 0.0 {
                (((proj(p, axis) - a0) / length * pieces as f32) as usize).min(pieces - 1)
            } else {
                0
            };
            split[k].push(i);
        }
        for piece in split.into_iter().filter(|p| !p.is_empty()) {
            out.push(spot(samples, &piece, axis, &exit));
        }
    }
    out
}

/// The principal axis of a point set on the ground plane and the one
/// across it.
fn axes(pts: &[V2]) -> (V2, V2) {
    let n = pts.len().max(1) as f32;
    let m = pts
        .iter()
        .fold([0.0, 0.0], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n]);
    let (mut xx, mut xy, mut yy) = (0.0f32, 0.0f32, 0.0f32);
    for p in pts {
        let d = sub2(*p, m);
        xx += d[0] * d[0];
        xy += d[0] * d[1];
        yy += d[1] * d[1];
    }
    let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
    let axis = [angle.cos(), angle.sin()];
    (axis, [-axis[1], axis[0]])
}

fn spot(
    samples: &[Sample],
    piece: &[usize],
    axis: V2,
    exit: &impl Fn(usize) -> Option<(i32, i32)>,
) -> Spot {
    let pt = |i: usize| at(samples[i].ij);
    let n = piece.len() as f32;
    let c = piece.iter().fold([0.0, 0.0], |a, &i| {
        [a[0] + pt(i)[0] / n, a[1] + pt(i)[1] / n]
    });
    let centre = *piece
        .iter()
        .min_by(|&&a, &&b| len2(sub2(pt(a), c)).total_cmp(&len2(sub2(pt(b), c))))
        .expect("a piece has samples");
    // The edge: along the biggest group of samples with standing room the
    // same way out, across that way; with none, along the region.
    let mut sides: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
    for &i in piece {
        if let Some(d) = exit(i) {
            sides.entry(d).or_default().push(i);
        }
    }
    let side = sides.into_iter().max_by_key(|(_, v)| v.len());
    let (line, along) = match &side {
        Some(((di, dj), v)) => (v.as_slice(), [-(*dj as f32), *di as f32]),
        None => (piece, axis),
    };
    let ends = |pick: &dyn Fn(f32, f32) -> bool| {
        line.iter()
            .copied()
            .reduce(|a, b| {
                if pick(dot2(pt(b), along), dot2(pt(a), along)) {
                    b
                } else {
                    a
                }
            })
            .expect("an edge has samples")
    };
    let (lo, hi) = (ends(&|b, a| b < a), ends(&|b, a| b > a));
    let point = |i: usize| {
        let p = pt(i);
        [p[0], p[1], samples[i].floor_z]
    };
    let mut edge = [point(lo), point(hi)];
    if lo == hi {
        let h = SAMPLE / 2.0;
        edge[0][0] -= along[0] * h;
        edge[0][1] -= along[1] * h;
        edge[1][0] += along[0] * h;
        edge[1][1] += along[1] * h;
    }
    let exit_bonus = if side.is_some() { 0.3 } else { 0.0 };
    let size_bonus = 0.3 * (n / 4.0).min(1.0);
    Spot {
        kind: Kind::Crouch,
        edge,
        stand: samples[centre].origin,
        fall: 0.0,
        launch: 0.0,
        drift: [0.0, 0.0],
        score: 0.4 + exit_bonus + size_bonus,
    }
}
