//! Playback controls belong to the host. They never enter the recorded
//! input stream, including while the transport itself is hidden.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use agent_proto::theater::{self as api, CameraMode};
use debug_menu::sources::method::CHEAT_SET;
use input::host::{HostButton, HostButtons, HostPadId, HostPadState};
use input::{GatedPad, PadGate};
use serde::Deserialize;
use serde_json::{Value, json};

use super::adapter::{Clip, Playback, Player, Source};
use super::{Entry, Theater, playback_page::{self, Command, Divergence, Jump, Keyframes, Menu}};
use crate::ui_kit::{input::Input, spec::ScreenSpec};
use super::prepare::{Preparation, Status as PrepareStatus};

const REFRESH: Duration = Duration::from_millis(150);
const LONG_PRESS: Duration = Duration::from_millis(500);
pub(super) const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Space,
    Left,
    Right,
    Up,
    Down,
    Slower,
    Faster,
    Hide,
    Hud,
    Clip,
    Screenshot,
    Camera,
    Forward,
    Backward,
    StrafeLeft,
    StrafeRight,
    Rise,
    Fall,
    Leave,
    Accept,
    Step,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Pause,
    Skip(i64),
    Seek(u64),
    Speed(bool),
    Camera,
    Player(bool),
    Hide,
    Hud,
    Mark,
    Screenshot,
    Leave,
    Step,
    Menu,
    CancelJump,
}

/// Transport fields may be absent while the child reopens after a seek.
#[derive(Debug, Default, Deserialize)]
struct AgentState {
    #[serde(default)]
    seeking: bool,
    #[serde(default)]
    paused: Option<bool>,
    #[serde(default)]
    speed: Option<f64>,
    #[serde(default)]
    tick: Option<u64>,
    #[serde(default)]
    length_ticks: Option<u64>,
    #[serde(default)]
    speed_reached: Option<f64>,
    compatibility: Option<api::CompatibilityState>,
    compatibility_reason: Option<String>,
    hud: Option<bool>,
    bar: Option<bool>,
    #[serde(default)]
    camera: Option<CameraMode>,
    #[serde(default)]
    camera_player: Option<String>,
    #[serde(default)]
    camera_seat: Option<u8>,
    camera_fallback: Option<String>,
    #[serde(default)]
    seek: Option<Jump>,
    #[serde(default)]
    divergence: Option<Divergence>,
}

#[derive(Clone, Debug)]
pub(super) struct Track {
    pub film: String,
    pub tick: u64,
    pub length: u64,
    pub paused: bool,
    pub speed: usize,
    pub speed_requested: f64,
    pub speed_reached: Option<f64>,
    pub compatibility: api::CompatibilityState,
    pub compatibility_reason: Option<String>,
    pub players: Vec<Player>,
    pub scoreboard: Vec<Player>,
    pub camera: CameraMode,
    camera_missing_view: bool,
    source: Source,
    local_viewports: bool,
    shared_rig: bool,
    pub tick_rate: u64,
    pub selected_player: usize,
    pub hud: bool,
    pub visible: bool,
    pub marks: (Option<u64>, Option<u64>),
    pub kills: Vec<u64>,
    pub keyframes: Keyframes,
    pub jump: Option<Jump>,
    pub seeking: bool,
    pub divergence: Option<Divergence>,
    range: Option<(u64, u64)>,
}

impl Track {
    pub(super) fn from_entry(entry: &Entry) -> Self {
        Self {
            film: entry.film.id.clone(),
            tick: 0,
            length: entry.film.length_ticks,
            paused: false,
            speed: 2,
            speed_requested: 1.0,
            speed_reached: None,
            compatibility: entry.film.compatibility,
            compatibility_reason: entry.film.compatibility_reason.clone(),
            players: entry.film.camera_roster().to_vec(),
            scoreboard: entry.film.scoreboard.clone(),
            camera: if entry.film.source == Source::Host {
                CameraMode::Free
            } else {
                CameraMode::Default
            },
            camera_missing_view: false,
            source: entry.film.source,
            tick_rate: entry.film.tick_rate(),
            local_viewports: entry.film.version.is_some_and(|v| v >= 3),
            shared_rig: false,
            selected_player: 0,
            hud: true,
            visible: true,
            marks: (None, None),
            kills: entry.film.kill_ticks.clone(),
            keyframes: Keyframes::default(),
            jump: None,
            seeking: false,
            divergence: None,
            range: entry
                .clip
                .as_ref()
                .map(|clip| (clip.from_tick, clip.to_tick)),
        }
    }

    pub(super) fn bounds(&self) -> (u64, u64) {
        self.range
            .map(|(from, to)| (from, to.saturating_sub(1).max(from)))
            .unwrap_or((0, self.length))
    }

    fn skip(&self, seconds: i64) -> u64 {
        let delta = seconds.saturating_mul(self.tick_rate as i64);
        let (lo, hi) = self.bounds();
        self.tick.saturating_add_signed(delta).clamp(lo, hi)
    }

    fn select_player(&mut self, forward: bool) {
        let count = self.players.len();
        if count != 0 {
            let delta = if forward { 1 } else { count - 1 };
            self.selected_player = (self.selected_player + delta) % count;
        }
    }

