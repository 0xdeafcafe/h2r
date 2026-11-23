//! The bounce rule (docs/game/superbounce.md, "The bounce rule"): whether
//! a primed body comes down into a superbounce, stepped a game tick at a
//! time over the floor polygons under it, with no game.
//!
//! A bounce happens on the one tick where all four hold:
//!
//! 1. the pair is primed (word −3): the caller's to know, not an input;
//! 2. the sweep is on: the tick's speed, with its gravity, is over
//!    [`SWEEP_SPEED`] (its square over [`SWEEP_SQ`], `0x002db359`);
//! 3. the swept capsule's axis goes through a floor polygon near an edge:
//!    the axis's lowest point a depth `cb` under the polygon, the crossing
//!    point `de` from the polygon's nearest edge, and `de` under
//!    [`EDGE_SCALE`]` × cb`;
//! 4. the body moves across that edge at [`DRIFT_MIN`] or more.
//!
//! A near miss fails one way each ([`Outcome`]): caught in the air (the
//! swept capsule reaches the floor from above), stopped in the floor (no
//! sweep, the body already through), a pop (the axis through a polygon
//! far from its edges), a level pinch (on an edge with no drift across
//! it), or a landing.
//!
//! [`predict`] is the predictor of docs/game/notes/2026-11-23-why-bounces-work.md
//! ("The predictor"), with two changes: the speed across the edge is the
//! horizontal velocity across the edge's horizontal line (the note's took
//! the in-plane direction, which on a ramp counts the fall itself), and
//! the drift the stick holds is an input ([`predict`]'s `drift`).
//!
//! The game's HUD reads the verdict once airborne, the bots accept or
//! reject a takeoff with it, and the bounce search scores it.

use crate::geom::{cross, dot, segment_xy, sub};
use crate::{Poly, RADIUS, Vec3};

/// A game tick (seconds).
pub const TICK: f32 = 1.0 / 30.0;
/// Gravity (units/s²), fitted to the control drops: 0.13904 units/s a tick.
pub const GRAVITY: f32 = 4.171;
/// How fast the air stick changes horizontal velocity (units/s²).
pub const AIR_ACCEL: f32 = 1.05;
/// The longest fall the predictor steps: eight seconds.
pub const MAX_TICKS: u32 = 240;

/// The speed (units/s) over which the biped's collision agent sweeps the
/// capsule along the tick's motion: the wrapper shape's `+0x24` on
/// `masterchief_mp` (Trace).
pub const SWEEP_SPEED: f32 = 5.4465;
/// [`SWEEP_SPEED`] squared, the wrapper's `+0x20`: the test is the tick's
/// velocity squared over it (`0x002db359..0x002db365`).
pub const SWEEP_SQ: f32 = 29.6644;
/// The origin to the centre of the capsule's bottom sphere: the bottom is
/// 0.01 under the origin, the radius [`RADIUS`].
pub const BASE: f32 = RADIUS - 0.01;
/// The contact tolerance (units): a capsule this near a floor at the
/// start of an unswept tick touches it. The origin 0.040 over a floor
/// lands, 0.045 goes through (Trace).
pub const CONTACT: f32 = 0.03;
/// The edge rule's scale: the normal is sideways when `de` is under this
/// times `cb`. The measured rule is `de < cb`; 0.9 is the predictor's
/// fit on the search's first round.
pub const EDGE_SCALE: f32 = 0.9;
/// The least speed (units/s) across the edge, on the bounce tick, that
/// leans the pinch into a launch. A drift of 0.10 leaves +0.07, 0.15
/// leaves +6.2 (Trace, at 5.39 in, 0.06 from the edge).
pub const DRIFT_MIN: f32 = 0.1;
/// The drift across the edge (units/s) that gives a full launch: 0.15
/// leaves +6.2, 0.20 +9.4, 0.30 +13.0.
pub const DRIFT_FULL: f32 = 0.15;
/// A polygon is a floor when its normal's Z is over this.
pub const FLOOR_NZ: f32 = 0.2;
/// The drop from the apex (units, origin to the origin standing on the
/// floor) whose sweep tick comes as the body reaches the floor: under
/// [`DROP_MIN`] the sweep never comes on before the floor, over
/// [`DROP_MAX`] it comes on too high and the capsule is caught.
pub const DROP_MIN: f32 = 3.35;
/// See [`DROP_MIN`].
pub const DROP_MAX: f32 = 3.60;
/// The drop with the most phases that bounce: about three in four, 2 cm
/// from an edge.
pub const DROP_BEST: f32 = 3.45;

