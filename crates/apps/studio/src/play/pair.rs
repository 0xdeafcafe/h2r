//! Finding the games to pair with, and the agent API's way into pairing.
//!
//! * [`Finder`]: looks for running games on its own thread (the games'
//!   default sockets in the agents directory, `agent_proto::discover`, and
//!   the other sockets `launch::discover` lists) and keeps the last answer.
//! * [`Notice`]: what the editor says when it sees a game it is not paired
//!   with, and whether that game's map is the one open here.
//! * [`Bridge`]: on [`crate::App`], where `pair.*` (`crate::api::pair`)
//!   leaves requests for [`super::Play`] and reads what it published, since
//!   the pairing itself lives with the window.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use agent_proto::Endpoint;
use agent_proto::discover::{self, Found, MapMatch};
use serde_json::Value;

use super::{LinkState, LinkView, launch};

/// How often the finder looks again.
pub const SCAN_EVERY: Duration = Duration::from_secs(1);

/// What a scan does: the games it found.
pub type ScanFn = Arc<dyn Fn() -> Vec<Found> + Send + Sync>;

/// The scan the editor runs: the agents directory (leftover sockets are
/// removed there), and the other sockets `launch::discover` knows (Play's
/// own, `$H2_AGENT`, the default `h2 serve` socket), which are never
/// removed.
pub fn default_scan() -> Vec<Found> {
    let dir = discover::dir();
    let mut found = discover::scan(&dir, discover::PROBE_TIMEOUT);
    let others: Vec<PathBuf> = launch::discover()
        .into_iter()
        .filter(|p| discover::is_socket(p) && p.parent() != Some(dir.as_path()))
        .collect();
    found.extend(discover::scan_paths(&others, discover::PROBE_TIMEOUT, false));
    found
}

/// Looks for games now and then, off the UI thread.
pub struct Finder {
    scan: ScanFn,
    pending: Option<Receiver<Vec<Found>>>,
    last: Option<Instant>,
    /// The last scan's games.
    pub games: Vec<Found>,
    /// A scan has finished at least once.
    pub scanned: bool,
}

impl Default for Finder {
    fn default() -> Self {
        Self::with_scan(Arc::new(default_scan))
    }
}

impl Finder {
    pub fn with_scan(scan: ScanFn) -> Finder {
        Finder {
            scan,
            pending: None,
            last: None,
            games: Vec::new(),
            scanned: false,
        }
    }

    /// Once a frame: takes a finished scan, starts the next when due.
    pub fn poll(&mut self) -> bool {
        let mut refreshed = false;
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(found) => {
                    refreshed = true;
                    self.games = found;
                    self.scanned = true;
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let due = self.last.is_none_or(|t| t.elapsed() >= SCAN_EVERY);
        if self.pending.is_none() && due {
            self.start();
        }
        refreshed
    }

    /// Looks again at the next [`Finder::poll`].
    pub fn look_again(&mut self) {
        self.last = None;
    }

    /// A child exit is newer than either a cached or an in-flight scan.
    pub(super) fn forget(&mut self, endpoint: &Endpoint) {
        self.games.retain(|g| &g.endpoint() != endpoint);
        self.pending = None;
        self.look_again();
    }

    fn start(&mut self) {
        self.last = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        let scan = Arc::clone(&self.scan);
        let spawned = std::thread::Builder::new()
            .name("h2-editor-find".into())
            .spawn(move || {
                let _ = tx.send(scan());
            });
        if spawned.is_ok() {
            self.pending = Some(rx);
        }
    }

    /// The games that answered (a busy listener may be a game loading, but
    /// is not offered).
    pub fn running(&self) -> Vec<&Found> {
        self.games.iter().filter(|g| g.status.is_some()).collect()
    }
}

/// The game reports the cache header name, which a renamed document can retain.
pub(crate) fn map_match(app: &crate::App, game: Option<&str>) -> MapMatch {
    let by_name = MapMatch::of(app.loaded_map(), game);
    let cache = app.session.source_identity().map(|(name, _)| name);
    if by_name == MapMatch::Different && MapMatch::of(cache, game) == MapMatch::Same {
        MapMatch::Same
    } else {
        by_name
    }
}

/// How a game's map compares with the editor's, in words.
pub fn relation_text(m: MapMatch, game: Option<&str>, open: Option<&str>) -> String {
    match (m, game, open) {
        (MapMatch::Same, Some(g), _) => format!("same map as the one open here ({g})"),
        (MapMatch::Different, Some(g), Some(o)) => format!("{g}; the editor has {o} open"),
        (_, Some(g), _) => g.to_owned(),
        (_, None, _) => "no map yet".to_owned(),
    }
}

/// What the editor tells the user about a game it can pair with.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    /// For the game link pill.
    pub short: String,
    /// The relationship between the game's map and the editor's.
    pub text: String,
    /// The socket the Pair button pairs with; `None` with several games
    /// (the user picks one).
    pub socket: Option<PathBuf>,
    pub map: Option<String>,
    pub matches: MapMatch,
}

