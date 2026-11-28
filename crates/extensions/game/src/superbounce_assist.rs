//! The superbounce fall assist: hold X while falling and the stick is
//! steered so the player lands on a bounce strip (docs/game/superbounce.md,
//! "Hold X: the fall assist").
//!
//! It works on input, not on physics. An input tap ([`AssistTap`]) sits at
//! `xapi`'s `XInputGetState`, under the agent's tap and over a recording's or
//! replay's. Each read for the first controller it checks one atomic
//! ([`superbounce_mode::active`]); only with the mode on, X held and the
//! player in the air does it read the player's body, pick a strip, and
//! replace the left stick in the packet the guest is about to read, and
//! take X out of it. It changes the packet before the tap under it sees it,
//! so a film or input log records the pad the guest got, and a replay (which
//! overwrites the packet) never runs the assist. The [`SuperbounceAssist`]
//! feature has no guest hooks: nothing about it enters a film's hook digest.
//!
//! The rules are pure functions over a [`Body`], the scan's spots and a
//! [`Ground`] (the map's collision, the HUD's mark and the last read's
//! strip), on the shared flight model (`superbounce_flight`): [`judge`]
//! (the gate, with the point of the strip to land on, [`stand_near`]),
//! [`choose`] (which strip, and why), the stick (the flight model's
//! pursuit of the aim, `superbounce_flight::pursue`), [`world_to_stick`]
//! and [`apply`] (the stick and the button). `superbounce.assist` shows
//! every strip judged and the reason ([`Report`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use guest_layout::memory::{GuestMemory, MemFault};
use input::gamepad::{AnalogButton, Gamepad};
use superbounce::{Kind, Spot, Vec3};
use xapi::tap::{InputTap, PACKET};

use crate::superbounce_flight as flight;
use crate::superbounce_mode;

/// The feature's name (`H2_FEATURES` switches it).
pub const FEATURE: &str = "superbounce-assist";

// ---- what the player holds ----

/// The button that asks for the assist. Pad X; on the keyboard that is E
/// (`crates/platform/input/src/bindings.rs`, a rebinding follows), which
/// the keyboard backend merges into the same pad packet.
pub const BUTTON: AnalogButton = AnalogButton::X;
/// A face button's pressure at which it counts as held (XInput's trigger
/// threshold).
pub const PRESSED: u8 = 30;
/// The controller port the assist works for: the first local player.
pub const PORT: u32 = 0;

// ---- the numbers ----

/// The strip's stand point must be within this horizontal distance (world
/// units).
pub const REACH_RADIUS: f32 = 3.0;
/// Aim this far before the stand point along the drift (world units), on
/// a strip with no measured landing ([`aim_at`]).
pub const AIM_BACK: f32 = 0.1;
/// The least speed along a strip's drift a landing wants (units/s): the
/// slowest of the recorded bounces, which all came down moving across the
/// seam along the drift at 0.3 to 1.3 ([`wants_drift`]).
pub const DRIFT_MIN: f32 = 0.3;
/// A walking approach's speed (units/s): the run a takeoff is solved for
/// when no run measured its own (`superbounce_guide::WALK_UP`).
pub const ARRIVE_SPEED: f32 = 0.8;
/// Gravity (units/s²): the 4.15 that fits the traced jump, the speed
/// model's (`superbounce::speed::GRAVITY`). The landings themselves are
/// flown on `superbounce_flight`'s.
pub const GRAVITY: f32 = superbounce::speed::GRAVITY;
/// Slack on the reach gate (units): the assist steers onto a strip whose
/// aim its pursuit (`superbounce_flight::pursue`) ends within this of, so
/// a stick a little weaker than the model's still gets the steering.
pub const HOLD_SLACK: f32 = 0.4;
/// The assist only works on a player falling, not one still rising
/// (units/s of vertical speed).
pub const FALLING_VZ: f32 = 0.5;
/// A strip this far below the player (units) or less is landed on.
pub const MIN_DROP: f32 = 0.02;
/// Falls longer than this (seconds) are not steered.
pub const MAX_TIME: f32 = 3.0;
/// The point landed on stays this far inside a strip's ends (world units):
/// the capsule's radius.
pub const END_MARGIN: f32 = 0.175;
/// A floor this far above a strip's point (world units), between it and the
/// player, covers it: a step or a kerb does not, a slab or a ledge does.
pub const COVER: f32 = 0.3;
/// How far over the player's feet a covering floor may be (world units):
/// the ledge just left is still over a strip tucked under it.
pub const COVER_TOP: f32 = 0.5;
/// The sight line to a strip runs from this far over the player's origin to
/// this far over the strip's point (world units).
pub const SIGHT_FROM: f32 = 0.3;
pub const SIGHT_TO: f32 = 0.25;
/// Strips whose edge is within this distance sideways are judged and
/// reported (world units).
pub const CONSIDER: f32 = 6.0;
/// `superbounce.assist` lists at most this many of them.
pub const REPORTED: usize = 12;
/// The aim vector's horizontal length below which a yaw means nothing
/// (looking straight up or down).
const MIN_AIM: f32 = 0.05;
/// Where an object's position is, in its body.
const POSITION: u32 = crate::telemetry::layout::BODY_POSITION;
/// A biped's unit kind in [`crate::skulls_custom::LocalUnit`].
const BIPED_KIND: u8 = 0;

// ---- the pure rules ----

/// What the assist needs of the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// The origin (object `+0x64`), the frame of the spots' heights.
    pub position: Vec3,
    /// World units a second.
    pub velocity: Vec3,
    /// The aim's yaw (radians, counter-clockwise from +X): the stick's
    /// forward.
    pub yaw: f32,
    pub airborne: bool,
    /// The stick the player holds, world axes, 0 to 1
    /// (`superbounce_flight::held`): the tap's is the pad's turned by the
    /// yaw, the mode's the unit's throttle.
    pub stick: [f32; 2],
}

/// The flight of `body` onto the flat height `z` with its stick held as
/// it is (`superbounce_flight::hold`).
fn held_onto(body: &Body, z: f32) -> Option<flight::Landing> {
    flight::hold(body.position, body.velocity, body.stick, |_| z)
}

/// The touchdown speed (units/s down) of `body`'s flight onto the height
/// `z`, on the flight model; 0 when it never comes down there.
pub fn touchdown_speed(body: &Body, z: f32) -> f32 {
    flight::fall(body.position[2], body.velocity[2], z).map_or(0.0, |l| l.speed)
}

/// The highest the flight gets: the body's height, and while it rises
/// the apex.
pub fn apex(body: &Body) -> f32 {
    let vz = body.velocity[2].max(0.0);
    body.position[2] + vz * vz / (2.0 * flight::GRAVITY)
}

/// A drift as a unit vector; zero stays zero.
pub fn drift_unit(drift: [f32; 2]) -> [f32; 2] {
    let n = drift[0].hypot(drift[1]);
    if n > 1e-6 && n.is_finite() {
        [drift[0] / n, drift[1] / n]
    } else {
        [0.0; 2]
    }
}

/// Why a strip can be steered onto, and where to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    /// Seconds to the last free row over the strip's height.
    pub time: f32,
    /// The point to cross ([`aim_at`]): the strip's point
    /// ([`stand_near`]) when it is a measured landing, else [`AIM_BACK`]
    /// before it along the drift.
    pub aim: [f32; 2],
    /// Where the player lands with the stick held as it is.
    pub landing: [f32; 2],
    /// How far that is from [`Reach::aim`].
    pub miss: f32,
    /// The stick (world axes) the pursuit of the aim holds now
    /// (`superbounce_flight::pursue`).
    pub stick: [f32; 2],
    /// How far from the aim the pursuit comes down: what the stick can
    /// not make up.
    pub short: f32,
}

/// What the choice knows besides the spots: the map's collision, the strip
/// the HUD marks and the strip the last read steered onto.
#[derive(Clone, Copy)]
pub struct Ground<'a> {
    /// The highest floor under a point (X and Y) at or below a height
    /// (`superbounce_capsule::Room::floor_at`).
    pub floor: &'a dyn Fn([f32; 2], f32) -> Option<f32>,
    /// Whether nothing solid stands between two points
    /// (`superbounce::view::LineOfSight`).
    pub clear: &'a dyn Fn(Vec3, Vec3) -> bool,
    /// The stand point of the strip the HUD marks: the navpoint's, the
    /// world marker's and the lock-on's ([`set_marked`]).
    pub marked: Option<Vec3>,
    /// The stand point of the strip the last read steered onto.
    pub kept: Option<Vec3>,
}

fn no_floor(_: [f32; 2], _: f32) -> Option<f32> {
    None
}

fn all_clear(_: Vec3, _: Vec3) -> bool {
    true
}

impl Ground<'static> {
    /// No collision, no mark and nothing kept: the spots alone.
    pub fn open() -> Ground<'static> {
        Ground {
            floor: &no_floor,
            clear: &all_clear,
            marked: None,
            kept: None,
        }
    }
}

/// Why a strip is not steered onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// It is not below the player ([`MIN_DROP`]).
    Above,
    /// The flight does not come down to its height within [`MAX_TIME`].
    Long,
    /// Its nearest point is further sideways than [`REACH_RADIUS`].
    Far,
    /// The stick cannot bring the landing onto it in the time left
    /// (the pursuit ends more than [`HOLD_SLACK`] from the aim).
    OutOfReach,
    /// A floor is between the player and it ([`COVER`]): it is under a
    /// slab, a bridge or the ledge the player left.
    Covered,
    /// Something solid stands between the player and it.
    Walled,
}

impl Refusal {
    /// As `superbounce.assist` names it.
    pub fn name(self) -> &'static str {
        match self {
            Refusal::Above => "above",
            Refusal::Long => "long",
            Refusal::Far => "far",
            Refusal::OutOfReach => "out_of_reach",
            Refusal::Covered => "covered",
            Refusal::Walled => "walled",
        }
    }
}

/// Whether the assist works on `body` at all: in the air and falling
/// ([`FALLING_VZ`]).
pub fn falling(body: &Body) -> bool {
    body.airborne && body.velocity[2] <= FALLING_VZ
}

/// The point of a strip's stand line nearest `xy`: the stand point moved
/// along the strip's edge, kept [`END_MARGIN`] inside its ends. A strip
/// bounces along its whole length, so this is where on it to land; the
/// stand point itself for a strip too short to have a choice.
pub fn stand_near(spot: &Spot, xy: [f32; 2]) -> Vec3 {
    let [a, b] = spot.edge;
    let e = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = e[0].hypot(e[1]);
    if !len.is_finite() || len <= 2.0 * END_MARGIN || !xy.iter().all(|v| v.is_finite()) {
        return spot.stand;
    }
    let along = |p: [f32; 2]| ((p[0] - a[0]) * e[0] + (p[1] - a[1]) * e[1]) / (len * len);
    let at = |t: f32| [a[0] + t * e[0], a[1] + t * e[1], a[2] + t * e[2]];
    // The stand point's place across the strip, kept.
    let on = at(along([spot.stand[0], spot.stand[1]]).clamp(0.0, 1.0));
    let off = [
        spot.stand[0] - on[0],
        spot.stand[1] - on[1],
        spot.stand[2] - on[2],
    ];
    let margin = END_MARGIN / len;
    let p = at(along([xy[0] - off[0], xy[1] - off[1]]).clamp(margin, 1.0 - margin));
    [p[0] + off[0], p[1] + off[1], p[2] + off[2]]
}

