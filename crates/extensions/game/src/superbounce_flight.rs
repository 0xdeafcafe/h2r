//! The superbounce flight model (docs/game/superbounce.md, "The flight
//! model"): a body in free flight, stepped a game tick at a time the way
//! the engine steps it. Everything that predicts a landing flies it: the
//! approach guide (`superbounce_guide`), the fall assist and the readout's
//! landing (`superbounce_assist`), and the second bounce
//! (`superbounce_second`).
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
//! - **Reach** ([`pursue`]): the best simple steering toward a point,
//!   flown on the same model. How near it ends is what the stick can still
//!   do; its first stick is the one to hold now.
//!
//! The numbers are measured, none guessed: the air stick's in the bot
//! drill's runs (docs/game/notes/2026-11-21-bot-bounce-sim.md, "the drill
//! four ways"), gravity in the sim's control drops
//! (docs/game/notes/2026-11-22-seam-drop-depth.md).

use superbounce::Vec3;

/// A game tick (seconds).
pub const TICK: f32 = 1.0 / 30.0;
/// Gravity (units/s²): the sim's fall model, fitted to the control drops
/// (`sim_bench::drops::FALL_GRAVITY`): the fall speed gains 0.13904 units/s
/// a tick.
pub const GRAVITY: f32 = sim_bench::drops::FALL_GRAVITY;
/// How fast the air stick changes the sideways velocity (units/s²): 0.035
/// units/s a tick in every drill run, braking and pushing
/// (`bots::bounce_route::AIR_ACCEL`).
pub const AIR_ACCEL: f32 = bots::bounce_route::AIR_ACCEL;
/// The sideways speed a full air stick drives toward (units/s): a run-jump
/// at 2 units/s slows to it with the stick held on
/// (`bots::bounce_route::AIR_TOP`).
pub const AIR_TOP: f32 = bots::bounce_route::AIR_TOP;
/// A stick shorter than this is no stick: the drill runs' standing jumps
/// held 0.15 toward the drift and gained no sideways speed
/// (`bots::bounce_route::MIN_STICK`).
pub const DEAD_ZONE: f32 = bots::bounce_route::MIN_STICK;
/// The longest flight flown: five seconds.
pub const MAX_TICKS: u32 = 150;
/// A last free row this far under the surface at its own place is not a
/// landing: the body never got over the surface.
pub const UNDER: f32 = 0.05;

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
/// no real stick is known: the takeoff solver, the tests, a seat with no
/// throttle read.
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
/// the same tick steps as [`fly`]: `z` itself when it is not rising.
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

#[cfg(test)]
mod tests {
    use sim_bench::drops::{FALL_CLEARANCE, FALL_OFFSET, FALL_STILL_ROWS, FALL_V0, fall_model};

    use super::*;

    fn near(a: f32, b: f32, eps: f32) {
        assert!((a - b).abs() <= eps, "{a} is not within {eps} of {b}");
    }

    fn flat(h: f32) -> impl Fn([f32; 2]) -> f32 {
        move |_| h
    }

    /// The numbers are the measured ones.
    #[test]
    fn the_model_is_the_measured_one() {
        near(AIR_ACCEL * TICK, 0.035, 1e-6);
        assert_eq!((AIR_TOP, DEAD_ZONE), (1.2, 0.2));
        near(GRAVITY * TICK, 0.13904, 1e-4);
    }

    /// The video's run-jump: off the floor at 2.0 units/s with the stick
    /// held forward, the speed drops 0.035 a tick to 1.2 and holds there;
    /// with the stick let go it drops the same way to a stop and stays.
    #[test]
    fn a_run_jump_brakes_to_the_top_with_the_stick_and_to_a_stop_without() {
        let mut v = [2.0, 0.0];
        for k in 1..=60 {
            v = air_step(v, [1.0, 0.0]);
            let want = (2.0 - 0.035 * k as f32).max(AIR_TOP);
            near(v[0], want, 1e-4);
            assert_eq!(v[1], 0.0);
        }
        let mut v = [2.0, 0.0];
        for k in 1..=80 {
            v = air_step(v, [0.0, 0.0]);
            near(v[0], (2.0 - 0.035 * k as f32).max(0.0), 1e-4);
        }
        assert_eq!(v, [0.0, 0.0]);
        // From rest a full stick pushes at the same rate, toward the top.
        near(air_step([0.0; 2], [0.0, -1.0])[1], -0.035, 1e-6);
        // A stick past the unit disc is the full stick that way.
        assert_eq!(
            air_step([0.0; 2], [3.0, 0.0]),
            air_step([0.0; 2], [1.0, 0.0])
        );
    }

