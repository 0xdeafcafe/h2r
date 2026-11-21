//! Navigation and standing jumps for an explicit controller route.

use super::traversal::toward;
use super::{
    Build, FLAT_JUMP_RISE, Follower, JumpArc, JumpSweep, Kind, NavMesh, V3, Waypoint, add, dist,
    dist_xy, scale, standing_clear, sub,
};
use crate::cache::MapData;
use crate::mind::RouteMotion;
use crate::moves;

#[derive(Clone, Debug)]
pub struct RouteWalk {
    follower: Follower,
    goal: V3,
    crouch: bool,
}

impl RouteWalk {
    /// Find a corridor for a player-directed move. Teleporter links are
    /// excluded so a route can preserve the player's movement state.
    pub fn new(
        map: &MapData,
        at: V3,
        goal: V3,
        crouch: bool,
        tick: u32,
    ) -> Result<Self, &'static str> {
        let mut path = map
            .nav
            .path_avoiding(at, goal, &|e| {
                e.kind == Kind::Teleporter || (!crouch && e.kind == Kind::CrouchJump)
            })
            .ok_or("no_path")?;
        // A paired traversal can finish a little way short of its exit.
        // Give the follower a final precise walking target in that case.
        if let [.., entry, exit] = path.as_slice()
            && entry.kind.traversal()
        {
            let exit = *exit;
            path.push(Waypoint {
                kind: Kind::Walk,
                ..exit
            });
        }
        Ok(Self {
            follower: Follower::new(path, u64::from(tick)),
            goal,
            crouch,
        })
    }

    /// The corridor being followed.
    pub fn follower(&self) -> &Follower {
        &self.follower
    }

    /// Where the walk ends.
    pub fn goal(&self) -> V3 {
        self.goal
    }

    /// Follow the bot's corners and traversal links. A move completes
    /// within the caller's three-dimensional arrival radius.
    pub fn step(
        &mut self,
        map: &MapData,
        at: V3,
        tick: u32,
        tolerance: f32,
        speed: f32,
    ) -> Result<Option<RouteMotion>, &'static str> {
        if dist(at, self.goal) <= tolerance {
            return Ok(None);
        }
        if self
            .follower
            .path
            .last()
            .is_none_or(|end| dist(end.at, self.goal) > tolerance)
        {
            return Err("goal_off_mesh");
        }
        if speed == 0.0 {
            return Ok(Some(RouteMotion::default()));
        }
        let steer =
            self.follower
                .steer_until(at, u64::from(tick), |d, _reach| moves::bend_clear(map, at, d), true);
        if steer.replan {
            return Err("path_lost");
        }
        let crouch = steer.crouch || (self.crouch && !steer.traversing);
        let pace = if steer.traversing || steer.jump {
            1.0
        } else {
            let pace = self.follower.pace(at);
            if self.follower.next + 1 == self.follower.path.len() {
                pace.min(stick_speed(5.0 * dist_xy(at, self.goal)).max(0.24))
            } else {
                pace
            }
        };
        let moving = steer.dir != [0.0; 2];
        let clear = crouch
            || steer.jump
            || self.follower.committed()
            || moves::headroom(map, at, steer.dir);
        Ok(Some(RouteMotion {
            dir: steer.dir,
            speed: if moving && clear {
                speed.min(pace)
            } else {
                0.0
            },
            yaw: moving.then(|| steer.dir[1].atan2(steer.dir[0])),
            jump: steer.jump,
            crouch,
        }))
    }
}

// The gravity estimate gives about 5.4 u/s down on the 2.43-unit
// Ascension drop. Six walking ticks per unit implies 5 u/s at 30 Hz.
const GRAVITY: f32 = 5.2;
const RUN_SPEED: f32 = 5.0;
const TAKEOFF_RADIUS: f32 = 0.1;
const LAND_RADIUS: f32 = 0.15;
const MOVE_DEADZONE: f32 = 0.15;

#[derive(Clone, Copy, Debug)]
pub struct RouteJump {
    pub from: V3,
    pub land: V3,
    pub dir: [f32; 2],
    pub run_up: V3,
    /// Planned horizontal takeoff speed, in world units per second.
    pub speed: f32,
    pub flight_seconds: f32,
}

/// Plan a jump near `from` to the requested landing floor. A standing
/// capsule must clear the same centre and shoulder sweep as a bot link.
pub fn plan_route_jump(map: &MapData, from: V3, land: V3) -> Option<RouteJump> {
    let landing = floor_point(&map.nav, land, LAND_RADIUS)?;
    let direction = toward(from, landing);
    [0.0, -0.06, 0.06].into_iter().find_map(|offset| {
        let candidate = add(from, [offset * direction[0], offset * direction[1], 0.0]);
        let takeoff = floor_point(&map.nav, candidate, TAKEOFF_RADIUS)?;
        if dist(takeoff, from) > TAKEOFF_RADIUS || !standing_clear(&map.walls, takeoff) {
            return None;
        }
        let vz = (2.0 * GRAVITY * FLAT_JUMP_RISE).sqrt();
        let flight_seconds = descending_time(takeoff[2] - landing[2], vz)?;
        let speed = dist_xy(takeoff, landing) / flight_seconds;
        if speed > RUN_SPEED {
            return None;
        }
        let lift = 0.5 * GRAVITY * flight_seconds.powi(2);
        let arc = JumpArc::sample(takeoff, landing, lift);
        let mut common_clear = 0;
        if arc.sweep(&map.walls, false, &mut common_clear) != JumpSweep::Clear {
            return None;
        }
        let dir = toward(takeoff, landing);
        let run_up = if speed <= 0.01 {
            takeoff
        } else {
            run_up(map, takeoff, dir)?
        };
        Some(RouteJump {
            from: takeoff,
            land: landing,
            dir,
            run_up,
            speed,
            flight_seconds,
        })
    })
}

