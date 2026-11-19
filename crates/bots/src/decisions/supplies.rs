//! A timer belongs to a map placement. Object handles change on respawn.

use super::{awareness, distance};
use crate::brain::{Me, Vec3};
use crate::mind::Skill;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Powerup {
    Overshield,
    Camo,
    /// Cached collection can spawn either effect. Sight resolves the type.
    Mixed,
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
    pub role: crate::arms::Role,
    /// Respawn disabled in the public match settings or placement.
    pub disabled: bool,
    /// Equipment taken by walking over it (overshield or active camouflage).
    pub powerup: Option<Powerup>,
}

/// Guest timing oracle for the placement-service trace.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Clock {
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Take {
    pub at: Vec3,
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
    unexplored: bool,
    present: bool,
    last_seen: u32,
    last_present: u32,
    expected_return: Option<u32>,
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
        if self.unexplored && self.indirect.is_none() {
            return 0.58;
        }
        let age = tick.wrapping_sub(self.last_seen);
        if self.watched && age == 0 {
            return if self.present { 1.0 } else { 0.0 };
        }
        let nominal = self.learned.or(self.supply.respawn).unwrap_or(1800);
        let (present, since, indirect) = self
            .indirect
            .filter(|&(_, t)| {
                self.unexplored
                    || tick.wrapping_sub(t) < age
                    || !self.watched && t == self.last_seen
            })
            .map_or((self.present, self.last_seen, false), |(up, t)| {
                (up, t, true)
            });
        let observed_interval = self
            .expected_return
            .zip(self.taken)
            .filter(|_| !indirect)
            .map(|(expected, taken)| expected.wrapping_sub(taken));
        let period = observed_interval.unwrap_or(nominal).max(1) as f32;
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
        self.unexplored = false;
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
            self.expected_return = None;
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
pub(super) struct Timers {
    sites: Vec<Site>,
    holders: Vec<Holder>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trip {
    Search,
    Contest,
}

impl Timers {
    pub(super) fn forget_holder(&mut self, victim: u32) {
        self.holders.retain(|h| h.player != victim);
    }

    pub(super) fn familiar(&mut self, pad: crate::places::PowerPad, tick: u32) {
        if self.sites.iter().any(|s| s.supply.site == pad.site) {
            return;
        }
        self.sites.push(Site {
            supply: Supply {
                site: pad.site,
                spawn: pad.at,
                at: pad.at,
                respawn: pad.period,
                powerup: pad.powerup,
                role: pad.role,
                on_disturbance: false,
                disabled: false,
                object: u32::MAX,
            },
            present: false,
            watched: false,
            last_seen: tick,
            last_present: tick,
            expected_return: None,
            unexplored: true,
            timing_cycle: false,
            learned: None,
            taken: None,
            indirect: None,
            broadcast: None,
            model: pad.model,
        });
    }

    pub(super) fn observe(
        &mut self,
        tick: u32,
        ground: Option<&[Supply]>,
        visible: impl Fn(Vec3) -> bool,
    ) {
        for site in &mut self.sites {
            let item = ground.and_then(|items| {
                items
                    .iter()
                    .find(|s| s.site == site.supply.site && visible(s.at))
                    .copied()
            });
            if item.is_some()
                || (ground.is_some() && visible(site.supply.spawn) && visible(site.supply.at))
            {
                site.observe(tick, item);
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
                    unexplored: false,
                    present: true,
                    last_seen: tick,
                    last_present: tick,
                    expected_return: None,
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
        for site in &mut self.sites {
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
            let mut matching = self.sites.iter_mut().filter(|s| s.model == Some(model));
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
        for s in &mut self.sites {
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
        if let Some(s) = self.sites.iter_mut().find(|s| s.supply.site == supply.site) {
            if (s.unexplored || now.wrapping_sub(tick) < now.wrapping_sub(s.last_seen))
                && s.indirect
                    .is_none_or(|(_, t)| now.wrapping_sub(tick) < now.wrapping_sub(t))
            {
                s.indirect = Some((present, tick));
            }
        } else {
            self.sites.push(Site {
                supply,
                unexplored: false,
                present,
                last_seen: tick,
                last_present: tick,
                expected_return: None,
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
            if let Some(p) = players
                .iter()
                .find(|p| p.id == h.player && visible(p.position))
            {
                if h.unit != p.unit {
                    return false;
                }
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
                if let Some(site) = self
                    .sites
                    .iter_mut()
                    .find(|s| !s.watched && s.supply.object == object && s.supply.powerup.is_none())
                {
                    site.indirect = Some((false, tick));
                }
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
            // A witnessed removal can date the first eligible service. The
            // schedule is public map knowledge; the live countdown is private.
            if just_taken
                && (site.supply.on_disturbance || site.supply.powerup.is_some())
                && !site.supply.disabled
                && let Some(rate) = observations.service_rate.filter(|&r| r > 0)
                && let Some(period) = site.learned.or(site.supply.respawn)
            {
                let rate = u32::from(rate);
                let phase = (u32::from(site.supply.site) % rate + rate - taken % rate) % rate;
                let service = taken.wrapping_add(if phase == 0 { rate } else { phase });
                site.expected_return = Some(if let Some(learned) = site.learned {
                    taken.wrapping_add(learned)
                } else {
                    service.wrapping_add(period.saturating_sub(rate))
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
                            Powerup::Mixed => false,
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
        personality: super::Personality,
        trip: Trip,
        position_value: impl Fn(Vec3) -> Option<f32>,
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
                let on_pad = site.present && site.watched && site.last_seen == tick;
                if s.powerup.is_none() && (has_power || on_pad || me.arms.is_none()) {
                    return None;
                }
                let dest = if on_pad { s.at } else { s.spawn };
                let dist = distance(me.position, dest);
                if dist > 9.0 + 25.0 * sense {
                    return None;
                }
                let patience = personality.wait_weight();
                let wait = (8.0 + 24.0 * patience) * sense;
                let interest = if s.powerup.is_none() {
                    personality.weapon_interest(s.role)
                } else {
                    0.0
                };
                let strength = position_value(dest)? * sense;
                if s.disabled && !site.present {
                    return None;
                }
                if let Some(return_tick) = site.expected_return.filter(|_| site.indirect.is_none())
                {
                    let left = return_tick.wrapping_sub(tick) as i32;
                    // Lead the expected service, then hold for a short grace
                    // period while contesting. Only sight confirms the return.
                    if left > 0 && left as f32 > dist * 12.0 * sense + wait {
                        return None;
                    }
                    if left >= -(wait.round() as i32) {
                        return Some((
                            dist + left.max(0) as f32 / 12.0 - 8.0 * strength - 5.0 * interest,
                            dest,
                        ));
                    }
                }
                if trip == Trip::Contest
                    && !(site.present
                        && !site.unexplored
                        && site.indirect.is_none()
                        && tick.wrapping_sub(site.last_seen) <= 90)
                {
                    return None;
                }
                let chance = site.probability(tick, dist * 6.0, sense);
                (chance + 0.14 * strength + 0.06 * interest >= 0.56).then_some((
                    dist + 18.0 * (1.0 - chance) - 8.0 * strength - 5.0 * interest,
                    dest,
                ))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    }
}

#[cfg(test)]
mod tests;
