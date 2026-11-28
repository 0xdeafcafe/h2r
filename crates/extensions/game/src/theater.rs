//! Film lifetime: observer-only checkpoints while recording, then seal and
//! optionally upload at the first slice after game over. A film plays from
//! its game start ([`film_play`]).
mod actions;
pub mod autorecord;
mod camera;
mod capture;
mod film_play;
pub mod extension;
mod game_start;
pub mod machine;
mod media;
pub use capture::{ClipCapture, ClipSink, ClipSpan, Picture};
pub use media::Media;
mod compatibility;
pub mod forge_film;
#[cfg(feature = "gpu")]
mod host_view;
#[cfg(feature = "gpu")]
pub use host_view::HostView;
pub use camera::{FilmCamera, RenderHooks};
mod rig;
mod ring;
mod controls;
mod metadata;
mod measurements;
mod unified;
mod objectives;
mod record;
pub mod start_point;
pub mod start_play;
pub mod start_point_agent;
pub mod start_point_state;
mod neutral_pads;
mod transport;
mod pacing;
mod watch;
pub use watch::Planned;
pub use film_play::configure_replay_child;
pub use transport::Launch;
pub(crate) use transport::SeekTaps;
mod video_menu;
mod verify;
pub use verify::VerificationMode;
use anyhow::{Context, Result, bail, ensure};
use rig::ReplayCamera;
pub use agent_proto::theater::CompatibilityState;
pub use compatibility::{Installation, NO_GAME_START, hook_digest};
pub use controls::{library_methods, playback_images};
use film::{FilmMeta, Header, Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::File,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Replay {
    pub verification: Option<VerificationMode>,
    pub path: PathBuf,
    pub render_every: u32,
    pub speed: f64,
    /// The tick the film opens at; zero for its game's first update.
    pub seek_tick: u64,
    pub paused_at_target: bool,
    pub presentation: Presentation,
    pub camera: FilmCamera,
    pub window_closed: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// The view and timeline state a film opens with (`Launch`).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Presentation {
    camera: ReplayCamera,
    hud: bool,
    bar: bool,
    kills: BTreeSet<u64>,
    pub clip_marks: [Option<u64>; 2],
}

impl Default for Presentation {
    fn default() -> Self {
        Self {
            camera: ReplayCamera::default(),
            hud: true,
            bar: true,
            kills: BTreeSet::new(),
            clip_marks: [None; 2],
        }
    }
}

impl Presentation {
    pub fn start_view(&mut self, view: agent_proto::theater::InitialView) {
        if let Some(mode) = view.camera {
            self.camera.mode = mode;
        }
        if view.seat.is_some() {
            self.camera.seat = view.seat;
        }
        // An explicit view replaces the default camera choice for the film.
        self.camera.initialized |= view.camera.is_some() || view.seat.is_some();
    }
}

struct Recording {
    /// Where the films go.
    dest: Dest,
    /// The game being recorded: its film file and stats container, open
    /// from the game's activation to its end.
    game: Option<record::GameFiles>,
    match_over: bool,
    meta: metadata::Capture,
    frame: record::Capture,
    unified: unified::Capture,
    upload: Option<UploadTarget>,
    /// What the last header appended to the stats container said of the
    /// game, so a join or a leave appends another.
    described: Option<metadata::Described>,
    /// The game in play's start, from activation to the film file
    /// ([`game_start`]).
    starts: game_start::Starts,
}

/// Where a recording's films go (docs/game/film-format.md, "Recording
/// every game").
enum Dest {
    /// A default recording: each game is a film of its own in this library
    /// directory, opened at the game's activation with tick 1 its first
    /// update.
    Library(PathBuf),
    /// A host's or an explicit (`--film`, `H2_FILM_OUT`) recording: one
    /// film, open from boot at the path its game's files hold
    /// ([`record::GameFiles::path`]); the recording completes at its game's
    /// end.
    Named,
}

impl Recording {
    fn library(&self) -> Option<&std::path::Path> {
        match &self.dest {
            Dest::Library(dir) => Some(dir),
            Dest::Named => None,
        }
    }
}

struct UploadTarget {
    url: String,
    token: String,
    game: String,
}

impl UploadTarget {
    fn from_env(game_id: Option<&str>, env: impl Fn(&str) -> Option<String>) -> Result<Option<Self>> {
        let (Some(url), Some(token)) = (env("H2_HOST_LIVE_URL"), env("H2_HOST_TOKEN")) else {
            return Ok(None);
        };
        let game = game_id.context("no lease game ID for film")?.to_owned();
        Ok(Some(Self { url, token, game }))
    }
}

enum Mode {
    Record(Recording),
    Replay(Replay),
}
/// The jump the player is serving, reported in `theater.state`. It stays
/// after the target is reached so the route remains visible until another
/// jump.
struct SeekProgress {
    target: u64,
    from_tick: u64,
    source: agent_proto::theater::SeekSource,
    /// The route the caller asked for (`theater.seek`'s `source`), which
    /// `theater.state` reports beside the route taken.
    requested_source: Option<agent_proto::theater::SeekRoute>,
}
struct State {
    verify: Option<verify::Verification>,
    mode: Mode,
    /// The film being played, from its game start ([`film_play`]). `None`
    /// while recording.
    play: Option<film_play::Play>,
    header: Header,
    tick: u64,
    paused: bool,
    speed: f64,
    rate: transport::PlaybackRate,
    step_until: Option<u64>,
    /// Present when the guest's `GAME_UPDATE_TICKS` is answered by the
    /// player (`theater/pacing.rs`): always, while a film plays.
    pacing: Option<transport::FrameClock>,
    seeking: Option<u64>,
    seek: Option<SeekProgress>,
    measurements: Option<measurements::Run>,
    presentation: Presentation,
    /// A start-route jump's target, until the film's game is queued again
    /// ([`Session::requeue`]).
    restart: Option<u64>,
    failure: Option<String>,
    complete: bool,
    /// What a default recording kept and why it kept nothing.
    outcome: Outcome,
    /// Present while a film plays in the viewer's own session
    /// ([`watch`]); the recorder is parked meanwhile.
    watch: Option<watch::WatchInfo>,
    /// The session ring of the film being played ([`ring`]); `None` while
    /// recording and in a verification.
    ring: Option<ring::Ring>,
    /// The machine's outside inputs since boot, for a client's film
    /// ([`machine`]); `None` when the session keeps no machine tape.
    machine: Option<machine::Machine>,
}

/// A default recording's account of itself: the films it sealed, the last
/// reason a game went without one, and the outside changes it carried on
/// through (`theater.status`, and the line printed at the session's end).
#[derive(Default)]
struct Outcome {
    films: Vec<PathBuf>,
    discard: Option<String>,
    notes: Vec<String>,
    /// The session-end line has been printed.
    reported: bool,
}

impl Outcome {
    /// The one line a default recording ends with: the films written, or
    /// why there are none.
    fn line(&self, dir: &std::path::Path, failure: Option<&str>) -> String {
        match self.films.len() {
            0 => {
                let why = self
                    .discard
                    .as_deref()
                    .or(failure)
                    .unwrap_or("no game was played this session");
                format!("theater: no film: {why}")
            }
            1 => format!("theater: 1 film saved to {}", dir.display()),
            n => format!("theater: {n} films saved to {}", dir.display()),
        }
    }
}
pub struct Facts {
    pub stamp: String,
    pub title_id: u32,
    pub epoch: u64,
    pub light: bool,
    pub bots: bool,
    pub address: String,
    pub live_service: bool,
    pub telemetry_hooks: live_proto::film::TelemetryHooks,
    pub render_hooks: film::RenderHooks,
    /// A player session's default recording keeps its player features on
    /// ([`autorecord`]). Host and simulation recordings never do.
    pub player_features: bool,
}
pub struct Session {
    version: u32,
    replaying: bool,
    state: Mutex<State>,
    video_menu: std::sync::OnceLock<Arc<settings::Store>>,
    capture: std::sync::OnceLock<Arc<ClipCapture>>,
    media: std::sync::OnceLock<(Media, std::sync::Weak<runtime::Runtime>)>,
    scaled_screenshot: std::sync::OnceLock<media::ScaledScreenshot>,
    #[cfg(feature = "gpu")]
    view: std::sync::OnceLock<Mutex<host_view::MountedView>>,
    upload: Mutex<Option<std::thread::JoinHandle<Result<()>>>>,
    require_upload: bool,
    /// Watching a film in the running game ([`watch`]).
    watch: watch::Hub,
}

/// What the boot loop does before its next slice ([`Session::before_slice`]).
pub enum SliceAction {
    Run,
    Stop,
}

/// Seals a default recording's game in play when it drops
/// ([`Session::seal_on_drop`]). It only locks the session.
pub struct Seal {
    session: Arc<Session>,
}

impl Drop for Seal {
    fn drop(&mut self) {
        if let Err(error) = self.session.finish(true) {
            eprintln!("theater: sealing the recording on exit: {error:#}");
        }
    }
}
impl Session {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keep this recording's machine tape: a client's game seals with it
    /// ([`machine`], docs/game/film-format.md "The machine tape"). Boot
    /// attaches it before the guest runs, so the tape starts at boot.
    pub fn attach_machine(&self, machine: machine::Machine) {
        let mut s = self.lock();
        if matches!(s.mode, Mode::Record(_)) {
            s.machine = Some(machine);
        }
    }
    pub fn open(
        record: Option<PathBuf>,
        replay: Option<Replay>,
        facts: Facts,
        xbe: &[u8],
        opts: &crate::RunOpts,
    ) -> Result<Option<Arc<Self>>> {
        let Facts {
            stamp,
            title_id,
            epoch,
            light: host_light,
            bots,
            address,
            live_service, telemetry_hooks, render_hooks,
            player_features,
        } = facts;
        let switches = hooks::Switches::from_env().map_err(anyhow::Error::msg)?;
        let admission = crate::replication_admission::FEATURE;
        let (mode, header, speed, seeking, paused, play) = if let Some(r) = replay {
            ensure!(record.is_none(), "cannot record and replay the same boot");
            ensure!((0.25..=4.).contains(&r.speed), "speed must be 0.25 to 4");
            // A film plays from its game start; a film without one is
            // refused, never played some other way.
            let play = film_play::Play::open(&r.path)?.context(NO_GAME_START)?;
            let length = play.length();
            let mut h = film::file::header_of(&r.path)?;
            let recorded = hooks::Switches::parse(&h.features).map_err(anyhow::Error::msg)?;
            let switches = if recorded.feature(admission, false) {
                switches.with(admission, true)
            } else {
                switches
            };
            // Nothing of the machine is replayed: the clock's epoch and the
            // scheduler's transfers are not the film's to equal.
            play.check_stamp(&film::xbe_digest(xbe))?;
            ensure!(
                h.xbe_sha256 == film::xbe_digest(xbe)
                    && h.title_id == title_id
                    && h.features == switches.spec(),
                "film boot facts differ"
            );
            if let Some(required) = h.render_hooks {
                ensure!(
                    render_hooks.supports(required),
                    "playback executable lacks the film's {required:?} render hooks"
                );
            }
            if let Some(plan) = h.telemetry_hooks {
                ensure!(
                    (!plan.weapon_fire || telemetry_hooks.weapon_fire)
                        && (!plan.damage_report || telemetry_hooks.damage_report)
                        && (!plan.object_delete || telemetry_hooks.object_delete),
                    "image lacks an observer required by the film telemetry plan"
                );
            }
            ensure!(r.seek_tick <= length, "seek beyond film");
            h.meta.length_ticks = length;
            // The film has no tick before its game's first update to stop at.
            let target = (r.seek_tick > 0).then(|| r.seek_tick.max(play.start().game.first_update));
            let (speed, paused) = (r.speed, r.paused_at_target);
            (Mode::Replay(r), h, speed, target, paused, Some(play))
        } else if let Some(path) = record {
            let started_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
            let lease_id = std::env::var("H2_LEASE").ok().filter(|id| !id.is_empty());
            let host_recording = host_light || lease_id.is_some();
            let game_id = host_recording.then(|| {
                std::env::var("H2_FILM_GAME_ID").ok().filter(|id| !id.is_empty())
                    .or_else(|| lease_id.clone())
            }).flatten();
            let xbe_sha256 = film::xbe_digest(xbe);
            let guest_image = film::image::GuestImage::capture(
                &stamp, recomp::host_triple(), &xbe_sha256,
            );
            // Every recording keeps its actions in the film file and its
            // observations in the stats container beside it.
            let header = Header {
                version: 3,
                title_id,
                xbe_sha256,
                guest_image: Some(guest_image),
                runtime_stamp: compatibility::SIMULATION_ABI.into(),
                simulation_hooks: None,
                epoch_100ns: epoch,
                lease_id,
                game_id,
                xnet_addr: address,
                lobby: std::env::var("H2_HOST_LOBBY").as_deref() == Ok("1"),
                lobby_machine_start: std::env::var("H2_HOST_LOBBY_MACHINE_START").as_deref()
                    != Ok("0"),
                host_bots: bots,
                telemetry: true,
                telemetry_hooks: Some(telemetry_hooks),
                render_hooks: Some(render_hooks),
                live_service,
                features: switches.with(admission, true).spec(),
                player_features: player_features && !host_recording,
                inputs_in_film: true,
                role: film::tape::Role::Local,
                transfers_per_slice: opts.transfers_per_slice,
                meta: FilmMeta {
                    id: live_proto::sign::random_id(),
                    map: String::new(),
                    variant: String::new(),
                    started_ms,
                    length_ticks: 0,
                    players: vec![],
                    image_stamp: stamp,
                    source: if host_recording {
                        Source::Host
                    } else {
                        Source::Local
                    },
                },
            };
            let upload = if host_recording {
                UploadTarget::from_env(header.game_id.as_deref(), |key| std::env::var(key).ok())?
            } else {
                None
            };
            // A default recording opens each game's film at the game's
            // activation; a named one opens its film now.
            let (dest, game) = if header.player_features {
                (Dest::Library(path), None)
            } else {
                let files = record::GameFiles::create(path, header.clone(), Some(0))?;
                (Dest::Named, Some(files))
            };
            (
                Mode::Record(Recording {
                    dest,
                    game,
                    match_over: false,
                    meta: metadata::Capture::new(&header),
                    frame: Default::default(),
                    unified: Default::default(),
                    upload,
                    described: None,
                    starts: Default::default(),
                }),
                header,
                1.,
                None,
                false,
                None,
            )
        } else {
            return Ok(None);
        };
        let seek = match &mode {
            Mode::Replay(r) if r.seek_tick > 0 => Some(SeekProgress {
                target: r.seek_tick,
                from_tick: 0,
                source: agent_proto::theater::SeekSource::Boot,
                requested_source: None,
            }),
            _ => None,
        };
        let mut presentation = match &mode {
            Mode::Replay(opts) => opts.presentation.clone(),
            Mode::Record(_) => Presentation::default(),
        };
        if let Mode::Replay(opts) = &mode {
            if !presentation.camera.initialized {
                if opts.render_every != 0 && header.meta.source == Source::Host {
                    presentation.camera.mode = agent_proto::theater::CameraMode::Free;
                }
                presentation.camera.initialized = true;
            }
            opts.camera.restore(&presentation);
        }
        let from_start = play.is_some();
        let own = watch::Own {
            xbe_sha256: film::xbe_digest(xbe),
            title_id,
            features: header.features.clone(),
        };
        let session = Arc::new(Self {
            version: header.version,
            video_menu: Default::default(),
            capture: Default::default(),
            media: Default::default(),
            scaled_screenshot: Default::default(),
            replaying: matches!(&mode, Mode::Replay(_)),
            require_upload: matches!(&mode, Mode::Record(rec) if rec.upload.is_some())
                && std::env::var("H2_HOST_ARCHIVE").as_deref() == Ok("film"),
            #[cfg(feature = "gpu")]
            view: Default::default(),
            state: Mutex::new(State {
                verify: verify::Verification::open(&mode, header.meta.length_ticks)?,
                mode,
                play,
                header,
                speed,
                rate: Default::default(),
                step_until: None,
                pacing: None,
                seeking,
                seek,
                measurements: None,
                paused,
                presentation,
                tick: 0,
                restart: None,
                failure: None,
                complete: false,
                outcome: Outcome::default(),
                watch: None,
                ring: None,
                machine: None,
            }),
            upload: Mutex::default(),
            watch: watch::Hub::new(own),
        });
        if from_start {
            // The player answers the guest's tick count itself: no wall
            // clock paces a film that starts from its game start.
            session.pace_in_guest();
            session.open_ring();
        }
        Ok(Some(session))
    }

    pub fn attach_capture(&self, capture: Arc<ClipCapture>) -> Result<()> {
        self.capture.set(capture).map_err(|_| anyhow::anyhow!("clip sink already attached"))
    }

    /// Begin sampling the new tick before its first present.
    fn advance_capture(&self) -> Result<bool> {
        let Some(capture) = self.capture.get() else { return Ok(false) };
        let tick = {
            let state = self.lock();
            if let Some(error) = &state.failure {
                bail!("{error}");
            }
            state.tick
        };
        capture.advance(tick)
    }

    pub fn has_engine_track(&self) -> bool {
        self.version >= 2
    }

    /// A player session's default recording, or the replay of one: the
    /// player features run as they did in the recorded session
    /// ([`autorecord`]). Otherwise a film turns them off on both sides.
    pub fn player_features(&self) -> bool {
        self.lock().header.player_features
    }

    /// This boot plays a film back rather than recording one.
    pub fn plays_back(&self) -> bool {
        self.replaying
    }

    /// Something outside the film (a session link joined, an editor write)
    /// is about to change the guest. A default recording carries on: the
    /// session's other games still become films, each played from its own
    /// game start. `ends_game` is a change with no record, which the game
    /// in play's film cannot replay: the film ends at the tick before it
    /// and is kept to there, and the rest of the game is not recorded. A
    /// change a feature records as an extension record
    /// ([`extension`]) is not this. A sealed film, an explicit recording
    /// and a replay are left alone.
    pub fn outside_change(&self, why: &str, ends_game: bool) {
        let mut s = self.lock();
        if !s.header.player_features
            || s.complete
            || s.failure.is_some()
            || !matches!(s.mode, Mode::Record(_))
        {
            return;
        }
        let ended = if ends_game { s.end_game() } else { None };
        let said = match ended {
            Some(ticks) if ticks > 0 => format!("{why}; the game in play's film ends at tick {ticks}"),
            Some(_) => format!("{why}; the game in play had no tick to keep"),
            None => why.to_owned(),
        };
        s.outcome.notes.push(said.clone());
        eprintln!(
            "theater: {said}; recording carries on{}",
            if ended.is_some() { ", without the rest of the game in play" } else { "" }
        );
    }

    /// What this session's recording has kept and why it kept nothing
    /// (`theater.status`).
    pub fn status(&self) -> agent_proto::theater::Status {
        let s = self.lock();
        let (recording, in_game) = match &s.mode {
            Mode::Record(rec) => (
                rec.game.as_ref().map(|game| game.path.display().to_string()),
                rec.in_game(),
            ),
            Mode::Replay(..) => (None, false),
        };
        agent_proto::theater::Status {
            default_recording: s.header.player_features && !self.replaying && s.watch.is_none(),
            recording,
            in_game,
            films: s.outcome.films.iter().map(|p| p.display().to_string()).collect(),
            last_discard: s.outcome.discard.clone(),
            notes: s.outcome.notes.clone(),
            failure: s.failure.clone(),
            watch_failure: self.watch_failure(),
        }
    }

    /// Seal a default recording's game in play when its owner goes away
    /// without a teardown (docs/game/film-format.md, "Recording every
    /// game"). The boot loop holds one across its whole slice loop: an
    /// error exit or a panic that skips the final [`Session::finish`] still
    /// seals the game in play, where a hard kill leaves it unsealed. The
    /// seal does nothing once the recording completed or failed on its own.
    /// `None` for anything but a default recording.
    pub fn seal_on_drop(self: &Arc<Self>) -> Option<Seal> {
        let s = self.lock();
        (!self.replaying && s.header.player_features && !s.complete).then(|| Seal {
            session: Arc::clone(self),
        })
    }

    pub fn render_hooks(&self) -> film::RenderHooks {
        self.lock().header.render_hooks
            .unwrap_or_else(crate::run_plan::compiled_render_hooks)
    }

    pub fn telemetry_plan(&self) -> live_proto::film::TelemetryHooks {
        self.lock().header.telemetry_plan()
    }

    /// The original wire admission plan for this isolated recording or replay.
    pub fn uses_replication_admission(&self) -> bool {
        hooks::Switches::parse(&self.lock().header.features)
            .expect("validated film feature switches")
            .feature(crate::replication_admission::FEATURE, false)
    }

    fn checkpoint(&self, guest: &runtime::Guest) {
        let mut s = self.lock();
        if s.complete || s.failure.is_some() {
            return;
        }
        // The ticks before the game's first update are the game's own
        // (menus, loading), not the film's.
        if s.play.as_ref().is_some_and(|play| !play.begun()) {
            return;
        }
        s.tick += 1;
        let tick = s.tick;
        let reached_target = s.seeking == Some(tick);
        if s.header.version >= 2 {
            let hash = crate::sim_glue::film_gameplay_hash(guest);
            if let Mode::Record(rec) = &mut s.mode {
                if let Err(e) = rec.post_tick(guest, tick, hash) {
                    s.fail(format!("writing engine tick: {e:#}"));
                    return;
                }
            } else {
                film_play::tick_done(&mut s, hash);
                // A divergence just after a ring load is the ring's: the
                // jump goes by the start route instead.
                if let Some(target) = s.ring_diverged() {
                    drop(s);
                    self.start_over(target);
                    return;
                }
                // The film tick the game reached, which a tick that applied
                // its update twice moves on by two.
                let at = s.tick;
                if let Some(v) = &mut s.verify {
                    v.tick(at);
                }
            }
            if s.failure.is_none() {
                Session::describe_game(&mut s);
            }
            if s.failure.is_none() {
                s.measure_match_start(tick);
            }
        }
        // A film's digests are in its ticks.
        if tick.is_multiple_of(30) && matches!(s.mode, Mode::Record(_)) {
            Self::checkpoint_interval(&mut s, guest);
        }
        if s.failure.is_some() {
            return;
        }
        s.ring_passed(guest);
        if reached_target {
            s.seeking = None;
            if let Some(run) = &mut s.measurements {
                run.reached_target(tick);
            }
        }
        s.completed_transport_tick(reached_target);
    }

    /// Append a header naming the game being recorded (its map, variant
    /// and players) to its stats container when that changes, so a stats
    /// container a hard kill left unsealed still names its game.
    fn describe_game(s: &mut State) {
        if s.header.version < 3 {
            return;
        }
        let Mode::Record(rec) = &mut s.mode else { return };
        let now = rec.meta.description();
        let Some(game) = now.clone() else { return };
        if now == rec.described {
            return;
        }
        if rec.game.is_none() {
            return;
        }
        rec.described = now;
        // The game's front is written as it is described, so a recording
        // that stops after this has a front naming it.
        let game_tick = s.tick;
        if let Err(e) = rec.refresh_front(game_tick) {
            eprintln!("theater: could not write this game's front: {e:#}");
        }
        let Some(files) = rec.game.as_mut() else { return };
        let mut header = files.header.clone();
        header.meta.map = game.map;
        header.meta.variant = game.variant;
        header.meta.players = game.players;
        if let Err(e) = files.stats.chunk(film::HEAD, &header) {
            s.fail(format!("writing the game's header: {e:#}"));
        }
    }

    /// A recording reads the game's roster every 30 ticks.
    fn checkpoint_interval(s: &mut State, guest: &runtime::Guest) {
        s.roster(guest);
        // And its front, with the scores so far, so a game that never
        // reaches its seal still lists with them.
        let tick = s.tick;
        if let Mode::Record(rec) = &mut s.mode
            && let Err(e) = rec.refresh_front(tick)
        {
            eprintln!("theater: could not refresh this game's front: {e:#}");
        }
    }

    /// Poll before executing another slice. The logical clock remains still
    /// during transport pauses. Seeking just removes the host wall-time wait.
    pub fn before_slice(
        &self,
        m: &mut kernel::Machine,
        agent: Option<&crate::agent_glue::AgentGlue>,
        rt: &runtime::Runtime,
    ) -> Result<SliceAction> {
        // A film watched in the running game is paced by the guest's own
        // tick count (`pacing.rs`); the scheduler has nothing to hold or
        // stop.
        if self.lock().watch.is_some() {
            return Ok(SliceAction::Run);
        }
        loop {
            let clip_done = self.advance_capture()?;
            #[cfg(feature = "gpu")]
            if !clip_done {
                self.draw_host_view(rt);
            }
            {
                let mut s = self.lock();
                if s.verify_range_ended() { s.complete = true; }
                let Mode::Replay(r) = &s.mode else {
                    return Ok(SliceAction::Run);
                };
                if let Some(e) = &s.failure {
                    s.save_verify_result()?;
                    bail!("{e}")
                }
                if r.window_closed.as_ref().is_some_and(|closed| closed()) {
                    return Ok(SliceAction::Stop);
                }
                if clip_done {
                    return Ok(SliceAction::Stop);
                }
                if s.complete {
                    // Keep the viewer's socket and transport alive at natural
                    // EOF until an outstanding verification reply is queued.
                    if agent.is_none_or(|a| !a.agent.connection_read_pending()) {
                        return Ok(SliceAction::Stop);
                    }
                } else if !s.holding() || s.pacing.is_some() {
                    // A film that the player paces in the guest takes no
                    // wall-clock pacer (`theater/pacing.rs`).
                    let pace = (s.seeking.is_none() && s.verify.is_none() && self.capture.get().is_none()
                        && s.pacing.is_none())
                        .then(|| (kernel::clock::Clock::FREQUENCY as f64 * s.speed) as u64);
                    if m.pacer.as_ref().map(|p| p.ticks_per_second) != pace {
                        m.pacer = pace.map(|rate| {
                            let mut p = kernel::pacer::Pacer::real_time();
                            p.ticks_per_second = rate;
                            p
                        });
                    }
                    return Ok(SliceAction::Run);
                }
            }
            let a = agent.context("paused film needs the agent API")?;
            a.agent
                .poll_theater(&mut agent::GuestReader::new(|| rt.memory()));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The sim's tick, wall or stop limit may end a match without GAME_END.
    /// A successful report means Live has settled its canonical game row.
    pub fn finish_uploaded_sim(&self, published: bool) -> Result<()> {
        if let Mode::Record(r) = &mut self.lock().mode {
            r.match_over = published;
        }
        self.finish(true)
    }

    pub fn finish(&self, teardown: bool) -> Result<()> {
        // A film that diverged while watched ends like End Film; the game
        // goes on (`watch.rs`).
        if !teardown && self.watch_diverged() {
            return Ok(());
        }
        if teardown {
            // A film still playing is dropped: the recorder seals its own.
            self.drop_watch();
        }
        self.finish_output(teardown)?;
        if teardown {
            if let Some(job) = self.upload.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let result = job.join().unwrap_or_else(|_| Err(anyhow::anyhow!("film upload worker stopped")));
                if self.require_upload {
                    result?;
                } else if let Err(error) = result {
                    eprintln!("theater: {error:#}");
                }
            }
            if self.require_upload && let Some(error) = self.lock().failure.as_ref() {
                bail!("host film: {error}");
            }
        }
        Ok(())
    }

    fn finish_output(&self, teardown: bool) -> Result<()> {
        let mut s = self.lock();
        if let Mode::Replay(replay) = &s.mode {
            let window_closed = replay.window_closed.as_ref().is_some_and(|closed| closed());
            if s.failure.is_some() || (teardown && s.complete) { s.save_verify_result()?; }
            if let Some(e) = &s.failure {
                bail!("{e}")
            }
            if teardown && s.complete
                && let Some(capture) = self.capture.get()
            {
                capture.advance(s.tick.saturating_add(1))?;
            }
            if teardown && !s.complete && !window_closed
                && !self.capture.get().is_some_and(|capture| capture.done())
            {
                bail!(
                    "playback stopped before the end of the film at tick {}",
                    s.tick
                )
            }
            if teardown && s.complete {
                eprintln!("theater: hashes matched through tick {}", s.tick)
            }
            return Ok(());
        }
        if s.complete || s.failure.is_some() {
            if teardown && s.failure.is_some() && s.header.player_features {
                // Keep what was recorded: the game in play is sealed up to
                // the tick the failure stopped the recording at.
                s.seal_game();
                s.report_session_end();
            }
            return Ok(());
        }
        let Mode::Record(rec) = &s.mode else {
            unreachable!()
        };
        if !teardown && (!rec.match_over || !rec.frame.updates.is_empty()) {
            return Ok(());
        }
        // A default recording seals each game's film at the game's end and
        // records the next game into a film of its own. At the session's
        // end it seals the game in play, if any (docs/game/film-format.md,
        // "Recording every game").
        if s.header.player_features {
            s.seal_game();
            if teardown {
                s.complete = true;
                s.report_session_end();
            }
            return Ok(());
        }
        let matched = rec.match_over;
        let tick = s.tick;
        s.header.meta.length_ticks = tick;
        let header = s.header.clone();
        let Mode::Record(rec) = &mut s.mode else {
            unreachable!()
        };
        let upload = rec.upload.take();
        let files = rec.game.take().context("film already closed")?;
        let path = match rec.seal(files, Some(header), None, tick) {
            Ok((path, header)) => {
                s.header = header;
                path
            }
            Err(e) => {
                s.fail(format!("closing film: {e}"));
                return Ok(());
            }
        };
        let Mode::Record(rec) = &mut s.mode else {
            unreachable!()
        };
        rec.frame = Default::default();
        s.complete = true;
        drop(s);
        if matched && let Some(target) = upload {
            // Create before spawning so a stopped uploader leaves its sealed
            // output recoverable when the supervisor cleans the lease.
            let receipt = std::env::var_os("H2_FILM_PENDING_RECEIPT").map(PathBuf::from)
                .and_then(|p| {
                    match File::create_new(&p) {
                        Ok(_) => Some(p),
                        Err(e) => {
                            eprintln!("theater: could not mark pending upload: {e}");
                            None
                        }
                    }
                });
            let worker = background::spawn("film-upload", move || {
                film::upload(&target.url, &target.token, &target.game, &path)
                    .with_context(|| format!("upload failed for {}", path.display()))?;
                if let Some(p) = receipt {
                    let _ = std::fs::remove_file(p);
                }
                Ok(())
            });
            match worker {
                Ok(job) => *self.upload.lock().unwrap_or_else(|e| e.into_inner()) = Some(job),
                Err(e) if self.require_upload => return Err(e).context("starting host film upload"),
                Err(e) => eprintln!("theater: could not start film upload: {e}"),
            }
        }
        Ok(())
    }
}
/// Seal a client's game with its machine tape, and say where it went.
fn keep(
    rec: &mut Recording,
    outcome: &mut Outcome,
    files: record::GameFiles,
    tape: Option<film::tape::Tape>,
    ticks: u64,
) {
    let role = files.header.role;
    match rec.seal(files, None, tape, ticks) {
        Ok((path, _)) => {
            eprintln!("theater: this {} game's film is {}", role.label(), path.display());
            outcome.films.push(path);
        }
        Err(error) => {
            eprintln!("theater: could not keep this game's film: {error:#}");
            outcome.discard = Some(format!("could not keep this game's film: {error:#}"));
        }
    }
}

impl State {
    fn fail(&mut self, why: String) {
        self.fail_at(self.tick, why)
    }

    /// Seal the film of the game a default recording just played, at the
    /// tick it reached, and make ready for the next game, whose film opens
    /// at its activation (docs/game/film-format.md, "Recording every
    /// game"). A client's game keeps its film with the machine tape, with
    /// or without updates of its own ([`machine`]). Any other game that
    /// never completed its first update (one gone before it, or a guest
    /// stopped inside it) has no film: its files are deleted. A film that cannot be sealed is logged and kept as the
    /// last discard reason.
    fn seal_game(&mut self) {
        let tick = self.tick;
        // A client's game is its machine: its film takes the tape from boot,
        // with or without simulation updates of its own.
        let client = self.mode_game_role().is_client();
        let tape = match (&self.machine, client) {
            (Some(machine), true) => match machine.tape() {
                Ok(tape) => Some(tape),
                Err(why) => {
                    self.outcome.discard = Some(format!("this client game has no film: {why}"));
                    None
                }
            },
            _ => None,
        };
        let Mode::Record(rec) = &mut self.mode else { return };
        rec.match_over = false;
        if let Some(files) = rec.game.take() {
            match files.base {
                Some(base) if tape.is_some() => {
                    keep(rec, &mut self.outcome, files, tape, tick.saturating_sub(base))
                }
                None if tape.is_some() => keep(rec, &mut self.outcome, files, tape, 0),
                // Base is set as the first update enters; its film tick is
                // pushed as it ends. The track learns the film's version
                // from that tick, so a seal before it has nothing to keep.
                _ if files.track.ticks() == 0 => files.discard(),
                None => files.discard(),
                Some(base) => match rec.seal(files, None, None, tick.saturating_sub(base)) {
                    Ok((path, _)) => {
                        eprintln!("theater: this game's film is {}", path.display());
                        self.outcome.films.push(path);
                    }
                    Err(error) => {
                        eprintln!("theater: could not keep this game's film: {error:#}");
                        self.outcome.discard =
                            Some(format!("could not keep this game's film: {error:#}"));
                    }
                },
            }
        }
        let Mode::Record(rec) = &mut self.mode else { return };
        rec.next_game();
    }

    /// The role of the game in play's film (`film::Header::role`), local
    /// when no game is in play.
    fn mode_game_role(&self) -> film::tape::Role {
        match &self.mode {
            Mode::Record(rec) => rec.game.as_ref().map_or(Default::default(), |g| g.header.role),
            _ => Default::default(),
        }
    }

    /// A new game activated in a default recording: the game before it
    /// ends here, and the new game's film opens, `<id>.h2film` with its
    /// stats container beside it, in the library. The library keeps
    /// [`autorecord::KEEP`] default films with this one.
    fn open_game(&mut self) {
        if !matches!(&self.mode, Mode::Record(rec) if rec.library().is_some()) {
            return;
        }
        self.seal_game();
        // Records a feature left outside any game are not this game's.
        drop(extension::outside());
        let header = self.header.clone();
        let Mode::Record(rec) = &mut self.mode else { return };
        let Some(dir) = rec.library().map(std::path::Path::to_owned) else { return };
        let now = SystemTime::now();
        if let Err(error) = autorecord::prune(&dir, autorecord::KEEP, now) {
            eprintln!("theater: could not make room in the film library: {error:#}");
        }
        let mut header = autorecord::game_header(&header);
        // Whose game it is, as the machine sees it at the game's activation.
        if let Some(machine) = &self.machine {
            header.role = machine.role();
        }
        let Mode::Record(rec) = &mut self.mode else { return };
        match record::GameFiles::create(autorecord::destination(&dir, now), header, None) {
            Ok(files) => rec.game = Some(files),
            Err(error) => {
                eprintln!("theater: this game has no film: {error:#}");
                self.outcome.discard = Some(format!("this game has no film: {error:#}"));
            }
        }
    }

    /// End the game in play's film at the tick before now and keep it: the
    /// ticks it holds are what was played, and the rest of the game is not
    /// recorded. Returns the film ticks it kept (none when the game had no
    /// first update yet), or `None` when no game was in play.
    fn end_game(&mut self) -> Option<u64> {
        let Mode::Record(rec) = &self.mode else { return None };
        if !rec.in_game() {
            return None;
        }
        let kept = rec
            .game
            .as_ref()
            .filter(|files| files.base.is_some())
            .map_or(0, |files| files.film.ticks());
        self.seal_game();
        Some(kept)
    }

    /// Forget the game in play and delete its film. Returns whether a game
    /// was in play.
    fn drop_game(&mut self, why: String) -> bool {
        let Mode::Record(rec) = &mut self.mode else { return false };
        if !rec.in_game() {
            return false;
        }
        rec.match_over = false;
        if let Some(files) = rec.game.take() {
            files.discard();
        }
        rec.next_game();
        self.outcome.discard = Some(why);
        true
    }

    /// Print the session's one closing line, once: the films written or why
    /// there are none.
    fn report_session_end(&mut self) {
        if self.outcome.reported {
            return;
        }
        self.outcome.reported = true;
        let Mode::Record(rec) = &self.mode else { return };
        let Some(dir) = rec.library() else { return };
        let line = self.outcome.line(dir, self.failure.as_deref());
        eprintln!("{line}");
    }

    fn fail_at(&mut self, tick: u64, why: String) {
        if self.failure.is_some() {
            return;
        }
        if let Some(v) = &mut self.verify { v.failed(tick, &why); }
        let reason = match &mut self.mode {
            Mode::Record(rec) => {
                rec.frame = Default::default();
                format!("recording stopped at tick {tick}: {why}")
            }
            Mode::Replay(_) => format!("first divergence at tick {tick}: {why}"),
        };
        eprintln!("theater: {reason}");
        self.failure = Some(reason);
    }
    fn roster(&mut self, g: &runtime::Guest) {
        self.header.meta.length_ticks = self.tick;
        if let Ok(info) = crate::telemetry::game_info(g)
            && info.is_match()
        {
            self.header.meta.map = info.map;
            self.header.meta.variant = info.variant.unwrap_or(info.mode);
            if let Ok(players) = crate::telemetry::players(g)
                && !players.is_empty()
            {
                self.header.meta.players = players
                    .into_iter()
                    .map(|p| film::Player {
                        name: p.player.name,
                        team: p.player.team,
                        score: p.stats.score,
                    })
                    .collect();
            }
        }
    }
}
/// The [`Observer`]'s feature id. No other feature may take it: the
/// in-game Theater's screens are `theater-films` and `theater-film-detail`.
pub const OBSERVER_FEATURE: &str = "theater-film";

pub struct Observer(pub Arc<Session>, pub Option<agent::Agent>);
impl hooks::Feature for Observer {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(OBSERVER_FEATURE, "Exact host film").doc("docs/game/theater.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // Hook installation changes guest dispatch boundaries. A v1 tape
        // must use the observer set that recorded its scheduler/call stream.
        if self.0.lock().header.version >= 2 {
            if self.0.version >= 3 {
                objectives::install(b, self.0.clone());
            }
            metadata::install(&self.0, b);
            camera::install(b, &self.0);
            actions::install(b, self.0.clone());
            pacing::install(b, self.0.clone());
        }
        let session = self.0.clone();
        let live = move || {
            let state = session.lock();
            (!crate::sim_bounces::in_fork() && !state.complete && state.failure.is_none()).then(|| "film cursor is live".into())
        };
        b.snapshot(video_menu::snapshot(self.0.video_menu.get(), live));
        let session = self.0.clone();
        let agent = self.1.clone();
        b.frame().on_tick("film checkpoint", move |o| {
            session.checkpoint(&o.guest());
            match session.advance_capture() {
                Ok(false) => {}
                Ok(true) | Err(_) => {
                    o.runtime.request_preempt(runtime::preempt::REQUEST);
                    return;
                }
            }
            loop {
                #[cfg(feature = "gpu")]
                session.draw_host_view(o.runtime);
                {
                    let s = session.lock();
                    let Mode::Replay(r) = &s.mode else { break };
                    // A film watched in the running game is neither a
                    // process to stop nor a tick to hold: a pause is the
                    // guest's zero-tick frame (`pacing.rs`).
                    if s.watch.is_some() {
                        break;
                    }
                    if s.failure.is_some()
                        || s.complete
                        || r.window_closed.as_ref().is_some_and(|closed| closed())
                    {
                        o.runtime.request_preempt(runtime::preempt::REQUEST);
                        break;
                    }
                    if !s.holding() || s.pacing.is_some() {
                        break;
                    }
                }
                // Hold the target tick in place, without adding a scheduler
                // slice. poll_theater does not advance any film/agent clocks.
                let Some(a) = &agent else { break };
                a.poll_theater(&mut agent::GuestReader::new(|| o.runtime.memory()));
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }).after_feature("live-telemetry");
        // V3 already dispatches PLAYER_KILLED when its recorded telemetry
        // profile enables events. An additional observer shares that dispatch.
        if self.0.has_unified_track() && self.0.telemetry_plan().enabled() {
            let session = self.0.clone();
            b.raw().observe_before(
                &guest_layout::points::PLAYER_KILLED,
                "film kill marker",
                move |_| {
                    let mut s = session.lock();
                    let tick = s.tick;
                    s.presentation.kills.insert(tick);
                },
            );
        }
        let session = self.0.clone();
        b.game().on_start("film game start", move |o| session.game_activated(&o.guest()));
        let session = self.0.clone();
        b.game().on_end("film scoreboard", move |o| {
            let mut s = session.lock();
            if !matches!(s.mode, Mode::Record(_)) || s.complete || s.failure.is_some() {
                return;
            }
            let g = o.guest();
            use crate::telemetry::layout::{GAME_ENDED, GAME_GLOBALS_PTR};
            if g.u32(GAME_GLOBALS_PTR)
                .ok()
                .filter(|p| *p != 0)
                .and_then(|p| p.checked_add(GAME_ENDED))
                .and_then(|p| g.u8(p).ok())
                .is_some_and(|v| v != 0)
                && crate::telemetry::game_info(&g).is_ok_and(|i| i.is_match())
            {
                s.roster(&g);
                let tick = s.tick;
                if let Mode::Record(r) = &mut s.mode {
                    match r.meta.observe(&g, tick, |source| r.unified.dropped(source)) {
                        Ok(events) => r.frame.events.extend(events),
                        Err(error) => {
                            s.fail(format!("reading required match identity: {error:#}"));
                            return;
                        }
                    }
                    r.match_over = true
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
