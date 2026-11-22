//! Navigation and standing jumps for an explicit controller route.

use super::traversal::toward;
use super::{
    Build, FLAT_JUMP_RISE, Follower, JumpArc, JumpSweep, Kind, NavMesh, V3, Waypoint, add, dist,
    dist_xy, scale, standing_clear, sub,
};
use crate::cache::MapData;
use crate::mind::RouteMotion;
use crate::moves;

/// A walk's goal may be off the nav mesh (a scanned bounce or crouch
/// spot sits on a seam or an edge) by this much (XY) from the path's end:
/// the walk follows the path to its end, then steers straight at the goal.
/// Farther off it is `goal_off_mesh`.
pub const TAIL_REACH: f32 = 1.5;
/// The straight last stretch may climb this much...
pub const TAIL_UP: f32 = 0.5;
/// ...or go down this much (a crease below the floors either side of it).
pub const TAIL_DOWN: f32 = 0.8;
/// The last stretch gives up after this long (ticks) without getting
/// [`TAIL_GAIN`] nearer.
pub const TAIL_TICKS: u32 = 45;
pub const TAIL_GAIN: f32 = 0.05;
/// A walk has arrived within its tolerance (XY) and this much (or the
/// tolerance, if more) up or down: a scanned spot's height is the seam's,
/// not the floor's the mesh gives beside it.
pub const ARRIVE_Z: f32 = 0.6;
/// A path's end this near its goal (XY) needs no last stretch.
const TAIL_EPS: f32 = 0.05;

/// Whether a body at `at` has arrived at `goal`, within `tolerance`.
pub fn arrived(at: V3, goal: V3, tolerance: f32) -> bool {
    dist_xy(at, goal) <= tolerance && (at[2] - goal[2]).abs() <= ARRIVE_Z.max(tolerance)
}

/// Whether the straight last stretch from a path's `end` to an off-mesh
/// `goal` can be walked: within [`TAIL_REACH`], [`TAIL_UP`] and
/// [`TAIL_DOWN`], and nothing solid between them at knee height.
fn tail_ok(map: &MapData, end: V3, goal: V3) -> bool {
    let dz = goal[2] - end[2];
    let knee = [0.0, 0.0, moves::KNEE];
    dist_xy(end, goal) <= TAIL_REACH
        && dz <= TAIL_UP
        && -dz <= TAIL_DOWN
        && map.walls.clear(add(end, knee), add(goal, knee))
}

/// The links a route walk never takes: a teleporter (the player's movement
/// state must survive the walk), and a crouch jump unless it walks
/// crouched.
fn skipped(kind: Kind, crouch: bool) -> bool {
    kind == Kind::Teleporter || (!crouch && kind == Kind::CrouchJump)
}

/// The corridor a [`RouteWalk`] from `at` to `goal` follows.
fn walk_path(map: &MapData, at: V3, goal: V3, crouch: bool) -> Option<Vec<Waypoint>> {
    let mut path = map
        .nav
        .path_avoiding(at, goal, &|e| skipped(e.kind, crouch))?;
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
    Some(path)
}

/// The length of the walk a [`RouteWalk`] from `at` to `goal` would take,
/// arriving within `tolerance`: the corridor, then the straight last
/// stretch to a goal off the mesh. `None` when the walk would fail
/// (`no_path`, or `goal_off_mesh`: the goal farther off the mesh than a
/// last stretch reaches). The planner's measure of a walk, so what it
/// plans is what a walk can do.
pub fn walk_length(map: &MapData, at: V3, goal: V3, crouch: bool, tolerance: f32) -> Option<f32> {
    let path = walk_path(map, at, goal, crouch)?;
    let end = path.last()?.at;
    if !arrived(end, goal, tolerance) && !tail_ok(map, end, goal) {
        return None;
    }
    let mut from = at;
    let mut total = 0.0;
    for w in &path {
        total += dist(from, w.at);
        from = w.at;
    }
    Some(total + dist(from, goal))
}

/// The straight last stretch of a walk to a goal off the mesh.
#[derive(Clone, Copy, Debug)]
struct Tail {
    /// Steering straight at the goal (the corridor is done).
    on: bool,
    /// The least distance (XY) to the goal on it, and when.
    best: f32,
    since: u32,
}