/// The point to come down on, X and Y, for `point` of `spot`'s stand line
/// ([`stand_near`]): the one landing point the assist steers to and the
/// readout's line-up points at (docs/game/superbounce.md, "The landing
/// point").
///
/// - A measured landing (`Spot::landed`: the verified file's, or a
///   learned line's aim) is the point as it is.
/// - A scan's stand is on the edge: the point is [`AIM_BACK`] before it
///   along the drift, the side the landing comes from. A V's arm has its
///   own drift, away from the side it is approached from, so each arm's
///   point is on its approach side.
pub fn aim_at(spot: &Spot, point: Vec3) -> [f32; 2] {
    if spot.landed {
        return [point[0], point[1]];
    }
    let dir = drift_unit(spot.drift);
    [point[0] - AIM_BACK * dir[0], point[1] - AIM_BACK * dir[1]]
}

/// The speed along `spot`'s drift of a body moving `v` (X and Y), units a
/// second; 0 for a strip with no drift.
pub fn across(spot: &Spot, v: [f32; 2]) -> f32 {
    let dir = drift_unit(spot.drift);
    v[0] * dir[0] + v[1] * dir[1]
}

/// Whether a landing on a strip with the drift `drift`, coming down at
/// `across` along it, still wants a push that way: the strip has a drift
/// and the speed along it is under [`DRIFT_MIN`] (still, or moving
/// against it).
pub fn wants_drift(drift: [f32; 2], across: f32) -> bool {
    drift_unit(drift) != [0.0; 2] && (across.is_nan() || across < DRIFT_MIN)
}

/// The reach cone for a falling `body` ([`falling`]) and a bounce spot:
/// the point of the strip to land on and how to get there, or why not.
///
/// Each is flown on the flight model (`superbounce_flight`):
///
/// - the strip is below the player, and the flight comes down to it
///   within [`MAX_TIME`];
/// - the strip's point nearest the landing with the stick held as it is
///   ([`stand_near`]) is within [`REACH_RADIUS`] sideways;
/// - the pursuit of the aim (`superbounce_flight::pursue`: the air stick
///   at its measured 1.05 units/s² toward at most 1.2 units/s) ends
///   within [`HOLD_SLACK`] of it;
/// - no floor is between the player and that point ([`covered`]), and
///   nothing solid stands between them.
pub fn judge(body: &Body, spot: &Spot, ground: &Ground<'_>) -> Result<(Vec3, Reach), Refusal> {
    let z = spot.stand[2];
    let drop = body.position[2] - z;
    if drop.is_nan() || drop < MIN_DROP {
        return Err(Refusal::Above);
    }
    let Some(held) = held_onto(body, z).filter(|l| l.ticks as f32 * flight::TICK <= MAX_TIME)
    else {
        return Err(Refusal::Long);
    };
    let time = held.ticks as f32 * flight::TICK;
    let landing = [held.row[0], held.row[1]];
    let point = stand_near(spot, landing);
    let side = (point[0] - body.position[0]).hypot(point[1] - body.position[1]);
    if side.is_nan() || side > REACH_RADIUS {
        return Err(Refusal::Far);
    }
    let aim = aim_at(spot, point);
    let miss = (aim[0] - landing[0]).hypot(aim[1] - landing[1]);
    let Some(pursuit) = flight::pursue(body.position, body.velocity, aim, |_| z)
        .filter(|p| !p.miss.is_nan() && p.miss <= HOLD_SLACK)
    else {
        return Err(Refusal::OutOfReach);
    };
    if covered(body.position[2], point, ground) {
        return Err(Refusal::Covered);
    }
    let lift = |p: Vec3, by: f32| [p[0], p[1], p[2] + by];
    if !(ground.clear)(lift(body.position, SIGHT_FROM), lift(point, SIGHT_TO)) {
        return Err(Refusal::Walled);
    }
    Ok((
        point,
        Reach {
            time,
            aim,
            landing,
            miss,
            stick: pursuit.stick_now,
            short: pursuit.miss,
        },
    ))
}

/// Whether a floor is between a player whose feet are at height `feet` and
/// `point` on a strip: the highest floor under the point, up to
/// [`COVER_TOP`] over the feet, is more than [`COVER`] above it.
pub fn covered(feet: f32, point: Vec3, ground: &Ground<'_>) -> bool {
    (ground.floor)([point[0], point[1]], feet + COVER_TOP).is_some_and(|z| z > point[2] + COVER)
}

/// [`judge`] with the spots alone ([`Ground::open`]): whether `body` can be
/// steered onto `spot`'s strip before it lands. `None` for a crouch spot or
/// a body that is not falling.
pub fn reach(body: &Body, spot: &Spot) -> Option<Reach> {
    if spot.kind != Kind::Bounce || !falling(body) {
        return None;
    }
    judge(body, spot, &Ground::open()).ok().map(|(_, r)| r)
}

/// What the choice says of one strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    /// The strip steered onto.
    Chosen,
    /// In reach, and another was chosen.
    Passed,
    Refused(Refusal),
}

impl Verdict {
    /// As `superbounce.assist` names it.
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Chosen => "chosen",
            Verdict::Passed => "passed",
            Verdict::Refused(r) => r.name(),
        }
    }

    /// Chosen first, then the others in reach, then the refused.
    fn rank(self) -> u8 {
        match self {
            Verdict::Chosen => 0,
            Verdict::Passed => 1,
            Verdict::Refused(_) => 2,
        }
    }
}

/// One strip the choice looked at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// Its place in the spots.
    pub index: usize,
    /// The strip's stand point.
    pub stand: Vec3,
    /// From the player to the strip's edge, sideways.
    pub distance: f32,
    /// The touchdown speed the fall to it gives, and the speed it needs
    /// (units/s, positive).
    pub speed: f32,
    pub need: f32,
    /// In reach: the point of the strip to land on, the seconds to it and
    /// how far the landing with the stick held misses the aim.
    pub point: Option<Vec3>,
    pub time: Option<f32>,
    pub miss: Option<f32>,
    pub verdict: Verdict,
}

/// Why the choice came out as it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reason {
    /// The strip steered onto at the last read is still in reach: the
    /// target does not change in a fall.
    Kept,
    /// The strip the HUD marks is in reach.
    Marked,
    /// Neither: the strip the fall is fast enough for that the held
    /// landing misses by least, else the slow one it misses by least.
    Nearest,
    /// The player is on the ground or rising.
    NotFalling,
    /// No strip within [`CONSIDER`].
    NoStrip,
    /// Every strip near was refused.
    Refused,
    /// The player's body could not be read.
    NoBody,
}

impl Reason {
    /// As `superbounce.assist` names it.
    pub fn name(self) -> &'static str {
        match self {
            Reason::Kept => "kept",
            Reason::Marked => "marked",
            Reason::Nearest => "nearest",
            Reason::NotFalling => "not_falling",
            Reason::NoStrip => "no_strip",
            Reason::Refused => "refused",
            Reason::NoBody => "no_body",
        }
    }
}

/// The assist's choice for one read: every strip near, judged, and the one
/// to steer onto.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// Each bounce strip whose edge is within [`CONSIDER`] sideways, in
    /// the spots' order.
    pub candidates: Vec<Candidate>,
    /// The chosen one's place in `candidates`.
    pub chosen: Option<usize>,
    pub plan: Option<Plan>,
    pub reason: Reason,
}

/// Chooses the strip to steer a falling `body` onto.
///
/// Every bounce strip near is judged ([`judge`]). Of the ones in reach:
///
/// 1. the strip the last read steered onto (`ground.kept`), so the target
///    never changes in the middle of a fall;
/// 2. else the strip the HUD marks (`ground.marked`), so the pull is toward
///    the mark the player sees;
/// 3. else the strip the held landing misses by least, one the fall is
///    fast enough for before one it is too slow for.
pub fn choose(spots: &[Spot], body: &Body, ground: &Ground<'_>) -> Choice {
    let none = |reason, candidates| Choice {
        candidates,
        chosen: None,
        plan: None,
        reason,
    };
    if !falling(body) {
        return none(Reason::NotFalling, Vec::new());
    }
    let at = [body.position[0], body.position[1]];
    let mut candidates: Vec<Candidate> = Vec::new();
    // Each strip in reach: its place in `candidates` and its reach.
    let mut passed: Vec<(usize, Reach)> = Vec::new();
    for (index, spot) in spots.iter().enumerate() {
        if spot.kind != Kind::Bounce {
            continue;
        }
        let distance = crate::superbounce_charge::edge_distance_xy(spot, at);
        if distance > CONSIDER {
            continue;
        }
        let mut c = Candidate {
            index,
            stand: spot.stand,
            distance,
            speed: touchdown_speed(body, spot.stand[2]),
            need: crate::superbounce_charge::need_of(spot),
            point: None,
            time: None,
            miss: None,
            verdict: Verdict::Passed,
        };
        match judge(body, spot, ground) {
            Ok((point, reach)) => {
                c.point = Some(point);
                c.time = Some(reach.time);
                c.miss = Some(reach.miss);
                passed.push((candidates.len(), reach));
            }
            Err(why) => c.verdict = Verdict::Refused(why),
        }
        candidates.push(c);
    }
    if candidates.is_empty() {
        return none(Reason::NoStrip, candidates);
    }
    let picked: Option<((usize, Reach), Reason)> = {
        let find = |stand: Option<Vec3>| {
            let stand = stand?;
            passed
                .iter()
                .copied()
                .find(|(i, _)| candidates[*i].stand == stand)
        };
        let slow = |i: usize| candidates[i].speed < candidates[i].need;
        if let Some(p) = find(ground.kept) {
            Some((p, Reason::Kept))
        } else if let Some(p) = find(ground.marked) {
            Some((p, Reason::Marked))
        } else {
            passed
                .iter()
                .copied()
                .min_by(|a, b| {
                    slow(a.0)
                        .cmp(&slow(b.0))
                        .then(a.1.miss.total_cmp(&b.1.miss))
                })
                .map(|p| (p, Reason::Nearest))
        }
    };
    let Some(((i, reach), reason)) = picked else {
        return none(Reason::Refused, candidates);
    };
    candidates[i].verdict = Verdict::Chosen;
    let plan = Plan {
        target: candidates[i].stand,
        point: candidates[i].point.unwrap_or(candidates[i].stand),
        stick: world_to_stick(reach.stick, body.yaw),
        reach,
    };
    Choice {
        candidates,
        chosen: Some(i),
        plan: Some(plan),
        reason,
    }
}

/// The strip to steer onto with the spots alone ([`Ground::open`]): of the
/// reachable bounce spots, the one the held landing misses by least
/// ([`choose`]'s third rule).
pub fn pick<'a>(spots: &'a [Spot], body: &Body) -> Option<(&'a Spot, Reach)> {
    let c = choose(spots, body, &Ground::open());
    let spot = &spots[c.candidates[c.chosen?].index];
    Some((spot, c.plan?.reach))
}