    /// A stick inside the dead zone is none: the body brakes.
    #[test]
    fn a_dead_zone_stick_is_no_stick() {
        assert_eq!(effective([0.15, 0.0]), [0.0, 0.0]);
        assert_eq!(effective([0.0, 0.199]), [0.0, 0.0]);
        assert_eq!(effective([0.2, 0.0]), [0.2, 0.0]);
        assert_eq!(effective([f32::NAN, 0.0]), [0.0, 0.0]);
        assert_eq!(
            air_step([0.5, 0.0], [0.1, 0.0]),
            air_step([0.5, 0.0], [0.0; 2])
        );
        near(air_step([0.5, 0.0], [0.1, 0.0])[0], 0.465, 1e-6);
    }

    /// The throttle's forward is the facing and its left is the facing
    /// turned a quarter left; the keeping stick is the speed over the top.
    #[test]
    fn the_stick_is_in_world_axes() {
        use std::f32::consts::FRAC_PI_2;
        let same = |a: [f32; 2], b: [f32; 2]| {
            near(a[0], b[0], 1e-6);
            near(a[1], b[1], 1e-6);
        };
        same(held([1.0, 0.0], 0.0), [1.0, 0.0]);
        same(held([0.0, 1.0], 0.0), [0.0, 1.0]);
        same(held([1.0, 0.0], FRAC_PI_2), [0.0, 1.0]);
        same(held([0.0, 1.0], FRAC_PI_2), [-1.0, 0.0]);
        same(held([1.0, 1.0], 0.0), [std::f32::consts::FRAC_1_SQRT_2; 2]);
        assert_eq!(held([f32::NAN, 0.0], 0.0), [0.0, 0.0]);
        assert_eq!(held([1.0, 0.0], f32::INFINITY), [0.0, 0.0]);
        same(keep([0.6, 0.0]), [0.5, 0.0]);
        same(keep([2.4, 0.0]), [1.0, 0.0]);
        assert_eq!(keep([f32::NAN, 0.0]), [0.0, 0.0]);
        // Under the keeping stick a body at or under the top keeps its
        // speed; one over it slows.
        near(air_step([0.6, 0.0], keep([0.6, 0.0]))[0], 0.6, 1e-6);
        near(air_step([2.0, 0.0], keep([2.0, 0.0]))[0], 1.965, 1e-6);
    }

    /// The video's run-jump, landed: off a floor at 2.0 units/s and the
    /// jump's 3.08 up, onto the same height. The vertical comes back down
    /// on row 43 either way. With the stick held forward the speed falls
    /// 0.035 a tick for 22 ticks (35.145 / 30 units) and holds 1.2 for 21
    /// (25.2 / 30): it lands 2.0115 out. Let go, it brakes all 43 ticks
    /// (52.89 / 30): 1.763 out. Coasting says 2.0 × 43 / 30 = 2.867, 0.86
    /// too far.
    #[test]
    fn a_braking_run_jump_lands_well_short_of_the_coasting_one() {
        let v = [2.0, 0.0, bots::bounce_route::JUMP_VZ];
        let on = hold([0.0; 3], v, [1.0, 0.0], flat(0.0)).unwrap();
        assert_eq!(on.ticks, 43);
        near(on.row[0], 2.0115, 2e-3);
        near(on.velocity[0], AIR_TOP, 1e-4);
        near(on.speed, 2.898, 2e-3);
        let off = hold([0.0; 3], v, [0.0, 0.0], flat(0.0)).unwrap();
        assert_eq!(off.ticks, 43);
        near(off.row[0], 1.763, 2e-3);
        near(off.velocity[0], 0.495, 1e-3);
        let coasting = 2.0 * 43.0 * TICK;
        assert!(coasting - on.row[0] > 0.8, "{on:?}");
    }

