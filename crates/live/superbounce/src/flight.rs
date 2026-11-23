//! The superbounce flight model (docs/game/superbounce.md, "The flight
//! model"): a body in free flight, stepped a game tick at a time the way
//! the engine steps it. Everything that predicts a landing flies it: the
//! game's approach guide, fall assist, readout landing and second bounce
//! (`superbounce_flight` in the game crate re-exports this module), and
//! the bots' routes and their flight net (`bots::bounce_route`,
//! `bots::bounce_route::chain`), so a bot and the HUD agree on where a
//! jump comes down.
//!
//! - **The vertical**: each tick the fall speed gains [`GRAVITY`]`×`[`TICK`],
//!   then the body moves by the new speed times [`TICK`]
//!   (`sim_bench::drops::fall_model`, fitted to the control drops).
//! - **The sideways** ([`air_step`]): the stick drives the sideways
//!   velocity toward the stick times [`AIR_TOP`], changing it by at most
//!   [`AIR_ACCEL`]`×`[`TICK`] a tick, braking and pushing alike. A stick
//!   inside the dead zone ([`DEAD_ZONE`]) is no stick: the body brakes
//!   toward a stop. Then the body moves by the new velocity.
//! - **The landing** ([`fly`]): the last row before the body comes down
//!   onto a surface, a height over each point. Only a downward crossing
//!   lands, so a rising flight lands on its way down.
//! - **The stick** ([`held`], [`keep`]): the player's, from the unit's
//!   throttle or the pad, in world axes; or, where no real stick exists,
//!   the one that keeps the body going as it goes.
//! - **Reach** ([`pursue`], [`reach_along`]): the best simple steering
//!   toward a point, flown on the same model. How near it ends is what the
//!   stick can still do; its first stick is the one to hold now.
//! - **Rank** ([`cost`]): how hard a seam is to come down on, the steer's
//!   share of the reach and the speed to spare.
//!
//! Every position is the body's origin (object `+0x64`); a surface's
//! height is the origin's height standing on it.
//!
//! The numbers are measured, none guessed: the air stick's in the bot
//! drill's runs (docs/game/notes/2026-11-21-bot-bounce-sim.md, "the drill
//! four ways"), gravity in the sim's control drops
//! (docs/game/notes/2026-11-22-seam-drop-depth.md). The game crate's
//! tests hold [`GRAVITY`] to `sim_bench::drops::FALL_GRAVITY`.

use crate::Vec3;

/// A game tick (seconds).
pub const TICK: f32 = 1.0 / 30.0;
/// Gravity (units/s²): the sim's fall model, fitted to the control drops
/// (`sim_bench::drops::FALL_GRAVITY`): the fall speed gains 0.13904 units/s
/// a tick.
pub const GRAVITY: f32 = 4.171;
/// How fast the air stick changes the sideways velocity (units/s²): 0.035
/// units/s a tick in every drill run, braking and pushing.
pub const AIR_ACCEL: f32 = 1.05;
/// The sideways speed a full air stick drives toward (units/s): under a
/// stick of 0.8 to 1 pushing on, the drill runs' flights held 0.9 to 1.3
/// and never went faster; a run-jump at 2 units/s slows to it with the
/// stick held on.
pub const AIR_TOP: f32 = 1.2;
/// A stick shorter than this is no stick: the drill runs' standing jumps
/// held 0.15 toward the drift and gained no sideways speed.
pub const DEAD_ZONE: f32 = 0.2;
/// The longest flight flown: eight seconds, the longest the second
/// bounce ranks (a 16 units/s Monster launch is back down in under 8).
pub const MAX_TICKS: u32 = 240;
/// A last free row this far under the surface at its own place is not a
/// landing: the body never got over the surface.
pub const UNDER: f32 = 0.05;
/// What the spare speed weighs in a seam's [`cost`] against the steer's
/// share of the reach.
pub const SPEED_WEIGHT: f32 = 0.5;

/// Where a flight comes down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    /// The last free row: the body's place on the tick before the one
    /// that would be at or under the surface.
    pub row: Vec3,
    /// Game ticks from the start to that row.
    pub ticks: u32,
    /// The fall speed on that row (units/s down): the touchdown speed.
    pub speed: f32,
    /// The sideways velocity on that row (units/s).
    pub velocity: [f32; 2],
}

