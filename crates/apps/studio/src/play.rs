//! Play and pair: start the game on the map being edited, or attach to a
//! game already running, and keep the two together while it runs
//! (docs/editor/editor.md, "Play and pair").
//!
//! * [`launch`]: the `h2 play` command, the `h2` binary, socket discovery.
//! * [`link`]: the agent socket, on its own thread.
//! * [`sync`]: which scenario edits go to the game, as `edit.set` writes.
//!
//! [`Play`] holds a pairing's state and turns the link's answers into the
//! editor's: the live players and objects for the viewport, the status
//! chip, edits sent as they are made, the spawn of a launched game, and
//! the camera and teleport moves. The UI (`ui::play`) only draws it and
//! calls its methods; tests drive it against the agent crate's server.

pub mod launch;
pub mod link;
pub mod sync;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use agent_proto::sim::SessionKind;
use agent_proto::{Endpoint, Phase, Status, method};
use scenario_edit::BlockKind;
use scenario_edit::geometry::{self, Ray, Vec3};
use serde_json::{Value, json};

use crate::app::App;
use link::{Event, Link, Options, Purpose};
use sync::{Baseline, Write};

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

/// An object the game reported.
#[derive(Clone, Debug, PartialEq)]
pub struct Live {
    pub id: String,
    pub kind: String,
    pub pos: Vec3,
    pub forward: Option<Vec3>,
    pub team: Option<i32>,
    pub name: Option<String>,
}

impl Live {
    fn from(v: &Value) -> Option<Live> {
        // A turret's position is relative to its vehicle: not drawn.
        if v.get("mounted_on").is_some_and(|m| !m.is_null()) {
            return None;
        }
        let vec = |k: &str| -> Option<Vec3> {
            let a = v.get(k)?.as_array()?;
            Some([
                a.first()?.as_f64()? as f32,
                a.get(1)?.as_f64()? as f32,
                a.get(2)?.as_f64()? as f32,
            ])
        };
        Some(Live {
            id: v.get("id")?.as_str()?.to_owned(),
            kind: v.get("kind")?.as_str()?.to_owned(),
            pos: vec("pos")?,
            forward: vec("forward"),
            team: v.get("team").and_then(Value::as_i64).map(|t| t as i32),
            name: v.get("name").and_then(Value::as_str).map(str::to_owned),
        })
    }
}