    /// A plumb drop is the sim's fall model: a drop from each of its
    /// heights, taken up where its fall starts (the still rows over, at
    /// its first speed), touches down from the model's row at its speed.
    #[test]
    fn a_plumb_drop_is_the_sims_fall_model() {
        for height in [2.43, 3.45, 3.65, 3.82, 4.5, 5.5] {
            let (n, v) = fall_model(height);
            let l = hold(
                [0.0, 0.0, height + FALL_OFFSET],
                [0.0, 0.0, -FALL_V0],
                [0.0; 2],
                flat(FALL_CLEARANCE),
            )
            .unwrap();
            assert_eq!(l.ticks, n - FALL_STILL_ROWS, "{height}");
            near(l.speed, v, 1e-3);
            assert_eq!((l.row[0], l.row[1], l.velocity), (0.0, 0.0, [0.0; 2]));
            let f = fall(height + FALL_OFFSET, -FALL_V0, FALL_CLEARANCE).unwrap();
            assert_eq!((f.ticks, f.speed), (l.ticks, l.speed));
        }
    }

    /// A rising flight lands only on its way down: onto a surface over
    /// the start and under the apex (1.137 up for a jump's 3.08), after
    /// the top; never onto one over the apex, nor one it starts under.
    #[test]
    fn a_rising_flight_lands_on_the_way_down() {
        let up = [0.0, 0.0, bots::bounce_route::JUMP_VZ];
        let l = hold([0.0; 3], up, [0.0; 2], flat(0.5)).unwrap();
        // The top is on row 22 (3.08 / 0.139).
        assert!(l.ticks > 22 && l.row[2] > 0.5, "{l:?}");
        assert!(l.row[2] - (l.speed + GRAVITY * TICK) * TICK <= 0.5);
        assert_eq!(hold([0.0; 3], up, [0.0; 2], flat(1.5)), None);
        assert_eq!(hold([0.0; 3], [0.0; 3], [0.0; 2], flat(1.0)), None);
        // Falling from rest onto the floor it stands a hair over.
        assert_eq!(
            hold([0.0, 0.0, 0.001], [0.0; 3], [0.0; 2], flat(0.0))
                .unwrap()
                .ticks,
            0
        );
    }

    /// The pursuit gets to a point inside what the stick can do, and says
    /// how far short it ends of one outside it. A drop of 4.0 from rest
    /// takes 41 ticks; a full stick from rest covers 0.974 in them.
    #[test]
    fn the_pursuit_reaches_inside_the_envelope_and_misses_outside_it() {
        for aim in [[0.5, 0.0], [0.9, 0.0], [0.3, 0.4], [-0.6, -0.2]] {
            let p = pursue([0.0, 0.0, 4.0], [0.0; 3], aim, flat(0.0)).unwrap();
            assert!(p.miss < 0.02, "{aim:?}: {p:?}");
            assert_eq!(p.landing.ticks, 41);
            // The first stick points at the aim.
            let s = p.stick_now;
            near(s[0] * aim[1] - s[1] * aim[0], 0.0, 1e-4);
            assert!(s[0] * aim[0] + s[1] * aim[1] > 0.0);
        }
        let out = pursue([0.0, 0.0, 4.0], [0.0; 3], [3.0, 0.0], flat(0.0)).unwrap();
        near(out.miss, 3.0 - 0.974, 5e-3);
        assert_eq!(out.stick_now, [1.0, 0.0]);
        // Moving away from the aim, the reach is smaller.
        let away = pursue([0.0, 0.0, 4.0], [-1.0, 0.0, 0.0], [0.9, 0.0], flat(0.0)).unwrap();
        assert!(away.miss > 0.5, "{away:?}");
        // Onto nothing, or toward no number: none.
        assert_eq!(
            pursue([0.0, 0.0, -1.0], [0.0; 3], [0.5, 0.0], flat(0.0)),
            None
        );
        assert_eq!(
            pursue([0.0, 0.0, 4.0], [0.0; 3], [f32::NAN, 0.0], flat(0.0)),
            None
        );
    }

    /// A number that is not one has no landing.
    #[test]
    fn nan_in_is_no_landing() {
        assert_eq!(
            hold([f32::NAN, 0.0, 2.0], [0.0; 3], [0.0; 2], flat(0.0)),
            None
        );
        assert_eq!(
            hold(
                [0.0, 0.0, 2.0],
                [0.0, f32::INFINITY, 0.0],
                [0.0; 2],
                flat(0.0)
            ),
            None
        );
        assert_eq!(fall(f32::NAN, 0.0, 0.0), None);
        // A NaN stick is no stick.
        let l = hold([0.0, 0.0, 2.0], [0.0; 3], [f32::NAN, 1.0], flat(0.0)).unwrap();
        assert_eq!((l.row[0], l.row[1]), (0.0, 0.0));
    }
}
