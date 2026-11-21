//! The superbounce marks drawn on the map (docs/game/superbounce.md,
//! "Floor marks" and "Lock-on"): small, precise floor marks in the HUD's
//! colour, labels in the HUD's font ([`Look`]).
//!
//! - **The chosen strip** (the navpoint's, while primed): its crease as a
//!   short capped line with a footprint ring and a centre dot, `Land, 3 m`
//!   (the drop) beside it.
//! - **The take-off**: a footprint ring with two feet and three chevrons
//!   toward the landing, `Jump (A)`; where it is comes from the map's
//!   floor ([`takeoff`]).
//! - **Other strips**: the crease line alone; past [`FAR_RANGE`] a small
//!   upright diamond. Crouch spots: the footprint and an approach arrow,
//!   and the same diamond far away.
//! - **The lock-on**, while primed and falling: corner brackets round the
//!   crease that close in as the predicted landing nears it, red and
//!   `Locked` within the slot at the speed it needs ([`lock_box`]). They
//!   ease by the wall clock ([`Ease`]), so they move as smoothly at 120
//!   frames a second as at 30; the rules behind them run per guest tick.
//!
//! The builders are pure: [`ring`], [`cross`], [`arrowhead`],
//! [`crease_mark`], [`diamond`], [`brackets`], [`lock_box`] and [`takeoff`]
//! make geometry, [`figures`] puts a frame's together from the nearest
//! spots ([`Item`]) and the player's [`Scene`], and [`frame`] reads both
//! from the mode and adds the airborne prompts
//! (`crate::superbounce_prompts`). What the scanner and the mode's state
//! give is read in one place, [`contract`].

use std::sync::Mutex;
use std::time::Instant;

use engine::GuestMemory;
use gfx::world_lines::{Fade, Glow, LineStyle, Pulse};
use superbounce::{Kind, Spot, Vec3};

use crate::superbounce_charge::HIGH_SLACK;
use crate::superbounce_font::{self as hud, Look};
use crate::superbounce_mode::{DIM, FADE_FAR, FADE_NEAR, MAX_LINES, NavKind, State};
use crate::superbounce_prompts::{
    self as prompts, Callout, FLAT_FALL, LABEL_RAISE, TAKEOFF_BACK, whole_metres,
};

/// The biped's capsule radius: the footprint ring.
pub const CAPSULE_RADIUS: f32 = 0.175;
/// Floor figures sit this far above the floor.
pub const LIFT: f32 = 0.02;
/// Segments of a full ring.
pub const RING_SEGMENTS: usize = 24;
/// The nearest crouch spot within this distance carries `Crouch`.
pub const CROUCH_LABEL_RANGE: f32 = 3.0;
/// The charge's phase is shown on the nearest crouch spot within this
/// horizontal distance of the player.
pub const PHASE_RANGE: f32 = 1.0;
/// A `likely` crouch spot's strength; a `proven` one's is 1.
pub const LIKELY: f32 = 0.5;
/// Crouch figures' strength while primed: a crouch would lose the charge.
pub const PRIMED_CROUCH: f32 = 0.2;
/// A strip's strength when the player's floor is too low to reach it.
pub const CLIMB: f32 = 0.5;
/// Spots further than this from the player (world units) are a small
/// diamond.
pub const FAR_RANGE: f32 = 10.0;
/// The far diamond's half height per world unit of distance: about the
/// same size on screen at any range.
pub const DIAMOND_SIZE: f32 = 0.012;
/// The far diamond floats this far over the stand point.
pub const DIAMOND_RAISE: f32 = 0.3;
/// The crease mark: half its length along the edge (at most half the
/// edge), and the caps' half length across it.
pub const CREASE_HALF: f32 = 0.26;
pub const CAP_HALF: f32 = 0.06;
/// The centre dot: a cross of arms this long.
pub const DOT: f32 = 0.025;
/// The take-off footprint: its ring, and the two feet in it.
pub const TAKEOFF_RADIUS: f32 = 0.25;
pub const FOOT_RADIUS: f32 = 0.05;
pub const FOOT_APART: f32 = 0.08;
/// The take-off's chevrons toward the landing: the first this far out
/// from its centre, then [`CHEVRON_GAP`] apart, [`CHEVRON_SIZE`] long.
pub const CHEVRON_OUT: f32 = TAKEOFF_RADIUS + 0.1;
pub const CHEVRON_GAP: f32 = 0.12;
pub const CHEVRON_SIZE: f32 = 0.1;
/// Their strengths, nearest the take-off first.
pub const CHEVRON_ALPHA: [f32; 3] = [0.45, 0.7, 0.95];
/// Within this horizontal distance of the take-off, its label pulses.
pub const TAKEOFF_NEAR: f32 = 0.15;
/// The take-off search: the step out from the strip, how far it looks,
/// and how far above the player's feet a floor still counts.
pub const TAKEOFF_STEP: f32 = 0.05;
pub const TAKEOFF_REACH: f32 = 4.0;
pub const TAKEOFF_TOP: f32 = 0.5;
/// A crouch spot's approach arrow: this far out from its ring.
pub const APPROACH_OUT: f32 = 0.15;
pub const APPROACH_SIZE: f32 = 0.12;
/// `Crouch` floats this far above the lip (or [`CROUCH_LABEL_HEIGHT`]
/// above the stand point without one).
pub const CROUCH_LABEL_RAISE: f32 = 0.15;
pub const CROUCH_LABEL_HEIGHT: f32 = 0.8;
/// The world labels' size: the HUD's own (`LabelFace::Hud` at 2).
pub const LABEL_SCALE: f32 = 2.0;
/// The lock-on's box, closed: half its length along the crease and
/// across it.
pub const LOCK_HALF: [f32; 2] = [0.3, 0.2];
/// It opens this much per unit the landing misses the slot by, up to
/// [`LOCK_OPEN_MAX`].
pub const LOCK_OPEN: f32 = 1.5;
pub const LOCK_OPEN_MAX: f32 = 0.9;
/// Missing, its centre moves toward the landing by half the miss, at most
/// this far.
pub const LOCK_SHIFT_MAX: f32 = 0.5;
/// A corner's arms: this fraction of the box's shorter half.
pub const LOCK_ARM: f32 = 0.5;
/// A box that appears starts this much wider and closes in.
pub const LOCK_APPEAR: f32 = 0.6;
/// Seconds the box takes to reach a new tick's size (wall clock).
pub const LOCK_EASE: f32 = 0.12;
/// The lock's red, and the label's (the approved exception to the HUD
/// colour: red is the game's "this will hit").
pub const RED: [f32; 3] = [1.0, 0.275, 0.196];

/// How far through the charge the player is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Near,
    Under,
    Held,
    Primed,
    Lost,
}

