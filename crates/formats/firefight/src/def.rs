use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Limits, Refusal, Result, Sha256, validate, wire};

pub const FORMAT: u32 = 3;
pub const MAX_DOC_BYTES: usize = 64 * 1024;
pub const MAX_PLAYERS: u8 = 16;
pub const MULTIPLIER_SCALE: u16 = 1_000;
/// Player teams a definition may have: one co-operative team, or two that
/// also fight each other.
pub const MAX_TEAMS: usize = 2;

pub(crate) const MAX_ROUNDS: usize = 12;
pub(crate) const WAVES_PER_ROUND: usize = 5;
pub(crate) const MAX_SQUADS: usize = 16;
pub(crate) const MAX_CLASSES: usize = 32;
pub(crate) const MAX_SKULLS: usize = 9;
/// Squads in a round's `allies`: one for each of four actors.
pub(crate) const MAX_ALLY_SQUADS: usize = 4;
/// Allies alive at once, at any player count; they count toward the live
/// actor cap besides.
pub const MAX_ALLIES: u16 = 4;
/// Rounds in a set that repeats: Firefight's own count.
pub const SET_ROUNDS: usize = 3;
/// Set skulls a definition lists: one for each of sets 2 to 4.
pub(crate) const MAX_SET_SKULLS: usize = 3;
/// The most a set may scale its squads by, in thousandths per set.
pub const MAX_SET_SCALE: u16 = 500;
/// Rows of a bonus round's points-to-lives table.
pub(crate) const MAX_BONUS_ROWS: usize = 6;
/// The longest bonus round: five minutes.
pub(crate) const MAX_BONUS_TICKS: u32 = 5 * 60 * 30;
/// The skulls (never Iron) the bonus round switches on for its
/// length: the ODST skull order's slots, each in `script.rs`'s skull list.
/// Iron is on in the bonus round besides ([`Skull::Iron`]).
pub const BONUS_SKULLS: [Skull; 6] = [
    Skull::Ghost,
    Skull::Catch,
    Skull::Thunderstorm,
    Skull::Famine,
    Skull::BlackEye,
    Skull::Mythic,
];
const MAX_CAPS: usize = MAX_PLAYERS as usize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Def {
    pub format: u32,
    pub name: String,
    /// Player teams: 1 (everyone against the AI) or 2 (two teams, each
    /// against the AI and the other team).
    pub teams: u8,
    #[serde(deserialize_with = "wire::bounded_seq::<_, _, MAX_ROUNDS>")]
    pub rounds: Vec<Round>,
    /// What happens after the last round: the game ends, or the next set
    /// begins.
    pub set: SetRules,
    pub lives: Lives,
    pub caps: Caps,
    pub scoring: Scoring,
    /// The round after each set: absent, none is played.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bonus: Option<Bonus>,
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

/// Sets: `rounds` describes one set. A finite definition plays it once and
/// ends `Won`; an endless one plays it again and again until the team is
/// out, and ends only `Lost`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetRules {
    /// The next set begins when the last round's last wave is cleared. Needs
    /// one team and [`SET_ROUNDS`] rounds.
    pub endless: bool,
    /// Squad counts grow by this many thousandths for each set played
    /// (set 3 is 2 x `scale` over set 1), capped by the actor cap.
    pub scale: u16,
    /// Skulls that arrive at the start of sets 2, 3 and 4, in that order, and
    /// stay on. None arrives after the list ends. Round skulls (`skulls_on`)
    /// switch off when a new set begins; these do not.
    #[serde(deserialize_with = "wire::bounded_seq::<_, _, MAX_SET_SKULLS>")]
    pub skulls: Vec<Skull>,
}

impl SetRules {
    /// One set, played once: what a finite definition says.
    pub fn finite() -> Self {
        Self {
            endless: false,
            scale: 0,
            skulls: Vec::new(),
        }
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
    /// Allied squads that arrive when the round starts, and again after a
    /// boss wave for any that fell. Each is [`Side::Ally`]. With two teams,
    /// the first belongs to team 0 and the second to team 1.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "wire::bounded_seq::<_, _, MAX_ALLY_SQUADS>"
    )]
    pub allies: Vec<SquadSpec>,
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
    /// Whom the squad fights for. A wave's squads are enemies; a round's
    /// `allies` are allies.
    #[serde(default, skip_serializing_if = "Side::is_enemy")]
    pub side: Side,
    /// How the squad reaches its zone.
    #[serde(default, skip_serializing_if = "Arrival::is_auto")]
    pub arrival: Arrival,
}

/// How a squad reaches its zone (docs/game/firefight.md, "The drop ship"
/// and "Drop pods"). Whatever the kit or the zone cannot do becomes the
/// next arrival on this list: pod, then ship, then foot, so a wave never
/// waits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arrival {
    /// By faction: a Covenant squad by ship, a Flood or human one by pod.
    #[default]
    Auto,
    /// Placed standing at the zone.
    Foot,
    Ship,
    Pod,
}

