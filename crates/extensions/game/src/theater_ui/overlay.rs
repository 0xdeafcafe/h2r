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
use super::{Entry, Theater, playback_page::{self, Command, Divergence, Jump, Menu}};
use crate::ui_kit::{input::Input, spec::ScreenSpec};
use crate::theater::SeekTaps;

const REFRESH: Duration = Duration::from_millis(150);
/// How long a film queued in this game may take to start before the viewer
/// is given the film page back (`Session::stall`).
const START_WAIT: Duration = Duration::from_secs(60);
const LONG_PRESS: Duration = Duration::from_millis(500);
/// Ticks the bar keeps a monitor look sentence: six seconds of film.
const MONITOR_NOTICE_TICKS: u64 = 180;
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
    camera_fallback: Option<api::CameraFallback>,
    #[serde(default)]
    monitor_looks: Vec<api::MonitorLookSource>,
    players: Option<Vec<Player>>,
    kills: Option<Vec<u64>>,
    bookmarks: Option<Vec<u64>>,
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
    pub bookmarks: Vec<u64>,
    pub jump: Option<Jump>,
    pub seeking: bool,
    pub divergence: Option<Divergence>,
    /// The seats whose unseen look the bar has said, and the sentence of
    /// the latest with the tick it was said at.
    monitor_seen: std::collections::BTreeSet<u8>,
    monitor_notice: Option<(String, u64)>,
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
            bookmarks: Vec::new(),
            jump: None,
            seeking: false,
            divergence: None,
            monitor_seen: std::collections::BTreeSet::new(),
            monitor_notice: None,
            range: entry
                .clip
                .as_ref()
                .map(|clip| (clip.from_tick, clip.to_tick)),
        }
    }

    /// A look the film recorded that this machine cannot draw: the bar says
    /// it once, when its seat first appears.
    pub(super) fn note_monitor_look(&mut self, look: &api::MonitorLookSource) {
        if self.monitor_seen.insert(look.seat) {
            self.monitor_notice = Some((super::adapter::monitor_sentence(look), self.tick));
        }
    }

    /// The sentence for a monitor look this machine cannot draw, for a few
    /// seconds after its seat first appears.
    pub(super) fn monitor_notice(&self) -> Option<&str> {
        let (sentence, at) = self.monitor_notice.as_ref()?;
        let shown = self.tick >= *at && self.tick - *at < MONITOR_NOTICE_TICKS;
        shown.then_some(sentence.as_str())
    }

    pub(super) fn bounds(&self) -> (u64, u64) {
        self.range
            .map(|(from, to)| (from, to.saturating_sub(1).max(from)))
            .unwrap_or((0, self.length))
    }

    fn skip(&self, seconds: i64) -> u64 {
        let delta = seconds.saturating_mul(self.tick_rate as i64);
        let (lo, hi) = self.bounds();
        self.seek_origin().saturating_add_signed(delta).clamp(lo, hi)
    }

    fn seek_origin(&self) -> u64 {
        self.jump.filter(|_| self.seeking).map_or(self.tick, |jump| jump.target)
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
        if let Some(players) = s.players.filter(|players| !players.is_empty()) {
            self.players = players;
            self.selected_player = self.selected_player.min(self.players.len() - 1);
        }
        if let Some(kills) = s.kills {
            self.kills.extend(kills);
            self.kills.sort_unstable();
            self.kills.dedup();
        }
        for look in &s.monitor_looks {
            self.note_monitor_look(look);
        }
        if let Some(bookmarks) = s.bookmarks {
            self.bookmarks = bookmarks;
        }
        if let Some(camera) = s.camera {
            self.camera_missing_view = s.camera_fallback == Some(api::CameraFallback::NoRecordedViewport);
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
            Control::CancelJump => {
                self.seeking = false;
                self.jump = None;
            }
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
    /// `theater.end`: leave the film this game is playing.
    End,
    State,
    Step,
    Export,
    ExportStatus,
    Share,
    Pause,
    Seek,
    Cancel,
    Speed(usize),
    Hud { game: Option<bool>, bar: Option<bool> },
    LegacyHud(bool),
    Camera,
}

/// What the loading panel shows: the film, its map, how long the load has
/// run and the status the transport reports.
pub(super) struct Loading {
    pub name: String,
    pub map: String,
    pub since: Instant,
    pub status: Option<String>,
}

impl Loading {
    fn of(entry: &Entry, now: Instant) -> Self {
        let film = &entry.film;
        Self {
            name: if film.variant_name.is_empty() { film.id.clone() } else { film.variant_name.clone() },
            map: film.map.clone(),
            since: now,
            status: None,
        }
    }
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
    hud_unavailable: bool,
    pub save_requested: bool,
    export: Option<String>,
    export_poll: Option<Instant>,
    seeks: SeekTaps,
    starting_seek: Option<u64>,
    cancel_on_connect: bool,
    pub open_error: Option<String>,
    pub screenshot: bool,
    pub leave_requested: bool,
    /// The film `theater.watch` queued, until its answer comes.
    opening: Option<Entry>,
    /// The film plays in this game (`theater.watch`), not in a player
    /// process: leaving is `theater.end`.
    in_game: bool,
    /// When the film's game was queued, while it has not yet started.
    opened_at: Option<Instant>,
    /// The loading panel's facts, from the queued game's answer to the
    /// film's first answer of the transport.
    loading: Option<Loading>,
    /// The film whose load failed, until the host hands the viewer back.
    pub failed_film: Option<String>,
    pub completed_clip: Option<Clip>,
    held_keys: HashSet<Key>,
    key_skip: SkipHold,
    camera_dirty: bool,
    last_motion: Option<Instant>,
    pad_motion: Drive,
    mouse_look: [f32; 2],
    mouse_zoom: f32,
    /// What the viewer last drove the film with: the page's legend shows
    /// its buttons.
    pub last_device: Device,
}

/// The kind of input the playback page's legend shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Device {
    #[default]
    Pad,
    Keyboard,
}