/// Where a primed, falling player will land, on the chosen strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    pub at: Vec3,
    /// The point of the crease nearest `at`.
    pub crease: Vec3,
    /// The crease's direction, unit and flat.
    pub along: [f32; 2],
    /// How far `at` is outside the slot (0 inside).
    pub miss: f32,
    /// In the slot and fast enough.
    pub locked: bool,
}

/// A bounce strip as drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strip {
    pub edge: [Vec3; 2],
    pub stand: Vec3,
    pub fall: f32,
}

/// A crouch spot as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Crouch {
    pub stand: Vec3,
    /// Unit, flat: from the stand point toward the standing room.
    pub out: [f32; 2],
    /// The edges the standing capsule hits, at their own height (the
    /// label goes over them).
    pub lip: Vec<[Vec3; 2]>,
    pub proven: bool,
}

/// One spot near the player, nearest first.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Strip(Strip),
    Crouch(Crouch),
}

/// The player, for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    /// The unit's origin, at its feet.
    pub at: Vec3,
    pub airborne: bool,
    /// Moving down.
    pub falling: bool,
    pub primed: bool,
    pub phase: Option<Phase>,
    pub landing: Option<Landing>,
    /// The chosen strip's stand point (the navpoint's strip, primed only).
    pub chosen: Option<Vec3>,
    /// Where to jump from for it.
    pub takeoff: Option<Vec3>,
}

/// What the scanner's spots and the mode's state carry, read in one
/// place (docs/game/superbounce.md, "Crouch spots: the stricter rule",
/// "Charge", "Landings").
pub mod contract {
    use super::*;
    use crate::superbounce_mode as mode;

    pub fn strip(s: &Spot) -> Strip {
        Strip {
            edge: s.edge,
            stand: s.stand,
            fall: s.fall,
        }
    }

    pub fn crouch(s: &Spot) -> Crouch {
        Crouch {
            stand: s.stand,
            out: [s.out[0], s.out[1]],
            lip: s
                .lip
                .edges()
                .iter()
                .copied()
                .filter(|e| *e != [[0.0; 3]; 2])
                .collect(),
            proven: matches!(s.status, superbounce::Status::Proven),
        }
    }

    pub fn phase(s: &State) -> Option<Phase> {
        let c = s.charge.as_ref()?;
        Some(match c.phase {
            mode::ChargePhase::Near => Phase::Near,
            mode::ChargePhase::Under => Phase::Under,
            mode::ChargePhase::Held => Phase::Held,
            mode::ChargePhase::Primed => Phase::Primed,
            mode::ChargePhase::Lost => Phase::Lost,
        })
    }

    pub fn landing(s: &State) -> Option<Landing> {
        let p = s.landing.as_ref()?;
        let [a, b] = p.edge;
        Some(Landing {
            at: p.at,
            crease: p.crease_point(),
            along: flat_unit([b[0] - a[0], b[1] - a[1]]).unwrap_or([1.0, 0.0]),
            miss: p.miss(),
            locked: p.locked(),
        })
    }

    /// The chosen strip: the navpoint's while it is over a strip.
    pub fn chosen(s: &State) -> Option<Vec3> {
        s.nav_target
            .filter(|n| n.kind == NavKind::Strip && s.primed())
            .map(|n| n.at)
    }
}

// ---- geometry ----

fn flat_unit(v: [f32; 2]) -> Option<[f32; 2]> {
    let len = v[0].hypot(v[1]);
    (len > 1e-6 && len.is_finite()).then(|| [v[0] / len, v[1] / len])
}

fn horizontal(a: Vec3, b: Vec3) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn distance3(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn offset(p: Vec3, d: [f32; 2], by: f32) -> Vec3 {
    [p[0] + d[0] * by, p[1] + d[1] * by, p[2]]
}

fn up(p: Vec3, by: f32) -> Vec3 {
    [p[0], p[1], p[2] + by]
}

fn middle([a, b]: [Vec3; 2]) -> Vec3 {
    [
        (a[0] + b[0]) / 2.0,
        (a[1] + b[1]) / 2.0,
        (a[2] + b[2]) / 2.0,
    ]
}

/// A flat ring of `radius` round `centre`, [`LIFT`] above it, as
/// [`RING_SEGMENTS`] edges end to end. `open` keeps every other one: a
/// broken ring.
pub fn ring(centre: Vec3, radius: f32, open: bool) -> Vec<[Vec3; 2]> {
    ring_of(centre, radius, open, RING_SEGMENTS)
}

fn ring_of(centre: Vec3, radius: f32, open: bool, segments: usize) -> Vec<[Vec3; 2]> {
    let point = |i: usize| {
        let a = std::f32::consts::TAU * (i % segments) as f32 / segments as f32;
        [
            centre[0] + radius * a.cos(),
            centre[1] + radius * a.sin(),
            centre[2] + LIFT,
        ]
    };
    (0..segments)
        .filter(|i| !open || i % 2 == 0)
        .map(|i| [point(i), point(i + 1)])
        .collect()
}

/// A flat cross of arms `half` long each way at `centre`, [`LIFT`] up,
/// on the diagonals: the centre dot.
pub fn cross(centre: Vec3, half: f32) -> Vec<[Vec3; 2]> {
    let c = up(centre, LIFT);
    let d = half * std::f32::consts::FRAC_1_SQRT_2;
    vec![
        [[c[0] - d, c[1] - d, c[2]], [c[0] + d, c[1] + d, c[2]]],
        [[c[0] - d, c[1] + d, c[2]], [c[0] + d, c[1] - d, c[2]]],
    ]
}

/// An arrowhead lying flat at `at`, [`LIFT`] up, pointing along `dir`:
/// two edges, wing to tip, `size` from the wings to the tip. None for no
/// direction.
pub fn arrowhead(at: Vec3, dir: [f32; 2], size: f32) -> Vec<[Vec3; 2]> {
    let Some(u) = flat_unit(dir) else {
        return Vec::new();
    };
    let c = up(at, LIFT);
    let n = [-u[1], u[0]];
    let tip = offset(c, u, size * 0.5);
    let back = offset(c, u, -size * 0.5);
    vec![
        [offset(back, n, size * 0.7), tip],
        [offset(back, n, -size * 0.7), tip],
    ]
}

/// The crease's direction (unit, flat), +X for an edge with no length.
pub fn along_of([a, b]: [Vec3; 2]) -> [f32; 2] {
    flat_unit([b[0] - a[0], b[1] - a[1]]).unwrap_or([1.0, 0.0])
}

/// How long the crease mark is each way: [`CREASE_HALF`], at most half
/// the edge (an edge with no length gets the full mark).
pub fn crease_half([a, b]: [Vec3; 2]) -> f32 {
    let half = horizontal(a, b) / 2.0;
    if half > 1e-4 {
        half.min(CREASE_HALF)
    } else {
        CREASE_HALF
    }
}

/// The crease as a short line through `centre` along `along`, `half`
/// each way, with a cap across each end [`CAP_HALF`] each way; [`LIFT`]
/// up. Three edges: the line, then the caps.
pub fn crease_mark(centre: Vec3, along: [f32; 2], half: f32) -> Vec<[Vec3; 2]> {
    let c = up(centre, LIFT);
    let n = [-along[1], along[0]];
    let (a, b) = (offset(c, along, -half), offset(c, along, half));
    vec![
        [a, b],
        [offset(a, n, -CAP_HALF), offset(a, n, CAP_HALF)],
        [offset(b, n, -CAP_HALF), offset(b, n, CAP_HALF)],
    ]
}

/// Whether a spot at `spot` is far from a player at `at`: past
/// [`FAR_RANGE`], drawn as a diamond.
pub fn far(at: Vec3, spot: Vec3) -> bool {
    distance3(at, spot) > FAR_RANGE
}

/// The far diamond's half height for a spot `distance` away: a constant
/// size on screen ([`DIAMOND_SIZE`] per unit).
pub fn diamond_half(distance: f32) -> f32 {
    DIAMOND_SIZE * distance.max(0.0)
}

/// An upright diamond over `spot` facing a player at `at`, `half` tall
/// each way and 0.7 of that wide, its bottom [`DIAMOND_RAISE`] over the
/// spot: four edges round it.
pub fn diamond(spot: Vec3, at: Vec3, half: f32) -> Vec<[Vec3; 2]> {
    let toward = flat_unit([at[0] - spot[0], at[1] - spot[1]]).unwrap_or([1.0, 0.0]);
    let side = [-toward[1], toward[0]];
    let c = up(spot, DIAMOND_RAISE + half);
    let (top, bottom) = (up(c, half), up(c, -half));
    let (left, right) = (offset(c, side, -0.7 * half), offset(c, side, 0.7 * half));
    vec![[top, right], [right, bottom], [bottom, left], [left, top]]
}

/// The lock-on's box: round `centre` on the floor, `half` along the
/// crease (`along`) and across it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LockBox {
    pub centre: Vec3,
    pub along: [f32; 2],
    pub half: [f32; 2],
    pub locked: bool,
}