/// The notice for the games the finder sees, when not paired.
pub fn notice(games: &[&Found], app: &crate::App) -> Option<Notice> {
    match games {
        [] => None,
        [g] => {
            let map = g.map();
            let m = map_match(app, map);
            let shown = map.unwrap_or("no map yet");
            let text = match m {
                MapMatch::Same => format!("Halo 2 is running {shown}, the map open here."),
                MapMatch::Different => format!(
                    "Halo 2 is running {shown}. The editor has {} open.",
                    app.loaded_map().unwrap_or("-")
                ),
                MapMatch::Unknown => format!("Halo 2 is running ({shown})."),
            };
            Some(Notice {
                short: map.map_or_else(
                    || "Halo 2 is running".into(),
                    |map| format!("Halo 2 is running {map}"),
                ),
                text,
                socket: Some(g.path.clone()),
                map: map.map(str::to_owned),
                matches: m,
            })
        }
        many => Some(Notice {
            short: format!("{} games are running", many.len()),
            text: format!("{} Halo 2 games are running: pick one under Pair.", many.len()),
            socket: None,
            map: None,
            matches: MapMatch::Unknown,
        }),
    }
}

/// A request left for [`super::Play`].
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Pair(Endpoint),
    PairSocket(Endpoint),
    Unpair,
    Apply(super::Apply),
    Play,
    PlayAnswer(super::playtest::Prompt, String),
    Stop,
    Edit(bool),
    Pause,
    Resume,
    Step(u64),
    MovePlayer(super::presence::PlayerMove),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    Finder,
    Named,
}

impl Request {
    pub fn refusal(&self, link: &LinkView) -> Option<&'static str> {
        match self {
            Self::Play | Self::PlayAnswer(..) if matches!(link.state, LinkState::Connecting | LinkState::Reconnecting { .. }) => {
                Some("wait for the game link to connect or cancel it")
            }
            Self::Play | Self::PlayAnswer(..) if link.playtest_busy => {
                Some("a playtest is already in progress")
            }
            Self::Play | Self::PlayAnswer(..) if !link.can_play => Some("open a map first"),
            Self::Stop if !link.can_stop => Some("the editor has no game of its own to stop"),
            Self::Edit(_) if !link.state.can_edit() => Some("pair with a game first"),
            Self::Pause | Self::Resume | Self::Step(_) if !link.can_pause => {
                Some("pair with a game first")
            }
            Self::Step(_) if !link.paused => Some("pause the game before stepping"),
            _ => None,
        }
    }
}

/// The host and session whose peer count the user saw in Pending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerConfirmation {
    pub(super) pairing: u64,
    pub(super) epoch: u64,
    pub peers: u32,
}

/// Where the agent API and the window's [`super::Play`] meet, on the app
/// they both see.
#[derive(Debug)]
pub struct Bridge {
    /// Pair on its own with the one game running, once per game. On by default.
    pub auto_pair: bool,
    pub pause_while_editing: bool,
    pub spawn: Option<super::Spawn>,
    pub playtest: Option<super::playtest::Offer>,
    pub prompt: Option<super::playtest::Prompt>,
    pub(super) next_prompt: u64,
    pub peer_confirmation: Option<PeerConfirmation>,
    /// Left by `pair.*`, taken once a frame.
    pub requests: Vec<Request>,
    /// What `Play` last published: `None` until a window has drawn a
    /// frame (headless, there is no pairing).
    pub published: Option<Value>,
    /// The same game-link state the bar and Game panel read.
    pub link: Option<LinkView>,
    pub pending: Vec<Value>,
    pub journal: Vec<Value>,
    pub leaving: bool,
    /// One socket and its exact tick samples for the current pairing.
    pub mirror: Option<live_mirror::Stream>,
    /// The sample shown this editor frame, shared by the viewport and API.
    pub shown: Option<live_mirror::Sample>,
    pub latest: bool,
    pub(crate) selection: super::select::LiveSelection,
    pub(crate) camera: super::presence::SharedCamera,
    /// Slot layouts verified against a loaded play copy. An absent baseline uses source slots.
    layouts: Vec<(scenario_edit::BlockKind, Option<u64>)>,
    /// Pairing, editor document and game epoch that accept a player move.
    pub(crate) player_scope: Option<(u64, u64, u64)>,
    pub(crate) player_move_available: bool,
    /// Counters reported by the game's writer for this stream.
    pub counters: Option<Value>,
    /// Map identity from the handshake, refreshed over live.status after a cut.
    pub stream_map: Value,
    /// A Play copy may include edits. Its checksum belongs to this open document.
    pub(super) play_copy: Option<(u64, u32)>,
}

