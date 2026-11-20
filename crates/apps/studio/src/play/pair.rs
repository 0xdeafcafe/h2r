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

use super::launch;

/// How often the finder looks again.
pub const SCAN_EVERY: Duration = Duration::from_secs(2);

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
    pub fn poll(&mut self) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(found) => {
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
    }

    /// Looks again at the next [`Finder::poll`].
    pub fn look_again(&mut self) {
        self.last = None;
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
    /// For the status bar.
    pub short: String,
    /// For the Game panel.
    pub text: String,
    /// The socket the Pair button pairs with; `None` with several games
    /// (the user picks one).
    pub socket: Option<PathBuf>,
    pub matches: MapMatch,
}

/// The notice for the games the finder sees, when not paired.
pub fn notice(games: &[&Found], open: Option<&str>) -> Option<Notice> {
    match games {
        [] => None,
        [g] => {
            let map = g.map();
            let m = MapMatch::of(open, map);
            let shown = map.unwrap_or("no map yet");
            let text = match m {
                MapMatch::Same => format!(
                    "Halo 2 is running {shown}, the map open here. It can pair: edits to its placements reach it as you make them."
                ),
                MapMatch::Different => format!(
                    "Halo 2 is running {shown}; the editor has {} open. It can pair, but edits stay here until the maps match.",
                    open.unwrap_or("-")
                ),
                MapMatch::Unknown => format!(
                    "Halo 2 is running ({shown}). It can pair; edits start once it is on the map open here."
                ),
            };
            Some(Notice {
                short: match m {
                    MapMatch::Same => format!("Halo 2 is running {shown} (matches)"),
                    _ => format!("Halo 2 is running ({shown})"),
                },
                text,
                socket: Some(g.path.clone()),
                matches: m,
            })
        }
        many => Some(Notice {
            short: format!("{} games are running", many.len()),
            text: format!("{} Halo 2 games are running: pick one under Pair.", many.len()),
            socket: None,
            matches: MapMatch::Unknown,
        }),
    }
}

/// A request left for [`super::Play`].
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Pair(Endpoint),
    Unpair,
}

/// Where the agent API and the window's [`super::Play`] meet, on the app
/// they both see.
#[derive(Debug)]
pub struct Bridge {
    /// Pair on its own with the one game running, once per game. On by
    /// default: pairing only reads the game; nothing is sent until an
    /// edit is made on a matching map.
    pub auto_pair: bool,
    /// Left by `pair.connect` and `pair.disconnect`, taken once a frame.
    pub requests: Vec<Request>,
    /// What `Play` last published: `None` until a window has drawn a
    /// frame (headless, there is no pairing).
    pub published: Option<Value>,
    /// One socket and its exact tick samples for the current pairing.
    pub mirror: Option<live_mirror::Stream>,
    /// The sample shown this editor frame, shared by the viewport and API.
    pub shown: Option<live_mirror::Sample>,
    pub latest: bool,
    pub show_objects: bool,
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
            requests: Vec::new(),
            published: None,
            mirror: None,
            shown: None,
            latest: false,
            show_objects: true,
            counters: None,
            stream_map: Value::Null,
            play_copy: None,
        }
    }
}

impl Bridge {
    pub(super) fn clear_mirror(&mut self) {
        self.mirror = None;
        self.shown = None;
        self.counters = None;
        self.stream_map = Value::Null;
        self.play_copy = None;
    }

    /// Only resolve definition handles and world-space coordinates against
    /// the editor map when the name belongs to this exact frame's epoch.
    pub(crate) fn matches_sample(&self, app: &crate::App, sample: &live_mirror::Sample) -> bool {
        if !app.session.is_open()
            || self.stream_map["epoch"].as_u64() != Some(sample.epoch)
            || MapMatch::of(app.loaded_map(), self.stream_map["name"].as_str()) != MapMatch::Same
        {
            return false;
        }
        let expected = self.play_copy
            .filter(|(revision, _)| *revision == app.session.world_rev())
            .map(|(_, sum)| sum)
            .or_else(|| app.session.source_checksum());
        match (expected, self.stream_map["checksum"].as_u64()) {
            (Some(a), Some(b)) => u64::from(a) == b,
            _ => true,
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