fn descending_time(height: f32, vz: f32) -> Option<f32> {
    let discriminant = vz * vz + 2.0 * GRAVITY * height;
    (discriminant >= 0.0)
        .then(|| (vz + discriminant.sqrt()) / GRAVITY)
        .filter(|t| *t > 0.0)
}

fn floor_point(nav: &NavMesh, at: V3, radius: f32) -> Option<V3> {
    let (_, floor) = nav.nearest(at, radius, radius, radius)?;
    (dist(at, floor) <= radius).then_some(floor)
}

fn run_up(map: &MapData, from: V3, dir: [f32; 2]) -> Option<V3> {
    let back = |distance: f32| add(from, [-distance * dir[0], -distance * dir[1], 0.0]);
    [0.6, 0.45, 0.3].into_iter().find_map(|distance| {
        let floor = |at: V3| {
            // Preserve XY so a snap across an edge can't produce a run-up.
            let (_, point) = map.nav.nearest(at, 0.0, 0.25, 0.25)?;
            standing_clear(&map.walls, point).then_some(point)
        };
        let start = floor(back(distance))?;
        let middle = floor(back(distance / 2.0))?;
        let step = Build::default().step;
        if (start[2] - middle[2]).abs() > step || (middle[2] - from[2]).abs() > step {
            return None;
        }
        let knee = [0.0, 0.0, moves::KNEE];
        map.walls
            .clear(add(start, knee), add(from, knee))
            .then_some(start)
    })
}

fn stick_speed(speed: f32) -> f32 {
    if speed <= 0.01 {
        0.0
    } else {
        MOVE_DEADZONE + (1.0 - MOVE_DEADZONE) * (speed / RUN_SPEED).clamp(0.0, 1.0)
    }
}

impl RouteJump {
    /// Ground input converges on the planned velocity along the takeoff
    /// line. Steering out lateral error keeps that velocity aimed at land.
    pub fn run_motion(self, at: V3, velocity: V3) -> RouteMotion {
        let sideways = [-self.dir[1], self.dir[0]];
        let offset = sub(self.from, at);
        let lateral_error = offset[0] * sideways[0] + offset[1] * sideways[1];
        let lateral_speed = (2.0 * lateral_error).clamp(-0.6, 0.6);
        let toward = [
            self.dir[0] * self.speed + sideways[0] * lateral_speed,
            self.dir[1] * self.speed + sideways[1] * lateral_speed,
        ];
        self.steer_velocity(toward, velocity)
    }

    /// True in the takeoff window with velocity close to the plan. Include
    /// a predicted tick because the game applies the pad after this sample.
    pub fn launch_ready(self, at: V3, velocity: V3, ticks_per_second: f32) -> bool {
        let next = add(at, scale(velocity, 1.0 / ticks_per_second.max(1.0)));
        let vx = velocity[0] - self.dir[0] * self.speed;
        let vy = velocity[1] - self.dir[1] * self.speed;
        dist(at, self.from).min(dist(next, self.from)) <= TAKEOFF_RADIUS
            && vx.hypot(vy) <= (0.2 * self.speed).max(0.35)
    }

    /// Whether the player passed the takeoff window without jumping.
    pub fn missed_takeoff(self, at: V3) -> bool {
        let past = sub(at, self.from);
        past[0] * self.dir[0] + past[1] * self.dir[1] > TAKEOFF_RADIUS
    }

    /// Aim the horizontal velocity at the landing using the remaining fall
    /// time from the observed height and vertical speed.
    pub fn air_motion(self, at: V3, velocity: V3) -> RouteMotion {
        let t = descending_time(at[2] - self.land[2], velocity[2])
            .unwrap_or(0.0)
            .max(0.12);
        let offset = sub(self.land, at);
        let want = [offset[0] / t, offset[1] / t];
        self.steer_velocity(want, velocity)
    }

    fn steer_velocity(self, want: [f32; 2], velocity: V3) -> RouteMotion {
        let dx = want[0] + 0.5 * (want[0] - velocity[0]);
        let dy = want[1] + 0.5 * (want[1] - velocity[1]);
        let speed = dx.hypot(dy);
        RouteMotion {
            dir: [dx / speed.max(1e-6), dy / speed.max(1e-6)],
            speed: stick_speed(speed),
            yaw: (self.dir != [0.0; 2]).then(|| self.dir[1].atan2(self.dir[0])),
            ..RouteMotion::default()
        }
    }
}
