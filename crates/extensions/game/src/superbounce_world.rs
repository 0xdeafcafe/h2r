//! The superbounce marks drawn on the map (docs/game/superbounce.md,
//! "Floor marks" and "Lock-on"): one marker, on the one spot the player
//! is going for, in the HUD's colour, its label in the HUD's font
//! ([`Look`]). No other spot is drawn.
//!
//! - **Not primed**: the crouch spot the navpoint is over, once the
//!   player is within [`NEAR_RANGE`] of it (`Crouch`, with an approach
//!   arrow). Further away, nothing.
//! - **Primed**: the chosen strip (the navpoint's, which the fall assist
//!   steers onto): the footprint ring and the strip's own line through
//!   it. On the ground, from a floor high enough, a pin stands over it
//!   with `Land, 3 m` (the drop). While the approach guide is on the
//!   strip, its box is the strip's only mark.
//! - **The takeoff**, primed on the ground: a bar across the floor where
//!   to leave for the strip, `Jump from here` or `Drop from here`
//!   (`superbounce_guide::place`). From a floor too low for the strip,
//!   the pin and the navpoint are on the takeoff instead, and the strip
//!   has no label.
//! - Rings and pins fade out as the eye comes within [`NEAR_GONE`] of
//!   them, so none fills the screen or runs off its top; a mark the map
//!   hides from the eye is drawn dim with no label ([`occlude`]).
//! - **The lock-on**, while primed and falling: the slot's outline, corner
//!   brackets round the crease that close in as the predicted landing
//!   nears it, red and `Locked` within the slot at the speed it needs
//!   ([`lock_box`]). They ease by the wall clock ([`Ease`]), so they move
//!   as smoothly at 120 frames a second as at 30; the rules behind them
//!   run per guest tick.
//!
//! The builders are pure: [`ring`], [`arrowhead`], [`strip_bar`],
//! [`brackets`], [`lock_box`] and [`lip`] make geometry, [`figures`]
//! puts a frame's together from the target spot ([`Item`]) and the
//! player's [`Scene`], and [`frame`] reads both from the mode. What the
//! scanner and the mode's state give is read in one place, [`contract`].

use std::sync::Mutex;
use std::time::Instant;

use guest_layout::GuestMemory;
use gfx::world_lines::{Fade, Glow, LineStyle, Pulse};
use superbounce::{Kind, Spot, Vec3};

use crate::superbounce_charge::HIGH_SLACK;
use crate::superbounce_font::{self as hud, Look};
use crate::superbounce_guide::{Lip, Takeoff};
use crate::superbounce_mode::{DIM, FADE_FAR, FADE_NEAR, NavKind, State};
use crate::superbounce_prompts::{Callout, FLAT_FALL, LABEL_RAISE, TAKEOFF_BACK, whole_metres};

/// The biped's capsule radius: the footprint ring.
pub const CAPSULE_RADIUS: f32 = 0.175;
/// Floor figures sit this far above the floor.
pub const LIFT: f32 = 0.02;
/// Segments of a full ring.
pub const RING_SEGMENTS: usize = 24;
/// The charge's phase is shown on the next crouch spot while the player
/// is within this horizontal distance of it.
pub const PHASE_RANGE: f32 = 1.0;
/// A `likely` crouch spot's strength; a `proven` one's is 1.
pub const LIKELY: f32 = 0.5;
/// Crouch figures' strength while primed: a crouch would lose the charge.
pub const PRIMED_CROUCH: f32 = 0.2;
/// A strip's strength when the player's floor is too low to reach it.
pub const CLIMB: f32 = 0.5;
/// The next spot's marker is one footprint ring ([`CAPSULE_RADIUS`]),
/// and a pin standing over it from [`PIN_FOOT`] to [`PIN_TOP`] above the
/// floor, its label over the top.
pub const PIN_FOOT: f32 = 0.2;
pub const PIN_TOP: f32 = 0.8;
/// Rings and pins fade out as the eye nears them: full from
/// [`NEAR_FULL`] away, gone within [`NEAR_GONE`] ([`near_fade`]).
pub const NEAR_GONE: f32 = 1.5;
pub const NEAR_FULL: f32 = 2.5;
/// The eye over the feet, for the fade and the sight test
/// (`superbounce_ghost::EYE_HEIGHT`).
pub const EYE: f32 = crate::superbounce_ghost::EYE_HEIGHT;
/// The takeoff bar: half its length across the run, and its arrow's size
/// and gap ahead of it.
pub const BAR_HALF: f32 = 0.3;
pub const BAR_ARROW: f32 = 0.12;
pub const BAR_ARROW_GAP: f32 = 0.1;
/// Not primed, the next crouch spot is marked once the player is within
/// this of it; running about further away, nothing is drawn in the world.
pub const NEAR_RANGE: f32 = 6.0;
/// The strip's line through the landing marker: half its length along the
/// edge (at most half the edge).
pub const CREASE_HALF: f32 = 0.26;
/// The lip search ([`lip`]): the step out from the strip, how far it
/// looks, how far over the strip's fall a floor still counts, and the
/// fan of ways it tries, in degrees off the first (against the drift).
pub const TAKEOFF_STEP: f32 = 0.05;
pub const TAKEOFF_REACH: f32 = 4.0;
pub const TAKEOFF_ABOVE: f32 = 1.0;
pub const LIP_FAN: [f32; 12] = [
    0.0, 30.0, -30.0, 60.0, -60.0, 90.0, -90.0, 120.0, -120.0, 150.0, -150.0, 180.0,
];
/// A crouch spot's approach arrow: this far out from its marker's ring,
/// and its size.
pub const APPROACH_OUT: f32 = 0.15;
pub const APPROACH_SIZE: f32 = 0.14;
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
    /// The stand point of the spot the navpoint is over: a crouch spot
    /// while not primed, the chosen strip while primed.
    pub next: Option<Vec3>,
    /// The chosen strip's takeoff (`superbounce_guide::place`), primed on
    /// the ground.
    pub takeoff: Option<Takeoff>,
    /// The edge of the seam the approach guide is on, while it shows.
    pub guide: Option<[Vec3; 2]>,
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