/// A world-axes stick as the left stick `[x (right), y (forward)]`, in
/// -1..1, for a body facing `yaw`: turned into the aim's frame and cut
/// back to the unit disc keeping its direction. In the air the stick is a
/// velocity command (`superbounce_flight::air_step`), so the assist sends
/// the pursuit's stick as it is.
pub fn world_to_stick(world: [f32; 2], yaw: f32) -> [f32; 2] {
    if !world.iter().chain(&[yaw]).all(|v| v.is_finite()) {
        return [0.0; 2];
    }
    let (s, c) = yaw.sin_cos();
    // Forward is (cos, sin); right, with Z up, is (sin, -cos).
    let x = world[0] * s - world[1] * c;
    let y = world[0] * c + world[1] * s;
    let n = x.hypot(y);
    if n > 1.0 { [x / n, y / n] } else { [x, y] }
}

/// The reverse of [`world_to_stick`]: the world-axes stick of a pad's
/// left stick (`superbounce_flight::held` of `[y, −x]`).
pub fn stick_to_world(stick: [f32; 2], yaw: f32) -> [f32; 2] {
    flight::held([stick[1], -stick[0]], yaw)
}

/// A stick as the pad's signed 16-bit axes.
pub fn stick_raw(stick: [f32; 2]) -> (i16, i16) {
    let q = |v: f32| (v.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
    (q(stick[0]), q(stick[1]))
}

/// What the assist does this read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// The strip's stand point: what names the strip.
    pub target: Vec3,
    /// The point of the strip being landed on ([`stand_near`]).
    pub point: Vec3,
    pub reach: Reach,
    /// The left stick to send.
    pub stick: [f32; 2],
}

/// The plan for `body` over `spots` alone ([`Ground::open`]), if a strip is
/// in reach.
pub fn plan(spots: &[Spot], body: &Body) -> Option<Plan> {
    choose(spots, body, &Ground::open()).plan
}

/// The landing an airborne `body` is headed for, for the HUD and the
/// world marker (docs/game/superbounce.md, "Landings"), rising or
/// falling: of the bounce strips its flight comes down onto (at or under
/// its apex, within [`MAX_TIME`], on the flight model with the stick held
/// as it is, `superbounce_flight::hold`) with no floor between
/// ([`covered`]) and a point ([`stand_near`]) within [`REACH_RADIUS`] of
/// that landing, the one it lands nearest the slot of.
///
/// - `at` is that landing, `speed` its stepped touchdown speed, `across`
///   the speed along the drift it comes down with.
/// - `verdict`: `on_slot` when it lands inside the slot; `fixable` when
///   the flight model's pursuit of the aim (`superbounce_flight::pursue`)
///   ends inside it; else `miss`, out of the stick's reach.
/// - `steer`: the pursuit's stick to hold now (world axes); zero on the
///   slot.
///
/// `None` on the ground, or with no such strip.
pub fn predict_on(
    spots: &[Spot],
    body: &Body,
    ground: &Ground<'_>,
) -> Option<superbounce_mode::Predicted> {
    use crate::superbounce_charge::{edge_distance_xy, need_of, slot_miss};
    use superbounce_mode::{Course, Predicted};
    if !body.airborne {
        return None;
    }
    let top = apex(body);
    let (spot, landing, _) = spots
        .iter()
        .filter(|s| s.kind == Kind::Bounce)
        .filter_map(|s| {
            let l = held_onto(body, s.stand[2])
                .filter(|l| l.ticks as f32 * flight::TICK <= MAX_TIME)?;
            let xy = [l.row[0], l.row[1]];
            let point = stand_near(s, xy);
            let side = (point[0] - xy[0]).hypot(point[1] - xy[1]);
            (side <= REACH_RADIUS && !covered(top, point, ground))
                .then(|| (s, l, edge_distance_xy(s, xy)))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))?;
    let xy = [landing.row[0], landing.row[1]];
    let aim = aim_at(spot, stand_near(spot, xy));
    let (verdict, steer) = if slot_miss(spot, xy) <= 0.0 {
        (Course::OnSlot, [0.0; 2])
    } else {
        match flight::pursue(body.position, body.velocity, aim, |_| spot.stand[2]) {
            Some(p) if slot_miss(spot, [p.landing.row[0], p.landing.row[1]]) <= 0.0 => {
                (Course::Fixable, p.stick_now)
            }
            Some(p) => (Course::Miss, p.stick_now),
            None => (Course::Miss, [0.0; 2]),
        }
    };
    Some(Predicted {
        at: [xy[0], xy[1], spot.stand[2]],
        aim,
        drift: drift_unit(spot.drift),
        across: across(spot, landing.velocity),
        verdict,
        speed: landing.speed,
        need: need_of(spot),
        edge: spot.edge,
        stand: spot.stand,
        fall: spot.fall,
        steer,
    })
}

/// [`predict_on`] with the spots alone ([`Ground::open`]).
pub fn predict(spots: &[Spot], body: &Body) -> Option<superbounce_mode::Predicted> {
    predict_on(spots, body, &Ground::open())
}

/// Whether the button is being swallowed: from the first read the assist
/// engaged on, until the player lets go. Without it the press that began in
/// the air would reach the game the moment the strip went out of reach (a
/// landing, a miss), as a fresh press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Latch(pub bool);

/// Puts the assist's decision into `pad`: the stick, and X out while the
/// button is swallowed. `held` is whether the player holds the button;
/// `plan` is what the assist would do (ignored when not held).
/// Returns whether the assist is engaged.
pub fn apply(latch: &mut Latch, pad: &mut Gamepad, held: bool, plan: Option<&Plan>) -> bool {
    let plan = plan.filter(|_| held);
    latch.0 = held && (latch.0 || plan.is_some());
    if let Some(p) = plan {
        let (lx, ly) = stick_raw(p.stick);
        pad.thumb_lx = lx;
        pad.thumb_ly = ly;
    }
    if latch.0 {
        pad.analog[BUTTON.index()] = 0;
    }
    plan.is_some()
}

// ---- what the agent sees ----

/// The assist's last read, for `superbounce.assist`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub engaged: bool,
    /// The strip's stand point.
    pub target: Option<Vec3>,
    /// The point of the strip being landed on.
    pub point: Option<Vec3>,
    /// The point being aimed at.
    pub aim: Option<[f32; 2]>,
    /// Where the player lands with their own stick held as it is.
    pub landing: Option<[f32; 2]>,
    /// Seconds to the last free row.
    pub time: Option<f32>,
    /// The left stick sent, `[x (right), y (forward)]`.
    pub stick: Option<[f32; 2]>,
    /// Why the choice came out as it did; `None` while the button is up.
    pub reason: Option<Reason>,
    /// The player's origin and velocity at the read.
    pub from: Option<Vec3>,
    pub velocity: Option<Vec3>,
    /// The strips judged: the chosen one, the others in reach, then the
    /// refused, nearest first, at most [`REPORTED`].
    pub candidates: Vec<Candidate>,
}

impl Report {
    const EMPTY: Report = Report {
        engaged: false,
        target: None,
        point: None,
        aim: None,
        landing: None,
        time: None,
        stick: None,
        reason: None,
        from: None,
        velocity: None,
        candidates: Vec::new(),
    };

    fn of(p: &Plan) -> Report {
        Report {
            engaged: true,
            target: Some(p.target),
            point: Some(p.point),
            aim: Some(p.reach.aim),
            landing: Some(p.reach.landing),
            time: Some(p.reach.time),
            stick: Some(p.stick),
            ..Report::EMPTY
        }
    }

    /// A held read: the body, the choice made over it, and whether the
    /// stick was the assist's.
    pub fn of_choice(body: &Body, choice: &Choice, engaged: bool) -> Report {
        let mut candidates = choice.candidates.clone();
        candidates.sort_by(|a, b| {
            a.verdict
                .rank()
                .cmp(&b.verdict.rank())
                .then(a.distance.total_cmp(&b.distance))
        });
        candidates.truncate(REPORTED);
        let plan = choice.plan.filter(|_| engaged);
        Report {
            reason: Some(choice.reason),
            from: Some(body.position),
            velocity: Some(body.velocity),
            candidates,
            ..plan.as_ref().map_or(Report::EMPTY, Report::of)
        }
    }
}

// ---- the crouch button ----

/// The player's input settings records (`0x51ea18 + 0x1c × player`), whose
/// bytes 8 to 0x17 map each action slot to the physical button that drives
/// it (docs/input/controller-layouts.md, "The action table").
const ACTION_TABLE: u32 = 0x0051_ea18;
const ACTION_TABLE_STRIDE: u32 = 0x1c;
const ACTION_SLOTS: u32 = 8;
/// Crouch is action slot 10 (docs/game/bots.md, "The action from a pad").
const CROUCH_SLOT: u32 = 10;
/// The Default layout's crouch button: the left stick's click.
const DEFAULT_CROUCH_BUTTON: u8 = 14;

/// Whether physical button `index` of the action table (0 to 7 the analog
/// buttons A, B, X, Y, Black, White, LT, RT; 8 to 15 the digital bits
/// d-pad up, down, left, right, Start, Back, left and right stick) is down
/// in `pad`. `None` for an index past 15.
pub fn button_down(pad: &Gamepad, index: u8) -> Option<bool> {
    match index {
        0..=7 => Some(pad.analog[usize::from(index)] >= PRESSED),
        8..=15 => Some(pad.buttons.0 & (1 << (index - 8)) != 0),
        _ => None,
    }
}

/// The button the player's layout crouches with, from the action table;
/// the Default layout's when it cannot be read.
fn crouch_button(memory: Option<&dyn xapi::GuestMemory>) -> u8 {
    let at = ACTION_TABLE + ACTION_TABLE_STRIDE * PORT + ACTION_SLOTS + CROUCH_SLOT;
    let mut b = [0u8; 1];
    match memory.map(|m| m.read(at, &mut b)) {
        Some(Ok(())) if b[0] <= 15 => b[0],
        _ => DEFAULT_CROUCH_BUTTON,
    }
}

/// The crouch button as the guest last read it on [`PORT`]:
/// [`CROUCH_UNKNOWN`], [`CROUCH_UP`] or [`CROUCH_DOWN`].
static CROUCH: AtomicU8 = AtomicU8::new(CROUCH_UNKNOWN);
const CROUCH_UNKNOWN: u8 = 0;
const CROUCH_UP: u8 = 1;
const CROUCH_DOWN: u8 = 2;

/// Whether the first player holds crouch in the pad the guest last read,
/// with the agent's steps and before a replay's; `None` when the tap is
/// not installed or the mode is off. The charge's `held` phase reads it.
pub fn crouch_held() -> Option<bool> {
    match CROUCH.load(Ordering::Relaxed) {
        CROUCH_UP => Some(false),
        CROUCH_DOWN => Some(true),
        _ => None,
    }
}

fn publish_crouch(held: Option<bool>) {
    let v = match held {
        Some(true) => CROUCH_DOWN,
        Some(false) => CROUCH_UP,
        None => CROUCH_UNKNOWN,
    };
    CROUCH.store(v, Ordering::Relaxed);
}

static ENABLED: AtomicBool = AtomicBool::new(false);
/// The player has turned the assist on (`superbounce.fall_assist`).
static OPTED_IN: AtomicBool = AtomicBool::new(false);
/// The tap is in place ([`install_tap`] wrapped the controller reads).
static TAPPED: AtomicBool = AtomicBool::new(false);
static ENGAGED: AtomicBool = AtomicBool::new(false);
static TEST_HOLD: AtomicBool = AtomicBool::new(false);
static REPORT: Mutex<Report> = Mutex::new(Report::EMPTY);
/// The read the assist last engaged on: the choice at the press, kept
/// after the button is let go.
static CHOSE: Mutex<Option<Report>> = Mutex::new(None);
/// The stand point of the strip the HUD marks ([`set_marked`]).
static MARKED: Mutex<Option<Vec3>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn publish(r: Report) {
    ENGAGED.store(r.engaged, Ordering::Relaxed);
    *lock(&REPORT) = r;
}

