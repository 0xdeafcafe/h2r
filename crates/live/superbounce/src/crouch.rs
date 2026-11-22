//! Forced-crouch spots: places on walkable floor where the standing
//! capsule hits the geometry and the crouched one doesn't, wide and long
//! enough to stand in, reachable on foot, then cut by the stricter rules:
//! in play, by a bounce strip, spread out, measured places first
//! (docs/game/superbounce.md, "Spots in the geometry", "Crouch", "Crouch
//! spots: the stricter rule").
//!
//! The measured spot on Ascension is a corner, not a low ceiling: the
//! player at (−10.91, 0.72, −1.16) is beside a small prop whose
//! overhanging lip (underside normal `(0, 0.7071, −0.7071)`, y 0.56..0.61,
//! z −0.73..−0.68) comes within 0.142 of the capsule's axis. Standing, the
//! capsule's top reaches into the lip; crouched, it stays under it. The
//! band where that holds is 5 cm wide, the lip's overhang.
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::geom::{
    Grid, V2, add, area2, closest_seg_seg, dot2, inside2, len, len2, plane_z, scale, seg_poly, sub,
    sub2, xy_ccw,
};
use crate::scan::{Face, ORIGIN_LIFT, Rules, WALKABLE, grid_of};
use crate::{
    CROUCHED, CrouchRegion, Kind, Lip, MAX_LIP, PROVEN_NEAR, Prime, RADIUS, STANDING, Spot, Status,
    Vec3, Verdict,
};

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
/// A region is at least this many samples: one alone is a speck.
const MIN_SAMPLES: usize = 2;
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
/// A bounce strip this close (straight line) scores a crouch spot up: you
/// prime, then go and bounce.
const BOUNCE_NEAR: f32 = 15.0;
/// In play: a stand more than this below the lowest starting location is
/// under the map (5 of Ascension's spots were, at z −5.4 to −21.1;
/// docs/game/notes/superbounce-spots.md).
const IN_PLAY_BELOW: f32 = 1.0;
/// By a strip: a bounce strip's stand within this (straight line).
const STRIP_NEAR: f32 = 8.0;
/// By a strip: when fewer spots than this pass, the nearest a strip are
/// kept up to it.
const MIN_KEPT: usize = 3;
/// Spread out: crouch spots within this of a better one are dropped.
const SPACING: f32 = 3.0;
/// Spread out: crouch spots kept per scan, best first.
const MAX_CROUCH: usize = 12;
/// A lip edge is clipped to this each side of its point nearest the
/// capsule, so a big ceiling's edge draws as the part overhead.
const LIP_HALF: f32 = 0.5;

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

    /// The edges of the faces the standing capsule hits at `origin`, one a
    /// face (the edge nearest the capsule's segment, clipped to
    /// [`LIP_HALF`] each side of its nearest point), nearest first, at most
    /// [`MAX_LIP`].
    fn lip(&self, origin: Vec3) -> Vec<[Vec3; 2]> {
        let mut near = Vec::new();
        self.obstacles.query(
            [origin[0] - RADIUS, origin[1] - RADIUS],
            [origin[0] + RADIUS, origin[1] + RADIUS],
            &mut near,
        );
        let (p0, p1) = segment(origin, STANDING);
        let mut hits: Vec<(f32, [Vec3; 2])> = Vec::new();
        for &i in &near {
            let o = &self.faces[i as usize];
            let Some(d) = face_hit(o, origin, STANDING) else {
                continue;
            };
            let v = &o.poly.verts;
            let edge = (0..v.len())
                .map(|k| {
                    let (a, b) = (v[k], v[(k + 1) % v.len()]);
                    let (c1, c2) = closest_seg_seg(p0, p1, a, b);
                    (len(sub(c1, c2)), a, b, c2)
                })
                .min_by(|x, y| x.0.total_cmp(&y.0));
            let Some((_, a, b, c)) = edge else {
                continue;
            };
            let clip = |end: Vec3| {
                let d = sub(end, c);
                let l = len(d);
                if l > LIP_HALF {
                    add(c, scale(d, LIP_HALF / l))
                } else {
                    end
                }
            };
            let e = [clip(a), clip(b)];
            if len(sub(e[0], e[1])) < 1e-3 || hits.iter().any(|(_, h)| same_edge(*h, e)) {
                continue;
            }
            hits.push((d, e));
        }
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        hits.into_iter().take(MAX_LIP).map(|(_, e)| e).collect()
    }

    /// The forced band's width at sample `s`: across it each way to where
    /// the crouch is no longer forced, along each lattice axis, and the
    /// narrower of the two. One axis alone would measure a band's length
    /// at its ends, where the way out runs along it; the narrower axis is
    /// at most √2 times the true width.
    fn band_width(&self, s: &Sample) -> f32 {
        let free = |(stand, crouch): (bool, bool)| !(stand && !crouch);
        let across = |d: V2| self.probe(s, d, free) + self.probe(s, [-d[0], -d[1]], free);
        across([1.0, 0.0]).min(across([0.0, 1.0]))
    }
}