impl Arrival {
    pub(crate) fn is_auto(&self) -> bool {
        *self == Self::Auto
    }

    /// The rules table's number for it.
    pub const fn number(self) -> i32 {
        match self {
            Self::Auto => 0,
            Self::Foot => 1,
            Self::Ship => 2,
            Self::Pod => 3,
        }
    }
}

/// Which side of the players a squad is on. An ally takes the team of the
/// players it serves, is not counted for a wave's clear, scores nothing
/// and is not scored for killing (docs/game/firefight.md, "Allies").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    #[default]
    Enemy,
    Ally,
}

impl Side {
    pub(crate) fn is_enemy(&self) -> bool {
        *self == Self::Enemy
    }
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
    /// Pick among the kit zones that carry this role, unseen or farther
    /// half first as for `Edge`.
    Role(ZoneRole),
}

/// Who a class fights for. The kit writes it on the class's record squad
/// as that faction's team, the game reads it back, and a squad is placed
/// on that team, so each faction is hostile to every other by the engine's
/// own "teams differ" test (docs/game/firefight.md, "Factions").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Faction {
    Covenant,
    Flood,
    Human,
}

/// The engine's own team numbers (docs/halo1/ai.md, **S**).
pub mod teams {
    /// Enemy humans (allied humans take the player team they serve).
    pub const HUMAN: u8 = 2;
    pub const COVENANT: u8 = 3;
    pub const FLOOD: u8 = 4;
}

impl Faction {
    pub const ALL: [Self; 3] = [Self::Covenant, Self::Flood, Self::Human];

    /// The team the faction's squads are placed on.
    pub const fn team(self) -> u8 {
        match self {
            Self::Covenant => teams::COVENANT,
            Self::Flood => teams::FLOOD,
            Self::Human => teams::HUMAN,
        }
    }

    /// The faction a team number is, if it is one.
    pub const fn from_team(team: u8) -> Option<Self> {
        match team {
            teams::COVENANT => Some(Self::Covenant),
            teams::FLOOD => Some(Self::Flood),
            teams::HUMAN => Some(Self::Human),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Covenant => "covenant",
            Self::Flood => "flood",
            Self::Human => "human",
        }
    }
}

/// What a kit zone is for: the roles it carries, written with the zone
/// (`ZoneSeen::roles`). Stage-one `line` squads are Elite tier 0-1,
/// `flank` tier 1-2, `heavy` tier 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneRole {
    Line,
    Flank,
    Heavy,
}

impl ZoneRole {
    /// The bit of `ZoneSeen::roles` and of the script's role column.
    pub const fn bit(self) -> u8 {
        match self {
            Self::Line => 1,
            Self::Flank => 2,
            Self::Heavy => 4,
        }
    }
}

/// Every role bit: a zone of an auto-built kit carries all of them.
pub const ALL_ZONE_ROLES: u8 = 0b111;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lives {
    pub pool_start: u16,
    pub per_round_bonus: u16,
    /// One pool per team; otherwise each player has their own count.
    pub shared: bool,
    /// At zero lives a dead player waits for the next grant (a round's
    /// end, the bonus round's end), not for the wave's end.
    #[serde(default, skip_serializing_if = "is_false")]
    pub zero_holds: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The bonus round that follows each set: a timed stretch of one weak
/// squad without end, every skull on, no life lost and no respawn in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bonus {
    /// How long it runs, in ticks.
    pub ticks: u32,
    /// The squad the reserve refills with until the time is up. Its
    /// `drop` and `weapon_tier` are read as in a wave.
    pub squad: SquadSpec,
    /// Ascending `(points, lives)`: a team that scores at least `points`
    /// in the bonus round is granted the `lives` of the last row it
    /// reaches (not the sum of the rows).
    #[serde(deserialize_with = "wire::bounded_seq::<_, _, MAX_BONUS_ROWS>")]
    pub lives_by_points: Vec<(u32, u16)>,
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
    Ghost = 7,
    BlackEye = 8,
    Catch = 9,
    /// Bonus round only: no respawn while it runs. A director rule; the
    /// engine's skull of this number is never set.
    Iron = 11,
    Mythic = 12,
    Angry = 13,
    LowGravity = 16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scoring {
    #[serde(deserialize_with = "wire::bounded_map::<_, _, _, MAX_CLASSES>")]
    pub by_class: BTreeMap<String, u16>,
    /// Points for killing a player of the other team, with two teams. Not
    /// scaled by difficulty or skulls. Zero with one team.
    pub player_kill: u16,
    /// Fixed-point multipliers, in thousandths.
    pub difficulty_multiplier: DifficultyMultipliers,
    /// Fixed-point multipliers, in thousandths. Omitted skulls use 1,000.
    #[serde(deserialize_with = "wire::bounded_map::<_, _, _, MAX_SKULLS>")]
    pub skull_multipliers: BTreeMap<Skull, u16>,
    /// Fraction of a kill's points credited to an assist, in thousandths.
    pub assist: u16,
}
