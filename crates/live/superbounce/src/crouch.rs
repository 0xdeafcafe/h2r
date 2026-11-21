//! Forced-crouch spots: places on walkable floor where the standing
//! capsule hits the geometry and the crouched one doesn't, wide and long
//! enough to stand in, reachable on foot, best near a bounce strip
//! (docs/game/superbounce.md, "Spots in the geometry", "Crouch").
//!
//! The measured spot on Ascension is a corner, not a low ceiling: the
//! player at (−10.91, 0.72, −1.16) is beside a small prop whose
//! overhanging lip (underside normal `(0, 0.7071, −0.7071)`, y 0.56..0.61,
//! z −0.73..−0.68) comes within 0.142 of the capsule's axis. Standing, the
//! capsule's top reaches into the lip; crouched, it stays under it. The
//! band where that holds is 5 cm wide, the lip's overhang.
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::geom::{
    Grid, V2, area2, dot2, inside2, len, len2, plane_z, seg_poly, sub, sub2, xy_ccw,
};
use crate::scan::{Face, ORIGIN_LIFT, WALKABLE, grid_of};
use crate::{CROUCHED, Kind, RADIUS, STANDING, Spot, Vec3};

/// Spacing (world units) of the candidate origins, on a world-aligned
/// lattice. Under the Ascension lip the forced-crouch band is 5 cm wide,
/// so the lattice is finer than that.
const SAMPLE: f32 = 0.04;
/// A capsule hits a polygon when it is in deeper than this.
const HIT_EPS: f32 = 0.005;
/// A face looking up more than this whose nearest point is under the
/// capsule's lower sphere centre is support (a slope at the feet), not
/// an obstacle.
const SUPPORT_NZ: f32 = 0.3;
/// Neighbouring samples are on the same floor within this height.
const STEP_Z: f32 = 0.15;
/// Samples tested at most, over the whole map.
const MAX_SAMPLES: usize = 8_000_000;
/// A spot's band, measured across its edge, is at least this wide. The
/// Ascension lip's band measures 5 cm in its test geometry
/// (`tests.rs`, `ascension_corner`); the hair-thin bands under low ledges,
/// where only the crouched capsule's rounded top slips under, are 1 to 2
/// cm (a ledge at 0.4: 0.157 to 0.17 from its edge).
const MIN_WIDTH: f32 = 0.04;
/// Its wide part is at least this long along the edge.
const MIN_LENGTH: f32 = 0.25;
/// Band widths are probed this far each way at most.
const PROBE_MAX: f32 = 0.3;
/// Coarse probe step, then three halvings (to 1.25 mm).
const PROBE_STEP: f32 = 0.01;
/// Exit samples probed per region at most, spread over its exits.
const MAX_PROBED: usize = 24;
/// Clear lattice cells walked at most looking for open floor; past this
/// the region counts as reachable.
const MAX_FLOOD: usize = 20_000;
/// A walkable face this far over a floor point (a prop's top) covers it:
/// the point is inside the prop, not open floor.
const COVER: f32 = STANDING + 2.0 * RADIUS;
/// Crouch spots within this of a better one are dropped.
const SUPPRESS: f32 = 1.5;
/// A bounce strip this close (straight line) scores a crouch spot up: you
/// prime, then go and bounce.
const BOUNCE_NEAR: f32 = 15.0;
/// Crouch spots kept per scan, best first.
const MAX_CROUCH: usize = 30;

/// One candidate origin and what the two capsules hit there.
struct Sample {
    ij: (i32, i32),
    /// The walkable face it's on.
    floor: usize,
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

/// The faces and their buckets.
struct Scene<'a> {
    faces: &'a [Face<'a>],
    /// Everything that isn't walkable floor: what can stop a capsule.
    obstacles: Grid,
    floors: Grid,
}

