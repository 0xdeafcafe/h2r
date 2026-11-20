//! Translate decision-layer priorities into the existing pad and navigation actions.

use super::*;
use crate::brain::Me;
use crate::decisions::{Intent, supplies::Trip};

impl Mind {
    pub(super) fn decide(
        &mut self,
        p: &Perception,
        me: Me,
        seen: &[Seen],
        map: Option<&MapData>,
    ) -> (Option<Goal>, Intent) {
        let goal = p.objective.as_ref().and_then(|o| {
            objective::plan(
                o,
                &me,
                if o.game == objective::Game::Juggernaut {
                    seen
                } else {
                    &p.others
                },
                p.teams,
            )
        });
        // Once a tick: a repeated frame returns the last pad before this.
        if let Some(map) = map {
            self.decisions.map.familiarize(
                map,
                p.tick,
                crate::decisions::awareness(self.skill),
                self.decisions.personality.caution,
            );
        }
        self.decisions.observe(p, me, map);
        let mut intent = self.decisions.decide(
            &Fight {
                p,
                me,
                seen,
                skill: self.skill,
                goal,
                map,
            },
            self.target.map(|(id, _)| id),
        );
        if goal.is_none()
            && intent.chosen.is_some()
            && !self.recovering
            && self.decisions.posture != Posture::Withdraw
            && let Some(pad) = self.decisions.map.due(me, p.tick, self, map, Trip::Contest)
        {
            intent.anchor = Some((pad, crate::nav::REACHED));
        }
        (goal, intent)
    }

    pub(super) fn choose_activity(
        &mut self,
        p: &Perception,
        me: Me,
        goal: Option<Goal>,
        intent: &Intent,
        calm: bool,
        map: Option<&MapData>,
    ) {
        if self.decisions.posture == Posture::Press && !self.fight.wants_cover {
            // Close to the near end of the usable band, even with a power gun.
            let (near, far) = self.fight.band;
            self.fight.band.1 = (near + 1.5).min(far);
        }
        self.doing = match (intent.chosen, self.kit, goal) {
            (_, _, Some(Goal::Carry(at))) => Doing::Carry(at),
            (_, _, Some(Goal::Keep)) => Doing::Keep,
            (_, Kit::Seek(pk) | Kit::Dual(pk), _) => Doing::Rearm(pk.position),
            (Some(o), _, _) => Doing::Fight(o.id),
            (None, _, Some(g)) => Doing::Play(g),
            _ => self.between_fights(p, me, intent.support, calm, map),
        };
    }

    fn between_fights(
        &self,
        p: &Perception,
        me: Me,
        support: Option<Vec3>,
        calm: bool,
        map: Option<&MapData>,
    ) -> Doing {
        let last = self.last_seen.map(|(_, at, _)| at);
        if self.recovering
            && let Some(at) = last
        {
            return Doing::Recover(at);
        }
        if let Some(at) = support.filter(|at| !self.stuck.blocked(*at, p.tick)) {
            return Doing::Support(at);
        }
        let behavior = self.decisions.behavior();
        if (self.decisions.disengaging(p.tick) || behavior.fear() > 0.28)
            && let Some(at) = last
        {
            return Doing::Recover(at);
        }
        if calm
            && self.kit == Kit::Fine
            && let Some(at) = self.decisions.map.due(me, p.tick, self, map, Trip::Search)
        {
            return Doing::Supply(at);
        }
        if let Some((id, at, tick)) = self.last_seen
            && !behavior.wants_revenge(id)
            && p.tick.wrapping_sub(tick) < behavior.hold_ticks()
        {
            return Doing::HoldAngle(at);
        }
        if let Some((_, at, seen)) = self.last_seen {
            // A distant, old sighting is less likely to pay for a long chase.
            let confidence = 1.0 - p.tick.wrapping_sub(seen) as f32 / MEMORY_TICKS as f32;
            let travel = len(sub(at, me.position)) * 6.0 / MEMORY_TICKS as f32;
            if confidence > travel.min(0.65) {
                return Doing::Chase(at);
            }
        }
        if let Some((at, heard)) = self.search {
            let confidence = 1.0 - p.tick.wrapping_sub(heard) as f32 / MEMORY_TICKS as f32;
            if confidence > 0.35
                && len(sub(me.position, at)) > 1.0
                && !self.stuck.blocked(at, p.tick)
            {
                return Doing::Support(at);
            }
        }
        if calm && let Some(map) = map {
            let range = me
                .arms
                .and_then(|a| a.held)
                .filter(|g| g.ready())
                .map(|g| g.role());
            let sense = crate::decisions::awareness(self.skill);
            let long = matches!(range, Some(arms::Role::Sniper | arms::Role::Precise));
            if long || p.observations.ahead {
                let personality = self.decisions.personality;
                let interest = range.map_or(0., |role| personality.weapon_interest(role));
                let reach = if long {
                    8.0 + (40.0 + 10.0 * interest) * sense
                } else {
                    7.0
                };
                let position_weight =
                    10.0 + 16.0 * sense + 5.0 * interest + 4.0 * personality.wait_weight();
                let known = (map.places.vantages.len() as f32 * sense).ceil() as usize;
                let worth = |v: &crate::places::Vantage| {
                    (v.score + 0.2 * self.experience.firing_position(v.at)) * position_weight
                        - len(sub(v.at, me.position))
                            * (1.0 - 0.4 * self.decisions.personality.caution)
                };
                if let Some(&v) = map
                    .places
                    .vantages
                    .iter()
                    .take(known)
                    .filter(|v| {
                        len(sub(v.at, me.position)) <= reach
                            && !self.stuck.blocked(v.at, p.tick)
                            && !self.decisions.map.hot(v.at, p.tick, 3.0)
                    })
                    .max_by(|a, b| worth(a).total_cmp(&worth(b)))
                {
                    return Doing::Vantage(v);
                }
            }
        }
        Doing::Roam
    }

