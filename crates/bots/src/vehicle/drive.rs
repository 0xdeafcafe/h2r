//! In a vehicle (docs/game/bots.md, section 3.9, "Drive"): following a
//! route, steering, getting unstuck, and when to get out. Pure.
//!
//! Halo 2 steers a Ghost or a Warthog by the camera: the left stick says
//! where to go relative to where the player looks, and the vehicle turns
//! to go there. So the driver looks where it wants to go (a carrot down
//! the route, or the infantry it means to run over) and pushes the stick
//! that way, as on foot; its speed comes down for a sharp turn. Ground
//! routes use chassis-width walk portals, with room for the wheels and
//! hull checked again before cutting a corner.

use super::Spec;
use crate::brain::Vec3;
use crate::cache::MapData;

/// A route point within this (XY) is passed.
pub const REACHED: f32 = 1.5;
/// The carrot: the furthest route point within this (XY), else the next.
pub const CARROT: f32 = 4.0;
/// At the end of the route, within this it stops.
pub const ARRIVED: f32 = 2.0;
/// The throttle at a turn of this many radians or more off the aim is
/// [`TURN_THROTTLE`]; full when straight ahead.
pub const SHARP_TURN: f32 = 1.6;
pub const TURN_THROTTLE: f32 = 0.35;
/// Stuck: throttle on and slower than this (world units a second)...
pub const STUCK_SPEED: f32 = 0.4;
/// ...for this many ticks: back up for [`REVERSE_TICKS`].
pub const STUCK_TICKS: u32 = 45;
pub const REVERSE_TICKS: u32 = 30;
/// Stuck this many times within [`STUCK_WINDOW`]: get out.
pub const STUCK_TRIES: u32 = 3;
pub const STUCK_WINDOW: u32 = 15 * 30;
/// Gets out under this body vitality.
pub const LEAVE_HEALTH: f32 = 0.2;
/// Gets out after this many ticks upside down or on its side.
pub const FLIPPED_TICKS: u32 = 30;
/// A gunner or passenger with nobody driving gets out after this long.
pub const DRIVERLESS_TICKS: u32 = 4 * 30;
/// Within this (XY) of a goal it must reach on foot (the enemy flag, the
/// ball), it gets out.
pub const ON_FOOT: f32 = 3.0;

fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// A route for a vehicle: the nav path's points to a goal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Route {
    points: Vec<Vec3>,
    next: usize,
    pub goal: Vec3,
}

impl Route {
    /// Through `points` (the nav path, its last point at or near `goal`).
    pub fn new(points: Vec<Vec3>, goal: Vec3) -> Route {
        Route {
            points,
            next: 0,
            goal,
        }
    }

    /// Straight there (no nav mesh).
    pub fn straight(goal: Vec3) -> Route {
        Route::new(vec![goal], goal)
    }

    pub fn done(&self) -> bool {
        self.next >= self.points.len()
    }

    /// Where to drive at from `at`: passed points are dropped, then the
    /// furthest of the next few within [`CARROT`], else the next. `None`
    /// once the route's end is within [`ARRIVED`].
    pub fn carrot(&mut self, at: Vec3) -> Option<Vec3> {
        // A vehicle overshoots: a point whose next is nearer is behind it.
        while self.next + 1 < self.points.len()
            && dist_xy(self.points[self.next + 1], at) <= dist_xy(self.points[self.next], at)
        {
            self.next += 1;
        }
        while let Some(p) = self.points.get(self.next) {
            let last = self.next + 1 == self.points.len();
            let reach = if last { ARRIVED } else { REACHED };
            if dist_xy(*p, at) > reach {
                break;
            }
            self.next += 1;
        }
        let first = *self.points.get(self.next)?;
        Some(
            self.points[self.next..]
                .iter()
                .take(4)
                .take_while(|p| dist_xy(**p, at) <= CARROT)
                .last()
                .copied()
                .unwrap_or(first),
        )
    }

    /// Follow the route in order. Only cut a corner if the direct span
    /// still fits the chassis, even when a waypoint is already nearby.
    pub fn carrot_clear(&mut self, at: Vec3, clear: impl Fn(Vec3) -> bool) -> Option<Vec3> {
        while self.next + 1 < self.points.len()
            && dist_xy(at, self.points[self.next]) < REACHED
            && clear(self.points[self.next + 1])
        {
            self.next += 1;
        }
        let first = *self.points.get(self.next)?;
        if self.next + 1 == self.points.len() && dist_xy(at, first) <= ARRIVED {
            self.next += 1;
            return None;
        }
        if !clear(first) {
            return None;
        }
        Some(
            self.points[self.next + 1..]
                .iter()
                .copied()
                .take(3)
                .take_while(|p| dist_xy(*p, at) <= CARROT && clear(*p))
                .last()
                .unwrap_or(first),
        )
    }
}