    pub(super) fn cameras(&self) -> &'static [(CameraMode, &'static str)] {
        match self.source {
            Source::Host => &CAMERAS,
            Source::Local if self.shared_rig => &RIG_CAMERAS,
            Source::Local if self.local_viewports => &LOCAL_CAMERAS,
            Source::Local => &LOCAL_CAMERAS[..1],
            Source::Archive => &[],
        }
    }

    fn cycle_camera(&mut self) {
        let available = self.cameras();
        if available.is_empty() {
            return;
        }
        let next = available
            .iter()
            .position(|(mode, _)| *mode == self.camera)
            .map_or(0, |i| (i + 1) % available.len());
        self.camera = available[next].0;
        self.camera_missing_view = false;
    }

    pub(super) fn camera_target(&self) -> Option<&Player> {
        self.players
            .get(self.selected_player)
            .filter(|p| p.seat.is_some() || !p.name.is_empty())
    }

    pub(super) fn camera_label(&self) -> &'static str {
        if self.camera_missing_view {
            return "Follow (no viewport)";
        }
        self.cameras().iter().find(|(mode, _)| *mode == self.camera)
            .map_or("Recorded view", |(_, label)| *label)
    }

    pub(super) fn can_mark_out(&self) -> bool {
        self.marks.0.is_some_and(|start| start < self.tick)
    }

    pub(super) fn clip_range(&self) -> Option<[u64; 2]> {
        let (Some(from), Some(to)) = self.marks else { return None };
        (from < to).then_some([from, to])
    }

    fn mark_out(&mut self) -> Option<Clip> {
        let from = self.marks.0.filter(|&from| from < self.tick)?;
        self.marks.1 = Some(self.tick);
        Some(Clip { film: self.film.clone(), from_tick: from, to_tick: self.tick })
    }

    fn mark(&mut self) -> Option<Clip> {
        if self.marks.1.is_none() && self.can_mark_out() {
            self.mark_out()
        } else {
            self.marks = (Some(self.tick), None);
            None
        }
    }

    fn read(&mut self, s: AgentState) {
        if let Some(tick) = s.tick {
            self.tick = tick;
        }
        if let Some(length) = s.length_ticks {
            self.length = length;
        }
        if let Some(paused) = s.paused {
            self.paused = paused;
        }
        if let Some(speed) = s.speed.filter(|speed| (0.25..=4.0).contains(speed)) {
            self.speed_requested = speed;
            self.speed = SPEEDS.iter().position(|&s| s >= speed).unwrap_or(SPEEDS.len() - 1);
        }
        self.speed_reached = s.speed_reached.filter(|speed| speed.is_finite() && *speed >= 0.0);
        if let Some(compatibility) = s.compatibility {
            self.compatibility = compatibility;
            if compatibility == api::CompatibilityState::Playable {
                self.compatibility_reason = None;
            } else if s.compatibility_reason.is_some() {
                self.compatibility_reason = s.compatibility_reason;
            }
        }
        self.shared_rig |= s.hud.is_some() && s.bar.is_some();
        self.hud = s.hud.unwrap_or(self.hud);
        self.visible = s.bar.unwrap_or(self.visible);
        self.seeking = s.seeking;
        self.jump = s.seek;
        if s.divergence.is_some() {
            self.divergence = s.divergence;
        }
        if let Some(camera) = s.camera {
            self.camera_missing_view = s.camera_fallback.as_deref() == Some("no-recorded-viewport");
            self.camera = if self.camera_missing_view { CameraMode::FirstPerson } else { camera };
            let selected = s
                .camera_seat
                .and_then(|seat| self.players.iter().position(|p| p.seat == Some(seat)))
                .or_else(|| {
                    s.camera_player
                        .and_then(|name| self.players.iter().position(|p| p.name == name))
                });
            if let Some(index) = selected {
                self.selected_player = index;
            }
        }
    }

    /// The local review model steps these same controls without an agent.
    pub(super) fn simulate(&mut self, c: Control) {
        match c {
            Control::Pause => self.paused = !self.paused,
            Control::Skip(n) => self.tick = self.skip(n),
            Control::Speed(up) => {
                self.speed = step(self.speed, SPEEDS.len(), up);
                self.speed_requested = SPEEDS[self.speed];
            }
            Control::Seek(tick) => self.tick = tick,
            Control::Camera => self.cycle_camera(),
            Control::Player(down) => self.select_player(down),
            Control::Hide => self.visible = !self.visible,
            Control::Hud => self.hud = !self.hud,
            Control::Mark => {
                self.mark();
            }
            Control::Step if self.paused => self.tick = self.tick.saturating_add(1).min(self.bounds().1),
            Control::CancelJump => self.seeking = false,
            Control::Screenshot | Control::Leave | Control::Menu | Control::Step => {}
        }
    }

}

const CAMERAS: [(CameraMode, &str); 4] = [
    (CameraMode::Free, "Fly"),
    (CameraMode::Orbit, "Orbit"),
    (CameraMode::Follow, "Follow"),
    (CameraMode::FirstPerson, "First person"),
];

const LOCAL_CAMERAS: [(CameraMode, &str); 2] = [
    (CameraMode::Default, "Recorded view"),
    (CameraMode::FirstPerson, "First person"),
];

const RIG_CAMERAS: [(CameraMode, &str); 5] = [
    (CameraMode::Default, "Recorded view"),
    CAMERAS[0],
    CAMERAS[1],
    CAMERAS[2],
    CAMERAS[3],
];

fn step(current: usize, count: usize, forward: bool) -> usize {
    if forward {
        (current + 1).min(count.saturating_sub(1))
    } else {
        current.saturating_sub(1)
    }
}

enum Request {
    Open,
    Prepare(String),
    State,
    Keyframes,
    Command,
    Export,
    ExportStatus,
    Share,
    Pause,
    Seek,
    Speed(usize),
    Hud { game: Option<bool>, bar: Option<bool> },
    LegacyHud(bool),
    Camera,
}

#[derive(Default)]
pub(super) struct Session {
    rpc: Option<Playback>,
    pending: HashMap<u64, Request>,
    pub track: Option<Track>,
    ready: bool,
    last_poll: Option<Instant>,
    pub notice: Option<String>,
    menu: Option<Menu>,
    last_keyframes: Option<Instant>,
    keyframes_unavailable: bool,
    hud_unavailable: bool,
    pub save_requested: bool,
    export: Option<String>,
    export_poll: Option<Instant>,
    pub preparation: Option<Preparation>,
    prepare_poll: Option<Instant>,
    pub open_error: Option<String>,
    pub screenshot: bool,
    pub leave_requested: bool,
    pub completed_clip: Option<Clip>,
    held_keys: HashSet<Key>,
    key_skip: SkipHold,
    camera_dirty: bool,
    last_motion: Option<Instant>,
    pad_motion: Drive,
    mouse_look: [f32; 2],
    mouse_zoom: f32,
}

