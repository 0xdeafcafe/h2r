//! Playback controls belong to the host. They never enter the recorded
//! input stream, including while the transport itself is hidden.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use agent_proto::theater::{self as api, CameraMode};
use debug_menu::{Line, Tone, sources::method::CHEAT_SET};
use input::host::{HostButton, HostButtons, HostPadId, HostPadState};
use input::{GatedPad, PadGate};
use serde::Deserialize;
use serde_json::{Value, json};

use super::adapter::{Clip, Playback, Player, Source};
use super::{Entry, Theater};

const REFRESH: Duration = Duration::from_millis(150);
const LONG_PRESS: Duration = Duration::from_millis(500);
const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

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
}

/// The agent only needs to provide fields it can observe during this tier.
/// Missing live scores keep the film's recorded roster values.
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
    players: Option<Vec<Player>>,
    #[serde(default)]
    camera: Option<CameraMode>,
    #[serde(default)]
    camera_player: Option<String>,
    #[serde(default)]
    camera_seat: Option<u8>,
    #[serde(default)]
    kills: Option<Vec<u64>>,
    #[serde(default)]
    bookmarks: Option<Vec<u64>>,
}

#[derive(Clone, Debug)]
pub(super) struct Track {
    pub film: String,
    pub title: String,
    pub tick: u64,
    pub length: u64,
    pub paused: bool,
    pub speed: usize,
    pub players: Vec<Player>,
    pub camera: CameraMode,
    source: Source,
    local_viewports: bool,
    tick_rate: u64,
    pub selected_player: usize,
    pub hud: bool,
    pub visible: bool,
    pub marks: (Option<u64>, Option<u64>),
    pub kills: Vec<u64>,
    pub bookmarks: Vec<u64>,
    range: Option<(u64, u64)>,
}

impl Track {
    pub(super) fn from_entry(entry: &Entry) -> Self {
        Self {
            film: entry.film.id.clone(),
            title: entry.film.map.clone(),
            tick: 0,
            length: entry.film.length_ticks,
            paused: false,
            speed: 2,
            players: entry.film.scoreboard.clone(),
            camera: if entry.film.source == Source::Host {
                CameraMode::Free
            } else {
                CameraMode::Default
            },
            source: entry.film.source,
            tick_rate: entry.film.tick_rate(),
            local_viewports: entry.film.version.is_some_and(|v| v >= 3),
            selected_player: 0,
            hud: true,
            visible: true,
            marks: (None, None),
            kills: entry.film.kill_ticks.clone(),
            bookmarks: Vec::new(),
            range: entry
                .clip
                .as_ref()
                .map(|clip| (clip.from_tick, clip.to_tick)),
        }
    }

