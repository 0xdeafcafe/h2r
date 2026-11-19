//! Getting unstuck (docs/game/bots.md, section 3.2): progress toward where
//! the bot is trying to go, measured from its position, and what to do
//! when there is none. Pure: positions and ticks in, an [`Unstick`] out.
//!
//! While the bot wants to move, a window of [`WINDOW`] ticks without
//! moving [`PROGRESS`] escalates, one step a window:
//!
//! 1. [`Level::Sidestep`]: strafe to one side and back off;
//! 2. [`Level::Jump`];
//! 3. [`Level::CrouchJump`]: jump, then crouch in the air;
//! 4. [`Level::Repath`]: give up on the way: the waypoint (or the goal) is
//!    blacklisted for [`BLACKLIST_TICKS`] and the caller plans anew.
//!
//! Moving [`PROGRESS`] the way it wants to go, at any level, is recovery:
//! back to [`Level::Moving`]; stuck again within [`RELAPSE_TICKS`] carries
//! on from the step that freed it. Two slower checks catch what the window misses:
//! staying within [`PEN_RADIUS`] for [`PEN_TICKS`] (oscillating between two
//! waypoints, circling, sliding along a wall), and the goal getting no
//! closer by [`GOAL_GAIN`] in [`GOAL_TICKS`]; either goes straight to
//! [`Level::Repath`], blacklisting the goal.

use crate::brain::Vec3;

/// Ticks without progress per escalation step (two thirds of a second).
pub const WINDOW: u32 = 20;
/// Moving this far (world units, XY) is progress. A bot walks about 1.5 in
/// a window.
pub const PROGRESS: f32 = 0.25;
/// Penned in: never this far from where it was...
pub const PEN_RADIUS: f32 = 1.0;
/// ...for this long, while trying to move.
pub const PEN_TICKS: u32 = 120;
/// The goal must get this much closer...
pub const GOAL_GAIN: f32 = 0.5;
/// ...every this long.
pub const GOAL_TICKS: u32 = 300;
/// A blacklisted point is avoided this long (10 s)...
pub const BLACKLIST_TICKS: u32 = 300;
/// ...within this.
pub const BLACKLIST_RADIUS: f32 = 0.75;
/// The most points kept; the oldest goes first.
pub const BLACKLIST_MAX: usize = 16;
/// Stuck again this soon after getting free picks up the escalation
/// where it left off.
pub const RELAPSE_TICKS: u32 = 90;
/// Jump is pressed this many ticks at the start of its step.
pub const JUMP_TICKS: u32 = 3;

/// How far along the escalation the bot is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Level {
    #[default]
    Moving,
    Sidestep,
    Jump,
    CrouchJump,
    Repath,
}

impl Level {
    fn next(self) -> Level {
        match self {
            Level::Moving => Level::Sidestep,
            Level::Sidestep => Level::Jump,
            Level::Jump => Level::CrouchJump,
            Level::CrouchJump | Level::Repath => Level::Repath,
        }
    }
}

/// What to do this tick to get unstuck.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Unstick {
    /// Walk this way instead (a world XY direction), if set. The bot keeps
    /// facing the way it wanted to go.
    pub dir: Option<[f32; 2]>,
    pub jump: bool,
    pub crouch: bool,
    /// Drop the path (and, for the goal, the goal) and plan again.
    pub repath: bool,
}

/// The stuck state machine. See the module docs.
#[derive(Clone, Debug, Default)]
pub struct Stuck {
    pub level: Level,
    /// The tick the level was entered.
    since: u32,
    /// Where the bot was when the current window started.
    anchor: Option<(Vec3, u32)>,
    /// The pen: a point and when the bot was last outside [`PEN_RADIUS`]
    /// of it.
    pen: Option<(Vec3, u32)>,
    /// The goal, its best distance and when it was reached.
    goal: Option<(Vec3, f32, u32)>,
    /// The side to step to next: +1 left, -1 right.
    side: f32,
    /// Points to keep away from, and the tick each expires.
    blacklist: Vec<(Vec3, u32)>,
    /// The level it last got free at, and when: stuck again within
    /// [`RELAPSE_TICKS`] carries on from there.
    relapse: Option<(Level, u32)>,
    /// How many times it has had to repath (for logs and tests).
    pub repaths: u32,
}

fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Whether `tick` is at or past `until`, across the counter's wrap.
fn reached(tick: u32, until: u32) -> bool {
    tick.wrapping_sub(until) < u32::MAX / 2
}

impl Stuck {
    pub fn new() -> Self {
        Stuck {
            side: 1.0,
            ..Stuck::default()
        }
    }

    /// Forgets the progress tracking (a new life, a new goal), not the
    /// blacklist.
    pub fn reset(&mut self) {
        self.level = Level::Moving;
        self.anchor = None;
        self.pen = None;
        self.goal = None;
    }

    /// Whether `p` is near a blacklisted point that has not expired.
    pub fn blocked(&self, p: Vec3, tick: u32) -> bool {
        self.blacklist
            .iter()
            .any(|&(b, until)| !reached(tick, until) && dist_xy(b, p) < BLACKLIST_RADIUS)
    }

    /// Whether the segment `a`-`b` passes near a blacklisted point.
    pub fn blocks_segment(&self, a: Vec3, b: Vec3, tick: u32) -> bool {
        self.blacklist
            .iter()
            .any(|&(p, until)| !reached(tick, until) && segment_dist_xy(a, b, p) < BLACKLIST_RADIUS)
    }

    /// Avoids `p` for [`BLACKLIST_TICKS`] from `tick`.
    pub fn blacklist(&mut self, p: Vec3, tick: u32) {
        self.blacklist.retain(|&(_, until)| !reached(tick, until));
        if self.blacklist.len() >= BLACKLIST_MAX {
            self.blacklist.remove(0);
        }
        self.blacklist.push((p, tick.wrapping_add(BLACKLIST_TICKS)));
    }

    /// The points still blacklisted at `tick`.
    pub fn blacklisted(&self, tick: u32) -> impl Iterator<Item = Vec3> + '_ {
        self.blacklist
            .iter()
            .filter(move |&&(_, until)| !reached(tick, until))
            .map(|&(p, _)| p)
    }

    /// One tick. `want` is the XY direction the bot is trying to walk
    /// (`None` when it means to stand still: nothing is measured then);
    /// `waypoint` the point it is walking to and `goal` where it is going in
    /// the end, for the blacklist.
    pub fn step(
        &mut self,
        tick: u32,
        at: Vec3,
        want: Option<[f32; 2]>,
        waypoint: Option<Vec3>,
        goal: Option<Vec3>,
    ) -> Unstick {
        let Some(want) = want.filter(|w| w[0].abs() + w[1].abs() > 1e-3) else {
            self.reset();
            return Unstick::default();
        };

        // A new goal starts the goal check over.
        match (goal, self.goal) {
            (Some(g), Some((old, _, _))) if dist_xy(g, old) < 1.0 => {}
            (Some(g), _) => self.goal = Some((g, dist_xy(g, at), tick)),
            (None, _) => self.goal = None,
        }

        // Progress over the window, the way it wants to go: recovery.
        // (A sidestep's own movement is not progress.)
        let (anchor, anchor_tick) = *self.anchor.get_or_insert((at, tick));
        let wl = (want[0] * want[0] + want[1] * want[1]).sqrt();
        let forward = ((at[0] - anchor[0]) * want[0] + (at[1] - anchor[1]) * want[1]) / wl;
        if forward >= PROGRESS {
            self.anchor = Some((at, tick));
            if self.level != Level::Moving {
                self.relapse = Some((self.level, tick));
            }
            self.level = Level::Moving;
        } else if tick.wrapping_sub(anchor_tick) >= WINDOW {
            self.anchor = Some((at, tick));
            self.level = match (self.level, self.relapse.take()) {
                // Stuck again soon after getting free: carry on from
                // where it got to, not from the start.
                (Level::Moving, Some((last, when))) if tick.wrapping_sub(when) < RELAPSE_TICKS => {
                    last.next()
                }
                (l, _) => l.next(),
            };
            self.since = tick;
            if self.level == Level::Sidestep {
                self.side = -self.side;
            }
        }

        // The pen.
        let (pen, pen_tick) = *self.pen.get_or_insert((at, tick));
        let mut give_up_goal = false;
        if dist_xy(at, pen) > PEN_RADIUS {
            self.pen = Some((at, tick));
        } else if tick.wrapping_sub(pen_tick) >= PEN_TICKS {
            give_up_goal = true;
        }
        // The goal.
        if let Some((g, best, best_tick)) = self.goal {
            let d = dist_xy(g, at);
            if d < best - GOAL_GAIN {
                self.goal = Some((g, d, tick));
            } else if tick.wrapping_sub(best_tick) >= GOAL_TICKS {
                give_up_goal = true;
            }
        }

        if give_up_goal || self.level == Level::Repath {
            let bad = if give_up_goal {
                goal.or(waypoint)
            } else {
                waypoint.or(goal)
            };
            let bad = bad.unwrap_or([at[0] + want[0], at[1] + want[1], at[2]]);
            self.blacklist(bad, tick);
            if give_up_goal && let Some(w) = waypoint {
                self.blacklist(w, tick);
            }
            self.repaths += 1;
            self.reset();
            self.relapse = None;
            return Unstick {
                repath: true,
                ..Unstick::default()
            };
        }

        let in_step = tick.wrapping_sub(self.since);
        let s = self.side;
        match self.level {
            Level::Moving | Level::Repath => Unstick::default(),
            Level::Sidestep => Unstick {
                // Left of `want` is (-y, x); back off a little too.
                dir: Some([
                    -want[1] * s * 0.9 - want[0] * 0.45,
                    want[0] * s * 0.9 - want[1] * 0.45,
                ]),
                ..Unstick::default()
            },
            Level::Jump => Unstick {
                jump: in_step < JUMP_TICKS,
                ..Unstick::default()
            },
            Level::CrouchJump => Unstick {
                jump: in_step < JUMP_TICKS,
                crouch: in_step >= 2,
                ..Unstick::default()
            },
        }
    }
}