/// Tells the assist which strip the HUD marks, by its stand point: the
/// navpoint's while primed (`superbounce_mode`, each tick). The choice
/// prefers it ([`choose`]).
pub fn set_marked(stand: Option<Vec3>) {
    *lock(&MARKED) = stand;
}

/// The strip the HUD marks, as [`set_marked`] left it.
pub fn marked() -> Option<Vec3> {
    *lock(&MARKED)
}

/// The choice for `body` on the loaded map: the scan's spots and collision
/// (`superbounce_mode`), the HUD's mark, and `kept` from the last read.
pub(crate) fn choose_now(body: &Body, kept: Option<Vec3>) -> Choice {
    use superbounce::view::LineOfSight;
    let room = superbounce_mode::current_room();
    let floor = |xy: [f32; 2], top: f32| room.floor_at(xy, top);
    let clear = |a: Vec3, b: Vec3| room.is_empty() || room.view().clear(a, b);
    let ground = Ground {
        floor: &floor,
        clear: &clear,
        marked: marked(),
        kept,
    };
    choose(&superbounce_mode::current_spots(), body, &ground)
}

/// Whether the player has turned the assist on: the
/// `superbounce.fall_assist` setting, off by default. Off, X is never
/// read, nothing steers, and nothing on the map or the HUD asks for X.
pub fn opted_in() -> bool {
    OPTED_IN.load(Ordering::Relaxed)
}

/// Sets what [`opted_in`] answers (the setting's live value).
pub fn set_opted_in(on: bool) {
    OPTED_IN.store(on, Ordering::Relaxed);
}

// ---- the verifier's pad ----

/// A whole pad for the first controller, set by `superbounce.verify`'s
/// trials on `player.0` (crates/extensions/game/src/superbounce_verify.rs):
/// the left stick, the crouch button of the player's layout, and A.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VerifyPad {
    /// `[x (right), y (forward)]`, -1..1.
    pub stick: [f32; 2],
    pub crouch: bool,
    pub jump: bool,
}

static VERIFY_PAD: Mutex<Option<VerifyPad>> = Mutex::new(None);

/// Replaces the first controller's pad with `pad` at every read until
/// called with `None`, whether the mode is on or not. `false` when there is
/// no tap to do it (the `superbounce-assist` feature is off).
pub fn set_verify_pad(pad: Option<VerifyPad>) -> bool {
    *lock(&VERIFY_PAD) = pad;
    TAPPED.load(Ordering::Relaxed)
}

/// Whether the tap is in place, so [`set_verify_pad`] reaches the game.
pub fn tapped() -> bool {
    TAPPED.load(Ordering::Relaxed)
}

/// `pad` with the verifier's pad in place of what the player pressed.
pub fn verify_pad_into(pad: &mut Gamepad, v: &VerifyPad, crouch_button: u8) {
    *pad = Gamepad::default();
    let (x, y) = stick_raw(v.stick);
    pad.thumb_lx = x;
    pad.thumb_ly = y;
    if v.jump {
        pad.analog[AnalogButton::A.index()] = 255;
    }
    if v.crouch {
        match crouch_button {
            0..=7 => pad.analog[usize::from(crouch_button)] = 255,
            8..=15 => pad.buttons.0 |= 1 << (crouch_button - 8),
            _ => pad.buttons.0 |= 1 << (DEFAULT_CROUCH_BUTTON - 8),
        }
    }
}

/// Whether the assist is steering the player now: the HUD can show it.
pub fn assisting() -> bool {
    ENGAGED.load(Ordering::Relaxed)
}

/// The assist's last read.
pub fn report() -> Report {
    lock(&REPORT).clone()
}

/// The read the assist last engaged on.
pub fn chose() -> Option<Report> {
    lock(&CHOSE).clone()
}

/// `superbounce.assist_test`: acts as if the button is held (`true`) or
/// stops (`false`), so an agent can try the assist with no pad. Returns
/// the setting.
pub fn set_test_hold(hold: bool) -> bool {
    TEST_HOLD.store(hold, Ordering::Relaxed);
    hold
}

/// `superbounce.assist` takes no parameters: null or `{}`.
pub fn no_params(params: &serde_json::Value) -> Result<(), &'static str> {
    if params.is_null() || params.as_object().is_some_and(|f| f.is_empty()) {
        Ok(())
    } else {
        Err("superbounce.assist takes no parameters")
    }
}

/// `superbounce.assist` as the agent answers it.
pub fn assist_reply() -> agent_proto::superbounce::Assist {
    let r = report();
    let active = superbounce_mode::active();
    let enabled = opted_in();
    let wide2 = |v: [f32; 2]| v.map(f64::from);
    let reason = match r.reason {
        Some(reason) => reason.name(),
        None if !active => "inactive",
        None if !enabled => "off",
        None => "not_held",
    };
    agent_proto::superbounce::Assist {
        active,
        enabled,
        test_hold: TEST_HOLD.load(Ordering::Relaxed),
        engaged: r.engaged,
        reason: reason.into(),
        target: r.target.map(wide),
        point: r.point.map(wide),
        aim: r.aim.map(wide2),
        landing: r.landing.map(wide2),
        fall_time: r.time.map(f64::from),
        stick: r.stick.map(wide2),
        from: r.from.map(wide),
        velocity: r.velocity.map(wide),
        candidates: r.candidates.iter().map(candidate_reply).collect(),
        chose: chose().as_ref().and_then(choice_reply),
    }
}

fn wide(v: Vec3) -> [f64; 3] {
    v.map(f64::from)
}

fn candidate_reply(c: &Candidate) -> agent_proto::superbounce::AssistCandidate {
    agent_proto::superbounce::AssistCandidate {
        stand: wide(c.stand),
        verdict: c.verdict.name().into(),
        distance: f64::from(c.distance),
        speed: f64::from(c.speed),
        need: f64::from(c.need),
        point: c.point.map(wide),
        fall_time: c.time.map(f64::from),
        miss: c.miss.map(f64::from),
    }
}

/// An engaged read as `superbounce.assist`'s `chose`.
fn choice_reply(r: &Report) -> Option<agent_proto::superbounce::AssistChoice> {
    Some(agent_proto::superbounce::AssistChoice {
        target: wide(r.target?),
        point: wide(r.point?),
        reason: r.reason?.name().into(),
        from: wide(r.from?),
        velocity: wide(r.velocity?),
        candidates: r.candidates.iter().map(candidate_reply).collect(),
    })
}

// ---- reading the game ----

struct Mem<'a>(&'a dyn xapi::GuestMemory);

impl GuestMemory for Mem<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.0.read(addr, buf).map_err(|_| MemFault {
            addr,
            len: buf.len() as u32,
            write: false,
        })
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })
    }
}

/// The body of the local player on controller `port`, its stick none
/// (the tap sets the pad's, [`pad_stick`]).
pub(crate) fn read_body(m: &dyn GuestMemory, port: u32) -> Option<Body> {
    use crate::telemetry::layout;
    let unit = crate::skulls_custom::local_units(m)
        .ok()?
        .into_iter()
        .find(|u| u.kind == BIPED_KIND && i64::from(u.user) == i64::from(port))?;
    let base = unit.body;
    let position = m.vec3(base.wrapping_add(POSITION)).ok()?;
    let velocity = m.vec3(base.wrapping_add(layout::BODY_VELOCITY)).ok()?;
    let aim = m.vec3(base.wrapping_add(layout::UNIT_AIM)).ok()?;
    let flags = m.u16(base.wrapping_add(layout::BIPED_FLAGS)).ok()?;
    if !position
        .iter()
        .chain(&velocity)
        .chain(&aim)
        .all(|v| v.is_finite())
        || aim[0].hypot(aim[1]) < MIN_AIM
    {
        return None;
    }
    Some(Body {
        position,
        velocity,
        yaw: aim[1].atan2(aim[0]),
        airborne: flags & 1 != 0,
        stick: [0.0; 2],
    })
}

/// The left stick of `pad` in world axes for a body facing `yaw`
/// ([`stick_to_world`]): the stick the player holds, before the assist
/// replaces it.
pub fn pad_stick(pad: &Gamepad, yaw: f32) -> [f32; 2] {
    let axis = |v: i16| f32::from(v) / f32::from(i16::MAX);
    stick_to_world([axis(pad.thumb_lx), axis(pad.thumb_ly)], yaw)
}

// ---- the tap ----

/// The assist at `XInputGetState`.
pub struct AssistTap {
    inner: Option<Box<dyn InputTap>>,
    /// Controller port each open handle was opened on.
    ports: HashMap<u32, u32>,
    latch: Latch,
    /// The report holds a held read.
    live: bool,
    /// The stand point of the strip the last read steered onto.
    kept: Option<Vec3>,
}

impl AssistTap {
    pub fn new(inner: Option<Box<dyn InputTap>>) -> Self {
        AssistTap {
            inner,
            ports: HashMap::new(),
            latch: Latch::default(),
            live: false,
            kept: None,
        }
    }

    fn release(&mut self) {
        if self.live {
            self.live = false;
            publish(Report::EMPTY);
        }
        self.latch = Latch::default();
        self.kept = None;
    }

