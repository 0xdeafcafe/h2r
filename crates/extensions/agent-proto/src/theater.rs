//! Film transport and presentation controls. Ticks count from boot.
use serde::{Deserialize, Serialize};

pub const PLAY: &str = "theater.play";
pub const STATE: &str = "theater.state";
pub const PAUSE: &str = "theater.pause";
pub const SPEED: &str = "theater.speed";
pub const SEEK: &str = "theater.seek";
pub const CAMERA: &str = "theater.camera";
pub const COMPAT: &str = "theater.compat";
pub const METHODS: [&str; 6] = [PLAY, STATE, PAUSE, SPEED, SEEK, CAMERA];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityState {
    Playable,
    #[default]
    Unavailable,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    pub id: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compatibility {
    pub state: CompatibilityState,
    pub reasons: Vec<String>,
    pub film: FilmContract,
    pub installed: Vec<InstalledImage>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilmContract {
    pub xbe: String,
    pub contract: Option<String>,
    pub hooks: Option<String>,
    pub abi: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledImage {
    pub stamp: String,
    pub state: CompatibilityState,
}

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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub id: String,
    pub tick: u64,
    pub length_ticks: u64,
    pub paused: bool,
    pub speed: f64,
    pub seeking: bool,
    #[serde(default = "loaded_compatibility")]
    pub compatibility: CompatibilityState,
    pub camera: CameraMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_seat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_player: Option<String>,
    #[serde(default)]
    pub kills: Vec<u64>,
}

fn loaded_compatibility() -> CompatibilityState {
    CompatibilityState::Playable
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compatibility_round_trips_both_wire_states() {
        for state in ["playable", "unavailable"] {
            let wire = json!({
                "state": state,
                "reasons": [],
                "film": {"xbe": "synthetic", "contract": null, "hooks": null, "abi": "h2-sim/1"},
                "installed": [{"stamp": "synthetic build", "state": state}],
            });
            let result: Compatibility = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(result).unwrap(), wire);
        }
    }
}
