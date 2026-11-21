//! Film transport and presentation controls. Ticks count from boot.
use serde::{Deserialize, Serialize};

pub const PLAY: &str = "theater.play";
pub const PREPARE: &str = "theater.prepare";
pub const STATE: &str = "theater.state";
pub const PAUSE: &str = "theater.pause";
pub const SPEED: &str = "theater.speed";
pub const SEEK: &str = "theater.seek";
pub const CAMERA: &str = "theater.camera";
pub const METHODS: [&str; 6] = [PLAY, STATE, PAUSE, SPEED, SEEK, CAMERA];

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Play {
    pub id: Option<String>,
    /// Optional clip bounds [start, end) when opening a library film.
    pub range: Option<[u64; 2]>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prepare {
    pub id: String,
}

/// Without an ID, state describes the playback transport.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrepareState {
    pub id: String,
    pub tick: u64,
    pub length_ticks: u64,
    #[serde(flatten)]
    pub status: PrepareStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PrepareStatus {
    Idle,
    Starting,
    Running,
    Complete,
    Failed { error: String },
}

impl PrepareStatus {
    pub fn active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn prepare_requests_and_state_round_trip() {
        let params = json!({"id": "live:synthetic"});
        assert_eq!(to_value(from_value::<Prepare>(params.clone()).unwrap()).unwrap(), params);
        assert_eq!(to_value(from_value::<StateParams>(params.clone()).unwrap()).unwrap(), params);
        assert_eq!(to_value(StateParams::default()).unwrap(), json!({}));
        assert!(from_value::<Prepare>(json!({})).is_err());
        assert!(from_value::<Prepare>(json!({"id": "a", "tick": 10})).is_err());

        let states = [
            (PrepareStatus::Idle, "idle"),
            (PrepareStatus::Starting, "starting"),
            (PrepareStatus::Running, "running"),
            (PrepareStatus::Complete, "complete"),
            (PrepareStatus::Failed { error: "synthetic divergence".into() }, "failed"),
        ];
        for (status, name) in states {
            let state = PrepareState { id: "synthetic".into(), tick: 90, length_ticks: 900, status };
            let wire = to_value(&state).unwrap();
            assert_eq!(wire["status"], name);
            if name == "failed" {
                assert_eq!(wire["error"], "synthetic divergence");
            } else {
                assert!(wire.get("error").is_none());
            }
            assert_eq!(from_value::<PrepareState>(wire).unwrap(), state);
        }
    }
}