pub(crate) fn crouches(faces: &[Face], bounces: &[Spot], rules: &Rules) -> Vec<Spot> {
    let (scene, samples) = sample(faces);
    let mut found = regions(&scene, &samples, bounces);
    select(&mut found, bounces, rules)
}

/// The scene, and every candidate origin tested.
fn sample<'a>(faces: &'a [Face<'a>]) -> (Scene<'a>, Vec<Sample>) {
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
    (scene, samples)
}

/// One forced-crouch region with how the filters judged it.
pub(crate) struct Candidate {
    pub report: CrouchRegion,
    /// Its samples' origins.
    pub origins: Vec<Vec3>,
}

/// How the stricter rules judged one candidate, before its verdicts go
/// into its report in order.
struct Judged {
    i: usize,
    proven: Option<Vec3>,
    in_play: Verdict,
    strip: Verdict,
    /// Distance to the nearest strip's stand; infinite with none.
    strip_d: f32,
    spread: Option<Verdict>,
}

/// The stricter rules (docs/game/superbounce.md, "Crouch spots: the
/// stricter rule"): measured places first and always; then, of the
/// spots in play, those by a strip best first, spread out and capped;
/// then, while fewer than [`MIN_KEPT`] are kept, the nearest a strip of
/// the rest. Each candidate's report gets every rule's verdict.
fn select(found: &mut Vec<Candidate>, bounces: &[Spot], rules: &Rules) -> Vec<Spot> {
    let mut judged = Vec::new();
    for (i, c) in found.iter().enumerate() {
        let Some(s) = c.report.spot else {
            continue;
        };
        let proven = rules
            .proven
            .iter()
            .map(|p| p.at)
            .find(|&p| c.origins.iter().any(|o| len(sub(*o, p)) <= PROVEN_NEAR));
        let in_play = match rules.lowest {
            None => Verdict {
                rule: "in play",
                pass: true,
                note: "no starting location over this geometry: not applied".into(),
            },
            Some(l) => Verdict {
                rule: "in play",
                pass: s.stand[2] >= l[2] - IN_PLAY_BELOW,
                note: format!(
                    "stand z {:.2}, lowest starting location {}, so no lower than {:.2}",
                    s.stand[2],
                    v3(l),
                    l[2] - IN_PLAY_BELOW
                ),
            },
        };
        let nearest = bounces
            .iter()
            .map(|b| (b.stand, len(sub(b.stand, s.stand))))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let strip = Verdict {
            rule: "by a strip",
            pass: nearest.is_some_and(|(_, d)| d <= STRIP_NEAR),
            note: match nearest {
                None => "no bounce strip on this geometry".into(),
                Some((at, d)) => format!(
                    "nearest strip stands at {}, {d:.2} away (within {STRIP_NEAR} passes)",
                    v3(at)
                ),
            },
        };
        judged.push(Judged {
            i,
            proven,
            in_play,
            strip,
            strip_d: nearest.map_or(f32::INFINITY, |(_, d)| d),
            spread: None,
        });
    }
    let score: Vec<f32> = judged
        .iter()
        .map(|k| found[k.i].report.spot.map_or(0.0, |s| s.score))
        .collect();
    let mut kept: Vec<Spot> = Vec::new();

    // Proven first: kept whatever the other rules say.
    let mut order: Vec<usize> = (0..judged.len())
        .filter(|&j| judged[j].proven.is_some())
        .collect();
    order.sort_by(|&a, &b| score[b].total_cmp(&score[a]));
    for j in order {
        let r = &mut found[judged[j].i].report;
        let mut s = r.spot.expect("judged on spot");
        s.status = Status::Proven;
        s.score = 1.0;
        r.spot = Some(s);
        r.status = Status::Proven;
        r.kept = true;
        judged[j].spread = Some(Verdict {
            rule: "spread out",
            pass: true,
            note: "proven: not cut".into(),
        });
        kept.push(s);
    }
    // A measured place no region is near (its lip is an object, not the
    // structure): a proven spot of its own, standing where it was
    // measured.
    let unmatched: Vec<Prime> = rules
        .proven
        .iter()
        .filter(|p| !judged.iter().any(|k| k.proven == Some(p.at)))
        .copied()
        .collect();
    for p in &unmatched {
        kept.push(measured_spot(p));
    }

    // Spread out and capped: `try_keep` keeps a spot unless the cap is
    // reached or a kept one is within SPACING.
    let try_keep = |found: &mut [Candidate], kept: &mut Vec<Spot>, j: &mut Judged| {
        let r = &mut found[j.i].report;
        let s = r.spot.expect("judged on spot");
        if kept.len() >= MAX_CROUCH {
            r.rejected = Some(format!("not among the {MAX_CROUCH} best"));
            j.spread = Some(Verdict {
                rule: "spread out",
                pass: false,
                note: format!("{MAX_CROUCH} spots already kept"),
            });
        } else if let Some(k) = kept.iter().find(|k| len(sub(k.stand, s.stand)) < SPACING) {
            r.suppressed_by = Some(k.stand);
            r.rejected = Some(format!(
                "within {SPACING} of a better spot (score {:.2})",
                k.score
            ));
            j.spread = Some(Verdict {
                rule: "spread out",
                pass: false,
                note: format!(
                    "crowded out by the spot at {}, {:.2} away, score {:.2}",
                    v3(k.stand),
                    len(sub(k.stand, s.stand)),
                    k.score
                ),
            });
        } else {
            r.kept = true;
            j.spread = Some(Verdict {
                rule: "spread out",
                pass: true,
                note: format!("no kept spot within {SPACING}"),
            });
            kept.push(s);
        }
    };

    // In play and by a strip, best first.
    let mut order: Vec<usize> = (0..judged.len())
        .filter(|&j| {
            let k = &judged[j];
            k.proven.is_none() && k.in_play.pass && k.strip.pass
        })
        .collect();
    order.sort_by(|&a, &b| score[b].total_cmp(&score[a]));
    for j in order {
        try_keep(&mut *found, &mut kept, &mut judged[j]);
    }

    // In play but far from a strip: the nearest, while fewer than
    // MIN_KEPT are kept.
    let mut order: Vec<usize> = (0..judged.len())
        .filter(|&j| {
            let k = &judged[j];
            k.proven.is_none() && k.in_play.pass && !k.strip.pass
        })
        .collect();
    order.sort_by(|&a, &b| {
        let (ja, jb) = (&judged[a], &judged[b]);
        ja.strip_d
            .total_cmp(&jb.strip_d)
            .then(score[b].total_cmp(&score[a]))
    });
    for j in order {
        if kept.len() >= MIN_KEPT {
            let k = &judged[j];
            found[k.i].report.rejected = Some(if k.strip_d.is_finite() {
                format!(
                    "no bounce strip within {STRIP_NEAR} (nearest {:.2}), and {MIN_KEPT} are kept",
                    k.strip_d
                )
            } else {
                format!("no bounce strip, and {MIN_KEPT} are kept")
            });
            continue;
        }
        try_keep(&mut *found, &mut kept, &mut judged[j]);
        let k = &mut judged[j];
        if found[k.i].report.kept {
            k.strip.pass = true;
            k.strip.note += &format!(": fewer than {MIN_KEPT} pass, kept as one of the nearest");
        }
    }

    // Under the map.
    for k in judged
        .iter()
        .filter(|k| k.proven.is_none() && !k.in_play.pass)
    {
        found[k.i].report.rejected =
            Some("under the map: below the lowest starting location".into());
    }

    // Every rule's verdict, in order.
    for k in judged {
        let r = &mut found[k.i].report;
        let s = r.spot.expect("judged on spot");
        let spread = k.spread.unwrap_or(Verdict {
            rule: "spread out",
            pass: false,
            note: "not reached".into(),
        });
        let proven = match k.proven {
            Some(p) => Verdict {
                rule: "proven",
                pass: true,
                note: format!(
                    "proven: a sample within {PROVEN_NEAR} of the measured place {}; kept, ranked first",
                    v3(p)
                ),
            },
            None if rules.proven.is_empty() => Verdict {
                rule: "proven",
                pass: false,
                note: "likely: no measured priming place on this map".into(),
            },
            None => Verdict {
                rule: "proven",
                pass: false,
                note: format!("likely: no measured priming place within {PROVEN_NEAR}"),
            },
        };
        let stand = Verdict {
            rule: "stand at the way out",
            pass: true,
            note: format!(
                "stand {} out ({:.2}, {:.2}), {} lip edges",
                v3(s.stand),
                s.out[0],
                s.out[1],
                s.lip.len()
            ),
        };
        r.rules = vec![k.in_play, k.strip, spread, proven, stand];
    }
    for p in &unmatched {
        found.push(measured_region(p));
    }
    kept
}