/// Whether a drop from the apex is in the window.
pub fn drop_in_window(drop: f32) -> bool {
    (DROP_MIN..=DROP_MAX).contains(&drop)
}

/// How a bounce came: which tick turned the sweep on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// The sweep came on this tick: a fall of 5.3075 to 5.4465 at the
    /// tick's start, the body having gone into the floor unswept the tick
    /// before. The drop window's route.
    Threshold,
    /// The sweep was already on: a faster fall whose origin starts the
    /// tick under `speed / 30 − 0.165` over the floor.
    Fast,
}

/// What a fall comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// A superbounce, if the pair is primed.
    Bounce(Route),
    /// The swept capsule reaches the floor from above: caught 5 to 15 cm
    /// over it.
    Caught,
    /// No sweep, and the body is already through the floor: it stops 4 to
    /// 9 cm in it.
    Stopped,
    /// The axis goes through a polygon away from its edges: one contact,
    /// normal up, a pop of +1.5 to +2.4.
    Pop,
    /// On an edge, with too little drift across it: two level contacts,
    /// no lean, stops in the floor.
    LevelPinch,
    /// An ordinary landing.
    Landing,
    /// No floor within the flight.
    NoFloor,
}

impl Outcome {
    /// `bounce`, `caught`, `stopped`, `pop`, `level_pinch`, `landing` or
    /// `no_floor`, as the agent API writes it.
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Bounce(Route::Threshold) => "bounce",
            Outcome::Bounce(Route::Fast) => "bounce_fast",
            Outcome::Caught => "caught",
            Outcome::Stopped => "stopped",
            Outcome::Pop => "pop",
            Outcome::LevelPinch => "level_pinch",
            Outcome::Landing => "landing",
            Outcome::NoFloor => "no_floor",
        }
    }

    pub fn bounces(self) -> bool {
        matches!(self, Outcome::Bounce(_))
    }
}

/// Where the swept axis went through a floor polygon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pierce {
    /// The crossing point, on the polygon.
    pub at: Vec3,
    /// `de`: the crossing point's distance to the polygon's nearest edge.
    pub edge: f32,
    /// `cb`: how far under the polygon's plane the axis's lowest point is.
    pub depth: f32,
    /// The horizontal speed across that edge (units/s).
    pub across: f32,
    /// The nearest edge, its two ends.
    pub line: [Vec3; 2],
    /// The polygon's index in the slice [`predict`] was given.
    pub poly: usize,
}

/// [`predict`]'s answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verdict {
    pub outcome: Outcome,
    /// Ticks from the start to the deciding tick (0: the first).
    pub tick: u32,
    /// The origin at the start of the deciding tick.
    pub row: Vec3,
    /// The deciding tick's speed (units/s), with its gravity.
    pub speed: f32,
    /// The capsule's distance from the floor at the start of an unswept
    /// deciding tick (negative inside it); 0 for a swept one.
    pub gap: f32,
    /// The polygon the axis went through: set for a bounce, a pop and a
    /// level pinch.
    pub pierce: Option<Pierce>,
}

impl Verdict {
    fn new(outcome: Outcome, tick: u32, row: Vec3, speed: f32) -> Verdict {
        Verdict {
            outcome,
            tick,
            row,
            speed,
            gap: 0.0,
            pierce: None,
        }
    }
}

/// A floor polygon with its plane.
struct Floor<'a> {
    index: usize,
    verts: &'a [Vec3],
    n: Vec3,
    d: f32,
}

fn floors(polys: &[Poly]) -> Vec<Floor<'_>> {
    polys
        .iter()
        .enumerate()
        .filter(|(_, p)| p.verts.len() >= 3 && p.normal[2] > FLOOR_NZ)
        .map(|(index, p)| Floor {
            index,
            verts: &p.verts,
            n: p.normal,
            d: dot(p.normal, p.verts[0]),
        })
        .collect()
}