impl LockBox {
    /// `by` wider each way.
    pub fn opened(&self, by: f32) -> LockBox {
        LockBox {
            half: [self.half[0] + by, self.half[1] + by],
            ..*self
        }
    }

    /// `t` (0..1) of the way from this box to `to`; the direction and the
    /// lock are `to`'s.
    pub fn lerp(&self, to: &LockBox, t: f32) -> LockBox {
        if t >= 1.0 {
            return *to;
        }
        let mix = |a: f32, b: f32| a + (b - a) * t;
        LockBox {
            centre: std::array::from_fn(|i| mix(self.centre[i], to.centre[i])),
            along: to.along,
            half: [mix(self.half[0], to.half[0]), mix(self.half[1], to.half[1])],
            locked: to.locked,
        }
    }
}

/// The lock-on's box for `l`: closed ([`LOCK_HALF`]) round the crease
/// point within the slot, opened by [`LOCK_OPEN`] per unit of miss up to
/// [`LOCK_OPEN_MAX`] and moved toward the landing by half the miss (at
/// most [`LOCK_SHIFT_MAX`]); red when `l` is locked.
pub fn lock_box(l: &Landing) -> LockBox {
    let miss = if l.miss.is_finite() {
        l.miss.max(0.0)
    } else {
        LOCK_OPEN_MAX / LOCK_OPEN
    };
    let open = (miss * LOCK_OPEN).min(LOCK_OPEN_MAX);
    let toward = [l.at[0] - l.crease[0], l.at[1] - l.crease[1]];
    let shift = match flat_unit(toward) {
        Some(d) if miss > 0.0 => {
            let by = (toward[0].hypot(toward[1]) * 0.5).min(LOCK_SHIFT_MAX);
            [d[0] * by, d[1] * by]
        }
        _ => [0.0; 2],
    };
    LockBox {
        centre: [l.crease[0] + shift[0], l.crease[1] + shift[1], l.crease[2]],
        along: l.along,
        half: [LOCK_HALF[0] + open, LOCK_HALF[1] + open],
        locked: l.locked,
    }
}

/// The box's four corner brackets, flat, [`LIFT`] up: at each corner an
/// arm along each side toward the middle, [`LOCK_ARM`] of the shorter
/// half long. Eight edges.
pub fn brackets(b: &LockBox) -> Vec<[Vec3; 2]> {
    let c = up(b.centre, LIFT);
    let u = flat_unit(b.along).unwrap_or([1.0, 0.0]);
    let n = [-u[1], u[0]];
    let arm = LOCK_ARM * b.half[0].min(b.half[1]);
    let mut out = Vec::with_capacity(8);
    for su in [-1.0f32, 1.0] {
        for sn in [-1.0f32, 1.0] {
            let corner = offset(offset(c, u, su * b.half[0]), n, sn * b.half[1]);
            out.push([corner, offset(corner, u, -su * arm)]);
            out.push([corner, offset(corner, n, -sn * arm)]);
        }
    }
    out
}

/// The lock-on easing from one box to the next by the wall clock: each
/// guest tick gives a new target and the box moves to it over
/// [`LOCK_EASE`] seconds, so it closes in smoothly at any frame rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ease {
    pub from: LockBox,
    pub to: LockBox,
    pub start: Instant,
}

impl Ease {
    /// The box shown at `now`: smoothstep from `from` to `to`.
    pub fn at(&self, now: Instant) -> LockBox {
        let t = now.saturating_duration_since(self.start).as_secs_f32() / LOCK_EASE;
        // Within a microsecond of the end is the end.
        let t = if t >= 1.0 - 1e-5 { 1.0 } else { t.max(0.0) };
        self.from.lerp(&self.to, t * t * (3.0 - 2.0 * t))
    }

    /// The ease toward `to` from `now`: from what `prev` shows then, or
    /// for a box that appears, from `to` [`LOCK_APPEAR`] wider.
    pub fn next(prev: Option<&Ease>, to: LockBox, now: Instant) -> Ease {
        Ease {
            from: prev.map_or(to.opened(LOCK_APPEAR), |p| p.at(now)),
            to,
            start: now,
        }
    }
}

