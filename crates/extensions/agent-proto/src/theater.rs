//! Film transport and presentation controls. Ticks count from boot.
use serde::{Deserialize, Serialize};

pub const PLAY: &str = "theater.play";
pub const STATE: &str = "theater.state";
pub const PAUSE: &str = "theater.pause";
pub const SPEED: &str = "theater.speed";
pub const SEEK: &str = "theater.seek";
pub const CAMERA: &str = "theater.camera";
pub const SCREENSHOT: &str = "theater.screenshot";
pub const CLIP_MARK: &str = "theater.clip.mark";
pub const CLIP_EXPORT: &str = "theater.clip.export";
pub const CLIP_STATUS: &str = "theater.clip.status";
pub const TRANSPORT_METHODS: [&str; 6] = [PLAY, STATE, PAUSE, SPEED, SEEK, CAMERA];
pub const METHODS: [&str; 10] = [
    PLAY,
    STATE,
    PAUSE,
    SPEED,
    SEEK,
    CAMERA,
    SCREENSHOT,
    CLIP_MARK,
    CLIP_EXPORT,
    CLIP_STATUS,
];

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
    pub camera: CameraMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_seat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_player: Option<String>,
    #[serde(default)]
    pub kills: Vec<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Screenshot {
    pub scale: Option<u8>,
    pub path: Option<std::path::PathBuf>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotSaved {
    pub path: std::path::PathBuf,
    pub width: u32,
    pub height: u32,
    pub tick: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipMark {
    #[serde(rename = "in")]
    pub start: Option<u64>,
    #[serde(rename = "out")]
    pub end: Option<u64>,
    #[serde(default)]
    pub clear: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipMarks {
    #[serde(rename = "in")]
    pub start: Option<u64>,
    #[serde(rename = "out")]
    pub end: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipFormat {
    Png,
    Mp4,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipExport {
    pub from: u64,
    pub to: u64,
    pub fps: u32,
    pub format: ClipFormat,
    pub path: Option<std::path::PathBuf>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipJob {
    pub job: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipStatus {
    pub frames_done: u64,
    pub frames_total: u64,
    pub path: std::path::PathBuf,
    pub error: Option<String>,
    /// True once the output has been published or the job has failed.
    pub finished: bool,
}
