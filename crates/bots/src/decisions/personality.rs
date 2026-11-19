//! Stable identity draws and the match-local reaction to what happened.

use crate::arms::Role;
use crate::brain::{Perception, Vec3};
use crate::emote::Kill;
use crate::mind::Skill;

use super::distance;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Personality {
    pub aggression: f32,
    pub caution: f32,
    pub teamwork: f32,
    pub objective_focus: f32,
    pub composure: f32,
    pub patience: f32,
    pub showboating: f32,
    pub weapon_loyalty: f32,
    pub chattiness: f32,
    pub favorite_weapon: Role,
}

// Aggression, caution, teamwork, objective focus, composure, patience,
// showboating, weapon loyalty, chattiness. Keep in sync with bots.md.
#[rustfmt::skip]
const BANDS: [[(f32, f32); 9]; 4] = [
    // Easy
    [(0.05, 0.95), (0.05, 0.90), (0.08, 0.85), (0.05, 0.90), (0.08, 0.50),
     (0.05, 0.48), (0.10, 0.95), (0.05, 0.95), (0.05, 0.95)],
    // Normal
    [(0.12, 0.92), (0.16, 0.88), (0.18, 0.88), (0.18, 0.88), (0.28, 0.70),
     (0.22, 0.72), (0.10, 0.87), (0.13, 0.88), (0.12, 0.90)],
    // Heroic
    [(0.24, 0.90), (0.30, 0.84), (0.32, 0.88), (0.35, 0.90), (0.50, 0.86),
     (0.42, 0.85), (0.12, 0.75), (0.20, 0.80), (0.25, 0.83)],
    // Legendary
    [(0.35, 0.88), (0.46, 0.82), (0.46, 0.87), (0.48, 0.89), (0.73, 0.96),
     (0.65, 0.91), (0.15, 0.67), (0.28, 0.72), (0.36, 0.80)],
];

const FAVORITES: [Role; 6] = [
    Role::Spray,
    Role::Precise,
    Role::Scatter,
    Role::Sniper,
    Role::Blast,
    Role::Blade,
];

impl Personality {
    pub fn seeded(seed: u64) -> Self {
        Self::for_skill(seed, Skill::Normal)
    }

    pub fn for_skill(mut seed: u64, skill: Skill) -> Self {
        let mut draw = || {
            seed = seed.wrapping_add(0x9e3779b97f4a7c15);
            let mut n = seed;
            n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
            ((n ^ (n >> 31)) >> 40) as f32 / 16777216.0
        };
        let band = BANDS[skill as usize];
        let [
            aggression,
            caution,
            teamwork,
            objective_focus,
            composure,
            patience,
            showboating,
            weapon_loyalty,
            chattiness,
        ] = band.map(|(lo, hi)| lo + (hi - lo) * draw());
        Self {
            aggression,
            caution,
            teamwork,
            objective_focus,
            composure,
            patience,
            showboating,
            weapon_loyalty,
            chattiness,
            favorite_weapon: FAVORITES[(draw() * FAVORITES.len() as f32) as usize],
        }
    }

    pub(crate) fn favorite_detour(self) -> f32 {
        if self.weapon_loyalty >= 0.5 {
            6.0 + 8.0 * self.weapon_loyalty
        } else {
            0.0
        }
    }

    /// Willingness to wait for an expected spawn, in 0..1.
    pub fn wait_weight(self) -> f32 {
        self.patience
    }

    /// Interest in a known pickup or expected spawn of this weapon role.
    pub fn weapon_interest(self, role: Role) -> f32 {
        if self.favorite_weapon == role {
            self.weapon_loyalty
        } else {
            0.0
        }
    }
}

