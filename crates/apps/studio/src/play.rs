//! Play and pair: start the game on the map being edited, or attach to a
//! game already running, and keep the two together while it runs
//! (docs/editor/editor.md, "Play and pair").
//!
//! * [`launch`]: the `h2 play` command, the `h2` binary, socket discovery.
//! * [`link`]: the agent socket, on its own thread.
//! * [`sync`] and [`tag_sync`]: conditional placement and tag writes for `edit.batch`.
//! * [`raw_sync`]: geometry and painted pixels as `edit.batch` raw entries.
//!
//! [`Play`] holds a pairing's state and turns the link's answers into the
//! editor's: the live players and objects for the viewport, the game
//! link in the bar, edits sent as they are made, and the spawn, camera and
//! teleport moves. The UI (`ui::play`) only draws it and
//! calls its methods; tests drive it against the agent crate's server.

pub mod auto;
pub(crate) mod display;
pub mod issues;
pub mod launch;
pub mod link;
pub mod pair;
pub mod objects;
pub mod playtest;
pub(crate) mod presence;
pub(crate) mod raw_sync;
pub(crate) mod select;
pub mod stream;
pub mod sync;
pub mod tag_sync;
mod capabilities;
mod edits;
mod feedback;
pub use edits::Apply;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(all(test, unix))]
mod reload_tests;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
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
use edits::Edits;
use link::{Event, Link, Options, Purpose};
use pair::{Finder, Notice, Request};
use playtest::{Destination, Waiting};
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
/// Time for the local player and move acknowledgement after the game connects or reloads.
const SPAWN_WAIT: Duration = Duration::from_secs(30);
/// Lifted this far above the ground a spawn or teleport lands on.
pub const LIFT: f32 = 0.25;

/// Where a launched game puts the player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Spawn {
    /// Where the level puts them.
    #[default]
    Level,
    /// The last local player position and facing on this map.
    Resume,
    /// On the ground under the editor camera's focus.
    Camera,
    /// At the selected placement (a starting location, a flag...).
    Selection,
}

impl Spawn {
    pub const ALL: [Spawn; 4] = [Spawn::Level, Spawn::Resume, Spawn::Camera, Spawn::Selection];
    pub fn label(self) -> &'static str {
        match self {
            Spawn::Level => "Level's spawn",
            Spawn::Resume => "Where I was",
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
    Reconnecting { backoff: auto::Backoff, why: String },
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
    Reconnecting { since: Instant },
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
            Self::Reconnecting { .. } => "reconnecting",
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
    pub unavailable: Vec<String>,
    pub issues: std::sync::Arc<issues::IssueIndex>,
    pub can_play: bool,
    pub playtest_busy: bool,
    pub can_stop: bool,
    pub paired: bool,
    pub(crate) controls: capabilities::Controls,
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
    closing_links: Vec<link::ClosingLink>,
    /// The endpoint to retry after the link drops.
    retry: Option<Endpoint>,
    pair_source: pair::Source,
    paired_pid: Option<u32>,
    pub conn: Conn,
    /// The last `session.status`.
    pub status: Option<Status>,
    /// Optional public tick stamp from hosts with counted map activations.
    status_activation: Option<u64>,
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
    presence: presence::Presence,
    objects: objects::State,
    reload_copy: Option<edits::ReloadCopy>,
    launch_baseline: Option<(Baseline, tag_sync::TagBaseline, raw_sync::RawBaseline)>,
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
    pending_spawn: Option<Destination>,
    places: HashMap<String, (u32, playtest::Place)>,
    play_world: u64,
    pairing_id: u64,
    waiting: Waiting,
    /// The full method list for this control connection, once received.
    methods: Option<HashSet<String>>,
    playtest_consent: bool,
    spawn_after_tick: u64,
    spawn_deadline: Option<Instant>,
    pause_on_return: bool,
    launches: u32,
    /// The `h2` binary typed in the panel (empty: found as `launch::find_h2`).
    pub h2_path: String,
    /// The socket typed in the panel.
    pub socket_text: String,
    /// What the last launch ran, for the panel.
    pub last_command: Option<String>,
    /// Looks for running games ([`Play::discovering`]).
    pub finder: Finder,
    /// Auto-pair attempts are forgotten when the socket leaves the finder.
    auto_tried: HashSet<PathBuf>,
    unpaired: auto::Unpaired,
    feedback: feedback::Feedback,
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
            closing_links: Vec::new(),
            retry: None,
            pair_source: pair::Source::Finder,
            paired_pid: None,
            conn: Conn::Off,
            status: None,
            status_activation: None,
            next_live_status: Instant::now(),
            scenario_tag: None,

            session: None,
            baseline: None,
            live_edits: false,
            next_push: Instant::now(),
            edits: Edits::default(),
            presence: Default::default(),
            objects: objects::State::default(),
            reload_copy: None,
            launch_baseline: None,
            reload: Vec::new(),
            controller: false,
            paused: false,
            child: None,
            child_endpoint: None,
            launched_copy: None,
            log_path: None,
            pending_spawn: None,
            places: HashMap::new(),
            play_world: 0,
            pairing_id: 0,
            waiting: Waiting::Idle,
            methods: None,
            playtest_consent: false,
            spawn_after_tick: 0,
            spawn_deadline: None,
            pause_on_return: false,
            launches: 0,
            h2_path: String::new(),
            socket_text: String::new(),
            last_command: None,
            // Looks at nothing: tests and anything headless. The window's
            // panel uses `discovering`.
            finder: Finder::with_scan(std::sync::Arc::new(Vec::<agent_proto::discover::Found>::new)),
            auto_tried: HashSet::new(),
            unpaired: auto::Unpaired::default(),
            feedback: feedback::Feedback::default(),
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
        self.link.as_ref().map(|l| &l.endpoint).or(self.retry.as_ref())
    }
    pub fn is_paired(&self) -> bool {
        self.conn == Conn::Connected
    }
    /// The game's map, from the last poll.
    pub fn map(&self) -> Option<&str> {
        self.status.as_ref()?.at.map.as_deref()
    }

