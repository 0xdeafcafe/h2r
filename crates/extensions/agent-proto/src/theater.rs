//! Film transport and presentation controls. Ticks count from boot.
use serde::{Deserialize, Serialize};

pub const EXIT: &str = "theater.exit";
pub const LIST: &str = "theater.list";
pub const SHARE: &str = "theater.share";
pub const OPEN: &str = "theater.open";
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Open {
    pub url: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialView {
    pub tick: Option<u32>,
    pub seat: Option<u8>,
    pub camera: Option<CameraMode>,
}
pub const PLAY: &str = "theater.play";
pub const PREPARE: &str = "theater.prepare";
pub const STATE: &str = "theater.state";
pub const PAUSE: &str = "theater.pause";
pub const SPEED: &str = "theater.speed";
pub const SEEK: &str = "theater.seek";
pub const CAMERA: &str = "theater.camera";
pub const COMPAT: &str = "theater.compat";
pub const SCREENSHOT: &str = "theater.screenshot";
pub const CLIP_MARK: &str = "theater.clip.mark";
pub const CLIP_EXPORT: &str = "theater.clip.export";
pub const CLIP_STATUS: &str = "theater.clip.status";
pub const HUD: &str = "theater.hud";
pub const KEYFRAMES: &str = "theater.keyframes";
pub const MEASUREMENTS: &str = "theater.measurements";
pub const STEP: &str = "theater.step";
pub const VERIFY: &str = "theater.verify";
pub const STATUS: &str = "theater.status";
pub const TRANSPORT_METHODS: [&str; 9] = [
    PLAY, STATE, PAUSE, SPEED, SEEK, CAMERA, HUD, KEYFRAMES, STEP,
];
pub const METHODS: [&str; 16] = [
    PLAY,
    STATE,
    PAUSE,
    SPEED,
    SEEK,
    CAMERA,
    HUD,
    KEYFRAMES,
    MEASUREMENTS,
    STEP,
    VERIFY,
    SCREENSHOT,
    CLIP_MARK,
    CLIP_EXPORT,
    CLIP_STATUS,
    STATUS,
];

/// Make a start point of the running game's arena and write it to a file
/// (`theater.start_point.capture`). The game agent answers it at a tick
/// boundary, so the state is the end of the tick in the reply.
pub const START_POINT_CAPTURE: &str = "theater.start_point.capture";
/// Read a start point, put its arena back into the running game and report
/// whether the loaded state hashes as the tag says
/// (`theater.start_point.load`).
pub const START_POINT_LOAD: &str = "theater.start_point.load";
pub const START_POINT_METHODS: [&str; 2] = [START_POINT_CAPTURE, START_POINT_LOAD];

/// `theater.start_point.capture`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartPointCapture {
    /// The file to write; an existing file is replaced.
    pub path: String,
    /// The tag key, as text (the tunnel key in a Live game; any text for an
    /// experiment).
    pub key: String,
    /// The per-game identifiers, as hex (at most 64 bytes). None: empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<String>,
}

/// What `theater.start_point.capture` wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPointCaptured {
    pub path: String,
    /// The tick whose end the state is.
    pub tick: u32,
    /// `lockstep::hash_arena` of the arena and the extension records, as 16
    /// hex digits.
    pub arena_hash: String,
    pub arena_bytes: u64,
    pub ext_bytes: u64,
    /// The file's size: tag, identifiers and both compressed frames.
    pub stored_bytes: u64,
}

/// `theater.start_point.load`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartPointLoad {
    pub path: String,
    pub key: String,
    /// The arena hash the film names for the start tick, as hex. None: the
    /// file's own, so only the tag and the digest are checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub film_hash: Option<String>,
}

/// What `theater.start_point.load` found. A refused file is an error and
/// changes nothing; a file that loads but hashes differently is reported
/// here with `matches: false`, and the game then holds the loaded state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPointLoaded {
    pub tick: u32,
    pub arena_hash: String,
    /// The hash of the state after the load.
    pub loaded_hash: String,
    /// `StartPoint::check_loaded` held.
    pub matches: bool,
    /// Why it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What this session's film recording is doing and has kept