/// The lanes under the wheels and hull all stay on drivable ground, and
/// the chassis fits under overhead geometry and between walls.
pub fn clear(map: &MapData, spec: Spec, from: Vec3, to: Vec3) -> bool {
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let length = dx.hypot(dy);
    if length < 0.01 {
        return true;
    }
    let side = [
        -dy * spec.half_width / length,
        dx * spec.half_width / length,
    ];
    let Some((_, center)) = map.nav.nearest(from, 0.25, 0.45, 1.5) else {
        return false;
    };
    let steps = (length / 0.45).ceil() as u32;
    for lane in [-1.0, 0.0, 1.0] {
        let [sx, sy] = side.map(|v| lane * v);
        let mut last = None;
        let mut z = center[2];
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let probe = [from[0] + dx * t + sx, from[1] + dy * t + sy, z + 0.35];
            let Some(poly) = map.nav.locate(probe, 0.70) else {
                return false;
            };
            let floor = map.nav.polys[poly as usize].z_at(probe);
            let q = [probe[0], probe[1], floor];
            let up = |p: Vec3, h: f32| [p[0], p[1], p[2] + h];
            if !map.walls.clear(up(q, 0.12), up(q, spec.height))
                || last.is_some_and(|p| {
                    !map.walls.clear(up(p, 0.26), up(q, 0.26))
                        || !map.walls.clear(up(p, spec.height), up(q, spec.height))
                })
            {
                return false;
            }
            last = Some(q);
            z = floor;
        }
    }
    true
}

/// How far the throttle goes for a turn `off` radians from the aim.
pub fn throttle(off: f32) -> f32 {
    let t = (off.abs() / SHARP_TURN).min(1.0);
    1.0 - t * (1.0 - TURN_THROTTLE)
}

/// Getting a vehicle unstuck: backs up when it does not move under
/// throttle, and gives up after a few tries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unstick {
    /// The first tick of the current slow spell under throttle.
    slow_since: Option<u32>,
    /// Backing up until this tick.
    reverse_until: u32,
    /// The ticks of the recent tries, and where the next goes.
    tries: [Option<u32>; STUCK_TRIES as usize],
    cursor: usize,
}

/// What [`Unstick::step`] says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unstuck {
    /// Drive on.
    Drive,
    /// Back up.
    Reverse,
    /// Stuck for good: get out.
    GiveUp,
}

impl Unstick {
    /// One tick at `speed` (world units a second), `pushing` whether it
    /// holds the throttle forward.
    pub fn step(&mut self, tick: u32, speed: f32, pushing: bool) -> Unstuck {
        if tick < self.reverse_until {
            return Unstuck::Reverse;
        }
        if !pushing || speed >= STUCK_SPEED {
            self.slow_since = None;
            return Unstuck::Drive;
        }
        let since = *self.slow_since.get_or_insert(tick);
        if tick.wrapping_sub(since) < STUCK_TICKS {
            return Unstuck::Drive;
        }
        self.slow_since = None;
        let recent = self
            .tries
            .iter()
            .flatten()
            .filter(|&&t| tick.wrapping_sub(t) <= STUCK_WINDOW)
            .count() as u32;
        if recent + 1 >= STUCK_TRIES {
            *self = Unstick::default();
            return Unstuck::GiveUp;
        }
        self.tries[self.cursor] = Some(tick);
        self.cursor = (self.cursor + 1) % self.tries.len();
        self.reverse_until = tick + REVERSE_TICKS;
        Unstuck::Reverse
    }

    pub fn reset(&mut self) {
        *self = Unstick::default();
    }
}

/// What the bot knows of its ride, for [`leave`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ride {
    pub health: f32,
    /// Ticks it has lain flipped (0 upright).
    pub flipped: u32,
    /// Ticks nobody has driven it, for a gunner or passenger.
    pub driverless: u32,
    pub driver: bool,
    /// Stuck for good ([`Unstuck::GiveUp`]).
    pub stuck: bool,
    /// Within [`ON_FOOT`] of a goal it must reach on foot.
    pub there: bool,
}

/// Why it gets out, if it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leave {
    /// About to blow up.
    Wrecked,
    Flipped,
    Stuck,
    /// A gun with nobody driving.
    Stranded,
    /// Where it was going needs feet.
    There,
    Flanked,
}

pub fn leave(r: &Ride) -> Option<Leave> {
    if r.health < LEAVE_HEALTH {
        Some(Leave::Wrecked)
    } else if r.flipped >= FLIPPED_TICKS {
        Some(Leave::Flipped)
    } else if r.driver && r.stuck {
        Some(Leave::Stuck)
    } else if !r.driver && r.driverless >= DRIVERLESS_TICKS {
        Some(Leave::Stranded)
    } else if r.there {
        Some(Leave::There)
    } else {
        None
    }
}

/// Whether to drive at an enemy on foot to run them over: the skill does,
/// the vehicle is one for it, and they are in front within `reach`.
pub fn splatter(splatters: bool, at: Vec3, facing: [f32; 2], foe: Vec3, reach: f32) -> bool {
    if !splatters {
        return false;
    }
    let to = [foe[0] - at[0], foe[1] - at[1]];
    let d = (to[0] * to[0] + to[1] * to[1]).sqrt();
    d > 0.3 && d < reach && (to[0] * facing[0] + to[1] * facing[1]) / d > 0.3
}

/// Splatters infantry within this, in front (world units).
pub const SPLATTER_RANGE: f32 = 15.0;

#[cfg(test)]
mod tests;
