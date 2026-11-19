//! Translate decision-layer priorities into the existing pad and navigation actions.

use super::*;
use crate::brain::Me;
use crate::decisions::Intent;

impl Mind {
    pub(super) fn decide(
        &mut self,
        p: &Perception,
        me: Me,
        seen: &[Seen],
        map: Option<&MapData>,
    ) -> (Option<Goal>, Intent) {
        let goal = p
            .objective
            .as_ref()
            .and_then(|o| objective::plan(o, &me, &p.others, p.teams));
        // Once a tick: a repeated frame returns the last pad before this.
        self.decisions.observe(p, me, map);
        let intent = self.decisions.decide(
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
        (goal, intent)
    }

    pub(super) fn choose_activity(
        &mut self,
        p: &Perception,
        me: Me,
        goal: Option<Goal>,
        intent: &Intent,
        calm: bool,
    ) {
        if self.decisions.posture == Posture::Press && !self.fight.wants_cover {
            // Close to the near end of the usable band, even with a power gun.
            let (near, far) = self.fight.band;
            self.fight.band.1 = (near + 1.5).min(far);
        }
        self.doing = match (intent.chosen, self.kit, goal) {
            (_, _, Some(Goal::Carry(at))) => Doing::Carry(at),
            (_, _, Some(Goal::Keep)) => Doing::Keep,
            (_, Kit::Seek(pk), _) => Doing::Rearm(pk.position),
            (Some(o), _, _) => Doing::Fight(o.id),
            (None, _, Some(g)) => Doing::Play(g),
            _ => self.between_fights(p, me, intent.support, calm),
        };
    }

    fn between_fights(&self, p: &Perception, me: Me, support: Option<Vec3>, calm: bool) -> Doing {
        let last = self.last_seen.map(|(_, at, _)| at);
        if self.recovering
            && let Some(at) = last
        {
            return Doing::Recover(at);
        }
        if let Some(at) = support.filter(|at| !self.stuck.blocked(*at, p.tick)) {
            return Doing::Support(at);
        }
        if self.decisions.disengaging(p.tick)
            && let Some(at) = last
        {
            return Doing::Recover(at);
        }
        if calm
            && self.kit == Kit::Fine
            && let Some(at) = self.decisions.map.due(me, p.tick, self.skill, &self.stuck)
        {
            return Doing::Supply(at);
        }
        if let Some(at) = last {
            return Doing::Chase(at);
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
        Doing::Roam
    }

    pub(crate) fn hear_deaths(&mut self, p: &Perception, kills: &[crate::emote::Kill]) {
        for k in kills {
            self.fight.forget(k.victim);
            if self
                .last_seen
                .is_some_and(|(id, _, seen)| id == k.victim && reached(k.tick, seen))
            {
                self.last_seen = None;
                self.tracks.remove(&k.victim);
            }
        }
        if self.decisions.hear(p, kills) {
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
