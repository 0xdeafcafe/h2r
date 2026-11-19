//! Playback controls belong to the host. They never enter the recorded
//! input stream, including while the transport itself is hidden.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use debug_menu::{Line, Tone};
use input::host::{HostButton, HostButtons, HostPadId, HostPadState};
use input::{GatedPad, PadGate};
use serde::Deserialize;
use serde_json::{Value, json};

use super::adapter::{Clip, Playback, Player};
use super::{Entry, Theater};

const REFRESH: Duration = Duration::from_millis(150);
const LONG_PRESS: Duration = Duration::from_millis(500);
const TICKS_PER_SECOND: u64 = 30;
const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Space,
    Left,
    Right,
    Up,
    Down,
    Slower,
    Faster,
    Camera,
    Hide,
    Hud,
    Clip,
    Screenshot,
    Leave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Pause,
    Skip(i64),
    Speed(bool),
    Player(bool),
    Camera,
    Hide,
    Hud,
    Mark,
    Screenshot,
    Leave,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Camera {
    FirstPerson,
    #[default]
    Follow,
    Orbit,
}

impl Camera {
    fn label(self) -> &'static str {
        match self {
            Self::FirstPerson => "First person",
            Self::Follow => "Follow",
            Self::Orbit => "Orbit",
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Self::FirstPerson => "first_person",
            Self::Follow => "follow",
            Self::Orbit => "orbit",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Follow => Self::Orbit,
            Self::Orbit => Self::FirstPerson,
            Self::FirstPerson => Self::Follow,
        }
    }
}

