//! Places learned in this match. All positions come from play.
use super::{distance, supplies::Timers};
use crate::brain::{Me, Perception, Vec3};
use crate::cache::MapData;
use crate::mind::{CHEST, EYE, Skill};
use crate::nav::{Edge, NavMesh, Waypoint};
use crate::stuck::Stuck;

pub(super) const HOT_TICKS: u32 = 600;

#[derive(Clone, Debug, Default)]
pub(crate) struct MapMemory {
    familiar: bool,
    deaths: Vec<(Vec3, u32)>,
    pub(super) timers: Timers,
    /// A defensible ball position, reconsidered on a short cooldown.
    shelter: Option<Shelter>,
    route_threat: Option<Vec3>,
}

#[derive(Clone, Debug)]
struct Shelter {
    spot: Option<Vec3>,
    threat: Option<Vec3>,
    tick: u32,
}

fn moved(previous: Option<Vec3>, current: Option<Vec3>, threshold: f32) -> bool {
    match (previous, current) {
        (Some(p), Some(q)) => distance(p, q) > threshold,
        (None, None) => false,
        _ => true,
    }
}

impl MapMemory {
    pub(crate) fn new_route_threat(&mut self, threat: Option<Vec3>) -> bool {
        if moved(self.route_threat, threat, 3.0) {
            self.route_threat = threat;
            return true;
        }
        false
    }

    pub(super) fn death(&mut self, at: Vec3, tick: u32) {
        if let Some(zone) = self.deaths.iter_mut().find(|(p, _)| distance(*p, at) < 1.5) {
            *zone = (at, tick);
        } else {
            if self.deaths.len() == 12 {
                self.deaths.remove(0);
            }
            self.deaths.push((at, tick));
        }
    }

    pub(crate) fn familiarize(&mut self, map: &MapData, tick: u32, sense: f32, personality: f32) {
        if self.familiar {
            return;
        }
        for pad in &map.places.pads {
            let rank = (u32::from(pad.site) * 73 + (personality * 127.) as u32) % 128;
            if rank as f32 / 128. < sense {
                self.timers.familiar(*pad, tick);
            }
        }
        self.familiar = true;
    }

    pub(super) fn observe(&mut self, p: &Perception, me: Me, map: Option<&MapData>) {
        self.deaths
            .retain(|(_, t)| p.tick.wrapping_sub(*t) < HOT_TICKS);
        let visible = |at: Vec3| {
            let dist = distance(me.position, at);
            let facing = (0..3)
                .map(|i| (at[i] - me.position[i]) * me.aim[i])
                .sum::<f32>();
            dist <= 25.0
                && (dist < 3.0 || facing > dist * 0.42)
                && map.is_none_or(|m| crate::perceive::in_view(me, at, 0.2, Some(m)))
        };
        self.timers
            .observe(p.tick, p.observations.supplies.as_deref(), visible);
        self.timers.update_known(p, visible);
        self.timers.carriers(p, me);
    }

    pub(super) fn holder(&self, id: u32) -> bool {
        self.timers.holder(id)
    }

    pub(crate) fn due(
        &self,
        me: Me,
        tick: u32,
        skill: Skill,
        stuck: &Stuck,
        map: Option<&MapData>,
        experience: &crate::experience::Experience,
        trip: super::supplies::Trip,
    ) -> Option<Vec3> {
        self.timers.due(me, tick, skill, trip, |at| {
            (!self.hot(at, tick, 3.0) && !stuck.blocked(at, tick))
                .then(|| map.map_or(0., |m| m.places.strength(at)) + 0.3 * experience.success(at))
        })
    }

    pub(crate) fn hot(&self, at: Vec3, tick: u32, radius: f32) -> bool {
        self.deaths.iter().any(|(p, t)| {
            let fade = 1.0 - tick.wrapping_sub(*t) as f32 / HOT_TICKS as f32;
            fade > 0.0 && distance(at, *p) < radius * fade.sqrt()
        })
    }

