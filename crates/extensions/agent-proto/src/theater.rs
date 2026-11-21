//! Film transport and presentation controls. Ticks count from boot.
use serde::{Deserialize, Serialize};

pub const PLAY: &str = "theater.play";
pub const STATE: &str = "theater.state";
pub const PAUSE: &str = "theater.pause";
pub const SPEED: &str = "theater.speed";
pub const SEEK: &str = "theater.seek";
pub const CAMERA: &str = "theater.camera";
pub const HUD: &str = "theater.hud";
pub const METHODS: [&str; 7] = [PLAY, STATE, PAUSE, SPEED, SEEK, CAMERA, HUD];

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

#[derive(Debug, Default, Serialize, Deserialize)]
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
    /// Horizontal FOV at 4:3, in degrees. Omit to keep the current lens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov: Option<f32>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hud {
    pub game: Option<bool>,
    pub bar: Option<bool>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct CameraPose {
    pub position: [f32; 3],
    /// Radians.
    pub yaw: f32,
    pub pitch: f32,
    /// Horizontal degrees at 4:3.
    pub fov: f32,
    /// Orbit radius in world units.
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CameraFallback {
    NoRecordedViewport,
}

fn shown() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub id: String,
    pub tick: u64,
    pub length_ticks: u64,
    pub paused: bool,
    pub speed: f64,
    pub seeking: bool,
    /// The mode actually used. A first-person request may fall back to follow.
    pub camera: CameraMode,
    #[serde(default)]
    pub camera_pose: Option<CameraPose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_fallback: Option<CameraFallback>,
    #[serde(default = "shown")]
    pub hud: bool,
    #[serde(default = "shown")]
    pub bar: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_seat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_player: Option<String>,
    #[serde(default)]
    pub kills: Vec<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn theater_camera_and_hud_wire() {
        let request: Camera = serde_json::from_value(json!({
            "mode": "first_person", "seat": 3, "fov": 85
        }))
        .unwrap();
        assert_eq!(request.mode, CameraMode::FirstPerson);
        assert_eq!(request.fov, Some(85.));
        assert_eq!(
            serde_json::to_value(request).unwrap()["mode"],
            "first-person"
        );

        let request: Hud = serde_json::from_value(json!({"bar": false})).unwrap();
        assert_eq!(request.game, None);
        assert_eq!(request.bar, Some(false));
        assert!(serde_json::from_value::<Hud>(json!({"visible": true})).is_err());
    }
}