/// Where to jump from for a strip at `stand` with `fall` and `drift`, for
/// a player whose feet are at `player`, with `floor(xy, top)` the height
/// of the highest floor under `xy` at or below `top`
/// (`superbounce_capsule::Room::floor_at`).
///
/// - A strip a standing jump reaches (`fall` at most [`FLAT_FALL`]): the
///   prompts' take-off, [`TAKEOFF_BACK`] back from the stand point
///   against the drift.
/// - Else: out from the strip toward the player (against the drift when
///   the player is over it), in steps of [`TAKEOFF_STEP`] up to
///   [`TAKEOFF_REACH`], the first floor at least `fall` above the strip
///   (less [`HIGH_SLACK`]) and no more than [`TAKEOFF_TOP`] above the
///   player's feet: the lip of the ledge to come from. The take-off is a
///   footprint back from that lip, on its floor.
///
/// `None` when no floor is found that way.
pub fn takeoff(
    stand: Vec3,
    fall: f32,
    drift: [f32; 2],
    player: Vec3,
    floor: impl Fn([f32; 2], f32) -> Option<f32>,
) -> Option<Vec3> {
    if !stand.iter().chain(&player).all(|v| v.is_finite()) {
        return None;
    }
    let back = flat_unit([-drift[0], -drift[1]]);
    if !(fall > FLAT_FALL) {
        return Some(offset(stand, back.unwrap_or([0.0; 2]), TAKEOFF_BACK));
    }
    let d = flat_unit([player[0] - stand[0], player[1] - stand[1]]).or(back)?;
    let need = stand[2] + fall - HIGH_SLACK;
    let top = player[2] + TAKEOFF_TOP;
    let steps = (TAKEOFF_REACH / TAKEOFF_STEP).round() as usize;
    (1..=steps).find_map(|i| {
        let s = i as f32 * TAKEOFF_STEP;
        let lip = floor([stand[0] + d[0] * s, stand[1] + d[1] * s], top).filter(|z| *z >= need)?;
        let s = s + CAPSULE_RADIUS;
        let at = [stand[0] + d[0] * s, stand[1] + d[1] * s];
        let z = floor(at, top).filter(|z| *z >= need).unwrap_or(lip);
        Some([at[0], at[1], z])
    })
}

/// Whether feet at `feet` are high enough to fall onto `strip`: it needs
/// no fall, or they are at least its fall above its stand point (less
/// [`HIGH_SLACK`]), as the navpoint's choice has it.
pub fn high_enough(feet: f32, strip: &Strip) -> bool {
    strip.fall <= FLAT_FALL || feet - strip.stand[2] >= strip.fall - HIGH_SLACK
}

/// A strip's strength: dimmed unless primed, halved when the player must
/// climb to fall onto it.
pub fn strip_strength(primed: bool, high_enough: bool) -> f32 {
    let p = if primed { 1.0 } else { DIM };
    if high_enough { p } else { p * CLIMB }
}

/// A crouch figure's strength: half for a `likely` spot, a fifth of that
/// while primed.
pub fn crouch_strength(proven: bool, primed: bool) -> f32 {
    let k = if proven { 1.0 } else { LIKELY };
    if primed { k * PRIMED_CROUCH } else { k }
}

// ---- figures ----

fn style(look: &Look, width: f32, alpha: f32) -> LineStyle {
    LineStyle {
        width,
        alpha,
        fade: Some(Fade {
            start: FADE_NEAR,
            end: FADE_FAR,
        }),
        label_scale: LABEL_SCALE,
        label_centered: true,
        label_face: look.face,
        ..LineStyle::default()
    }
}

fn callout(edges: Vec<[Vec3; 2]>, anchor: Vec3, color: [f32; 3], style: LineStyle) -> Callout {
    Callout {
        edges,
        anchor,
        color,
        label: None,
        style,
    }
}

fn far_figure(out: &mut Vec<Callout>, spot: Vec3, at: Vec3, alpha: f32, look: &Look) {
    let half = diamond_half(distance3(at, spot));
    out.push(callout(
        diamond(spot, at, half),
        spot,
        look.colour,
        style(look, 1.5, alpha),
    ));
}

/// The chosen strip: the crease, the footprint ring and the dot; `Land, 3
/// m` beside it on the ground. Red while the lock holds.
fn landing_figure(out: &mut Vec<Callout>, s: &Strip, scene: &Scene, look: &Look) {
    let locked = scene.landing.is_some_and(|l| l.locked);
    let colour = if locked { RED } else { look.colour };
    let mut line = style(look, 3.0, 1.0);
    line.glow = Some(Glow {
        width: 3.0,
        alpha: 0.35,
    });
    let mut crease = callout(
        crease_mark(s.stand, along_of(s.edge), crease_half(s.edge)),
        up(s.stand, LABEL_RAISE),
        colour,
        line,
    );
    // In the air the lock speaks; on the ground the mark says where and
    // from how high.
    if scene.landing.is_none() {
        crease.label = Some(if s.fall > FLAT_FALL {
            format!("Land, {} m", whole_metres(s.fall))
        } else {
            "Land".into()
        });
    }
    out.push(crease);
    out.push(callout(
        ring(s.stand, CAPSULE_RADIUS, false),
        s.stand,
        colour,
        style(look, 2.0, 1.0),
    ));
    out.push(callout(
        cross(s.stand, DOT),
        s.stand,
        colour,
        style(look, 3.0, 1.0),
    ));
}

fn strip_figure(out: &mut Vec<Callout>, s: &Strip, scene: &Scene, look: &Look) {
    if scene.chosen == Some(s.stand) {
        return landing_figure(out, s, scene, look);
    }
    let alpha = strip_strength(scene.primed, high_enough(scene.at[2], s));
    if far(scene.at, s.stand) {
        return far_figure(out, s.stand, scene.at, alpha, look);
    }
    out.push(callout(
        crease_mark(s.stand, along_of(s.edge), crease_half(s.edge)),
        s.stand,
        look.colour,
        style(look, 2.0, alpha),
    ));
}

/// The take-off: a footprint ring with two feet, three chevrons toward the
/// landing at `land`, and `Jump (A)`, pulsing once the player is on it.
fn takeoff_figure(out: &mut Vec<Callout>, at: Vec3, land: Vec3, scene: &Scene, look: &Look) {
    let toward = flat_unit([land[0] - at[0], land[1] - at[1]]);
    let mut ring_style = style(look, 2.5, 1.0);
    ring_style.pulse = (horizontal(scene.at, at) <= TAKEOFF_NEAR).then_some(Pulse {
        period: 0.4,
        depth: 0.6,
    });
    let mut r = callout(
        ring(at, TAKEOFF_RADIUS, false),
        up(at, LABEL_RAISE),
        look.colour,
        ring_style,
    );
    r.label = Some(format!("Jump {}", look.a_button()));
    out.push(r);
    // The feet, side by side across the way to go.
    let across = toward.map_or([0.0, 1.0], |u| [-u[1], u[0]]);
    let feet = [-1.0f32, 1.0]
        .into_iter()
        .flat_map(|s| ring_of(offset(at, across, s * FOOT_APART), FOOT_RADIUS, false, 8))
        .collect();
    out.push(callout(feet, at, look.colour, style(look, 1.5, 0.9)));
    // Chevrons only where the landing is clear of the ring.
    let Some(u) = toward else {
        return;
    };
    let room = horizontal(at, land) - CAPSULE_RADIUS;
    for (i, alpha) in CHEVRON_ALPHA.into_iter().enumerate() {
        let d = CHEVRON_OUT + CHEVRON_GAP * i as f32;
        if d + CHEVRON_SIZE > room {
            break;
        }
        out.push(callout(
            arrowhead(offset(at, u, d), u, CHEVRON_SIZE),
            at,
            look.colour,
            style(look, 2.2, alpha),
        ));
    }
}

