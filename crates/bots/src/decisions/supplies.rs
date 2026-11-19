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
    model: Option<u32>,
    indirect: Option<(bool, u32)>,
    broadcast: Option<(bool, u32)>,
}

impl Site {
    /// Probability at arrival. A view now is the only certain observation.
    fn probability(&self, tick: u32, arrival: f32, sense: f32) -> f32 {
        let age = tick.wrapping_sub(self.last_seen);
        if self.watched && age == 0 {
            return if self.present { 1.0 } else { 0.0 };
        }
        let period = self.learned.or(self.supply.respawn).unwrap_or(1800) as f32;
        let (present, since, indirect) = self
            .indirect
            .filter(|&(_, t)| tick.wrapping_sub(t) < age || !self.watched && t == self.last_seen)
            .map_or((self.present, self.last_seen, false), |(up, t)| {
                (up, t, true)
            });
        let elapsed = tick.wrapping_sub(since) as f32;
        if present {
            let confidence = if indirect { 0.78 } else { 0.94 };
            return 0.52 + (confidence - 0.52) / (1.0 + elapsed / period);
        }
        let cycle = self
            .taken
            .filter(|_| !indirect)
            .map_or(elapsed, |t| tick.wrapping_sub(t) as f32);
        // Timing a pickup gives a head start, but a fresh empty-pad
        // sighting still overrides an overdue estimate.
        let estimate = (cycle + arrival * sense) / (period * (1.25 - 0.25 * sense));
        let rise = estimate.clamp(0.0, 1.0).powi(2);
        let freshness = ((elapsed + arrival * sense) / (0.15 * period)).min(1.0);
        (if indirect { 0.19 } else { 0.04 }) + 0.74 * rise * freshness
    }

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
        self.indirect = None;
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
                    model: None,
                    indirect: None,
                    broadcast: None,
                });
            }
        }
    }

    pub(super) fn carriers(&mut self, p: &crate::brain::Perception, me: Me) {
        for site in &mut self.0 {
            if site.watched && site.present {
                if let Some(pk) = p.pickups.iter().find(|pk| pk.id == site.supply.object) {
                    site.model = Some(pk.gun.def);
                }
            }
        }
        let models = me
            .arms
            .into_iter()
            .flat_map(|a| [a.held, a.other, a.left])
            .flatten()
            .map(|g| g.def)
            .chain(p.observations.contacts.iter().filter_map(|c| c.weapon));
        for model in models {
            let mut matching = self.0.iter_mut().filter(|s| s.model == Some(model));
            if let Some(site) = matching.next()
                && matching.next().is_none()
                && !site.watched
            {
                site.indirect = Some((false, p.tick));
            }
        }
    }

    pub(super) fn reports(&mut self, tick: u32) -> Vec<(Supply, bool)> {
        let mut out = Vec::new();
        for s in &mut self.0 {
            if s.watched
                && s.last_seen == tick
                && s.broadcast
                    .is_none_or(|(up, t)| up != s.present || tick.wrapping_sub(t) >= 60)
            {
                out.push((s.supply, s.present));
                s.broadcast = Some((s.present, tick));
            }
        }
        out
    }

    pub(super) fn hear(&mut self, supply: Supply, present: bool, tick: u32, now: u32) {
        if let Some(s) = self.0.iter_mut().find(|s| s.supply.site == supply.site) {
            if now.wrapping_sub(tick) < now.wrapping_sub(s.last_seen)
                && s.indirect
                    .is_none_or(|(_, t)| now.wrapping_sub(tick) < now.wrapping_sub(t))
            {
                s.indirect = Some((present, tick));
            }
        } else {
            self.0.push(Site {
                supply,
                present,
                last_seen: tick,
                watched: false,
                taken: None,
                timing_cycle: false,
                learned: None,
                model: None,
                indirect: Some((present, tick)),
                broadcast: None,
            });
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
                let on_pad = site.present && site.watched && site.last_seen == tick;
                if !s.powerup && (has_power || on_pad || me.arms.is_none()) {
                    return None;
                }
                let dest = if on_pad { s.at } else { s.spawn };
                let dist = distance(me.position, dest);
                if dist > 9.0 + 25.0 * sense || avoid(dest) {
                    return None;
                }
                let chance = site.probability(tick, dist * 6.0, sense);
                (chance >= 0.56).then_some((dist + 18.0 * (1.0 - chance), dest))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    }
}

#[cfg(test)]
mod tests;