impl Session {
    pub fn connect(&mut self, agent: &agent::Agent) {
        self.rpc = Some(Playback::new(agent));
    }

    pub fn play(
        &mut self,
        entry: &Entry,
        view: Option<agent_proto::theater::InitialView>,
    ) -> Result<(), String> {
        if self.pending.values().any(|r| matches!(r, Request::Open)) {
            return Err("A film is already opening or playing".into());
        }
        let rpc = self.rpc.as_mut().ok_or("Playback agent is unavailable")?;
        // A clip opens at its first tick; the track keeps the viewer in it.
        let from = entry
            .clip
            .as_ref()
            .and_then(|clip| u32::try_from(clip.from_tick).ok())
            .or(view.and_then(|v| v.tick));
        let id = rpc.send(
            agent_proto::theater::WATCH,
            json!({
                "id": entry.film.id,
                "tick": from,
                "seat": view.and_then(|v| v.seat),
                "camera": view.and_then(|v| v.camera),
            }),
        );
        self.opening = Some(entry.clone());
        self.pending.insert(id, Request::Open);
        Ok(())
    }

    pub fn playback(
        entry: &Entry,
        rpc: Playback,
        [mark_in, mark_out]: [Option<u64>; 2],
        starting_seek: Option<u64>,
    ) -> Self {
        let mut track = Track::from_entry(entry);
        track.marks = (mark_in, mark_out);
        if let Some(target) = starting_seek {
            track.seeking = true;
            track.jump = Some(Jump { target, from_tick: 0, remaining_ticks: target });
        }
        Self {
            track: Some(track),
            rpc: Some(rpc),
            starting_seek,
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
        self.stall(now);
        self.pump_transport(now);
        if let Some(seconds) = self.key_skip.held(now) {
            self.control_at(Control::Skip(seconds), now);
        }
        if self.menu.is_none() {
            self.update_camera(now);
        }
        if let Some(job) = &self.export
            && self.export_poll.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1))
            && !self.pending.values().any(|r| matches!(r, Request::ExportStatus))
        {
            let job = job.clone();
            self.export_poll = Some(now);
            self.send(api::CLIP_STATUS, json!({"job": job}), Request::ExportStatus);
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

    /// A film queued in this game whose game has not started within
    /// [`START_WAIT`] is given up: the bar goes, the film is disarmed
    /// (`theater.end`) and the film page says why.
    fn stall(&mut self, now: Instant) -> bool {
        let waiting = self.in_game
            && !self.ready
            && self.opened_at.is_some_and(|at| now.duration_since(at) >= START_WAIT);
        if !waiting {
            return false;
        }
        self.fail_load("The film's game did not start".into());
        true
    }

    /// The film's load failed: the loading state and the bar go, the film
    /// is disarmed (`theater.end`), and the host opens the film page again
    /// with `why`.
    pub fn fail_load(&mut self, why: String) {
        self.failed_film = self.track.as_ref().map(|t| t.film.clone());
        self.opened_at = None;
        self.loading = None;
        self.track = None;
        self.in_game = false;
        self.ready = false;
        self.menu = None;
        self.open_error = Some(why);
        self.send(api::END, json!({}), Request::End);
    }

    fn pump_transport(&mut self, now: Instant) {
        if !self.ready
            || self
                .pending
                .values()
                .any(|r| matches!(r, Request::Seek | Request::Cancel))
        {
            return;
        }
        if std::mem::take(&mut self.cancel_on_connect) {
            if self
                .track
                .as_ref()
                .is_some_and(|track| track.seeking)
            {
                self.send(api::SEEK, json!({"cancel": true}), Request::Cancel);
            }
        } else if let Some(target) = self.seeks.take_due(now) {
            self.seek(target);
        }
    }

    fn answer(&mut self, request: Request, answer: Result<Value, String>) {
        match (request, answer) {
            (Request::Open, Ok(value)) if value["queued"].as_bool() == Some(false) => {
                self.opening = None;
                self.open_error = Some("The film's game could not be queued".into());
            }
            (Request::Open, Ok(_)) => {
                // The film's game is queued: the bar is this game's own.
                if let Some(entry) = self.opening.take() {
                    let now = Instant::now();
                    self.track = Some(Track::from_entry(&entry));
                    self.ready = false;
                    self.in_game = true;
                    self.opened_at = Some(now);
                    self.loading = Some(Loading::of(&entry, now));
                    self.menu = None;
                }
            }
            (Request::Open, Err(error)) => {
                self.opening = None;
                self.open_error = Some(error);
            }
            (Request::End, answer) => {
                if let Err(error) = answer {
                    self.notice = Some(error);
                } else {
                    // The menu is queued; the film page opens when it loads.
                    self.track = None;
                    self.loading = None;
                    self.ready = false;
                    self.in_game = false;
                    self.menu = None;
                }
            }
            (Request::State, Err(_)) if self.in_game && self.ready => {
                // The game left the film: it is no longer playing.
                self.track = None;
                self.ready = false;
                self.in_game = false;
                self.menu = None;
            }
            (Request::State, Ok(value)) if self.in_game && value["active"] == json!(false) => {
                // No film plays in this game: its game came back to the
                // menu before it started, or the film was left. The bar
                // goes and the film page (opened again at the menu) says
                // why.
                if !self.ready {
                    self.open_error = Some("The film's game did not load".into());
                }
                self.track = None;
                self.ready = false;
                self.in_game = false;
                self.opened_at = None;
                self.menu = None;
            }
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
                    self.loading = None;
                    self.starting_seek = None;
                    let seeking = s.seeking;
                    track.read(s);
                    if let Some((start, end)) = track.range
                        && !seeking
                        && !track.paused
                        && (track.tick < start || track.tick >= end)
                        && !self.seeks.is_pending()
                        && !self.cancel_on_connect
                        && !self
                            .pending
                            .values()
                            .any(|r| matches!(r, Request::Seek | Request::Cancel))
                    {
                        self.seek(start);
                    }
                }
            }
            (Request::State, Err(status)) if !self.ready && self.loading.is_some() => {
                // The transport's own word for the wait ("film … is
                // loading"); the panel says it. Other answers are the
                // menu's, and the panel keeps waiting.
                if status.contains("is loading")
                    && let Some(loading) = &mut self.loading
                {
                    loading.status = Some(status);
                }
            }
            (Request::State, Err(_)) if !self.ready || self.track.as_ref().is_some_and(|t| t.seeking) => {}
            (Request::Seek | Request::Cancel | Request::Step, Ok(value)) => {
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
            (Request::ExportStatus, Err(_)) if self.track.as_ref().is_some_and(|track| track.seeking) => {}
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
        if let Some(loading) = self.loading.as_ref().filter(|_| !self.ready) {
            return Some(playback_page::loading(loading, Instant::now()));
        }
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
                    self.send(api::CLIP_EXPORT, json!({ "from": from, "to": to, "format": format, "fps": fps }), Request::Export);
                }
            }
            Command::Save => self.save_requested = true,
            Command::Share(visibility) => {
                let id = track.film.clone();
                let clip = track.clip_range();
                self.send("theater.share", json!({ "id": id, "visibility": visibility, "clip": clip }), Request::Share);
            }
            Command::End if self.in_game => {
                if !self.pending.values().any(|r| matches!(r, Request::End)) {
                    self.send(api::END, json!({}), Request::End);
                }
            }
            Command::End => self.leave_requested = true,
            Command::Cancel => {}
        }
    }

    pub fn control(&mut self, c: Control) {
        self.control_at(c, Instant::now());
    }

    fn control_at(&mut self, c: Control, now: Instant) {
        let Some(track) = &mut self.track else { return };
        match c {
            Control::Hide => {
                let shown = !track.visible;
                self.set_hud(None, Some(shown));
            }
            Control::Leave | Control::CancelJump => {
                let had_taps = self.seeks.take().is_some();
                if track.seeking
                    || self.starting_seek.is_some()
                    || self.pending.values().any(|r| matches!(r, Request::Seek | Request::Cancel))
                {
                    self.cancel_on_connect = true;
                } else if c == Control::Leave && !had_taps {
                    self.press(if self.menu.is_some() { Input::B } else { Input::Start });
                }
            }
            _ if !self.ready => {}
            Control::Menu if !track.seeking => {
                self.seeks.take();
                self.menu = Some(Menu::default());
                self.key_skip = SkipHold::default();
                self.mouse_look = [0.0; 2];
                self.mouse_zoom = 0.0;
                self.send(api::PAUSE, json!({}), Request::Pause);
            }
            Control::Step => {
                if track.paused
                    && !track.seeking
                    && track.tick < track.bounds().1
                    && !self
                        .pending
                        .values()
                        .any(|r| matches!(r, Request::Step | Request::Seek | Request::Cancel))
                {
                    self.send(api::STEP, json!(api::Step { ticks: 1 }), Request::Step);
                }
            }
            Control::Menu => {}
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
                let delta = seconds.saturating_mul(track.tick_rate as i64);
                self.seeks
                    .push(track.seek_origin(), delta, track.bounds(), now);
                if seconds.unsigned_abs() == 30
                    && !self
                        .pending
                        .values()
                        .any(|r| matches!(r, Request::Seek | Request::Cancel))
                    && let Some(target) = self.seeks.take()
                {
                    self.seek(target);
                }
            }
            Control::Seek(tick) => {
                self.seeks.take();
                self.seek(tick);
            }
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
            self.send(api::HUD, json!({ "game": game, "bar": bar }), Request::Hud { game, bar });
        }
    }

    /// A film in this game owns the pads once its game is playing; while it
    /// is queued the menu keeps them.
    pub fn owns_pads(&self) -> bool {
        self.track.is_some() && (self.ready || !self.in_game)
    }

    /// A film is loaded and its transport answers: the bar is drawn.
    pub fn loaded(&self) -> bool {
        self.track.is_some() && self.ready
    }

    /// A film queued in this game has not played its first tick: the host
    /// draws the loading panel, not the bar.
    pub fn loading(&self) -> bool {
        self.track.is_some() && self.loading.is_some() && !self.ready
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
        if down {
            self.last_device = Device::Keyboard;
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
        if !self
            .pending
            .values()
            .any(|r| matches!(r, Request::Seek | Request::Cancel))
        {
            let clip_marks = self
                .track
                .as_ref()
                .map(|track| [track.marks.0, track.marks.1]);
            let params = api::Seek {
                tick: Some(tick),
                clip_marks,
                ..Default::default()
            };
            self.send(api::SEEK, json!(params), Request::Seek);
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
    fn moving(&self) -> bool {
        self.strafe != 0.0
            || self.forward != 0.0
            || self.rise != 0.0
            || self.look != [0.0; 2]
    }

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
        // Every pad drives the film, the desktop's player one too: it is the
        // first physical pad merged with the keyboard, reported as a pad
        // that is not physical, and the Theater's own keys never reach it.
        session.pad_motion = if active && !entering && session.menu.is_none() {
            pads.iter()
                .map(|p| Drive::pad(&p.pad.state))
                .find(|d| d.moving())
                .unwrap_or_default()
        } else {
            Drive::default()
        };
        for pad in pads.iter_mut() {
            let track = self.pads.entry(pad.key).or_default();
            let state = &mut pad.pad.state;
            let buttons = pressed(state);
            if active && (buttons.bits() & !track.buttons.bits() != 0 || Drive::pad(state).moving()) {
                session.last_device = Device::Pad;
            }
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

/// The buttons down on `state`: its digital ones and the face buttons and
/// bumpers a pad reports as pressures (the desktop's merged player one
/// reports those only as pressures).
fn pressed(state: &HostPadState) -> HostButtons {
    let mut buttons = state.buttons;
    if let Some(pressures) = state.analog_buttons {
        for (button, pressure) in [
            HostButton::South,
            HostButton::East,
            HostButton::West,
            HostButton::North,
            HostButton::RightBumper,
            HostButton::LeftBumper,
        ]
        .into_iter()
        .zip(pressures)
        {
            if pressure >= 0.25 {
                buttons.set(button, true);
            }
        }
    }
    buttons
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
    fn a_monitor_look_the_machine_cannot_draw_is_said_once_when_its_seat_appears() {
        let state = |tick: u64| -> AgentState {
            serde_json::from_value(json!({
                "tick": tick,
                "monitor_looks": [
                    {"seat": 2, "map": "invented_map", "reason": "monitor_look_source"}
                ]
            }))
            .unwrap()
        };
        let mut track = sample_track();
        track.read(AgentState::default());
        assert!(track.monitor_notice().is_none());
        track.read(state(300));
        let said = track.monitor_notice().expect("said when the seat appears").to_owned();
        assert!(said.contains("Seat 2") && said.contains("invented_map"), "{said}");
        // The state repeats the look; the bar does not say it again.
        track.tick = 300 + MONITOR_NOTICE_TICKS;
        assert!(track.monitor_notice().is_none());
        track.read(state(300 + MONITOR_NOTICE_TICKS));
        assert!(track.monitor_notice().is_none());
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
        for method in [api::PAUSE, api::SPEED, api::SEEK, api::CAMERA, api::HUD] {
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
        let now = Instant::now();
        for i in 0..5 {
            let tap = now + Duration::from_millis(i * 40);
            session.control_at(Control::Skip(5), tap);
            session.pump_transport(tap);
        }
        session.pump_transport(now + Duration::from_millis(410));
        session.control(Control::Hud);
        drive(&mut session);
        assert_eq!(
            received.try_iter().collect::<Vec<_>>(),
            [
                (api::PAUSE, json!({})),
                (api::SPEED, json!({"speed": 2.0})),
                (api::SEEK, json!({"tick": 810, "clip_marks": [null, null]})),
                (api::HUD, json!({"game": false, "bar": null})),
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

    /// The desktop's player one is the first pad merged with the keyboard,
    /// reported as a pad that is not physical, with A/B/X/Y and the bumpers
    /// as pressures: its presses drive the film, the guest sees it neutral,
    /// and the legend follows it.
    #[test]
    fn the_merged_player_one_pad_drives_the_film() {
        use input::host::HostPad;
        let theater = super::super::Theater::default();
        {
            let mut session = theater.session();
            let mut track = sample_track();
            track.players = ["A", "B"]
                .map(|name| Player {
                    name: name.into(),
                    ..Player::default()
                })
                .to_vec();
            session.track = Some(track);
            session.ready = true;
            session.last_device = Device::Keyboard;
        }
        let mut gate = Gate::new(theater.clone(), None);
        let pad = |pressures: [f32; 6], buttons: &[HostButton]| {
            let mut state = HostPadState {
                analog_buttons: Some(pressures),
                ..HostPadState::default()
            };
            for button in buttons {
                state.buttons.set(*button, true);
            }
            vec![GatedPad {
                key: (0, HostPadId(0)),
                pad: HostPad {
                    id: HostPadId(0),
                    name: "Player one (pad + keyboard/mouse)".into(),
                    state,
                    physical: false,
                    supports_rumble: true,
                },
            }]
        };
        let idle = [0.0; 6];
        // The first sync takes the pads; nothing fires on it.
        gate.filter(&mut pad(idle, &[]));
        assert_eq!(theater.session().last_device, Device::Keyboard);

        // RB (the sixth pressure is LB, the fifth RB): the next player.
        let mut pads = pad([0.0, 0.0, 0.0, 0.0, 1.0, 0.0], &[]);
        gate.filter(&mut pads);
        assert_eq!(pads[0].pad.state.analog_buttons, None, "the guest sees a neutral pad");
        {
            let session = theater.session();
            assert_eq!(session.track.as_ref().unwrap().selected_player, 1);
            assert_eq!(session.last_device, Device::Pad);
        }
        // Held, it does not fire again; LB goes back.
        gate.filter(&mut pad([0.0, 0.0, 0.0, 0.0, 1.0, 0.0], &[]));
        gate.filter(&mut pad([0.0, 0.0, 0.0, 0.0, 0.0, 1.0], &[]));
        assert_eq!(theater.session().track.as_ref().unwrap().selected_player, 0);

        // Y: the next camera.
        let camera = theater.session().track.as_ref().unwrap().camera;
        gate.filter(&mut pad([0.0, 0.0, 0.0, 1.0, 0.0, 0.0], &[]));
        assert_ne!(theater.session().track.as_ref().unwrap().camera, camera);

        // A key moves the legend back to the keyboard; Start opens the menu.
        theater.session().key(Key::Camera, true, false);
        assert_eq!(theater.session().last_device, Device::Keyboard);
        gate.filter(&mut pad(idle, &[HostButton::Start]));
        assert!(theater.session().menu.is_some());
        assert_eq!(theater.session().last_device, Device::Pad);
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
    #[test]
    fn a_film_queued_in_this_game_draws_no_bar_and_keeps_the_pads_until_it_plays() {
        let mut session = Session {
            track: Some(sample_track()),
            in_game: true,
            opened_at: Some(Instant::now()),
            ..Default::default()
        };
        assert!(!session.loaded(), "queued: the menu is still showing");
        assert!(!session.owns_pads());
        session.ready = true;
        assert!(session.loaded() && session.owns_pads());
    }

    fn queued() -> Session {
        let mut session = Session::default();
        session.opening = Some(Entry {
            film: super::super::adapter::FilmMeta {
                id: "test".into(),
                map: "Lockout".into(),
                variant_name: "Slayer".into(),
                ..Default::default()
            },
            clip: None,
            archive_clip: None,
        });
        session.answer(Request::Open, Ok(json!({"queued": true})));
        session
    }

    #[test]
    fn the_loading_panel_shows_from_watch_to_the_first_tick_then_the_bar_takes_over() {
        let mut session = queued();
        assert!(session.loading() && !session.loaded());
        let panel = session.view().expect("the panel");
        assert_eq!(panel.name, super::super::LOADING);
        // The transport's word for the wait is on the panel.
        session.answer(Request::State, Err("film test is loading".into()));
        assert!(session.loading() && !session.loaded(), "the menu's other answers change nothing");
        session.answer(Request::State, Err("no method".into()));
        assert_eq!(
            session.loading.as_ref().and_then(|l| l.status.as_deref()),
            Some("film test is loading")
        );
        session.answer(Request::State, Ok(json!({"tick": 1})));
        assert!(!session.loading() && session.loaded());
        assert_eq!(session.view().expect("the bar").name, super::super::OVERLAY);
    }

    #[test]
    fn the_bar_is_not_drawn_before_the_film_plays() {
        let session = queued();
        assert!(!session.loaded(), "the host draws the bar only for a loaded film");
        assert!(!Session::default().loaded() && !Session::default().loading());
    }

    #[test]
    fn a_failed_load_ends_the_loading_state_and_names_the_film_and_reason() {
        let mut session = queued();
        session.fail_load("The film's game could not start".into());
        assert!(!session.loading() && !session.loaded() && session.track.is_none());
        assert_eq!(session.failed_film.as_deref(), Some("test"));
        assert_eq!(session.open_error.as_deref(), Some("The film's game could not start"));
        assert!(session.view().is_none());
    }

    #[test]
    fn a_film_whose_game_never_starts_is_given_up_with_a_reason() {
        let start = Instant::now();
        let mut session = Session {
            track: Some(sample_track()),
            in_game: true,
            opened_at: Some(start),
            ..Default::default()
        };
        assert!(!session.stall(start + START_WAIT - Duration::from_secs(1)));
        assert!(session.track.is_some());
        assert!(session.stall(start + START_WAIT));
        assert!(session.track.is_none() && !session.in_game && !session.loading());
        assert_eq!(session.open_error.as_deref(), Some("The film's game did not start"));
    }

    #[test]
    fn a_queued_film_is_loaded_only_when_its_game_answers_the_transport() {
        let queued = || Session {
            track: Some(sample_track()),
            in_game: true,
            opened_at: Some(Instant::now()),
            ..Default::default()
        };
        // Loading: the game answers unavailable, and the menu keeps its
        // pads, draws no bar and its 60 s wait runs on.
        let mut session = queued();
        session.answer(Request::State, Err("film test is loading: its game has not started".into()));
        assert!(!session.loaded() && !session.owns_pads());
        assert!(session.opened_at.is_some() && session.open_error.is_none());

        // No film plays in the game (it came back to its menu): the bar
        // goes and the page says why, instead of a bar over the menu.
        session.answer(Request::State, Ok(json!({"id": null, "active": false})));
        assert!(session.track.is_none() && !session.in_game && !session.loaded());
        assert_eq!(session.open_error.as_deref(), Some("The film's game did not load"));
        assert!(!session.stall(Instant::now() + START_WAIT), "nothing is left waiting");

        // The film's game answers: playback.
        let mut session = queued();
        session.answer(Request::State, Ok(json!({"id": "test", "tick": 5, "length_ticks": 100})));
        assert!(session.loaded() && session.owns_pads());
        assert_eq!(session.track.as_ref().unwrap().tick, 5);
    }
}
