//! Where to go with nobody in view (docs/game/bots.md, section 3.2,
//! "Roaming"): to likely places, not at random. A bot remembers where it
//! has seen enemies ([`Roam::saw`]) and where it has been
//! ([`Roam::visit`]); a roam goal ([`Roam::pick`]) scores, in turn:
//!
//! - a recent sighting: highest, fading over [`SIGHT_TICKS`];
//! - a weapon lying in the world worth taking;
//! - the map's spawn points: on a large map with no sighting for a while,
//!   the likeliest place to find anyone is where players come from;
//! - a few polygons picked at random, so it still explores;
//!
//! less where it has been lately (fading over [`VISIT_TICKS`]) and less
//! right next to it, plus a random wobble the skill sets ([`noise`]): Easy
//! wanders nearly at random, Legendary goes where the fight is. The first
//! candidate the caller accepts (reachable, not blacklisted) wins.

use crate::arms::Pickup;
use crate::brain::Vec3;
use crate::mind::Skill;
use crate::nav::NavMesh;

/// A sighting counts for this long (60 s)...
pub const SIGHT_TICKS: u32 = 1800;
/// ...and one this close to another is the same place.
pub const SIGHT_MERGE: f32 = 4.0;
pub const SIGHT_MAX: usize = 8;
/// Where it has been counts against a goal this long (40 s)...
pub const VISIT_TICKS: u32 = 1200;
/// ...within this.
pub const VISIT_RADIUS: f32 = 5.0;
pub const VISIT_MAX: usize = 12;
/// A goal this close is barely worth the walk.
pub const NEAR: f32 = 4.0;
/// Random polygons among the candidates.
pub const RANDOM_PICKS: usize = 6;

/// The scores: a fresh sighting, a weapon, a spawn point, a random
/// polygon; and the penalties: a place just visited, a goal under
/// [`NEAR`].
pub const SIGHT_SCORE: f32 = 4.0;
pub const WEAPON_SCORE: f32 = 1.5;
pub const START_SCORE: f32 = 1.0;
pub const RANDOM_SCORE: f32 = 0.5;
pub const VISITED_PENALTY: f32 = 2.5;
pub const NEAR_PENALTY: f32 = 2.0;

/// The random wobble on each candidate's score, by skill.
pub fn noise(skill: Skill) -> f32 {
    match skill {
        Skill::Easy => 8.0,
        Skill::Normal => 2.0,
        Skill::Heroic => 1.0,
        Skill::Legendary => 0.4,
    }
}

/// One bot's roaming memory.
#[derive(Clone, Debug)]
pub struct Roam {
    rng: u64,
    /// Where enemies were seen, and when (latest).
    pub sightings: Vec<(Vec3, u32)>,
    /// Where it has been, and when.
    pub visited: Vec<(Vec3, u32)>,
}

fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// How much of `span` is left `since` ticks after `then`, 1 down to 0.
fn fresh(now: u32, then: u32, span: u32) -> f32 {
    let age = now.wrapping_sub(then);
    if age >= span {
        0.0
    } else {
        1.0 - age as f32 / span as f32
    }
}

impl Roam {
    pub fn new(seed: u64) -> Self {
        Roam {
            rng: (seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x726f_616d) | 1,
            sightings: Vec::new(),
            visited: Vec::new(),
        }
    }

    fn next(&mut self) -> u64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn uniform(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// An enemy was seen at `at`.
    pub fn saw(&mut self, at: Vec3, tick: u32) {
        Self::note(&mut self.sightings, at, tick, SIGHT_MERGE, SIGHT_MAX);
    }

    /// The bot was at `at` (it got to a goal, or chose a new one here).
    pub fn visit(&mut self, at: Vec3, tick: u32) {
        Self::note(&mut self.visited, at, tick, VISIT_RADIUS * 0.5, VISIT_MAX);
    }

    fn note(list: &mut Vec<(Vec3, u32)>, at: Vec3, tick: u32, merge: f32, max: usize) {
        if let Some(e) = list.iter_mut().find(|(p, _)| dist_xy(*p, at) < merge) {
            *e = (at, tick);
            return;
        }
        if list.len() >= max {
            // The oldest goes.
            if let Some(k) = (0..list.len()).max_by_key(|&k| tick.wrapping_sub(list[k].1)) {
                list.remove(k);
            }
        }
        list.push((at, tick));
    }

    /// The score of going to `c` from `at` (before the wobble).
    pub fn penalty(&self, at: Vec3, c: Vec3, tick: u32) -> f32 {
        let visited: f32 = self
            .visited
            .iter()
            .filter(|(p, _)| dist_xy(*p, c) < VISIT_RADIUS)
            .map(|&(_, t)| fresh(tick, t, VISIT_TICKS))
            .fold(0.0, f32::max);
        let near = if dist_xy(at, c) < NEAR {
            NEAR_PENALTY
        } else {
            0.0
        };
        VISITED_PENALTY * visited + near
    }

    /// The candidates, best first: `(score, point)`. `starts` are the map's
    /// spawn points.
    pub fn candidates(
        &mut self,
        at: Vec3,
        tick: u32,
        nav: &NavMesh,
        pickups: &[Pickup],
        starts: &[Vec3],
        skill: Skill,
    ) -> Vec<(f32, Vec3)> {
        let mut c: Vec<(f32, Vec3)> = Vec::new();
        for &(p, t) in &self.sightings {
            let f = fresh(tick, t, SIGHT_TICKS);
            if f > 0.0 {
                c.push((SIGHT_SCORE * f + 0.5, p));
            }
        }
        for pk in pickups.iter().filter(|pk| pk.useful()) {
            c.push((WEAPON_SCORE, pk.position));
        }
        for &s in starts {
            c.push((START_SCORE, s));
        }
        let n = nav.polys.len();
        if n > 0 {
            for _ in 0..RANDOM_PICKS {
                let i = (self.next() % n as u64) as usize;
                c.push((RANDOM_SCORE, nav.polys[i].center));
            }
        }
        let wobble = noise(skill);
        for e in &mut c {
            e.0 -= self.penalty(at, e.1, tick);
            e.0 += wobble * self.uniform();
        }
        c.sort_by(|a, b| b.0.total_cmp(&a.0));
        c
    }

    /// A goal: the best candidate `ok` accepts (the caller checks it can
    /// get there and has not got stuck going there).
    pub fn pick(
        &mut self,
        at: Vec3,
        tick: u32,
        nav: &NavMesh,
        pickups: &[Pickup],
        starts: &[Vec3],
        skill: Skill,
        ok: &dyn Fn(Vec3) -> bool,
    ) -> Option<Vec3> {
        self.candidates(at, tick, nav, pickups, starts, skill)
            .into_iter()
            .map(|(_, p)| p)
            .find(|&p| ok(p))
    }
}

#[cfg(test)]
mod tests;