    fn bounds(&self) -> (u64, u64) {
        self.range
            .map(|(from, to)| (from, to.saturating_sub(1)))
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

    fn cameras(&self) -> &'static [(CameraMode, &'static str)] {
        match self.source {
            Source::Host => &CAMERAS,
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
    }

    fn camera_target(&self) -> Option<&Player> {
        self.players
            .get(self.selected_player)
            .filter(|p| p.seat.is_some() || !p.name.is_empty())
    }

    fn mark(&mut self) -> Option<Clip> {
        match self.marks {
            (Some(from), None) if self.tick > from => {
                self.marks.1 = Some(self.tick);
                Some(Clip {
                    film: self.film.clone(),
                    from_tick: from,
                    to_tick: self.tick,
                })
            }
            _ => {
                self.marks = (Some(self.tick), None);
                None
            }
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
        if let Some(speed) = s.speed.and_then(|x| SPEEDS.iter().position(|&s| s == x)) {
            self.speed = speed;
        }
        if let Some(players) = s.players {
            let selected = self.players.get(self.selected_player).map(|p| &p.name);
            self.selected_player = selected
                .and_then(|name| players.iter().position(|p| &p.name == name))
                .unwrap_or(self.selected_player.min(players.len().saturating_sub(1)));
            self.players = players;
        }
        if let Some(camera) = s.camera {
            self.camera = camera;
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
        if let Some(kills) = s.kills {
            self.kills.extend(kills);
            self.kills.sort_unstable();
            self.kills.dedup();
        }
        if let Some(marks) = s.bookmarks {
            self.bookmarks = marks;
        }
    }

    /// The local review model steps these same controls without an agent.
    pub(super) fn simulate(&mut self, c: Control) {
        match c {
            Control::Pause => self.paused = !self.paused,
            Control::Skip(n) => self.tick = self.skip(n),
            Control::Speed(up) => self.speed = step(self.speed, SPEEDS.len(), up),
            Control::Seek(tick) => self.tick = tick,
            Control::Camera => self.cycle_camera(),
            Control::Player(down) => self.select_player(down),
            Control::Hide => self.visible = !self.visible,
            Control::Hud => self.hud = !self.hud,
            Control::Mark => {
                self.mark();
            }
            Control::Screenshot | Control::Leave => {}
        }
    }

    pub(super) fn view(&self, notice: Option<&str>) -> OverlayView {
        let player_line = |i, p: &Player| {
            let name = crate::ui_kit::text_fit::line(&p.name, 170);
            let text = p.score.map_or_else(|| name.clone(), |score| format!("{name} / {score}"));
            line(
                text,
                if self.selected_player == i {
                    Tone::Selected
                } else {
                    Tone::Normal
                },
            )
        };
        let first = self
            .selected_player
            .saturating_sub(3)
            .min(self.players.len().saturating_sub(7));
        let mut players = vec![line("Players / Score", Tone::Title)];
        players.extend(
            self.players
                .iter()
                .enumerate()
                .skip(first)
                .take(7)
                .map(|(i, p)| player_line(i, p)),
        );
        if self.players.is_empty() {
            players.push(line("No players", Tone::Greyed));
        }
        let mut top_right = vec![line("Camera", Tone::Title)];
        for &(mode, label) in self.cameras() {
            top_right.push(line(
                label,
                if self.camera == mode {
                    Tone::Selected
                } else {
                    Tone::Normal
                },
            ));
        }
        top_right.push(line(
            if self.hud { "HUD on" } else { "HUD off" },
            Tone::Normal,
        ));
        let transport = if self.paused { "Paused" } else { "Playing" };
        let mut bottom = vec![
            line(
                format!(
                    "{transport} | {} / {} | {}x",
                    super::selector::clock(self.tick, self.tick_rate),
                    super::selector::clock(self.length, self.tick_rate),
                    SPEEDS[self.speed]
                ),
                Tone::Title,
            ),
            line(self.timeline(43), Tone::Normal),
            line("A/Space Play/Pause | LT/RT or [ ] Speed", Tone::Hint),
            line("Left/Right 5s (hold 30s) | LB/RB Player", Tone::Hint),
            line("Y/C Camera | Sticks/Mouse + WASD Move", Tone::Hint),
            line("Start/J HUD | D-pad Up/F12 Capture", Tone::Hint),
            line("X Mark clip | Back/H Hide | B/Esc Leave", Tone::Hint),
        ];
        if let Some(start) = self.marks.0 {
            let end = self.marks.1.map_or_else(
                || "--".into(),
                |tick| super::selector::clock(tick, self.tick_rate),
            );
            bottom.push(line(
                format!(
                    "Clip {} - {end}",
                    super::selector::clock(start, self.tick_rate)
                ),
                Tone::Normal,
            ));
        }
        if let Some(notice) = notice {
            bottom.push(line(
                notice.chars().take(51).collect::<String>(),
                Tone::Normal,
            ));
        }
        OverlayView {
            players,
            top_right,
            bottom,
        }
    }

    fn timeline(&self, width: usize) -> String {
        let mut marks = vec!['-'; width];
        let (start, end) = self.bounds();
        let pos = |t: u64| {
            ((u128::from(t.clamp(start, end) - start) * (width - 1) as u128)
                / u128::from((end - start).max(1))) as usize
        };
        for (ticks, symbol) in [(&self.kills, '*'), (&self.bookmarks, '|')] {
            for &t in ticks {
                if (start..=end).contains(&t) {
                    marks[pos(t)] = symbol;
                }
            }
        }
        if let Some(t) = self.marks.0 {
            marks[pos(t)] = '[';
        }
        if let Some(t) = self.marks.1 {
            marks[pos(t)] = ']';
        }
        marks[pos(self.tick)] = 'o';
        marks.into_iter().collect()
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

fn line(text: impl Into<String>, tone: Tone) -> Line {
    Line {
        text: text.into(),
        tone,
    }
}

fn step(current: usize, count: usize, forward: bool) -> usize {
    if forward {
        (current + 1).min(count.saturating_sub(1))
    } else {
        current.saturating_sub(1)
    }
}

/// Three plates, passed directly to the host compositor's menu renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayView {
    pub players: Vec<Line>,
    pub top_right: Vec<Line>,
    pub bottom: Vec<Line>,
}

enum Request {
    Open,
    State,
    Pause,
    Seek,
    Speed(usize),
    Hud(bool),
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
        self.update_camera(now);
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
            (Request::State, Err(_)) if !self.ready => {}
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
                }
            }
            (Request::Hud(hud), Ok(_)) => {
                if let Some(track) = &mut self.track {
                    track.hud = hud;
                }
            }
            (_, Err(error)) => self.notice = Some(error),
            _ => {}
        }
    }

    pub fn view(&self) -> Option<OverlayView> {
        let track = self.track.as_ref()?;
        track.visible.then(|| {
            let mut view = track.view(self.notice.as_deref());
            if !self.ready {
                view.bottom.truncate(1);
                view.bottom[0] = line("Opening film...", Tone::Title);
            }
            view
        })
    }

    pub fn control(&mut self, c: Control) {
        let Some(track) = &mut self.track else { return };
        match c {
            Control::Hide => track.visible = !track.visible,
            Control::Leave => self.leave_requested = true,
            _ if !self.ready => {}
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
                if !self.pending.values().any(|r| matches!(r, Request::Hud(_))) {
                    let hud = !track.hud;
                    self.send(
                        CHEAT_SET,
                        json!({"name": "hide_hud", "value": !hud}),
                        Request::Hud(hud),
                    );
                }
            }
            Control::Mark => {
                if let Some(clip) = track.mark() {
                    self.completed_clip = Some(clip);
                }
            }
            Control::Screenshot => self.screenshot = true,
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
            let params = api::Seek { tick, clip_marks };
            self.send(api::SEEK, json!(params), Request::Seek);
        }
    }

    pub fn scrub(&mut self, fraction: f32) -> bool {
        if let Some(track) = &self.track
            && self.ready
            && track.visible
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
        if self
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
        if self
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
            let player = match track.camera {
                CameraMode::Orbit | CameraMode::Follow | CameraMode::FirstPerson => {
                    track.camera_target()
                }
                CameraMode::Default | CameraMode::Free => None,
            };
            self.send(
                api::CAMERA,
                json!({
                    "mode": track.camera,
                    "player": player.map(|p| &p.name),
                    "seat": player.and_then(|p| p.seat),
                    "movement": movement,
                    "look": look,
                    "zoom": zoom
                }),
                Request::Camera,
            );
        }
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
        session.pad_motion = if active && !entering {
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
            if active && !entering {
                for (button, c) in [
                    (HostButton::South, Control::Pause),
                    (HostButton::East, Control::Leave),
                    (HostButton::West, Control::Mark),
                    (HostButton::Back, Control::Hide),
                    (HostButton::North, Control::Camera),
                    (HostButton::Start, Control::Hud),
                    (HostButton::LeftBumper, Control::Player(false)),
                    (HostButton::RightBumper, Control::Player(true)),
                    (HostButton::DpadUp, Control::Screenshot),
                ] {
                    if buttons.contains(button) && !track.buttons.contains(button) {
                        session.control(c);
                    }
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
                playable_here: true,
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
                "paused": true, "tick": 450, "speed": 0.5,
                "kills": [110], "bookmarks": [250]
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
        let timeline = track.timeline(100);
        assert!(timeline.contains('*') && timeline.contains('|') && timeline.contains('['));
    }

    #[test]
    fn first_person_uses_host_recordings_or_recorded_local_viewports() {
        let mut track = sample_track();
        assert_eq!(track.view(None).top_right[1], line("Fly", Tone::Selected));
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
        assert_eq!(
            track.view(None).top_right[1],
            line("Recorded view", Tone::Selected)
        );
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