impl Scene<'_> {
    /// The origin standing on walkable face `floor` over `p`: the lower
    /// sphere resting on the slope, its centre r above the plane, so the
    /// origin is r / nz − r over the floor point.
    fn origin(&self, floor: usize, p: V2) -> Option<(f32, Vec3)> {
        let f = self.faces[floor].poly;
        let fz = plane_z(f, p)?;
        let nz = f.normal[2];
        Some((fz, [p[0], p[1], fz + RADIUS / nz - RADIUS + ORIGIN_LIFT]))
    }

    /// Whether the standing capsule, then (only when it hits) the crouched
    /// one, hit anything at `origin`.
    fn capsules(&self, origin: Vec3, near: &mut Vec<u32>) -> (bool, bool) {
        self.obstacles.query(
            [origin[0] - RADIUS, origin[1] - RADIUS],
            [origin[0] + RADIUS, origin[1] + RADIUS],
            near,
        );
        let stand = capsule_hits(self.faces, near, origin, STANDING);
        (
            stand,
            stand && capsule_hits(self.faces, near, origin, CROUCHED),
        )
    }

    /// Open floor at `p`, about height `z`: walkable floor there, and no
    /// walkable face over it within [`COVER`] (a prop's top).
    fn open_floor(&self, p: V2, z: f32) -> bool {
        let mut near = Vec::new();
        self.floors.query(p, p, &mut near);
        let mut on = false;
        for &i in &near {
            let f = self.faces[i as usize].poly;
            let Some(fz) = plane_z(f, p) else {
                continue;
            };
            if !inside2(&xy_ccw(&f.verts), p, 1e-4) {
                continue;
            }
            if (fz - z).abs() <= STEP_Z {
                on = true;
            } else if fz > z + 0.05 && fz < z + COVER {
                return false;
            }
        }
        on
    }

    /// Distance from the sample `s` along `d` until `clear` holds, to
    /// [`PROBE_MAX`].
    fn probe(&self, s: &Sample, d: V2, clear: impl Fn((bool, bool)) -> bool) -> f32 {
        let mut near = Vec::new();
        let p = at(s.ij);
        let mut test = |t: f32| {
            let q = [p[0] + d[0] * t, p[1] + d[1] * t];
            self.origin(s.floor, q)
                .is_none_or(|(_, o)| clear(self.capsules(o, &mut near)))
        };
        let mut t = PROBE_STEP;
        while t <= PROBE_MAX {
            if test(t) {
                let (mut lo, mut hi) = (t - PROBE_STEP, t);
                for _ in 0..3 {
                    let m = (lo + hi) / 2.0;
                    if test(m) {
                        hi = m;
                    } else {
                        lo = m;
                    }
                }
                return (lo + hi) / 2.0;
            }
            t += PROBE_STEP;
        }
        PROBE_MAX
    }

    /// The forced band's width at exit sample `s`, across its exit `d`:
    /// out to where standing is clear, plus back in to where the crouch
    /// is no longer forced.
    fn band_width(&self, s: &Sample, d: (i32, i32)) -> f32 {
        let d = [d.0 as f32, d.1 as f32];
        let out = self.probe(s, d, |(stand, _)| !stand);
        let back = self.probe(s, [-d[0], -d[1]], |(stand, crouch)| !(stand && !crouch));
        out + back
    }
}

pub(crate) fn crouches(faces: &[Face], bounces: &[Spot]) -> Vec<Spot> {
    let floors: Vec<usize> = (0..faces.len())
        .filter(|&i| faces[i].poly.normal[2] > WALKABLE)
        .collect();
    let obstacles: Vec<usize> = (0..faces.len())
        .filter(|&i| faces[i].poly.normal[2] <= WALKABLE)
        .collect();
    let scene = Scene {
        faces,
        obstacles: grid_of(faces, &obstacles),
        floors: grid_of(faces, &floors),
    };
    let mut samples = Vec::new();
    let mut near = Vec::new();
    let mut hits = Vec::new();
    for &fi in &floors {
        let f = &faces[fi];
        let (lo, hi) = (
            [f.b.min[0] - RADIUS, f.b.min[1] - RADIUS],
            [f.b.max[0] + RADIUS, f.b.max[1] + RADIUS],
        );
        scene.obstacles.query(lo, hi, &mut near);
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
            let Some((floor_z, origin)) = scene.origin(fi, p) else {
                continue;
            };
            let (stand_hit, crouch_hit) = scene.capsules(origin, &mut hits);
            samples.push(Sample {
                ij,
                floor: fi,
                floor_z,
                origin,
                stand_hit,
                crouch_hit,
            });
        }
        if samples.len() > MAX_SAMPLES {
            break;
        }
    }
    keep_best(regions(&scene, &samples, bounces))
}