/// Half the length of a measured spot's edge, across its way out.
const MEASURED_EDGE: f32 = 0.2;

/// A [`PROVEN`](crate::PROVEN) place with no region: a crouch spot at
/// `at`, its edge a short line across its way out on the floor.
fn measured_spot(p: &Prime) -> Spot {
    let out = if p.out == [0.0, 0.0] {
        [1.0, 0.0]
    } else {
        p.out
    };
    let across = [-out[1], out[0]];
    let floor = p.at[2] - ORIGIN_LIFT;
    let end = |k: f32| {
        [
            p.at[0] + across[0] * MEASURED_EDGE * k,
            p.at[1] + across[1] * MEASURED_EDGE * k,
            floor,
        ]
    };
    Spot {
        kind: Kind::Crouch,
        edge: [end(-1.0), end(1.0)],
        stand: p.at,
        score: 1.0,
        status: Status::Proven,
        out: p.out,
        ..Spot::default()
    }
}

/// What `--explain` says of a measured place with no region.
fn measured_region(p: &Prime) -> Candidate {
    let note = if p.note.is_empty() {
        String::new()
    } else {
        format!(": {}", p.note)
    };
    Candidate {
        report: CrouchRegion {
            samples: 0,
            min: p.at,
            max: p.at,
            widths: Vec::new(),
            length: 0.0,
            reachable: true,
            reach: "not walked: a measured place".into(),
            rejected: None,
            spot: Some(measured_spot(p)),
            suppressed_by: None,
            kept: true,
            rules: vec![Verdict {
                rule: "proven",
                pass: true,
                note: format!(
                    "proven: measured at {}, where the geometry shows no forced crouch (the lip is an object the scan doesn't read); kept as measured{note}",
                    v3(p.at)
                ),
            }],
            status: Status::Proven,
        },
        origins: vec![p.at],
    }
}

