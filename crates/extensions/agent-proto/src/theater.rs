//! Film transport and presentation controls. Ticks count from boot.
use serde::{Deserialize, Serialize};

pub const PLAY: &str = "theater.play";
pub const STATE: &str = "theater.state";
pub const PAUSE: &str = "theater.pause";
pub const SPEED: &str = "theater.speed";
pub const SEEK: &str = "theater.seek";
pub const CAMERA: &str = "theater.camera";
pub const KEYFRAMES: &str = "theater.keyframes";
pub const VERIFY: &str = "theater.verify";
pub const METHODS: [&str; 8] = [PLAY, STATE, PAUSE, SPEED, SEEK, CAMERA, KEYFRAMES, VERIFY];

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Play {
    pub id: Option<String>,
    /// Optional clip bounds [start, end) when opening a library film.
    pub range: Option<[u64; 2]>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Speed {
    pub speed: f64,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seek {
    pub tick: u64,
    /// The overlay's in and out ticks, preserved across the seek.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_marks: Option<[Option<u64>; 2]>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CameraMode {
    #[default]
    Default,
    Free,
    Orbit,
    Follow,
    #[serde(alias = "first_person")]
    FirstPerson,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    pub mode: CameraMode,
    #[serde(default)]
    pub seat: Option<u8>,
    #[serde(default)]
    pub player: Option<String>,
    /// Yaw and pitch offsets in radians.
    #[serde(default)]
    pub look: [f32; 2],
    /// Strafe, forward and rise distances in world units.
    #[serde(default)]
    pub movement: [f32; 3],
    /// Change the orbit radius in world units.
    #[serde(default)]
    pub zoom: f32,
}
/// The cheapest recorded state a seek restored or replays from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeekSource {
    Ring,
    Keyframe,
    Sidecar,
    Boot,
}

/// The jump the player is serving, kept after it completes so the restore
/// cost stays visible until the next jump. `restore_ms` is null until the
/// restore finishes, and stays null when no keyframe was restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeekInfo {
    pub target: u64,
    pub from_tick: u64,
    pub source: SeekSource,
    pub warm: bool,
    pub remaining_ticks: u64,
    pub restore_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub id: String,
    pub tick: u64,
    pub length_ticks: u64,
    pub paused: bool,
    pub speed: f64,
    pub seeking: bool,
    pub camera: CameraMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_seat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_player: Option<String>,
    #[serde(default)]
    pub kills: Vec<u64>,
    #[serde(default)]
    pub seek: Option<SeekInfo>,
}

/// Keyframe ticks by storage location and the local cache budgets in bytes.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Keyframes {
    /// Ticks of the film's own machine keyframes, in order.
    pub film: Vec<u64>,
    pub sidecar: Vec<u64>,
    pub ring: Vec<u64>,
    /// Ticks of the film's recorded D3D capture boundaries, in order.
    pub capture_points: Vec<u64>,
    pub ring_bytes: u64,
    pub ring_cap: u64,
    pub sidecar_bytes: u64,
}

/// Tick bounds are inclusive. A missing bound covers that end of the film.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Verify {
    pub from: Option<u64>,
    pub to: Option<u64>,
    pub ring: bool,
}

impl Verify {
    pub fn bounds(self, length: u64) -> Result<[u64; 2], &'static str> {
        let from = self.from.unwrap_or(0);
        let to = self.to.unwrap_or(length);
        if from > to || to > length {
            return Err("verification range is outside the film");
        }
        Ok([from, to])
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Divergence {
    pub tick: u64,
    pub kind: String,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationReport {
    pub ticks: u64,
    pub digests_checked: u64,
    pub arena_hashes_checked: u64,
    pub ram_hashes_checked: u64,
    pub first_divergence: Option<Divergence>,
}

#[cfg(test)]
mod verify_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn verification_request_and_report_round_trip() {
        for (wire, expected) in [
            (json!({}), [0, 60]),
            (json!({"from": 30, "to": 30, "ring": true}), [30, 30]),
            (json!({"from": 10}), [10, 60]),
        ] {
            let request: Verify = serde_json::from_value(wire).unwrap();
            assert_eq!(request.bounds(60), Ok(expected));
            assert_eq!(serde_json::from_value::<Verify>(serde_json::to_value(request).unwrap()).unwrap(), request);
        }
        for wire in [json!({"from": 2, "to": 1}), json!({"to": 61})] {
            assert!(serde_json::from_value::<Verify>(wire).unwrap().bounds(60).is_err());
        }
        assert!(serde_json::from_value::<Verify>(json!({"ring": 1})).is_err());
        assert!(serde_json::from_value::<Verify>(json!({"target": 2})).is_err());
        let report = VerificationReport {
            first_divergence: Some(Divergence { tick: 4, kind: "RAM differs".into() }),
            ..Default::default()
        };
        assert_eq!(serde_json::from_slice::<VerificationReport>(&serde_json::to_vec(&report).unwrap()).unwrap(), report);
    }
}