impl Session {
    pub fn connect(&mut self, agent: &agent::Agent) {
        self.rpc = Some(Playback::new(agent));
    }

    pub fn play(&mut self, entry: &Entry) -> Result<(), String> {
        if self.pending.values().any(|r| matches!(r, Request::Open)) {
            return Err("A film is already opening or playing".into());
        }
        let rpc = self.rpc.as_mut().ok_or("Playback agent is unavailable")?;
        let range = entry
            .clip
            .as_ref()
            .map(|clip| [clip.from_tick, clip.to_tick]);
        let id = rpc.send(
            agent_proto::theater::PLAY,
            json!({"id": entry.film.id, "range": range}),
        );
        self.pending.insert(id, Request::Open);
        Ok(())
    }

    pub(super) fn prepare(&mut self, entry: &Entry) -> Result<(), String> {
        if self.preparation.as_ref().is_some_and(Preparation::active) {
            return Err("A film is already being prepared".into());
        }
        if self.rpc.is_none() {
            return Err("Playback agent is unavailable".into());
        }
        let id = entry.film.id.clone();
        self.preparation = Some(Preparation::starting(&id, entry.film.length_ticks));
        self.prepare_poll = Some(Instant::now());
        self.send("theater.prepare", json!({"id": id}), Request::Prepare(id));
        Ok(())
    }

    pub fn playback(entry: &Entry, rpc: Playback, [mark_in, mark_out]: [Option<u64>; 2]) -> Self {
        let mut track = Track::from_entry(entry);
        track.marks = (mark_in, mark_out);
        Self {
            track: Some(track),
            rpc: Some(rpc),
            ..Self::default()
        }
    }

    fn send(&mut self, method: &'static str, params: Value, kind: Request) {
        if let Some(rpc) = &mut self.rpc {
            self.pending.insert(rpc.send(method, params), kind);
        }
    }

