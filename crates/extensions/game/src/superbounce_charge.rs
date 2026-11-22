//! The superbounce charge, the next thing to do and the landings
//! (docs/game/superbounce.md, "Charge, crouch spots and landings").
//!
//! The pair word has no amount, so the charge is the player's place in the
//! recipe (`near`, `under`, `held`, then `primed`) and, once primed, the
//! time a still body has before it sleeps and its pair is rebuilt at 0.
//! [`Tracker::step`] turns one tick's reads ([`Tick`]) into the
//! [`ChargePhase`] and its fill, why a lost charge went ([`LostReason`]),
//! the prompt-line [`Verb`], the [`NavTarget`] and, on a touchdown by a
//! strip, the filed [`Landing`].
//! `superbounce_mode` publishes them on its state; the predicted landing
//! ([`Predicted`]) is `superbounce_assist::predict`'s.
//!
//! Everything here is pure: the rules run on synthetic tick sequences in
//! the tests.

use superbounce::{Kind, Spot, Vec3};

use crate::superbounce_mode::{RIBBON_WIDTH, primed};

/// A crouched player within this distance (world units) of a crouch
/// spot's stand point is `near` it.
pub const NEAR_RADIUS: f32 = 1.5;
/// A primed body moving slower than this (units/s) counts a still tick.
pub const STILL_SPEED: f32 = 0.03;
/// Still ticks after which a primed body sleeps and its pair is rebuilt at
/// 0 (38 to 40 measured: docs/game/superbounce.md, "Charge").
pub const SLEEP_TICKS: u32 = 38;
/// Ticks `lost` shows before the charge is hidden: a second at 30 Hz.
pub const LOST_TICKS: u32 = 30;
/// Still ticks after which a primed body is warned to keep moving: half
/// of [`SLEEP_TICKS`], 19, where the fill reaches 0.5.
pub const WARN_STILL_TICKS: u32 = SLEEP_TICKS / 2;
/// A charge lost after at least this many still ticks was lost to
/// standing still: the body slept. 30, under the 38 to 40 measured, so a
/// sleep a few ticks early still reads as one; a loss after fewer is not
/// put down to standing still.
pub const STOOD_STILL_TICKS: u32 = 30;
/// Ticks after a loss the prompt line asks for a new charge: 5 s at
/// 30 Hz, the `lost` second and four more.
pub const AGAIN_TICKS: u32 = 150;
/// A touchdown within this distance (world units) of a strip's edge is
/// that strip's landing.
pub const LANDING_RADIUS: f32 = 0.5;
/// A landing whose vertical speed comes out over this (units/s) bounced.
pub const BOUNCE_VZ: f32 = 8.0;
/// A body falling at least this fast (units/s, down negative) on the tick
/// before is touching down when it stops falling.
pub const TOUCHDOWN_VZ: f32 = -1.0;
/// It has stopped falling when its vertical speed is over this.
pub const LANDED_VZ: f32 = -0.5;
/// Gravity (units/s²), as the assist has it.
pub const GRAVITY: f32 = crate::superbounce_assist::GRAVITY;

// ---- what the mode publishes ----

/// How far through the recipe the player is, or that the charge just went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChargePhase {
    /// Crouched within [`NEAR_RADIUS`] of a crouch spot, the standing
    /// capsule fits. Fill 0.
    Near,
    /// Crouched, crouch held, and the standing capsule does not fit.
    /// Fill 1/3.
    Under,
    /// The crouch button is up and the request (unit flags bit 23) is
    /// still set: the forced crouch. Fill 2/3.
    Held,
    /// The word is -3 and the player is not crouching. Fill 1 less the
    /// still ticks over [`SLEEP_TICKS`].
    Primed,
    /// The charge just went (the word left -3, or a crouch while primed).
    /// Fill 0 for [`LOST_TICKS`], then the charge is hidden.
    Lost,
}

/// Why the charge went (docs/game/superbounce.md, "Charge"). A landing
/// is not a reason: nothing measured has one use the charge up (the word
/// stays -3 after a bounce until the body sleeps,
/// docs/game/notes/superbounce-spots.md), so a loss on a landing is
/// whichever of these it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LostReason {
    /// The word left -3 after [`STOOD_STILL_TICKS`] still ticks or more:
    /// the body slept and its pair was rebuilt at 0.
    StoodStill,
    /// The crouch request was set: a crouch ends the charge at once.
    Crouched,
    /// The word left -3 for another reason (a respawn, a vehicle, the
    /// pair gone, or a sleep the still count missed).
    Unknown,
}

/// A loss: why, and the tick it was on (the game's, or the tracker's own
/// count where the game's could not be read, as a [`Landing`]'s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Lost {
    pub reason: LostReason,
    pub tick: u32,
}

/// The charge meter's state: `None` on [`crate::superbounce_mode::State`]
/// is hidden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Charge {
    pub phase: ChargePhase,
    /// 0..1, the meter's fill.
    pub fill: f32,
    /// The loss, in [`ChargePhase::Lost`]; `None` in every other phase.
    pub lost: Option<Lost>,
}

