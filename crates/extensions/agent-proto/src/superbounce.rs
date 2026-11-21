//! `superbounce.*`: superbounce and forced-crouch spots found in a map's
//! collision geometry (docs/runtime/agent-api.md, "superbounce"; the
//! scan is `crates/live/superbounce`, docs/game/superbounce.md). Wire
//! types only.

use serde::{Deserialize, Serialize};

// ---- superbounce.spots ----

/// `superbounce.spots`: the spots the bots know for a map. A read.
pub const SPOTS: &str = "superbounce.spots";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotsParams {
    /// The map's short name (`ascension`); default the loaded map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// Only `bounce` or `crouch` spots; default both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// One spot. Coordinates are world units, Z up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotRecord {
    /// `bounce`: a slot a primed landing launches out of. `crouch`: a
    /// place a standing player is forced into a crouch, which primes.
    pub kind: String,
    /// `measured` (a game run bounced from it) or `scan` (found in the
    /// collision geometry).
    pub source: String,
    /// The line to draw.
    pub edge: [[f32; 3]; 2],
    /// Where a player stands to use it: a unit's origin, standing.
    pub stand: [f32; 3],
    /// Bounce: how far to fall onto the slot (0 for a standing jump).
    pub fall: f32,
    /// Bounce: the launch speed straight up (world units a second).
    pub launch: f32,
    /// A world direction (unit, X and Y) to drift along when landing, or
    /// zero.
    pub drift: [f32; 2],
    /// 0..1: how sure the scan is; 1 for a measured spot.
    pub score: f32,
}

/// The result of `superbounce.spots`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spots {
    /// The map the spots are for.
    pub map: String,
    /// The map has a scan (the nav worker has made or read one); the
    /// measured spots are listed either way.
    pub scanned: bool,
    /// Measured spots first, then scanned, best first.
    pub spots: Vec<SpotRecord>,
}
