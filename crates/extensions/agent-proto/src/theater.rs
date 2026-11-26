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
pub const MEASUREMENTS: &str = "theater.measurements";
pub const STEP: &str = "theater.step";
/// The session ring's entries (`theater.keyframes`).
pub const KEYFRAMES: &str = "theater.keyframes";
pub const VERIFY: &str = "theater.verify";
pub const STATUS: &str = "theater.status";
/// Watch a film in the running game: queue its options as the next game
/// (`theater.watch`).
pub const WATCH: &str = "theater.watch";
/// Leave the film being watched for the film page (`theater.end`).
pub const END: &str = "theater.end";
/// The methods of a game that records and watches in one session. They are
/// not in [`METHODS`], which a film player process answers.
pub const WATCH_METHODS: [&str; 2] = [WATCH, END];
pub const TRANSPORT_METHODS: [&str; 9] = [
    PLAY, STATE, PAUSE, SPEED, SEEK, CAMERA, HUD, STEP, KEYFRAMES,
];
pub const METHODS: [&str; 16] = [
    PLAY,
    STATE,
    PAUSE,
    SPEED,
    SEEK,
    CAMERA,
    HUD,
    MEASUREMENTS,
    STEP,
    VERIFY,
    SCREENSHOT,
    CLIP_MARK,
    CLIP_EXPORT,
    CLIP_STATUS,
    STATUS,
    KEYFRAMES,
];

/// `theater.watch`: the film to watch, by library id or by path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    /// A film of the library (`theater.list`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// A film file, container path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The tick to watch from: the film plays unrendered to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraMode>,
}

impl Watch {
    /// The view the film opens with.
    pub fn view(&self) -> InitialView {
        InitialView { tick: self.tick, seat: self.seat, camera: self.camera }
    }

    /// Exactly one of `id` and `path` names the film.
    pub fn check(&self) -> Result<(), &'static str> {
        match (&self.id, &self.path) {
            (Some(_), None) | (None, Some(_)) => Ok(()),
            _ => Err("name the film by id or by path, not both and not neither"),
        }
    }
}

/// What `theater.watch` queued. The game loads the film's map next;
/// `theater.state` reports the film once its first update has run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watching {
    pub id: String,
    pub length_ticks: u64,
    /// The film's options are queued as the next game.
    pub queued: bool,
}

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
    /// The film file of the game being recorded, as a path, from the
    /// game's activation to its end.
    pub recording: Option<String>,
    /// A game is being recorded now.
    pub in_game: bool,
    /// Films sealed so far this session, oldest first, as film file paths.
    pub films: Vec<String>,
    /// Why the last game went without a film, if one did.
    pub last_discard: Option<String>,
    /// Changes from outside the film the recording carried on through.
    pub notes: Vec<String>,
    /// The failure that stopped the recording, if one did.
    pub failure: Option<String>,
    /// Why the last film watched in this game stopped before the viewer
    /// left it (a start or digest divergence), if one did.
    #[serde(default)]
    pub watch_failure: Option<String>,
}

/// Host wall-clock timings and write-watch counts for an instrumented run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measurements {
    pub child_to_first_slice_ms: Option<u64>,
    pub boot_to_match_start_ms: Option<u64>,
    pub match_start_tick: Option<u64>,
    pub fast_forward_ms: Option<u64>,
    pub fast_forward_ticks: Option<u64>,
    pub page_windows: Vec<PageWindow>,
    pub page_windows_omitted: u64,
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

/// A route callers can force. A missing route runs forward from the tick
/// shown when the target is ahead of it, and otherwise starts the film's
/// game again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeekRoute {
    Boot,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub ticks: u64,
}

/// `theater.keyframes`: the session ring of the film playing
/// (docs/game/theater-design.md, "The session ring").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyframes {
    /// The film ticks the ring holds an entry for, oldest first.
    pub ring: Vec<u64>,
    /// The entries' stored bytes.
    pub ring_bytes: u64,
    /// The most stored bytes the ring keeps before it thins.
    pub ring_cap: u64,
    /// Why the ring is off for this film: `H2_THEATER_RING=0`, or an entry
    /// that failed its check. Absent while it is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<String>,
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
/// How a seek reaches its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeekSource {
    /// The film runs on from the tick shown, unrendered.
    Forward,
    /// The film's game is queued again and runs on from its start.
    Boot,
    /// The session ring's latest entry at or before the target is loaded in
    /// the running game, and the film runs on from it.
    Ring,
}

