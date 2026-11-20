//! On foot (docs/game/bots.md, section 3.9, "Go and take", "Board"):
//! which vehicle to go for and where to stand to get in. Pure.
//!
//! Getting in is the game's own: the bot stands by the seat and holds the
//! action button (X), as a player does; the game picks the seat nearest
//! its entry point. So the planner picks a vehicle and a seat, and a point
//! beside that seat ([`approach`]) inside the tag's entry limits.

use super::{Class, Entry, SeatRole, Sight, Vehicle, Wheels, flat};
use crate::brain::Vec3;
use crate::cache::MapData;
use crate::mind::EYE;

/// Within this (XY) of the point beside the seat, it holds X.
pub const ENTER_RANGE: f32 = 0.33;
/// Holds X this long at one side, then tries the other.
pub const ENTER_TICKS: u32 = 40;
/// Gives up on a vehicle it has not got into after this long.
pub const MOUNT_TIMEOUT: u32 = 12 * 30;
/// Gives up a boarding after this long.
pub const BOARD_TIMEOUT: u32 = 3 * 30;
/// A vehicle it gave up on, or just got out of, is left alone this long.
pub const SHUN_TICKS: u32 = 10 * 30;
/// Takes no vehicle worse than this (body vitality).
pub const TAKE_HEALTH: f32 = 0.4;
/// Takes no vehicle moving faster than this (world units a second):
/// somebody is pushing or shooting it about.
pub const TAKE_SPEED: f32 = 0.5;
/// A teammate this much nearer to a vehicle, and within [`CLAIM_RANGE`]
/// of it, has it.
pub const CLAIM_SHARE: f32 = 0.8;
pub const CLAIM_RANGE: f32 = 6.0;
/// Each world unit of height between the bot and a vehicle counts as this
/// many across (it is on another floor).
pub const CLIMB_COST: f32 = 3.0;
/// Boards only a vehicle on its own level (world units).
pub const BOARD_HEIGHT: f32 = 1.0;
/// A rider's seat counted as a gunner's is worth this many world units
/// less than a driver's (a free Warthog beats the gun on a teammate's).
pub const GUNNER_COST: f32 = 4.0;

/// Why it goes to a vehicle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A free one, to drive it or to man its gun.
    Take,
    /// An enemy's, to throw them out.
    Board,
}

/// A vehicle to go to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub why: Why,
    /// The vehicle object to get in: the vehicle, or the turret on it for
    /// a gunner's seat.
    pub vehicle: u32,
    /// The whole vehicle (`vehicle` itself unless that is a turret).
    pub root: u32,
    pub role: SeatRole,
}

/// The bot on foot, as the planners need it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Foot {
    pub at: Vec3,
    pub team: i16,
    /// A team game: same-team players are friends.
    pub teams: bool,
}

impl Foot {
    fn friend(&self, team: i16) -> bool {
        self.teams && team == self.team
    }
}

fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// The cost of walking from `at` to `to`: across, plus height.
fn cost(at: Vec3, to: Vec3) -> f32 {
    dist_xy(at, to) + CLIMB_COST * (to[2] - at[2]).abs()
}

/// A free vehicle to take, if one is worth it: the nearest (by
/// [`cost`]) within the skill's range whose driver's seat is free and
/// with nobody of the other side aboard, in good shape, the right way up
/// and standing still; that no teammate is clearly nearer to; that
/// `shunned` does not rule out (one it gave up on). With
/// [`Wheels::gunner`], the gun on a teammate's vehicle is one too.
///
/// `friends` are the teammates on foot (positions); `speed` a vehicle's
/// speed in world units a second.
pub fn take(
    me: &Foot,
    sight: &Sight,
    friends: &[Vec3],
    w: &Wheels,
    speed: impl Fn(u32) -> f32,
    shunned: impl Fn(u32) -> bool,
) -> Option<Pick> {
    if w.take_range <= 0.0 {
        return None;
    }
    let mut best: Option<(f32, Pick)> = None;
    for v in sight.vehicles.iter().filter(|v| v.on.is_none()) {
        let spec = v.class.spec();
        let d = cost(me.at, v.position);
        if !spec.drive
            || d > w.take_range
            || shunned(v.id)
            || v.health < TAKE_HEALTH
            || !v.upright()
            || speed(v.id) > TAKE_SPEED
        {
            continue;
        }
        let mut driven = false;
        if sight.crew(v.id).any(|(r, role)| {
            driven |= role == SeatRole::Driver;
            !me.friend(r.team)
        }) {
            continue;
        }
        let pick = if v.enterable(SeatRole::Driver) && !driven {
            // A teammate walking to it first has it.
            let claimed = friends
                .iter()
                .any(|f| dist_xy(*f, v.position) < CLAIM_RANGE.min(d * CLAIM_SHARE));
            if claimed {
                continue;
            }
            (
                d,
                Pick {
                    why: Why::Take,
                    vehicle: v.id,
                    root: v.id,
                    role: SeatRole::Driver,
                },
            )
        } else if w.gunner && driven {
            // A teammate drives: its gun, if one is free.
            let Some(gun) = sight.parts(v.id).find(|p| p.enterable(SeatRole::Gunner)) else {
                continue;
            };
            (
                d + GUNNER_COST,
                Pick {
                    why: Why::Take,
                    vehicle: gun.id,
                    root: v.id,
                    role: SeatRole::Gunner,
                },
            )
        } else {
            continue;
        };
        if best.as_ref().is_none_or(|(c, _)| pick.0 < *c) {
            best = Some(pick);
        }
    }
    best.map(|(_, p)| p)
}

