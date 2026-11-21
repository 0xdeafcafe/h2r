use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Limits, Refusal, Result, Sha256, validate, wire};

pub const FORMAT: u32 = 1;
pub const MAX_DOC_BYTES: usize = 64 * 1024;
pub const MAX_PLAYERS: u8 = 16;
pub const MULTIPLIER_SCALE: u16 = 1_000;

pub(crate) const MAX_ROUNDS: usize = 12;
pub(crate) const WAVES_PER_ROUND: usize = 5;
pub(crate) const MAX_SQUADS: usize = 16;
pub(crate) const MAX_CLASSES: usize = 32;
pub(crate) const MAX_SKULLS: usize = 9;
const MAX_CAPS: usize = MAX_PLAYERS as usize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Def {
    pub format: u32,
    pub name: String,
    #[serde(deserialize_with = "wire::bounded_seq::<_, _, MAX_ROUNDS>")]
    pub rounds: Vec<Round>,
    pub lives: Lives,
    pub caps: Caps,
    pub scoring: Scoring,
}

impl Def {
    /// Decode untrusted JSON, enforce the wire limits and admit it against the kit.
    pub fn from_bytes(bytes: &[u8], limits: &Limits) -> Result<Self> {
        wire::check_size(bytes.len(), "document bytes", MAX_DOC_BYTES)?;
        let def = serde_json::from_slice(bytes).map_err(wire::json_error)?;
        validate(&def, limits)?;
        Ok(def)
    }

    /// Canonical JSON of a structurally valid definition, independent of a kit.
    pub fn canonical_json(&self) -> Result<String> {
        crate::validate::definition(self)?;
        wire::measure(self, MAX_DOC_BYTES)?;
        let value = serde_json::to_value(self).map_err(wire::json_error)?;
        let json =
            content::canonicalize(&value).map_err(|error| Refusal::Json(error.to_string()))?;
        wire::check_size(json.len(), "document bytes", MAX_DOC_BYTES)?;
        Ok(json)
    }

    /// SHA-256 of the entire canonical definition, including its version.
    pub fn id(&self) -> Result<Sha256> {
        Ok(Sha256::of(self.canonical_json()?.as_bytes()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Round {
    #[serde(deserialize_with = "wire::bounded_seq::<_, _, WAVES_PER_ROUND>")]
    pub waves: Vec<Wave>,
    /// Skulls enabled at the start of this round. Earlier skulls stay on.
    #[serde(deserialize_with = "wire::bounded_seq::<_, _, MAX_SKULLS>")]
    pub skulls_on: Vec<Skull>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wave {
    #[serde(deserialize_with = "wire::bounded_seq::<_, _, MAX_SQUADS>")]
    pub squads: Vec<SquadSpec>,
    pub boss: bool,
    pub gap_ticks: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SquadSpec {
    pub class: String,
    pub count: Count,
    pub drop: Drop,
    pub weapon_tier: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Count {
    pub per_player: u8,
    pub min: u16,
    pub max: u16,
}

impl Count {
    /// Scale by the number of players and apply the authored bounds.
    pub fn for_players(self, players: u8) -> u16 {
        (u16::from(self.per_player) * u16::from(players))
            .max(self.min)
            .min(self.max)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Drop {
    /// Pick among the kit's edge zones.
    Edge,
    /// Prefer this zero-based kit zone if available and unseen.
    PreferZone(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lives {
    pub pool_start: u16,
    pub per_round_bonus: u16,
    pub shared: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caps {
    /// Ascending inclusive upper bounds on player count, with one actor cap each.
    #[serde(deserialize_with = "wire::bounded_seq::<_, _, MAX_CAPS>")]
    pub live_actors_by_players: Vec<(u8, u16)>,
}

impl Caps {
    /// The cap for a supported player count in a validated definition.
    pub fn for_players(&self, players: u8) -> Option<u16> {
        if players == 0 {
            return None;
        }
        self.live_actors_by_players
            .iter()
            .find(|&&(upper, _)| players <= upper)
            .map(|&(_, actors)| actors)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Easy,
    Normal,
    Heroic,
    Legendary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DifficultyMultipliers {
    pub easy: u16,
    pub normal: u16,
    pub heroic: u16,
    pub legendary: u16,
}

impl DifficultyMultipliers {
    pub fn get(self, difficulty: Difficulty) -> u16 {
        match difficulty {
            Difficulty::Easy => self.easy,
            Difficulty::Normal => self.normal,
            Difficulty::Heroic => self.heroic,
            Difficulty::Legendary => self.legendary,
        }
    }
}

/// Skulls with Firefight effects. Discriminants match the session skull indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum Skull {
    Assassins = 2,
    Thunderstorm = 3,
    Famine = 4,
    Blind = 6,
    Ghost = 7,
    Catch = 9,
    Mythic = 12,
    Angry = 13,
    LowGravity = 16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scoring {
    #[serde(deserialize_with = "wire::bounded_map::<_, _, _, MAX_CLASSES>")]
    pub by_class: BTreeMap<String, u16>,
    /// Fixed-point multipliers, in thousandths.
    pub difficulty_multiplier: DifficultyMultipliers,
    /// Fixed-point multipliers, in thousandths. Omitted skulls use 1,000.
    #[serde(deserialize_with = "wire::bounded_map::<_, _, _, MAX_SKULLS>")]
    pub skull_multipliers: BTreeMap<Skull, u16>,
    /// Fraction of a kill's points credited to an assist, in thousandths.
    pub assist: u16,
}
