//! Play and pair: start the game on the map being edited, or attach to a
//! game already running, and keep the two together while it runs
//! (docs/editor/editor.md, "Play and pair").
//!
//! * [`launch`]: the `h2 play` command, the `h2` binary, socket discovery.
//! * [`link`]: the agent socket, on its own thread.
//! * [`sync`] and [`tag_sync`]: conditional placement and tag writes for `edit.batch`.
//!
//! [`Play`] holds a pairing's state and turns the link's answers into the
//! editor's: the live players and objects for the viewport, the game
//! link in the bar, edits sent as they are made, and the spawn, camera and
//! teleport moves. The UI (`ui::play`) only draws it and
//! calls its methods; tests drive it against the agent crate's server.

pub mod launch;
pub mod link;
pub mod pair;
pub mod sync;
pub mod tag_sync;
pub use edits::Apply;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use agent_proto::sim::SessionKind;
use agent_proto::{Endpoint, Phase, Status, method};
use scenario_edit::BlockKind;
use scenario_edit::geometry::{self, Ray, Vec3};
use serde_json::{Value, json};

use crate::app::{Action, App};
use agent_proto::discover::MapMatch;
use link::{Event, Link, Options, Purpose};
use pair::{Finder, Notice, Request};
use sync::Baseline;

/// How long a launched game may take to open its socket (`h2 play` may
/// build the game image first).
pub const LAUNCH_WAIT: Duration = Duration::from_secs(20 * 60);
/// How often edits are looked for and sent while paired.
pub const PUSH_EVERY: Duration = Duration::from_millis(200);
/// A pause holds this long at most before the game resumes by itself
/// (the agent's limit; docs/runtime/agent-api.md "Control").
pub const PAUSE_MAX_MS: u64 = 600_000;
/// Ticks into a level before a launched game's player is moved to the
/// chosen spawn (the level's first ticks run its start-up scripts).
pub const SPAWN_AFTER_TICKS: u64 = 30;
/// Lifted this far above the ground a spawn or teleport lands on.
pub const LIFT: f32 = 0.25;

/// Where a launched game puts the player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Spawn {
    /// Where the level puts them.
    #[default]
    Level,
    /// On the ground under the editor camera's focus.
    Camera,
    /// At the selected placement (a starting location, a flag...).
    Selection,
}

impl Spawn {
    pub const ALL: [Spawn; 3] = [Spawn::Level, Spawn::Camera, Spawn::Selection];
    pub fn label(self) -> &'static str {
        match self {
            Spawn::Level => "Level's spawn",
            Spawn::Camera => "Editor camera",
            Spawn::Selection => "Selected placement",
        }
    }
}

/// The link's state.
#[derive(Clone, Debug, PartialEq)]
pub enum Conn {
    Off,
    Connecting,
    Connected,
    Lost(String),
}

/// What the link offers the editor, including whether writes can reach it.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    NoGame,
    Found {
        map: Option<String>,
        matches: MapMatch,
    },
    Connecting,
    Viewing {
        map: Option<String>,
        phase: Option<Phase>,
        tick: Option<u64>,
    },
    Editing {
        map: String,
        phase: Phase,
        tick: u64,
    },
    Blocked {
        why: String,
    },
    OtherMap {
        game: String,
        open: String,
    },
    Lost {
        why: String,
    },
}

