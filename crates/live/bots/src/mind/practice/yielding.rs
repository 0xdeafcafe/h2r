//! Who is using a site (docs/game/bots.md, "Letting players go first"):
//! the floor a bounce or a prime takes up, the other players as the
//! practice tracks them, and whether one of them is on it, on the way
//! onto it, or about to walk into the bot. Pure: the practice feeds it the
//! perception's players each tick.

use crate::brain::{Seen, Vec3};

/// A site's floor reaches this far (XY, world units) round the line from
/// its approach over its target ...
pub const SITE_RADIUS: f32 = 1.5;
/// ... and runs on this far past the target: the landing.
pub const LAND_PAST: f32 = 1.0;
/// A crouch spot's floor: this far round the line from its prime's start
/// to its stand.
pub const CROUCH_RADIUS: f32 = 1.2;
/// A player counts on a floor from this far under it to this far over
/// it, or higher while in the air over it (a flight off it), to
/// [`AIR_OVER`].
pub const LEVEL_UNDER: f32 = 1.5;
pub const LEVEL_OVER: f32 = 3.0;
pub const AIR_OVER: f32 = 25.0;
/// In the air: moving up or down faster than this (world units a second).
pub const AIRBORNE_VZ: f32 = 1.0;
/// How far ahead (seconds) a player's course is followed, in steps of
/// [`LOOK_STEP`].
pub const LOOK: f32 = 1.5;
pub const LOOK_STEP: f32 = 0.25;
/// A player moving slower than this (world units a second) is not on
/// the way anywhere.
pub const MOVING: f32 = 0.8;
/// What a player on the way onto a floor counts as, against 1 for one
/// on it.
pub const APPROACHING: f32 = 0.8;
/// A moving player whose course passes this close (XY) to a standing bot
/// within [`LOOK`], from no farther than [`PATH_NEAR`], has the bot in
/// their way.
pub const PATH_CLEAR: f32 = 1.0;
pub const PATH_NEAR: f32 = 8.0;
/// A sample older than this (ticks) gives no speed.
const STALE: u32 = 3;
const TICKS: f32 = 30.0;

/// The floor a bounce or a prime takes up: a line and the reach round it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    pub a: Vec3,
    pub b: Vec3,
    pub r: f32,
}

impl Zone {
    /// A bounce's: from its approach over the target and [`LAND_PAST`]
    /// on, [`SITE_RADIUS`] round.
    pub fn site(approach: Vec3, target: Vec3) -> Zone {
        let d = [target[0] - approach[0], target[1] - approach[1]];
        let l = d[0].hypot(d[1]);
        let b = if l > 1e-3 {
            [
                target[0] + d[0] / l * LAND_PAST,
                target[1] + d[1] / l * LAND_PAST,
                target[2],
            ]
        } else {
            target
        };
        Zone {
            a: approach,
            b,
            r: SITE_RADIUS,
        }
    }

    /// A crouch spot's: from its prime's start to its stand,
    /// [`CROUCH_RADIUS`] round.
    pub fn crouch(start: Vec3, stand: Vec3) -> Zone {
        Zone {
            a: start,
            b: stand,
            r: CROUCH_RADIUS,
        }
    }

    /// How far `p` is from its line (XY).
    pub fn dist(&self, p: Vec3) -> f32 {
        segment_dist_xy(self.a, self.b, p)
    }

    /// Whether `p`, moving up or down at `vz`, is on its level: near its
    /// floor, or in the air over it.
    pub fn level(&self, p: Vec3, vz: f32) -> bool {
        let z = self.a[2].min(self.b[2]);
        let over = p[2] - z;
        (-LEVEL_UNDER..=LEVEL_OVER).contains(&over)
            || (over > LEVEL_OVER && over <= AIR_OVER && vz.abs() > AIRBORNE_VZ)
    }

    /// Whether `p` (moving up or down at `vz`) is on it.
    pub fn holds(&self, p: Vec3, vz: f32) -> bool {
        self.dist(p) <= self.r && self.level(p, vz)
    }
}