impl Landing {
    /// Seconds from the start to the last free row.
    pub fn time(&self) -> f32 {
        self.ticks as f32 * TICK
    }
}

fn unit_disc(w: [f32; 2]) -> [f32; 2] {
    let n = w[0].hypot(w[1]);
    if n > 1.0 { [w[0] / n, w[1] / n] } else { w }
}

/// The stick as the engine takes it (world axes): cut back to the unit
/// disc keeping its direction; none inside [`DEAD_ZONE`], or for a stick
/// that is not a number.
pub fn effective(stick: [f32; 2]) -> [f32; 2] {
    let n = stick[0].hypot(stick[1]);
    if !n.is_finite() || n < DEAD_ZONE {
        return [0.0; 2];
    }
    unit_disc(stick)
}

/// One tick of the sideways velocity `v` (units/s) under `stick` (world
/// axes, length 0 to 1): toward the stick times [`AIR_TOP`], changed by
/// at most [`AIR_ACCEL`]`×`[`TICK`].
pub fn air_step(v: [f32; 2], stick: [f32; 2]) -> [f32; 2] {
    let s = effective(stick);
    let want = [s[0] * AIR_TOP, s[1] * AIR_TOP];
    let d = [want[0] - v[0], want[1] - v[1]];
    let n = d[0].hypot(d[1]);
    let most = AIR_ACCEL * TICK;
    if n > most {
        [v[0] + d[0] * most / n, v[1] + d[1] * most / n]
    } else {
        want
    }
}

/// The world-axes stick of a throttle `[forward, left]` (the unit's
/// `+0x1b0`, or the pad's `[y, −x]`) for a body facing `yaw` (radians from
/// +X toward +Y): forward is `(cos yaw, sin yaw)`, left `(−sin yaw, cos
/// yaw)`. Cut back to the unit disc; none for a number that is not one.
pub fn held(throttle: [f32; 2], yaw: f32) -> [f32; 2] {
    if !(throttle.iter().all(|v| v.is_finite()) && yaw.is_finite()) {
        return [0.0; 2];
    }
    let (s, c) = yaw.sin_cos();
    let [f, l] = throttle;
    unit_disc([f * c - l * s, f * s + l * c])
}

/// The stick that keeps a body moving `v` going the way it goes: `v` over
/// [`AIR_TOP`], cut back to the unit disc. A body faster than the top
/// slows to it under this stick, and one slower than the dead zone's
/// speed (`DEAD_ZONE × AIR_TOP`) brakes, as the engine's would. For where
/// no real stick is known: the takeoff solver, the bots' air steering,
/// the tests, a seat with no throttle read.
pub fn keep(v: [f32; 2]) -> [f32; 2] {
    if !v.iter().all(|x| x.is_finite()) {
        return [0.0; 2];
    }
    unit_disc([v[0] / AIR_TOP, v[1] / AIR_TOP])
}

/// The flight of a body at `position` moving `velocity`, with `stick(xy,
/// v, ticks)` the stick (world axes) held on each row, until it comes
/// down onto `surface` (the height the body is stopped at over a point).
/// Each tick the fall speed gains [`GRAVITY`]`×`[`TICK`] and the sideways
/// velocity takes one [`air_step`]; then the body moves by both times
/// [`TICK`]. The last row whose next is moving down and at or under the
/// surface is the [`Landing`].
///
/// `None` when it does not come down within [`MAX_TICKS`], when the last
/// free row is more than [`UNDER`] under the surface (it came down from
/// under it), or for a number that is not one.
pub fn fly(
    position: Vec3,
    velocity: Vec3,
    mut stick: impl FnMut([f32; 2], [f32; 2], u32) -> [f32; 2],
    surface: impl Fn([f32; 2]) -> f32,
) -> Option<Landing> {
    if !position.iter().chain(&velocity).all(|v| v.is_finite()) {
        return None;
    }
    let (mut p, mut down, mut v) = (position, -velocity[2], [velocity[0], velocity[1]]);
    for ticks in 0..=MAX_TICKS {
        let next_down = down + GRAVITY * TICK;
        let next_v = air_step(v, stick([p[0], p[1]], v, ticks));
        let next = [
            p[0] + next_v[0] * TICK,
            p[1] + next_v[1] * TICK,
            p[2] - next_down * TICK,
        ];
        if next_down > 0.0 && next[2] <= surface([next[0], next[1]]) {
            let over = p[2] - surface([p[0], p[1]]);
            return (over >= -UNDER).then_some(Landing {
                row: p,
                ticks,
                speed: down.max(0.0),
                velocity: v,
            });
        }
        (p, down, v) = (next, next_down, next_v);
    }
    None
}