    pub(crate) fn hear_deaths(
        &mut self,
        p: &Perception,
        kills: &[crate::emote::Kill],
        map: Option<&MapData>,
    ) {
        for k in kills {
            if !k.betrayal
                && k.attacker.is_some_and(|id| {
                    Some(id) == p.observations.player.map(|(own, _)| own) && id != k.victim
                })
            {
                self.experience.own_kill(k.victim, k.tick);
            }
            self.fight.forget(k.victim);
            if self
                .last_seen
                .is_some_and(|(id, _, seen)| id == k.victim && reached(k.tick, seen))
            {
                // The kill feed retires the old contact. Look towards a
                // plausible distant spawn, with no knowledge of the actual spawn.
                if let (Some(me), Some(map)) = (p.me, map) {
                    self.search = map
                        .places
                        .starts
                        .iter()
                        .filter(|&&at| !crate::perceive::in_view(me, at, CHEST, Some(map)))
                        .max_by(|a, b| {
                            len(sub(**a, me.position)).total_cmp(&len(sub(**b, me.position)))
                        })
                        .map(|&at| (at, k.tick));
                }
                self.last_seen = None;
                self.tracks.remove(&k.victim);
            }
        }
        if self.decisions.hear(p, kills) {
            self.foot_plan.expedite();
            self.follower = None;
        }
    }

    pub(super) fn carrier_threat_changed(&mut self, tick: u32) -> bool {
        let threat = if matches!(self.doing, Doing::Carry(_)) {
            self.last_seen
                .filter(|(_, _, t)| tick.wrapping_sub(*t) < 90)
                .map(|(_, at, _)| at)
                .or_else(|| {
                    self.decisions
                        .inbox
                        .iter()
                        .min_by_key(|c| (tick.wrapping_sub(c.tick), c.sender))
                        .map(|c| c.at)
                })
        } else {
            None
        };
        self.decisions.map.new_route_threat(threat)
    }

    /// Keeping the ball: shelter from the threat, else away from it. One
    /// at arm's length is charged and looked at, and returned for the mind
    /// to melee (the buttons' timing, mind::pad).
    pub(super) fn keep_ball(
        &mut self,
        at: Vec3,
        seen: &[Seen],
        tick: u32,
        map: Option<&MapData>,
        bodies: &[Vec3],
        pad: &mut HostPadState,
    ) -> (Option<Vec3>, Option<Walk>, Option<Seen>) {
        let near = seen
            .iter()
            .map(|o| (*o, len(sub(o.position, at))))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((foe, _)) = near.filter(|(_, d)| *d < MELEE_RANGE * 1.3) {
            let to = sub(foe.position, at);
            let flat = to[0].hypot(to[1]).max(1e-6);
            return (
                Some(up(foe.position, CHEST)),
                Some(Walk::straight([to[0] / flat, to[1] / flat])),
                Some(foe),
            );
        }
        let threat = near.map(|(o, _)| o.position).or_else(|| {
            self.last_seen
                .filter(|(_, _, t)| tick.wrapping_sub(*t) < 90)
                .map(|(_, p, _)| p)
        });
        let walk = if let Some(spot) = self.decisions.map.shelter(at, threat, tick, map) {
            if len(sub(spot, at)) <= crate::nav::REACHED {
                None
            } else {
                self.go(at, spot, tick, map, pad, bodies)
            }
        } else {
            near.filter(|(_, d)| *d < KEEP_AWAY)
                .and_then(|(foe, _)| self.away(at, foe.position, map, bodies))
        };
        (None, walk, None)
    }
}
