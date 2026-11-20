//! Capture evidence for the Live mod galleries (docs/live/live-api.md).

use content::{MapId, Sha256};
use serde::{Deserialize, Serialize};

pub const PROOF_HEADER: &str = "x-h2-capture-proof";
pub const SCREENSHOT_REVIEWED: &str = "ModScreenshotReviewed";
pub const SCREENSHOT_REPORTED: &str = "ModScreenshotReported";

/// A private, single-account capture session issued for an installed map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturePermit {
    pub game_id: String,
    pub token: String,
}

/// World-space camera basis in game coordinates (Z up).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraPose {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
}

/// Evidence attached to the exact image bytes the game returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureProof {
    pub game_id: String,
    pub tick: u64,
    pub map_id: MapId,
    pub map: String,
    pub camera: CameraPose,
    pub digest: Sha256,
    /// Required for the private capture workflow. Live games have a roster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permit: Option<String>,
}
