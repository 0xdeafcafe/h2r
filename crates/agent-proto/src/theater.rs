//! Transport for an exact host film. Ticks count from boot.
use serde::{Deserialize, Serialize};

pub const PLAY: &str = "theater.play";
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
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    pub mode: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub id: String,
    pub tick: u64,
    pub length_ticks: u64,
    pub paused: bool,
    pub speed: f64,
    pub seeking: bool,
    pub camera: String,
}