fn crouch_label_at(c: &Crouch) -> Vec3 {
    let lip = c
        .lip
        .iter()
        .flat_map(|e| e.iter().map(|p| p[2]))
        .fold(f32::NEG_INFINITY, f32::max);
    if lip.is_finite() {
        [c.stand[0], c.stand[1], lip + CROUCH_LABEL_RAISE]
    } else {
        up(c.stand, CROUCH_LABEL_HEIGHT)
    }
}

/// A crouch spot: its footprint and an approach arrow outside it,
/// pointing in until the button is up, then out.
fn crouch_figure(
    out: &mut Vec<Callout>,
    c: &Crouch,
    phase: Option<Phase>,
    label: bool,
    scene: &Scene,
    look: &Look,
) {
    let alpha = crouch_strength(c.proven, scene.primed);
    if far(scene.at, c.stand) {
        return far_figure(out, c.stand, scene.at, alpha, look);
    }
    let under = matches!(phase, Some(Phase::Under | Phase::Held));
    let mut foot = style(look, if under { 3.0 } else { 1.5 }, alpha);
    foot.pulse = under.then_some(Pulse {
        period: 0.8,
        depth: 0.5,
    });
    let mut ring_callout = callout(
        ring(c.stand, CAPSULE_RADIUS, false),
        crouch_label_at(c),
        look.colour,
        foot,
    );
    ring_callout.label = label.then(|| "Crouch".into());
    out.push(ring_callout);
    if let Some(o) = flat_unit(c.out) {
        let leaving = matches!(phase, Some(Phase::Held | Phase::Primed));
        let dir = if leaving { o } else { [-o[0], -o[1]] };
        out.push(callout(
            arrowhead(
                offset(c.stand, o, CAPSULE_RADIUS + APPROACH_OUT),
                dir,
                APPROACH_SIZE,
            ),
            c.stand,
            look.colour,
            style(look, 2.0, alpha),
        ));
    }
}

/// The marks for one frame. `items` are the spots to draw, nearest first:
/// the chosen strip its landing mark, other strips their crease line (a
/// diamond past [`FAR_RANGE`]), crouch spots their footprint and approach
/// arrow (a diamond far away), the nearest within
/// [`CROUCH_LABEL_RANGE`] the word `Crouch`. The charge's phase shows on
/// the nearest crouch spot within [`PHASE_RANGE`]. Primed on the ground,
/// the take-off for the chosen strip. The lock-on is the [`Ease`]'s,
/// drawn per present ([`lock_callouts`]).
pub fn figures(items: &[Item], scene: &Scene, look: &Look) -> Vec<Callout> {
    let nearest_crouch = |range: f32, d: fn(Vec3, Vec3) -> f32| {
        items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| match it {
                Item::Crouch(c) => Some((i, d(scene.at, c.stand))),
                Item::Strip(_) => None,
            })
            .filter(|(_, d)| *d <= range)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let phase_at = nearest_crouch(PHASE_RANGE, horizontal);
    let label_at = nearest_crouch(CROUCH_LABEL_RANGE, distance3);
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Strip(s) => strip_figure(&mut out, s, scene, look),
            Item::Crouch(c) => {
                let phase = if phase_at == Some(i) {
                    scene.phase
                } else {
                    None
                };
                crouch_figure(&mut out, c, phase, label_at == Some(i), scene, look);
            }
        }
    }
    if scene.primed
        && !scene.airborne
        && let (Some(at), Some(land)) = (scene.takeoff, scene.chosen)
    {
        takeoff_figure(&mut out, at, land, scene, look);
    }
    out
}

/// The `max` spots nearest `at` (by their edges' middle), nearest first.
pub fn nearest(spots: &[Spot], at: Vec3, max: usize) -> Vec<&Spot> {
    let mut near: Vec<(f32, &Spot)> = spots
        .iter()
        .map(|s| (distance3(middle(s.edge), at), s))
        .collect();
    let by_distance = |a: &(f32, &Spot), b: &(f32, &Spot)| a.0.total_cmp(&b.0);
    if near.len() > max {
        near.select_nth_unstable_by(max, by_distance);
        near.truncate(max);
    }
    near.sort_by(by_distance);
    near.into_iter().map(|(_, s)| s).collect()
}

/// Everything the mode draws on the map this frame but the lock-on: the
/// marks for the [`MAX_LINES`] nearest spots (and the chosen strip
/// whatever its distance) and, primed in the air with the fall assist on,
/// its prompts. Nothing without a position.
pub fn frame(guest: &dyn GuestMemory, s: &State, spots: &[Spot]) -> Vec<Callout> {
    use crate::superbounce_assist as assist;
    let Some(at) = s.position else {
        return Vec::new();
    };
    let look = hud::look(guest);
    let primed = s.primed();
    let chosen = contract::chosen(s);
    let mut near = nearest(spots, at, MAX_LINES);
    if let Some(c) = chosen
        && !near
            .iter()
            .any(|sp| sp.kind == Kind::Bounce && sp.stand == c)
        && let Some(sp) = spots
            .iter()
            .find(|sp| sp.kind == Kind::Bounce && sp.stand == c)
    {
        near.push(sp);
    }
    let items: Vec<Item> = near
        .into_iter()
        .map(|sp| match sp.kind {
            Kind::Bounce => Item::Strip(contract::strip(sp)),
            Kind::Crouch => Item::Crouch(contract::crouch(sp)),
        })
        .collect();
    let falling = s.vz.is_some_and(|v| v < 0.0);
    let scene = Scene {
        at,
        airborne: s.airborne,
        falling,
        primed,
        phase: contract::phase(s),
        landing: contract::landing(s).filter(|_| primed && s.airborne && falling),
        chosen,
        takeoff: s.takeoff.filter(|_| primed),
    };
    let mut out = figures(&items, &scene, &look);
    // In the air, the assist's prompts (with the assist on: `Hold X`,
    // `Guiding`); on the ground the floor marks say where to jump and land.
    let body = (primed && s.airborne)
        .then(|| assist::read_body(guest, assist::PORT))
        .flatten();
    if let Some(body) = &body {
        let on = assist::opted_in();
        let target = Some(body)
            .filter(|_| on)
            .and_then(|b| assist::pick(spots, b));
        out.extend(prompts::callouts(
            spots,
            body,
            true,
            target.as_ref().map(|(sp, reach)| (*sp, reach)),
            assist::assisting(),
            on,
            &look,
        ));
    }
    out
}

/// The lock-on's target box this tick: primed, airborne and falling onto
/// a predicted landing, else `None`.
pub fn lock_target(s: &State) -> Option<LockBox> {
    let falling = s.vz.is_some_and(|v| v < 0.0);
    if !(s.primed() && s.airborne && falling) {
        return None;
    }
    contract::landing(s).map(|l| lock_box(&l))
}