    /// Every panel in a frame reads the same issues and navigation targets.
    pub(crate) fn displayed_link(&self, app: &App) -> std::sync::Arc<LinkView> {
        app.pairing.link.clone().unwrap_or_else(|| self.link_view(app).into())
    }

    /// The state shared by the bar, the Game panel and `pair.status`.
    pub fn link_view(&self, app: &App) -> LinkView {
        use LinkState as State;
        use LinkTone as Tone;

        let paired = self.is_paired();
        let paused = paired && self.paused;
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
            Conn::Reconnecting { backoff, why } => (
                State::Reconnecting { since: backoff.since },
                Tone::Notice,
                "Reconnecting...".into(),
                Some(why.clone()),
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
        let mut unavailable = self.unavailable_features();
        unavailable.extend(self.presence.reasons().map(str::to_owned));
        let issues = app.pairing.pending.issues(app, self.refused_move_at(Instant::now()));
        let mut view = LinkView {
            issues,
            state,
            tone,
            text,
            why,
            unavailable,
            can_play: app.loaded_map().is_some()
                && matches!(self.waiting, Waiting::Idle | Waiting::Pick)
                && !matches!(self.conn, Conn::Connecting | Conn::Reconnecting { .. }),
            playtest_busy: !matches!(self.waiting, Waiting::Idle | Waiting::Pick),
            can_stop: self.child.is_some(),
            paired,
            controls: self.controls(),
            paused,
        };
        if let Some(summary) = view.issue_summary() {
            view.text.push_str(" · ");
            view.text.push_str(&summary);
        }
        view
    }

    /// Leave a UI request for the next update.
    pub fn request(&mut self, r: Request) {
        if matches!(r, Request::Edit(false) | Request::Unpair) {
            self.objects.cancel_unsent();
        }
        self.queue.push(r);
    }

    /// Withdraw an offered object write before publishing the View transition.
    fn enter_view(&mut self) {
        self.objects.cancel_unsent();
        self.live_edits = false;
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
            Ok(endpoint) => self.request(Request::PairSocket(endpoint)),
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
        if matches!(self.conn, Conn::Connected | Conn::Connecting | Conn::Reconnecting { .. }) {
            return None;
        }
        pair::notice(&self.finder.running(), app)
    }

    /// How the paired game's map compares with the one open here.
    pub fn map_match(&self, app: &App) -> MapMatch {
        pair::map_match(app, self.map())
    }

    fn request_pair(&mut self, app: &mut App, endpoint: Endpoint, source: pair::Source) {
        if self.is_paired() && self.endpoint() != Some(&endpoint) {
            self.edits.next_pair = Some((endpoint, source));
            self.request_leave(app);
        } else {
            self.pair(app, endpoint);
            self.pair_source = source;
        }
    }

    /// Attach to a game offered by the finder. The game is taken
    /// to hold the map as the editor shows it now.
    pub fn pair(&mut self, app: &mut App, endpoint: Endpoint) {
        let own = self.child_endpoint.as_ref() == Some(&endpoint);
        let play_after_pair = self.waiting == Waiting::Pick;
        self.start(app, endpoint, Options::default());
        if play_after_pair {
            self.waiting = Waiting::Pair;
        }
        self.edits.own_start = own;
        self.baseline = None;
        app.info(format!(
            "pairing with the game at {}",
            self.endpoint().map(ToString::to_string).unwrap_or_default()
        ));
    }

    fn start(&mut self, app: &mut App, endpoint: Endpoint, options: Options) {
        self.close_presence();
        self.presence = Default::default();
        self.feedback.clear_moves();
        self.pair_source = pair::Source::Finder;
        self.paired_pid = self.pid_for(&endpoint);
        if let Endpoint::Unix(path) = &endpoint {
            self.auto_tried.insert(path.clone());
            self.unpaired.allow(path, self.paired_pid);
        }
        app.tags.live_datums = None;
        app.pairing.clear_mirror();
        app.pairing.play_copy = self
            .launched_copy
            .as_ref()
            .filter(|(game, _, _)| game == &endpoint)
            .map(|(_, revision, checksum)| (*revision, *checksum));
        self.waiting = Waiting::Idle;
        self.methods = None;
        self.playtest_consent = false;
        self.pause_on_return = false;
        self.spawn_after_tick = 0;
        self.spawn_deadline = None;
        self.play_world = app.session.world_rev();
        self.pairing_id += 1;
        self.link = Some(Link::open(endpoint, options));
        self.retry = None;
        self.conn = Conn::Connecting;
        self.status = None;
        self.status_activation = None;
        self.next_live_status = Instant::now();
        self.scenario_tag = None;

        self.session = None;
        self.edits = Edits::default();
        self.reload_copy = None;
        self.enter_view();
        self.reload.clear();
        self.controller = false;
        self.paused = false;
        self.pending_spawn = None;
    }

    /// Drop the pairing (the game keeps running; our control is released).
    pub fn unpair(&mut self, app: &mut App) {
        if let Some(Endpoint::Unix(path)) = self.endpoint().cloned() {
            self.auto_tried.remove(&path);
            self.unpaired.remember(path, self.paired_pid);
        }
        if self.link.is_some() {
            app.info("unpaired from the game");
        }
        self.disconnect(app);
    }

    /// Start the game on the open map with the editor's bytes, paired.
    pub fn launch(&mut self, app: &mut App) {
        if let Err(why) = self.launch_copy(app) {
            app.error(format!("Play: {why}"));
        }
    }

    fn launch_copy(&mut self, app: &mut App) -> Result<(), String> {
        let launch = self.prepare(app)?;
        self.run_launch(app, launch)
    }

    /// The launch for the open map: writes the edited map to the scratch
    /// directory for `H2_MAP_DIR`.
    pub fn prepare(&mut self, app: &mut App) -> Result<launch::Launch, String> {
        let session = &app.session;
        let document = session.map_name().ok_or("open a map first")?;
        let h2 = launch::find_h2(Some(PathBuf::from(self.h2_path.trim())).as_deref())
            .ok_or("no h2 binary: build it beside the editor (cargo build --release -p cli --bin h2), install it (cargo install --path crates/apps/cli), or name it under Advanced in the Game panel")?;
        let disc = app
            .source()
            .and_then(launch::source_disc)
            .or_else(|| editor_core::default_disc(None))
            .ok_or("h2 play needs the disc: open its image or its extracted maps folder, or set H2_DISC")?;
        let data = session
            .maps_dir()
            .parent()
            .map(PathBuf::from)
            .ok_or("no data root")?;
        let map_dir = launch::scratch().join("maps");
        let level = launch::write_launch_copy(session, &disc, &data, &map_dir)?;
        // A Map Pack map plays from the editor's bytes (the map override) on
        // the Halo 2 disc. `h2 play` installs the map pack itself from beside
        // that disc (`mappack.iso`); say so when this one is elsewhere.
        if app.is_mappack_map(document)
            && let Some(pack) = app.mappack().map(|p| p.path().to_owned())
        {
            let found = editor_core::mappack::beside(&disc);
            if !found.is_some_and(|f| matches!((f.canonicalize(), pack.canonicalize()), (Ok(a), Ok(b)) if a == b)) {
                app.info(format!(
                    "Play: {document} is a Map Pack map and {} is not beside the disc, so h2 play will not install the map pack; it plays from the editor's copy. Put the pack beside the disc as mappack.iso to install it as well",
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
            map_dir: Some(map_dir),
        })
    }

    fn run_launch(&mut self, app: &mut App, l: launch::Launch) -> Result<(), String> {
        let spawn = self.destination(app);
        self.stop_game(app);
        if self.spawn_choice(app) != Spawn::Level && spawn.is_none() {
            app.error(format!(
                "Play: no {} to spawn at; the level's spawn is used",
                self.spawn_choice(app).label().to_lowercase()
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
                let raw = raw_sync::RawBaseline::of_copy(app);
                self.launch_baseline = app
                    .session
                    .scenario()
                    .map(|s| (Baseline::of(s), tag_sync::TagBaseline::of(&app.tags), raw));
                self.pending_spawn = spawn;
                self.waiting = Waiting::Launch;
                app.info(format!("Play: {line} (log: {})", log.display()));
                Ok(())
            }
            Err(e) => Err(format!("starting {}: {e}", l.h2.display())),
        }
    }

    /// End the game this editor started.
    pub fn stop_game(&mut self, app: &mut App) {
        if let Some(mut c) = self.child.take() {
            if matches!(c.try_wait(), Ok(None)) {
                let _ = c.kill();
                app.info("stopped the game");
            }
            let _ = c.wait();
            self.child_gone(app);
        }
    }

    fn child_gone(&mut self, app: &mut App) {
        if let Some(ep) = self.child_endpoint.take() {
            self.finder.forget(&ep);
            if self.launched_copy.as_ref().is_some_and(|(game, _, _)| game == &ep) {
                self.launched_copy = None;
            }
            if self.endpoint() == Some(&ep) {
                self.disconnect(app);
            }
        }
    }

    /// The position selected by [`Self::spawn_choice`], if any.
    pub fn spawn_point(&self, app: &App) -> Option<Vec3> {
        match self.spawn_choice(app) {
            Spawn::Level => None,
            Spawn::Resume => self.saved_place(app).map(|p| p.pos),
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
        self.update_at(app, Instant::now());
    }

    fn update_at(&mut self, app: &mut App, now: Instant) {
        self.expire_reload(app, now);
        if self.waiting != Waiting::Idle && self.play_world != app.session.world_rev() {
            self.cancel_reload();
        }
        if self.finder.poll() {
            self.unpaired.retain_processes(auto::process_alive);
            if let Some(pid) = self.endpoint().and_then(|ep| self.pid_for(ep)) {
                self.paired_pid = Some(pid);
            }
            self.auto_tried.retain(|path| {
                self.finder.games.iter().any(|g| &g.path == path)
                    || matches!(&self.child_endpoint, Some(Endpoint::Unix(own)) if own == path)
            });
        }
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
            self.child_gone(app);
        }
        self.reconnect(app, now);
        let events = self.link.as_ref().map(Link::drain).unwrap_or_default();
        for e in events {
            self.on_event(app, e);
        }
        if let Some(stream) = &app.pairing.mirror {
            let previous_epoch = app.pairing.shown.as_ref().map(|sample| sample.epoch);
            app.pairing.display.refresh(
                stream.snapshot().as_deref(),
                now,
                app.pairing.latest || self.paused,
                &mut app.pairing.shown,
            );
            if let Some(sample) = &app.pairing.shown
                && (previous_epoch != Some(sample.epoch) || now >= self.next_live_status)
            {
                if app.pairing.stream_map.is_null() {
                    app.pairing.stream_map = stream.status(false)["map"].clone();
                }
                self.next_live_status = now + Duration::from_secs(1);
                if self.has_method(agent_proto::live::STATUS) {
                    self.call(agent_proto::live::STATUS, Value::Null, Purpose::LiveStatus);
                }
            }
        }
        // Revalidate queued controls against the newest map, session and local unit.
        self.take_requests(app);
        self.send_rules_reload(app);
        self.auto_pair(app);
        self.remember_place(app);
        self.maybe_spawn(app);
        self.continue_playtest(app);
        if self.waiting == Waiting::Idle && std::mem::take(&mut self.pause_on_return) {
            self.window_focus(app, true);
        }
        if !matches!(self.conn, Conn::Reconnecting { .. }) {
            if now >= self.next_push {
                self.next_push = now + PUSH_EVERY;
                self.push_edits(app);
            } else if self.edits.document_changed(app) {
                self.refresh_pending(app, false);
            }
        }
        let loaded = self.baseline.as_ref()
            .filter(|_| self.edits.world == Some(app.session.world_rev()))
            .map(|(_, base)| base);
        app.pairing.update_placement_layouts(&app.session, loaded);
        select::update(app, now);
        Self::update_shared_camera(app);
        self.update_objects(app, now);
        self.expire_move_feedback(app, now);
        self.publish(app);
        self.update_presence(app, now);
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
                Request::Pair(e) => self.request_pair(app, e, pair::Source::Finder),
                Request::PairSocket(e) => self.request_pair(app, e, pair::Source::Named),
                Request::Unpair => self.request_leave(app),
                Request::Apply(action) => {
                    if let Err(why) = self.apply_edits(app, action) {
                        app.error(why);
                    }
                }
                Request::Play | Request::PlayAnswer(..) => {
                    let answer = match r {
                        Request::PlayAnswer(prompt, choice) => Some((prompt, choice)),
                        _ => None,
                    };
                    if let Err(why) = self.playtest(app, answer) {
                        app.error(why);
                    }
                }
                Request::Stop => self.stop_game(app),
                Request::Edit(true) => {
                    if let Err(why) = self.apply_edits(app, Apply::Send) {
                        app.error(why);
                    }
                }
                Request::Edit(false) => {
                    if let Some(copy) = &mut self.reload_copy {
                        copy.resume_edits = false;
                    }
                    self.enter_view();
                    self.edits.own_start = false;
                    self.playtest_consent = false;
                }
                Request::Pause => self.pause(),
                Request::Resume => self.resume(),
                Request::Step(ticks) => self.step(ticks),
                Request::MovePlayer(request) => {
                    if let Err(why) = self.send_player_move(app, request) {
                        self.game_refusal(app, why);
                    }
                }
                Request::PlayerMoveRefused(scope, why) => {
                    if scope == self.move_context(app) {
                        self.game_refusal(app, why);
                    }
                }
            }
        }
    }

    pub fn auto_target(&self, on: bool, app: &App) -> Option<PathBuf> {
        if !on
            || !matches!(self.conn, Conn::Off | Conn::Lost(_))
            || (matches!(self.conn, Conn::Lost(_)) && self.pair_source == pair::Source::Named)
        {
            return None;
        }
        auto::choose(
            &self.finder.running(),
            |game| pair::map_match(app, game.map()) == MapMatch::Same,
            self.child_endpoint.as_ref(),
            |game| {
                self.auto_tried.contains(&game.path)
                    || self.unpaired.contains(&game.path, self.pid_for(&game.endpoint()))
            },
        )
    }

    fn auto_pair(&mut self, app: &mut App) {
        if let Some(path) = self.auto_target(app.pairing.auto_pair, app) {
            self.pair(app, Endpoint::Unix(path));
        }
    }

    fn pid_for(&self, ep: &Endpoint) -> Option<u32> {
        let Endpoint::Unix(path) = ep else {
            return None;
        };
        if Some(ep) == self.child_endpoint.as_ref()
            && let Some(child) = &self.child
        {
            return Some(child.id());
        }
        self.finder
            .games
            .iter()
            .find(|g| &g.path == path)
            .and_then(|g| g.pid)
            .or_else(|| agent_proto::discover::pid_of(path))
    }

    fn reconnectable(&self) -> bool {
        self.retry.as_ref().is_some_and(|ep| {
            (self.pair_source == pair::Source::Named && self.conn != Conn::Connecting)
                || Some(ep) == self.child_endpoint.as_ref()
                || self.finder.games.iter().any(|g| &g.endpoint() == ep)
        })
    }

    fn disconnect(&mut self, app: &mut App) {
        self.close_presence();
        self.presence = Default::default();
        self.feedback.clear_moves();
        self.retry = None;
        self.pair_source = pair::Source::Finder;
        self.paired_pid = None;
        self.conn = Conn::Off;
        self.pause_on_return = false;
        self.methods = None;
        self.waiting = Waiting::Idle;
        self.status = None;
        self.status_activation = None;
        self.session = None;
        self.baseline = None;
        self.edits = Edits::default();
        self.reload_copy = None;
        self.controller = false;
        self.paused = false;
        self.enter_view();
        self.pending_spawn = None;
        self.reload.clear();
        app.tags.live_datums = None;
        app.pairing.clear_mirror();
    }

    fn reconnect(&mut self, app: &mut App, now: Instant) {
        if !matches!(self.conn, Conn::Reconnecting { .. }) {
            return;
        }
        if !self.reconnectable() {
            app.info("game link closed; the game is no longer listed");
            self.disconnect(app);
            return;
        }
        let Conn::Reconnecting { backoff, why } = &mut self.conn else { return };
        if backoff.expired(now) {
            self.link = None;
            app.error(format!("game link: {why}"));
            self.conn = Conn::Lost(why.clone());
            self.enter_view();
            self.cancel_reload();
        } else if self.link.is_none() && backoff.due(now) {
            self.link = self.retry.clone().map(|ep| Link::open(ep, Options::default()));
        }
    }

    /// What `pair.status` and `pair.list` answer, read each frame.
    fn publish(&self, app: &mut App) {
        let (state, why) = match &self.conn {
            Conn::Off => ("off", None),
            Conn::Connecting => ("connecting", None),
            Conn::Connected => ("connected", None),
            Conn::Reconnecting { why, .. } => ("reconnecting", Some(why.clone())),
            Conn::Lost(w) => ("lost", Some(w.clone())),
        };
        let link = self.link_view(app);
        let offer = self.offer(app, &link);
        if app.pairing.prompt.is_some() && app.pairing.prompt != offer.prompt {
            app.pairing.prompt = None;
        }
        app.pairing.playtest = Some(offer);
        app.pairing.peer_confirmation = self.peer_confirmation();
        let at = self.status.as_ref().map(|s| &s.at);
        let edits_blocked = self.edits_blocked(app);
        let player_scope = self.player_move_scope(app, edits_blocked.as_deref());
        app.pairing.set_player_scope(player_scope);
        app.pairing.move_context = Some(self.move_context(app));
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
            "edits_blocked": edits_blocked,
            "edit_refusal": self.edit_refusal(app),
            "edit_peers": self.edits.peers,
            "auto_pair": app.pairing.auto_pair,
            "games": self.finder.running().len(),
            "ask": app.pairing.prompt.as_ref().map(|p| p.ask.name()),
            "prompt": app.pairing.prompt.as_ref().map(|p| p.id),
            "playtest_busy": link.playtest_busy,
        });
        app.pairing.published = Some(published);
        app.pairing.link = Some(link.into());
    }