    pub fn pump(&mut self) {
        if let Some(rpc) = &mut self.rpc {
            let answers = rpc.answers();
            for (id, answer) in answers {
                if let Some(request) = self.pending.remove(&id) {
                    self.answer(request, answer);
                }
            }
        }
        let now = Instant::now();
        if let Some(seconds) = self.key_skip.held(now) {
            self.control(Control::Skip(seconds));
        }
        if self.menu.is_none() {
            self.update_camera(now);
        }
        if let Some(job) = self.preparation.as_ref().filter(|job| job.active())
            && self.prepare_poll.is_none_or(|last| now.duration_since(last) >= REFRESH)
            && !self.pending.values().any(|r| matches!(r, Request::Prepare(_)))
        {
            let id = job.id.clone();
            self.prepare_poll = Some(now);
            self.send(api::STATE, json!({"id": id}), Request::Prepare(id));
        }
        if let Some(job) = &self.export
            && self.export_poll.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1))
            && !self.pending.values().any(|r| matches!(r, Request::ExportStatus))
        {
            let job = job.clone();
            self.export_poll = Some(now);
            self.send("theater.clip.status", json!({"job": job}), Request::ExportStatus);
        }
        if self.ready && !self.keyframes_unavailable
            && self.last_keyframes.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1))
            && !self.pending.values().any(|r| matches!(r, Request::Keyframes))
        {
            self.last_keyframes = Some(now);
            self.send("theater.keyframes", json!({}), Request::Keyframes);
        }
        if self.track.is_some()
            && self
                .last_poll
                .is_none_or(|last| now.duration_since(last) >= REFRESH)
            && !self
                .pending
                .values()
                .any(|req| matches!(req, Request::State))
        {
            self.last_poll = Some(now);
            self.send(agent_proto::theater::STATE, json!({}), Request::State);
        }
    }

    fn answer(&mut self, request: Request, answer: Result<Value, String>) {
        match (request, answer) {
            (Request::Prepare(id), result) => {
                let result = result.and_then(|value| {
                    let state: Preparation = serde_json::from_value(value).map_err(|e| e.to_string())?;
                    if state.id == id {
                        Ok(state)
                    } else {
                        Err("Preparation status belongs to a different film".into())
                    }
                });
                match result {
                    Ok(state) => self.preparation = Some(state),
                    Err(error) => {
                        if let Some(state) = self.preparation.as_mut().filter(|job| job.id == id) {
                            state.status = PrepareStatus::Failed { error };
                        }
                    }
                }
            }
            (Request::Open, Ok(_)) => {}
            (Request::Open, Err(error)) => self.open_error = Some(error),
            (Request::State, Ok(value)) => {
                if let (Some(track), Ok(mut s)) =
                    (&mut self.track, serde_json::from_value::<AgentState>(value))
                {
                    if self.camera_dirty
                        || self.pending.values().any(|r| matches!(r, Request::Camera))
                    {
                        s.camera = None;
                    }
                    self.ready = true;
                    let seeking = s.seeking;
                    track.read(s);
                    if let Some((start, end)) = track.range
                        && !seeking
                        && (track.tick < start || track.tick >= end)
                        && !self.pending.values().any(|r| matches!(r, Request::Seek))
                    {
                        self.seek(start);
                    }
                }
            }
            (Request::State, Err(_)) if !self.ready || self.track.as_ref().is_some_and(|t| t.seeking) => {}
            (Request::Keyframes, Ok(value)) => {
                if let (Some(track), Ok(keyframes)) = (&mut self.track, serde_json::from_value(value)) {
                    track.keyframes = keyframes;
                }
            }
            (Request::Keyframes, Err(error)) => {
                self.keyframes_unavailable = error.contains("no method") || error.contains("not found");
            }
            (Request::Seek, Ok(value)) => {
                if let (Some(track), Ok(state)) = (&mut self.track, serde_json::from_value(value)) {
                    track.read(state);
                }
            }
            (Request::Export, Ok(value)) => {
                self.export = value.get("job").and_then(Value::as_str).map(str::to_owned);
                self.export_poll = None;
                self.notice = Some("Clip export started".into());
            }
            (Request::ExportStatus, Ok(value)) => {
                self.notice = Some(if let Some(error) = value.get("error").and_then(Value::as_str) {
                    self.export = None;
                    format!("Clip export failed: {error}")
                } else if value.get("finished").and_then(Value::as_bool) == Some(true) {
                    self.export = None;
                    "Clip exported".into()
                } else {
                    let done = value.get("frames_done").and_then(Value::as_u64).unwrap_or(0);
                    let total = value.get("frames_total").and_then(Value::as_u64).unwrap_or(0);
                    format!("Exporting {done} / {total} frames")
                });
            }
            (Request::ExportStatus, Err(error)) => {
                self.export = None;
                self.notice = Some(error);
            }
            (Request::Share, Ok(value)) => {
                self.notice = Some(value.get("url").and_then(Value::as_str)
                    .map_or_else(|| "Film shared".into(), |url| format!("Shared: {url}")));
            }
            (Request::Pause, Ok(value)) => {
                if let Some(track) = &mut self.track {
                    track.paused = value
                        .get("paused")
                        .and_then(Value::as_bool)
                        .unwrap_or(!track.paused);
                }
            }
            (Request::Seek, Err(error)) => {
                if let Some(track) = &mut self.track {
                    track.range = None;
                }
                self.notice = Some(error);
            }
            (Request::Speed(speed), Ok(_)) => {
                if let Some(track) = &mut self.track {
                    track.speed = speed;
                    track.speed_requested = SPEEDS[speed];
                }
            }
            (Request::Hud { game, bar }, result) => match result {
                Ok(value) => {
                    if let Some(track) = &mut self.track {
                        track.visible = value.get("bar").and_then(Value::as_bool).or(bar).unwrap_or(track.visible);
                        track.hud = value.get("hud").and_then(Value::as_bool).or(game).unwrap_or(track.hud);
                    }
                }
                Err(error) if error.contains("no method") => {
                    self.hud_unavailable = true;
                    self.set_hud(game, bar);
                }
                Err(error) => self.notice = Some(error),
            },
            (Request::LegacyHud(hud), Ok(_)) => {
                if let Some(track) = &mut self.track {
                    track.hud = hud;
                }
            }
            (_, Err(error)) => self.notice = Some(error),
            _ => {}
        }
    }

    pub fn view(&self) -> Option<ScreenSpec> {
        let track = self.track.as_ref()?;
        Some(if let Some(menu) = &self.menu {
            menu.spec(track, self.notice.as_deref())
        } else {
            playback_page::bar(track, self.ready, self.notice.as_deref())
        })
    }

    pub(super) fn timeline(&self) -> Option<[u32; 4]> {
        (self.ready && self.menu.is_none() && self.track.as_ref().is_some_and(|track|
            track.visible && track.compatibility == api::CompatibilityState::Playable))
            .then_some(playback_page::TIMELINE)
    }

    pub(super) fn press(&mut self, input: Input) {
        if let (Some(menu), Some(track)) = (&mut self.menu, &self.track) {
            if let Some(command) = menu.press(track, input) {
                self.command(command);
            }
            return;
        }
        let control = match input {
            Input::A => Control::Pause,
            Input::B => Control::CancelJump,
            Input::X => Control::Mark,
            Input::Y => Control::Camera,
            Input::Start => Control::Menu,
            Input::Back => Control::Hide,
            Input::Up => Control::Screenshot,
            Input::Down => Control::Step,
            Input::Left => Control::Skip(-5),
            Input::Right => Control::Skip(5),
        };
        self.control(control);
    }

    fn command(&mut self, command: Command) {
        let Some(track) = &mut self.track else { return };
        match command {
            Command::Resume => {
                self.menu = None;
                self.send(api::PLAY, json!({}), Request::Pause);
            }
            Command::Restart => {
                self.menu = None;
                let start = track.bounds().0;
                self.send(api::PLAY, json!({}), Request::Pause);
                self.seek(start);
            }
            Command::Player(player) => {
                track.selected_player = player;
                if !matches!(track.camera, CameraMode::FirstPerson | CameraMode::Follow | CameraMode::Orbit) {
                    track.camera = CameraMode::FirstPerson;
                }
                self.send_camera([0.; 3], [0.; 2], 0.);
            }
            Command::Camera(mode) => {
                track.camera = mode;
                track.camera_missing_view = false;
                self.send_camera([0.; 3], [0.; 2], 0.);
            }
            Command::Screenshot => self.screenshot = true,
            Command::MarkIn => track.marks = (Some(track.tick), None),
            Command::MarkOut => self.completed_clip = track.mark_out(),
            Command::ClearMarks => track.marks = (None, None),
            Command::Export(format, fps) => {
                if self.export.is_some() || self.pending.values().any(|r| matches!(r, Request::Export)) {
                    self.notice = Some("A clip is already exporting".into());
                    return;
                }
                if let Some([from, to]) = track.clip_range() {
                    self.send("theater.clip.export", json!({ "from": from, "to": to, "format": format, "fps": fps }), Request::Export);
                }
            }
            Command::Save => self.save_requested = true,
            Command::Share(visibility) => {
                let id = track.film.clone();
                let clip = track.clip_range();
                self.send("theater.share", json!({ "id": id, "visibility": visibility, "clip": clip }), Request::Share);
            }
            Command::End => self.leave_requested = true,
            Command::Cancel => {}
        }
    }

    pub fn control(&mut self, c: Control) {
        let Some(track) = &mut self.track else { return };
        match c {
            Control::Hide => {
                let shown = !track.visible;
                self.set_hud(None, Some(shown));
            }
            Control::Leave => {
                if track.seeking {
                    self.control(Control::CancelJump);
                } else {
                    self.press(if self.menu.is_some() { Input::B } else { Input::Start });
                }
            }
            _ if !self.ready => {}
            Control::Menu if !track.seeking => {
                self.menu = Some(Menu::default());
                self.key_skip = SkipHold::default();
                self.mouse_look = [0.0; 2];
                self.mouse_zoom = 0.0;
                self.send(api::PAUSE, json!({}), Request::Pause);
            }
            Control::CancelJump if track.seeking => {
                self.send(api::SEEK, json!({ "cancel": true }), Request::Seek);
            }
            Control::Step if track.paused && !track.seeking => {
                self.send("theater.step", json!({"ticks": 1}), Request::Command);
            }
            Control::Menu | Control::CancelJump | Control::Step => {}
            Control::Pause => {
                if !self.pending.values().any(|r| matches!(r, Request::Pause)) {
                    let method = if track.paused {
                        agent_proto::theater::PLAY
                    } else {
                        agent_proto::theater::PAUSE
                    };
                    self.send(method, json!({}), Request::Pause);
                }
            }
            Control::Skip(seconds) => {
                let tick = track.skip(seconds);
                self.seek(tick);
            }
            Control::Seek(tick) => self.seek(tick),
            Control::Camera => {
                track.cycle_camera();
                self.mouse_look = [0.0; 2];
                self.mouse_zoom = 0.0;
                self.camera_dirty = true;
            }
            Control::Speed(up) => {
                let speed = step(track.speed, SPEEDS.len(), up);
                if speed != track.speed {
                    self.send(
                        agent_proto::theater::SPEED,
                        json!({ "speed": SPEEDS[speed] }),
                        Request::Speed(speed),
                    );
                }
            }
            Control::Player(down) => {
                track.select_player(down);
                self.camera_dirty |= matches!(
                    track.camera,
                    CameraMode::Orbit | CameraMode::Follow | CameraMode::FirstPerson
                );
            }
            Control::Hud => {
                let shown = !track.hud;
                self.set_hud(Some(shown), None);
            }
            Control::Mark => {
                if let Some(clip) = track.mark() {
                    self.completed_clip = Some(clip);
                }
            }
            Control::Screenshot => self.screenshot = true,
        }
    }

    fn set_hud(&mut self, game: Option<bool>, bar: Option<bool>) {
        if self.pending.values().any(|r| matches!(r, Request::Hud { .. } | Request::LegacyHud(_))) {
            return;
        }
        if self.hud_unavailable {
            if let Some(track) = &mut self.track {
                track.visible = bar.unwrap_or(track.visible);
            }
            if let Some(game) = game {
                self.send(CHEAT_SET, json!({"name": "hide_hud", "value": !game}), Request::LegacyHud(game));
            }
        } else {
            self.send("theater.hud", json!({ "game": game, "bar": bar }), Request::Hud { game, bar });
        }
    }

    pub fn owns_pads(&self) -> bool {
        self.track.is_some()
    }

    pub fn key(&mut self, key: Key, down: bool, repeat: bool) -> bool {
        if !down && self.held_keys.remove(&key) {
            if matches!(key, Key::Left | Key::Right)
                && let Some(seconds) = self.update_key_skip()
            {
                self.control(Control::Skip(seconds));
            }
            return true;
        }
        if down && self.held_keys.contains(&key) {
            return true;
        }
        if self.track.is_none() {
            return false;
        }
        if !down || repeat || !self.held_keys.insert(key) {
            return true;
        }
        if self.menu.is_some() {
            if let Some(input) = menu_key(key) {
                self.press(input);
            }
            return true;
        }
        let c = match key {
            Key::Space => Control::Pause,
            Key::Slower => Control::Speed(false),
            Key::Faster => Control::Speed(true),
            Key::Up => Control::Player(false),
            Key::Down => Control::Player(true),
            Key::Hide => Control::Hide,
            Key::Hud => Control::Hud,
            Key::Clip => Control::Mark,
            Key::Screenshot => Control::Screenshot,
            Key::Camera => Control::Camera,
            Key::Leave => Control::Leave,
            Key::Accept => return true,
            Key::Step => Control::Step,
            Key::Forward
            | Key::Backward
            | Key::StrafeLeft
            | Key::StrafeRight
            | Key::Rise
            | Key::Fall => return true,
            Key::Left | Key::Right => {
                if let Some(seconds) = self.update_key_skip() {
                    self.control(Control::Skip(seconds));
                }
                return true;
            }
        };
        self.control(c);
        true
    }

    fn seek(&mut self, tick: u64) {
        if !self.pending.values().any(|r| matches!(r, Request::Seek)) {
            let clip_marks = self
                .track
                .as_ref()
                .map(|track| [track.marks.0, track.marks.1]);
            self.send(api::SEEK, json!({"tick": tick, "clip_marks": clip_marks}), Request::Seek);
        }
    }

    pub fn scrub(&mut self, fraction: f32) -> bool {
        if let Some(track) = &self.track
            && self.ready
            && self.menu.is_none()
            && track.visible
            && track.compatibility == api::CompatibilityState::Playable
            && fraction.is_finite()
        {
            let (start, end) = track.bounds();
            let offset = (f64::from(fraction.clamp(0.0, 1.0)) * (end - start) as f64).round();
            self.control(Control::Seek(start.saturating_add(offset as u64).min(end)));
            return true;
        }
        false
    }

    pub fn mouse_look(&mut self, dx: f64, dy: f64) -> bool {
        if self.menu.is_none() && self
            .track
            .as_ref()
            .is_some_and(|t| matches!(t.camera, CameraMode::Free | CameraMode::Orbit))
        {
            if dx.is_finite() && dy.is_finite() {
                self.mouse_look[0] = (self.mouse_look[0] - dx as f32 * 0.004)
                    .clamp(-std::f32::consts::PI, std::f32::consts::PI);
                self.mouse_look[1] = (self.mouse_look[1] - dy as f32 * 0.004)
                    .clamp(-std::f32::consts::PI, std::f32::consts::PI);
            }
            return true;
        }
        false
    }

    pub fn mouse_zoom(&mut self, amount: f32) -> bool {
        if self.menu.is_none() && self
            .track
            .as_ref()
            .is_some_and(|t| t.camera == CameraMode::Orbit)
            && amount.is_finite()
        {
            self.mouse_zoom = (self.mouse_zoom - 0.5 * amount).clamp(-12.0, 12.0);
            return true;
        }
        false
    }

    fn update_camera(&mut self, now: Instant) {
        let previous = self.last_motion.replace(now).unwrap_or(now);
        let dt = now
            .saturating_duration_since(previous)
            .as_secs_f32()
            .min(0.1);
        let Some(track) = &self.track else { return };
        if !self.ready || self.pending.values().any(|r| matches!(r, Request::Camera)) {
            return;
        }
        let held = |key| f32::from(u8::from(self.held_keys.contains(&key)));
        let forward =
            (self.pad_motion.forward + held(Key::Forward) - held(Key::Backward)).clamp(-1.0, 1.0);
        let strafe = (self.pad_motion.strafe + held(Key::StrafeRight) - held(Key::StrafeLeft))
            .clamp(-1.0, 1.0);
        let rise = (self.pad_motion.rise + held(Key::Rise) - held(Key::Fall)).clamp(-1.0, 1.0);
        let (mut movement, mut look, mut zoom) = ([0.0; 3], [0.0; 2], 0.0);
        if matches!(track.camera, CameraMode::Free | CameraMode::Orbit) {
            look = [
                self.mouse_look[0] - dt * 2.0 * self.pad_motion.look[0],
                self.mouse_look[1] + dt * 2.0 * self.pad_motion.look[1],
            ];
            if track.camera == CameraMode::Free {
                movement = [strafe, forward, rise].map(|axis| axis * dt * 6.0);
            } else {
                zoom = self.mouse_zoom - forward * dt * 6.0;
            }
        }
        self.mouse_look = [0.0; 2];
        self.mouse_zoom = 0.0;
        if self.camera_dirty || movement != [0.0; 3] || look != [0.0; 2] || zoom != 0.0 {
            self.camera_dirty = false;
            self.send_camera(movement, look, zoom);
        }
    }

    fn send_camera(&mut self, movement: [f32; 3], look: [f32; 2], zoom: f32) {
        let Some(track) = &self.track else { return };
        let player = match track.camera {
            CameraMode::Orbit | CameraMode::Follow | CameraMode::FirstPerson => track.camera_target(),
            CameraMode::Default | CameraMode::Free => None,
        };
        let params = json!({
            "mode": track.camera,
            "player": player.map(|p| &p.name),
            "seat": player.and_then(|p| p.seat),
            "movement": movement,
            "look": look,
            "zoom": zoom
        });
        self.send(api::CAMERA, params, Request::Camera);
    }

    fn update_key_skip(&mut self) -> Option<i64> {
        let left = i64::from(self.held_keys.contains(&Key::Left));
        let right = i64::from(self.held_keys.contains(&Key::Right));
        self.key_skip.update(right - left, Instant::now())
    }

    pub fn release_keys(&mut self) {
        self.held_keys.clear();
        self.key_skip = SkipHold::default();
        self.mouse_look = [0.0; 2];
        self.mouse_zoom = 0.0;
    }
}