/// Best score first, dropping any within [`SUPPRESS`] of a better one, at
/// most [`MAX_CROUCH`].
fn keep_best(mut spots: Vec<Spot>) -> Vec<Spot> {
    spots.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Spot> = Vec::new();
    for s in spots {
        if kept.len() == MAX_CROUCH {
            break;
        }
        if kept.iter().all(|k| len(sub(k.stand, s.stand)) >= SUPPRESS) {
            kept.push(s);
        }
    }
    kept
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
/// are one region. A region is a spot when it touches floor where
/// standing is clear, that floor reaches open floor (not the inside of a
/// prop), and its band is wide and long enough to stand in.
fn regions(scene: &Scene, samples: &[Sample], bounces: &[Spot]) -> Vec<Spot> {
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
    const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    // Where standing is clear next door: the direction out to it.
    let exit = |i: usize| -> Option<(i32, i32)> {
        SIDES
            .into_iter()
            .find(|&(di, dj)| neighbour(i, di, dj).any(|k| !samples[k].stand_hit))
    };
    // Walk clear samples from `start` (4-neighbours, one floor) until a
    // cell with no sample that is open floor: reachable on foot.
    let reaches_open = |start: Vec<usize>| -> bool {
        let mut seen: HashSet<usize> = start.iter().copied().collect();
        let mut queue: VecDeque<usize> = start.into_iter().collect();
        while let Some(i) = queue.pop_front() {
            if seen.len() > MAX_FLOOD {
                return true;
            }
            let s = &samples[i];
            for (di, dj) in SIDES {
                let mut there = neighbour(i, di, dj).peekable();
                if there.peek().is_none() {
                    if scene.open_floor(at((s.ij.0 + di, s.ij.1 + dj)), s.floor_z) {
                        return true;
                    }
                    continue;
                }
                for k in there {
                    if !samples[k].stand_hit && seen.insert(k) {
                        queue.push_back(k);
                    }
                }
            }
        }
        false
    };
    let mut out = Vec::new();
    for members in groups.into_values() {
        let exits: Vec<(usize, (i32, i32))> = members
            .iter()
            .filter_map(|&i| exit(i).map(|d| (i, d)))
            .collect();
        if exits.is_empty() {
            continue;
        }
        let doors: Vec<usize> = exits
            .iter()
            .flat_map(|&(i, (di, dj))| neighbour(i, di, dj).filter(|&k| !samples[k].stand_hit))
            .collect();
        if !reaches_open(doors) {
            continue;
        }
        // The band's width across its edge at up to MAX_PROBED exits.
        let stride = exits.len().div_ceil(MAX_PROBED);
        let mut wide = Vec::new();
        let mut widest = 0.0f32;
        for &(i, d) in exits.iter().step_by(stride) {
            let w = scene.band_width(&samples[i], d);
            widest = widest.max(w);
            if w >= MIN_WIDTH {
                wide.push(i);
            }
        }
        let pts: Vec<V2> = wide.iter().map(|&i| at(samples[i].ij)).collect();
        let (axis, _) = axes(&pts);
        let (a0, a1) = pts
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &p| {
                (lo.min(dot2(p, axis)), hi.max(dot2(p, axis)))
            });
        let length = a1 - a0;
        if wide.len() < 2 || length < MIN_LENGTH {
            continue;
        }
        out.push(spot(
            samples, &members, &exits, axis, widest, length, bounces,
        ));
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
    members: &[usize],
    exits: &[(usize, (i32, i32))],
    axis: V2,
    width: f32,
    length: f32,
    bounces: &[Spot],
) -> Spot {
    let pt = |i: usize| at(samples[i].ij);
    // The stand: the sample nearest the nearest bounce strip's stand,
    // when one is within BOUNCE_NEAR (the end you leave from to go and
    // bounce); otherwise the sample nearest the region's centre.
    let n = members.len() as f32;
    let c = members.iter().fold([0.0, 0.0], |a, &i| {
        [a[0] + pt(i)[0] / n, a[1] + pt(i)[1] / n]
    });
    let centre = [c[0], c[1], samples[members[0]].origin[2]];
    let bounce = bounces
        .iter()
        .map(|b| (b, len(sub(b.stand, centre))))
        .filter(|(_, d)| *d <= BOUNCE_NEAR)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let toward = |target: Vec3, flat: bool| {
        *members
            .iter()
            .min_by(|&&a, &&b| {
                let d = |i: usize| {
                    let o = samples[i].origin;
                    let dz = if flat { 0.0 } else { o[2] - target[2] };
                    len([o[0] - target[0], o[1] - target[1], dz])
                };
                d(a).total_cmp(&d(b))
            })
            .expect("a region has samples")
    };
    let stand = match bounce {
        Some((b, _)) => toward(b.stand, false),
        None => toward(centre, true),
    };
    // The edge: along the biggest group of exits the same way out,
    // across that way.
    let mut sides: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
    for &(i, d) in exits {
        sides.entry(d).or_default().push(i);
    }
    let ((di, dj), line) = sides
        .into_iter()
        .max_by_key(|(_, v)| v.len())
        .expect("a spot has exits");
    let along = [-(dj as f32), di as f32];
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
        edge[0][0] -= axis[0] * h;
        edge[0][1] -= axis[1] * h;
        edge[1][0] += axis[0] * h;
        edge[1][1] += axis[1] * h;
    }
    // 0.4, plus up to 0.2 for width (to 15 cm), 0.1 for length (to 1
    // unit), 0.3 for a bounce strip nearby (closer is more).
    let near = bounce.map_or(0.0, |(_, d)| 1.0 - d / BOUNCE_NEAR);
    let score = 0.4 + 0.2 * (width / 0.15).min(1.0) + 0.1 * length.min(1.0) + 0.3 * near;
    Spot {
        kind: Kind::Crouch,
        edge,
        stand: samples[stand].origin,
        fall: 0.0,
        launch: 0.0,
        drift: [0.0, 0.0],
        score,
    }
}
