//! Multiplayer decisions. The host supplies observations, teammates exchange
//! sightings, and each mind keeps its own memory across lives in the match.

mod map;
pub(crate) mod radio;
pub mod supplies;

use radio::Call;

use crate::brain::{Me, Perception, Seen, Vec3};
use crate::cache::MapData;
use crate::emote::Kill;
use crate::mind::{EYE, Skill};
use crate::objective::Goal;
use crate::tactics;

pub(crate) fn distance(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The decisions' sense of the fight for a skill, with the thinking
/// brain's tuning over it when it has one ([`crate::tuning`]).
pub(crate) fn awareness(skill: Skill) -> f32 {
    let base = match skill {
        Skill::Easy => 0.3,
        Skill::Normal => 0.65,
        Skill::Heroic => 0.85,
        Skill::Legendary => 1.0,
    };
    match crate::tuning::current() {
        Some(t) => t.awareness(base),
        None => base,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Personality {
    pub aggression: f32,
    pub caution: f32,
    pub teamwork: f32,
    pub objective_focus: f32,
}

impl Personality {
    pub fn seeded(mut seed: u64) -> Self {
        // SplitMix64, independent of aim and movement's random streams.
        let mut draw = || {
            seed = seed.wrapping_add(0x9e3779b97f4a7c15);
            let mut n = seed;
            n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
            ((n ^ (n >> 31)) >> 40) as f32 / 16777216.0
        };
        Self {
            aggression: draw(),
            caution: draw(),
            teamwork: draw(),
            objective_focus: draw(),
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum Posture {
    #[default]
    Normal,
    Withdraw,
    Press,
}

#[derive(Clone, Debug)]
pub struct Decisions {
    pub personality: Personality,
    pub posture: Posture,
    pub(crate) map: map::MapMemory,
    pub(crate) inbox: Vec<Call>,
    pub(crate) report: Option<Call>,
    // The latest living roster survives the frame in which a teammate dies.
    mates: Vec<u32>,
    team: Option<i16>,
    trade: Option<(u32, Vec3, u32)>,
    last_shield: Option<f32>,
    hit: Option<u32>,
    withdrew: Option<u32>,
}

impl Decisions {
    pub fn new(seed: u64) -> Self {
        Self {
            personality: Personality::seeded(seed),
            posture: Posture::Normal,
            map: Default::default(),
            inbox: Vec::new(),
            report: None,
            mates: Vec::new(),
            team: None,
            trade: None,
            last_shield: None,
            hit: None,
            withdrew: None,
        }
    }

    /// Called before consuming this frame's roster, including while dead.
    /// A kill feed identifies the killer but never gives a firing position.
    pub(crate) fn hear(&mut self, p: &Perception, kills: &[Kill]) -> bool {
        let Some((id, team)) = p.observations.player else {
            return false;
        };
        let mut changed = false;
        for k in kills {
            if self
                .trade
                .is_some_and(|(killer, _, since)| k.victim == killer && k.tick >= since)
            {
                self.trade = None;
            }
            let Some(at) = k.at else { continue };
            if p.tick.wrapping_sub(k.tick) >= map::HOT_TICKS {
                continue;
            }
            let mate = p.teams && self.team == Some(team) && self.mates.contains(&k.victim);
            if id == k.victim || mate {
                self.map.death(at, k.tick);
                changed = true;
                if mate
                    && !k.betrayal
                    && let Some(killer) = k.attacker.filter(|&a| a != k.victim)
                {
                    self.trade = Some((killer, at, k.tick));
                }
            }
        }
        changed
    }

    pub(crate) fn observe(&mut self, p: &Perception, me: Me, map: Option<&MapData>) {
        self.team = Some(me.team);
        self.mates.clear();
        self.mates.extend(
            p.others
                .iter()
                .filter(|o| p.teams && o.team == me.team)
                .map(|o| o.id),
        );
        if self.last_shield.is_some_and(|old| old - me.shield > 0.02) {
            self.hit = Some(p.tick);
        }
        self.last_shield = Some(me.shield);
        self.map.observe(p, me, map);
    }

    pub(crate) fn hear_supplies(&mut self, calls: &[radio::SupplyCall], tick: u32) {
        for c in calls {
            self.map.timers.hear(c.item, c.present, c.tick, tick);
        }
    }

    pub(crate) fn supply_reports(&mut self, p: &Perception) -> Vec<radio::SupplyCall> {
        let Some((sender, team)) = p.observations.player.filter(|_| p.teams && p.me.is_some())
        else {
            return vec![];
        };
        self.map
            .timers
            .reports(p.tick)
            .into_iter()
            .map(|(item, present)| radio::SupplyCall {
                sender,
                team,
                item,
                present,
                tick: p.tick,
            })
            .collect()
    }

    pub(crate) fn disengaging(&self, tick: u32) -> bool {
        self.withdrew.is_some_and(|t| tick.wrapping_sub(t) < 90)
    }

    pub(crate) fn died(&mut self, tick: u32) {
        self.trade = None;
        self.report = None;
        self.hit = None;
        self.last_shield = None;
        self.withdrew = None;
        self.posture = Posture::Normal;
        self.map.unobserved(tick);
    }

    pub(crate) fn decide(&mut self, f: &Fight<'_>, current: Option<u32>) -> Intent {
        let eye = [f.me.position[0], f.me.position[1], f.me.position[2] + EYE];
        let chosen =
            tactics::choose_weighted(f.seen, eye, f.me.aim, current, |o| f.target_weight(self, o));
        self.posture = chosen.map_or(Posture::Normal, |o| f.posture(self, o));
        if self.posture == Posture::Withdraw {
            self.withdrew = Some(f.p.tick);
        }
        self.report = f
            .p
            .observations
            .player
            .filter(|_| f.p.teams)
            .zip(chosen)
            .map(|((sender, team), o)| Call {
                sender,
                team,
                target: o.id,
                at: o.position,
                tick: f.p.tick,
                help: f.me.shield < 0.4 || self.hit.is_some_and(|t| f.p.tick.wrapping_sub(t) < 30),
            });
        Intent {
            chosen,
            anchor: chosen.and_then(|o| f.leash(o, self.personality)),
            support: if chosen.is_none() && f.goal.is_none() {
                f.support(self)
            } else {
                None
            },
        }
    }
}

pub(crate) struct Intent {
    pub chosen: Option<Seen>,
    pub anchor: Option<(Vec3, f32)>,
    pub support: Option<Vec3>,
}

fn clear(map: Option<&MapData>, mut a: Vec3, mut b: Vec3) -> bool {
    a[2] += EYE;
    b[2] += EYE;
    map.is_none_or(|m| m.walls.clear(a, b))
}

fn facing(from: Vec3, aim: Vec3, to: Vec3, cosine: f32) -> bool {
    let d = distance(from, to);
    d > 0.01 && (0..3).map(|i| aim[i] * (to[i] - from[i])).sum::<f32>() > cosine * d
}

/// Only enemies in the mind's own view enter target selection.
pub(crate) struct Fight<'a> {
    pub p: &'a Perception,
    pub me: Me,
    pub seen: &'a [Seen],
    pub skill: Skill,
    pub goal: Option<Goal>,
    pub map: Option<&'a MapData>,
}

impl Fight<'_> {
    fn contact(&self, id: u32) -> Option<&crate::brain::Contact> {
        self.p.observations.contacts.iter().find(|c| c.id == id)
    }

    /// An opponent committed to another fight. In FFA both participants
    /// must be in view. A teammate may instead call for help directly.
    fn engaged(&self, o: Seen) -> bool {
        let Some(read) = self.contact(o.id) else {
            return false;
        };
        let mut opponents = self
            .p
            .others
            .iter()
            .filter(|other| other.id != o.id && (!self.p.teams || other.team != o.team));
        opponents.any(|other| {
            if !(self.p.teams && other.team == self.me.team)
                && !self.seen.iter().any(|v| v.id == other.id)
            {
                return false;
            }
            let Some(r) = self.contact(other.id) else {
                return false;
            };
            distance(other.position, o.position) < 18.0
                && facing(o.position, read.aim, other.position, 0.93)
                && (r.shield.is_some_and(|s| s < 0.55)
                    || facing(other.position, r.aim, o.position, 0.93))
                && clear(self.map, o.position, other.position)
        })
    }

    fn opening(&self, o: Seen, traits: Personality) -> bool {
        self.contact(o.id).is_some_and(|r| {
            r.empty_magazine
                || r.shield
                    .is_some_and(|s| s < (0.18 + 0.30 * traits.aggression) * awareness(self.skill))
        }) || self.engaged(o)
    }

    fn target_weight(&self, d: &Decisions, o: Seen) -> f32 {
        let traits = d.personality;
        if matches!(self.goal, Some(Goal::Hunt(id, _)) if id == o.id) {
            return 0.15;
        }
        let team = (0.65 + traits.teamwork) * awareness(self.skill);
        let mut claimed = 0;
        for c in d.inbox.iter().filter(|c| c.target == o.id) {
            if c.help && distance(self.me.position, c.at) < 24.0 {
                return 1.0 / (1.0 + 4.0 * team);
            }
            // The lower slot holds a shared target. Higher slots peel off
            // when there is a comparable alternative, preventing oscillation.
            if self.p.tick.wrapping_sub(c.tick) <= 15
                && self
                    .p
                    .observations
                    .player
                    .is_some_and(|(id, _)| id > c.sender)
            {
                claimed += 1;
            }
        }
        if self.p.teams
            && d.trade.is_some_and(|(killer, at, t)| {
                killer == o.id
                    && self.p.tick.wrapping_sub(t) < 90
                    && distance(self.me.position, at) < 22.0
            })
        {
            return 1.0 / (1.0 + 5.0 * team);
        }
        let mut weight = 1.0 + 4.0 * team * claimed as f32;
        if self.seen.len() > 1
            && self
                .p
                .objective
                .as_ref()
                .and_then(|o| o.hill.as_ref())
                .is_some_and(|h| h.contains(o.position))
        {
            weight /= 1.0 + 3.0 * awareness(self.skill);
        }
        if self.opening(o, traits) {
            weight /= 1.0 + (0.8 + traits.aggression) * awareness(self.skill);
        }
        weight
    }

    fn posture(&self, d: &Decisions, o: Seen) -> Posture {
        let traits = d.personality;
        let sense = awareness(self.skill);
        let radius = 8.0 + 12.0 * sense;
        let enemies = self
            .seen
            .iter()
            .filter(|e| distance(e.position, self.me.position) <= radius)
            .count();
        let friends = self
            .p
            .others
            .iter()
            .filter(|a| {
                self.p.teams
                    && a.team == self.me.team
                    && distance(a.position, self.me.position) < radius
                    && clear(self.map, self.me.position, a.position)
            })
            .count();
        let margin = if sense + 0.5 * traits.caution - 0.25 * traits.aggression >= 0.65 {
            1
        } else {
            2
        };
        // In FFA a mutually engaged pair is an opening, until a third enemy joins.
        let distracted_pair =
            !self.p.teams && enemies == 2 && self.me.shield >= 0.65 && self.engaged(o);
        if enemies > friends + margin && !distracted_pair {
            return Posture::Withdraw;
        }
        if self.me.shield >= 0.55 && self.opening(o, traits) && sense + traits.aggression > 0.72 {
            Posture::Press
        } else {
            Posture::Normal
        }
    }

    fn support(&self, d: &Decisions) -> Option<Vec3> {
        let (id, team) = self.p.observations.player.filter(|_| self.p.teams)?;
        let regroup = d.disengaging(self.p.tick);
        let reach = (12.0 + 20.0 * d.personality.teamwork) * (0.5 + awareness(self.skill));
        if !regroup
            && let Some(c) = d
                .inbox
                .iter()
                .filter(|c| distance(self.me.position, c.at) < reach)
                .min_by_key(|c| (!c.help, self.p.tick.wrapping_sub(c.tick), c.sender))
        {
            return Some(c.at);
        }
        if !regroup
            && let Some((_, at, tick)) = d.trade
            && self.p.tick.wrapping_sub(tick) < 90
            && distance(self.me.position, at) < reach
        {
            return Some(at);
        }
        let mate = self
            .p
            .others
            .iter()
            .filter(|o| o.team == team && (o.id < id || regroup))
            .min_by(|a, b| {
                distance(self.me.position, a.position)
                    .total_cmp(&distance(self.me.position, b.position))
            })?;
        let gap = if regroup {
            3.0
        } else {
            10.0 + 9.0 * (1.0 - d.personality.teamwork)
        };
        (distance(self.me.position, mate.position) > gap).then_some(mate.position)
    }

    fn leash(&self, target: Seen, traits: Personality) -> Option<(Vec3, f32)> {
        let goal = self.goal?;
        if self
            .p
            .objective
            .as_ref()
            .is_some_and(|o| o.game == crate::objective::Game::Koth)
        {
            return match goal {
                Goal::Go(at) => Some((at, 0.0)),
                Goal::Hold(at, r) => Some((at, r)),
                _ => None,
            };
        }
        let radius = 4.0 + 8.0 * (1.0 - traits.objective_focus * awareness(self.skill));
        match goal {
            Goal::Escort(at)
                if distance(at, target.position) > radius
                    || distance(at, self.me.position) > radius =>
            {
                Some((at, crate::mind::ESCORT_RANGE))
            }
            Goal::Hold(at, r)
                if distance(target.position, at) > r || distance(self.me.position, at) > r =>
            {
                Some((at, r))
            }
            Goal::Hunt(id, at) if id != target.id => Some((at, 0.0)),
            Goal::Go(at)
                if distance(target.position, at) > radius
                    && (distance(target.position, self.me.position) > radius
                        || distance(target.position, at) > distance(self.me.position, at)) =>
            {
                Some((at, 0.0))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
