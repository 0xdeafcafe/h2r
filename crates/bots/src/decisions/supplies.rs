//! A timer belongs to a map placement. Object handles change on respawn.

use super::{awareness, distance};
use crate::brain::{Me, Vec3};
use crate::mind::Skill;

/// A power item observed close to the placement that produced it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Supply {
    pub site: u8,
    /// Salted object handle. A gun brought back to the pad is the same
    /// instance. A respawn is a new one.
    pub object: u32,
    pub spawn: Vec3,
    pub at: Vec3,
    /// Placement override or item collection default, in game ticks.
    pub respawn: Option<u32>,
    /// Equipment taken by walking over it (overshield or active camouflage).
    pub powerup: bool,
}

#[derive(Clone, Copy, Debug)]
struct Site {
    supply: Supply,
    present: bool,
    last_seen: u32,
    watched: bool,
    taken: Option<u32>,
    /// The bot has had sight of this empty pad since the pickup.
    timing_cycle: bool,
    learned: Option<u32>,
}

impl Site {
    fn observe(&mut self, tick: u32, item: Option<Supply>) {
        let continuous = self.watched && tick.wrapping_sub(self.last_seen) <= 30;
        self.timing_cycle &= continuous;
        if let Some(item) = item {
            if !self.present
                && item.object != self.supply.object
                && self.timing_cycle
                && let Some(taken) = self.taken
            {
                let period = tick.wrapping_sub(taken);
                if period > 0 {
                    self.learned = Some(period);
                }
            }
            self.supply = item;
            self.taken = None;
            self.present = true;
        } else if self.present {
            // Coming back late to an empty placement cannot date a pickup.
            self.taken = continuous.then_some(tick);
            self.timing_cycle = continuous;
            self.present = false;
        }
        self.watched = true;
        self.last_seen = tick;
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Timers(Vec<Site>);

impl Timers {
    pub(super) fn observe(
        &mut self,
        tick: u32,
        ground: Option<&[Supply]>,
        visible: impl Fn(Vec3) -> bool,
    ) {
        for site in &mut self.0 {
            if let Some(items) = ground
                && visible(site.supply.spawn)
            {
                site.observe(
                    tick,
                    items.iter().find(|s| s.site == site.supply.site).copied(),
                );
            } else {
                // A witnessed pickup still has its deadline while the bot
                // is away, but the duration of this cycle can't be learned.
                site.watched = false;
                site.timing_cycle = false;
            }
        }
        for &supply in ground.unwrap_or_default() {
            if !self.0.iter().any(|s| s.supply.site == supply.site) && visible(supply.at) {
                self.0.push(Site {
                    supply,
                    present: true,
                    last_seen: tick,
                    watched: true,
                    taken: None,
                    timing_cycle: false,
                    learned: None,
                });
            }
        }
    }

    pub(super) fn due(
        &self,
        me: Me,
        tick: u32,
        skill: Skill,
        avoid: impl Fn(Vec3) -> bool,
    ) -> Option<Vec3> {
        let sense = awareness(skill);
        let has_power = me.arms.is_some_and(|a| {
            [a.held, a.other]
                .into_iter()
                .flatten()
                .any(|g| g.role().power() && g.rating() > 0)
        });
        self.0
            .iter()
            .filter_map(|site| {
                let s = site.supply;
                // The ordinary arms plan handles weapons actually on the floor,
                // including whether to drop a weapon and press the action button.
                if !s.powerup && (has_power || site.present || me.arms.is_none()) {
                    return None;
                }
                let dest = if site.present { s.at } else { s.spawn };
                let dist = distance(me.position, dest);
                if dist > 9.0 + 25.0 * sense || avoid(dest) {
                    return None;
                }
                let wait_cost = if site.present {
                    if tick.wrapping_sub(site.last_seen) > 90 {
                        return None;
                    }
                    0.0
                } else {
                    let elapsed = tick.wrapping_sub(site.taken?);
                    // A witnessed period accounts for the variant's actual
                    // respawn behavior. The placement value is an estimate.
                    let interval = site.learned.or(s.respawn)?;
                    let left = i64::from(interval) - i64::from(elapsed);
                    let lead = (dist * 6.0 + 15.0) * sense;
                    if left < -60 || left as f32 > lead {
                        return None;
                    }
                    left.max(0) as f32 / 6.0
                };
                Some((dist + wait_cost, dest))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    }
}

#[cfg(test)]
mod tests;