/// (`theater.status`). It answers on any session; a playback or an
/// explicit `--film` recording reports `default_recording: false`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The session records the games a player plays into the film library.
    pub default_recording: bool,
    /// The running tape's file while recording, as a path.
    pub tape: Option<String>,
    /// A game is on the tape now.
    pub in_game: bool,
    /// Films cut so far this session, oldest first, as paths.
    pub games_cut: Vec<String>,
    /// Why the last tape or game went without a film, if one did.
    pub last_discard: Option<String>,
    /// Changes from outside the tape the recording carried on through.
    pub notes: Vec<String>,
    /// The failure that stopped the recording, if one did.
    pub failure: Option<String>,
}

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
    /// Monitor looks in the film this machine cannot draw. They are not
    /// reasons: the film still plays, and the seat draws the marker.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub monitor_looks: Vec<MonitorLookSource>,
}

/// A recorded Forge monitor look this machine cannot draw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorLookSource {
    pub seat: u8,
    /// The source map the look's model needs.
    pub map: String,
    /// `monitor_look_source`.
    pub reason: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<InitialView>,
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
    #[serde(default)]
    pub speed_reached: Option<f64>,
    pub seeking: bool,
    #[serde(default = "loaded_compatibility")]
    pub compatibility: CompatibilityState,
    /// The mode actually used. A first-person request may fall back to follow.
    pub camera: CameraMode,
    #[serde(default)]
    pub camera_pose: Option<CameraPose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_fallback: Option<CameraFallback>,
    /// Monitor looks the film has recorded so far that this machine cannot
    /// draw (`monitor_look_source`): the seat shows the marker.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub monitor_looks: Vec<MonitorLookSource>,
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
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn start_point_requests_and_replies_round_trip() {
        for wire in [
            json!({"path":"/tmp/a.sp","key":"k"}),
            json!({"path":"/tmp/a.sp","key":"k","ids":"0a0b"}),
        ] {
            let p: StartPointCapture = from_value(wire.clone()).unwrap();
            assert_eq!(to_value(p).unwrap(), wire);
        }
        for wire in [
            json!({"path":"/tmp/a.sp","key":"k"}),
            json!({"path":"/tmp/a.sp","key":"k","film_hash":"00000000000000ff"}),
        ] {
            let p: StartPointLoad = from_value(wire.clone()).unwrap();
            assert_eq!(to_value(p).unwrap(), wire);
        }
        assert!(from_value::<StartPointCapture>(json!({"path":"p"})).is_err());
        assert!(from_value::<StartPointLoad>(json!({"path":"p","key":"k","x":1})).is_err());
        let loaded = json!({
            "tick":9, "arena_hash":"0000000000000001", "loaded_hash":"0000000000000002",
            "matches":false, "error":"differs"
        });
        let r: StartPointLoaded = from_value(loaded.clone()).unwrap();
        assert_eq!(to_value(r).unwrap(), loaded);
        assert!(!METHODS.iter().any(|m| START_POINT_METHODS.contains(m)));
    }

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
            "speed":4.0, "seeking":false, "camera":"default", "kills":[], "hud":true, "bar":true, "camera_pose":null, "compatibility":"playable"
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

#[cfg(test)]
mod camera_tests {
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

fn loaded_compatibility() -> CompatibilityState {
    CompatibilityState::Playable
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compatibility_round_trips_both_wire_states() {
        for state in ["playable", "unavailable"] {
            let wire = json!({
                "state": state,
                "reasons": [],
                "film": {"xbe": "synthetic", "contract": null, "hooks": null, "abi": "h2-sim/2"},
                "installed": [{"stamp": "synthetic build", "state": state}],
            });
            let result: Compatibility = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(result).unwrap(), wire);
        }
    }
}