fn v3(p: Vec3) -> String {
    format!("({:.2}, {:.2}, {:.2})", p[0], p[1], p[2])
}

fn same_edge(a: [Vec3; 2], b: [Vec3; 2]) -> bool {
    let close = |p: Vec3, q: Vec3| len(sub(p, q)) < 1e-3;
    (close(a[0], b[0]) && close(a[1], b[1])) || (close(a[0], b[1]) && close(a[1], b[0]))
}

/// Every forced-crouch region with a sample within `r` of `at`, judged.
pub(crate) fn explain(
    faces: &[Face],
    bounces: &[Spot],
    rules: &Rules,
    at: Vec3,
    r: f32,
) -> Vec<CrouchRegion> {
    let (scene, samples) = sample(faces);
    let mut found = regions(&scene, &samples, bounces);
    select(&mut found, bounces, rules);
    found
        .into_iter()
        .filter(|c| c.origins.iter().any(|&o| len(sub(o, at)) <= r))
        .map(|c| c.report)
        .collect()
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
    near.iter()
        .any(|&i| face_hit(&faces[i as usize], origin, height).is_some())
}

/// The capsule's segment: origin + r to origin + height − r.
fn segment(origin: Vec3, height: f32) -> (Vec3, Vec3) {
    (
        [origin[0], origin[1], origin[2] + RADIUS],
        [origin[0], origin[1], origin[2] + height - RADIUS],
    )
}

