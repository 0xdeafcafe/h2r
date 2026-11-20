//! The menu summary shared by a sealed engine film and its Live listing.
pub mod events;
pub mod machine_index;
pub mod range;
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
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlayerSummary {
    pub seat: u8,
    pub name: String,
    #[serde(default, with = "optional_xuid")]
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
