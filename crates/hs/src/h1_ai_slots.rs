//! Converted Halo 1 AI calls on the retail Halo 2 interpreter.
//!
//! Only a named slot and a marked function-name atom opt a call into this
//! ABI. Halo 2 never evaluates that atom. Its data word selects the
//! operation, which supplies the real argument types to the native
//! fixed-signature evaluator. The retail descriptor still supplies the
//! result type. See docs/halo1/ai-control.md.

use crate::casts::types::{AI, BOOLEAN, LONG, OBJECT, OBJECT_LIST, REAL, SHORT, STRING, VOID};

pub const VEHICLE_COUNT: u16 = 241;
pub const CONTROL: u16 = 300;
pub const SEE_OBJECT: u16 = 305;
const UNIT: u16 = 51;
const TEAM: u16 = 45;
pub const ACTOR_TYPE: u16 = 46;

pub const VEHICLE_ROWS: u16 = 96;
pub const VEHICLE_WIDTH: u16 = 11;
pub const FOLLOW_WORDS: u16 = 300;
pub const MIGRATION_WORDS: u16 = 12;
pub const DEFAULT_FOLLOW_RADIUS: f32 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registry {
    Vehicles,
    ActivationLinks,
    Playfight,
    TargetPreference,
    Follow,
}

impl Registry {
    pub const fn words(self) -> u16 {
        match self {
            Self::Vehicles => VEHICLE_ROWS * VEHICLE_WIDTH,
            Self::ActivationLinks => 128,
            Self::Playfight => 4,
            Self::TargetPreference => 512,
            Self::Follow => FOLLOW_WORDS + MIGRATION_WORDS * 2,
        }
    }

    pub const fn initial(self, index: u16) -> i32 {
        match self {
            Self::Vehicles if matches!(index % VEHICLE_WIDTH, 2..=4) => 0,
            Self::Playfight => 0,
            Self::Follow if index >= FOLLOW_WORDS || index % 3 == 0 => 0,
            Self::Follow if index % 3 == 2 => DEFAULT_FOLLOW_RADIUS.to_bits() as i32,
            _ => -1,
        }
    }

    pub const fn hook_shift(self) -> Option<u32> {
        match self {
            Self::Playfight => Some(0),
            Self::TargetPreference => Some(16),
            _ => None,
        }
    }

    pub const fn tick(self) -> Option<Operation> {
        match self {
            Self::Vehicles => Some(Operation::VehicleTick),
            Self::ActivationLinks => Some(Operation::ActivationTick),
            Self::Follow => Some(Operation::FollowTick),
            Self::Playfight | Self::TargetPreference => None,
        }
    }
}

/// The final global packs the indices of data needed by native hooks.
pub const HOOK_STATE: &str = "h1_ai_hook_state";

pub const CONTROL_TOKEN: &str = "h1_ai_control";
pub const DISCARD_EVALUATOR: u32 = 0x002a_b6c0;
pub const VEHICLE_COUNT_EVALUATOR: u32 = 0x002a_4310;
pub const SEE_OBJECT_EVALUATOR: u32 = 0x002a_53b0;

/// Every retail builtin slot taken over conditionally by the AI lane.
/// Keeping these together lets the converter and the runtime check the
/// same index, evaluator and result type.
pub const CLAIMS: &[Claim] = &[
    Claim {
        slot: VEHICLE_COUNT,
        evaluator: VEHICLE_COUNT_EVALUATOR,
        result: SHORT,
        retail_parameters: &[UNIT],
    },
    Claim {
        slot: CONTROL,
        evaluator: DISCARD_EVALUATOR,
        result: VOID,
        retail_parameters: &[AI],
    },
    Claim {
        slot: SEE_OBJECT,
        evaluator: SEE_OBJECT_EVALUATOR,
        result: VOID,
        retail_parameters: &[AI, OBJECT],
    },
];

pub struct Claim {
    pub slot: u16,
    pub evaluator: u32,
    pub result: u16,
    pub retail_parameters: &'static [u16],
}

