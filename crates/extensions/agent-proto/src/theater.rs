//! Film transport and presentation controls. Ticks count from boot.
use serde::{Deserialize, Serialize};

pub const PLAY: &str = "theater.play";
pub const STATE: &str = "theater.state";
pub const PAUSE: &str = "theater.pause";
pub const SPEED: &str = "theater.speed";
pub const SEEK: &str = "theater.seek";
pub const CAMERA: &str = "theater.camera";
pub const KEYFRAMES: &str = "theater.keyframes";
pub const MEASUREMENTS: &str = "theater.measurements";
pub const STEP: &str = "theater.step";
pub const METHODS: [&str; 9] = [
    PLAY,
    STATE,
    PAUSE,
    SPEED,
    SEEK,
    CAMERA,
    KEYFRAMES,
    MEASUREMENTS,
    STEP,
];

/// Host wall-clock timings and write-watch counts for an instrumented run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measurements {
    pub child_to_first_slice_ms: Option<u64>,
    pub boot_to_match_start_ms: Option<u64>,
    pub match_start_tick: Option<u64>,
    pub fast_forward_ms: Option<u64>,
    pub fast_forward_ticks: Option<u64>,
    pub captures: Vec<CaptureMeasurement>,
    pub page_windows: Vec<PageWindow>,
    pub captures_omitted: u64,
    pub page_windows_omitted: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureMeasurement {
    pub tick: u64,
    pub slices: u64,
    pub elapsed_ms: u64,
    pub published: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageWindow {
    pub seconds: u32,
    pub from_tick: u64,
    pub to_tick: u64,
    pub pages: u64,
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
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seek {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SeekRoute>,
    /// The overlay's in and out ticks, preserved across the seek.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_marks: Option<[Option<u64>; 2]>,
}

/// A cold route callers can force. A missing route uses the best available
/// keyframe, then boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeekRoute {
    Boot,
    Keyframe,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub ticks: u64,
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
    #[serde(default)]
    pub speed_reached: Option<f64>,
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

/// The recorded states the player can seek from, and the film's capture
/// points. The session ring and sidecar keyframes do not exist yet: both
/// lists are empty and the byte counts are zero.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn theater_seek_and_step_params_round_trip() {
        for wire in [
            json!({"tick":0}),
            json!({"tick":300,"source":"boot","clip_marks":[10,null]}),
            json!({"tick":300,"source":"keyframe"}),
            json!({"cancel":true}),
            json!({"tick":42,"cancel":true}),
        ] {
            let seek: Seek = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(seek).unwrap(), wire);
        }
        assert!(serde_json::from_value::<Seek>(json!({"tick":1,"source":"ring"})).is_err());
        let wire = json!({"ticks":1});
        let step: Step = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(step).unwrap(), wire);
    }

    #[test]
    fn theater_state_round_trips_with_progress_and_reached_speed() {
        let mut wire = json!({
            "id":"fixture", "tick":30, "length_ticks":200, "paused":false,
            "speed":4.0, "seeking":false, "camera":"default", "kills":[]
        });
        let old: State = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(old.seek, None);
        assert_eq!(old.speed_reached, None);

        wire["speed_reached"] = json!(2.25);
        wire["seek"] = json!({
            "target":30, "from_tick":120, "source":"keyframe", "warm":false,
            "remaining_ticks":0, "restore_ms":85
        });
        let current: State = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(current.speed_reached, Some(2.25));
        assert_eq!(current.seek.unwrap().restore_ms, Some(85));
        assert_eq!(serde_json::to_value(current).unwrap(), wire);
    }
}
