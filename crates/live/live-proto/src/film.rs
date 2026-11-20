//! The menu summary shared by a sealed engine film and its Live listing.
pub mod events;
pub mod machine_index;
pub mod range;
pub mod privacy;
pub mod stream;
pub mod objects;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const MAX_SUMMARY_BYTES: usize = 128 << 10;
/// A single request stays within either Live home's request-body limit.
pub const SINGLE_UPLOAD_BYTES: u64 = 64 << 20;
/// Larger films stage fixed-size parts, then publish one verified object.
pub const PART_BYTES: u64 = 16 << 20;
pub const MAX_UPLOAD_BYTES: u64 = 2 << 30;

/// Observer chains installed on the guest dispatch table. Adding or moving
/// hooks requires a new variant so playback can reproduce the original plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryObservers {
    Off,
    EventsV1,
    EventsAndImpactsV1,
}

/// The slim host image can omit individual generated functions. The replay
/// uses exactly the selected optional points even on a full desktop image.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelemetryHooks {
    pub observers: TelemetryObservers,
    pub weapon_fire: bool,
    pub damage_report: bool,
    pub object_delete: bool,
}
impl TelemetryHooks {
    pub const OFF: Self = Self {
        observers: TelemetryObservers::Off,
        weapon_fire: false,
        damage_report: false,
        object_delete: false,
    };
    pub const EVENTS: Self = Self {
        observers: TelemetryObservers::EventsV1,
        weapon_fire: true,
        damage_report: true,
        object_delete: false,
    };
    pub const UNIFIED: Self = Self {
        observers: TelemetryObservers::EventsAndImpactsV1,
        weapon_fire: true,
        damage_report: true,
        object_delete: true,
    };
    pub fn enabled(self) -> bool {
        self.observers != TelemetryObservers::Off
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Unknown,
    Custom,
    Matchmaking,
    SystemLink,
    CoopCampaign,
    Campaign,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub level: String,
    pub game_type: String,
    pub mode: Mode,
    pub variant_name: String,
    pub duration_ticks: u64,
    pub players: Vec<PlayerSummary>,
    /// Optional observations that were unreadable or could not be represented.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dropped_samples: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlayerSummary {
    pub seat: u8,
    pub name: String,
    #[serde(default, with = "optional_xuid", skip_serializing_if = "Option::is_none")]
    pub xuid: Option<u64>,
    pub team: i32,
    pub score: i64,
    pub medals: BTreeMap<String, u32>,
}

mod optional_xuid {
    use super::*;
    use crate::Xuid;

    pub fn serialize<S: serde::Serializer>(id: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        id.map(Xuid).serialize(s)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        Ok(Option::<Xuid>::deserialize(d)?.map(|id| id.0))
    }
}

/// First disagreement with the immutable game ledger. Preserve that ledger
/// for player totals/ranks. The film retains its own final results and events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsDiscrepancy {
    pub cause: DiscrepancyCause,
    // Zero is GameStarted, one is the first subsequent canonical event.
    pub event_index: usize,
    pub film_tick: Option<u64>,
    pub telemetry_position: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscrepancyCause {
    MissingStart,
    MissingEnd,
    IncompleteObservations,
    InvalidEventSequence,
    DifferentStart,
    DifferentEvent,
    IdleTimeout,
    TelemetryExtendsPastFilm,
    TelemetryEndedEarly,
}