/// The segment's distance to face `o` when the capsule of `height` at
/// `origin` is in it deeper than [`HIT_EPS`] and it isn't support.
fn face_hit(o: &Face, origin: Vec3, height: f32) -> Option<f32> {
    let (p0, p1) = segment(origin, height);
    let (z0, z1) = (p0[2], p1[2]);
    let b = o.b;
    if b.min[0] > origin[0] + RADIUS
        || b.max[0] < origin[0] - RADIUS
        || b.min[1] > origin[1] + RADIUS
        || b.max[1] < origin[1] - RADIUS
        || b.min[2] > z1 + RADIUS
        || b.max[2] < z0 - RADIUS
    {
        return None;
    }
    let (d, nearest) = seg_poly(p0, p1, &o.poly.verts);
    (d < RADIUS - HIT_EPS && !(o.poly.normal[2] > SUPPORT_NZ && nearest[2] < z0 + 1e-3))
        .then_some(d)
}

/// How the walk out of a region to open floor ended.
enum Reach {
    /// At a cell with no sample that is open floor, after this many clear
    /// samples.
    Open(V2, usize),
    /// Every clear sample walked (this many), none next to open floor.
    Enclosed(usize),
    /// The walk passed [`MAX_FLOOD`]: counted reachable.
    Capped,
}