    fn assist(
        &mut self,
        handle: u32,
        status: u32,
        packet: &mut [u8; PACKET],
        memory: Option<&dyn xapi::GuestMemory>,
    ) {
        // A verifier's trial owns the first pad whatever the mode.
        if status == 0
            && self.ports.get(&handle) == Some(&PORT)
            && let Some(v) = *lock(&VERIFY_PAD)
        {
            let raw: &[u8; Gamepad::SIZE] = (&packet[4..])
                .try_into()
                .expect("a packet is a number and a gamepad");
            let mut pad = Gamepad::from_bytes(raw);
            let button = crouch_button(memory);
            verify_pad_into(&mut pad, &v, button);
            packet[4..].copy_from_slice(&pad.to_bytes());
            publish_crouch(Some(v.crouch));
            if self.live || self.latch.0 {
                self.release();
            }
            return;
        }
        // A Clanker playing the player's seat (player_bot.rs) gives the
        // pad, until a real press in it hands the seat back.
        if status == 0
            && self.ports.get(&handle) == Some(&PORT)
            && crate::player_bot::active()
            && let Some(m) = memory
        {
            let raw: &[u8; Gamepad::SIZE] = (&packet[4..])
                .try_into()
                .expect("a packet is a number and a gamepad");
            // What the player holds, not what the agent's steps made of it: the
            // agent's own presses (`h2 bots test`'s driver) are not the player's.
            let held = agent::live_pad(handle).unwrap_or_else(|| Gamepad::from_bytes(raw));
            if let Some(pad) = crate::player_bot::pad_read(&held, &Mem(m)) {
                packet[4..].copy_from_slice(&pad.to_bytes());
                publish_crouch(None);
                if self.live || self.latch.0 {
                    self.release();
                }
                return;
            }
        }
        // The cost of a game that is not a superbounce one.
        if !superbounce_mode::active() {
            if self.live || self.latch.0 {
                self.release();
            }
            publish_crouch(None);
            return;
        }
        if status != 0 || self.ports.get(&handle) != Some(&PORT) {
            return;
        }
        let raw: &[u8; Gamepad::SIZE] = (&packet[4..])
            .try_into()
            .expect("a packet is a number and a gamepad");
        let mut pad = Gamepad::from_bytes(raw);
        // The charge's `held` phase: the crouch button of the player's own
        // layout, in the pad as the guest gets it.
        publish_crouch(button_down(&pad, crouch_button(memory)));
        // The ghost run's button (superbounce_ghost.rs).
        crate::superbounce_ghost::publish_layout(memory);
        crate::superbounce_ghost::publish_button(button_down(
            &pad,
            crate::superbounce_ghost::current_button(),
        ));
        // Opt in: with the setting off the button is the game's.
        let held =
            opted_in() && (pad.analog(BUTTON) >= PRESSED || TEST_HOLD.load(Ordering::Relaxed));
        if !held && !self.latch.0 {
            self.release();
            return;
        }
        if !held {
            // The latch's last read: the press is over.
            self.kept = None;
        }
        let read = held.then(|| {
            memory
                .and_then(|m| read_body(&Mem(m), PORT))
                .map(|b| Body {
                    stick: pad_stick(&pad, b.yaw),
                    ..b
                })
                .map(|b| (b, choose_now(&b, self.kept)))
        });
        let plan = read.as_ref().and_then(|r| r.as_ref()?.1.plan);
        let engaged = apply(&mut self.latch, &mut pad, held, plan.as_ref());
        packet[4..].copy_from_slice(&pad.to_bytes());
        // The strip stays the target while it is in reach; a new strip is
        // chosen only once it is not.
        let first = engaged && self.kept.is_none();
        self.kept = plan.filter(|_| engaged).map(|p| p.target);
        match read {
            Some(found) => {
                self.live = true;
                let report = match &found {
                    Some((body, choice)) => Report::of_choice(body, choice, engaged),
                    None => Report {
                        reason: Some(Reason::NoBody),
                        ..Report::EMPTY
                    },
                };
                if first {
                    *lock(&CHOSE) = Some(report.clone());
                }
                publish(report);
            }
            None if self.live => {
                self.live = false;
                publish(Report::EMPTY);
            }
            None => {}
        }
    }
}

impl InputTap for AssistTap {
    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]) {
        self.read_with_memory(handle, status, packet, None);
    }

    fn read_with_memory(
        &mut self,
        handle: u32,
        status: &mut u32,
        packet: &mut [u8; PACKET],
        memory: Option<&dyn xapi::GuestMemory>,
    ) {
        // Before the tap under this one: a recording logs the packet the
        // guest gets, and a replay overwrites it.
        self.assist(handle, *status, packet, memory);
        if let Some(t) = &mut self.inner {
            t.read_with_memory(handle, status, packet, memory);
        }
    }

    fn sync(&mut self, input: &mut input::InputManager) {
        match &mut self.inner {
            Some(t) => t.sync(input),
            None => input.sync(),
        }
    }

    fn restore_input(
        &mut self,
        cursor: u64,
        input: &mut input::InputManager,
    ) -> Result<(), String> {
        match &mut self.inner {
            Some(t) => t.restore_input(cursor, input),
            None => Ok(()),
        }
    }

    fn opened(&mut self, handle: u32, port: u32) {
        self.ports.insert(handle, port);
        if let Some(t) = &mut self.inner {
            t.opened(handle, port);
        }
    }

    fn closed(&mut self, handle: u32) {
        self.ports.remove(&handle);
        if let Some(t) = &mut self.inner {
            t.closed(handle);
        }
    }

    fn cursor(&self) -> Option<u64> {
        self.inner.as_ref().and_then(|t| t.cursor())
    }

    fn seek(&mut self, cursor: u64) -> Result<(), String> {
        match &mut self.inner {
            Some(t) => t.seek(cursor),
            None => Ok(()),
        }
    }
}

/// Wraps `xapi`'s input tap (a recording's, a replay's, or none) in the
/// assist, when the feature is on. Call once, before the agent installs its
/// own tap over the result.
pub fn install_tap<S: xapi::Storage>(xapi: Option<&Mutex<xapi::Xapi<S>>>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(x) = xapi else { return };
    let mut x = lock(x);
    let inner = x.take_input_tap();
    x.set_input_tap(Some(Box::new(AssistTap::new(inner))));
    TAPPED.store(true, Ordering::Relaxed);
}

// ---- the feature ----

/// The feature: its manifest and its snapshot policy. The work is in the
/// tap ([`install_tap`]); it has no guest hooks.
pub struct SuperbounceAssist;