/// The lock-on's ease, kept between captures: each capture eases from what
/// was shown at that moment.
static EASE: Mutex<Option<Ease>> = Mutex::new(None);

/// Eases toward `target` from `now` (`None` clears it), keeping the ease
/// for the next capture.
pub fn ease_lock(target: Option<LockBox>, now: Instant) -> Option<Ease> {
    let mut shown = EASE.lock().unwrap_or_else(|e| e.into_inner());
    let next = target.map(|t| Ease::next(shown.as_ref(), t, now));
    *shown = next;
    next
}

/// What a captured frame keeps of the lock-on: its ease and the look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LockMarks {
    pub ease: Ease,
    pub look: Look,
}

impl LockMarks {
    /// The lock-on for a capture of `s` at `now`.
    pub fn capture(guest: &dyn GuestMemory, s: &State, now: Instant) -> Option<LockMarks> {
        let ease = ease_lock(lock_target(s), now)?;
        Some(LockMarks {
            ease,
            look: hud::look(guest),
        })
    }

    /// Its lines at `now` (a present's time).
    pub fn callouts(&self, now: Instant) -> Vec<Callout> {
        lock_callouts(&self.ease.at(now), &self.look)
    }
}

/// The lock-on's lines for box `b`: the brackets, in the HUD colour, and
/// red with `Locked` over them while the lock holds.
pub fn lock_callouts(b: &LockBox, look: &Look) -> Vec<Callout> {
    let colour = if b.locked { RED } else { look.colour };
    let mut s = style(look, 3.0, 1.0);
    s.glow = Some(Glow {
        width: 3.0,
        alpha: 0.4,
    });
    let mut c = callout(brackets(b), up(b.centre, LABEL_RAISE), colour, s);
    c.label = b.locked.then(|| "Locked".into());
    vec![c]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::world_lines::LabelFace;
    use std::time::Duration;

    const LOOK: Look = Look {
        colour: [0.3, 0.6, 0.9],
        face: LabelFace::Hud,
        glyphs: true,
    };

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn scene() -> Scene {
        Scene {
            at: [0.0; 3],
            airborne: false,
            falling: false,
            primed: false,
            phase: None,
            landing: None,
            chosen: None,
            takeoff: None,
        }
    }

    fn strip(x: f32, fall: f32) -> Strip {
        Strip {
            edge: [[x, -0.5, 0.0], [x, 0.5, 0.0]],
            stand: [x, 0.0, 0.0],
            fall,
        }
    }

    fn crouch(x: f32, proven: bool) -> Crouch {
        Crouch {
            stand: [x, 0.0, 0.0],
            out: [1.0, 0.0],
            lip: vec![[[x - 0.2, 0.2, 0.6], [x + 0.2, 0.2, 0.7]]],
            proven,
        }
    }

    fn landing(at: Vec3, miss: f32, locked: bool) -> Landing {
        Landing {
            at,
            crease: [0.0, at[1].clamp(-0.5, 0.5), 0.0],
            along: [0.0, 1.0],
            miss,
            locked,
        }
    }

    #[test]
    fn a_ring_is_closed_at_its_radius_just_above_the_floor() {
        let c = [1.0, 2.0, 3.0];
        let r = ring(c, CAPSULE_RADIUS, false);
        assert_eq!(r.len(), RING_SEGMENTS);
        for [a, b] in &r {
            for p in [a, b] {
                assert!(close((p[0] - c[0]).hypot(p[1] - c[1]), CAPSULE_RADIUS));
                assert!(close(p[2], c[2] + LIFT));
            }
        }
        for w in r.windows(2) {
            assert_eq!(w[0][1], w[1][0]);
        }
        let (first, last) = (r[0][0], r[RING_SEGMENTS - 1][1]);
        assert!(close(first[0], last[0]) && close(first[1], last[1]));
        let open = ring(c, CAPSULE_RADIUS, true);
        assert_eq!(open.len(), RING_SEGMENTS / 2);
        assert_ne!(open[0][1], open[1][0]);
    }

    #[test]
    fn a_cross_has_two_arms_through_its_centre() {
        let x = cross([0.0, 0.0, 1.0], 0.2);
        assert_eq!(x.len(), 2);
        for [a, b] in &x {
            assert!(close((a[0] + b[0]) / 2.0, 0.0) && close((a[1] + b[1]) / 2.0, 0.0));
            assert!(close(distance3(*a, *b), 0.4));
            assert!(close(a[2], 1.0 + LIFT));
        }
    }

    #[test]
    fn an_arrowhead_points_along_its_direction() {
        let a = arrowhead([0.0; 3], [-1.0, 0.0], 0.2);
        assert_eq!(a.len(), 2);
        let tip = a[0][1];
        assert_eq!(tip, a[1][1]);
        assert!(close(tip[0], -0.1));
        assert!(a[0][0][0] > tip[0] && a[0][0][1] * a[1][0][1] < 0.0);
        assert!(arrowhead([0.0; 3], [0.0, 0.0], 0.2).is_empty());
    }

    /// The crease mark is short and capped: at most [`CREASE_HALF`] each
    /// way along the edge, at most half the edge, caps across its ends.
    #[test]
    fn the_crease_is_a_short_capped_line() {
        let s = strip(2.0, 1.0);
        assert!(close(crease_half(s.edge), 0.5f32.min(CREASE_HALF)));
        assert!(close(crease_half([[0.0; 3], [0.2, 0.0, 0.0]]), 0.1));
        assert_eq!(crease_half([[0.0; 3]; 2]), CREASE_HALF);
        let m = crease_mark(s.stand, along_of(s.edge), CREASE_HALF);
        assert_eq!(m.len(), 3);
        let [a, b] = m[0];
        assert!(close(a[1], -CREASE_HALF) && close(b[1], CREASE_HALF));
        assert!(close(a[0], 2.0) && close(a[2], LIFT));
        for [p, q] in &m[1..] {
            assert!(close(p[1], q[1]), "a cap runs across");
            assert!(close((p[0] - q[0]).abs(), 2.0 * CAP_HALF));
        }
    }

    /// A spot past [`FAR_RANGE`] is a diamond whose size grows with its
    /// distance, so it stays small on screen.
    #[test]
    fn far_spots_shrink_to_a_small_diamond() {
        assert!(!far([0.0; 3], [FAR_RANGE, 0.0, 0.0]));
        assert!(far([0.0; 3], [FAR_RANGE + 0.1, 0.0, 0.0]));
        assert!(close(diamond_half(20.0), 2.0 * diamond_half(10.0)));
        let d = diamond([20.0, 0.0, 0.0], [0.0; 3], 0.24);
        assert_eq!(d.len(), 4);
        // Upright, facing the player: it spans Z and Y, not X.
        let xs: Vec<f32> = d.iter().flatten().map(|p| p[0]).collect();
        assert!(xs.iter().all(|x| close(*x, 20.0)));
        let top = d[0][0];
        assert!(close(top[2], DIAMOND_RAISE + 0.48));
        let far_strip = [Item::Strip(strip(20.0, 1.0))];
        let out = figures(&far_strip, &scene(), &LOOK);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].edges.len(), 4);
        assert!(out[0].label.is_none());
        let far_crouch = [Item::Crouch(crouch(-15.0, false))];
        let out = figures(&far_crouch, &scene(), &LOOK);
        assert_eq!((out.len(), out[0].edges.len()), (1, 4));
        assert_eq!(out[0].style.alpha, LIKELY);
    }

    #[test]
    fn a_strip_is_bright_when_primed_and_the_floor_is_high_enough() {
        let s = strip(0.0, 1.0);
        assert!(high_enough(1.0, &s) && high_enough(0.96, &s) && !high_enough(0.9, &s));
        assert!(high_enough(-5.0, &strip(0.0, 0.0)));
        assert_eq!(strip_strength(true, true), 1.0);
        assert_eq!(strip_strength(true, false), CLIMB);
        assert_eq!(strip_strength(false, true), DIM);
        assert_eq!(strip_strength(false, false), DIM * CLIMB);
        // A near strip that is not chosen: the crease line alone.
        let s = Scene {
            primed: true,
            at: [0.0, 0.0, 0.5],
            ..scene()
        };
        let out = figures(&[Item::Strip(strip(2.0, 1.0))], &s, &LOOK);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].edges.len(), 3);
        assert_eq!(out[0].style.alpha, CLIMB);
        assert!(out[0].label.is_none());
    }

    #[test]
    fn a_likely_crouch_spot_is_half_strength_and_all_drop_to_a_fifth_while_primed() {
        assert_eq!(crouch_strength(true, false), 1.0);
        assert_eq!(crouch_strength(false, false), LIKELY);
        assert_eq!(crouch_strength(true, true), PRIMED_CROUCH);
        assert_eq!(crouch_strength(false, true), LIKELY * PRIMED_CROUCH);
        let items = [Item::Crouch(crouch(0.5, false))];
        let primed = Scene {
            primed: true,
            ..scene()
        };
        let out = figures(&items, &primed, &LOOK);
        assert!(out.iter().all(|c| c.style.alpha == LIKELY * PRIMED_CROUCH));
    }

    #[test]
    fn a_crouch_spot_is_its_footprint_and_an_approach_arrow_labelled_when_nearest() {
        let items: Vec<Item> = (0..4)
            .map(|i| Item::Crouch(crouch(1.0 + i as f32, i % 2 == 0)))
            .collect();
        let out = figures(&items, &scene(), &LOOK);
        // Ring and arrow each.
        assert_eq!(out.len(), 4 * 2);
        let labels: Vec<_> = out.iter().filter_map(|c| c.label.as_deref()).collect();
        assert_eq!(labels, ["Crouch"]);
        let first = &out[0];
        assert!(close(first.anchor[0], 1.0) && close(first.anchor[2], 0.7 + CROUCH_LABEL_RAISE));
        assert_eq!(first.color, LOOK.colour);
        assert_eq!(out[2].style.alpha, LIKELY);
        // The arrow points in (-X, `out` is +X) until the button is up.
        let arrow = |phase| {
            let s = Scene { phase, ..scene() };
            let out = figures(&[Item::Crouch(crouch(0.5, true))], &s, &LOOK);
            let [wing, tip] = out[1].edges[0];
            (tip[0] - wing[0]).signum()
        };
        assert_eq!(arrow(None), -1.0);
        assert_eq!(arrow(Some(Phase::Under)), -1.0);
        assert_eq!(arrow(Some(Phase::Held)), 1.0);
        // Under the lip the footprint is bold and pulses.
        let under = Scene {
            phase: Some(Phase::Under),
            ..scene()
        };
        let out = figures(&[Item::Crouch(crouch(0.5, true))], &under, &LOOK);
        assert!(out[0].style.pulse.is_some() && out[0].style.width > 1.5);
        // A spot out of the phase's range does not take it.
        let out = figures(&[Item::Crouch(crouch(2.0, true))], &under, &LOOK);
        assert!(out[0].style.pulse.is_none());
    }

    /// The chosen strip: the crease, its footprint ring and dot, `Land, 3
    /// m` with the drop; the take-off with its ring, feet, chevrons toward
    /// the landing and `Jump (A)`.
    #[test]
    fn the_chosen_strip_says_land_and_its_takeoff_says_jump() {
        let chosen = strip(3.0, 1.0);
        let s = Scene {
            primed: true,
            at: [0.0, 0.0, 1.0],
            chosen: Some(chosen.stand),
            takeoff: Some([1.0, 0.0, 1.0]),
            ..scene()
        };
        let out = figures(&[Item::Strip(chosen)], &s, &LOOK);
        let labels: Vec<_> = out.iter().filter_map(|c| c.label.as_deref()).collect();
        assert_eq!(labels, ["Land, 3 m", "Jump \u{e100}"]);
        // Crease, ring, dot; take-off ring, feet, three chevrons.
        assert_eq!(out.len(), 3 + 2 + 3);
        assert_eq!(out[0].edges.len(), 3);
        assert_eq!(out[1].edges.len(), RING_SEGMENTS);
        assert_eq!(out[2].edges.len(), 2);
        assert!(out.iter().all(|c| c.color == LOOK.colour));
        // The chevrons point at the landing (+X) and brighten toward it.
        for c in &out[5..] {
            let [wing, tip] = c.edges[0];
            assert!(tip[0] > wing[0]);
        }
        assert!(out[5].style.alpha < out[7].style.alpha);
        // Not on the take-off: still; on it, the label pulses.
        assert!(out[3].style.pulse.is_none());
        let on = Scene {
            at: [1.05, 0.0, 1.0],
            ..s
        };
        assert!(
            figures(&[Item::Strip(chosen)], &on, &LOOK)[3]
                .style
                .pulse
                .is_some()
        );
        // A standing-jump strip says just `Land`, and its take-off is too
        // close for chevrons.
        let flat = strip(3.0, 0.0);
        let s = Scene {
            takeoff: Some([2.8, 0.0, 0.0]),
            ..s
        };
        let out = figures(&[Item::Strip(flat)], &s, &LOOK);
        let labels: Vec<_> = out.iter().filter_map(|c| c.label.as_deref()).collect();
        assert_eq!(labels, ["Land", "Jump \u{e100}"]);
        assert_eq!(out.len(), 3 + 2);
        // In the air there is no take-off and the lock speaks, not `Land`.
        let air = Scene {
            airborne: true,
            falling: true,
            landing: Some(landing([3.0, 0.0, 0.0], 0.0, false)),
            ..s
        };
        let out = figures(&[Item::Strip(chosen)], &air, &LOOK);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|c| c.label.is_none()));
        // Locked: the mark is red.
        let locked = Scene {
            landing: Some(landing([3.0, 0.0, 0.0], 0.0, true)),
            ..air
        };
        assert!(
            figures(&[Item::Strip(chosen)], &locked, &LOOK)
                .iter()
                .all(|c| c.color == RED)
        );
    }

    /// The box closes round the crease as the miss goes to 0, opens with
    /// it up to a limit, and moves toward the landing; red only locked.
    #[test]
    fn the_lock_closes_in_as_the_landing_nears_the_crease() {
        let on = lock_box(&landing([0.0, 0.2, 0.0], 0.0, true));
        assert_eq!(on.half, LOCK_HALF);
        assert_eq!(on.centre, [0.0, 0.2, 0.0]);
        assert!(on.locked);
        let near = lock_box(&landing([0.3, 0.2, 0.0], 0.2, false));
        let further = lock_box(&landing([0.5, 0.2, 0.0], 0.4, false));
        assert!(close(near.half[0], LOCK_HALF[0] + 0.2 * LOCK_OPEN));
        assert!(further.half[0] > near.half[0] && near.half[0] > on.half[0]);
        assert!(!near.locked);
        // Toward the error, by half of it.
        assert!(close(near.centre[0], 0.15) && close(near.centre[1], 0.2));
        // Opening and moving stop at their limits.
        let wild = lock_box(&landing([9.0, 0.2, 0.0], 8.9, false));
        assert!(close(wild.half[0], LOCK_HALF[0] + LOCK_OPEN_MAX));
        assert!(close(wild.centre[0], LOCK_SHIFT_MAX));
        // In the slot but off the line: closed, not moved.
        let inside = lock_box(&landing([0.05, 0.2, 0.0], 0.0, true));
        assert_eq!(inside.centre, [0.0, 0.2, 0.0]);
        // Eight arms, two at each corner, along the box's sides.
        let b = brackets(&on);
        assert_eq!(b.len(), 8);
        let arm = LOCK_ARM * LOCK_HALF[1];
        for [corner, end] in &b {
            assert!(close(distance3(*corner, *end), arm));
            assert!(close((corner[0]).abs(), LOCK_HALF[1]));
            assert!(close((corner[1] - 0.2).abs(), LOCK_HALF[0]));
        }
        // Red with `Locked` only when locked.
        let red = lock_callouts(&on, &LOOK);
        assert_eq!(
            (red[0].color, red[0].label.as_deref()),
            (RED, Some("Locked"))
        );
        let blue = lock_callouts(&near, &LOOK);
        assert_eq!(
            (blue[0].color, blue[0].label.as_deref()),
            (LOOK.colour, None)
        );
    }

    /// The lock eases by the wall clock: from what was shown when a tick's
    /// target came, to it over [`LOCK_EASE`], whatever the present rate.
    #[test]
    fn the_lock_eases_by_the_wall_clock() {
        let t0 = Instant::now();
        let to = lock_box(&landing([0.0, 0.0, 0.0], 0.0, true));
        let first = Ease::next(None, to, t0);
        // A new box appears wider and closes in.
        assert!(close(first.at(t0).half[0], LOCK_HALF[0] + LOCK_APPEAR));
        let ease = Duration::from_secs_f32(LOCK_EASE);
        assert_eq!(first.at(t0 + ease), to);
        assert_eq!(first.at(t0 + ease * 4), to);
        // Half way is half way (smoothstep), and it never overshoots.
        let mid = first.at(t0 + ease / 2).half[0];
        assert!(close(mid, LOCK_HALF[0] + LOCK_APPEAR / 2.0));
        let mut last = f32::INFINITY;
        for i in 0..=12 {
            let h = first.at(t0 + ease * i / 12).half[0];
            assert!(h <= last + 1e-6 && h >= LOCK_HALF[0] - 1e-6);
            last = h;
        }
        // The next tick's ease starts from what is shown then: no jump.
        let t1 = t0 + ease / 3;
        let wider = lock_box(&landing([0.4, 0.0, 0.0], 0.3, false));
        let second = Ease::next(Some(&first), wider, t1);
        let (a, b) = (second.at(t1), first.at(t1));
        assert_eq!((a.centre, a.half), (b.centre, b.half));
        assert_eq!(second.at(t1 + ease), wider);
        // The colour is the tick's at once: the lock is a per-tick rule.
        assert!(!a.locked);
        // Sampled at 30, 60 or 120 a second, the box at a moment is the
        // same: it depends on the time, not on how often it is drawn.
        let at = t0 + Duration::from_millis(50);
        assert_eq!(first.at(at), first.at(at));
        // The shared ease follows, and clears with no target.
        assert!(ease_lock(Some(to), t0).is_some());
        let kept = ease_lock(Some(wider), t1).unwrap();
        assert_eq!(kept.from, Ease::next(None, to, t0).at(t1));
        assert_eq!(ease_lock(None, t1), None);
    }

    /// The take-off for a fall is the lip of the nearest floor high enough,
    /// toward the player, a footprint back on it; a standing jump's is
    /// 0.2 back against the drift.
    #[test]
    fn the_takeoff_is_on_the_ledge_toward_the_player() {
        // A ledge at z 2 from x 1 on; the strip at the origin needs 1.5.
        let floor = |xy: [f32; 2], top: f32| {
            let z = if xy[0] >= 0.99 { 2.0 } else { 0.0 };
            (z <= top).then_some(z)
        };
        let t = takeoff([0.0; 3], 1.5, [1.0, 0.0], [3.0, 0.0, 2.0], floor).unwrap();
        assert!(
            (t[0] - (1.0 + CAPSULE_RADIUS)).abs() < TAKEOFF_STEP,
            "{t:?}"
        );
        assert!(close(t[1], 0.0) && close(t[2], 2.0));
        // Too low a ledge for the fall: none.
        assert_eq!(
            takeoff([0.0; 3], 2.5, [1.0, 0.0], [3.0, 0.0, 2.0], floor),
            None
        );
        // A floor above the player's reach (over TAKEOFF_TOP) does not count.
        assert_eq!(
            takeoff([0.0; 3], 1.5, [1.0, 0.0], [3.0, 0.0, 1.0], floor),
            None
        );
        // Over the strip: against the drift.
        let t = takeoff([0.0; 3], 1.5, [-1.0, 0.0], [0.0, 0.0, 2.0], floor).unwrap();
        assert!(t[0] > 1.0);
        // A standing jump: the prompts' take-off.
        let t = takeoff([0.0; 3], 0.0, [0.0, 2.0], [5.0, 5.0, 0.0], floor).unwrap();
        assert!(close(t[1], -TAKEOFF_BACK) && close(t[0], 0.0));
        assert_eq!(
            takeoff([f32::NAN; 3], 1.5, [1.0, 0.0], [0.0; 3], floor),
            None
        );
    }
}
