//! A timer belongs to a map placement. Object handles change on respawn.

use super::{awareness, distance};
use crate::brain::{Me, Vec3};
use crate::mind::Skill;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Powerup {
    Overshield,
    Camo,
}

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
    /// The placed weapon starts timing on use or movement (mode zero).
    pub on_disturbance: bool,
    /// Equipment taken by walking over it (overshield or active camouflage).
    pub powerup: Option<Powerup>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Clock {
    /// No countdown yet. The host will test the respawn condition at this tick.
    Idle {
        service: u32,
        rate: u32,
        deleted: bool,
    },
    Disabled,
    Due(u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Player {
    pub id: u32,
    pub unit: u32,
    pub position: Vec3,
    pub hands: [Option<u32>; 2],
    pub overshield: bool,
    pub camo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Take {
    pub site: u8,
    pub object: u32,
    pub player: u32,
    pub unit: u32,
    pub tick: u32,
}

#[derive(Clone, Copy, Debug)]
struct Holder {
    player: u32,
    unit: u32,
    seen: u32,
    object: u32,
    powerup: Option<Powerup>,
}

#[derive(Clone, Copy, Debug)]
struct Site {
    supply: Supply,
    present: bool,
    last_seen: u32,
    last_present: u32,
    watched: bool,
    taken: Option<u32>,
    /// The bot has had sight of this empty pad since the pickup.
    timing_cycle: bool,
    learned: Option<u32>,
    clock: Option<Clock>,
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
            self.last_present = tick;
            self.supply = item;
            self.taken = None;
            self.clock = None;
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
pub(super) struct Timers {
    sites: Vec<Site>,
    holders: Vec<Holder>,
}

impl Timers {
    pub(super) fn observe(
        &mut self,
        tick: u32,
        ground: Option<&[Supply]>,
        visible: impl Fn(Vec3) -> bool,
    ) {
        for site in &mut self.sites {
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
            if !self.sites.iter().any(|s| s.supply.site == supply.site) && visible(supply.at) {
                self.sites.push(Site {
                    supply,
                    present: true,
                    last_seen: tick,
                    last_present: tick,
                    watched: true,
                    taken: None,
                    timing_cycle: false,
                    learned: None,
                    clock: None,
                });
            }
        }
    }

    /// Consult placement events only on a witnessed removal. Hands and
    /// effects identify people only inside the bot's field of view.
    pub(super) fn update_known(
        &mut self,
        p: &crate::brain::Perception,
        visible: impl Fn(Vec3) -> bool,
    ) {
        let tick = p.tick;
        let observations = &p.observations;
        let players = &observations.supply_players;
        self.holders.retain_mut(|h| {
            if h.powerup.is_none()
                && p.pickups
                    .iter()
                    .any(|gun| gun.id == h.object && visible(gun.position))
            {
                return false;
            }
            let Some(p) = players
                .iter()
                .find(|p| p.id == h.player && p.unit == h.unit)
            else {
                return false;
            };
            if visible(p.position) {
                match h.powerup {
                    Some(Powerup::Camo) if !p.camo => return false,
                    Some(Powerup::Overshield) if !p.overshield => return false,
                    Some(_) => h.seen = tick,
                    _ => {}
                }
            }
            tick.wrapping_sub(h.seen) < 300
        });
        let visible_players: Vec<_> = players.iter().filter(|p| visible(p.position)).collect();
        for p in &visible_players {
            for &object in p.hands.iter().flatten() {
                if let Some(h) = self
                    .holders
                    .iter_mut()
                    .find(|h| h.object == object && h.powerup.is_none())
                {
                    h.player = p.id;
                    h.unit = p.unit;
                    h.seen = tick;
                } else if observations.held_power.contains(&object)
                    || self
                        .sites
                        .iter()
                        .any(|s| s.supply.object == object && s.supply.powerup.is_none())
                {
                    self.holders.push(Holder {
                        player: p.id,
                        unit: p.unit,
                        object,
                        seen: tick,
                        powerup: None,
                    });
                }
            }
        }
        for site in &mut self.sites {
            if !site.watched {
                continue;
            }
            let object = site.supply.object;
            if site.present {
                self.holders.retain(|h| h.object != object);
                continue;
            }
            let Some(mut taken) = site.taken else {
                continue;
            };
            let just_taken = taken == tick;
            let event = if just_taken {
                observations.takes.iter().find(|e| {
                    e.site == site.supply.site
                        && e.object == object
                        && e.tick.wrapping_sub(site.last_present)
                            <= tick.wrapping_sub(site.last_present)
                })
            } else {
                None
            };
            if let Some(e) = event {
                taken = e.tick;
                site.taken = Some(taken);
            }
            if let Some(&clock) = observations
                .placement_clocks
                .get(usize::from(site.supply.site))
            {
                site.clock = Some(match (clock, site.supply.respawn) {
                    // The bot may turn away before the spawner first services
                    // this removal. Project only the first pending service.
                    (
                        Clock::Idle {
                            service,
                            rate,
                            deleted,
                        },
                        Some(period),
                    ) if (deleted || site.supply.on_disturbance)
                        && service.wrapping_sub(taken) <= rate =>
                    {
                        Clock::Due(service.wrapping_add(period.saturating_sub(rate)))
                    }
                    (clock, _) => clock,
                });
            }
            let Some(kind) = site.supply.powerup else {
                continue;
            };
            if !just_taken {
                continue;
            }
            let recipient = if let Some(event) = event {
                visible_players
                    .iter()
                    .copied()
                    .find(|p| p.id == event.player && p.unit == event.unit)
            } else {
                let mut candidates = visible_players
                    .iter()
                    .copied()
                    .filter(|p| distance(p.position, site.supply.spawn) < 1.0);
                candidates.next().filter(|p| {
                    candidates.next().is_none()
                        && match kind {
                            Powerup::Camo => p.camo,
                            Powerup::Overshield => p.overshield,
                        }
                })
            };
            if let Some(p) = recipient {
                self.holders.push(Holder {
                    player: p.id,
                    unit: p.unit,
                    object,
                    seen: tick,
                    powerup: Some(kind),
                });
            }
        }
    }

    pub(super) fn holder(&self, id: u32) -> bool {
        self.holders.iter().any(|h| h.player == id)
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
        self.sites
            .iter()
            .filter_map(|site| {
                let s = site.supply;
                // The ordinary arms plan handles weapons actually on the floor,
                // including whether to drop a weapon and press the action button.
                if s.powerup.is_none() && (has_power || site.present || me.arms.is_none()) {
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
                    let left = match site.clock {
                        Some(Clock::Due(at)) => i64::from(at.wrapping_sub(tick) as i32),
                        Some(Clock::Disabled | Clock::Idle { .. }) => return None,
                        None => i64::from(site.learned.or(s.respawn)?) - i64::from(elapsed),
                    };
                    // Allow a route and turn-in, then a short wait on the pad.
                    // This travel budget is a policy, independent of the guest clock.
                    let lead = (dist * 12.0 + 20.0) * sense;
                    if left < -60 || left as f32 > lead {
                        return None;
                    }
                    left.max(0) as f32 / 12.0
                };
                Some((dist + wait_cost, dest))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    }
}

#[cfg(test)]
mod tests;