impl LinkState {
    /// Stable spelling for `pair.status`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::NoGame => "no_game",
            Self::Found { .. } => "found",
            Self::Connecting => "connecting",
            Self::Viewing { .. } => "viewing",
            Self::Editing { .. } => "editing",
            Self::Blocked { .. } => "blocked",
            Self::OtherMap { .. } => "other_map",
            Self::Lost { .. } => "lost",
        }
    }

    /// Whether the user can toggle Edit for this pairing.
    pub fn can_edit(&self) -> bool {
        matches!(self, Self::Viewing { .. } | Self::Editing { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTone {
    Idle,
    Notice,
    View,
    Live,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkView {
    pub state: LinkState,
    pub tone: LinkTone,
    pub text: String,
    pub why: Option<String>,
    pub can_play: bool,
    pub can_stop: bool,
    pub can_pause: bool,
    pub paused: bool,
}

pub(super) fn phase_label(phase: Phase, paused: bool) -> &'static str {
    match (paused, phase) {
        (true, _) => "paused",
        (_, Phase::Loading) => "loading",
        (_, Phase::Menu) => "menu",
        (_, Phase::Gameplay) => "playing",
        (_, Phase::Cinematic) => "cinematic",
    }
}

pub(super) fn session_label(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::None => "No simulation",
        SessionKind::Local => "Local session",
        SessionKind::CoopHost => "Co-op host",
        SessionKind::CoopGuest => "Co-op guest",
        SessionKind::MpHost => "Multiplayer host",
        SessionKind::MpClient => "Multiplayer client",
        SessionKind::Unknown => "Unknown session kind",
    }
}

/// A pairing with a game, launched or attached.
pub struct Play {
    link: Option<Link>,
    /// The endpoint to retry after the link drops.
    retry: Option<Endpoint>,
    pub conn: Conn,
    /// The last `session.status`.
    pub status: Option<Status>,
    /// Next read of the binary stream's writer counters over the control link.
    next_live_status: Instant,
    /// Scenario datum from edit.map, for the placement paths in edit.batch.
    scenario_tag: Option<String>,
    pub session: Option<SessionKind>,
    /// What the game is taken to hold, and the map it is for.
    baseline: Option<(String, Baseline)>,
    /// Send edits as they are made.
    pub live_edits: bool,
    next_push: Instant,
    edits: Edits,
    launch_baseline: Option<(Baseline, tag_sync::TagBaseline)>,
    /// Blocks with entries added or removed since the baseline.
    pub reload: Vec<BlockKind>,
    /// We hold `session.claim`.
    pub controller: bool,
    pub paused: bool,
    /// The launched game, its log, and the spawn it waits for.
    child: Option<Child>,
    child_endpoint: Option<Endpoint>,
    /// Identity of the editor's launch copy, including when it pairs again.
    launched_copy: Option<(Endpoint, u64, u32)>,
    pub log_path: Option<PathBuf>,
    pending_spawn: Option<Vec3>,
    pub spawn: Spawn,
    launches: u32,
    /// The `h2` binary typed in the panel (empty: found as `launch::find_h2`).
    pub h2_path: String,
    /// The socket typed in the panel.
    pub socket_text: String,
    /// What the last launch ran, for the panel.
    pub last_command: Option<String>,
    /// Looks for running games ([`Play::discovering`]).
    pub finder: Finder,
    /// Sockets already paired with, by hand or by itself: auto-pair takes
    /// each game once, so an Unpair stays unpaired.
    auto_tried: HashSet<PathBuf>,
    /// UI requests taken at the next update.
    queue: Vec<Request>,
}

impl Default for Play {
    fn default() -> Self {
        Self::new()
    }
}

impl Play {
    pub fn new() -> Self {
        Self {
            link: None,
            retry: None,
            conn: Conn::Off,
            status: None,
            next_live_status: Instant::now(),
            scenario_tag: None,

            session: None,
            baseline: None,
            live_edits: false,
            next_push: Instant::now(),
            edits: Edits::default(),
            launch_baseline: None,
            reload: Vec::new(),
            controller: false,
            paused: false,
            child: None,
            child_endpoint: None,
            launched_copy: None,
            log_path: None,
            pending_spawn: None,
            spawn: Spawn::Level,
            launches: 0,
            h2_path: String::new(),
            socket_text: String::new(),
            last_command: None,
            // Looks at nothing: tests and anything headless. The window's
            // panel uses `discovering`.
            finder: Finder::with_scan(std::sync::Arc::new(Vec::<agent_proto::discover::Found>::new)),
            auto_tried: HashSet::new(),
            queue: Vec::new(),
        }
    }

    /// A pairing that looks for running games ([`pair::default_scan`]).
    pub fn discovering() -> Self {
        Self {
            finder: Finder::default(),
            ..Self::new()
        }
    }

    pub fn endpoint(&self) -> Option<&Endpoint> {
        self.link.as_ref().map(|l| &l.endpoint)
    }
    pub fn is_paired(&self) -> bool {
        self.conn == Conn::Connected
    }
    /// The game's map, from the last poll.
    pub fn map(&self) -> Option<&str> {
        self.status.as_ref()?.at.map.as_deref()
    }

    /// The state shared by the bar, the Game panel and `pair.status`.
    pub fn link_view(&self, app: &App) -> LinkView {
        use LinkState as State;
        use LinkTone as Tone;

        let can_pause = self.is_paired();
        let paused = can_pause && self.paused;
        let (state, tone, text, why) = match &self.conn {
            Conn::Off => match self.notice(app) {
                Some(n) => (
                    State::Found {
                        map: n.map,
                        matches: n.matches,
                    },
                    Tone::Notice,
                    n.short,
                    Some(n.text),
                ),
                None => (State::NoGame, Tone::Idle, "No game".into(), None),
            },
            Conn::Connecting => (
                State::Connecting,
                Tone::Notice,
                "Connecting...".into(),
                None,
            ),
            Conn::Lost(why) => (
                State::Lost { why: why.clone() },
                Tone::Error,
                "Game lost".into(),
                Some(why.clone()),
            ),
            Conn::Connected => {
                let at = self.status.as_ref().map(|s| &s.at);
                let map = self.map();
                if let (MapMatch::Different, Some(game), Some(open)) =
                    (self.map_match(app), map, app.loaded_map())
                {
                    (
                        State::OtherMap {
                            game: game.into(),
                            open: open.into(),
                        },
                        Tone::Warn,
                        format!("Game runs {game}"),
                        Some(format!("Edits stay here while the editor has {open} open.")),
                    )
                } else if !self.live_edits && self.edit_refusal(app).is_none() {
                    let phase = at.map(|a| a.phase);
                    let tick = at.map(|a| a.tick.unwrap_or(a.ticks));
                    let shown_map = map.unwrap_or("game");
                    let activity = phase.map_or("waiting for status", |p| phase_label(p, paused));
                    (
                        State::Viewing {
                            map: map.map(str::to_owned),
                            phase,
                            tick,
                        },
                        Tone::View,
                        format!("Viewing {shown_map} · {activity}"),
                        None,
                    )
                } else if let Some(why) = self.edit_refusal(app) {
                    let kind = self
                        .session
                        .filter(|k| k.writes().err() == Some(why.as_str()));
                    let short = match kind {
                        Some(SessionKind::CoopHost | SessionKind::CoopGuest) => "co-op session",
                        Some(SessionKind::MpClient) => "multiplayer client",
                        Some(SessionKind::Unknown) => "unknown session",
                        _ => why.as_str(),
                    };
                    let text = format!("Edits stay here: {short}");
                    (
                        State::Blocked { why: why.clone() },
                        Tone::Warn,
                        text,
                        Some(why),
                    )
                } else {
                    // edits_blocked has established the map and status.
                    let at = at.expect("an editable game has reported its status");
                    let map = map.expect("an editable game has reported its map");
                    (
                        State::Editing {
                            map: map.into(),
                            phase: at.phase,
                            tick: at.tick.unwrap_or(at.ticks),
                        },
                        Tone::Live,
                        format!("Editing {map} live"),
                        None,
                    )
                }
            }
        };
        LinkView {
            state,
            tone,
            text,
            why,
            can_play: app.loaded_map().is_some() && self.conn != Conn::Connecting,
            can_stop: self.child.is_some(),
            can_pause,
            paused,
        }
    }

    /// Leave a UI request for the next update.
    pub fn request(&mut self, r: Request) {
        self.queue.push(r);
    }

    pub fn retry_endpoint(&self) -> Option<&Endpoint> {
        self.retry.as_ref()
    }

    pub fn pair_socket_text(&mut self, app: &mut App) {
        let text = self.socket_text.trim();
        let endpoint = if text.is_empty() {
            Ok(Endpoint::Unix(agent_proto::default_socket()))
        } else {
            Endpoint::parse(text)
        };
        match endpoint {
            Ok(endpoint) => self.request(Request::Pair(endpoint)),
            Err(e) => app.error(format!("pair: {e}")),
        }
    }

    /// Select the game map through the usual unsaved-changes prompt.
    pub fn open_game_map(&self, app: &mut App) {
        let Some(game) = self.map() else { return };
        let map = app
            .maps()
            .iter()
            .chain(&app.mappack_maps())
            .find(|name| MapMatch::of(Some(name), Some(game)) == MapMatch::Same)
            .cloned();
        match map {
            Some(name) => app.dispatch(Action::SelectMap(name)),
            None => app.error(format!(
                "Open a source that contains {game}."
            )),
        }
    }

    /// The running games the editor is not paired with, as one notice:
    /// none while paired or connecting.
    pub fn notice(&self, app: &App) -> Option<Notice> {
        if matches!(self.conn, Conn::Connected | Conn::Connecting) {
            return None;
        }
        pair::notice(&self.finder.running(), app.loaded_map())
    }

    /// How the paired game's map compares with the one open here.
    pub fn map_match(&self, app: &App) -> MapMatch {
        MapMatch::of(app.loaded_map(), self.map())
    }

    /// Attach to a game already running at `endpoint`. The game is taken
    /// to hold the map as the editor shows it now.
    pub fn pair(&mut self, app: &mut App, endpoint: Endpoint) {
        if let Endpoint::Unix(p) = &endpoint {
            self.auto_tried.insert(p.clone());
        }
        self.start(app, endpoint, Options::default());
        self.baseline = None;
        app.info(format!(
            "pairing with the game at {}",
            self.endpoint().map(ToString::to_string).unwrap_or_default()
        ));
    }

    fn start(&mut self, app: &mut App, endpoint: Endpoint, options: Options) {
        app.tags.live_datums = None;
        app.pairing.clear_mirror();
        app.pairing.play_copy = self
            .launched_copy
            .as_ref()
            .filter(|(game, _, _)| game == &endpoint)
            .map(|(_, revision, checksum)| (*revision, *checksum));
        self.link = Some(Link::open(endpoint, options));
        self.retry = None;
        self.conn = Conn::Connecting;
        self.status = None;
        self.next_live_status = Instant::now();
        self.scenario_tag = None;

        self.session = None;
        self.edits = Edits::default();
        self.live_edits = false;
        self.reload.clear();
        self.controller = false;
        self.paused = false;
        self.pending_spawn = None;
    }

    /// Drop the pairing (the game keeps running; our control is released).
    pub fn unpair(&mut self, app: &mut App) {
        if self.link.take().is_some() {
            app.info("unpaired from the game");
        }
        app.pairing.clear_mirror();
        self.conn = Conn::Off;
        self.baseline = None;
        app.tags.live_datums = None;
        self.edits = Edits::default();
        self.retry = None;
        self.live_edits = false;
        self.controller = false;
        self.paused = false;
    }

    /// Start the game on the open map with the editor's bytes, paired.
    pub fn launch(&mut self, app: &mut App) {
        match self.prepare(app) {
            Ok(l) => self.run_launch(app, l),
            Err(e) => app.error(format!("Play: {e}")),
        }
    }

    /// The launch for the open map: writes the edited map to the scratch
    /// directory for `H2_MAP_DIR`.
    pub fn prepare(&mut self, app: &mut App) -> Result<launch::Launch, String> {
        let level = app.loaded_map().ok_or("open a map first")?.to_owned();
        let session = &app.session;
        let scenario = session.scenario().ok_or("open a map first")?;
        let h2 = launch::find_h2(Some(PathBuf::from(self.h2_path.trim())).as_deref())
            .ok_or("no h2 binary: install it (cargo install --path crates/apps/cli) or name it under Advanced in the Game panel")?;
        let disc = match app.source() {
            Some(editor_core::MapSource::Disc(d)) => Some(d.path().to_owned()),
            _ => editor_core::default_disc(None),
        }
        .ok_or("h2 play needs the disc image: open the disc, or set H2_DISC")?;
        let data = session
            .maps_dir()
            .parent()
            .map(PathBuf::from)
            .ok_or("no data root")?;
        let map_dir = if session.is_derived() {
            if session.is_dirty() {
                return Err(
                    "a new map plays from its package: save it (Save as package...) first".into(),
                );
            }
            None
        } else {
            let source = session.source().ok_or("open a map first")?;
            let dir = launch::scratch().join("maps");
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let out = dir.join(format!("{level}.map"));
            scenario_edit::save::save_as(source, scenario, &out)
                .map_err(|e| format!("writing {}: {e}", out.display()))?;
            Some(dir)
        };
        // A Map Pack map plays from the editor's bytes (the map override) on
        // the Halo 2 disc. `h2 play` installs the map pack itself from beside
        // that disc (`mappack.iso`); say so when this one is elsewhere.
        if app.is_mappack_map(&level)
            && let Some(pack) = app.mappack().map(|p| p.path().to_owned())
        {
            let found = editor_core::mappack::beside(&disc);
            if !found.is_some_and(|f| matches!((f.canonicalize(), pack.canonicalize()), (Ok(a), Ok(b)) if a == b)) {
                app.info(format!(
                    "Play: {level} is a Map Pack map and {} is not beside the disc, so h2 play will not install the map pack; it plays from the editor's copy. Put the pack beside the disc as mappack.iso to install it as well",
                    pack.display()
                ));
            }
        }
        self.launches += 1;
        let socket = launch::socket_for_launch(self.launches);
        if let Some(d) = socket.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::remove_file(&socket);
        Ok(launch::Launch {
            h2,
            disc,
            data,
            level,
            socket,
            map_dir,
        })
    }

    fn run_launch(&mut self, app: &mut App, l: launch::Launch) {
        self.stop_game(app);
        let spawn = self.spawn_point(app);
        if self.spawn != Spawn::Level && spawn.is_none() {
            app.error(format!(
                "Play: no {} to spawn at; the level's spawn is used",
                self.spawn.label().to_lowercase()
            ));
        }
        let log = launch::scratch().join(format!("play-{}.log", self.launches));
        let line = l.shell_line();
        match l.spawn(&log) {
            Ok(child) => {
                self.child = Some(child);
                self.log_path = Some(log.clone());
                self.last_command = Some(line.clone());
                let endpoint = Endpoint::Unix(l.socket.clone());
                self.child_endpoint = Some(endpoint.clone());
                self.launched_copy = l.map_dir.as_ref().and_then(|dir| {
                    // The checksum of the saved launch copy includes unsaved
                    // edits to the document. Later edits keep this identity.
                    let source = scenario_edit::MapSource::file(dir.join(format!("{}.map", l.level)));
                    let checksum = source.open().ok()?.header().checksum;
                    Some((endpoint.clone(), app.session.world_rev(), checksum))
                });
                self.start(
                    app,
                    endpoint,
                    Options {
                        wait: LAUNCH_WAIT,
                        ..Options::default()
                    },
                );
                self.edits.own_start = true;
                self.live_edits = true;
                self.launch_baseline = app
                    .session
                    .scenario()
                    .map(|s| (Baseline::of(s), tag_sync::TagBaseline::of(&app.tags)));
                self.pending_spawn = spawn;
                app.info(format!("Play: {line} (log: {})", log.display()));
            }
            Err(e) => app.error(format!("Play: starting {}: {e}", l.h2.display())),
        }
    }

    /// End the game this editor started.
    pub fn stop_game(&mut self, app: &mut App) {
        if let Some(mut c) = self.child.take() {
            self.launched_copy = None;
            if matches!(c.try_wait(), Ok(None)) {
                let _ = c.kill();
                app.info("stopped the game");
            }
            let _ = c.wait();
            self.unpair(app);
        }
    }

    /// Where [`Self::spawn`] puts the player now, if anywhere.
    pub fn spawn_point(&self, app: &App) -> Option<Vec3> {
        match self.spawn {
            Spawn::Level => None,
            Spawn::Camera => Some(ground(app, app.session.camera().target)),
            Spawn::Selection => app.session.selected_transform().map(|t| {
                let p = t.position;
                [p[0], p[1], p[2] + LIFT]
            }),
        }
    }

    /// Once a frame: the link's news, the launched game's state, and edits
    /// to send.
    pub fn update(&mut self, app: &mut App) {
        self.finder.poll();
        self.take_requests(app);
        self.auto_pair(app);
        if let Some(c) = &mut self.child
            && let Ok(Some(code)) = c.try_wait()
        {
            app.info(format!(
                "the game exited ({code}){}",
                self.log_path
                    .as_ref()
                    .map(|p| format!("; its log: {}", p.display()))
                    .unwrap_or_default()
            ));
            self.child = None;
            self.launched_copy = None;
            self.link = None;
            app.pairing.clear_mirror();
            self.retry = None;
            self.conn = Conn::Off;
            self.paused = false;
            self.live_edits = false;
            self.controller = false;
        }
        let events = self.link.as_ref().map(Link::drain).unwrap_or_default();
        for e in events {
            self.on_event(app, e);
        }
        if let Some(stream) = &app.pairing.mirror {
            let previous_epoch = app.pairing.shown.as_ref().map(|sample| sample.epoch);
            app.pairing.shown = stream.sample(app.pairing.latest || self.paused);
            if let Some(sample) = &app.pairing.shown
                && (previous_epoch != Some(sample.epoch) || Instant::now() >= self.next_live_status)
            {
                if app.pairing.stream_map.is_null() {
                    app.pairing.stream_map = stream.status(false)["map"].clone();
                }
                self.next_live_status = Instant::now() + Duration::from_secs(1);
                self.call(agent_proto::live::STATUS, Value::Null, Purpose::LiveStatus);
            }
        }
        self.maybe_spawn(app);
        if Instant::now() >= self.next_push {
            self.next_push = Instant::now() + PUSH_EVERY;
            self.push_edits(app);
        }
        self.publish(app);
    }

    /// Game-link controls requested by the UI and by `pair.*`.
    fn take_requests(&mut self, app: &mut App) {
        let mut reqs = std::mem::take(&mut self.queue);
        reqs.append(&mut app.pairing.requests);
        for r in reqs {
            if let Some(why) = r.refusal(&self.link_view(app)) {
                app.error(why);
                continue;
            }
            match r {
                Request::Pair(e) if self.is_paired() && self.endpoint() != Some(&e) => {
                    self.edits.next_pair = Some(e);
                    self.request_leave(app);
                }
                Request::Pair(e) => self.pair(app, e),
                Request::Unpair => self.request_leave(app),
                Request::Apply(action) => {
                    if let Err(why) = self.apply_edits(app, action) {
                        app.error(why);
                    }
                }
                Request::Play => self.launch(app),
                Request::Stop => self.stop_game(app),
                Request::Edit(true) => {
                    if let Err(why) = self.apply_edits(app, Apply::Send) {
                        app.error(why);
                    }
                }
                Request::Edit(false) => {
                    self.live_edits = false;
                    self.edits.own_start = false;
                }
                Request::Pause => self.pause(),
                Request::Resume => self.resume(),
                Request::Step(ticks) => self.step(ticks),
            }
        }
    }

    /// The socket auto-pair would take now: the one running game, when
    /// nothing is paired, the setting is on, and it has not been paired
    /// with before.
    pub fn auto_target(&self, on: bool) -> Option<PathBuf> {
        if !on || self.child.is_some() || matches!(self.conn, Conn::Connected | Conn::Connecting) {
            return None;
        }
        match self.finder.running().as_slice() {
            [g] if !self.auto_tried.contains(&g.path) => Some(g.path.clone()),
            _ => None,
        }
    }

    fn auto_pair(&mut self, app: &mut App) {
        let Some(path) = self.auto_target(app.pairing.auto_pair) else {
            return;
        };
        let map = self
            .finder
            .running()
            .first()
            .and_then(|g| g.map().map(str::to_owned));
        app.info(format!(
            "found Halo 2 running {}: pairing with it (turn off Auto-pair in the game link)",
            map.as_deref().unwrap_or("with no map yet")
        ));
        self.pair(app, Endpoint::Unix(path));
    }

    /// What `pair.status` and `pair.list` answer, read each frame.
    fn publish(&self, app: &mut App) {
        let (state, why) = match &self.conn {
            Conn::Off => ("off", None),
            Conn::Connecting => ("connecting", None),
            Conn::Connected => ("connected", None),
            Conn::Lost(w) => ("lost", Some(w.clone())),
        };
        let at = self.status.as_ref().map(|s| &s.at);
        let published = json!({
            "state": state,
            "why": why,
            "socket": self.endpoint().map(ToString::to_string),
            "map": self.map(),
            "phase": at.map(|a| format!("{:?}", a.phase).to_lowercase()),
            "paused": at.map(|a| a.paused),
            "tick": at.map(|a| a.tick.unwrap_or(a.ticks)),
            "matches": match self.map_match(app) {
                MapMatch::Same => "same",
                MapMatch::Different => "different",
                MapMatch::Unknown => "unknown",
            },
            "open_map": app.loaded_map(),
            "live_edits": self.live_edits,
            "edits_blocked": self.edits_blocked(app),
            "edit_refusal": self.edit_refusal(app),
            "edit_peers": self.edits.peers,
            "auto_pair": app.pairing.auto_pair,
            "games": self.finder.running().len(),
        });
        app.pairing.published = Some(published);
        app.pairing.link = Some(self.link_view(app));
    }

    fn call(&self, method: &str, params: Value, purpose: Purpose) {
        if let Some(l) = &self.link {
            l.call(method, params, purpose);
        }
    }

    fn on_event(&mut self, app: &mut App, e: Event) {
        match e {
            Event::Connected => {
                self.conn = Conn::Connected;
                self.call(method::METHODS, Value::Null, Purpose::Capabilities);
                app.pairing.mirror = self.endpoint().cloned().map(live_mirror::Stream::open);
                app.info(format!(
                    "paired with the game at {}",
                    self.endpoint().map(ToString::to_string).unwrap_or_default()
                ));
            }
            Event::Lost(why) => {
                app.error(format!("game link: {why}"));
                self.conn = Conn::Lost(why);
                self.retry = self.link.take().map(|l| l.endpoint.clone());
                app.pairing.clear_mirror();
                self.controller = false;
            }
            Event::Poll(status) => {
                if let Ok(s) = serde_json::from_value::<Status>(status) {
                    self.paused = s.at.paused;
                    self.controller = s.controller;
                    self.status = Some(s);
                    if !self.edits.map_requested {
                        self.edits.map_requested = true;
                        self.call(
                            "edit.map",
                            json!({"known_epoch":self.known_tag_epoch()}),
                            Purpose::Map,
                        );
                    }
                }
            }
            Event::Answer {
                purpose,
                method,
                result,
            } => self.on_answer(app, purpose, &method, result),
        }
    }

    fn on_answer(
        &mut self,
        app: &mut App,
        purpose: Purpose,
        method: &str,
        result: Result<Value, agent_proto::RpcError>,
    ) {
        match (purpose, result) {
            (Purpose::LiveStatus, result) => {
                let stream_id = app.pairing.mirror.as_ref()
                    .map(|stream| stream.status(false)["stream"].clone());
                app.pairing.counters = result.ok().and_then(|reply| {
                    let id = stream_id?;
                    app.pairing.stream_map = reply["map"].clone();
                    reply["streams"].as_array()?.iter().find(|s| s["stream"] == id).cloned()
                });
            }
            (Purpose::Capabilities, Ok(v)) => {
                let has = |name: &str| v.as_array().is_some_and(|a| a.iter().any(|m| m == name));
                self.edits.respawn = has("world.object.respawn");
                self.edits.reload = has("map.reload");
            }
            (Purpose::Map, result) => self.edit_map(app, result),
            (Purpose::Journal(epoch), result) => self.edit_journal(app, epoch, result),
            (Purpose::Revert { leave, epoch }, result) => self.reverted(app, leave, epoch, result),
            (Purpose::Edit(id), result) => self.batch_answer(app, id, result),
            (Purpose::Teleport, Ok(_)) => app.info("moved the player"),
            (Purpose::Control(what), Ok(_)) => {
                match what {
                    "pause" => self.paused = true,
                    "resume" => self.paused = false,
                    "claim" => self.controller = true,
                    "release" => self.controller = false,
                    _ => {}
                }
                app.info(format!("game: {what}"));
            }
            (_, Err(e)) => app.error(format!("{method}: {e}")),
            (Purpose::Other, Ok(v)) => app.info(format!("{method}: {v}")),
        }
    }

    /// Why edits aren't being sent to the game now, if they aren't.
    pub fn edits_blocked(&self, app: &App) -> Option<String> {
        if !self.is_paired() {
            return Some("not paired".into());
        }
        self.edit_refusal(app)
            .or_else(|| (!self.live_edits).then(|| "View mode".into()))
    }

    fn maybe_spawn(&mut self, app: &mut App) {
        let Some(pos) = self.pending_spawn else {
            return;
        };
        let Some(s) = &self.status else { return };
        if s.at.phase != Phase::Gameplay || s.at.map_ticks < SPAWN_AFTER_TICKS {
            return;
        }
        let Some(kind) = self.session else { return };
        self.pending_spawn = None;
        match kind.writes() {
            Ok(()) => self.teleport(app, pos),
            Err(e) => app.error(format!("spawn: not moving the player: {e}")),
        }
    }

    /// Move the editor camera to the player, looking the way they face.
    pub fn go_to_player(&self, app: &mut App) {
        if let Some(sample) = &app.pairing.shown
            && !app.pairing.matches_sample(app, sample)
        {
            app.error("open the streamed map to position its editor camera");
            return;
        }
        let Some((player, object)) = app.pairing.player() else {
            app.error("no player in the game to go to");
            return;
        };
        let pos = if object.parent.is_some() {
            let Some(pos) = app.pairing.shown.as_ref().and_then(|s| {
                crate::viewport::mirror::WorldPoses::new(s).position(object.id)
            }) else {
                app.error("the player's parent transform is unavailable");
                return;
            };
            pos
        } else {
            object.position
        };
        let (index, fwd) = (player.index, player.aim);
        let cam = app.session.camera_mut();
        cam.target = [pos[0], pos[1], pos[2] + 0.6];
        if fwd[0] != 0.0 || fwd[1] != 0.0 {
            // Camera::forward is (-cos yaw cos pitch, -sin yaw cos pitch, -sin pitch).
            cam.yaw = (-fwd[1]).atan2(-fwd[0]);
        }
        cam.pitch = 0.3;
        cam.distance = 3.0;
        app.info(format!(
            "camera at player.{index} ({:.1}, {:.1}, {:.1})",
            pos[0], pos[1], pos[2]
        ));
    }

    /// Move the player to `pos` (`sim.state.set`; refused by the game in a
    /// co-op or client session, and here first).
    pub fn teleport(&self, app: &mut App, pos: Vec3) {
        if !self.is_paired() {
            app.error("not paired with a game");
            return;
        }
        if self.map_match(app) != MapMatch::Same
            || app.pairing.shown.as_ref().is_some_and(|s| !app.pairing.matches_sample(app, s))
        {
            app.error("open the streamed map to teleport the player to an editor position");
            return;
        }
        if let Some(Err(e)) = self.session.map(SessionKind::writes) {
            app.error(format!("not moving the player: {e}"));
            return;
        }
        let index = app.pairing.player().map_or(0, |(player, _)| player.index);
        let player = format!("player.{index}");
        self.call(
            agent_proto::sim::method::STATE_SET,
            json!({"player": player, "pos": [pos[0], pos[1], pos[2]]}),
            Purpose::Teleport,
        );
    }

    /// Move the player to the ground under the camera's focus.
    pub fn teleport_to_camera(&self, app: &mut App) {
        let p = ground(app, app.session.camera().target);
        self.teleport(app, p);
    }

    pub fn pause(&mut self) {
        self.ensure_claim();
        self.call(
            method::PAUSE,
            json!({"max_ms": PAUSE_MAX_MS}),
            Purpose::Control("pause"),
        );
    }
    pub fn resume(&mut self) {
        self.call(method::RESUME, Value::Null, Purpose::Control("resume"));
        // Give control back so other clients (and agents) may act.
        self.call(method::RELEASE, Value::Null, Purpose::Control("release"));
        self.controller = false;
    }
    pub fn step(&mut self, ticks: u64) {
        self.ensure_claim();
        self.call(
            method::STEP,
            json!({"ticks": ticks}),
            Purpose::Control("step"),
        );
    }
    fn ensure_claim(&mut self) {
        if !self.controller {
            self.call(method::CLAIM, Value::Null, Purpose::Control("claim"));
            self.controller = true;
        }
    }
}

/// The ground under `p` (the structure's nearest surface below a point a
/// little above it), lifted by [`LIFT`]; `p` itself when there is none.
pub fn ground(app: &App, p: Vec3) -> Vec3 {
    let ray = Ray {
        origin: [p[0], p[1], p[2] + 1.0],
        dir: [0.0, 0.0, -1.0],
    };
    let at = geometry::surface(app.session.world().0, &ray).unwrap_or(p);
    [at[0], at[1], at[2] + LIFT]
}

/// The structure's surface under a viewport point (0..1), for "teleport
/// to the cursor".
pub fn surface_at(app: &App, u: f32, v: f32, aspect: f32) -> Option<Vec3> {
    let ray = app.session.camera().ray(u, v, aspect);
    let at = geometry::surface(app.session.world().0, &ray)?;
    Some([at[0], at[1], at[2] + LIFT])
}