fn menu_key(key: Key) -> Option<Input> {
    Some(match key {
        Key::Up => Input::Up,
        Key::Down => Input::Down,
        Key::Left => Input::Left,
        Key::Right => Input::Right,
        Key::Accept | Key::Space => Input::A,
        Key::Leave => Input::B,
        _ => return None,
    })
}

fn menu_button(button: HostButton) -> Option<Input> {
    Some(match button {
        HostButton::South => Input::A,
        HostButton::East => Input::B,
        HostButton::Start => Input::Start,
        HostButton::Back => Input::Back,
        HostButton::DpadUp => Input::Up,
        HostButton::DpadDown => Input::Down,
        HostButton::DpadLeft => Input::Left,
        HostButton::DpadRight => Input::Right,
        _ => return None,
    })
}

#[derive(Default)]
struct SkipHold(Option<(i64, Instant, bool)>);

impl SkipHold {
    fn held(&mut self, now: Instant) -> Option<i64> {
        let (dir, since, fired) = self.0.as_mut()?;
        if !*fired && now.duration_since(*since) >= LONG_PRESS {
            *fired = true;
            Some(*dir * 30)
        } else {
            None
        }
    }

    fn update(&mut self, dir: i64, now: Instant) -> Option<i64> {
        let mut skip = self.held(now);
        if self.0.is_some_and(|(was, _, _)| was == dir) {
            return skip;
        }
        if let Some((old, _, false)) = self.0.take() {
            skip = Some(old * 5);
        }
        if dir != 0 {
            self.0 = Some((dir, now, false));
        }
        skip
    }
}