impl Default for Bridge {
    fn default() -> Self {
        Bridge {
            auto_pair: true,
            pause_while_editing: false,
            spawn: None,
            playtest: None,
            prompt: None,
            next_prompt: 1,
            peer_confirmation: None,
            requests: Vec::new(),
            published: None,
            link: None,
            pending: Vec::new(),
            journal: Vec::new(),
            leaving: false,
            mirror: None,
            shown: None,
            latest: false,
            selection: Default::default(),
            camera: Default::default(),
            layouts: Vec::new(),
            player_scope: None,
            player_move_available: false,
            counters: None,
            stream_map: Value::Null,
            play_copy: None,
        }
    }
}

impl Bridge {
    pub(crate) fn show_prompt(&mut self, prompt: Option<super::playtest::Prompt>) {
        if let Some(prompt) = &prompt {
            self.next_prompt = prompt.id + 1;
        }
        self.prompt = prompt;
    }

    pub(super) fn clear_mirror(&mut self) {
        self.prompt = None;
        self.peer_confirmation = None;
        self.pending.clear();
        self.journal.clear();
        self.leaving = false;
        self.play_copy = None;
        self.drop_stream();
    }

    pub(super) fn drop_stream(&mut self) {
        self.selection = Default::default();
        self.camera = Default::default();
        self.layouts.clear();
        self.player_scope = None;
        self.player_move_available = false;
        self.mirror = None;
        self.shown = None;
        self.counters = None;
        self.stream_map = Value::Null;
    }

    pub(super) fn update_placement_layouts(
        &mut self,
        session: &scenario_edit::Session,
        loaded: Option<&super::sync::Baseline>,
    ) {
        self.layouts.clear();
        if let (Some(scenario), Some(loaded)) = (session.scenario(), loaded) {
            self.layouts.extend(scenario_edit::layout::BLOCKS.iter().map(|block| {
                let kind = block.kind;
                let revision = loaded.layout_matches(scenario, kind)
                    .then(|| scenario.block(kind).layout_rev());
                (kind, revision)
            }));
        }
    }

    pub(crate) fn placement_matches(
        &self,
        session: &scenario_edit::Session,
        kind: scenario_edit::BlockKind,
    ) -> bool {
        let Some(scenario) = session.scenario() else { return true };
        let current = scenario.block(kind).layout_rev();
        match self.layouts.iter().find(|(block, _)| *block == kind) {
            Some((_, revision)) => *revision == Some(current),
            None => self.play_copy.is_none() && current == 0,
        }
    }

    /// Only resolve definition handles and world-space coordinates against
    /// the editor map when the name belongs to this exact frame's epoch.
    pub(crate) fn matches_sample(&self, app: &crate::App, sample: &live_mirror::Sample) -> bool {
        if !app.session.is_open()
            || self.stream_map["epoch"].as_u64() != Some(sample.epoch)
            || map_match(app, self.stream_map["name"].as_str()) != MapMatch::Same
        {
            return false;
        }
        let expected = self.play_copy
            .filter(|(revision, _)| *revision == app.session.world_rev())
            .map(|(_, sum)| sum)
            .or_else(|| app.session.source_checksum());
        match (expected, self.stream_map["checksum"].as_u64()) {
            (Some(a), Some(b)) => u64::from(a) == b,
            // Different documents can retain the same internal cache name.
            // Using that alias for coordinates needs the checksum as well.
            _ => MapMatch::of(app.loaded_map(), self.stream_map["name"].as_str()) == MapMatch::Same,
        }
    }

    pub fn player(&self) -> Option<(&agent_proto::live::Player, &agent_proto::live::Object)> {
        let shown = self.shown.as_ref()?;
        shown.players.iter().filter_map(|player| {
            let unit = player.unit?;
            let object = shown.objects.iter().find(|object| object.id == unit)?;
            Some((player, object))
        }).min_by_key(|(player, _)| (player.local_user.unwrap_or(u16::MAX), player.index))
    }
}