/// The floor polygons of `polys` (normal Z over [`FLOOR_NZ`]) whose
/// bounds come within `r` of `xy`, horizontally: the set to hand
/// [`predict`] for one flight.
pub fn floors_near(polys: &[Poly], xy: [f32; 2], r: f32) -> Vec<Poly> {
    polys
        .iter()
        .filter(|p| p.verts.len() >= 3 && p.normal[2] > FLOOR_NZ)
        .filter(|p| {
            let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
            for v in &p.verts {
                for k in 0..2 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            (0..2).all(|k| xy[k] >= lo[k] - r && xy[k] <= hi[k] + r)
        })
        .cloned()
        .collect()
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn norm(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

/// The point on segment `a b` nearest `p`.
fn foot(p: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    let t = if l2 > 1e-12 {
        (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    add(a, scale(ab, t))
}

/// For `x` on `f`'s plane: whether it is inside the polygon (either
/// winding), its distance to the nearest edge, and that edge.
fn inside(f: &Floor, x: Vec3) -> (bool, f32, [Vec3; 2]) {
    let mut sign = 0.0f32;
    let mut ins = true;
    let mut best = (f32::INFINITY, [f.verts[0], f.verts[0]]);
    for i in 0..f.verts.len() {
        let (a, b) = (f.verts[i], f.verts[(i + 1) % f.verts.len()]);
        let c = dot(cross(sub(b, a), sub(x, a)), f.n);
        if c.abs() > 1e-7 {
            if sign == 0.0 {
                sign = c.signum();
            } else if c.signum() != sign {
                ins = false;
            }
        }
        let d = norm(sub(x, foot(x, a, b)));
        if d < best.0 {
            best = (d, [a, b]);
        }
    }
    (ins, best.0, best.1)
}

/// The capsule's bottom sphere, centred at `c`, against `f`: its gap to
/// the polygon (negative through it) and the centre's height over the
/// plane.
fn sphere_gap(f: &Floor, c: Vec3) -> (f32, f32) {
    let h = dot(f.n, c) - f.d;
    let on = sub(c, scale(f.n, h));
    let (ins, _, [a, b]) = inside(f, on);
    if ins {
        (h - RADIUS, h)
    } else {
        (norm(sub(c, foot(c, a, b))) - RADIUS, h)
    }
}

/// The horizontal speed of `v` across the horizontal line of `[a, b]`;
/// the whole horizontal speed for an edge with no horizontal length.
fn across(v: Vec3, [a, b]: [Vec3; 2]) -> f32 {
    let e = [b[0] - a[0], b[1] - a[1]];
    let l = e[0].hypot(e[1]);
    if l < 1e-6 {
        return v[0].hypot(v[1]);
    }
    ((v[0] * -e[1] + v[1] * e[0]) / l).abs()
}

/// How the swept axis `a..e`, moving `v`, meets the floors: the best of a
/// bounce, a level pinch and a pop, in that order.
fn pierce(floors: &[Floor], a: Vec3, e: Vec3, v: Vec3) -> Option<(Outcome, Pierce)> {
    let rank = |o: Outcome| match o {
        Outcome::Bounce(_) => 0,
        Outcome::LevelPinch => 1,
        _ => 2,
    };
    let mut best: Option<(Outcome, Pierce)> = None;
    for f in floors {
        let (da, db) = (dot(f.n, a) - f.d, dot(f.n, e) - f.d);
        if da < 0.0 || db > 0.0 || da == db {
            continue;
        }
        let x = add(a, scale(sub(e, a), da / (da - db)));
        let (ins, de, line) = inside(f, x);
        if !ins {
            continue;
        }
        let p = Pierce {
            at: x,
            edge: de,
            depth: -db,
            across: across(v, line),
            line,
            poly: f.index,
        };
        let o = if de >= EDGE_SCALE * p.depth {
            Outcome::Pop
        } else if p.across >= DRIFT_MIN {
            Outcome::Bounce(Route::Threshold)
        } else {
            Outcome::LevelPinch
        };
        if best.is_none_or(|(b, _)| rank(o) < rank(b)) {
            best = Some((o, p));
        }
    }
    best
}

/// The least gap from the capsule's bottom sphere at `c` to a floor whose
/// plane `from` is over (the floors the body comes down onto).
fn gap(floors: &[Floor], from: Vec3, c: Vec3) -> f32 {
    floors
        .iter()
        .filter(|f| dot(f.n, from) - f.d > 0.0)
        .map(|f| sphere_gap(f, c).0)
        .fold(f32::INFINITY, f32::min)
}

/// `v` toward `want`, by at most `step`.
fn toward(v: [f32; 2], want: [f32; 2], step: f32) -> [f32; 2] {
    let d = [want[0] - v[0], want[1] - v[1]];
    let n = d[0].hypot(d[1]);
    if n > step {
        [v[0] + d[0] * step / n, v[1] + d[1] * step / n]
    } else {
        want
    }
}

/// What a body at origin `start` moving `velocity` comes to on `polys`
/// (any polygons; the floors among them are used), with `drift` the
/// sideways velocity the air stick holds ([`AIR_ACCEL`] a second toward
/// it; zero for no stick, which brakes).
///
/// Each tick, as the engine steps it: the fall speed gains a tick of
/// gravity and the sideways velocity steps toward `drift`. Moving down:
///
/// - swept (the tick's speed squared over [`SWEEP_SQ`]): the bottom
///   sphere's centre is swept a tick along the motion. Through a floor
///   polygon, the edge rule decides: a bounce, a level pinch or a pop.
///   Not through, but within [`CONTACT`] of a floor: caught.
/// - unswept: the capsule within [`CONTACT`] of a floor where it stands
///   touches it: stopped when more than [`CONTACT`] inside, else a
///   landing.
///
/// Then the body moves a tick. [`Outcome::NoFloor`] after [`MAX_TICKS`],
/// or for a number that is not one. The pair word is not an input: the
/// verdict is what a primed body does.
pub fn predict(start: Vec3, velocity: Vec3, drift: [f32; 2], polys: &[Poly]) -> Verdict {
    if !start
        .iter()
        .chain(&velocity)
        .chain(&drift)
        .all(|v| v.is_finite())
    {
        return Verdict::new(Outcome::NoFloor, 0, start, 0.0);
    }
    let floors = floors(polys);
    let (mut p, mut v) = (start, velocity);
    let mut swept = dot(v, v) > SWEEP_SQ;
    for tick in 0..=MAX_TICKS {
        let h = toward([v[0], v[1]], drift, AIR_ACCEL * TICK);
        let next = [h[0], h[1], v[2] - GRAVITY * TICK];
        let sq = dot(next, next);
        let speed = sq.sqrt();
        let base = add(p, [0.0, 0.0, BASE]);
        if next[2] < 0.0 {
            if sq > SWEEP_SQ {
                let end = add(base, scale(next, TICK));
                if let Some((o, pierce)) = pierce(&floors, base, end, next) {
                    let o = match o {
                        Outcome::Bounce(_) if swept => Outcome::Bounce(Route::Fast),
                        o => o,
                    };
                    return Verdict {
                        pierce: Some(pierce),
                        ..Verdict::new(o, tick, p, speed)
                    };
                }
                if gap(&floors, base, end) < CONTACT {
                    return Verdict::new(Outcome::Caught, tick, p, speed);
                }
            } else {
                let g = gap(&floors, base, base);
                if g < CONTACT {
                    let o = if g < -CONTACT {
                        Outcome::Stopped
                    } else {
                        Outcome::Landing
                    };
                    return Verdict {
                        gap: g,
                        ..Verdict::new(o, tick, p, speed)
                    };
                }
            }
        }
        p = add(p, scale(next, TICK));
        v = next;
        swept = sq > SWEEP_SQ;
    }
    Verdict::new(Outcome::NoFloor, MAX_TICKS, p, v[2].abs())
}

/// The nearest edge of the floor under a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeNear {
    /// The horizontal distance from the point to the edge.
    pub distance: f32,
    /// The edge, its two ends.
    pub line: [Vec3; 2],
    /// Unit, horizontal, square to the edge: the way across it.
    pub across: [f32; 2],
    /// The floor polygon's index in the slice given.
    pub poly: usize,
}

/// The floor polygon under `at` (the highest whose plane is at or under
/// `at`'s Z, over its X and Y) and its edge nearest `at`, horizontally.
/// `None` with no floor under it.
pub fn edge_under(polys: &[Poly], at: Vec3) -> Option<EdgeNear> {
    let xy = [at[0], at[1]];
    floors(polys)
        .into_iter()
        .filter_map(|f| {
            let z = (f.d - f.n[0] * xy[0] - f.n[1] * xy[1]) / f.n[2];
            let (ins, _, _) = inside(&f, [xy[0], xy[1], z]);
            (ins && z <= at[2] + 1e-3).then_some((f, z))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .and_then(|(f, _)| {
            let n = f.verts.len();
            (0..n)
                .map(|i| [f.verts[i], f.verts[(i + 1) % n]])
                .map(|line| (segment_xy(line, xy), line))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(distance, line)| {
                    let e = [line[1][0] - line[0][0], line[1][1] - line[0][1]];
                    let l = e[0].hypot(e[1]).max(1e-6);
                    EdgeNear {
                        distance,
                        line,
                        across: [-e[1] / l, e[0] / l],
                        poly: f.index,
                    }
                })
        })
}

#[cfg(test)]
mod tests;