#[derive(Default)]
struct Drive {
    strafe: f32,
    forward: f32,
    rise: f32,
    look: [f32; 2],
}

impl Drive {
    fn pad(state: &HostPadState) -> Self {
        let axis = |v: f32| {
            if v.is_finite() && v.abs() > 0.18 {
                v.signum() * ((v.abs().min(1.0) - 0.18) / 0.82)
            } else {
                0.0
            }
        };
        let down = |button| f32::from(u8::from(state.buttons.contains(button)));
        Self {
            strafe: axis(state.left_stick.0),
            forward: axis(state.left_stick.1),
            rise: down(HostButton::RightStick) - down(HostButton::LeftStick),
            look: [axis(state.right_stick.0), axis(state.right_stick.1)],
        }
    }
}

#[derive(Default)]
struct Pad {
    buttons: HostButtons,
    triggers: (bool, bool),
    skip: SkipHold,
    latched: bool,
}

pub(super) struct Gate {
    theater: Theater,
    next: Option<Box<dyn PadGate>>,
    pads: HashMap<(usize, HostPadId), Pad>,
    active: bool,
}

impl Gate {
    pub fn new(theater: Theater, next: Option<Box<dyn PadGate>>) -> Self {
        Self {
            theater,
            next,
            pads: HashMap::new(),
            active: false,
        }
    }
}

