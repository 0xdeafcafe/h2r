//! A gamertag fixes a Clanker's tendencies. Match RNG decides which
//! opportunities it takes. Archetypes describe the draws; they do not
//! select a separate decision policy.

use crate::decisions::Personality;
use crate::mind::Skill;
use crate::tuning::Tuning;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Archetype {
    Scout,
    Brawler,
    Grenadier,
    Marksman,
    Skirmisher,
    Gunslinger,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct NameProfile {
    pub seed: u64,
    pub try_hard: f32,
    pub archetype: Archetype,
}

/// Relative tendencies in (0, 1), measured inside the difficulty's
/// band. Reaction jitter increases toward the less consistent end.
/// Weapon preference is loyalty to the name's favorite weapon family.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Weights {
    pub radar_use: f32,
    pub noob_combo: f32,
    pub dual_wield: f32,
    pub aggression: f32,
    pub grenades: f32,
    pub melee: f32,
    pub weapon_preference: f32,
    pub range_preference: f32,
    pub strafe: f32,
    pub jump: f32,
    pub reaction_jitter: f32,
    pub retreat: f32,
}

impl Weights {
    pub(crate) fn spacing(self, (near, far): (f32, f32)) -> (f32, f32) {
        let width = (far - near).max(0.0);
        // Narrow within the held gun's band. Even a distance-loving bot
        // still has to close with an SMG or sword.
        (near + 0.20 * width * self.range_preference,
         far - 0.30 * width * (1.0 - self.range_preference))
    }
}

impl From<[f32; 12]> for Weights {
    fn from(values: [f32; 12]) -> Self {
        let [radar_use, noob_combo, dual_wield, aggression, grenades, melee, weapon_preference,
            range_preference, strafe, jump, reaction_jitter, retreat] = values;
        Self {
            radar_use, noob_combo, dual_wield, aggression, grenades, melee, weapon_preference,
            range_preference, strafe, jump, reaction_jitter, retreat,
        }
    }
}

struct Trait {
    /// Fraction supplied by try_hard. A negative weight takes its inverse.
    correlation: f32,
    /// Easy, Normal, Heroic, Legendary. All are interior probability bands.
    bands: [(f32, f32); 4],
    /// A unit of custom-game push (or Twitchy) shifts the raw percentile
    /// toward the corresponding band edge before rescaling.
    push: f32,
    twitchy: f32,
}

const fn axis(correlation: f32, bands: [(f32, f32); 4], push: f32, twitchy: f32) -> Trait {
    Trait { correlation, bands, push, twitchy }
}

// Keep the row order with Weights::from. Independent lanes of SplitMix64
// let the same name express different tendencies at the same try_hard.
#[rustfmt::skip]
const TRAITS: [Trait; 12] = [
    //           try_hard    Easy        Normal       Heroic       Legendary     push   twitchy
    /* radar */ axis( 0.72, [(0.22,0.68),(0.38,0.83),(0.50,0.92),(0.64,0.97)],  0.0, 0.0),
    /* combo */ axis( 0.65, [(0.07,0.32),(0.20,0.63),(0.40,0.85),(0.59,0.95)],  0.0, 0.0),
    /* dual  */ axis(-0.40, [(0.20,0.88),(0.18,0.84),(0.15,0.78),(0.12,0.72)],  0.0, 0.0),
    /* push  */ axis( 0.18, [(0.05,0.95),(0.12,0.92),(0.24,0.90),(0.35,0.88)],  1.0, 0.0),
    /* nades */ axis( 0.43, [(0.12,0.65),(0.26,0.78),(0.43,0.90),(0.53,0.95)],  0.4, 0.0),
    /* melee */ axis( 0.22, [(0.22,0.82),(0.27,0.87),(0.32,0.90),(0.39,0.93)],  0.6, 0.0),
    /* guns  */ axis(-0.16, [(0.05,0.95),(0.13,0.88),(0.20,0.80),(0.28,0.72)],  0.0, 0.0),
    /* range */ axis( 0.12, [(0.05,0.95),(0.10,0.90),(0.15,0.87),(0.18,0.85)], -0.6, 0.0),
    /* strafe*/ axis( 0.68, [(0.14,0.60),(0.31,0.76),(0.49,0.90),(0.61,0.96)],  0.0, 0.0),
    /* jump  */ axis( 0.54, [(0.05,0.30),(0.10,0.48),(0.18,0.64),(0.24,0.73)],  0.2, 0.0),
    /* jitter*/ axis(-0.74, [(0.34,0.95),(0.23,0.80),(0.12,0.61),(0.06,0.47)],  0.0, -0.20),
    /*retreat*/ axis( 0.24, [(0.05,0.90),(0.16,0.88),(0.30,0.84),(0.46,0.82)], -0.8, 0.0),
];