/// How long the strip's line is each way ([`strip_bar`]): [`CREASE_HALF`],
/// at most half the edge (an edge with no length gets the full line).
pub fn crease_half([a, b]: [Vec3; 2]) -> f32 {
    let half = horizontal(a, b) / 2.0;
    if half > 1e-4 {
        half.min(CREASE_HALF)
    } else {
        CREASE_HALF
    }
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

/// Where to look for the takeoff of a strip at `stand` with `fall` and
/// `drift`, for a player whose feet are at `player`, with `floor(xy,
/// top)` the height of the highest floor under `xy` at or below `top`
/// (`superbounce_capsule::Room::floor_at`): the lip the approach guide
/// places the takeoff from (`superbounce_guide::place`).
///
/// - A strip a standing jump reaches (`fall` at most [`FLAT_FALL`]): the
///   prompts' take-off, [`TAKEOFF_BACK`] back from the stand point
///   against the drift (toward the player with none), heading at the
///   strip.
/// - Else: out from the strip against the drift first, then through
///   [`LIP_FAN`] round it (round the way to the player with no drift),
///   in steps of [`TAKEOFF_STEP`] up to [`TAKEOFF_REACH`], the first floor
///   at least `fall` above the strip (less [`HIGH_SLACK`]) and at most
///   [`TAKEOFF_ABOVE`] over that: the lip of the ledge to come from,
///   heading back at the strip. The player's own height does not limit
///   it: a lip over the player is the one to climb to.
///
/// `None` when no floor is found that way.
pub fn lip(
    stand: Vec3,
    fall: f32,
    drift: [f32; 2],
    player: Vec3,
    floor: impl Fn([f32; 2], f32) -> Option<f32>,
) -> Option<Lip> {
    if !stand.iter().chain(&player).all(|v| v.is_finite()) {
        return None;
    }
    let toward = flat_unit([player[0] - stand[0], player[1] - stand[1]]);
    let back = flat_unit([-drift[0], -drift[1]])
        .or(toward)
        .unwrap_or([1.0, 0.0]);
    let found = |at: Vec3, out: [f32; 2]| Lip {
        at,
        heading: [-out[0], -out[1]],
        speed: None,
        run: None,
    };
    if !(fall > FLAT_FALL) {
        let at = offset(stand, back, TAKEOFF_BACK);
        let z = floor([at[0], at[1]], stand[2] + TAKEOFF_ABOVE).unwrap_or(stand[2]);
        return Some(found([at[0], at[1], z], back));
    }
    let need = stand[2] + fall - HIGH_SLACK;
    let top = stand[2] + fall + TAKEOFF_ABOVE;
    let steps = (TAKEOFF_REACH / TAKEOFF_STEP).round() as usize;
    LIP_FAN.iter().find_map(|deg| {
        let (sin, cos) = deg.to_radians().sin_cos();
        let d = [back[0] * cos - back[1] * sin, back[0] * sin + back[1] * cos];
        (1..=steps).find_map(|i| {
            let at = offset(stand, d, i as f32 * TAKEOFF_STEP);
            let z = floor([at[0], at[1]], top).filter(|z| *z >= need)?;
            Some(found([at[0], at[1], z], d))
        })
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

/// The marker's lines: [`style`] with no fade, so the next spot shows
/// from across the map.
fn bold(look: &Look, width: f32, alpha: f32) -> LineStyle {
    LineStyle {
        fade: None,
        ..style(look, width, alpha)
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

/// How much of a ring or a pin `distance` from the eye shows: 1 from
/// [`NEAR_FULL`] out, 0 within [`NEAR_GONE`], straight between. A mark
/// at the feet would fill the screen, and a pin over the head runs off
/// its top.
pub fn near_fade(distance: f32) -> f32 {
    if !distance.is_finite() {
        return 1.0;
    }
    ((distance - NEAR_GONE) / (NEAR_FULL - NEAR_GONE)).clamp(0.0, 1.0)
}

/// `style` with its alpha scaled by `fade`.
fn faded(mut style: LineStyle, fade: f32) -> LineStyle {
    style.alpha *= fade;
    style
}

/// The marker of the next spot, at `at`: the footprint ring of
/// [`CAPSULE_RADIUS`] on the floor (pulsing with `pulse`), and with `pin`
/// a pin standing over it with `label` at its top. Without the pin the
/// label, if any, is on the ring. The ring fades by `ring_fade`, the pin
/// and its label by the eye's distance to its top ([`near_fade`]);
/// whatever has faded out is left out. Two callouts with the pin, one
/// without: the ring, the pin.
#[allow(clippy::too_many_arguments)]
fn marker(
    out: &mut Vec<Callout>,
    at: Vec3,
    colour: [f32; 3],
    label: Option<String>,
    pin: bool,
    pulse: Option<Pulse>,
    ring_fade: f32,
    eye: Vec3,
    look: &Look,
) {
    let top = up(at, PIN_TOP);
    let mut ring_style = bold(look, 3.0, 1.0);
    ring_style.glow = Some(Glow {
        width: 3.0,
        alpha: 0.35,
    });
    ring_style.pulse = pulse;
    let mut footprint = callout(
        ring(at, CAPSULE_RADIUS, false),
        up(at, LIFT),
        colour,
        faded(ring_style, ring_fade),
    );
    let pin_fade = near_fade(distance3(eye, top));
    if !pin || pin_fade <= 0.0 {
        footprint.label = label.filter(|_| !pin);
        if ring_fade > 0.0 {
            out.push(footprint);
        }
        return;
    }
    if ring_fade > 0.0 {
        out.push(footprint);
    }
    let mut stem = callout(
        vec![[up(at, PIN_FOOT), top]],
        up(top, LABEL_RAISE),
        colour,
        faded(bold(look, 2.0, 0.9), pin_fade),
    );
    stem.label = label;
    out.push(stem);
}

/// The strip's own line through `centre` along `along`, `half` each way,
/// [`LIFT`] up: which way the strip runs.
pub fn strip_bar(centre: Vec3, along: [f32; 2], half: f32) -> Vec<[Vec3; 2]> {
    let c = up(centre, LIFT);
    vec![[offset(c, along, -half), offset(c, along, half)]]
}

/// The eye of a player whose feet are at `at`.
fn eye_of(at: Vec3) -> Vec3 {
    up(at, EYE)
}

/// The chosen strip: its footprint ring with the strip's line through
/// it. On the ground, from a floor high enough ([`high_enough`]), or with
/// no takeoff to go to, it has its pin and `Land, 3 m`; from a floor too
/// low with a takeoff, neither: the takeoff has the pin. In the air it
/// has neither: the lock speaks, and it is red while the lock holds.
/// While the approach guide is on the strip, the guide's box is its only
/// mark, and none of this is drawn. The ring fades near the eye
/// ([`near_fade`]).
fn landing_figure(out: &mut Vec<Callout>, s: &Strip, scene: &Scene, look: &Look) {
    if scene.guide == Some(s.edge) {
        return;
    }
    let locked = scene.landing.is_some_and(|l| l.locked);
    let colour = if locked { RED } else { look.colour };
    let climb = !high_enough(scene.at[2], s) && scene.takeoff.is_some();
    let label = (!scene.airborne && !climb).then(|| {
        if s.fall > FLAT_FALL {
            format!("Land, {} m", whole_metres(s.fall))
        } else {
            "Land".into()
        }
    });
    let pin = label.is_some();
    let eye = eye_of(scene.at);
    let fade = near_fade(distance3(eye, s.stand));
    marker(out, s.stand, colour, label, pin, None, fade, eye, look);
    if fade > 0.0 {
        out.push(callout(
            strip_bar(s.stand, along_of(s.edge), crease_half(s.edge)),
            s.stand,
            colour,
            faded(bold(look, 3.0, 1.0), fade),
        ));
    }
}

/// The takeoff bar: a line across the floor at `t.at`, [`BAR_HALF`] each
/// way across the run, and an arrow just past it along the run, the way
/// to leave; `Jump from here` or `Drop from here` on it. With `pin` (the
/// player's floor is too low for the strip) a pin stands over it with the
/// label at its top instead. The bar does not fade: it is what to stand
/// on. An unchecked takeoff (no modelled flight from it bounces) is dim.
pub fn takeoff_figure(out: &mut Vec<Callout>, t: &Takeoff, pin: bool, eye: Vec3, look: &Look) {
    let Some(h) = flat_unit(t.heading) else {
        return;
    };
    let across = [-h[1], h[0]];
    let c = up(t.at, LIFT);
    let alpha = if t.checked { 1.0 } else { DIM };
    let mut edges = vec![[offset(c, across, -BAR_HALF), offset(c, across, BAR_HALF)]];
    edges.extend(arrowhead(offset(t.at, h, BAR_ARROW_GAP), h, BAR_ARROW));
    let label = t.leave.label().to_owned();
    let mut bar = callout(edges, up(t.at, LIFT), look.colour, bold(look, 3.5, alpha));
    let pin_fade = near_fade(distance3(eye, up(t.at, PIN_TOP)));
    if !pin || pin_fade <= 0.0 {
        bar.label = Some(label);
        out.push(bar);
        return;
    }
    out.push(bar);
    let top = up(t.at, PIN_TOP);
    let mut stem = callout(
        vec![[up(t.at, PIN_FOOT), top]],
        up(top, LABEL_RAISE),
        look.colour,
        faded(bold(look, 2.0, 0.9 * alpha), pin_fade),
    );
    stem.label = Some(label);
    out.push(stem);
}

/// The crouch spot to go to: the marker with `Crouch`, and an approach
/// arrow outside it, pointing in until the button is up, then out. Under
/// the lip (`phase` under or held) the ring pulses. Its ring does not
/// fade near the eye: under the lip is where it pulses. Its pin does.
fn crouch_figure(out: &mut Vec<Callout>, c: &Crouch, phase: Option<Phase>, eye: Vec3, look: &Look) {
    let under = matches!(phase, Some(Phase::Under | Phase::Held));
    let pulse = under.then_some(Pulse {
        period: 0.8,
        depth: 0.5,
    });
    marker(
        out,
        c.stand,
        look.colour,
        Some("Crouch".into()),
        true,
        pulse,
        1.0,
        eye,
        look,
    );
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
            bold(look, 2.5, 0.9),
        ));
    }
}

/// The marks for one frame: one marker, on the one spot the player is
/// going for, and nothing else. `items` are the candidates.
///
/// - Not primed, it is the crouch spot the navpoint is over
///   (`scene.next`), once the player is within [`NEAR_RANGE`] of it: the
///   marker, `Crouch` and its approach arrow. The charge's phase shows on
///   it within [`PHASE_RANGE`]. Further away, nothing.
/// - Primed, it is the chosen strip (`scene.chosen`): the marker and its
///   line, with `Land, 3 m` on the ground ([`landing_figure`]), and on the
///   ground its takeoff's bar ([`takeoff_figure`]). In a fall the slot's
///   outline is the [`Ease`]'s, drawn per present ([`lock_callouts`]).
/// - In the air nothing of a crouch spot is drawn: a crouch is for the
///   ground.
/// - No other spot is drawn, whatever its kind.
pub fn figures(items: &[Item], scene: &Scene, look: &Look) -> Vec<Callout> {
    let mut out = Vec::new();
    let eye = eye_of(scene.at);
    for item in items {
        match item {
            Item::Strip(s) if scene.primed && scene.chosen == Some(s.stand) => {
                if let Some(t) = scene.takeoff.filter(|_| !scene.airborne) {
                    takeoff_figure(&mut out, &t, !high_enough(scene.at[2], s), eye, look);
                }
                landing_figure(&mut out, s, scene, look);
                return out;
            }
            Item::Crouch(c)
                if !scene.primed
                    && !scene.airborne
                    && scene.next == Some(c.stand)
                    && distance3(scene.at, c.stand) <= NEAR_RANGE =>
            {
                let phase = scene
                    .phase
                    .filter(|_| horizontal(scene.at, c.stand) <= PHASE_RANGE);
                crouch_figure(&mut out, c, phase, eye, look);
                return out;
            }
            _ => {}
        }
    }
    out
}

/// The spots a frame may mark, of `spots`: the chosen strip while primed,
/// else the crouch spot at `next`. At most one of each.
pub fn targets(
    spots: &[Spot],
    primed: bool,
    chosen: Option<Vec3>,
    next: Option<Vec3>,
) -> Vec<Item> {
    let strip = spots
        .iter()
        .find(|sp| primed && sp.kind == Kind::Bounce && chosen == Some(sp.stand))
        .map(|sp| Item::Strip(contract::strip(sp)));
    let crouch = spots
        .iter()
        .find(|sp| !primed && sp.kind == Kind::Crouch && next == Some(sp.stand))
        .map(|sp| Item::Crouch(contract::crouch(sp)));
    strip.into_iter().chain(crouch).collect()
}

/// Everything the mode draws on the map this frame but the lock-on: the
/// one target's marker ([`figures`]). Nothing without a position.
pub fn frame(guest: &dyn GuestMemory, s: &State, spots: &[Spot]) -> Vec<Callout> {
    let Some(at) = s.position else {
        return Vec::new();
    };
    let look = hud::look(guest);
    let primed = s.primed();
    let chosen = contract::chosen(s);
    let next = s.nav_target.map(|n| n.at);
    let falling = s.vz.is_some_and(|v| v < 0.0);
    let scene = Scene {
        at,
        airborne: s.airborne,
        falling,
        primed,
        phase: contract::phase(s),
        landing: contract::landing(s).filter(|_| primed && s.airborne && falling),
        chosen,
        next,
        takeoff: s.takeoff,
        guide: s.guide.as_ref().map(|g| g.seam.edge),
    };
    // The chosen strip may be a seam of the targets' set the mode does
    // not mark (the faced seam): it is looked for among both.
    let shown = crate::superbounce_targets::current();
    let all: Vec<Spot> = spots
        .iter()
        .chain(shown.iter().map(|t| &t.spot))
        .copied()
        .collect();
    let mut out = figures(&targets(&all, primed, chosen, next), &scene, &look);
    // Primed, every seam of the set has a line on its collision edge
    // (docs/game/superbounce.md, "Seam lines").
    if primed {
        out.extend(seam_callouts(
            &crate::superbounce_seams::lines(&shown, at, chosen),
            &look,
        ));
    }
    let room = crate::superbounce_mode::current_room();
    occlude(&mut out, eye_of(at), |a, b| room.blocked(a, b));
    out
}

/// The chosen seam's line: its width, pixels at 1080p.
pub const SEAM_WIDTH: f32 = 4.0;
/// Another seam's line.
pub const SEAM_WIDTH_OTHER: f32 = 2.5;

/// The seam lines as callouts: each line's segments
/// (`superbounce_seams::segments`: the collision edge, vertex to vertex,
/// and the chosen seam's end ticks) in the HUD's colour at the line's
/// alpha, the chosen seam's [`SEAM_WIDTH`] wide and the others'
/// [`SEAM_WIDTH_OTHER`]. No label and no fade of the style's own: the
/// line's alpha has the distance in it.
pub fn seam_callouts(lines: &[crate::superbounce_seams::SeamLine], look: &Look) -> Vec<Callout> {
    lines
        .iter()
        .map(|l| {
            let width = if l.chosen { SEAM_WIDTH } else { SEAM_WIDTH_OTHER };
            callout(
                crate::superbounce_seams::segments(l),
                crate::superbounce_seams::anchor(l),
                look.colour,
                bold(look, width, l.alpha),
            )
        })
        .collect()
}

/// Marks the map hides from the eye: each callout whose anchor `blocked(eye,
/// anchor)` says is behind a polygon is drawn dim ([`DIM`] of its alpha)
/// and loses its label, so a mark behind a hill does not read as being on
/// its near face (`superbounce_capsule::Room::blocked`).
pub fn occlude(out: &mut [Callout], eye: Vec3, blocked: impl Fn(Vec3, Vec3) -> bool) {
    for c in out.iter_mut() {
        if blocked(eye, c.anchor) {
            c.style.alpha *= DIM;
            c.label = None;
        }
    }
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

/// The lock-on's target box as drawn: [`lock_target`], and none while the
/// approach guide is on a seam and the screen is `full` (the guide's
/// marks are fullscreen's): its box is on the seam then, and red for a
/// landing that will bounce (docs/game/superbounce.md, "The approach
/// guide").
pub fn lock_shown(s: &State, full: bool) -> Option<LockBox> {
    lock_target(s).filter(|_| !(full && s.guide.is_some()))
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
    /// The lock-on for a capture of `s` at `now`, on a screen that is
    /// `full` or split ([`lock_shown`]).
    pub fn capture(
        guest: &dyn GuestMemory,
        s: &State,
        full: bool,
        now: Instant,
    ) -> Option<LockMarks> {
        let ease = ease_lock(lock_shown(s, full), now)?;
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
    use crate::superbounce_guide::Leave;
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

    /// A seam line's callout is the collision edge itself, vertex to
    /// vertex: the chosen seam's bold with its end ticks, another's
    /// thinner at its own alpha, neither with a label or a distance fade.
    #[test]
    fn a_seam_line_is_drawn_on_the_collision_edge() {
        use crate::superbounce_seams::{LIFT as SEAM_LIFT, SeamLine};
        let edge = [[1.0, 5.0, -3.2], [3.0, 5.0, -3.2]];
        let line = |chosen, alpha| SeamLine {
            edge,
            stand: [2.0, 4.976, -3.19],
            chosen,
            alpha,
        };
        let out = seam_callouts(&[line(false, 0.6), line(true, 1.0)], &LOOK);
        assert_eq!(out.len(), 2);
        let on_edge = [[1.0, 5.0, -3.2 + SEAM_LIFT], [3.0, 5.0, -3.2 + SEAM_LIFT]];
        assert_eq!(out[0].edges, vec![on_edge]);
        assert_eq!((out[0].style.width, out[0].style.alpha), (SEAM_WIDTH_OTHER, 0.6));
        assert_eq!(out[1].edges[0], on_edge);
        assert_eq!(out[1].edges.len(), 3);
        assert_eq!((out[1].style.width, out[1].style.alpha), (SEAM_WIDTH, 1.0));
        for c in &out {
            assert_eq!(c.color, LOOK.colour);
            assert!(c.label.is_none() && c.style.fade.is_none());
            // The anchor is on the edge, by the landing point.
            assert_eq!(c.anchor, [2.0, 5.0, -3.2 + SEAM_LIFT]);
        }
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
            next: None,
            takeoff: None,
            guide: None,
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
    fn an_arrowhead_points_along_its_direction() {
        let a = arrowhead([0.0; 3], [-1.0, 0.0], 0.2);
        assert_eq!(a.len(), 2);
        let tip = a[0][1];
        assert_eq!(tip, a[1][1]);
        assert!(close(tip[0], -0.1));
        assert!(a[0][0][0] > tip[0] && a[0][0][1] * a[1][0][1] < 0.0);
        assert!(arrowhead([0.0; 3], [0.0, 0.0], 0.2).is_empty());
    }

    /// The strip's line through its marker is short: at most
    /// [`CREASE_HALF`] each way along the edge, at most half the edge.
    #[test]
    fn the_strips_line_is_short_and_runs_along_the_edge() {
        let s = strip(2.0, 1.0);
        assert!(close(crease_half(s.edge), 0.5f32.min(CREASE_HALF)));
        assert!(close(crease_half([[0.0; 3], [0.2, 0.0, 0.0]]), 0.1));
        assert_eq!(crease_half([[0.0; 3]; 2]), CREASE_HALF);
        let m = strip_bar(s.stand, along_of(s.edge), CREASE_HALF);
        assert_eq!(m.len(), 1);
        let [a, b] = m[0];
        assert!(close(a[1], -CREASE_HALF) && close(b[1], CREASE_HALF));
        assert!(close(a[0], 2.0) && close(b[0], 2.0) && close(a[2], LIFT));
    }

    fn labels(out: &[Callout]) -> Vec<&str> {
        out.iter().filter_map(|c| c.label.as_deref()).collect()
    }

    /// The callouts that are a ring of `radius` round `at`.
    fn rings(out: &[Callout], at: Vec3, radius: f32) -> Vec<&Callout> {
        out.iter()
            .filter(|c| {
                c.edges.len() >= 12
                    && c.edges
                        .iter()
                        .all(|[p, _]| close(horizontal(*p, at), radius))
            })
            .collect()
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
        assert_eq!(crouch_strength(true, false), 1.0);
        assert_eq!(crouch_strength(false, false), LIKELY);
        assert_eq!(crouch_strength(true, true), PRIMED_CROUCH);
        assert_eq!(crouch_strength(false, true), LIKELY * PRIMED_CROUCH);
    }

    /// Not primed: one marker, on the crouch spot the navpoint is over,
    /// and nothing of any other spot.
    #[test]
    fn not_primed_the_marker_is_on_the_next_crouch_spot_alone() {
        let next = crouch(4.0, true);
        let items = [
            Item::Crouch(next.clone()),
            Item::Strip(strip(1.5, 1.0)),
            Item::Crouch(crouch(2.0, true)),
            Item::Crouch(crouch(3.0, false)),
        ];
        let s = Scene {
            next: Some(next.stand),
            ..scene()
        };
        let out = figures(&items, &s, &LOOK);
        // The marker (one footprint ring, the pin) and its arrow.
        assert_eq!(out.len(), 2 + 1);
        assert_eq!(labels(&out), ["Crouch"]);
        let ring = rings(&out, next.stand, CAPSULE_RADIUS);
        assert_eq!(ring.len(), 1);
        assert!(ring[0].style.fade.is_none() && ring[0].style.width >= 3.0);
        assert_eq!(ring[0].style.alpha, 1.0);
        // The pin stands over it with the label at its top.
        let pin = &out[1];
        assert_eq!(pin.edges.len(), 1);
        let [foot, top] = pin.edges[0];
        assert!(close(foot[2], PIN_FOOT) && close(top[2], PIN_TOP));
        assert!(pin.anchor[2] > PIN_TOP && pin.label.is_some());
        // Every line is the next spot's: nothing at the others.
        assert!(out.iter().all(|c| horizontal(c.anchor, next.stand) < 1.0));
        assert!(out.iter().all(|c| c.color == LOOK.colour));
        // With no next spot, nothing at all.
        assert!(figures(&items, &scene(), &LOOK).is_empty());
        // In the air, nothing: a crouch is for the ground.
        let air = Scene {
            airborne: true,
            ..s
        };
        assert!(figures(&items, &air, &LOOK).is_empty());
    }

    /// Running about unprimed away from the next spot, the world is bare.
    #[test]
    fn unprimed_and_far_from_the_spot_nothing_is_drawn() {
        let next = crouch(1.0, true);
        let items = [Item::Crouch(next.clone()), Item::Strip(strip(1.5, 1.0))];
        let at = |x: f32| Scene {
            at: [x, 0.0, 0.0],
            next: Some(next.stand),
            ..scene()
        };
        assert!(!figures(&items, &at(1.0 + NEAR_RANGE - 0.1), &LOOK).is_empty());
        assert!(figures(&items, &at(1.0 + NEAR_RANGE + 0.1), &LOOK).is_empty());
        assert!(figures(&items, &at(40.0), &LOOK).is_empty());
    }

    #[test]
    fn the_crouch_markers_arrow_points_in_then_out_and_its_ring_pulses_under_the_lip() {
        let c = crouch(0.5, true);
        let at = |phase, x: f32| {
            let s = Scene {
                phase,
                at: [x, 0.0, 0.0],
                next: Some(c.stand),
                ..scene()
            };
            figures(&[Item::Crouch(c.clone())], &s, &LOOK)
        };
        // The arrow points in (-X, `out` is +X) until the button is up.
        let arrow = |phase| {
            let out = at(phase, 0.0);
            let [wing, tip] = out.last().unwrap().edges[0];
            (tip[0] - wing[0]).signum()
        };
        assert_eq!(arrow(None), -1.0);
        assert_eq!(arrow(Some(Phase::Under)), -1.0);
        assert_eq!(arrow(Some(Phase::Held)), 1.0);
        // It sits outside the ring, on the way out.
        let [_, tip] = at(None, 0.0).last().unwrap().edges[0];
        assert!(tip[0] > 0.5 + CAPSULE_RADIUS);
        // Standing on the spot the pin has faded out (it would run off
        // the top of the screen); the ring stays, to pulse under the lip.
        let near = at(Some(Phase::Under), 0.5);
        assert_eq!(near.len(), 2);
        assert!(labels(&near).is_empty());
        assert_eq!(rings(&near, c.stand, CAPSULE_RADIUS).len(), 1);
        // Under the lip the ring pulses; standing away it does not.
        assert!(at(Some(Phase::Under), 0.0)[0].style.pulse.is_some());
        assert!(at(None, 0.0)[0].style.pulse.is_none());
        assert!(at(Some(Phase::Under), 3.0)[0].style.pulse.is_none());
    }

    /// Primed: the chosen strip and no other, however many are near.
    #[test]
    fn primed_only_the_chosen_strip_is_marked() {
        let chosen = strip(3.0, 1.0);
        let items = [
            Item::Strip(strip(2.0, 0.4)),
            Item::Strip(chosen),
            Item::Strip(strip(4.0, 1.0)),
            Item::Crouch(crouch(1.0, true)),
        ];
        let s = Scene {
            primed: true,
            at: [0.0, 0.0, 1.0],
            chosen: Some(chosen.stand),
            next: Some(chosen.stand),
            ..scene()
        };
        let out = figures(&items, &s, &LOOK);
        // The footprint ring, the pin, the strip's line.
        assert_eq!(out.len(), 3);
        assert_eq!(labels(&out), ["Land, 3 m"]);
        assert_eq!(rings(&out, chosen.stand, CAPSULE_RADIUS).len(), 1);
        assert!(out.iter().all(|c| horizontal(c.anchor, chosen.stand) < 0.5));
        let [a, b] = out[2].edges[0];
        assert!(close(a[0], 3.0) && close(b[0], 3.0) && a[1] < b[1]);
        let flat = Scene {
            chosen: Some([5.0, 0.0, 0.0]),
            ..s
        };
        assert_eq!(
            labels(&figures(&[Item::Strip(strip(5.0, 0.0))], &flat, &LOOK)),
            ["Land"]
        );
        // No strip chosen: nothing, though strips are near. Not primed: no
        // strip either.
        let none = Scene { chosen: None, ..s };
        assert!(figures(&items, &none, &LOOK).is_empty());
        let unprimed = Scene { primed: false, ..s };
        assert!(figures(&items, &unprimed, &LOOK).is_empty());
        // In the air there is no pin and no label: ring and line; the
        // lock-on is the slot's outline.
        let air = Scene {
            airborne: true,
            falling: true,
            landing: Some(landing([3.0, 0.0, 0.0], 0.0, false)),
            ..s
        };
        let out = figures(&items, &air, &LOOK);
        assert_eq!(out.len(), 2);
        assert!(labels(&out).is_empty());
        let rising = Scene {
            airborne: true,
            ..s
        };
        assert!(labels(&figures(&items, &rising, &LOOK)).is_empty());
        // Locked: the mark is red.
        let locked = Scene {
            landing: Some(landing([3.0, 0.0, 0.0], 0.0, true)),
            ..air
        };
        assert!(
            figures(&items, &locked, &LOOK)
                .iter()
                .all(|c| c.color == RED)
        );
    }

    /// A frame's candidates are one spot: the chosen strip while primed,
    /// else the next crouch spot.
    #[test]
    fn a_frame_has_one_target() {
        let spot = |kind, x: f32| Spot {
            kind,
            stand: [x, 0.0, 0.0],
            edge: [[x, -0.5, 0.0], [x, 0.5, 0.0]],
            ..Spot::default()
        };
        let spots = [
            spot(Kind::Bounce, 1.0),
            spot(Kind::Bounce, 2.0),
            spot(Kind::Crouch, 3.0),
            spot(Kind::Crouch, 4.0),
        ];
        let at = |x: f32| Some([x, 0.0, 0.0]);
        let t = targets(&spots, true, at(2.0), at(2.0));
        assert!(matches!(&t[..], [Item::Strip(s)] if s.stand[0] == 2.0));
        let t = targets(&spots, false, None, at(4.0));
        assert!(matches!(&t[..], [Item::Crouch(c)] if c.stand[0] == 4.0));
        // Primed, the crouch spots are not candidates; unprimed, the
        // strips are not; with no target there is none.
        assert!(targets(&spots, true, None, at(3.0)).is_empty());
        assert!(targets(&spots, false, at(1.0), at(1.0)).is_empty());
        assert!(targets(&spots, false, None, None).is_empty());
    }

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

    /// A ledge at z 2 from x 1 on, over a floor at z 0.
    fn ledge(xy: [f32; 2], top: f32) -> Option<f32> {
        let z = if xy[0] >= 0.99 { 2.0 } else { 0.0 };
        (z <= top).then_some(z)
    }

    /// The lip search: against the drift first, the first floor high
    /// enough for the fall and no higher than a unit over it, wherever
    /// the player is; then a fan round it; a standing jump's is 0.2 back
    /// against the drift. Each heads back at the strip.
    #[test]
    fn the_lip_is_found_against_the_drift_then_in_a_fan() {
        // The strip at the origin drifts -X and needs 1.5: the ledge is
        // behind it, against the drift.
        let l = lip([0.0; 3], 1.5, [-1.0, 0.0], [-3.0, 0.0, 0.0], ledge).unwrap();
        assert!((l.at[0] - 1.0).abs() <= TAKEOFF_STEP, "{l:?}");
        assert!(close(l.at[1], 0.0) && close(l.at[2], 2.0));
        assert_eq!(l.heading, [-1.0, 0.0]);
        assert_eq!((l.speed, l.run), (None, None));
        // From below the ledge (the player on the strip's floor) it is
        // found all the same: the review's frames 67 to 87.
        let below = lip([0.0; 3], 1.5, [-1.0, 0.0], [0.0, 0.5, 0.0], ledge).unwrap();
        assert_eq!(below.at, l.at);
        // Drifting +X the ledge is with the drift: the fan finds it 120
        // degrees round, heading back at the strip.
        let fan = lip([0.0; 3], 1.5, [1.0, 0.0], [-3.0, 0.0, 0.0], ledge).unwrap();
        assert!(fan.at[0] > 0.98 && close(fan.at[2], 2.0), "{fan:?}");
        let r = fan.at[0].hypot(fan.at[1]);
        assert!(close(fan.heading[0], -fan.at[0] / r) && close(fan.heading[1], -fan.at[1] / r));
        assert!(close(fan.at[0] / r, 0.5), "{fan:?}");
        // Too low a ledge for the fall: none. One far over it: none.
        assert_eq!(lip([0.0; 3], 2.5, [-1.0, 0.0], [0.0; 3], ledge), None);
        assert_eq!(lip([0.0; 3], 0.5, [-1.0, 0.0], [0.0; 3], ledge), None);
        // A standing jump: the prompts' take-off, heading at the strip.
        let t = lip([0.0; 3], 0.0, [0.0, 2.0], [5.0, 5.0, 0.0], ledge).unwrap();
        assert!(close(t.at[1], -TAKEOFF_BACK) && close(t.at[0], 0.0));
        assert_eq!(t.heading, [0.0, 1.0]);
        assert_eq!(lip([f32::NAN; 3], 1.5, [1.0, 0.0], [0.0; 3], ledge), None);
    }

    fn takeoff_at(x: f32, leave: Leave, checked: bool) -> Takeoff {
        Takeoff {
            at: [x, 0.0, 2.0],
            heading: [-1.0, 0.0],
            half: 0.05,
            speed: 0.8,
            leave,
            checked,
        }
    }

    /// Primed on the ground the takeoff is a bar across the floor with
    /// `Jump from here` or `Drop from here`. From a floor too low for the
    /// strip, the pin is on the takeoff and the strip has no `Land`; from
    /// one high enough the strip keeps its pin and label, and the bar its
    /// label on the floor. In the air there is no bar.
    #[test]
    fn the_takeoff_is_a_bar_on_the_floor_with_the_pin_when_too_low() {
        let chosen = strip(0.0, 1.5);
        let items = [Item::Strip(chosen)];
        let t = takeoff_at(1.2, Leave::Drop, true);
        let low = Scene {
            primed: true,
            at: [-4.0, 0.0, 0.0],
            chosen: Some(chosen.stand),
            next: Some(chosen.stand),
            takeoff: Some(t),
            ..scene()
        };
        let out = figures(&items, &low, &LOOK);
        assert_eq!(labels(&out), ["Drop from here"]);
        // The bar lies across the run at the takeoff, with its arrow.
        let bar = &out[0];
        let [a, b] = bar.edges[0];
        assert!(close(a[0], 1.2) && close(b[0], 1.2) && close((a[1] - b[1]).abs(), 2.0 * BAR_HALF));
        assert!(close(a[2], 2.0 + LIFT) && bar.edges.len() == 3);
        // The pin is over the takeoff, its label at its top.
        let pin = out.iter().find(|c| c.label.is_some()).unwrap();
        assert!(close(pin.anchor[0], 1.2) && pin.anchor[2] > 2.0 + PIN_TOP);
        // The strip: its ring and line, no label.
        assert_eq!(rings(&out, chosen.stand, CAPSULE_RADIUS).len(), 1);
        // A jump's takeoff says so.
        let jump = Scene {
            takeoff: Some(takeoff_at(1.2, Leave::Jump, true)),
            ..low
        };
        assert_eq!(labels(&figures(&items, &jump, &LOOK)), ["Jump from here"]);
        // High enough: the strip keeps `Land`, the bar its label on the
        // floor, with no pin.
        let high = Scene {
            at: [4.0, 0.0, 2.0],
            ..low
        };
        let out = figures(&items, &high, &LOOK);
        assert_eq!(labels(&out), ["Drop from here", "Land, 5 m"]);
        assert!(close(out[0].anchor[2], 2.0 + LIFT));
        // No takeoff found: the strip keeps `Land`, high enough or not.
        let none = Scene {
            takeoff: None,
            ..low
        };
        assert_eq!(labels(&figures(&items, &none, &LOOK)), ["Land, 5 m"]);
        // An unchecked takeoff is dim.
        let guess = Scene {
            takeoff: Some(takeoff_at(1.2, Leave::Drop, false)),
            ..low
        };
        assert_eq!(figures(&items, &guess, &LOOK)[0].style.alpha, DIM);
        // In the air: no bar.
        let air = Scene {
            airborne: true,
            ..low
        };
        assert!(labels(&figures(&items, &air, &LOOK)).is_empty());
        assert!(
            figures(&items, &air, &LOOK)
                .iter()
                .all(|c| c.anchor[0] < 0.5)
        );
    }

    /// While the approach guide is on the strip, the guide's box is its
    /// only mark: no ring, no line, no pin; the takeoff bar stays.
    #[test]
    fn while_the_guide_is_on_the_strip_only_its_box_marks_it() {
        let chosen = strip(0.0, 1.5);
        let items = [Item::Strip(chosen)];
        let s = Scene {
            primed: true,
            at: [4.0, 0.0, 2.0],
            chosen: Some(chosen.stand),
            next: Some(chosen.stand),
            takeoff: Some(takeoff_at(1.2, Leave::Jump, true)),
            guide: Some(chosen.edge),
            ..scene()
        };
        let out = figures(&items, &s, &LOOK);
        assert_eq!(labels(&out), ["Jump from here"]);
        assert!(rings(&out, chosen.stand, CAPSULE_RADIUS).is_empty());
        assert!(out.iter().all(|c| horizontal(c.anchor, chosen.stand) > 1.0));
        // On another seam the strip is marked as ever.
        let other = Scene {
            guide: Some([[9.0, 0.0, 0.0], [9.0, 1.0, 0.0]]),
            ..s
        };
        assert_eq!(
            rings(
                &figures(&items, &other, &LOOK),
                chosen.stand,
                CAPSULE_RADIUS
            )
            .len(),
            1
        );
    }

    /// Rings and pins fade out as the eye nears them: gone within 1.5
    /// units, full from 2.5. A strip at the feet is not drawn, and its pin
    /// does not run off the top of the screen.
    #[test]
    fn rings_and_pins_fade_out_near_the_eye() {
        assert_eq!(near_fade(0.0), 0.0);
        assert_eq!(near_fade(NEAR_GONE), 0.0);
        assert!(close(near_fade(2.0), 0.5));
        assert_eq!(near_fade(NEAR_FULL), 1.0);
        assert_eq!(near_fade(40.0), 1.0);
        let chosen = strip(0.0, 0.0);
        let items = [Item::Strip(chosen)];
        let at = |x: f32| Scene {
            primed: true,
            at: [x, 0.0, 0.0],
            chosen: Some(chosen.stand),
            next: Some(chosen.stand),
            ..scene()
        };
        // Standing on it: nothing.
        assert!(figures(&items, &at(0.3), &LOOK).is_empty());
        // Half way through the fade: the ring at half strength.
        let x = (2.0f32.powi(2) - EYE.powi(2)).sqrt();
        let out = figures(&items, &at(x), &LOOK);
        let ring = rings(&out, chosen.stand, CAPSULE_RADIUS);
        assert!(close(ring[0].style.alpha, 0.5), "{}", ring[0].style.alpha);
        // Far off: full.
        let out = figures(&items, &at(6.0), &LOOK);
        assert_eq!(
            rings(&out, chosen.stand, CAPSULE_RADIUS)[0].style.alpha,
            1.0
        );
    }

    /// A mark the map hides from the eye is dim and has no label; one in
    /// sight is as it was.
    #[test]
    fn a_mark_behind_geometry_is_dim_and_unlabelled() {
        let chosen = strip(0.0, 0.0);
        let s = Scene {
            primed: true,
            at: [6.0, 0.0, 0.0],
            chosen: Some(chosen.stand),
            next: Some(chosen.stand),
            ..scene()
        };
        let shown = figures(&[Item::Strip(chosen)], &s, &LOOK);
        assert_eq!(labels(&shown), ["Land"]);
        // A wall at x 3 between the eye and the strip.
        let room = crate::superbounce_capsule::Room::new(&[superbounce::Poly {
            verts: vec![
                [3.0, -2.0, -1.0],
                [3.0, 2.0, -1.0],
                [3.0, 2.0, 3.0],
                [3.0, -2.0, 3.0],
            ],
            normal: [1.0, 0.0, 0.0],
        }]);
        let eye = [6.0, 0.0, EYE];
        let mut hidden = shown.clone();
        occlude(&mut hidden, eye, |a, b| room.blocked(a, b));
        assert!(labels(&hidden).is_empty());
        for (h, c) in hidden.iter().zip(&shown) {
            assert!(close(h.style.alpha, c.style.alpha * DIM));
        }
        // With nothing in the way, nothing changes.
        let mut seen = shown.clone();
        occlude(&mut seen, eye, |_, _| false);
        assert_eq!(seen, shown);
    }
}