impl PadGate for Gate {
    fn filter(&mut self, pads: &mut [GatedPad]) {
        let now = Instant::now();
        let mut session = self.theater.session();
        let active = session.owns_pads();
        let entering = active && !self.active;
        let leaving = self.active && !active;
        self.active = active;
        self.pads.retain(|k, _| pads.iter().any(|p| p.key == *k));
        session.pad_motion = if active && !entering && session.menu.is_none() {
            pads.iter()
                .find(|p| p.pad.physical)
                .map_or_else(Drive::default, |p| Drive::pad(&p.pad.state))
        } else {
            Drive::default()
        };
        for pad in pads.iter_mut().filter(|p| p.pad.physical) {
            let track = self.pads.entry(pad.key).or_default();
            let state = &mut pad.pad.state;
            let buttons = state.buttons;
            let triggers = (state.left_trigger >= 0.35, state.right_trigger >= 0.35);
            if active && !entering && session.menu.is_some() {
                for button in [
                    HostButton::South, HostButton::East, HostButton::Start, HostButton::Back,
                    HostButton::DpadUp, HostButton::DpadDown, HostButton::DpadLeft, HostButton::DpadRight,
                ] {
                    if buttons.contains(button) && !track.buttons.contains(button) {
                        session.press(menu_button(button).unwrap());
                        break;
                    }
                }
                track.skip = SkipHold::default();
            } else if active && !entering {
                for (button, c) in [
                    (HostButton::South, Control::Pause),
                    (HostButton::East, Control::CancelJump),
                    (HostButton::West, Control::Mark),
                    (HostButton::Back, Control::Hide),
                    (HostButton::North, Control::Camera),
                    (HostButton::Start, Control::Menu),
                    (HostButton::LeftBumper, Control::Player(false)),
                    (HostButton::RightBumper, Control::Player(true)),
                    (HostButton::DpadUp, Control::Screenshot),
                    (HostButton::DpadDown, Control::Step),
                ] {
                    if buttons.contains(button) && !track.buttons.contains(button) {
                        session.control(c);
                    }
                }
                let hud = |buttons: HostButtons| {
                    buttons.contains(HostButton::LeftStick) && buttons.contains(HostButton::RightStick)
                };
                if hud(buttons) && !hud(track.buttons) {
                    session.control(Control::Hud);
                }
                if triggers.0 && !track.triggers.0 {
                    session.control(Control::Speed(false));
                }
                if triggers.1 && !track.triggers.1 {
                    session.control(Control::Speed(true));
                }
                let dir = i64::from(buttons.contains(HostButton::DpadRight))
                    - i64::from(buttons.contains(HostButton::DpadLeft));
                if let Some(seconds) = track.skip.update(dir, now) {
                    session.control(Control::Skip(seconds));
                }
            } else {
                track.skip = SkipHold::default();
            }
            track.buttons = buttons;
            track.triggers = triggers;
            if leaving {
                track.latched = true;
            }
            if track.latched && !active {
                track.latched = buttons.bits() != 0 || triggers.0 || triggers.1;
                if track.latched {
                    neutral(state);
                }
            }
        }
        drop(session);
        if active {
            for pad in pads {
                neutral(&mut pad.pad.state);
            }
        } else if let Some(next) = &mut self.next {
            next.filter(pads);
        }
    }
}

fn neutral(state: &mut HostPadState) {
    *state = HostPadState {
        prefiltered: state.prefiltered,
        ..HostPadState::default()
    };
}

#[cfg(test)]
mod tests {
    use super::super::adapter::{FilmMeta, Source};
    use super::*;

    fn sample_track() -> Track {
        Track::from_entry(&Entry {
            film: FilmMeta {
                id: "test".into(),
                map: "Lockout".into(),
                variant_name: "Slayer".into(),
                length_ticks: 3000,
                source: Source::Host,
                image_stamp: "test".into(),
                compatibility: api::CompatibilityState::Playable,
                ..FilmMeta::default()
            },
            clip: None,
            archive_clip: None,
        })
    }

    #[test]
    fn theater_playback_state_and_clip_marks_follow_the_agent_tick() {
        let mut track = sample_track();
        track.read(
            serde_json::from_value(json!({
                "paused": true, "tick": 450, "speed": 0.5
            }))
            .unwrap(),
        );
        assert!(track.paused);
        assert_eq!(track.skip(-30), 0);
        assert_eq!(track.skip(5), 600);
        assert_eq!(track.speed, 1);
        assert!(track.mark().is_none());
        track.tick = 930;
        assert_eq!(
            track.mark(),
            Some(Clip {
                film: "test".into(),
                from_tick: 450,
                to_tick: 930
            })
        );
        assert_eq!(track.clip_range(), Some([450, 930]));
    }

    #[test]
    fn first_person_uses_host_recordings_or_recorded_local_viewports() {
        let mut track = sample_track();
        assert_eq!(track.camera_label(), "Fly");
        for mode in [
            CameraMode::Orbit,
            CameraMode::Follow,
            CameraMode::FirstPerson,
        ] {
            track.cycle_camera();
            assert_eq!(track.camera, mode);
        }
        track.source = Source::Local;
        track.camera = CameraMode::Default;
        track.cycle_camera();
        assert_eq!(track.camera, CameraMode::Default);
        assert_eq!(track.camera_label(), "Recorded view");
        track.local_viewports = true;
        track.cycle_camera();
        assert_eq!(track.camera, CameraMode::FirstPerson);
        track.cycle_camera();
        assert_eq!(track.camera, CameraMode::Default);
    }