/// The jump the player is serving, kept after it completes until the next
/// jump.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeekInfo {
    pub target: u64,
    pub from_tick: u64,
    pub source: SeekSource,
    pub remaining_ticks: u64,
    /// The time the ring entry took to load, for a ring jump that has loaded
    /// it, in milliseconds rounded up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// Tick bounds are inclusive. A missing bound covers that end of the film.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Verify {
    pub from: Option<u64>,
    pub to: Option<u64>,
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
    fn watch_params_and_result_round_trip() {
        for wire in [
            json!({"id":"auto-1"}),
            json!({"path":"/films/a.h2film","tick":300,"seat":2,"camera":"first-person"}),
        ] {
            let watch: Watch = from_value(wire.clone()).unwrap();
            assert_eq!(watch.check(), Ok(()));
            assert_eq!(to_value(watch).unwrap(), wire);
        }
        let both: Watch = from_value(json!({"id":"a","path":"b"})).unwrap();
        assert!(both.check().is_err());
        assert!(from_value::<Watch>(json!({})).unwrap().check().is_err());
        assert!(from_value::<Watch>(json!({"id":"a","paused":true})).is_err());
        let view = from_value::<Watch>(json!({"id":"a","tick":9,"seat":1})).unwrap().view();
        assert_eq!((view.tick, view.seat, view.camera), (Some(9), Some(1), None));
        let queued = json!({"id":"a","length_ticks":900,"queued":true});
        let result: Watching = from_value(queued.clone()).unwrap();
        assert_eq!(to_value(result).unwrap(), queued);
        assert_eq!(WATCH_METHODS, ["theater.watch", "theater.end"]);
        assert!(!METHODS.iter().any(|m| WATCH_METHODS.contains(m)));
        assert_eq!(to_value(SeekSource::Forward).unwrap(), json!("forward"));
    }

    #[test]
    fn theater_seek_and_step_params_round_trip() {
        for wire in [
            json!({"tick":0}),
            json!({"tick":300,"source":"boot","clip_marks":[10,null]}),
            json!({"cancel":true}),
            json!({"tick":42,"cancel":true}),
        ] {
            let seek: Seek = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(seek).unwrap(), wire);
        }
        for gone in ["ring", "keyframe", "sidecar"] {
            assert!(serde_json::from_value::<Seek>(json!({"tick":1,"source":gone})).is_err());
        }
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
            "target":30, "from_tick":120, "source":"boot", "remaining_ticks":30
        });
        let current: State = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(current.speed_reached, Some(2.25));
        assert_eq!(current.seek.unwrap().source, SeekSource::Boot);
        assert_eq!(serde_json::to_value(current).unwrap(), wire);
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
            (json!({"from": 30, "to": 30}), [30, 30]),
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
            first_divergence: Some(Divergence { tick: 4, kind: "gameplay digest differs".into() }),
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

#[cfg(test)]
mod ring_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_ring_jump_and_the_ring_round_trip() {
        let wire = json!({
            "target":200, "from_tick":900, "source":"ring", "remaining_ticks":50, "restore_ms":4
        });
        let seek: SeekInfo = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!((seek.source, seek.restore_ms), (SeekSource::Ring, Some(4)));
        assert_eq!(serde_json::to_value(seek).unwrap(), wire);

        let on = json!({"ring":[150,300,450], "ring_bytes":786432, "ring_cap":268435456});
        let ring: Keyframes = serde_json::from_value(on.clone()).unwrap();
        assert_eq!(ring.off, None);
        assert_eq!(serde_json::to_value(ring).unwrap(), on);
        let off = json!({"ring":[], "ring_bytes":0, "ring_cap":0, "off":"H2_THEATER_RING=0"});
        let ring: Keyframes = serde_json::from_value(off.clone()).unwrap();
        assert_eq!(serde_json::to_value(ring).unwrap(), off);

        assert!(TRANSPORT_METHODS.contains(&KEYFRAMES) && METHODS.contains(&KEYFRAMES));
        // The ring is the player's route: a caller asks only for the start.
        assert!(serde_json::from_value::<Seek>(json!({"tick":1,"source":"ring"})).is_err());
    }
}