/// The next thing to do, on the HUD's prompt line (the recipe's steps
/// with the crouch button the game draws, the warning, the new charge).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verb {
    /// "Let go of (crouch)": [`ChargePhase::Under`].
    LetGoOfCrouch,
    /// "Walk out to stand up": [`ChargePhase::Held`].
    WalkOutToStand,
    /// "Keep moving or lose the charge": primed and still for
    /// [`WARN_STILL_TICKS`] or more.
    KeepMoving,
    /// "Find a crouch spot to charge again": for [`AGAIN_TICKS`] after a
    /// loss, while no recipe step has begun.
    ChargeAgain(Lost),
}

/// What the navpoint points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NavKind {
    /// A crouch spot: the player is not primed.
    Crouch,
    /// A bounce strip: the player is primed.
    Strip,
}

/// The next thing to do: the nearest crouch spot while not primed; while
/// primed the chosen strip ([`nav_target`], and the predicted landing's
/// strip while falling: `superbounce_mode`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavTarget {
    /// The spot's stand point, world units.
    pub at: Vec3,
    pub kind: NavKind,
    /// From the player's origin to `at`, world units.
    pub distance: f32,
    /// A strip's `fall`, the drop to come from (world units): what the
    /// navpoint's number says over a strip. 0 for a crouch spot.
    pub fall: f32,
}

/// Whether a predicted landing is on the slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Course {
    /// The coasting landing is inside the slot.
    OnSlot,
    /// It is not, and the assist can still bring it in (its reach gate).
    Fixable,
    /// It is not, and the assist cannot.
    Miss,
}

/// The landing the assist predicts for a falling player, on the strip it
/// is falling toward.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Predicted {
    /// Where the player lands coasting, at the strip's floor height.
    pub at: Vec3,
    pub verdict: Course,
    /// The vertical speed the player will touch down at (units/s,
    /// positive): `√(vz² + 2 × 4.15 × height)`.
    pub speed: f32,
    /// The touchdown speed the strip needs ([`need_of`]).
    pub need: f32,
    /// The strip: its edge (the crease line), stand point, slot width
    /// ([`width_of`]) and fall.
    pub edge: [Vec3; 2],
    pub stand: Vec3,
    pub width: f32,
    pub fall: f32,
}

impl Predicted {
    /// How far the landing is outside the slot, horizontally: 0 inside it.
    pub fn miss(&self) -> f32 {
        (segment_distance_xy(self.edge, [self.at[0], self.at[1]]) - self.width / 2.0).max(0.0)
    }

    /// The lock (docs/game/superbounce.md, "Lock-on"): the landing is in
    /// the slot and fast enough to sink onto it.
    pub fn locked(&self) -> bool {
        self.verdict == Course::OnSlot && self.speed >= self.need
    }

    /// The point of the crease nearest the landing.
    pub fn crease_point(&self) -> Vec3 {
        let [a, b] = self.edge;
        let t = segment_t_xy(self.edge, [self.at[0], self.at[1]]);
        [
            a[0] + t * (b[0] - a[0]),
            a[1] + t * (b[1] - a[1]),
            a[2] + t * (b[2] - a[2]),
        ]
    }
}

/// What a landing by a strip came to, by its first failed test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LandingResult {
    /// The vertical speed came out over [`BOUNCE_VZ`].
    Bounce,
    /// The word was not -3 on the tick before.
    NotPrimed,
    /// The origin was outside the slot.
    Missed,
    /// The speed in was under the strip's need.
    TooSlow,
    /// Primed, in the slot, fast enough, and no bounce.
    NoBounce,
}

/// A landing filed at touchdown within [`LANDING_RADIUS`] of a strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    pub result: LandingResult,
    /// The game tick it was filed on (the tracker's own count where the
    /// game's could not be read).
    pub tick: u32,
    /// The origin on the tick before touchdown, world units.
    pub at: Vec3,
}

// ---- the strip's slot ----

/// A strip's slot width (world units): the gap between its joints, the
/// scanner's `width`; a spot from before the scanner had it (0) reads as
/// the floor ribbon's [`RIBBON_WIDTH`].
pub fn width_of(spot: &Spot) -> f32 {
    if spot.width > 0.0 { spot.width } else { RIBBON_WIDTH }
}

/// The touchdown speed a strip needs (units/s): its measured `need`;
/// else, with a measured drop (`fall`) and no need, what that fall gives
/// on top of a standing jump's landing (`superbounce::speed::touchdown`);
/// else the one threshold every seam shares,
/// `superbounce::speed::BOUNCE_SPEED`. The game's spots carry the model's
/// `need` already (`superbounce::speed::apply`), so the speed model is
/// tuned in that one place.
pub fn need_of(spot: &Spot) -> f32 {
    if spot.need > 0.0 {
        return spot.need;
    }
    if spot.fall.is_finite() && spot.fall > 0.0 {
        return superbounce::speed::touchdown(spot.fall);
    }
    superbounce::speed::BOUNCE_SPEED
}