/// [`fly`] with one stick held the whole way.
pub fn hold(
    position: Vec3,
    velocity: Vec3,
    stick: [f32; 2],
    surface: impl Fn([f32; 2]) -> f32,
) -> Option<Landing> {
    fly(position, velocity, |_, _, _| stick, surface)
}

/// The vertical alone: a body at height `z` moving `vz` (up positive)
/// onto a floor at `height`. The vertical does not depend on the stick.
pub fn fall(z: f32, vz: f32, height: f32) -> Option<Landing> {
    hold([0.0, 0.0, z], [0.0, 0.0, vz], [0.0; 2], |_| height)
}

/// The highest a body at height `z` moving `vz` (up positive) gets, on
/// the same steps as [`fly`]: `z` itself for a body not rising. `z` for a
/// number that is not one.
pub fn apex(z: f32, vz: f32) -> f32 {
    if !(z.is_finite() && vz.is_finite()) {
        return z;
    }
    let (mut top, mut at, mut up) = (z, z, vz);
    for _ in 0..=MAX_TICKS {
        up -= GRAVITY * TICK;
        if up <= 0.0 {
            break;
        }
        at += up * TICK;
        top = top.max(at);
    }
    top
}

/// What the stick can still do toward a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pursuit {
    /// Where the pursuit comes down.
    pub landing: Landing,
    /// How far (X and Y) that is from the point: 0 when the stick gets
    /// there.
    pub miss: f32,
    /// The stick to hold now (world axes, as the engine takes it,
    /// [`effective`]).
    pub stick_now: [f32; 2],
}

/// The flight of a body at `position` moving `velocity` steered toward
/// `aim` (X and Y) onto `surface`, with the best simple steering: the
/// vertical alone gives the ticks `n` to the surface's height at the aim
/// ([`fall`]); each row the velocity wanted is the way to the aim over
/// the time left (`n` less the row, at least a tick), and the stick is
/// that over [`AIR_TOP`], cut back to the unit disc. The velocity follows
/// it by [`air_step`], as the engine's does. `None` when the flight does
/// not come down onto the surface.
pub fn pursue(
    position: Vec3,
    velocity: Vec3,
    aim: [f32; 2],
    surface: impl Fn([f32; 2]) -> f32,
) -> Option<Pursuit> {
    if !aim.iter().all(|v| v.is_finite()) {
        return None;
    }
    let n = fall(position[2], velocity[2], surface(aim))?.ticks;
    let mut first: Option<[f32; 2]> = None;
    let landing = fly(
        position,
        velocity,
        |p, _, ticks| {
            let left = n.saturating_sub(ticks).max(1) as f32 * TICK;
            let want = [(aim[0] - p[0]) / left, (aim[1] - p[1]) / left];
            let s = unit_disc([want[0] / AIR_TOP, want[1] / AIR_TOP]);
            first.get_or_insert(effective(s));
            s
        },
        &surface,
    )?;
    Some(Pursuit {
        landing,
        miss: (landing.row[0] - aim[0]).hypot(landing.row[1] - aim[1]),
        stick_now: first.unwrap_or([0.0; 2]),
    })
}

