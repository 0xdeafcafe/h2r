//! The authoritative, tick-addressable part of a host or local film.
mod ticks;
mod track;
mod unified;

pub use live_proto::film::stream;
pub use live_proto::film::{Mode, PlayerSummary, Summary};
pub use track::{KeyframeInfo, KeyframeSource, TrackReader, TrackWriter, summary};
pub(crate) use track::{decode_keyframe, encode_keyframe, split_keyframe};
pub use unified::RecordData;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const DESC: crate::Tag = *b"DESC";
pub const INPT: crate::Tag = *b"INPT";
pub const SUMM: crate::Tag = *b"SUMM";
pub const TIDX: crate::Tag = *b"TIDX";
pub const MAX_SEATS: usize = 16;

/// The identity of the bytes actually mounted by the recording machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapIdentity {
    pub name: String,
    /// Origin of the mounted scenario, e.g. retail, converted_halo1 or mod.
    pub origin: String,
    pub sha256: String,
    #[serde(default)]
    pub package_id: Option<String>,
    /// Present for a sparse map store: (logical offset, byte count) spans
    /// covered by the domain-separated digest, in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranges: Option<Vec<(u64, u64)>>,
    /// Guest cache-slot source (validated DVD / installed map path).
    #[serde(default)]
    pub cache_path: Option<String>,
}

/// Match settings resolved by the game. Machine identity, start time and HLE
/// settings live in the outer HEAD and cover the boot before a match exists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchHeader {
    /// Completed film tick at which the active map and variant were observed.
    #[serde(default)]
    pub match_start_tick: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks_per_second: Option<u16>,
    pub map: MapIdentity,
    pub variant_blob: Vec<u8>,
    pub game_type: String,
    pub mode: Mode,
    pub difficulty: Option<String>,
    pub skulls: Vec<String>,
    pub seed: Option<u64>,
    pub roster: Vec<Seat>,
    pub host_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatKind {
    #[default]
    Local,
    Remote,
    Phantom,
    Bot,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seat {
    pub seat: u8,
    pub name: String,
    pub xuid: Option<u64>,
    pub team: i32,
    pub kind: SeatKind,
    pub bot_params: Option<serde_json::Value>,
}

/// Events are ordered as observed during the tick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Join { player: Seat },
    Leave { seat: u8 },
    Quit { seat: u8 },
    Team { seat: u8, team: i32 },
    HostMigration { #[serde(default)] host_id: String },
    Pause { paused: bool },
}

/// Little-endian XINPUT_GAMEPAD and/or a retail player_action (0x5c
/// bytes). The seat number is the absolute player slot on the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeatInput {
    pub seat: u8,
    pub pad: Option<[u8; 18]>,
    pub action: Option<[u8; 92]>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// Completed tick count since boot. The first tick is 1.
    pub tick: u64,
    pub inputs: Vec<SeatInput>,
    pub events: Vec<Event>,
    /// Observed after the last completed tick, e.g. a quit on the scoreboard.
    pub closing_events: Vec<Event>,
    pub post_hash: Option<u64>,
}

impl Tick {
    pub fn verify(&self, actual: Option<u64>) -> Result<()> {
        if let Some(expected) = self.post_hash {
            ensure!(
                actual == Some(expected),
                "first divergence at tick {}: expected {expected:016x}, observed {actual:?}",
                self.tick
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyframe {
    /// Last completed tick in this machine state. Step resumes at tick + 1.
    pub tick: u64,
    pub slices: u64,
    pub calls: u64,
    pub lobby_boundary: u64,
    /// JSON changes to the boot disk manifest up to this boundary (empty when unchanged).
    pub disk_delta: Vec<u8>,
    /// Machine snapshot including the build key. A RAM delta refers to its
    /// standalone anchor by the snapshot name `film-{tick}`.
    pub snapshot: Vec<u8>,
    /// Standalone H2SNAP bytes needed to decode this snapshot's RAM section.
    /// All other sections and the guest disk are relative to boot or complete.
    pub ram_anchor: Option<(u64, Vec<u8>)>,
}

#[cfg(test)]
mod tests;