    /// Try safe portals first, then relax danger, then the stuck blacklist.
    /// Even a carrier can take the sole way home.
    pub(crate) fn path(
        &self,
        nav: &NavMesh,
        at: Vec3,
        goal: Vec3,
        tick: u32,
        stuck: &Stuck,
        caution: f32,
    ) -> Option<Vec<Waypoint>> {
        let blocked = |e: &Edge| {
            if e.kind.traversal() {
                stuck.blocked(e.a, tick) || stuck.blocked(e.b, tick)
            } else {
                stuck.blocks_segment(e.a, e.b, tick)
            }
        };
        let threat = self.route_threat;
        if !self.deaths.is_empty() || threat.is_some() {
            let danger = |p| {
                self.hot(p, tick, 2.5 + caution * 2.0)
                    || threat.is_some_and(|t| distance(t, p) < 5.0)
            };
            // Every corridor ends in the goal polygon. Compare its entrances
            // even when that unavoidable destination is dangerous.
            let end = nav.poly_near(goal);
            let safe = nav.path_avoiding(at, goal, &|e| {
                let mid = if e.kind.traversal() {
                    e.a
                } else {
                    std::array::from_fn(|i| (e.a[i] + e.b[i]) * 0.5)
                };
                blocked(e)
                    || danger(mid)
                    || (e.kind.traversal() && Some(e.to) != end && danger(e.b))
                    || (Some(e.to) != end && danger(nav.polys[e.to as usize].center))
            });
            if safe.is_some() {
                return safe;
            }
        }
        nav.path_avoiding(at, goal, &blocked)
            .or_else(|| nav.path(at, goal))
    }

    /// Preserve the roam scores within each group. A dangerous goal is
    /// still available when every other candidate is unreachable.
    pub(crate) fn safe_first(&self, candidates: &mut [(f32, Vec3)], tick: u32) {
        if !self.deaths.is_empty() {
            candidates.sort_by_key(|(_, at)| self.hot(*at, tick, 4.0));
        }
    }

    pub(super) fn unobserved(&mut self, tick: u32) {
        self.timers.observe(tick, None, |_| false);
        self.leave_shelter();
    }

    pub(crate) fn leave_shelter(&mut self) {
        self.shelter = None;
    }

    /// Use a reachable hiding place with a broken sightline to a known
    /// enemy. Without a sighting, prefer positions screened on more sides.
    pub(crate) fn shelter(
        &mut self,
        at: Vec3,
        threat: Option<Vec3>,
        tick: u32,
        map: Option<&MapData>,
    ) -> Option<Vec3> {
        let map = map?;
        if let Some(last) = &self.shelter
            && tick.wrapping_sub(last.tick) < 60
            && !moved(last.threat, threat, 1.5)
        {
            return last.spot;
        }
        let spot = if let Some(foe) = threat {
            if !map.walls.clear(raised(foe, EYE), raised(at, CHEST)) {
                Some(at)
            } else {
                crate::moves::cover(map, at, foe)
            }
        } else {
            let cover = |p: Vec3| {
                let eye = raised(p, EYE);
                [[4.0, 0.0], [-4.0, 0.0], [0.0, 4.0], [0.0, -4.0]]
                    .iter()
                    .filter(|d| !map.walls.clear(eye, [eye[0] + d[0], eye[1] + d[1], eye[2]]))
                    .count()
            };
            let mut best = (cover(at), at);
            for d in [[3.0, 0.0], [-3.0, 0.0], [0.0, 3.0], [0.0, -3.0]] {
                let q = [at[0] + d[0], at[1] + d[1], at[2]];
                if map.nav.locate(q, 0.5).is_none() || self.hot(q, tick, 3.0) {
                    continue;
                }
                let score = cover(q);
                if score > best.0 && map.nav.path(at, q).is_some() {
                    best = (score, q);
                }
            }
            Some(best.1)
        };
        self.shelter = Some(Shelter { spot, threat, tick });
        spot
    }
}

fn raised(p: Vec3, h: f32) -> Vec3 {
    [p[0], p[1], p[2] + h]
}
