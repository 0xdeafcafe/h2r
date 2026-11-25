//! The tick-stepped flight the takeoff gate steers before handing the
//! fall to `superbounce::rule::predict`. The route's own planning uses
//! the analytic model in `bounce_route`; this small model is only for
//! judging that takeoff against the bounce rule.

use crate::brain::Vec3;

const TICK: f32 = 1.0 / 30.0;
const GRAVITY: f32 = 4.171;
const MAX_TICKS: u32 = 240;
const UNDER: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    pub row: Vec3,
    pub ticks: u32,
    pub speed: f32,
    pub velocity: [f32; 2],
}

fn unit_disc(w: [f32; 2]) -> [f32; 2] {
    let n = w[0].hypot(w[1]);
    if n > 1.0 { [w[0] / n, w[1] / n] } else { w }
}

fn effective(stick: [f32; 2]) -> [f32; 2] {
    let n = stick[0].hypot(stick[1]);
    if !n.is_finite() || n < super::MIN_STICK {
        return [0.0; 2];
    }
    unit_disc(stick)
}

fn air_step(v: [f32; 2], stick: [f32; 2]) -> [f32; 2] {
    let s = effective(stick);
    let want = [s[0] * super::AIR_TOP, s[1] * super::AIR_TOP];
    let d = [want[0] - v[0], want[1] - v[1]];
    let n = d[0].hypot(d[1]);
    let most = super::AIR_ACCEL * TICK;
    if n > most {
        [v[0] + d[0] * most / n, v[1] + d[1] * most / n]
    } else {
        want
    }
}

fn fly(
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

fn hold(
    position: Vec3,
    velocity: Vec3,
    stick: [f32; 2],
    surface: impl Fn([f32; 2]) -> f32,
) -> Option<Landing> {
    fly(position, velocity, |_, _, _| stick, surface)
}

fn fall(z: f32, vz: f32, height: f32) -> Option<Landing> {
    hold([0.0, 0.0, z], [0.0, 0.0, vz], [0.0; 2], |_| height)
}

/// The highest a body at `z` moving `vz` gets on the same tick steps.
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pursuit {
    pub landing: Landing,
    pub miss: f32,
    pub stick_now: [f32; 2],
}

/// Fly toward `aim` until the flight reaches `surface`.
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
            let s = unit_disc([want[0] / super::AIR_TOP, want[1] / super::AIR_TOP]);
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