impl Claim {
    pub fn matches(&self, builtin: &crate::Builtin) -> bool {
        builtin.evaluate() == self.evaluator
            && builtin.return_type() == self.result
            && builtin.parameters() == self.retail_parameters
            && builtin.flags() & 2 == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Operation {
    SeePlayers = 1,
    Free = 2,
    FreeUnits = 3,
    MigrateUnits = 4,
    Teleport = 5,
    TeleportUnsupported = 6,
    Maneuver = 7,
    VehicleEncounter = 8,
    VehicleDistance = 9,
    VehicleActorType = 10,
    VehicleTeam = 11,
    VehicleActors = 12,
    VehicleDisable = 13,
    VehicleTick = 14,
    VehicleCount = 15,
    AttachFree = 16,
    TimerExpire = 17,
    LinkActivation = 18,
    ActivationTick = 19,
    Playfight = 20,
    PreferAi = 21,
    PreferPlayer = 22,
    ClearPreference = 23,
    MigrateSpeak = 24,
    FollowPlayers = 25,
    FollowAi = 26,
    FollowUnit = 27,
    FollowDisable = 28,
    FollowDistance = 29,
    FollowTick = 30,
    AutomaticMigrationTarget = 31,
}

impl Operation {
    const MARKER: u32 = 0x4831_0000; // H1 in the high word

    pub const fn marker(self) -> u32 {
        Self::MARKER | self as u32
    }

    pub fn from_marker(word: u32) -> Option<Self> {
        Some(match word.checked_sub(Self::MARKER)? {
            1 => Self::SeePlayers,
            2 => Self::Free,
            3 => Self::FreeUnits,
            4 => Self::MigrateUnits,
            5 => Self::Teleport,
            6 => Self::TeleportUnsupported,
            7 => Self::Maneuver,
            8 => Self::VehicleEncounter,
            9 => Self::VehicleDistance,
            10 => Self::VehicleActorType,
            11 => Self::VehicleTeam,
            12 => Self::VehicleActors,
            13 => Self::VehicleDisable,
            14 => Self::VehicleTick,
            15 => Self::VehicleCount,
            16 => Self::AttachFree,
            17 => Self::TimerExpire,
            18 => Self::LinkActivation,
            19 => Self::ActivationTick,
            20 => Self::Playfight,
            21 => Self::PreferAi,
            22 => Self::PreferPlayer,
            23 => Self::ClearPreference,
            24 => Self::MigrateSpeak,
            25 => Self::FollowPlayers,
            26 => Self::FollowAi,
            27 => Self::FollowUnit,
            28 => Self::FollowDisable,
            29 => Self::FollowDistance,
            30 => Self::FollowTick,
            31 => Self::AutomaticMigrationTarget,
            _ => return None,
        })
    }

    pub const fn slot(self) -> u16 {
        match self {
            Self::VehicleCount => VEHICLE_COUNT,
            _ => CONTROL,
        }
    }

    /// Setters take an additional index of the registry in the script globals.
    pub const fn registry(self) -> Option<Registry> {
        match self {
            Self::VehicleEncounter
            | Self::VehicleDistance
            | Self::VehicleActorType
            | Self::VehicleTeam
            | Self::VehicleActors
            | Self::VehicleDisable => Some(Registry::Vehicles),
            Self::LinkActivation => Some(Registry::ActivationLinks),
            Self::FollowPlayers
            | Self::FollowAi
            | Self::FollowUnit
            | Self::FollowDisable
            | Self::FollowDistance
            | Self::AutomaticMigrationTarget => Some(Registry::Follow),
            Self::Playfight => Some(Registry::Playfight),
            Self::PreferAi | Self::PreferPlayer | Self::ClearPreference => {
                Some(Registry::TargetPreference)
            }
            _ => None,
        }
    }

    pub const fn parameters(self) -> &'static [u16] {
        match self {
            Self::SeePlayers | Self::Free | Self::Teleport | Self::TeleportUnsupported => &[AI],
            Self::FreeUnits => &[OBJECT_LIST],
            Self::MigrateUnits => &[OBJECT_LIST, AI],
            Self::MigrateSpeak => &[AI, AI, STRING],
            Self::Maneuver => &[STRING],
            Self::VehicleEncounter | Self::VehicleActors => &[UNIT, AI, LONG],
            Self::VehicleDistance => &[UNIT, REAL, LONG],
            Self::VehicleActorType => &[UNIT, ACTOR_TYPE, LONG],
            Self::VehicleTeam => &[UNIT, TEAM, LONG],
            Self::VehicleDisable => &[UNIT, LONG],
            Self::VehicleTick => &[LONG],
            Self::VehicleCount => &[UNIT],
            Self::AttachFree => &[UNIT, LONG],
            Self::TimerExpire => &[AI, BOOLEAN],
            Self::LinkActivation => &[AI, AI, LONG],
            Self::ActivationTick => &[LONG],
            Self::Playfight => &[AI, BOOLEAN, LONG],
            Self::PreferAi => &[AI, AI, LONG],
            Self::PreferPlayer
            | Self::ClearPreference
            | Self::FollowPlayers
            | Self::FollowDisable => &[AI, LONG],
            Self::FollowAi => &[AI, AI, LONG],
            Self::FollowUnit => &[AI, UNIT, LONG],
            Self::FollowDistance => &[AI, REAL, LONG],
            Self::FollowTick => &[LONG],
            Self::AutomaticMigrationTarget => &[AI, BOOLEAN, LONG],
        }
    }
}

/// One simultaneous maneuver edge. The optional global is the script
/// switch of its original Halo 1 platoon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManeuverEdge {
    pub from: u16,
    pub to: u16,
    pub enabled_by: Option<u16>,
}

impl ManeuverEdge {
    pub const WIDTH: usize = 12;

    pub fn append_to(self, routes: &mut String) {
        use std::fmt::Write;
        let switch = self.enabled_by.unwrap_or(u16::MAX);
        write!(routes, "{:04x}{:04x}{switch:04x}", self.from, self.to).expect("write to string");
    }

    pub fn decode(record: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(record).ok()?;
        let from = u16::from_str_radix(text.get(..4)?, 16).ok()?;
        let to = u16::from_str_radix(text.get(4..8)?, 16).ok()?;
        let switch = u16::from_str_radix(text.get(8..Self::WIDTH)?, 16).ok()?;
        Some(Self {
            from,
            to,
            enabled_by: (switch != u16::MAX).then_some(switch),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_claims_are_distinct_and_markers_are_exact() {
        assert!(CLAIMS.windows(2).all(|pair| pair[0].slot < pair[1].slot));
        assert_eq!(
            Operation::from_marker(Operation::Free.marker()),
            Some(Operation::Free)
        );
        assert_eq!(Operation::from_marker(0), None);
        assert_eq!(
            Operation::from_marker(Operation::Free.marker() | 0x100),
            None
        );
    }
}