/// Where along `edge` (0 to 1) the point nearest `xy` is, horizontally.
fn segment_t_xy([a, b]: [Vec3; 2], xy: [f32; 2]) -> f32 {
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let len2 = ex * ex + ey * ey;
    if len2 > 0.0 {
        (((xy[0] - a[0]) * ex + (xy[1] - a[1]) * ey) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The horizontal distance from `xy` to the segment `edge`.
fn segment_distance_xy(edge: [Vec3; 2], xy: [f32; 2]) -> f32 {
    let [a, b] = edge;
    let t = segment_t_xy(edge, xy);
    (a[0] + t * (b[0] - a[0]) - xy[0]).hypot(a[1] + t * (b[1] - a[1]) - xy[1])
}

/// The horizontal distance from `xy` to a strip's edge segment.
pub fn edge_distance_xy(spot: &Spot, xy: [f32; 2]) -> f32 {
    segment_distance_xy(spot.edge, xy)
}

/// How far `xy` is outside a strip's slot, horizontally: 0 inside it.
pub fn slot_miss(spot: &Spot, xy: [f32; 2]) -> f32 {
    (edge_distance_xy(spot, xy) - width_of(spot) / 2.0).max(0.0)
}

fn distance(a: Vec3, b: Vec3) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// The 3D distance from `p` to a spot's edge segment.
fn edge_distance(spot: &Spot, p: Vec3) -> f32 {
    let [a, b] = spot.edge;
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if len2 > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    distance(p, [a[0] + t * ab[0], a[1] + t * ab[1], a[2] + t * ab[2]])
}

/// The spot of `kind` whose stand point is nearest `at`, and that distance.
pub fn nearest_stand(spots: &[Spot], kind: Kind, at: Vec3) -> Option<(&Spot, f32)> {
    spots
        .iter()
        .filter(|s| s.kind == kind)
        .map(|s| (s, distance(s.stand, at)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// Whether feet at height `feet` are high enough to fall onto `strip`: it
/// needs no fall (a standing jump), or they are at least its fall above its
/// stand point, less [`HIGH_SLACK`].
pub fn high_enough(feet: f32, strip: &Spot) -> bool {
    strip.fall <= crate::superbounce_prompts::FLAT_FALL
        || feet - strip.stand[2] >= strip.fall - HIGH_SLACK
}

/// [`high_enough`] forgives this much (world units): a floor a hair under
/// the fall still makes it, as the fall is worked from a need with margin.
pub const HIGH_SLACK: f32 = 0.05;

/// The navpoint's target for a player at `at`. While `primed`, the chosen
/// strip: the nearest strip whose fall the player's floor is high enough
/// for ([`high_enough`]), else the nearest strip; the world's landing and
/// take-off marks are on the same one (`superbounce_world`). Not primed,
/// the nearest crouch spot.
pub fn nav_target(spots: &[Spot], at: Vec3, primed: bool) -> Option<NavTarget> {
    let target = |s: &Spot, distance| NavTarget {
        at: s.stand,
        kind: if primed {
            NavKind::Strip
        } else {
            NavKind::Crouch
        },
        distance,
        fall: if primed { s.fall } else { 0.0 },
    };
    if !primed {
        return nearest_stand(spots, Kind::Crouch, at).map(|(s, d)| target(s, d));
    }
    let nearest = |reachable: bool| {
        spots
            .iter()
            .filter(|s| s.kind == Kind::Bounce && (!reachable || high_enough(at[2], s)))
            .map(|s| (s, distance(s.stand, at)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    };
    nearest(true)
        .or_else(|| nearest(false))
        .map(|(s, d)| target(s, d))
}

/// The navpoint's target for a player falling onto `landing`'s strip:
/// that strip, so the navpoint, the world's marks and the lock agree.
pub fn nav_to_landing(landing: &Predicted, at: Vec3) -> NavTarget {
    NavTarget {
        at: landing.stand,
        kind: NavKind::Strip,
        distance: distance(landing.stand, at),
        fall: landing.fall,
    }
}

/// A landing's result, by the first failed test: `bounce` when the
/// vertical speed comes out over [`BOUNCE_VZ`], else `not primed` (the word
/// on the tick before was not -3), `missed` (the origin was outside the
/// slot), `too slow` (the speed in, positive, was under [`need_of`]), then
/// `no bounce`.
pub fn classify(
    spot: &Spot,
    word_before: Option<u32>,
    origin: Vec3,
    speed_in: f32,
    vz_out: f32,
) -> LandingResult {
    if vz_out > BOUNCE_VZ {
        LandingResult::Bounce
    } else if !word_before.is_some_and(primed) {
        LandingResult::NotPrimed
    } else if slot_miss(spot, [origin[0], origin[1]]) > 0.0 {
        LandingResult::Missed
    } else if speed_in < need_of(spot) {
        LandingResult::TooSlow
    } else {
        LandingResult::NoBounce
    }
}

/// Whether a body touches down this tick: airborne and falling at
/// [`TOUCHDOWN_VZ`] or faster on the tick before (`was_airborne`,
/// `was_vz`), and now stopped falling ([`LANDED_VZ`]), launched, or on
/// the ground. Anywhere on the map; [`strip_landing`] says whether it was
/// by a strip.
pub fn touchdown(was_airborne: bool, was_vz: f32, airborne: bool, vz: f32) -> bool {
    was_airborne && was_vz <= TOUCHDOWN_VZ && (vz >= LANDED_VZ || !airborne)
}

/// The index in `spots` of the strip a touchdown at `at` is by: the
/// nearest bounce spot whose edge is within [`LANDING_RADIUS`].
pub fn landing_spot(spots: &[Spot], at: Vec3) -> Option<usize> {
    spots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == Kind::Bounce)
        .map(|(i, s)| (i, edge_distance(s, at)))
        .filter(|(_, d)| *d <= LANDING_RADIUS)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// How many bounce spots have their edge within [`LANDING_RADIUS`] of `at`:
/// 1 by a single seam, more where strips meet (`superbounce_chain` keeps
/// it on a bounce's award, for the study of Monster Bounces).
pub fn strips_near(spots: &[Spot], at: Vec3) -> usize {
    spots
        .iter()
        .filter(|s| s.kind == Kind::Bounce && edge_distance(s, at) <= LANDING_RADIUS)
        .count()
}

/// A touchdown's result by the nearest strip within [`LANDING_RADIUS`] of
/// the origin on the tick before (`at`, with the word `word_before` and
/// the vertical speed `vz_before` then), coming out at `vz`; `None` with no
/// strip that near. The speed in counts the fall from `at` to the strip's
/// stand height.
pub fn strip_landing(
    spots: &[Spot],
    word_before: Option<u32>,
    at: Vec3,
    vz_before: f32,
    vz: f32,
) -> Option<LandingResult> {
    let spot = &spots[landing_spot(spots, at)?];
    let height = (at[2] - spot.stand[2]).max(0.0);
    let speed_in = (vz_before * vz_before + 2.0 * GRAVITY * height).sqrt();
    Some(classify(spot, word_before, at, speed_in, vz))
}

// ---- the tracker ----

/// What one tick reads of the local player.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tick {
    /// The game tick, when it can be read.
    pub tick: Option<u32>,
    /// The level pair's word; `None` without a pair.
    pub word: Option<u32>,
    pub position: Option<Vec3>,
    /// World units a second.
    pub velocity: Option<Vec3>,
    /// Biped flags bit 0.
    pub airborne: bool,
    /// The crouch request, unit flags bit 23.
    pub request: bool,
    /// Whether the crouch button is down in the pad the guest reads;
    /// `None` when it is not known (`superbounce_assist::crouch_held`).
    pub crouch_held: Option<bool>,
    /// Whether the standing capsule fits at the origin; `None` when it is
    /// not known (`superbounce_capsule`).
    pub standing_fits: Option<bool>,
}

/// What [`Tracker::step`] publishes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Step {
    pub charge: Option<Charge>,
    pub verb: Option<Verb>,
    pub nav_target: Option<NavTarget>,
    pub last_landing: Option<Landing>,
    /// Ticks the primed body has been still (0 while not primed).
    pub still_ticks: u32,
}

/// The previous tick, for the landing.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Prev {
    word: Option<u32>,
    position: Vec3,
    vz: f32,
    airborne: bool,
}

/// The state the charge and the landings keep from tick to tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tracker {
    /// Steps taken.
    count: u32,
    was_primed: bool,
    still: u32,
    /// The step the charge was lost on.
    lost_at: Option<u32>,
    lost: Option<Lost>,
    prev: Option<Prev>,
    last_landing: Option<Landing>,
}

impl Tracker {
    pub const fn new() -> Tracker {
        Tracker {
            count: 0,
            was_primed: false,
            still: 0,
            lost_at: None,
            lost: None,
            prev: None,
            last_landing: None,
        }
    }

    /// One tick: the charge, the verb, the navpoint and the last landing.
    pub fn step(&mut self, t: &Tick, spots: &[Spot]) -> Step {
        self.count = self.count.wrapping_add(1);
        self.landing(t, spots);
        // A crouch while primed ends the charge at once (Trace).
        let primed_now = t.word.is_some_and(primed) && !t.request;
        if primed_now {
            let still = t.velocity.is_some_and(|v| {
                let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                speed < STILL_SPEED
            });
            self.still = if still {
                self.still.saturating_add(1)
            } else {
                0
            };
            self.lost_at = None;
        } else {
            if self.was_primed {
                // `still` is still the count up to the tick before.
                let reason = if t.request {
                    LostReason::Crouched
                } else if self.still >= STOOD_STILL_TICKS {
                    LostReason::StoodStill
                } else {
                    LostReason::Unknown
                };
                self.lost_at = Some(self.count);
                self.lost = Some(Lost {
                    reason,
                    tick: t.tick.unwrap_or(self.count),
                });
            }
            self.still = 0;
        }
        self.was_primed = primed_now;
        let since_lost = self.lost_at.map(|at| self.count.wrapping_sub(at));
        let lost = since_lost.is_some_and(|n| n < LOST_TICKS);
        let near = t.position.is_some_and(|p| {
            nearest_stand(spots, Kind::Crouch, p).is_some_and(|(_, d)| d <= NEAR_RADIUS)
        });
        let phase = if primed_now {
            let left = 1.0 - self.still as f32 / SLEEP_TICKS as f32;
            Some((ChargePhase::Primed, left.clamp(0.0, 1.0)))
        } else if t.request && t.crouch_held == Some(false) {
            // The engine keeps the request with the button up only while
            // the standing capsule does not fit: its own answer.
            Some((ChargePhase::Held, 2.0 / 3.0))
        } else if t.request && t.crouch_held != Some(false) && t.standing_fits == Some(false) {
            Some((ChargePhase::Under, 1.0 / 3.0))
        } else if lost {
            Some((ChargePhase::Lost, 0.0))
        } else if t.request && near {
            Some((ChargePhase::Near, 0.0))
        } else {
            None
        };
        let charge = phase.map(|(phase, fill)| Charge {
            phase,
            fill,
            lost: self.lost.filter(|_| phase == ChargePhase::Lost),
        });
        let verb = match charge.map(|c| c.phase) {
            Some(ChargePhase::Under) => Some(Verb::LetGoOfCrouch),
            Some(ChargePhase::Held) => Some(Verb::WalkOutToStand),
            Some(ChargePhase::Primed) if self.still >= WARN_STILL_TICKS => Some(Verb::KeepMoving),
            // Near is the HUD's own step ("hold (crouch) and walk under").
            Some(ChargePhase::Lost) | None if since_lost.is_some_and(|n| n < AGAIN_TICKS) => {
                self.lost.map(Verb::ChargeAgain)
            }
            _ => None,
        };
        Step {
            charge,
            verb,
            nav_target: t.position.and_then(|p| nav_target(spots, p, primed_now)),
            last_landing: self.last_landing,
            still_ticks: self.still,
        }
    }

    /// Files a landing when the player touches down by a strip: falling at
    /// [`TOUCHDOWN_VZ`] or faster on the tick before, and now stopped
    /// ([`LANDED_VZ`]), launched, or on the ground.
    fn landing(&mut self, t: &Tick, spots: &[Spot]) {
        let now = match (t.position, t.velocity) {
            (Some(p), Some(v)) if v[2].is_finite() => Some((p, v[2])),
            _ => None,
        };
        if let (Some(prev), Some((_, vz))) = (self.prev, now)
            && touchdown(prev.airborne, prev.vz, t.airborne, vz)
            && let Some(result) = strip_landing(spots, prev.word, prev.position, prev.vz, vz)
        {
            self.last_landing = Some(Landing {
                result,
                tick: t.tick.unwrap_or(self.count),
                at: prev.position,
            });
        }
        self.prev = now.map(|(position, vz)| Prev {
            word: t.word,
            position,
            vz,
            airborne: t.airborne,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUS_3: u32 = 0xffff_fffd;

    fn spot(kind: Kind, stand: Vec3, fall: f32) -> Spot {
        Spot {
            kind,
            edge: [
                [stand[0], stand[1] - 1.0, stand[2]],
                [stand[0], stand[1] + 1.0, stand[2]],
            ],
            stand,
            fall,
            launch: if kind == Kind::Bounce { 17.9 } else { 0.0 },
            drift: [0.0, 0.0],
            score: 1.0,
            ..Spot::default()
        }
    }

    /// A crouch spot at the origin and a strip 10 units along +X.
    fn spots() -> Vec<Spot> {
        vec![
            spot(Kind::Crouch, [0.0, 0.0, 0.0], 0.0),
            spot(Kind::Bounce, [10.0, 0.0, -3.0], 1.0),
        ]
    }

    fn standing(at: Vec3) -> Tick {
        Tick {
            tick: None,
            word: Some(0),
            position: Some(at),
            velocity: Some([0.0; 3]),
            airborne: false,
            request: false,
            crouch_held: Some(false),
            standing_fits: Some(true),
        }
    }

    fn phase(s: &Step) -> Option<ChargePhase> {
        s.charge.map(|c| c.phase)
    }

    #[test]
    fn the_recipe_steps_through_near_under_held_primed_then_drains_and_is_lost() {
        let spots = spots();
        let mut t = Tracker::new();
        let at = [0.5, 0.0, 0.0];
        // Standing by the spot: nothing yet.
        assert_eq!(t.step(&standing(at), &spots).charge, None);
        // Crouched by it, the standing capsule fits: near.
        let crouched = Tick {
            request: true,
            crouch_held: Some(true),
            ..standing(at)
        };
        let s = t.step(&crouched, &spots);
        assert_eq!(
            s.charge,
            Some(Charge {
                phase: ChargePhase::Near,
                fill: 0.0,
                lost: None,
            })
        );
        assert_eq!(s.verb, None);
        // Under the lip: under, and the verb is to let go.
        let under = Tick {
            standing_fits: Some(false),
            ..crouched
        };
        let s = t.step(&under, &spots);
        assert_eq!(phase(&s), Some(ChargePhase::Under));
        assert!((s.charge.unwrap().fill - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(s.verb, Some(Verb::LetGoOfCrouch));
        // The button up and the request kept: held, walk out.
        let held = Tick {
            crouch_held: Some(false),
            ..under
        };
        let s = t.step(&held, &spots);
        assert_eq!(phase(&s), Some(ChargePhase::Held));
        assert!((s.charge.unwrap().fill - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(s.verb, Some(Verb::WalkOutToStand));
        // Out and standing, the word at -3: primed, a still tick gone.
        let primed = Tick {
            word: Some(MINUS_3),
            ..standing(at)
        };
        let s = t.step(&primed, &spots);
        assert_eq!(phase(&s), Some(ChargePhase::Primed));
        assert!((s.charge.unwrap().fill - (1.0 - 1.0 / SLEEP_TICKS as f32)).abs() < 1e-6);
        assert_eq!((s.verb, s.still_ticks), (None, 1));
        // Still for 38 ticks in all: the fill reaches 0, and from 19 the
        // prompt line warns.
        let mut last = s;
        for i in 1..SLEEP_TICKS {
            last = t.step(&primed, &spots);
            let warned = i + 1 >= WARN_STILL_TICKS;
            assert_eq!(last.verb == Some(Verb::KeepMoving), warned, "{i}");
        }
        assert_eq!(last.still_ticks, SLEEP_TICKS);
        assert_eq!(last.charge.unwrap().fill, 0.0);
        // Moving puts the count back.
        let moving = Tick {
            velocity: Some([1.0, 0.0, 0.0]),
            ..primed
        };
        let s = t.step(&moving, &spots);
        assert_eq!((s.still_ticks, s.charge.unwrap().fill), (0, 1.0));
        assert_eq!(s.verb, None);
        // The word leaves -3 while moving: lost for a second, then
        // hidden, for no reason the tracker knows; the prompt line asks
        // for a new charge for 5 s.
        let woke = standing([5.0, 0.0, 0.0]);
        let first = t.step(&woke, &spots);
        let lost = first.charge.unwrap().lost.unwrap();
        assert_eq!(lost.reason, LostReason::Unknown);
        for i in 1..LOST_TICKS {
            let s = t.step(&woke, &spots);
            assert_eq!(
                s.charge,
                Some(Charge {
                    phase: ChargePhase::Lost,
                    fill: 0.0,
                    lost: Some(lost),
                }),
                "{i}"
            );
            assert_eq!(s.verb, Some(Verb::ChargeAgain(lost)), "{i}");
        }
        for i in LOST_TICKS..AGAIN_TICKS {
            let s = t.step(&woke, &spots);
            assert_eq!(s.charge, None, "{i}");
            assert_eq!(s.verb, Some(Verb::ChargeAgain(lost)), "{i}");
        }
        assert_eq!(t.step(&woke, &spots).verb, None);
    }

    /// A primed body still for `still` ticks, then the word gone.
    fn lost_after(still: u32) -> (Tracker, Step) {
        let spots = spots();
        let mut t = Tracker::new();
        let primed = Tick {
            tick: Some(100),
            word: Some(MINUS_3),
            ..standing([5.0, 0.0, 0.0])
        };
        for _ in 0..still {
            t.step(&primed, &spots);
        }
        let slept = Tick {
            tick: Some(100 + still),
            word: None,
            ..primed
        };
        let s = t.step(&slept, &spots);
        (t, s)
    }

    #[test]
    fn a_sleep_after_standing_still_is_lost_to_standing_still() {
        let (_, s) = lost_after(SLEEP_TICKS);
        assert_eq!(phase(&s), Some(ChargePhase::Lost));
        let lost = s.charge.unwrap().lost.unwrap();
        assert_eq!(
            lost,
            Lost {
                reason: LostReason::StoodStill,
                tick: 100 + SLEEP_TICKS
            }
        );
        assert_eq!(s.verb, Some(Verb::ChargeAgain(lost)));
        assert_eq!(s.still_ticks, 0);
        // From STOOD_STILL_TICKS still ticks it is standing still; under,
        // the tracker does not know.
        let reason = |still| lost_after(still).1.charge.unwrap().lost.unwrap().reason;
        assert_eq!(reason(STOOD_STILL_TICKS), LostReason::StoodStill);
        assert_eq!(reason(STOOD_STILL_TICKS - 1), LostReason::Unknown);
        assert_eq!(reason(1), LostReason::Unknown);
    }

    #[test]
    fn a_warning_shows_from_half_the_sleep_and_lines_up_with_half_the_fill() {
        assert_eq!(WARN_STILL_TICKS, 19);
        assert_eq!(1.0 - WARN_STILL_TICKS as f32 / SLEEP_TICKS as f32, 0.5);
        // Moving clears it.
        let spots = spots();
        let mut t = Tracker::new();
        let primed = Tick {
            word: Some(MINUS_3),
            ..standing([5.0, 0.0, 0.0])
        };
        for _ in 0..WARN_STILL_TICKS {
            t.step(&primed, &spots);
        }
        assert_eq!(t.step(&primed, &spots).verb, Some(Verb::KeepMoving));
        let moving = Tick {
            velocity: Some([1.0, 0.0, 0.0]),
            ..primed
        };
        assert_eq!(t.step(&moving, &spots).verb, None);
    }

    #[test]
    fn a_recipe_step_takes_the_line_from_the_new_charge_hint() {
        let (mut t, s) = lost_after(1);
        assert!(matches!(s.verb, Some(Verb::ChargeAgain(_))));
        let spots = spots();
        // Crouched under a lip: the step's own verb, and the lost meter
        // gives way to under.
        let under = Tick {
            request: true,
            crouch_held: Some(true),
            standing_fits: Some(false),
            ..standing([0.0; 3])
        };
        let s = t.step(&under, &spots);
        assert_eq!(phase(&s), Some(ChargePhase::Under));
        assert_eq!(s.verb, Some(Verb::LetGoOfCrouch));
        assert_eq!(s.charge.unwrap().lost, None);
        // Crouched by a spot with room to stand, after the lost second:
        // near, which the HUD gives its own verb.
        let near = Tick {
            standing_fits: Some(true),
            ..under
        };
        for _ in 0..LOST_TICKS {
            t.step(&near, &spots);
        }
        let s = t.step(&near, &spots);
        assert_eq!((phase(&s), s.verb), (Some(ChargePhase::Near), None));
    }

    #[test]
    fn a_crouch_while_primed_loses_the_charge_at_once() {
        let spots = spots();
        let mut t = Tracker::new();
        let primed = Tick {
            word: Some(MINUS_3),
            velocity: Some([1.0, 0.0, 0.0]),
            ..standing([5.0, 0.0, 0.0])
        };
        assert_eq!(phase(&t.step(&primed, &spots)), Some(ChargePhase::Primed));
        let crouch = Tick {
            request: true,
            crouch_held: Some(true),
            ..primed
        };
        let s = t.step(&crouch, &spots);
        assert_eq!(phase(&s), Some(ChargePhase::Lost));
        let lost = s.charge.unwrap().lost.unwrap();
        assert_eq!(lost.reason, LostReason::Crouched);
        assert_eq!(s.verb, Some(Verb::ChargeAgain(lost)));
        // Crouched while still a long while: still a crouch.
        let mut t = Tracker::new();
        let still = Tick {
            velocity: Some([0.0; 3]),
            ..primed
        };
        for _ in 0..STOOD_STILL_TICKS {
            t.step(&still, &spots);
        }
        let crouch = Tick {
            request: true,
            crouch_held: Some(true),
            ..still
        };
        let s = t.step(&crouch, &spots);
        assert_eq!(phase(&s), Some(ChargePhase::Lost));
        assert_eq!(s.charge.unwrap().lost.unwrap().reason, LostReason::Crouched);
    }

    #[test]
    fn under_needs_the_capsule_test_and_held_the_button() {
        let spots = spots();
        let mut t = Tracker::new();
        // Without the button, a request where standing does not fit is
        // under; it never reads as held.
        let unknown = Tick {
            request: true,
            crouch_held: None,
            standing_fits: Some(false),
            ..standing([0.0; 3])
        };
        assert_eq!(phase(&t.step(&unknown, &spots)), Some(ChargePhase::Under));
        // Without the capsule test, held still shows from the button.
        let held = Tick {
            crouch_held: Some(false),
            standing_fits: None,
            ..unknown
        };
        assert_eq!(phase(&t.step(&held, &spots)), Some(ChargePhase::Held));
        // Crouched far from any crouch spot with room to stand: nothing.
        let far = Tick {
            request: true,
            crouch_held: Some(true),
            ..standing([5.0, 5.0, 0.0])
        };
        assert_eq!(t.step(&far, &spots).charge, None);
    }

    #[test]
    fn the_navpoint_is_the_crouch_spot_until_primed_then_the_strip() {
        let spots = spots();
        let mut t = Tracker::new();
        let at = [4.0, 0.0, 0.0];
        let n = t.step(&standing(at), &spots).nav_target.unwrap();
        assert_eq!((n.kind, n.at), (NavKind::Crouch, [0.0, 0.0, 0.0]));
        assert!((n.distance - 4.0).abs() < 1e-6);
        let primed = Tick {
            word: Some(MINUS_3),
            ..standing(at)
        };
        let n = t.step(&primed, &spots).nav_target.unwrap();
        assert_eq!((n.kind, n.at), (NavKind::Strip, [10.0, 0.0, -3.0]));
        assert!((n.distance - 45f32.sqrt()).abs() < 1e-5);
        // A crouch while primed points back at the crouch spot.
        let crouch = Tick {
            request: true,
            ..primed
        };
        assert_eq!(
            t.step(&crouch, &spots).nav_target.unwrap().kind,
            NavKind::Crouch
        );
        // No position, or no spot of the kind: no target.
        assert_eq!(t.step(&Tick::default(), &spots).nav_target, None);
        assert_eq!(nav_target(&spots[..1], at, true), None);
    }

    /// Primed, the navpoint is over the nearest strip the player's floor
    /// is high enough for, with its fall; with none high enough, the
    /// nearest strip.
    #[test]
    fn the_primed_navpoint_prefers_a_strip_the_floor_can_fall_onto() {
        let spots = vec![
            // Near, but its fall is 2 and the floor is 1 above it.
            spot(Kind::Bounce, [1.0, 0.0, -1.0], 2.0),
            // Further, 3 below with a fall of 1.
            spot(Kind::Bounce, [6.0, 0.0, -3.0], 1.0),
        ];
        let n = nav_target(&spots, [0.0; 3], true).unwrap();
        assert_eq!((n.at, n.fall), ([6.0, 0.0, -3.0], 1.0));
        // From lower down neither is high enough: the nearest.
        let n = nav_target(&spots, [0.0, 0.0, -2.5], true).unwrap();
        assert_eq!((n.at, n.fall), ([1.0, 0.0, -1.0], 2.0));
        // A crouch spot's fall is 0.
        let crouch = nav_target(&self::spots(), [0.0; 3], false).unwrap();
        assert_eq!(crouch.fall, 0.0);
        let at = |z| spot(Kind::Bounce, [0.0, 0.0, z], 1.0);
        assert!(high_enough(0.0, &at(-0.96)));
        assert!(!high_enough(0.0, &at(-0.9)));
        assert!(high_enough(-5.0, &spot(Kind::Bounce, [0.0; 3], 0.0)));
    }

    fn predicted(at: Vec3, verdict: Course, speed: f32) -> Predicted {
        Predicted {
            at,
            verdict,
            speed,
            need: 5.0,
            edge: [[0.0, -1.0, 0.0], [0.0, 1.0, 0.0]],
            stand: [0.0; 3],
            width: 0.2,
            fall: 1.0,
        }
    }

    /// The lock is in the slot and fast enough; the miss is the distance
    /// outside the slot; the crease point is the edge's nearest.
    #[test]
    fn a_landing_locks_in_the_slot_at_its_need() {
        let on = predicted([0.05, 0.5, 0.0], Course::OnSlot, 6.0);
        assert!(on.locked());
        assert_eq!(on.miss(), 0.0);
        assert_eq!(on.crease_point(), [0.0, 0.5, 0.0]);
        assert!(!predicted([0.05, 0.5, 0.0], Course::OnSlot, 4.0).locked());
        let off = predicted([0.6, 3.0, 0.0], Course::Miss, 6.0);
        assert!(!off.locked());
        // Past the end: the crease's end point.
        assert_eq!(off.crease_point(), [0.0, 1.0, 0.0]);
        assert!((off.miss() - (0.6f32.hypot(2.0) - 0.1)).abs() < 1e-5);
        // Falling onto it, the navpoint is that strip with its fall.
        let n = nav_to_landing(&on, [0.0, 0.0, 3.0]);
        assert_eq!((n.at, n.kind, n.fall), ([0.0; 3], NavKind::Strip, 1.0));
        assert!((n.distance - 3.0).abs() < 1e-6);
    }

    #[test]
    fn landings_are_classified_by_their_first_failed_test() {
        let strip = spot(Kind::Bounce, [10.0, 0.0, -3.0], 1.0);
        let need = need_of(&strip);
        assert!((need - (9.0 + 2.0 * 4.15f32).sqrt()).abs() < 1e-5);
        let on = [10.0, 0.0, -3.0];
        let off = [10.3, 0.0, -3.0];
        use LandingResult::*;
        assert_eq!(classify(&strip, Some(0), off, 1.0, 12.0), Bounce);
        assert_eq!(classify(&strip, Some(0), on, 9.0, -0.04), NotPrimed);
        assert_eq!(classify(&strip, None, on, 9.0, -0.04), NotPrimed);
        assert_eq!(classify(&strip, Some(MINUS_3), off, 9.0, -0.04), Missed);
        assert_eq!(
            classify(&strip, Some(MINUS_3), on, need - 0.1, -0.04),
            TooSlow
        );
        assert_eq!(
            classify(&strip, Some(MINUS_3), on, need + 0.1, -0.04),
            NoBounce
        );
        // Inside the slot is within half its width of the edge.
        assert_eq!(slot_miss(&strip, [10.09, 0.5]), 0.0);
        assert!((slot_miss(&strip, [10.3, 0.5]) - 0.2).abs() < 1e-5);
        // A strip with no measured need or drop needs the one threshold
        // every seam shares.
        let bare = spot(Kind::Bounce, on, 0.0);
        assert_eq!(need_of(&bare), superbounce::speed::BOUNCE_SPEED);
        assert_eq!(classify(&bare, Some(MINUS_3), on, 5.0, -0.04), TooSlow);
        assert_eq!(classify(&bare, Some(MINUS_3), on, 5.5, -0.04), NoBounce);
    }

    #[test]
    fn a_touchdown_by_a_strip_files_the_landing_once() {
        let spots = spots();
        let mut t = Tracker::new();
        let fall = |z: f32, vz: f32| Tick {
            tick: Some(6752),
            word: Some(MINUS_3),
            position: Some([10.0, 0.0, z]),
            velocity: Some([0.0, 0.0, vz]),
            airborne: true,
            ..Tick::default()
        };
        assert_eq!(t.step(&fall(-2.8, -5.2), &spots).last_landing, None);
        assert_eq!(t.step(&fall(-2.98, -5.4), &spots).last_landing, None);
        // The launch tick: up at +12.45.
        let bounce = Tick {
            tick: Some(6753),
            ..fall(-2.9, 12.45)
        };
        let l = t.step(&bounce, &spots).last_landing.unwrap();
        assert_eq!(l.result, LandingResult::Bounce);
        assert_eq!((l.tick, l.at), (6753, [10.0, 0.0, -2.98]));
        // Rising, it is not filed again.
        let up = Tick {
            tick: Some(6754),
            ..fall(-2.5, 12.0)
        };
        assert_eq!(t.step(&up, &spots).last_landing.unwrap().tick, 6753);
        // An unprimed landing on the ground files `not primed`.
        let mut t = Tracker::new();
        let unprimed = |z: f32, vz: f32, airborne: bool| Tick {
            word: Some(0),
            airborne,
            ..fall(z, vz)
        };
        t.step(&unprimed(-2.98, -5.4, true), &spots);
        let l = t
            .step(&unprimed(-3.0, -0.04, false), &spots)
            .last_landing
            .unwrap();
        assert_eq!(l.result, LandingResult::NotPrimed);
        // A landing away from every strip files nothing.
        let mut t = Tracker::new();
        let away = |vz: f32, airborne: bool| Tick {
            position: Some([20.0, 0.0, -3.0]),
            ..unprimed(-3.0, vz, airborne)
        };
        t.step(&away(-5.4, true), &spots);
        assert_eq!(t.step(&away(0.0, false), &spots).last_landing, None);
    }
}