#[derive(Clone, Debug)]
pub struct RouteWalk {
    follower: Follower,
    goal: V3,
    crouch: bool,
    /// The last stretch, for a goal off the mesh within [`TAIL_REACH`].
    tail: Option<Tail>,
    /// A last stretch that stops getting nearer ends the walk where the
    /// body got to ([`RouteWalk::ending_near`]), rather than failing.
    near: bool,
}

impl RouteWalk {
    /// Find a corridor for a player-directed move. Teleporter links are
    /// excluded so a route can preserve the player's movement state. A
    /// goal off the mesh within [`TAIL_REACH`] of the corridor's end is
    /// walked to straight from there.
    pub fn new(
        map: &MapData,
        at: V3,
        goal: V3,
        crouch: bool,
        tick: u32,
    ) -> Result<Self, &'static str> {
        let path = walk_path(map, at, goal, crouch).ok_or("no_path")?;
        let tail = path
            .last()
            .filter(|end| !arrived(end.at, goal, TAIL_EPS) && tail_ok(map, end.at, goal))
            .map(|_| Tail {
                on: false,
                best: f32::INFINITY,
                since: tick,
            });
        Ok(Self {
            follower: Follower::new(path, u64::from(tick)),
            goal,
            crouch,
            tail,
            near: false,
        })
    }

    /// The same, with a last stretch that stops getting nearer ending the
    /// walk where the body is, for a walk the next step finishes by its
    /// own steering (a prime's start, a bounce's approach).
    pub fn ending_near(mut self) -> Self {
        self.near = true;
        self
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
    /// within the caller's arrival radius (XY, and [`ARRIVE_Z`] up or
    /// down). A goal off the mesh is reached by the last stretch: straight
    /// at it from the corridor's end.
    pub fn step(
        &mut self,
        map: &MapData,
        at: V3,
        tick: u32,
        tolerance: f32,
        speed: f32,
    ) -> Result<Option<RouteMotion>, &'static str> {
        if arrived(at, self.goal, tolerance) {
            return Ok(None);
        }
        let end = self.follower.path.last().map(|w| w.at);
        let last = self.follower.next + 1 >= self.follower.path.len();
        let stretching = match (self.tail.as_mut(), end) {
            (Some(t), Some(end)) => {
                // At the corridor's end: on to the goal, straight.
                if !t.on && last && dist_xy(at, end) <= super::REACHED {
                    t.on = true;
                    t.best = f32::INFINITY;
                    t.since = tick;
                }
                t.on
            }
            _ if end.is_none_or(|end| !arrived(end, self.goal, tolerance)) => {
                return Err("goal_off_mesh");
            }
            _ => false,
        };
        if stretching {
            return self.last_stretch(at, tick, speed);
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
            if self.follower.next + 1 == self.follower.path.len() && self.tail.is_none() {
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

    /// Straight at the goal, slowing as it nears it, until it is there or
    /// stops getting nearer.
    fn last_stretch(
        &mut self,
        at: V3,
        tick: u32,
        speed: f32,
    ) -> Result<Option<RouteMotion>, &'static str> {
        let Some(t) = self.tail.as_mut() else {
            return Err("goal_off_mesh");
        };
        let goal = self.goal;
        if goal[2] - at[2] > super::FELL {
            // Fell below it.
            return Err("path_lost");
        }
        let d = dist_xy(at, goal);
        if d < t.best - TAIL_GAIN {
            t.best = d;
            t.since = tick;
        }
        if tick.wrapping_sub(t.since) > TAIL_TICKS {
            return if self.near {
                Ok(None)
            } else {
                Err("tail_stuck")
            };
        }
        if speed == 0.0 || d < 1e-4 {
            return Ok(Some(RouteMotion::default()));
        }
        let dir = [(goal[0] - at[0]) / d, (goal[1] - at[1]) / d];
        Ok(Some(RouteMotion {
            dir,
            speed: speed.min(stick_speed(5.0 * d).max(0.24)),
            yaw: Some(dir[1].atan2(dir[0])),
            jump: false,
            crouch: self.crouch,
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
