//! The rocket launcher's lock-on (docs/game/bots.md, section 3.8,
//! "Lock-on"). Halo 2's rocket launcher locks on to a vehicle the
//! reticle stays on for a moment; a locked rocket follows it. So against
//! a vehicle the bot aims at the vehicle's middle, keeps it there until
//! the lock, then fires. Against infantry nothing changes: the rocket is
//! fired unlocked, at the feet for a skill that aims there
//! (`tactics::Style::feet`).
//!
//! The lock's own time is not traced (section 3.8, "Open"): the bot holds
//! for [`LOCK_TICKS`] plus its skill's margin. Pure; time is the game's
//! tick.

use crate::arms::{Gun, Role};
use crate::brain::Vec3;
use crate::mind::Skill;

/// Ticks the reticle is held on a vehicle before the bot counts it
/// locked (1 s).
pub const LOCK_TICKS: u32 = 30;
/// The aim point over a vehicle's origin.
pub const MIDDLE: f32 = 0.3;
/// A vehicle's radius for "in the reticle" (world units).
pub const SIZE: f32 = 0.8;

/// Whether a gun locks on to vehicles: a blast weapon with a zoom, the
/// rocket launcher (the fuel rod and the brute shot have no zoom,
/// section 3.6's zoom levels).
pub fn locks_on(g: &Gun) -> bool {
    g.role() == Role::Blast && g.profile.zoom >= 1
}

/// How a skill uses the lock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LockSkill {
    /// Waits for the lock against a vehicle; without, it fires as at
    /// infantry.
    pub waits: bool,
    /// Ticks it holds past [`LOCK_TICKS`] to be sure.
    pub margin: u32,
    /// Ticks off the vehicle it forgives without starting again (a shaky
    /// hand).
    pub grace: u32,
}

impl LockSkill {
    pub fn of(skill: Skill) -> LockSkill {
        match skill {
            Skill::Easy => LockSkill {
                waits: false,
                margin: 0,
                grace: 0,
            },
            Skill::Normal => LockSkill {
                waits: true,
                margin: 12,
                grace: 3,
            },
            Skill::Heroic => LockSkill {
                waits: true,
                margin: 6,
                grace: 6,
            },
            Skill::Legendary => LockSkill {
                waits: true,
                margin: 2,
                grace: 8,
            },
        }
    }
    /// Ticks on target until locked.
    pub fn hold(&self) -> u32 {
        LOCK_TICKS + self.margin
    }
}

/// Whether a vehicle at `at` is in the reticle of an eye at `eye` aiming
/// along `aim`: within the vehicle's apparent size plus `cone` radians.
pub fn on_target(eye: Vec3, aim: Vec3, at: Vec3, cone: f32) -> bool {
    let to = [at[0] - eye[0], at[1] - eye[1], at[2] - eye[2]];
    let d = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
    let a = (aim[0] * aim[0] + aim[1] * aim[1] + aim[2] * aim[2]).sqrt();
    if d < 1e-4 || a < 1e-6 {
        return false;
    }
    let cos = (to[0] * aim[0] + to[1] * aim[1] + to[2] * aim[2]) / (d * a);
    cos.clamp(-1.0, 1.0).acos() < (SIZE / d).atan() + cone
}

/// Where the lock stands this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locking {
    /// Holding on it: do not fire yet.
    Holding,
    /// Locked: fire.
    Locked,
}

/// The lock's state, kept by the mind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lock {
    /// The vehicle held on, the tick the hold began and the last tick it
    /// was in the reticle.
    held: Option<(u32, u32, u32)>,
}

impl Lock {
    /// One tick on vehicle `id`, `on` whether it is in the reticle.
    pub fn step(&mut self, id: u32, on: bool, tick: u32, k: &LockSkill) -> Locking {
        let held = match self.held {
            Some((v, since, last)) if v == id && tick.wrapping_sub(last) <= k.grace + 1 => {
                Some((v, since, if on { tick } else { last }))
            }
            _ if on => Some((id, tick, tick)),
            _ => None,
        };
        self.held = held;
        match held {
            Some((_, since, last)) if last == tick && tick.wrapping_sub(since) >= k.hold() => {
                Locking::Locked
            }
            _ => Locking::Holding,
        }
    }

    /// Lets go (no vehicle target, another weapon, a new life).
    pub fn reset(&mut self) {
        self.held = None;
    }

    /// The vehicle it is holding on, if any.
    pub fn on(&self) -> Option<u32> {
        self.held.map(|(v, _, _)| v)
    }
}

#[cfg(test)]
mod tests;