impl hooks::Feature for SuperbounceAssist {
    fn manifest(&self) -> hooks::Manifest {
        // Not presentation-only: it changes the pad the guest reads. It
        // adds no hook, so a film's hook digest is unchanged by it, and a
        // film records the pad as sent.
        hooks::Manifest::new(
            FEATURE,
            "Superbounce fall assist: hold X to steer onto a strip",
        )
        .doc("docs/game/superbounce.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // Everything is re-read from guest RAM at each pad read; what the
        // tap keeps (the swallowed-button latch, the test hold, the last
        // report) is momentary, and losing it in a load costs one press.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds the feature to `registry`. Whether the tap is installed follows
/// `H2_FEATURES` (the kill switch); whether it steers follows the
/// `superbounce.fall_assist` setting in `settings`, read live (off without
/// a store).
pub fn install(
    registry: &mut hooks::Registry,
    settings: Option<Arc<settings::Store>>,
) -> anyhow::Result<()> {
    let on = registry
        .add(&SuperbounceAssist)
        .map_err(|e| anyhow::anyhow!("superbounce assist: {e}"))?;
    ENABLED.store(on, Ordering::Relaxed);
    set_opted_in(false);
    if let Some(s) = &settings {
        follow_setting(s);
    }
    Ok(())
}

/// Makes [`opted_in`] follow the `superbounce.fall_assist` setting in
/// `store`: its value now, then each change.
pub fn follow_setting(store: &settings::Store) {
    let key = crate::setting_defs::SUPERBOUNCE_FALL_ASSIST.key;
    set_opted_in(store.handle::<bool>(key).get());
    store.on_change(key, |v| {
        set_opted_in(matches!(v, settings::Value::Bool(true)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot_at(stand: Vec3, drift: [f32; 2]) -> Spot {
        Spot {
            kind: Kind::Bounce,
            edge: [stand, stand],
            stand,
            fall: 1.0,
            launch: 12.0,
            drift,
            score: 1.0,
            ..Spot::default()
        }
    }

    /// An airborne body holding the stick that keeps it going
    /// (`superbounce_flight::keep`).
    fn falling(at: Vec3, v: Vec3) -> Body {
        Body {
            position: at,
            velocity: v,
            yaw: 0.0,
            airborne: true,
            stick: flight::keep([v[0], v[1]]),
        }
    }

    fn near(a: f32, b: f32, eps: f32) {
        assert!((a - b).abs() <= eps, "{a} vs {b} (±{eps})");
    }

    // ---- the landing prediction ----

    /// The landing is flown on the flight model: a drop of 2.4 from rest
    /// comes down from its 31st row at 4.31 units/s (31 × 0.13903), where
    /// it started across.
    #[test]
    fn the_landing_is_flown_on_the_flight_model() {
        let body = falling([0.3, 0.2, 2.4], [0.0; 3]);
        let l = held_onto(&body, 0.0).unwrap();
        assert_eq!((l.ticks, l.row[0], l.row[1]), (31, 0.3, 0.2));
        near(l.speed, 4.310, 1e-3);
        near(touchdown_speed(&body, 0.0), l.speed, 1e-6);
        // Under the floor, nothing.
        assert_eq!(touchdown_speed(&falling([0.0, 0.0, -1.0], [0.0; 3]), 0.0), 0.0);
        // The apex: where it is when falling, over it when rising.
        assert_eq!(apex(&body), 2.4);
        near(
            apex(&falling([0.0, 0.0, 1.0], [0.0, 0.0, 3.0])),
            1.0 + 9.0 / (2.0 * flight::GRAVITY),
            1e-5,
        );
    }

    // ---- the reach cone ----

    fn gate(body: Body, spot: Spot) -> bool {
        reach(&body, &spot).is_some()
    }

    #[test]
    fn a_strip_below_and_close_is_in_reach() {
        let spot = spot_at([0.0, 0.0, 0.0], [1.0, 0.0]);
        let r = reach(&falling([0.3, 0.2, 2.4], [0.0; 3]), &spot).unwrap();
        near(r.aim[0], -AIM_BACK, 1e-6);
        near(r.time, 31.0 / 30.0, 1e-5);
        assert_eq!(r.landing, [0.3, 0.2]);
        // The stick gets there: it points at the aim.
        assert!(r.short < 0.02 && r.stick[0] < 0.0, "{r:?}");
    }

    #[test]
    fn the_cone_refuses_what_is_above_far_or_not_a_fall() {
        let spot = spot_at([0.0, 0.0, 0.0], [1.0, 0.0]);
        let ok = falling([0.0, 0.0, 2.4], [0.0; 3]);
        assert!(gate(ok, spot));
        // Not below the player, or level with them.
        assert!(!gate(falling([0.0, 0.0, -1.0], [0.0; 3]), spot));
        assert!(!gate(falling([0.0, 0.0, 0.01], [0.0; 3]), spot));
        // Too far sideways, however long the fall.
        assert!(!gate(falling([3.5, 0.0, 30.0], [0.0; 3]), spot));
        // On the ground, or rising (a bounce's flight, a jump's start).
        assert!(!gate(
            Body {
                airborne: false,
                ..ok
            },
            spot
        ));
        assert!(!gate(falling([0.0, 0.0, 2.4], [0.0, 0.0, 12.0]), spot));
        // A crouch spot is not a strip.
        assert!(!gate(
            ok,
            Spot {
                kind: Kind::Crouch,
                ..spot
            }
        ));
    }

    /// The cone is the air stick's: 1.05 units/s² toward at most 1.2
    /// units/s. The aim 2.4 away takes a long fall: 82 ticks from 16.0
    /// reach it, the 10 from 0.3 do not.
    #[test]
    fn the_cone_widens_with_the_time_left_to_steer() {
        let spot = spot_at([2.5, 0.0, 0.0], [1.0, 0.0]);
        // 2.5 sideways: out of reach from a short fall, within it from a
        // long one.
        assert!(!gate(falling([0.0, 0.0, 0.3], [0.0; 3]), spot));
        assert!(!gate(falling([0.0, 0.0, 4.0], [0.0; 3]), spot));
        assert!(gate(falling([0.0, 0.0, 16.0], [0.0; 3]), spot));
        // Coasting the wrong way takes it out of reach again.
        assert!(!gate(falling([0.0, 0.0, 1.0], [-3.0, 0.0, 0.0]), spot));
    }

    #[test]
    fn a_drift_less_spot_aims_at_the_stand_point_at_rest() {
        let spot = spot_at([1.0, 1.0, 0.0], [0.0, 0.0]);
        let r = reach(&falling([1.0, 1.0, 2.0], [0.0; 3]), &spot).unwrap();
        assert_eq!(r.aim, [1.0, 1.0]);
        assert_eq!((r.miss, r.stick), (0.0, [0.0, 0.0]));
    }

    #[test]
    fn the_spot_the_held_landing_misses_by_least_is_picked() {
        let a = spot_at([1.5, 0.0, 0.0], [1.0, 0.0]);
        let b = spot_at([-0.4, 0.2, 0.0], [1.0, 0.0]);
        let body = falling([0.0, 0.0, 2.0], [0.0; 3]);
        let spots = [a, b];
        let (picked, _) = pick(&spots, &body).unwrap();
        assert_eq!(picked.stand, b.stand);
        assert!(pick(&[], &body).is_none());
    }

    // ---- the stick ----

    #[test]
    fn the_stick_is_in_the_aims_frame() {
        let a = 1.0;
        let at = |accel: [f32; 2], yaw: f32| world_to_stick(accel, yaw);
        let same = |s: [f32; 2], x: f32, y: f32| {
            near(s[0], x, 1e-5);
            near(s[1], y, 1e-5);
        };
        use std::f32::consts::{FRAC_PI_2, PI};
        // Facing +X: forward is +X, left (+Y) is stick left.
        same(at([a, 0.0], 0.0), 0.0, 1.0);
        same(at([0.0, a], 0.0), -1.0, 0.0);
        // Facing +Y: +X is on the right.
        same(at([a, 0.0], FRAC_PI_2), 1.0, 0.0);
        same(at([0.0, a], FRAC_PI_2), 0.0, 1.0);
        // Facing -X: the world turns around.
        same(at([a, 0.0], PI), 0.0, -1.0);
        same(at([0.0, a], PI), 1.0, 0.0);
        // Facing -Y.
        same(at([a, 0.0], -FRAC_PI_2), -1.0, 0.0);
        same(at([0.0, a], -FRAC_PI_2), 0.0, -1.0);
    }

    #[test]
    fn the_stick_round_trips_and_saturates_keeping_its_direction() {
        for yaw in [0.0f32, 0.7, 2.0, -1.2, 3.0, -3.0] {
            let want = [0.5, -0.6];
            let back = stick_to_world(world_to_stick(want, yaw), yaw);
            near(back[0], want[0], 1e-4);
            near(back[1], want[1], 1e-4);
            let hard = world_to_stick([300.0, -400.0], yaw);
            near(hard[0].hypot(hard[1]), 1.0, 1e-5);
            let w = stick_to_world(hard, yaw);
            near(w[0] / w[1], 300.0 / -400.0, 1e-3);
        }
        assert_eq!(world_to_stick([f32::NAN, 0.0], 0.0), [0.0, 0.0]);
        assert_eq!(stick_raw([1.0, -1.0]), (32767, -32767));
        assert_eq!(stick_raw([0.0, 2.0]), (0, 32767));
    }

    /// Falls from `height` onto a strip at the origin with drift +X, the
    /// assist steering at 30 Hz. The plant is the engine's air: the stick
    /// drives the sideways velocity toward it times the top at `gain`
    /// times the measured 0.035 units/s a tick (`superbounce_flight`),
    /// and the fall gains its speed, then moves. Returns the last free
    /// row's place, relative to the aim point, and the velocity there.
    fn simulate(
        start: [f32; 2],
        v0: [f32; 2],
        height: f32,
        yaw: f32,
        gain: f32,
    ) -> ([f32; 2], [f32; 2]) {
        const DT: f32 = flight::TICK;
        let spot = spot_at([0.0, 0.0, 0.0], [1.0, 0.0]);
        let (mut p, mut v) = (start, v0);
        let (mut z, mut down) = (height, 0.0f32);
        loop {
            let body = Body {
                position: [p[0], p[1], z],
                velocity: [v[0], v[1], -down],
                yaw,
                airborne: true,
                stick: [0.0; 2],
            };
            let pad = plan(&[spot], &body).map_or([0.0; 2], |pl| pl.stick);
            let s = flight::effective(stick_to_world(pad, yaw));
            let want = [s[0] * flight::AIR_TOP, s[1] * flight::AIR_TOP];
            let d = [want[0] - v[0], want[1] - v[1]];
            let n = d[0].hypot(d[1]);
            let most = gain * flight::AIR_ACCEL * DT;
            let next_v = if n > most {
                [v[0] + d[0] * most / n, v[1] + d[1] * most / n]
            } else {
                want
            };
            let next_down = down + flight::GRAVITY * DT;
            if z - next_down * DT <= 0.0 {
                break;
            }
            v = next_v;
            p = [p[0] + v[0] * DT, p[1] + v[1] * DT];
            (z, down) = (z - next_down * DT, next_down);
        }
        let aim = [-AIM_BACK, 0.0];
        ([p[0] - aim[0], p[1] - aim[1]], v)
    }

    /// With the measured stick the assist lands each fall within the
    /// catch of the aim, from offsets the stick can make up in the fall
    /// (6.0 is 51 ticks: 1.35 from rest), at any facing.
    #[test]
    fn the_controller_lands_a_fall_on_the_aim_point() {
        // Each: where it starts (from the aim point), its drift, the drop.
        let cases = [
            ([0.5, 0.3], [0.0, 0.0], 6.0),
            ([-0.8, 0.6], [0.5, 0.0], 6.0),
            ([-0.4, -0.6], [-0.5, 0.3], 6.0),
        ];
        for yaw in [0.0f32, 1.1, -2.5] {
            for (off, v0, h) in cases {
                let start = [off[0] - AIM_BACK, off[1]];
                let (p, _) = simulate(start, v0, h, yaw, 1.0);
                let err = p[0].hypot(p[1]);
                assert!(err < 0.05, "yaw {yaw} {off:?}: error {err}");
            }
        }
    }

    #[test]
    fn the_controller_still_lands_close_when_the_stick_is_weaker_or_stronger_than_thought() {
        for gain in [0.7f32, 1.4] {
            let (p, _) = simulate([0.4 - AIM_BACK, 0.3], [0.0; 2], 6.0, 0.5, gain);
            assert!(p[0].hypot(p[1]) < 0.1, "gain {gain}: {p:?}");
        }
    }

    // ---- the button ----

    fn pad_with(x: u8) -> Gamepad {
        let mut pad = Gamepad::default();
        pad.analog[BUTTON.index()] = x;
        pad.thumb_lx = 111;
        pad.thumb_ly = -222;
        pad
    }

    fn a_plan() -> Plan {
        plan(
            &[spot_at([0.0, 0.0, 0.0], [1.0, 0.0])],
            &falling([0.4, 0.2, 2.0], [0.0; 3]),
        )
        .unwrap()
    }

    #[test]
    fn engaged_the_stick_is_the_assists_and_x_is_gone() {
        let p = a_plan();
        let (mut latch, mut pad) = (Latch::default(), pad_with(255));
        assert!(apply(&mut latch, &mut pad, true, Some(&p)));
        assert_eq!((pad.thumb_lx, pad.thumb_ly), stick_raw(p.stick));
        assert_eq!(pad.analog(BUTTON), 0);
        assert!(latch.0);
    }

    #[test]
    fn with_no_strip_in_reach_x_and_the_stick_pass_through() {
        let (mut latch, mut pad) = (Latch::default(), pad_with(255));
        assert!(!apply(&mut latch, &mut pad, true, None));
        assert_eq!(pad, pad_with(255));
        assert!(!latch.0);
    }

    #[test]
    fn not_held_nothing_changes_even_with_a_plan() {
        let p = a_plan();
        let (mut latch, mut pad) = (Latch::default(), pad_with(0));
        assert!(!apply(&mut latch, &mut pad, false, Some(&p)));
        assert_eq!(pad, pad_with(0));
    }

    #[test]
    fn the_press_stays_swallowed_until_it_is_let_go() {
        let p = a_plan();
        let mut latch = Latch::default();
        apply(&mut latch, &mut pad_with(255), true, Some(&p));
        // The strip goes out of reach (the landing) with X still down: the
        // game must not see a press.
        let mut pad = pad_with(255);
        assert!(!apply(&mut latch, &mut pad, true, None));
        assert_eq!(pad.analog(BUTTON), 0);
        assert_eq!((pad.thumb_lx, pad.thumb_ly), (111, -222));
        // Let go, then a new press passes.
        let mut pad = pad_with(0);
        apply(&mut latch, &mut pad, false, None);
        assert!(!latch.0);
        let mut pad = pad_with(255);
        apply(&mut latch, &mut pad, true, None);
        assert_eq!(pad.analog(BUTTON), 255);
    }

    #[test]
    fn a_press_that_began_on_the_ground_passes_until_the_assist_engages() {
        let (mut latch, mut pad) = (Latch::default(), pad_with(255));
        apply(&mut latch, &mut pad, true, None);
        assert_eq!(pad.analog(BUTTON), 255);
        // Engaging later swallows it from then on.
        let p = a_plan();
        let mut pad = pad_with(255);
        apply(&mut latch, &mut pad, true, Some(&p));
        assert_eq!(pad.analog(BUTTON), 0);
    }

    /// Off until the player turns it on, following the setting live; the
    /// reply says which. One test: the switch is process-wide.
    #[test]
    fn the_assist_is_opt_in_follows_the_setting_and_the_reply_says_so() {
        use crate::setting_defs::{ALL, SUPERBOUNCE_FALL_ASSIST};
        let store = settings::Store::defaults(ALL);
        follow_setting(&store);
        assert!(!opted_in(), "off by default");
        assert!(!assist_reply().enabled);
        store
            .set(SUPERBOUNCE_FALL_ASSIST.key, settings::Value::Bool(true))
            .unwrap();
        assert!(opted_in() && assist_reply().enabled);
        store
            .set(SUPERBOUNCE_FALL_ASSIST.key, settings::Value::Bool(false))
            .unwrap();
        assert!(!opted_in());
    }

    #[test]
    fn the_test_hold_is_a_switch() {
        assert!(set_test_hold(true));
        assert!(assist_reply().test_hold);
        assert!(!set_test_hold(false));
        assert!(!assist_reply().test_hold);
    }

    #[test]
    fn the_reply_carries_the_plan_in_wide_numbers() {
        let p = a_plan();
        let r = Report::of(&p);
        assert!(r.engaged);
        assert_eq!(r.target, Some(p.target));
        assert_eq!(r.stick, Some(p.stick));
        assert_eq!(Report::EMPTY, Report::default());
        assert!(no_params(&serde_json::json!({})).is_ok());
        assert!(no_params(&serde_json::json!({"hold": true})).is_err());
    }

    /// The prediction: on the slot, fixable when the flight model's
    /// pursuit gets into the slot, else a miss; rising too.
    #[test]
    fn the_predicted_landing_says_on_the_slot_fixable_or_a_miss() {
        use superbounce_mode::Course;
        let spots = [spot_at([0.0, 0.0, 0.0], [0.0, 0.0])];
        let on = predict(&spots, &falling([0.0, 0.0, 2.0], [0.0, 0.0, -1.0])).unwrap();
        assert_eq!((on.verdict, on.steer), (Course::OnSlot, [0.0; 2]));
        assert_eq!(on.at, [0.0, 0.0, 0.0]);
        // Stepped: the 22nd row's 1 + 22 × 0.13903.
        near(on.speed, 4.0587, 1e-3);
        near(on.need, (9.0 + 2.0 * GRAVITY * 1.0).sqrt(), 1e-5);
        // Half a unit off with 34 ticks to go: the stick brings it in,
        // pushed back toward the strip.
        let off = predict(&spots, &falling([0.5, 0.0, 4.0], [0.0, 0.0, -1.0])).unwrap();
        assert_eq!(off.verdict, Course::Fixable);
        assert!(off.steer[0] < -0.3 && off.steer[1] == 0.0, "{:?}", off.steer);
        // Nearly 3 off with a short fall left: it cannot.
        let far = predict(&spots, &falling([2.8, 0.0, 0.3], [0.0, 0.0, -1.0])).unwrap();
        assert_eq!(far.verdict, Course::Miss);
        // Rising, it comes down on the strip all the same.
        let up = predict(&spots, &falling([0.0, 0.0, 2.0], [0.0, 0.0, 1.0])).unwrap();
        assert_eq!(up.verdict, Course::OnSlot);
        near(up.speed, 4.0052, 1e-3);
        // On the ground, under the strip and never over it, or past its
        // reach: none.
        let ground = Body {
            airborne: false,
            ..falling([0.0, 0.0, 2.0], [0.0, 0.0, -1.0])
        };
        assert_eq!(predict(&spots, &ground), None);
        assert_eq!(
            predict(&spots, &falling([0.0, 0.0, -1.0], [0.0, 0.0, -1.0])),
            None
        );
        assert_eq!(
            predict(&spots, &falling([0.0, 0.0, -1.0], [0.0, 0.0, 1.0])),
            None
        );
        assert_eq!(
            predict(&spots, &falling([5.0, 0.0, 2.0], [0.0, 0.0, -1.0])),
            None
        );
    }

    /// The video's run-jump: off a floor at 2.0 units/s, jumping, onto a
    /// seam on the same floor 2.0115 on. With the stick held forward the
    /// air brakes it to 1.2 and it lands on the seam (the flight model's
    /// test, `superbounce_flight`); coasting would put it 2.867 out, 0.86
    /// past, and a coasting prediction told the player to pull back. Let
    /// go, it brakes to 1.763, short: fixable, by pushing on.
    #[test]
    fn a_braking_run_jump_is_on_the_slot_not_past_it() {
        use superbounce_mode::Course;
        let x = 2.0115;
        let seam = Spot {
            edge: [[x, -1.0, 0.0], [x, 1.0, 0.0]],
            landed: true,
            ..spot_at([x, 0.0, 0.0], [1.0, 0.0])
        };
        let jump = Body {
            stick: [1.0, 0.0],
            ..falling([0.0; 3], [2.0, 0.0, bots::bounce_route::JUMP_VZ])
        };
        let p = predict(&[seam], &jump).unwrap();
        assert_eq!(p.verdict, Course::OnSlot);
        near(p.at[0], x, 2e-3);
        near(p.across, flight::AIR_TOP, 1e-4);
        // The coasting landing is well past the catch.
        assert!(superbounce::catch_miss(seam.edge, [2.0 * 43.0 / 30.0, 0.0]) > 0.7);
        let let_go = predict(
            &[seam],
            &Body {
                stick: [0.0; 2],
                ..jump
            },
        )
        .unwrap();
        near(let_go.at[0], 1.763, 2e-3);
        assert_eq!(let_go.verdict, Course::Fixable);
        assert!(let_go.steer[0] > 0.99 && let_go.steer[1].abs() < 1e-4, "{:?}", let_go.steer);
    }

    /// A seam further than the measured stick can make up is a miss,
    /// whatever a guessed acceleration says: from rest, 31 ticks over the
    /// floor, a full stick covers 0.579, short of a seam 1.2 off (the
    /// old gate, half of 3 units/s² times the time squared plus 0.4, said
    /// 2.1).
    #[test]
    fn a_seam_beyond_the_sticks_reach_is_a_miss() {
        use superbounce_mode::Course;
        let seam = Spot {
            edge: [[1.2, -1.0, 0.0], [1.2, 1.0, 0.0]],
            ..spot_at([1.2, 0.0, 0.0], [0.0, 0.0])
        };
        let body = falling([0.0, 0.0, 2.4], [0.0; 3]);
        let p = predict(&[seam], &body).unwrap();
        assert_eq!(p.verdict, Course::Miss);
        // The stick to hold is still toward it, all but full.
        assert!(p.steer[0] > 0.9 && p.steer[1] == 0.0, "{:?}", p.steer);
        assert_eq!(
            judge(&body, &seam, &Ground::open()).map(|_| ()),
            Err(Refusal::OutOfReach)
        );
    }

    #[test]
    fn the_crouch_button_is_read_by_the_layouts_index() {
        let mut pad = Gamepad::default();
        assert_eq!(button_down(&pad, DEFAULT_CROUCH_BUTTON), Some(false));
        pad.buttons = input::gamepad::Buttons::LEFT_THUMB;
        assert_eq!(button_down(&pad, DEFAULT_CROUCH_BUTTON), Some(true));
        assert_eq!(button_down(&pad, 15), Some(false));
        pad.analog[AnalogButton::B.index()] = PRESSED;
        assert_eq!(button_down(&pad, 1), Some(true));
        assert_eq!(button_down(&pad, 16), None);
        assert_eq!(crouch_button(None), DEFAULT_CROUCH_BUTTON);
        publish_crouch(Some(true));
        assert_eq!(crouch_held(), Some(true));
        publish_crouch(None);
        assert_eq!(crouch_held(), None);
    }

    // ---- the choice ----

    /// A strip along X from `x0` to `x1` at `y` and height `z`, crease
    /// side toward -Y: stand on its middle, drift +Y.
    fn strip_along_x(x0: f32, x1: f32, y: f32, z: f32) -> Spot {
        Spot {
            edge: [[x0, y, z], [x1, y, z]],
            ..spot_at([(x0 + x1) / 2.0, y - 0.02, z + 0.01], [0.0, 1.0])
        }
    }

    fn verdict_of(c: &Choice, stand: Vec3) -> Verdict {
        c.candidates
            .iter()
            .find(|k| k.stand == stand)
            .expect("the strip was judged")
            .verdict
    }

    #[test]
    fn the_point_to_land_on_moves_along_a_long_strip_and_stays_inside_its_ends() {
        let s = strip_along_x(-2.0, 2.0, 0.0, 0.0);
        let p = stand_near(&s, [1.2, 0.5]);
        near(p[0], 1.2, 1e-5);
        near(p[1], -0.02, 1e-5);
        near(p[2], 0.01, 1e-5);
        // Past an end: a capsule's radius inside it.
        near(stand_near(&s, [5.0, 0.0])[0], 2.0 - END_MARGIN, 1e-5);
        near(stand_near(&s, [-9.0, 3.0])[0], -2.0 + END_MARGIN, 1e-5);
        // A strip too short to have a choice, and a point that is no
        // number: the stand point.
        let short = strip_along_x(-0.1, 0.1, 0.0, 0.0);
        assert_eq!(stand_near(&short, [0.09, 0.0]), short.stand);
        assert_eq!(stand_near(&s, [f32::NAN, 0.0]), s.stand);
    }

    /// The landing-point rule: a measured landing is aimed at as it is,
    /// wherever the fall is along the strip; a scan's stand is led by
    /// [`AIM_BACK`] on the side the drift comes from. The prediction
    /// carries the same point, and the speed along the drift.
    #[test]
    fn the_aim_is_the_measured_landing_or_the_edge_led_on_the_approach_side() {
        // Ascension's seam along x at y 4.000, drifting +y; the proven
        // drop came down at (-10.457, 3.976).
        let scan = Spot {
            edge: [[-11.10, 4.0, -3.2], [-9.98, 4.0, -3.2]],
            ..spot_at([-10.54, 4.0, -3.19], [0.0, 1.0])
        };
        let proven = Spot {
            stand: [-10.457, 3.976, -3.19],
            landed: true,
            ..scan
        };
        let a = aim_at(&scan, scan.stand);
        near(a[0], -10.54, 1e-5);
        near(a[1], 4.0 - AIM_BACK, 1e-5);
        assert_eq!(aim_at(&proven, proven.stand), [-10.457, 3.976]);
        // The assist steers to it: 0.024 before the seam, not 0.1.
        let body = falling([-10.457, 3.6, -0.8], [0.0; 3]);
        let r = reach(&body, &proven).unwrap();
        near(r.aim[0], -10.457, 1e-5);
        near(r.aim[1], 3.976, 1e-5);
        near(reach(&body, &scan).unwrap().aim[1], 3.9, 1e-5);
        // Along the strip the measured place across it is kept.
        let along = aim_at(&proven, stand_near(&proven, [-10.0, 3.0]));
        near(along[0], -10.155, 1e-4);
        near(along[1], 3.976, 1e-5);
        // The prediction's aim and drift are the same.
        let p = predict(&[proven], &falling([-10.457, 3.6, -0.8], [0.0, 0.5, -1.0])).unwrap();
        near(p.aim[1], 3.976, 1e-5);
        assert_eq!(p.drift, [0.0, 1.0]);
        near(p.across, 0.5, 1e-6);
        // A V's arm drifts away from its approach side: its point is on
        // that side of its stand.
        let arm = spot_at([0.0, 0.3, 0.0], [0.0, 1.0]);
        let mirror = spot_at([0.0, -0.3, 0.0], [0.0, -1.0]);
        near(aim_at(&arm, arm.stand)[1], 0.3 - AIM_BACK, 1e-6);
        near(aim_at(&mirror, mirror.stand)[1], -0.3 + AIM_BACK, 1e-6);
    }

    /// A landing wants a push along the drift while it comes down still
    /// or against it, and none on a strip with no drift.
    #[test]
    fn a_landing_wants_the_drift_the_recorded_bounces_had() {
        let s = spot_at([0.0; 3], [0.0, 2.0]);
        near(across(&s, [3.0, 0.5]), 0.5, 1e-6);
        near(across(&s, [3.0, -0.5]), -0.5, 1e-6);
        assert!(wants_drift(s.drift, 0.0));
        assert!(wants_drift(s.drift, -1.0));
        assert!(wants_drift(s.drift, DRIFT_MIN - 0.01));
        assert!(!wants_drift(s.drift, DRIFT_MIN));
        assert!(!wants_drift(s.drift, 1.3));
        assert!(wants_drift(s.drift, f32::NAN));
        assert!(!wants_drift([0.0; 2], 0.0));
        assert_eq!(across(&spot_at([0.0; 3], [0.0; 2]), [3.0, 0.5]), 0.0);
    }

    #[test]
    fn a_fall_onto_one_end_of_a_long_strip_is_not_pulled_to_its_middle() {
        let s = strip_along_x(-2.0, 2.0, 0.0, 0.0);
        // Facing +X, so the stick's forward is along the strip.
        let p = plan(&[s], &falling([1.5, 0.3, 2.0], [0.0; 3])).unwrap();
        assert_eq!(p.target, s.stand);
        near(p.point[0], 1.5, 1e-5);
        near(p.reach.aim[0], 1.5, 1e-5);
        near(p.reach.aim[1], -0.02 - AIM_BACK, 1e-5);
        // Nothing along the strip: only across it, onto it (-Y is right):
        // the 0.42 over the 29 ticks left, 0.43 units/s, a stick of 0.36.
        near(p.stick[1], 0.0, 1e-4);
        assert!(p.stick[0] > 0.3, "{:?}", p.stick);
    }

    #[test]
    fn a_strip_under_a_floor_is_refused_and_the_one_on_it_is_chosen() {
        // The player falls over a floor at height 0. One strip is on it, a
        // unit to the side; another is right below the player and 1.5
        // under the floor.
        let on = spot_at([1.0, 0.0, 0.0], [1.0, 0.0]);
        let under = spot_at([0.1, 0.0, -1.5], [1.0, 0.0]);
        let spots = [on, under];
        let body = falling([0.1, 0.0, 4.0], [0.0; 3]);
        // With the spots alone the one right below wins.
        assert_eq!(pick(&spots, &body).unwrap().0.stand, under.stand);
        let floor = |_: [f32; 2], top: f32| (top >= 0.0).then_some(0.0);
        let ground = Ground {
            floor: &floor,
            ..Ground::open()
        };
        let c = choose(&spots, &body, &ground);
        assert_eq!(c.plan.unwrap().target, on.stand);
        assert_eq!(c.reason, Reason::Nearest);
        assert_eq!(verdict_of(&c, on.stand), Verdict::Chosen);
        assert_eq!(
            verdict_of(&c, under.stand),
            Verdict::Refused(Refusal::Covered)
        );
        // The strip's own floor, a step or a kerb by it do not cover it.
        assert!(!covered(2.0, on.stand, &ground));
        let kerb = |_: [f32; 2], _: f32| Some(COVER - 0.05);
        assert!(!covered(
            2.0,
            on.stand,
            &Ground {
                floor: &kerb,
                ..Ground::open()
            }
        ));
        // The ledge the player has just left, a little over the feet, does.
        let ledge = |_: [f32; 2], top: f32| (top >= 2.2).then_some(2.2);
        assert!(covered(
            2.0,
            on.stand,
            &Ground {
                floor: &ledge,
                ..Ground::open()
            }
        ));
    }

    #[test]
    fn a_strip_behind_a_wall_is_refused() {
        let spot = spot_at([1.0, 0.0, 0.0], [1.0, 0.0]);
        let body = falling([0.0, 0.0, 4.0], [0.0; 3]);
        // A wall at x 0.5.
        let wall = |a: Vec3, b: Vec3| (a[0] < 0.5) == (b[0] < 0.5);
        let ground = Ground {
            clear: &wall,
            ..Ground::open()
        };
        let c = choose(&[spot], &body, &ground);
        assert_eq!(c.plan, None);
        assert_eq!(c.reason, Reason::Refused);
        assert_eq!(
            verdict_of(&c, spot.stand),
            Verdict::Refused(Refusal::Walled)
        );
        // From the strip's side of it, it is in reach.
        let c = choose(&[spot], &falling([0.8, 0.0, 4.0], [0.0; 3]), &ground);
        assert_eq!(c.reason, Reason::Nearest);
    }

    #[test]
    fn the_marked_strip_is_chosen_over_a_nearer_one() {
        let nearer = spot_at([0.3, 0.0, 0.0], [1.0, 0.0]);
        let marked = spot_at([-0.8, 0.0, 0.0], [1.0, 0.0]);
        let spots = [nearer, marked];
        let body = falling([0.0, 0.0, 4.0], [0.0; 3]);
        let c = choose(&spots, &body, &Ground::open());
        assert_eq!(
            (c.plan.unwrap().target, c.reason),
            (nearer.stand, Reason::Nearest)
        );
        let ground = Ground {
            marked: Some(marked.stand),
            ..Ground::open()
        };
        let c = choose(&spots, &body, &ground);
        assert_eq!(
            (c.plan.unwrap().target, c.reason),
            (marked.stand, Reason::Marked)
        );
        assert_eq!(verdict_of(&c, nearer.stand), Verdict::Passed);
        assert_eq!(c.candidates[c.chosen.unwrap()].stand, marked.stand);
        // A mark that is out of reach is not steered onto.
        let far = Ground {
            marked: Some([40.0, 0.0, 0.0]),
            ..Ground::open()
        };
        let c = choose(&spots, &body, &far);
        assert_eq!(
            (c.plan.unwrap().target, c.reason),
            (nearer.stand, Reason::Nearest)
        );
    }

    #[test]
    fn the_strip_steered_onto_is_kept_while_it_is_in_reach() {
        let a = spot_at([0.3, 0.0, 0.0], [1.0, 0.0]);
        let b = spot_at([-0.8, 0.0, 0.0], [1.0, 0.0]);
        let spots = [a, b];
        // The last read steered onto b; this one a is nearer and marked.
        let ground = Ground {
            marked: Some(a.stand),
            kept: Some(b.stand),
            ..Ground::open()
        };
        let c = choose(&spots, &falling([0.0, 0.0, 4.0], [0.0; 3]), &ground);
        assert_eq!((c.plan.unwrap().target, c.reason), (b.stand, Reason::Kept));
        // Once b is out of reach (too little fall left) the mark is taken.
        let c = choose(&spots, &falling([0.25, 0.0, 0.1], [0.0; 3]), &ground);
        assert_eq!(
            (c.plan.unwrap().target, c.reason),
            (a.stand, Reason::Marked)
        );
        assert_eq!(
            verdict_of(&c, b.stand),
            Verdict::Refused(Refusal::OutOfReach)
        );
    }

    #[test]
    fn a_strip_the_fall_is_fast_enough_for_is_chosen_before_a_slow_one() {
        // A short hop over one strip, with a long fall to another beside
        // it: the hop is too slow to bounce, the fall is not.
        let slow = spot_at([0.1, 0.0, 0.0], [1.0, 0.0]);
        let fast = spot_at([0.5, 0.0, -2.0], [1.0, 0.0]);
        let spots = [slow, fast];
        let c = choose(&spots, &falling([0.0, 0.0, 0.3], [0.0; 3]), &Ground::open());
        assert_eq!(c.plan.unwrap().target, fast.stand);
        let judged = |stand: Vec3| *c.candidates.iter().find(|k| k.stand == stand).unwrap();
        assert!(judged(slow.stand).speed < judged(slow.stand).need);
        assert!(judged(fast.stand).speed >= judged(fast.stand).need);
        assert!(judged(slow.stand).miss.unwrap() < judged(fast.stand).miss.unwrap());
    }

    #[test]
    fn the_choice_says_why_there_is_none() {
        let spot = spot_at([4.0, 0.0, 0.0], [1.0, 0.0]);
        let body = falling([0.0, 0.0, 2.0], [0.0; 3]);
        // On the ground or rising.
        let ground = Body {
            airborne: false,
            ..body
        };
        let c = choose(&[spot], &ground, &Ground::open());
        assert_eq!((c.reason, c.candidates.len()), (Reason::NotFalling, 0));
        // 4 units away: judged, and refused as too far.
        let c = choose(&[spot], &body, &Ground::open());
        assert_eq!((c.reason, c.plan), (Reason::Refused, None));
        assert_eq!(verdict_of(&c, spot.stand), Verdict::Refused(Refusal::Far));
        near(c.candidates[0].distance, 4.0, 1e-5);
        // Past what is looked at, crouch spots, and no spots at all.
        let away = spot_at([CONSIDER + 1.0, 0.0, 0.0], [1.0, 0.0]);
        let crouch = Spot {
            kind: Kind::Crouch,
            ..spot_at([0.0, 0.0, 0.0], [0.0, 0.0])
        };
        let c = choose(&[away, crouch], &body, &Ground::open());
        assert_eq!((c.reason, c.candidates.len()), (Reason::NoStrip, 0));
        assert_eq!(choose(&[], &body, &Ground::open()).reason, Reason::NoStrip);
        // Each refusal has its name on the wire.
        let names: Vec<_> = [
            Refusal::Above,
            Refusal::Long,
            Refusal::Far,
            Refusal::OutOfReach,
            Refusal::Covered,
            Refusal::Walled,
        ]
        .map(|r| Verdict::Refused(r).name())
        .to_vec();
        assert_eq!(
            names,
            ["above", "long", "far", "out_of_reach", "covered", "walled"]
        );
    }

    #[test]
    fn the_report_lists_the_chosen_strip_first_then_the_rest_nearest_first() {
        let chosen = spot_at([0.6, 0.0, 0.0], [1.0, 0.0]);
        let passed = spot_at([-0.9, 0.0, 0.0], [1.0, 0.0]);
        let far = spot_at([0.0, 4.0, 0.0], [1.0, 0.0]);
        let above = spot_at([0.1, 0.0, 5.0], [1.0, 0.0]);
        let body = falling([0.0, 0.0, 4.0], [0.0; 3]);
        let spots = [above, far, passed, chosen];
        let c = choose(&spots, &body, &Ground::open());
        let r = Report::of_choice(&body, &c, true);
        let order: Vec<_> = r.candidates.iter().map(|k| k.verdict.name()).collect();
        assert_eq!(order, ["chosen", "passed", "above", "far"]);
        assert!(r.engaged);
        assert_eq!(r.target, Some(chosen.stand));
        assert_eq!(r.point, Some(chosen.stand));
        assert_eq!(r.reason, Some(Reason::Nearest));
        assert_eq!(
            (r.from, r.velocity),
            (Some(body.position), Some(body.velocity))
        );
        // The reply names them the same way.
        let wire = choice_reply(&r).unwrap();
        assert_eq!(wire.reason, "nearest");
        assert_eq!(wire.candidates[0].verdict, "chosen");
        assert_eq!(wire.candidates[2].verdict, "above");
        assert!(wire.candidates[2].point.is_none());
        // Held with nothing to steer onto: the candidates and the reason,
        // and no target.
        let none = choose(&[far], &body, &Ground::open());
        let r = Report::of_choice(&body, &none, false);
        assert!(!r.engaged && r.target.is_none() && r.stick.is_none());
        assert_eq!(r.reason, Some(Reason::Refused));
        assert_eq!(r.candidates.len(), 1);
        assert!(choice_reply(&r).is_none());
        // At most REPORTED of them.
        let many: Vec<Spot> = (0..REPORTED + 8)
            .map(|i| spot_at([0.1 * i as f32, 0.0, 0.0], [1.0, 0.0]))
            .collect();
        let r = Report::of_choice(&body, &choose(&many, &body, &Ground::open()), true);
        assert_eq!(r.candidates.len(), REPORTED);
        assert_eq!(r.candidates[0].verdict, Verdict::Chosen);
    }

    #[test]
    fn the_predicted_landing_skips_a_strip_under_a_floor() {
        let on = spot_at([0.4, 0.0, 0.0], [0.0, 0.0]);
        let under = spot_at([0.0, 0.0, -1.5], [0.0, 0.0]);
        let spots = [on, under];
        let body = falling([0.0, 0.0, 2.0], [0.0, 0.0, -1.0]);
        assert_eq!(predict(&spots, &body).unwrap().stand, under.stand);
        let floor = |_: [f32; 2], top: f32| (top >= 0.0).then_some(0.0);
        let ground = Ground {
            floor: &floor,
            ..Ground::open()
        };
        assert_eq!(predict_on(&spots, &body, &ground).unwrap().stand, on.stand);
    }

    #[test]
    fn the_mark_is_what_the_mode_last_set() {
        set_marked(Some([1.0, 2.0, 3.0]));
        assert_eq!(marked(), Some([1.0, 2.0, 3.0]));
        set_marked(None);
        assert_eq!(marked(), None);
    }
}
