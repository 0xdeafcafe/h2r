//! `sim.hashes`: the running game's post-tick digests, the rows sim mode
//! writes to its report as `state_hashes_v1` (docs/runtime/sim.md §6), so
//! a rendered run can be compared tick by tick with another
//! (docs/graphics/framescene.md, section 9.3).

use serde::{Deserialize, Serialize};

/// `sim.hashes`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HashesParams {
    /// Only rows from this tick on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_tick: Option<u32>,
    /// At most this many rows (the first ones from `from_tick`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
}

/// One post-tick digest: the sim report's `TickHash`, field for field.
/// Hex strings keep all 64 bits in JSON clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashRow {
    pub tick: u32,
    pub gameplay: String,
    /// The game-state arena's hash, under `impl.sim.arena-hash` when the
    /// arena was readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arena: Option<String>,
}

/// `sim.hashes`' answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashesReply {
    /// In tick order.
    pub rows: Vec<HashRow>,
    /// Rows recorded since the last reset that are no longer kept (the
    /// oldest go first past [`HashesReply::KEEP`]).
    pub dropped: u64,
    /// Times the series started again: the tick went back (a new map, a
    /// snapshot load) or jumped, as the sim report's tally does.
    pub resets: u64,
    /// More rows than `max` matched.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl HashesReply {
    /// Rows kept: an hour of ticks at 30 a second (about 2.6 MB).
    pub const KEEP: usize = 30 * 60 * 60;
}