/// How far the full stick along `dir` (a unit vector, X and Y) moves the
/// landing of a body at `position` moving `velocity` past `landing` (the
/// landing with the stick it has), measured along `dir`; 0 when it comes
/// down short of it or not at all. The stick's reach the way a steer
/// asks for.
pub fn reach_along(
    position: Vec3,
    velocity: Vec3,
    landing: [f32; 2],
    dir: [f32; 2],
    surface: impl Fn([f32; 2]) -> f32,
) -> f32 {
    hold(position, velocity, dir, surface)
        .map_or(0.0, |l| (l.row[0] - landing[0]) * dir[0] + (l.row[1] - landing[1]) * dir[1])
        .max(0.0)
}

/// How hard a seam is to come down on, lower easier: the steer's share of
/// the stick's reach that way (`steer` over `reach`, 0 to 1; 0 with no
/// steer) plus [`SPEED_WEIGHT`] times the seam's `need` over the
/// touchdown `speed` (so 0.5 with no speed to spare, less with more). The
/// game's second bounce and the bots' flight net rank by it.
pub fn cost(steer: f32, reach: f32, need: f32, speed: f32) -> f32 {
    let share = if steer > 1e-6 {
        (steer / reach.max(1e-3)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let spare = if speed > 0.0 { need / speed } else { 1.0 };
    share + SPEED_WEIGHT * spare
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32, eps: f32) {
        assert!((a - b).abs() <= eps, "{a} is not within {eps} of {b}");
    }

    fn flat(h: f32) -> impl Fn([f32; 2]) -> f32 {
        move |_| h
    }

    /// The top of a jump on the tick steps: 3.08 up rises 22 rows.
    #[test]
    fn the_apex_is_the_highest_row() {
        let top = apex(0.0, 3.08);
        // Analytic v²/2g is 1.137; the steps land a little under it.
        assert!(top > 1.05 && top < 1.137, "{top}");
        assert_eq!(apex(2.0, 0.0), 2.0);
        assert_eq!(apex(2.0, -1.0), 2.0);
        // A flight lands on a surface under its apex and never on one
        // over it.
        assert!(hold([0.0; 3], [0.0, 0.0, 3.08], [0.0; 2], flat(top - 0.02)).is_some());
        assert_eq!(hold([0.0; 3], [0.0, 0.0, 3.08], [0.0; 2], flat(top + 2.0 * UNDER)), None);
    }

    /// A Monster's launch comes back down within the longest flight.
    #[test]
    fn a_monster_launch_comes_back_down() {
        let l = fall(0.0, 16.0, 0.0).unwrap();
        assert!(l.time() < 8.0, "{l:?}");
        near(l.speed, 16.0, 0.2);
    }

    /// The reach along a steer is what the full stick adds that way, and
    /// none for a body that cannot get further.
    #[test]
    fn the_reach_along_a_steer_is_the_full_sticks() {
        let from = [0.0, 0.0, 4.0];
        let base = hold(from, [0.0; 3], [0.0; 2], flat(0.0)).unwrap();
        let r = reach_along(from, [0.0; 3], [base.row[0], base.row[1]], [1.0, 0.0], flat(0.0));
        // A drop of 4.0 from rest takes 41 ticks; a full stick from rest
        // covers 0.974 in them.
        near(r, 0.974, 5e-3);
        assert_eq!(reach_along(from, [0.0; 3], [5.0, 0.0], [1.0, 0.0], flat(0.0)), 0.0);
        assert_eq!(reach_along([0.0, 0.0, -1.0], [0.0; 3], [0.0; 2], [1.0, 0.0], flat(0.0)), 0.0);
    }

    /// The cost: the share of the reach plus half the need over the speed.
    #[test]
    fn the_cost_weighs_the_steer_and_the_spare_speed() {
        near(cost(0.0, 1.0, 5.0, 10.0), 0.25, 1e-6);
        near(cost(0.5, 1.0, 5.0, 5.0), 1.0, 1e-6);
        near(cost(3.0, 1.0, 5.0, 5.0), 1.5, 1e-6);
        near(cost(0.5, 0.0, 5.0, 5.0), 1.5, 1e-6);
        near(cost(0.0, 1.0, 5.0, 0.0), 0.5, 1e-6);
        // No need: only the steer counts.
        near(cost(0.25, 1.0, 0.0, 7.0), 0.25, 1e-6);
    }
}