/// A pairing with a game, launched or attached.
pub struct Play {
    link: Option<Link>,
    pub conn: Conn,
    /// The last `session.status`.
    pub status: Option<Status>,
    /// The last poll's objects.
    pub objects: Vec<Live>,
    /// When the last poll came.
    pub polled: Option<Instant>,
    /// The game's scenario datum (`edit.tags`), for `edit.set`.
    scenario_tag: Option<String>,
    /// The map the datum and session kind were read on.
    read_on: Option<String>,
    pub session: Option<SessionKind>,
    /// What the game is taken to hold, and the map it is for.
    baseline: Option<(String, Baseline)>,
    /// Send edits as they are made.
    pub live_edits: bool,
    next_push: Instant,
    sent: HashMap<u64, Write>,
    next_write: u64,
    /// Blocks with entries added or removed since the baseline.
    pub reload: Vec<BlockKind>,
    told_when: bool,
    told_mismatch: bool,
    /// We hold `session.claim`.
    pub controller: bool,
    pub paused: bool,
    /// The launched game, its log, and the spawn it waits for.
    child: Option<Child>,
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
            conn: Conn::Off,
            status: None,
            objects: Vec::new(),
            polled: None,
            scenario_tag: None,
            read_on: None,
            session: None,
            baseline: None,
            live_edits: true,
            next_push: Instant::now(),
            sent: HashMap::new(),
            next_write: 0,
            reload: Vec::new(),
            told_when: false,
            told_mismatch: false,
            controller: false,
            paused: false,
            child: None,
            log_path: None,
            pending_spawn: None,
            spawn: Spawn::Level,
            launches: 0,
            h2_path: String::new(),
            socket_text: String::new(),
            last_command: None,
        }
    }

    pub fn endpoint(&self) -> Option<&Endpoint> {
        self.link.as_ref().map(|l| &l.endpoint)
    }
    pub fn is_paired(&self) -> bool {
        self.conn == Conn::Connected
    }
    /// A game this editor started is still running.
    pub fn game_running(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }
    /// The game's map, from the last poll.
    pub fn map(&self) -> Option<&str> {
        self.status.as_ref()?.at.map.as_deref()
    }

    /// The status chip's text: the connection, the game's map and tick.
    pub fn chip(&self) -> String {
        match &self.conn {
            Conn::Off => "Game: not paired".into(),
            Conn::Connecting => "Game: connecting...".into(),
            Conn::Lost(_) => "Game: lost".into(),
            Conn::Connected => match &self.status {
                None => "Game: paired".into(),
                Some(s) => {
                    let phase = match (s.at.paused, s.at.phase) {
                        (true, _) => "paused",
                        (_, Phase::Loading) => "loading",
                        (_, Phase::Menu) => "menu",
                        (_, Phase::Gameplay) => "playing",
                        (_, Phase::Cinematic) => "cinematic",
                    };
                    format!(
                        "Game: {} {phase} tick {}",
                        s.at.map.as_deref().unwrap_or("-"),
                        s.at.tick.unwrap_or(s.at.ticks)
                    )
                }
            },
        }
    }

    /// Attach to a game already running at `endpoint`. The game is taken
    /// to hold the map as the editor shows it now.
    pub fn pair(&mut self, app: &mut App, endpoint: Endpoint) {
        self.start(app, endpoint, Options::default());
        self.baseline = baseline_of(app);
        app.info(format!(
            "pairing with the game at {}",
            self.endpoint().map(ToString::to_string).unwrap_or_default()
        ));
    }

    fn start(&mut self, _app: &mut App, endpoint: Endpoint, options: Options) {
        self.link = Some(Link::open(endpoint, options));
        self.conn = Conn::Connecting;
        self.status = None;
        self.objects.clear();
        self.polled = None;
        self.scenario_tag = None;
        self.read_on = None;
        self.session = None;
        self.sent.clear();
        self.reload.clear();
        self.told_when = false;
        self.told_mismatch = false;
        self.controller = false;
        self.paused = false;
        self.pending_spawn = None;
    }

    /// Drop the pairing (the game keeps running; our control is released).
    pub fn unpair(&mut self, app: &mut App) {
        if self.link.take().is_some() {
            app.info("unpaired from the game");
        }
        self.conn = Conn::Off;
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
            .ok_or("no h2 binary: install it (cargo install --path crates/apps/cli) or name it in the Game panel")?;
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
                self.start(
                    app,
                    Endpoint::Unix(l.socket.clone()),
                    Options {
                        wait: LAUNCH_WAIT,
                        ..Options::default()
                    },
                );
                self.baseline = baseline_of(app);
                self.pending_spawn = spawn;
                app.info(format!("Play: {line} (log: {})", log.display()));
            }
            Err(e) => app.error(format!("Play: starting {}: {e}", l.h2.display())),
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
            self.link = None;
            self.conn = Conn::Off;
        }
        let events = self.link.as_ref().map(Link::drain).unwrap_or_default();
        for e in events {
            self.on_event(app, e);
        }
        self.maybe_spawn(app);
        if Instant::now() >= self.next_push {
            self.next_push = Instant::now() + PUSH_EVERY;
            self.push_edits(app);
        }
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
                app.info(format!(
                    "paired with the game at {}",
                    self.endpoint().map(ToString::to_string).unwrap_or_default()
                ));
            }
            Event::Lost(why) => {
                app.error(format!("game link: {why}"));
                self.conn = Conn::Lost(why);
                self.link = None;
                self.controller = false;
            }
            Event::Poll { status, world } => {
                if let Ok(s) = serde_json::from_value::<Status>(status) {
                    self.paused = s.at.paused;
                    self.controller = s.controller;
                    let map = s.at.map.clone();
                    self.status = Some(s);
                    if map.is_some() && map != self.read_on {
                        // A new map: its scenario and session, read again.
                        self.read_on = map;
                        self.scenario_tag = None;
                        self.session = None;
                        self.call(
                            "edit.tags",
                            json!({"group": "scnr", "limit": 1}),
                            Purpose::Scenario,
                        );
                        self.call(
                            agent_proto::sim::method::SESSION,
                            Value::Null,
                            Purpose::SessionKind,
                        );
                    }
                }
                if let Some(objs) = world.get("objects").and_then(Value::as_array) {
                    self.objects = objs.iter().filter_map(Live::from).collect();
                }
                self.polled = Some(Instant::now());
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
        result: Result<Value, String>,
    ) {
        match (purpose, result) {
            (Purpose::Scenario, Ok(v)) => {
                self.scenario_tag = v.get("scenario").and_then(Value::as_str).map(str::to_owned);
            }
            (Purpose::Scenario, Err(e)) => {
                app.error(format!(
                    "the game can't take tag edits ({e}); live edits are off"
                ));
            }
            (Purpose::SessionKind, Ok(v)) => {
                // Only `kind` is needed (SessionView's other fields may grow).
                self.session = Some(
                    v.get("kind")
                        .cloned()
                        .and_then(|k| serde_json::from_value::<SessionKind>(k).ok())
                        .unwrap_or_default(),
                );
            }
            (Purpose::SessionKind, Err(_)) => self.session = Some(SessionKind::Unknown),
            (Purpose::Edit(id), r) => {
                let Some(w) = self.sent.remove(&id) else {
                    return;
                };
                match r {
                    Ok(v) => {
                        let old = v.get("old").and_then(Value::as_str);
                        if old.is_some_and(|o| o != w.was) && !self.told_mismatch {
                            self.told_mismatch = true;
                            app.error(format!(
                                "the game's {:?} {} {} differed from the editor's before this edit: \
                                 it may be running another version of the map (Play starts it with the editor's)",
                                w.block, w.index, w.field
                            ));
                        }
                    }
                    Err(e) => app.error(format!(
                        "live edit of {:?} {} {}: {e}",
                        w.block, w.index, w.field
                    )),
                }
            }
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
        if !self.live_edits {
            return Some("live edits are off".into());
        }
        let Some(open) = app.loaded_map() else {
            return Some("no map is open".into());
        };
        let Some(game) = self.map() else {
            return Some("the game's map isn't known yet".into());
        };
        if game != open {
            return Some(format!("the game runs {game}, the editor has {open}"));
        }
        match &self.baseline {
            Some((m, _)) if m == open => {}
            _ => return Some("pair again to take the open map as the game's".into()),
        }
        if self.scenario_tag.is_none() {
            return Some("the game's scenario isn't known yet".into());
        }
        match self.session {
            None => Some("the session kind isn't known yet".into()),
            Some(k) => k.writes().err().map(str::to_owned),
        }
    }

    /// Send the fields changed since the baseline.
    fn push_edits(&mut self, app: &mut App) {
        if self.edits_blocked(app).is_some() {
            return;
        }
        let (Some(scenario), Some(tag), Some((_, base))) = (
            app.session.scenario(),
            self.scenario_tag.clone(),
            self.baseline.as_mut(),
        ) else {
            return;
        };
        let d = sync::diff(base, scenario);
        if d.reload != self.reload {
            if !d.reload.is_empty() {
                app.info(format!(
                    "entries added or removed in {:?}: the game sees them after Play again (the map loads anew)",
                    d.reload
                ));
            }
            self.reload = d.reload.clone();
        }
        if d.writes.is_empty() {
            return;
        }
        for w in d.writes {
            base.wrote(&w);
            self.next_write += 1;
            let id = self.next_write;
            if let Some(l) = &self.link {
                l.call(
                    "edit.set",
                    json!({"tag": tag, "group": "scnr", "path": w.path, "value": w.value}),
                    Purpose::Edit(id),
                );
            }
            self.sent.insert(id, w);
        }
        if !self.told_when {
            self.told_when = true;
            app.info(
                "sent the edit to the game's scenario: an object shows it when it is made again \
                 (a respawn, a checkpoint revert, or Play again); live objects don't move yet",
            );
        }
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

    /// The local player (player one), from the last poll.
    pub fn player(&self) -> Option<&Live> {
        let players = self.objects.iter().filter(|o| o.kind == "player");
        players
            .clone()
            .find(|o| o.id == "player.0")
            .or_else(|| players.into_iter().next())
    }

    /// Move the editor camera to the player, looking the way they face.
    pub fn go_to_player(&self, app: &mut App) {
        let Some(p) = self.player() else {
            app.error("no player in the game to go to");
            return;
        };
        let (pos, fwd) = (p.pos, p.forward);
        let cam = app.session.camera_mut();
        cam.target = [pos[0], pos[1], pos[2] + 0.6];
        if let Some(f) = fwd.filter(|f| f[0] != 0.0 || f[1] != 0.0) {
            // Camera::forward is (-cos yaw cos pitch, -sin yaw cos pitch, -sin pitch).
            cam.yaw = (-f[1]).atan2(-f[0]);
        }
        cam.pitch = 0.3;
        cam.distance = 3.0;
        app.info(format!(
            "camera at {} ({:.1}, {:.1}, {:.1})",
            p.id, pos[0], pos[1], pos[2]
        ));
    }

    /// Move the player to `pos` (`sim.state.set`; refused by the game in a
    /// co-op or client session, and here first).
    pub fn teleport(&self, app: &mut App, pos: Vec3) {
        if !self.is_paired() {
            app.error("not paired with a game");
            return;
        }
        if let Some(Err(e)) = self.session.map(SessionKind::writes) {
            app.error(format!("not moving the player: {e}"));
            return;
        }
        let player = self
            .player()
            .map_or("player.0".to_owned(), |p| p.id.clone());
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

/// The editor's placements as the game is taken to hold them, for the
/// open map.
fn baseline_of(app: &App) -> Option<(String, Baseline)> {
    Some((
        app.loaded_map()?.to_owned(),
        Baseline::of(app.session.scenario()?),
    ))
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