/// The XY distance from `p` to the segment `a`-`b`.
pub fn segment_dist_xy(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if l2 < 1e-12 {
        0.0
    } else {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0)
    };
    let c = [a[0] + ab[0] * t, a[1] + ab[1] * t, 0.0];
    dist_xy(c, p)
}

/// Bodies in the way: someone within [`BODY_AHEAD`] ahead of `at` along
/// `dir` and within [`BODY_WIDTH`] of the line. Steers around the nearest
/// such: `dir` turned away from them. Pure.
pub fn around_bodies(at: Vec3, dir: [f32; 2], bodies: &[Vec3]) -> [f32; 2] {
    let l = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
    if l < 1e-6 {
        return dir;
    }
    let f = [dir[0] / l, dir[1] / l];
    let mut nearest: Option<(f32, f32)> = None;
    for b in bodies {
        let d = [b[0] - at[0], b[1] - at[1]];
        let ahead = d[0] * f[0] + d[1] * f[1];
        let side = -d[0] * f[1] + d[1] * f[0];
        if (b[2] - at[2]).abs() > 0.6
            || ahead <= 0.0
            || ahead > BODY_AHEAD
            || side.abs() > BODY_WIDTH
        {
            continue;
        }
        if nearest.is_none_or(|(a, _)| ahead < a) {
            nearest = Some((ahead, side));
        }
    }
    let Some((_, side)) = nearest else {
        return dir;
    };
    // They are to the left (side > 0): go right, and the other way; dead
    // ahead, right.
    let away = if side > 0.0 { -1.0 } else { 1.0 };
    let left = [-f[1], f[0]];
    let out = [f[0] * 0.5 + left[0] * away, f[1] * 0.5 + left[1] * away];
    let n = (out[0] * out[0] + out[1] * out[1]).sqrt();
    [out[0] / n * l, out[1] / n * l]
}

/// How far ahead a body is in the way (world units)...
pub const BODY_AHEAD: f32 = 0.8;
/// ...and how far off the line.
pub const BODY_WIDTH: f32 = 0.35;

#[cfg(test)]
mod tests;
