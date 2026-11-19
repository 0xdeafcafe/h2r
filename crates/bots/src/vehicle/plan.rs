//! On foot (docs/game/bots.md, section 3.9, "Go and take", "Board"):
//! which vehicle to go for and where to stand to get in. Pure.
//!
//! Getting in is the game's own: the bot stands by the seat and holds the
//! action button (X), as a player does; the game picks the seat nearest
//! its entry point. So the planner picks a vehicle and a seat, and a point
//! beside that seat ([`approach`]) from the vehicle's [`Spec`].

use super::{Class, SeatRole, Sight, Spec, Vehicle, Wheels};
use crate::brain::Vec3;

/// Within this (XY) of the point beside the seat, it holds X.
pub const ENTER_RANGE: f32 = 0.7;
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
        let crew: Vec<_> = sight.crew(v.id).collect();
        if crew.iter().any(|(r, _)| !me.friend(r.team)) {
            continue;
        }
        let driven = crew.iter().any(|(_, role)| *role == SeatRole::Driver);
        let pick = if v.free(SeatRole::Driver).is_some() && !driven {
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
            let Some(gun) = sight
                .parts(v.id)
                .find(|p| p.free(SeatRole::Gunner).is_some())
            else {
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

/// Where to stand to get in `role`'s seat of the vehicle `root`, on `side`
/// (0 the spec's side, 1 the other; a driver on the right side of a
/// Warthog gets its passenger's seat, which is still a ride).
pub fn approach(root: &Vehicle, role: SeatRole, side: u32) -> Vec3 {
    let spec: Spec = root.class.spec();
    let [f, l] = match role {
        SeatRole::Driver => spec.driver_at,
        SeatRole::Gunner | SeatRole::Passenger => spec.gunner_at,
    };
    let l = if side % 2 == 1 { -l } else { l };
    root.local([f, l])
}

#[cfg(test)]
mod tests;