    fn call(&self, method: &str, params: Value, purpose: Purpose) {
        if let Some(l) = &self.link {
            l.call(method, params, purpose);
        }
    }

    fn on_event(&mut self, app: &mut App, e: Event) {
        let reconnecting = matches!(self.conn, Conn::Reconnecting { .. });
        match e {
            Event::ObjectSkipped(ticket) => self.objects.skipped(ticket, self.live_edits),
            Event::Connected => {
                if !reconnecting {
                    self.conn = Conn::Connected;
                    app.info(format!(
                        "paired with the game at {}",
                        self.endpoint().map(ToString::to_string).unwrap_or_default()
                    ));
                }
                self.call(method::METHODS, Value::Null, Purpose::Capabilities);
            }
            Event::Lost(why) => {
                if self.is_paired() {
                    self.feedback.toast(format!("Game link dropped: {why}"), true, Instant::now());
                }
                self.objects.transport_lost();
                if matches!(self.waiting, Waiting::Spawn(_)) {
                    app.error("game link closed before the player move was acknowledged");
                }
                self.presence.transport_lost();
                self.feedback.clear_moves();
                self.retry = self.link.take().map(|l| l.endpoint.clone()).or(self.retry.take());
                self.controller = false;
                self.paused = false;
                self.status = None;
                self.status_activation = None;
                self.session = None;
                if let Some(copy) = &mut self.reload_copy {
                    // Its reply may have been lost after the host queued the reload.
                    // The reopened socket only reads the identity of the loaded copy.
                    copy.check_identity = true;
                } else if self.waiting != Waiting::Launch || self.pending_spawn.is_none() {
                    // Once a move is dispatched its outcome is unknown on a drop.
                    self.waiting = Waiting::Idle;
                    self.pending_spawn = None;
                    self.spawn_deadline = None;
                }
                self.methods = None;
                app.pairing.prompt = None;
                app.pairing.drop_stream();
                app.tags.live_datums = None;
                if self.edits.interrupted_write() {
                    // The last write may have reached the game. Re-read its journal before
                    // offering edits or a restore on this connection again.
                    self.enter_view();
                    self.edits = Edits::default();
                    self.baseline = None;
                    app.pairing.leaving = false;
                    app.pairing.journal.clear();
                    app.info("game link interrupted a write; checking the game journal");
                }
                self.next_push = Instant::now();
                self.edits.transport_lost();
                if let Conn::Reconnecting { why: last, .. } = &mut self.conn {
                    *last = why;
                } else if self.reconnectable() {
                    app.info(format!("Game link interrupted: {why}"));
                    self.conn = Conn::Reconnecting {
                        backoff: auto::Backoff::new(Instant::now()),
                        why,
                    };
                } else if self.conn == Conn::Connecting {
                    app.error(format!("game link: {why}"));
                    self.conn = Conn::Lost(why);
                } else {
                    app.info("game link closed; the game is no longer listed");
                    self.disconnect(app);
                }
            }
            Event::Poll(status) => {
                let activation = status["at"]["activation"].as_u64();
                if let Ok(s) = serde_json::from_value::<Status>(status) {
                    self.status_activation = activation;
                    self.paused = s.at.paused;
                    self.controller = s.controller;
                    self.status = Some(s);
                    if self.has_method(agent_proto::hot::method::EDIT_MAP) && !self.edits.map_requested {
                        self.edits.map_requested = true;
                        self.call(
                            agent_proto::hot::method::EDIT_MAP,
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
        self.revalidate_object_dispatch(app);
        if reconnecting && self.is_paired() {
            self.feedback.toast("Game link restored".into(), false, Instant::now());
            app.info("Game link restored");
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
            (kind @ (Purpose::Indicator | Purpose::Marks), result) => {
                self.presence_answer(app, kind, result);
            }
            (Purpose::Capabilities, result) => self.received_methods(app, result),
            (Purpose::Map, result) => self.edit_map(app, result),
            (Purpose::Journal(epoch), result) => self.edit_journal(app, epoch, result),
            (Purpose::Revert { leave, epoch }, result) => self.reverted(app, leave, epoch, result),
            (Purpose::Take { id, at }, result) => self.take_answer(app, id, at, result),
            (Purpose::Object(ticket), result) => self.object_answer(app, ticket, result),
            (Purpose::Edit(id), result) if self.edits.raw.owns(id) => self.raw_answer(app, result),
            (Purpose::Edit(id), result) => self.batch_answer(app, id, result),
            (Purpose::Reload(id), result) => self.reload_answer(app, id, result),
            (Purpose::Focus, Ok(_)) => {}
            (Purpose::Spawn(ticket), result) => {
                if self.waiting == Waiting::Spawn(ticket) {
                    self.pending_spawn = None;
                    self.spawn_deadline = None;
                    match result {
                        Ok(_) => self.finish_playtest(app),
                        Err(e) => {
                            self.waiting = Waiting::Idle;
                            self.feedback.problem(app, format!("could not move the player: {e}"));
                        }
                    }
                }
            }
            (Purpose::Teleport(ticket), result) => self.move_answer(app, ticket, result),
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
            (Purpose::Rules, result) => self.rules_answer(app, result),
            (Purpose::Other, _) if method == agent_proto::editor::method::MARKS => {}
            (_, Err(e)) => self.feedback.problem(app, format!("{method}: {e}")),
            (Purpose::Other, Ok(v)) => app.info(format!("{method}: {v}")),
        }
    }

    /// Sends the script editor's queued reload (`app.rules.outbox`) as
    /// `rules.reload`, or says why it can't go.
    fn send_rules_reload(&mut self, app: &mut App) {
        let Some(params) = app.rules.outbox.take() else {
            return;
        };
        use crate::app::rules_edit::ReloadState;
        let why = if !self.is_paired() {
            Some("pair with a running game first".to_string())
        } else if !self.has_method(agent_proto::rules::method::RELOAD) {
            Some(
                "the paired game has no rules.reload (a build without the rules feature, or H2_FEATURES leaves it out)"
                    .to_string(),
            )
        } else {
            None
        };
        match why {
            Some(why) => app.rules.reload = ReloadState::Refused(why),
            None => self.call(agent_proto::rules::method::RELOAD, params, Purpose::Rules),
        }
    }

    fn rules_answer(&mut self, app: &mut App, result: Result<Value, agent_proto::RpcError>) {
        use crate::app::rules_edit::ReloadState;
        let state = match result {
            Ok(v) => match serde_json::from_value::<agent_proto::rules::ReloadResult>(v) {
                Ok(r) => ReloadState::Done(r),
                Err(e) => ReloadState::Refused(format!("the game's answer did not read: {e}")),
            },
            Err(e) => ReloadState::Refused(e.message),
        };
        if let Some((_, line)) = crate::app::rules_edit::reload_line(&state) {
            if matches!(state, ReloadState::Refused(_)) {
                app.error(line);
            } else {
                app.info(line);
            }
        }
        app.rules.reload = state;
    }

    /// Why edits aren't being sent to the game now, if they aren't.
    pub fn edits_blocked(&self, app: &App) -> Option<String> {
        if !self.is_paired() {
            return Some("not paired".into());
        }
        self.edit_refusal(app)
            .or_else(|| (!self.live_edits).then(|| "View mode".into()))
    }

    fn arm_spawn_wait(&mut self) {
        self.spawn_deadline = self.pending_spawn.map(|_| Instant::now() + SPAWN_WAIT);
    }

    fn abandon_spawn(&mut self, app: &mut App, why: &str) {
        self.pending_spawn = None;
        self.spawn_deadline = None;
        self.waiting = Waiting::Idle;
        app.error(format!("Play: {why}"));
    }

    fn maybe_spawn(&mut self, app: &mut App) {
        let Some(destination) = self.pending_spawn else {
            return;
        };
        if self.waiting == Waiting::Reload || !self.is_paired() {
            return;
        }
        let deadline = *self
            .spawn_deadline
            .get_or_insert_with(|| Instant::now() + SPAWN_WAIT);
        if Instant::now() >= deadline {
            self.abandon_spawn(
                app,
                "timed out waiting for the local player to reach the chosen spawn",
            );
            return;
        }
        if matches!(self.waiting, Waiting::Spawn(_)) || self.methods.is_none() {
            return;
        }
        if let Some(missing) = [
            agent_proto::sim::method::STATE_SET,
            agent_proto::hot::method::EDIT_MAP,
            agent_proto::hot::method::EDIT_CHANGES,
            agent_proto::live::SUBSCRIBE,
        ]
        .into_iter()
        .find(|name| !self.has_method(name))
        {
            self.abandon_spawn(app, &format!("moving the local player needs {missing}"));
            return;
        }
        let Some(s) = &self.status else { return };
        if s.at.phase != Phase::Gameplay
            || s.at.map_ticks < SPAWN_AFTER_TICKS
            || s.at.ticks < self.spawn_after_tick
            || self.session.is_none()
            || !self.edits.journal_ready
            || self.map_match(app) != MapMatch::Same
            || self.edits.epoch.is_none()
        {
            return;
        }
        if app.pairing.shown.as_ref().is_some_and(|s| !app.pairing.matches_sample(app, s)) {
            return;
        }
        if let Some(why) = self.map_refusal(app).or_else(|| self.session_refusal(app)) {
            self.abandon_spawn(app, &why);
            return;
        }
        let Some((player, _)) = app.pairing.player().filter(|(p, _)| p.local_user.is_some()) else {
            return;
        };
        let mut params = match destination {
            Destination::Position(pos) => json!({"pos": pos}),
            Destination::Place(place) => place.json(),
        };
        params["player"] = json!(format!("player.{}", player.index));
        self.waiting = Waiting::Spawn(deadline);
        self.call(agent_proto::sim::method::STATE_SET, params, Purpose::Spawn(deadline));
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
    pub fn teleport(&mut self, app: &mut App, pos: Vec3) {
        self.player_here(app, pos, false);
    }

    /// Move the player to the ground under the camera's focus.
    pub fn teleport_to_camera(&mut self, app: &mut App) {
        let p = ground(app, app.session.camera().target);
        self.teleport(app, p);
    }

    pub fn pause(&mut self) {
        if self.controls().pause.is_some() {
            return;
        }
        self.ensure_claim();
        self.call(
            method::PAUSE,
            json!({"max_ms": PAUSE_MAX_MS}),
            Purpose::Control("pause"),
        );
    }
    pub fn resume(&mut self) {
        if self.controls().resume.is_some() {
            return;
        }
        self.ensure_claim();
        self.call(method::RESUME, Value::Null, Purpose::Control("resume"));
        self.release_control();
    }

    fn release_control(&mut self) {
        if !self.controller || !self.has_method(method::RELEASE) {
            return;
        }
        // Give control back so other clients (and agents) may act.
        self.call(method::RELEASE, Value::Null, Purpose::Control("release"));
        self.controller = false;
    }
    pub fn step(&mut self, ticks: u64) {
        if self.controls().step.is_some() {
            return;
        }
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
    surface_under_ray(app, &app.session.camera().ray(u, v, aspect))
}

fn surface_under_ray(app: &App, ray: &Ray) -> Option<Vec3> {
    // A hidden structure is not a surface the cursor can land on.
    let at = app.session.surface_hit_visible(ray, &app.layers)?;
    Some([at[0], at[1], at[2] + LIFT])
}