/// The agent only needs to provide fields it can observe during this tier.
/// Missing live scores keep the film's recorded roster values.
#[derive(Debug, Default, Deserialize)]
struct AgentState {
    #[serde(default, alias = "playing")]
    active: Option<bool>,
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
    pub pov: usize,
    camera: Camera,
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
            players: entry.film.players.clone(),
            pov: 0,
            camera: Camera::Follow,
            hud: true,
            visible: true,
            marks: (None, None),
            kills: Vec::new(),
            bookmarks: Vec::new(),
            range: entry
                .clip
                .as_ref()
                .map(|clip| (clip.from_tick, clip.to_tick)),
        }
    }

    fn skip(&self, seconds: i64) -> u64 {
        let delta = seconds * TICKS_PER_SECOND as i64;
        let (lo, hi) = self.range.unwrap_or((0, self.length));
        self.tick.saturating_add_signed(delta).clamp(lo, hi)
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
            let selected = self.players.get(self.pov).map(|p| &p.name);
            self.pov = selected
                .and_then(|name| players.iter().position(|p| &p.name == name))
                .unwrap_or(self.pov.min(players.len().saturating_sub(1)));
            self.players = players;
        }
        if let Some(kills) = s.kills {
            self.kills = kills;
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
            Control::Player(down) => self.pov = step(self.pov, self.players.len(), down),
            Control::Camera => self.camera = self.camera.next(),
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
            let name: String = p.name.chars().take(17).collect();
            let score = p.score.map_or_else(|| "--".into(), |s| s.to_string());
            line(
                format!("{name:<17} {score:>4}"),
                if self.pov == i {
                    Tone::Selected
                } else {
                    Tone::Normal
                },
            )
        };
        let first = self
            .pov
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
            players.push(line("No player POV", Tone::Greyed));
        }
        let top_right = vec![
            line(self.camera.label(), Tone::Title),
            line("Free - after keyframes", Tone::Greyed),
            line(if self.hud { "HUD on" } else { "HUD off" }, Tone::Normal),
        ];
        let transport = if self.paused { "Paused" } else { "Playing" };
        let mut bottom = vec![
            line(
                format!(
                    "{transport}   {} / {}   {}x",
                    time(self.tick),
                    time(self.length),
                    SPEEDS[self.speed]
                ),
                Tone::Title,
            ),
            line(self.timeline(43), Tone::Normal),
            line("A/Space Play/Pause   LT/RT or [ ] Speed", Tone::Hint),
            line("Left/Right 5s (hold 30s)   Up/Down POV", Tone::Hint),
            line("Y/C Camera  LB/J HUD  RB/F12 Screenshot", Tone::Hint),
            line("X Mark clip   Back/H Hide   B/Esc Leave", Tone::Hint),
        ];
        if let Some(start) = self.marks.0 {
            let end = self.marks.1.map_or_else(|| "--".into(), time);
            bottom.push(line(format!("Clip  {} - {end}", time(start)), Tone::Normal));
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
        let pos = |t: u64| {
            ((u128::from(t.min(self.length)) * (width - 1) as u128)
                / u128::from(self.length.max(1))) as usize
        };
        for (ticks, symbol) in [(&self.kills, '*'), (&self.bookmarks, '|')] {
            for &t in ticks {
                marks[pos(t)] = symbol;
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

fn time(t: u64) -> String {
    let seconds = t / TICKS_PER_SECOND;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

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
    Play,
    State,
    Pause,
    Seek,
    Speed(usize),
    Camera(Camera, usize),
    Hud(bool),
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
}

impl Session {
    pub fn connect(&mut self, agent: &agent::Agent) {
        self.rpc = Some(Playback::new(agent));
    }

    pub fn play(&mut self, entry: &Entry) -> Result<(), String> {
        if self.track.is_some() {
            return Err("A film is already opening or playing".into());
        }
        let rpc = self.rpc.as_mut().ok_or("Playback agent is unavailable")?;
        let id = rpc.play(&entry.film.id);
        self.track = Some(Track::from_entry(entry));
        self.pending.insert(id, Request::Play);
        self.ready = false;
        self.notice = Some("Opening film...".into());
        self.last_poll = None;
        Ok(())
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
        if self.ready
            && self
                .last_poll
                .is_none_or(|last| now.duration_since(last) >= REFRESH)
            && !self
                .pending
                .values()
                .any(|req| matches!(req, Request::State))
        {
            self.last_poll = Some(now);
            self.send("theater.state", json!({}), Request::State);
        }
    }

    fn answer(&mut self, request: Request, answer: Result<Value, String>) {
        match (request, answer) {
            (Request::Play, Ok(_)) => {
                self.ready = true;
                self.notice = None;
            }
            (Request::Play, Err(error)) => {
                self.ready = false;
                self.track = None;
                self.open_error = Some(error);
            }
            (Request::State, Ok(value)) => {
                if let (Some(track), Ok(s)) =
                    (&mut self.track, serde_json::from_value::<AgentState>(value))
                {
                    if s.active == Some(false) {
                        self.ready = false;
                        self.track = None;
                    } else {
                        track.read(s);
                        if let Some((start, end)) = track.range
                            && (track.tick < start || track.tick >= end)
                            && !self.pending.values().any(|r| matches!(r, Request::Seek))
                        {
                            self.send("theater.seek", json!({ "tick": start }), Request::Seek);
                        }
                    }
                }
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
                }
            }
            (Request::Camera(camera, target), Ok(_)) => {
                if let Some(track) = &mut self.track {
                    track.camera = camera;
                    track.pov = target;
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
                self.send("theater.seek", json!({ "tick": tick }), Request::Seek);
            }
            Control::Speed(up) => {
                let speed = step(track.speed, SPEEDS.len(), up);
                if speed != track.speed {
                    self.send(
                        "theater.speed",
                        json!({ "speed": SPEEDS[speed] }),
                        Request::Speed(speed),
                    );
                }
            }
            Control::Player(down) => {
                let target = step(track.pov, track.players.len(), down);
                if target != track.pov {
                    let camera = track.camera;
                    self.camera(camera, target);
                }
            }
            Control::Camera => {
                let (camera, target) = (track.camera.next(), track.pov);
                self.camera(camera, target);
            }
            Control::Hud => {
                let hud = !track.hud;
                self.send(
                    "cheat.set",
                    json!({"name": "hide_hud", "value": !hud}),
                    Request::Hud(hud),
                );
            }
            Control::Mark => {
                if let Some(clip) = track.mark() {
                    self.completed_clip = Some(clip);
                }
            }
            Control::Screenshot => self.screenshot = true,
        }
    }

    fn camera(&mut self, camera: Camera, target: usize) {
        let Some(track) = &self.track else { return };
        let args = if track.players.get(target).is_some() {
            json!({ "mode": camera.wire(), "target": target })
        } else {
            json!({ "mode": camera.wire() })
        };
        self.send("theater.camera", args, Request::Camera(camera, target));
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
            Key::Camera => Control::Camera,
            Key::Hide => Control::Hide,
            Key::Hud => Control::Hud,
            Key::Clip => Control::Mark,
            Key::Screenshot => Control::Screenshot,
            Key::Leave => Control::Leave,
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

    fn update_key_skip(&mut self) -> Option<i64> {
        let left = i64::from(self.held_keys.contains(&Key::Left));
        let right = i64::from(self.held_keys.contains(&Key::Right));
        self.key_skip.update(right - left, Instant::now())
    }

    pub fn release_keys(&mut self) {
        self.held_keys.clear();
        self.key_skip = SkipHold::default();
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
                    (HostButton::North, Control::Camera),
                    (HostButton::Back, Control::Hide),
                    (HostButton::LeftBumper, Control::Hud),
                    (HostButton::RightBumper, Control::Screenshot),
                    (HostButton::DpadUp, Control::Player(false)),
                    (HostButton::DpadDown, Control::Player(true)),
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
                variant: "Slayer".into(),
                started_ms: 0,
                length_ticks: 3000,
                players: Vec::new(),
                source: Source::Host,
                image_stamp: "test".into(),
                playable_here: true,
                saved: false,
                result: None,
                host: None,
            },
            clip: None,
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