fn draw(seed: u64, lane: u64) -> f32 {
    let mut n = seed.wrapping_add(lane.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    n = (n ^ (n >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    // Keep a small interior margin even after a narrow stylistic band
    // rescales the percentile and rounds it to f32.
    let u = (((n ^ (n >> 31)) >> 41) as f32 + 0.5) / 8_388_608.0;
    0.0001 + 0.9998 * u
}

impl NameProfile {
    /// FNV-1a 64 over UTF-8 with the same ASCII case fold as Gamertag::key.
    /// The hash is independent of platform, roster order and match seed.
    pub fn for_name(name: &str) -> Self {
        let seed = name.as_bytes().iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
            (h ^ u64::from(b.to_ascii_lowercase())).wrapping_mul(0x100_0000_01b3)
        });
        let mut profile = Self { seed, try_hard: draw(seed, 1), archetype: Archetype::Scout };
        let raw = Weights::from(profile.percentiles());
        let strongest = [
            (Archetype::Scout, raw.radar_use),
            (Archetype::Brawler, raw.melee),
            (Archetype::Grenadier, raw.grenades),
            (Archetype::Marksman, raw.range_preference),
            (Archetype::Skirmisher, raw.strafe),
            (Archetype::Gunslinger, raw.dual_wield),
        ];
        profile.archetype = strongest.into_iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("six archetypes").0;
        profile
    }

    fn percentiles(self) -> [f32; 12] {
        std::array::from_fn(|i| {
            let c = TRAITS[i].correlation;
            let latent = if c < 0.0 { 1.0 - self.try_hard } else { self.try_hard };
            (1.0 - c.abs()) * draw(self.seed, 2 + i as u64) + c.abs() * latent
        })
    }

    pub fn weights(&self, skill: Skill) -> Weights {
        self.styled_weights(skill, &Tuning::default())
    }

    pub(crate) fn styled_weights(&self, skill: Skill, tuning: &Tuning) -> Weights {
        let raw = self.percentiles();
        std::array::from_fn(|i| {
            let trait_ = &TRAITS[i];
            let shift = (trait_.push * tuning.push_bias
                + if tuning.twitchy { trait_.twitchy } else { 0.0 }).clamp(-0.9, 0.9);
            let x = raw[i] + shift * if shift < 0.0 { raw[i] } else { 1.0 - raw[i] };
            let (min, max) = trait_.bands[skill as usize];
            min + (max - min) * x
        }).into()
    }

    pub fn personality(&self, skill: Skill) -> Personality {
        self.personality_with(skill, self.weights(skill))
    }

    pub(crate) fn personality_with(&self, skill: Skill, w: Weights) -> Personality {
        Personality {
            aggression: w.aggression,
            caution: w.retreat,
            weapon_loyalty: w.weapon_preference,
            ..Personality::for_skill(self.seed, skill)
        }
    }

    pub fn label(&self) -> &'static str {
        match self.archetype {
            Archetype::Scout => "Scout",
            Archetype::Brawler => "Brawler",
            Archetype::Grenadier => "Grenadier",
            Archetype::Marksman => "Marksman",
            Archetype::Skirmisher => "Skirmisher",
            Archetype::Gunslinger => "Gunslinger",
        }
    }

    /// A short card line. Rank the neutral name draws so a difficulty
    /// change or a wider band doesn't rename someone's strongest tendency.
    pub fn summary(&self) -> String {
        let w = Weights::from(self.percentiles());
        let mut traits = [
            ("radar", w.radar_use),
            ("noob combo", w.noob_combo),
            ("dual wield", w.dual_wield),
            ("pushing", w.aggression),
            ("grenades", w.grenades),
            ("melee", w.melee),
            ("favorite gun", w.weapon_preference),
            ("long range", w.range_preference),
            ("close range", 1.0 - w.range_preference),
            ("strafing", w.strafe),
            ("jumping", w.jump),
            ("quick reactions", 1.0 - w.reaction_jitter),
            ("taking cover", w.retreat),
        ];
        traits.sort_by(|a, b| b.1.total_cmp(&a.1));
        let style = if self.try_hard < 1.0 / 3.0 {
            "Casual"
        } else if self.try_hard < 2.0 / 3.0 {
            "Competitive"
        } else {
            "Sweaty"
        };
        format!("{style}: {}, {}", traits[0].0, traits[1].0)
    }
}

#[cfg(test)]
mod tests;