/// The XY distance from `p` to the segment `a`-`b`.
pub fn segment_dist_xy(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 1e-9 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [a[0] + d[0] * t, a[1] + d[1] * t];
    (p[0] - q[0]).hypot(p[1] - q[1])
}

/// Another player as the practice sees them: where, how fast (from the
/// last sample), and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Other {
    /// The player's slot ([`Seen::id`]).
    pub id: u32,
    pub at: Vec3,
    /// World units a second; zero with no sample before.
    pub vel: Vec3,
    pub tick: u32,
}

impl Other {
    /// Where it will be in `t` seconds, going on as it goes.
    pub fn ahead(&self, t: f32) -> Vec3 {
        [
            self.at[0] + self.vel[0] * t,
            self.at[1] + self.vel[1] * t,
            self.at[2] + self.vel[2] * t,
        ]
    }

    pub fn speed_xy(&self) -> f32 {
        self.vel[0].hypot(self.vel[1])
    }
}

/// The other living players, tracked from tick to tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Others {
    pub now: Vec<Other>,
}

impl Others {
    /// This tick's players (the perception's `others`, every living player
    /// but the bot): each one's speed from its last sample.
    pub fn see(&mut self, seen: &[Seen], tick: u32) {
        let next = seen
            .iter()
            .map(|s| {
                let vel = self
                    .now
                    .iter()
                    .find(|o| o.id == s.id)
                    .filter(|o| (1..=STALE).contains(&tick.wrapping_sub(o.tick)))
                    .map_or([0.0; 3], |o| {
                        let dt = tick.wrapping_sub(o.tick) as f32 / TICKS;
                        [
                            (s.position[0] - o.at[0]) / dt,
                            (s.position[1] - o.at[1]) / dt,
                            (s.position[2] - o.at[2]) / dt,
                        ]
                    });
                Other {
                    id: s.id,
                    at: s.position,
                    vel,
                    tick,
                }
            })
            .collect();
        self.now = next;
    }
}

/// How much `o` uses `z`: 1 on it, [`APPROACHING`] when its course over
/// the next [`LOOK`] goes onto it, else 0.
pub fn use_of(z: &Zone, o: &Other) -> f32 {
    if z.holds(o.at, o.vel[2]) {
        return 1.0;
    }
    if o.speed_xy() < MOVING {
        return 0.0;
    }
    let steps = (LOOK / LOOK_STEP).round() as u32;
    let onto = (1..=steps).any(|k| z.holds(o.ahead(k as f32 * LOOK_STEP), o.vel[2]));
    if onto { APPROACHING } else { 0.0 }
}

/// Whether the bot standing at `me` is in the way of `o`: `o` is moving,
/// within [`PATH_NEAR`], and its course over the next [`LOOK`] passes
/// within [`PATH_CLEAR`] of the bot. The way to step: across the course,
/// to the side the bot is already on.
pub fn in_the_way(me: Vec3, o: &Other) -> Option<[f32; 2]> {
    let speed = o.speed_xy();
    if speed < MOVING || dist_xy(me, o.at) > PATH_NEAR || (me[2] - o.at[2]).abs() > LEVEL_OVER {
        return None;
    }
    let ahead = o.ahead(LOOK);
    if segment_dist_xy(o.at, ahead, me) > PATH_CLEAR {
        return None;
    }
    let along = [o.vel[0] / speed, o.vel[1] / speed];
    // Ahead of it, not behind.
    let rel = [me[0] - o.at[0], me[1] - o.at[1]];
    if rel[0] * along[0] + rel[1] * along[1] <= 0.0 {
        return None;
    }
    let left = [-along[1], along[0]];
    let side = rel[0] * left[0] + rel[1] * left[1];
    Some(if side >= 0.0 {
        left
    } else {
        [-left[0], -left[1]]
    })
}

fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