    #[cfg(unix)]
    #[test]
    fn theater_overlay_sends_controls_to_the_playback_agent_socket() {
        use agent::{Access, Agent, GameStatus, Reader, Server};
        use agent_proto::{Endpoint, Object, theater as api};

        struct Empty;
        impl Reader for Empty {
            fn status(&mut self) -> GameStatus {
                GameStatus::default()
            }

            fn objects(&mut self) -> Result<Vec<Object>, String> {
                Ok(Vec::new())
            }
        }

        let agent = Agent::new();
        let (calls, received) = std::sync::mpsc::channel();
        agent.add_method(api::STATE, Access::Read, |_| Ok(json!({"tick": 60})));
        for method in [api::PAUSE, api::SPEED, api::SEEK, api::CAMERA, CHEAT_SET] {
            let calls = calls.clone();
            agent.add_method(method, Access::Act, move |params| {
                calls.send((method, params)).unwrap();
                Ok(json!({"paused": true}))
            });
        }
        let path = std::env::temp_dir().join(format!("h2t-{}.sock", live_proto::sign::random_id()));
        let server = Server::listen_on(&Endpoint::Unix(path.clone()), agent.clone()).unwrap();
        let mut track = sample_track();
        track.players = ["A", "B"]
            .map(|name| Player {
                name: name.into(),
                ..Player::default()
            })
            .to_vec();
        let mut session = Session {
            track: Some(track),
            rpc: Some(Playback::socket(path).unwrap()),
            ..Session::default()
        };
        let drive = |session: &mut Session| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                session.pump();
                agent.poll_theater(&mut Empty);
                if session.ready && session.pending.is_empty() {
                    break;
                }
                assert!(Instant::now() < deadline, "playback RPC did not complete");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        drive(&mut session);
        session.control(Control::Player(true));
        assert_eq!(session.track.as_ref().unwrap().selected_player, 1);
        assert!(!session.camera_dirty, "fly mode has no target player");
        session.control(Control::Camera);
        drive(&mut session);
        let (method, params) = received.try_recv().unwrap();
        assert_eq!(method, api::CAMERA);
        assert_eq!(params["mode"], "orbit");
        assert_eq!(params["player"], "B");
        session.control(Control::Player(true));
        drive(&mut session);
        let (method, params) = received.try_recv().unwrap();
        assert_eq!(method, api::CAMERA);
        assert_eq!(params["player"], "A");
        session.control(Control::Pause);
        session.control(Control::Speed(true));
        session.control(Control::Skip(5));
        session.control(Control::Hud);
        drive(&mut session);
        assert_eq!(
            received.try_iter().collect::<Vec<_>>(),
            [
                (api::PAUSE, json!({})),
                (api::SPEED, json!({"speed": 2.0})),
                (api::SEEK, json!({"tick": 210, "clip_marks": [null, null]})),
                (CHEAT_SET, json!({"name": "hide_hud", "value": true})),
            ]
        );
        assert!(!session.track.as_ref().unwrap().hud);
        agent.shutdown();
        drop(server);
    }

    #[test]
    fn playback_page_input_reports_bar_pause_and_picker_via_agent() {
        struct Empty;
        impl agent::Reader for Empty {
            fn status(&mut self) -> agent::GameStatus { Default::default() }
            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> { Ok(vec![]) }
        }

        let theater = Theater::default();
        *theater.session() = Session { track: Some(sample_track()), ready: true, ..Default::default() };
        let agent = agent::Agent::new();
        theater.add_playback_methods(&agent);
        let (send, receive) = std::sync::mpsc::sync_channel(8);
        let client = agent.connect(send);
        let call = |method, params| {
            agent.submit(client, &json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params}).to_string());
            agent.poll_theater(&mut Empty);
            serde_json::from_str::<Value>(&receive.try_recv().unwrap()).unwrap()
        };

        assert_eq!(call(agent_proto::method::UI_SCREEN, json!({}))["result"]["screen"], super::super::OVERLAY);
        let press = |input| call(agent_proto::method::UI_INPUT, json!({"input": input}));
        assert_eq!(press("start")["result"]["screen"], super::super::PAUSE_MENU);
        for _ in 0..3 { press("down"); }
        let picker = press("a");
        assert_eq!(picker["result"]["screen"], super::super::PAUSE_MENU);
        assert_eq!(picker["result"]["dialog"]["name"], crate::ui_kit::picker::GAME_NAME);
        assert_eq!(press("b")["result"]["focus"], 3);
        assert_eq!(press("b")["result"]["screen"], super::super::OVERLAY);
        assert!(press("unknown")["error"].is_object());
    }

    #[test]
    fn keyboard_and_controller_use_the_same_navigation_for_all_pause_rows() {
        let mut t = sample_track();
        t.tick = 60;
        t.marks = (Some(10), Some(30));
        t.players.push(Player { name: "Chief".into(), ..Default::default() });
        for row in 0..10 {
            let mut keyboard = Menu::default();
            let mut controller = Menu::default();
            for _ in 0..row {
                keyboard.press(&t, menu_key(Key::Down).unwrap());
                controller.press(&t, menu_button(HostButton::DpadDown).unwrap());
            }
            assert_eq!(keyboard.spec(&t, None).focus, row);
            assert_eq!(
                keyboard.press(&t, menu_key(Key::Accept).unwrap()),
                controller.press(&t, menu_button(HostButton::South).unwrap()),
            );
            assert_eq!(keyboard.spec(&t, None), controller.spec(&t, None));
        }
    }

    #[test]
    fn skip_hold_sends_one_short_or_long_request() {
        let now = Instant::now();
        let mut d = SkipHold::default();
        assert_eq!(d.update(-1, now), None);
        assert_eq!(d.update(0, now + Duration::from_millis(100)), Some(-5));
        assert_eq!(d.update(1, now), None);
        assert_eq!(d.held(now + LONG_PRESS), Some(30));
        assert_eq!(d.held(now + LONG_PRESS * 2), None);
        assert_eq!(d.update(0, now + LONG_PRESS * 2), None);
    }
}