const REVENGE_TICKS: u32 = 300;
const TAUNT_TICKS: u32 = 90;
pub(super) const DUEL_TICKS: u32 = 60;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoodView {
    pub confidence: f32,
    pub tilt: f32,
    pub fear: f32,
    pub streak: u32,
    pub revenge_target: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
struct Corpse {
    killer: u32,
    at: Vec3,
    tick: u32,
    crouching: bool,
    presses: u8,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Mood {
    state: MoodView,
    tick: Option<u32>,
    revenge_since: Option<u32>,
    corpse: Option<Corpse>,
    pub(super) favored_duel: Option<(u32, u32)>,
}

impl Mood {
    pub(super) fn view(&self) -> MoodView {
        self.state
    }

    pub(super) fn advance(&mut self, tick: u32, personality: Personality) {
        if let Some(last) = self.tick.replace(tick) {
            let dt = tick.wrapping_sub(last);
            if dt >= 1 << 31 {
                *self = Self {
                    tick: Some(tick),
                    ..Self::default()
                };
                return;
            }
            if dt > 0
                && (self.state.confidence > 0.0 || self.state.tilt > 0.0 || self.state.fear > 0.0)
            {
                let half_life = 540.0 - 300.0 * personality.composure;
                let decay = (-(dt as f32) / half_life).exp2();
                self.state.confidence *= decay;
                self.state.tilt *= decay;
                self.state.fear *= decay;
            }
        }
        if self
            .revenge_since
            .is_some_and(|t| tick.wrapping_sub(t) >= REVENGE_TICKS)
        {
            self.state.revenge_target = None;
            self.revenge_since = None;
        }
    }

    pub(super) fn hear(
        &mut self,
        p: &Perception,
        id: u32,
        kills: &[Kill],
        personality: Personality,
    ) {
        self.advance(p.tick, personality);
        let response = 1.0 - 0.75 * personality.composure;
        for k in kills {
            if p.tick.wrapping_sub(k.tick) >= REVENGE_TICKS {
                continue;
            }
            if self.state.revenge_target == Some(k.victim) {
                self.state.revenge_target = None;
                self.revenge_since = None;
            }
            if self.corpse.is_some_and(|corpse| corpse.killer == k.victim) {
                self.corpse = None;
            }
            if k.victim == id {
                let lost_duel = !k.betrayal
                    && self.favored_duel.is_some_and(|(opponent, tick)| {
                        Some(opponent) == k.attacker && k.tick.wrapping_sub(tick) < DUEL_TICKS
                    });
                self.state.streak = 0;
                self.state.confidence *= 1.0 - 0.45 * response;
                let tilt = 0.40
                    + if k.betrayal {
                        0.28
                    } else if lost_duel {
                        0.25
                    } else {
                        0.0
                    };
                self.state.tilt = (self.state.tilt + tilt * response).min(1.0);
                self.state.fear = (self.state.fear + 0.44 * response).min(1.0);
                let enemy = k.attacker.filter(|&a| a != id && !k.betrayal);
                self.state.revenge_target = enemy;
                self.revenge_since = enemy.map(|_| k.tick);
                self.corpse = k.at.zip(enemy).map(|(at, killer)| Corpse {
                    killer,
                    at,
                    tick: k.tick,
                    crouching: false,
                    presses: 0,
                });
                self.favored_duel = None;
            } else if k.attacker == Some(id) && !k.betrayal {
                self.state.streak = self.state.streak.saturating_add(1);
                let bonus = 0.24 + 0.07 * self.state.streak.saturating_sub(1).min(4) as f32;
                self.state.confidence = (self.state.confidence + bonus * response).min(1.0);
                self.state.fear *= 0.65;
                self.state.tilt *= 0.85;
            }
        }
        if let Some(mut corpse) = self.corpse.take() {
            let age = p.tick.wrapping_sub(corpse.tick);
            if age >= TAUNT_TICKS || (p.me.is_some() && age != 0) {
                return;
            }
            let nearby = p.others.iter().any(|o| {
                o.id == corpse.killer && distance(o.position, corpse.at) <= crate::emote::NEAR
            });
            if let Some(contact) = p
                .observations
                .contacts
                .iter()
                .find(|c| c.id == corpse.killer)
                && nearby
            {
                corpse.presses += u8::from(contact.crouching && !corpse.crouching);
                corpse.crouching = contact.crouching;
            }
            if corpse.presses >= 2 {
                self.state.tilt = (self.state.tilt + 0.32 * response).min(1.0);
            } else {
                self.corpse = Some(corpse);
            }
        }
    }
}

/// The weights that decision consumers get from the current personality and mood.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Behavior {
    pub personality: Personality,
    pub mood: MoodView,
}

impl Behavior {
    pub fn drive(self) -> f32 {
        (0.85 * self.mood.tilt + 0.65 * self.mood.confidence
            - self.mood.fear * (0.45 + 0.55 * self.personality.caution))
            .clamp(-1.0, 1.0)
    }

    pub fn fear(self) -> f32 {
        (self.mood.fear * (0.55 + 0.7 * self.personality.caution)
            - 0.55 * self.mood.tilt
            - 0.65 * self.mood.confidence)
            .max(0.0)
    }

    pub fn wants_revenge(self, id: u32) -> bool {
        self.mood.revenge_target == Some(id) && self.mood.tilt >= 0.35 && self.fear() < 0.25
    }

    pub fn hold_ticks(self) -> u32 {
        let patience = self.personality.wait_weight() - self.drive() * 0.45;
        (90.0 * (patience - 0.68).clamp(0.0, 0.4)) as u32
    }

    pub fn strafe_hold(self) -> f32 {
        0.75 + 0.50 * self.personality.wait_weight()
    }

    pub fn stand_bonus(self) -> f32 {
        0.16 * self.personality.wait_weight() - 0.14 * self.drive()
    }

    pub fn spacing(self) -> f32 {
        (1.0 - 0.55 * self.drive()).clamp(0.55, 1.25)
    }

    pub fn melee_risk(self) -> bool {
        self.personality.showboating + 0.35 * self.drive() > 0.65
    }

    pub fn grenade_rate(self) -> f32 {
        (1.0 + 1.25 * self.mood.tilt - 0.45 * self.fear()).clamp(0.65, 2.25)
    }

    pub fn taunt_weight(self) -> f32 {
        (0.25 + 1.5 * self.personality.showboating + 0.25 * self.mood.confidence).min(2.0)
    }

    pub fn radio_interval(self) -> u32 {
        (super::radio::MAX_REPORT_INTERVAL as f32 - 20.0 * self.personality.chattiness).round()
            as u32
    }

    pub fn aim_noise(self) -> f32 {
        1.0 + (0.08 * self.mood.tilt + 0.06 * self.fear()).min(0.10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temperament_draws_keep_each_skill_band_and_identity() {
        let mut aggression = [(1.0f32, 0.0f32); 4];
        for skill in Skill::ALL {
            let index = skill as usize;
            for seed in 0..4096 {
                let p = Personality::for_skill(seed, skill);
                assert_eq!(p, Personality::for_skill(seed, skill));
                assert_eq!(p.favorite_weapon, Personality::seeded(seed).favorite_weapon);
                let traits = [
                    p.aggression,
                    p.caution,
                    p.teamwork,
                    p.objective_focus,
                    p.composure,
                    p.patience,
                    p.showboating,
                    p.weapon_loyalty,
                    p.chattiness,
                ];
                for (value, (lo, hi)) in traits.into_iter().zip(BANDS[index]) {
                    assert!(
                        (lo..=hi).contains(&value),
                        "{skill:?}: {value} outside {lo}..{hi}"
                    );
                }
                let range = &mut aggression[index];
                range.0 = range.0.min(p.aggression);
                range.1 = range.1.max(p.aggression);
            }
        }
        for pair in aggression.windows(2) {
            assert!(pair[0].1 - pair[0].0 > pair[1].1 - pair[1].0);
        }
        assert!(aggression[3].0 < 0.4 && aggression[3].1 > 0.85);
        assert!(BANDS[0][4].1 < BANDS[3][4].0);
        assert!(BANDS[0][5].1 < BANDS[3][5].0);
        assert_ne!(Personality::seeded(39), Personality::seeded(40));
    }
}