/// Forced samples that neighbour each other (8-neighbours on one floor)
/// are one region. A region is a spot when it is more than a speck,
/// touches floor where standing is clear, that floor reaches open floor
/// (not the inside of a prop), and its band is wide enough to stand in
/// somewhere across its edge.
fn regions(scene: &Scene, samples: &[Sample], bounces: &[Spot]) -> Vec<Candidate> {
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
    // The way out of sample `i`: the sum of the sides with standing room,
    // unit; the first such side when they cancel.
    let way_out = |i: usize| -> V2 {
        let mut d = [0.0f32, 0.0];
        for (di, dj) in SIDES {
            if neighbour(i, di, dj).any(|k| !samples[k].stand_hit) {
                d[0] += di as f32;
                d[1] += dj as f32;
            }
        }
        let l = len2(d);
        if l > 1e-6 {
            [d[0] / l, d[1] / l]
        } else {
            exit(i).map_or([0.0, 0.0], |(di, dj)| [di as f32, dj as f32])
        }
    };
    // Walk clear samples from `start` (4-neighbours, one floor) until a
    // cell with no sample that is open floor: reachable on foot.
    let reaches_open = |start: Vec<usize>| -> Reach {
        let mut seen: HashSet<usize> = start.iter().copied().collect();
        let mut queue: VecDeque<usize> = start.into_iter().collect();
        while let Some(i) = queue.pop_front() {
            if seen.len() > MAX_FLOOD {
                return Reach::Capped;
            }
            let s = &samples[i];
            for (di, dj) in SIDES {
                let mut there = neighbour(i, di, dj).peekable();
                if there.peek().is_none() {
                    let p = at((s.ij.0 + di, s.ij.1 + dj));
                    if scene.open_floor(p, s.floor_z) {
                        return Reach::Open(p, seen.len());
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
        Reach::Enclosed(seen.len())
    };
    let mut out = Vec::new();
    for members in groups.into_values() {
        let origins: Vec<Vec3> = members.iter().map(|&i| samples[i].origin).collect();
        let (min, max) = origins.iter().fold(
            ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]),
            |(lo, hi), o| {
                (
                    std::array::from_fn(|k| lo[k].min(o[k])),
                    std::array::from_fn(|k| hi[k].max(o[k])),
                )
            },
        );
        let pts: Vec<V2> = members.iter().map(|&i| at(samples[i].ij)).collect();
        let (axis, _) = axes(&pts);
        let (a0, a1) = pts
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &p| {
                (lo.min(dot2(p, axis)), hi.max(dot2(p, axis)))
            });
        let length = (a1 - a0).max(0.0);
        let mut report = CrouchRegion {
            samples: members.len(),
            min,
            max,
            widths: Vec::new(),
            length,
            reachable: false,
            reach: String::new(),
            rejected: None,
            spot: None,
            suppressed_by: None,
            kept: false,
            rules: Vec::new(),
            status: Status::Likely,
        };
        let exits: Vec<(usize, (i32, i32))> = members
            .iter()
            .filter_map(|&i| exit(i).map(|d| (i, d)))
            .collect();
        let verdict = (|| {
            if members.len() < MIN_SAMPLES {
                return Err(format!("{} sample: a speck", members.len()));
            }
            if exits.is_empty() {
                report.reach = "no sample has standing room next door".into();
                return Err("no exits".into());
            }
            let doors: Vec<usize> = exits
                .iter()
                .flat_map(|&(i, (di, dj))| neighbour(i, di, dj).filter(|&k| !samples[k].stand_hit))
                .collect();
            match reaches_open(doors) {
                Reach::Open(p, n) => {
                    report.reachable = true;
                    report.reach = format!(
                        "reaches open floor at ({:.2}, {:.2}) after {n} clear samples",
                        p[0], p[1]
                    );
                }
                Reach::Capped => {
                    report.reachable = true;
                    report.reach =
                        format!("walk passed {MAX_FLOOD} clear samples: counted reachable");
                }
                Reach::Enclosed(n) => {
                    report.reach = format!(
                        "enclosed: {n} clear samples, none next to open floor (or all under a walkable face within {COVER:.3})"
                    );
                    return Err("enclosed".into());
                }
            }
            // The band's width across its edge at up to MAX_PROBED exits.
            let stride = exits.len().div_ceil(MAX_PROBED);
            let mut widest = 0.0f32;
            for &(i, d) in exits.iter().step_by(stride) {
                let w = scene.band_width(&samples[i]);
                widest = widest.max(w);
                report.widths.push((samples[i].origin, [d.0, d.1], w));
            }
            if widest < MIN_WIDTH {
                return Err(format!(
                    "band too thin: widest {:.3} across its edge, under {MIN_WIDTH}",
                    widest
                ));
            }
            Ok(spot(
                scene, samples, &members, &exits, axis, widest, length, bounces, &way_out,
            ))
        })();
        match verdict {
            Ok(s) => report.spot = Some(s),
            Err(why) => report.rejected = Some(why),
        }
        out.push(Candidate { report, origins });
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

#[allow(clippy::too_many_arguments)]
fn spot(
    scene: &Scene,
    samples: &[Sample],
    members: &[usize],
    exits: &[(usize, (i32, i32))],
    axis: V2,
    width: f32,
    length: f32,
    bounces: &[Spot],
    way_out: &dyn Fn(usize) -> V2,
) -> Spot {
    let pt = |i: usize| at(samples[i].ij);
    let n = members.len() as f32;
    let c = members.iter().fold([0.0, 0.0], |a, &i| {
        [a[0] + pt(i)[0] / n, a[1] + pt(i)[1] / n]
    });
    let centre = [c[0], c[1], samples[members[0]].origin[2]];
    let nearest = bounces
        .iter()
        .map(|b| (b, len(sub(b.stand, centre))))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    // The score only counts a strip within BOUNCE_NEAR.
    let bounce = nearest.filter(|(_, d)| *d <= BOUNCE_NEAR);
    // The stand: the exit sample (standing room next door) nearest the
    // nearest strip's stand, where the crouch lets go and the word turns
    // on the way to bounce; with no strip, the exit nearest the centre.
    let toward = |target: Vec3, flat: bool| {
        exits
            .iter()
            .map(|&(i, _)| i)
            .min_by(|&a, &b| {
                let d = |i: usize| {
                    let o = samples[i].origin;
                    let dz = if flat { 0.0 } else { o[2] - target[2] };
                    len([o[0] - target[0], o[1] - target[1], dz])
                };
                d(a).total_cmp(&d(b))
            })
            .expect("a spot has exits")
    };
    let stand = match nearest {
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
    let origin = samples[stand].origin;
    Spot {
        kind: Kind::Crouch,
        edge,
        stand: origin,
        score,
        width,
        out: way_out(stand),
        lip: Lip::new(&scene.lip(origin)),
        ..Spot::default()
    }
}