/// An enemy vehicle to board: one an enemy drives, within the skill's
/// boarding range on the bot's level, going slower than its boarding
/// speed. The nearest.
pub fn board(
    me: &Foot,
    sight: &Sight,
    w: &Wheels,
    speed: impl Fn(u32) -> f32,
    shunned: impl Fn(u32) -> bool,
) -> Option<Pick> {
    if w.board_range <= 0.0 || w.board_speed <= 0.0 {
        return None;
    }
    sight
        .vehicles
        .iter()
        .filter(|v| v.on.is_none() && v.class != Class::Fighter && !shunned(v.id))
        .filter(|v| {
            sight
                .crew(v.id)
                .any(|(r, role)| role == SeatRole::Driver && !me.friend(r.team))
                && v.seats.iter().any(|s| {
                    !s.entries.is_empty()
                        && s.boards
                            .and_then(|i| v.seats.get(i as usize))
                            .is_some_and(|driver| {
                                driver.rider.is_some() && driver.role == SeatRole::Driver
                            })
                })
        })
        .filter(|v| (v.position[2] - me.at[2]).abs() < BOARD_HEIGHT)
        .filter(|v| speed(v.id) <= w.board_speed)
        .map(|v| (dist_xy(me.at, v.position), v))
        .filter(|(d, _)| *d <= w.board_range)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, v)| Pick {
            why: Why::Board,
            vehicle: v.id,
            root: v.id,
            role: SeatRole::Driver,
        })
}

/// The closest stand point that meets the selected seat's entry geometry.
pub fn approach(
    vehicle: &Vehicle,
    pick: Pick,
    from: Vec3,
    attempt: u32,
    map: Option<&MapData>,
) -> Option<(Vec3, Entry)> {
    vehicle
        .seats
        .iter()
        .filter(|seat| match pick.why {
            Why::Take => seat.role == pick.role && seat.rider.is_none() && seat.boards.is_none(),
            Why::Board => seat
                .boards
                .and_then(|i| vehicle.seats.get(i as usize))
                .is_some_and(|s| s.role == SeatRole::Driver && s.rider.is_some()),
        })
        .flat_map(|seat| seat.entries.iter().copied())
        .filter_map(|entry| {
            let [x, y] = flat(entry.forward);
            let clearance = entry.radius * if attempt == 0 { 0.72 } else { 0.42 };
            let raw = [
                entry.at[0] + x * clearance,
                entry.at[1] + y * clearance,
                from[2],
            ];
            let point = if let Some(map) = map {
                map.nav
                    .nearest([raw[0], raw[1], entry.at[2]], 0.55, 0.3, 2.5)?
                    .1
            } else {
                raw
            };
            (in_entry(vehicle, entry, point)
                && map.is_none_or(|m| m.walls.clear(eye(point), entry.at)))
            .then_some((point, entry))
        })
        .min_by(|(p, _), (q, _)| cost(from, *p).total_cmp(&cost(from, *q)))
}

fn eye(at: Vec3) -> Vec3 {
    [at[0], at[1], at[2] + EYE]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] - b[i])
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    (0..3).map(|i| a[i] * b[i]).sum()
}

pub fn in_entry(vehicle: &Vehicle, entry: Entry, at: Vec3) -> bool {
    let from_marker = sub(eye(at), entry.at);
    let d = dot(from_marker, from_marker).sqrt();
    let from_vehicle = sub(at, vehicle.position);
    d <= entry.radius
        && dot(from_vehicle, from_vehicle) <= entry.max_distance.powi(2)
        && (d < 1e-4 || dot(from_marker, entry.forward) >= d * entry.facing.cos())
}

pub fn looking_at_entry(entry: Entry, at: Vec3, aim: Vec3) -> bool {
    let to = sub(entry.at, eye(at));
    dot(to, aim) >= dot(to, to).sqrt() * entry.look.cos()
}

/// Pick a stationary or friendly mounted gun that can see a contested point.
pub fn take_turret(
    me: &Foot,
    sight: &Sight,
    contested: Vec3,
    map: Option<&MapData>,
    shunned: impl Fn(u32) -> bool,
) -> Option<Pick> {
    sight
        .vehicles
        .iter()
        .filter_map(|gun| {
            if gun.class != Class::Turret
                || !gun.enterable(SeatRole::Gunner)
                || gun.health < TAKE_HEALTH
                || !gun.upright()
            {
                return None;
            }
            let root = sight.root(gun);
            if root.health < TAKE_HEALTH
                || !root.upright()
                || shunned(root.id)
                || gun.on.is_some()
                    && !sight
                        .crew(root.id)
                        .any(|(r, role)| role == SeatRole::Driver && me.friend(r.team))
                || sight.crew(root.id).any(|(r, _)| !me.friend(r.team))
            {
                return None;
            }
            let range = dist_xy(gun.position, contested);
            if !(4.0..=crate::mind::FIRE_RANGE).contains(&range)
                || map.is_some_and(|m| !m.walls.clear(eye(gun.position), eye(contested)))
            {
                return None;
            }
            let pick = Pick {
                why: Why::Take,
                vehicle: gun.id,
                root: root.id,
                role: SeatRole::Gunner,
            };
            let (at, _) = approach(gun, pick, me.at, 0, map)?;
            let d = cost(me.at, at);
            (d <= 8.0).then_some((d, pick))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, pick)| pick)
}

/// A threat on the flank is more urgent than keeping the gun aimed downrange.
pub fn flanked(at: Vec3, aim: Vec3, foes: impl IntoIterator<Item = Vec3>) -> bool {
    let front = flat(aim);
    foes.into_iter().any(|foe| {
        let to = sub(foe, at);
        let range = to[0].hypot(to[1]);
        to[2].abs() < 1.8
            && (range < 1.8 || range < 5.0 && to[0] * front[0] + to[1] * front[1] < 0.55 * range)
    })
}

#[cfg(test)]
mod tests;
