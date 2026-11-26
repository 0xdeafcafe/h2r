//! Film lifetime: observer-only checkpoints while recording, then seal and
//! optionally upload at the first slice after game over. A film plays from
//! its game start ([`film_play`]).
mod actions;
pub mod autorecord;
mod camera;
mod capture;
mod film_play;
mod game_start;
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
use film::{FilmMeta, Header, Reader, Source, Writer};
use serde::{Deserialize, Serialize};
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
    out: Option<Writer<File>>,
    file: PathBuf,
    match_over: bool,
    track: film::blam::TrackWriter,
    /// A default recording's film file: its actions and digests, written
    /// as the game is played (docs/game/input-film.md).
    film: Option<Arc<Mutex<film::file::FilmWriter<File>>>>,
    meta: metadata::Capture,
    frame: record::Capture,
    unified: unified::Capture,
    upload: Option<UploadTarget>,
    /// Wall time of the current game's first tick: a cut film's start.
    game_started_ms: Option<u64>,
    /// What the last header appended to the tape said of the game, so a
    /// new game, a join or a leave appends another.
    described: Option<metadata::Described>,
    /// The game in play's start, from activation to the film file
    /// ([`game_start`]).
    starts: game_start::Starts,
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
}

/// A default recording's account of itself: the films it cut, the last
/// reason a game or tape went without one, and the outside changes it
/// carried on through (`theater.status`, and the line printed at the
/// session's end).
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

/// Cuts the game in play and discards the tape when it drops
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
            let mut h = Reader::open(&r.path)?.header;
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
            // A default recording and a host's keep their actions in a film
            // file, which a host sends to Live beside its stats stream; an
            // explicit recording is one container.
            let split = player_features || host_recording;
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
                inputs_in_film: split,
                tape_id: None,
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
            let mut file = File::options();
            file.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                file.mode(0o600);
            }
            let mut out = Writer::new(
                file.open(&path)
                    .with_context(|| path.display().to_string())?,
                &header,
            )?;
            let film = if split {
                let mut file = File::options();
                file.read(true).write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    file.mode(0o600);
                }
                let film_path = film::file::beside(&path);
                let seed = film::file::Seed { header: header.clone(), descriptor: Default::default() };
                let writer = film::file::FilmWriter::new(
                    file.open(&film_path).with_context(|| film_path.display().to_string())?,
                    &seed,
                    30,
                )?;
                Some(Arc::new(Mutex::new(writer)))
            } else {
                None
            };
            (
                Mode::Record(Recording {
                    out: Some(out),
                    file: path,
                    match_over: false,
                    track: if split {
                        film::blam::TrackWriter::default().without_inputs()
                    } else {
                        Default::default()
                    },
                    film,
                    meta: metadata::Capture::new(&header),
                    frame: Default::default(),
                    unified: Default::default(),
                    upload,
                    game_started_ms: None,
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
            }),
            upload: Mutex::default(),
            watch: watch::Hub::new(own),
        });
        if from_start {
            // The player answers the guest's tick count itself: no wall
            // clock paces a film that starts from its game start.
            session.pace_in_guest();
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

    /// Something outside the tape (a session link joined, an editor write)
    /// is about to change the guest. A default recording carries on: the
    /// session's other games still become films, each played from its own
    /// game start. `drops_game` also drops the game in play, whose ticks the
    /// tape can no longer replay; its film is never cut. A sealed film, an
    /// explicit recording and a replay are left alone.
    pub fn outside_change(&self, why: &str, drops_game: bool) {
        let mut s = self.lock();
        if !s.header.player_features
            || s.complete
            || s.failure.is_some()
            || !matches!(s.mode, Mode::Record(_))
        {
            return;
        }
        let dropped = drops_game && s.drop_game(format!("{why}; the game in play is not kept"));
        if !dropped {
            s.outcome.notes.push(why.to_owned());
        }
        eprintln!(
            "theater: {why}; recording carries on{}",
            if dropped { ", without the game in play" } else { "" }
        );
    }

    /// What this session's recording has kept and why it kept nothing
    /// (`theater.status`).
    pub fn status(&self) -> agent_proto::theater::Status {
        let s = self.lock();
        let (tape, in_game) = match &s.mode {
            Mode::Record(rec) if s.header.player_features && rec.out.is_some() => {
                (Some(rec.file.display().to_string()), rec.in_game())
            }
            Mode::Record(rec) => (None, rec.in_game()),
            Mode::Replay(..) => (None, false),
        };
        agent_proto::theater::Status {
            default_recording: s.header.player_features && !self.replaying && s.watch.is_none(),
            tape,
            in_game,
            games_cut: s.outcome.films.iter().map(|p| p.display().to_string()).collect(),
            last_discard: s.outcome.discard.clone(),
            notes: s.outcome.notes.clone(),
            failure: s.failure.clone(),
            watch_failure: self.watch_failure(),
        }
    }

    /// Cut and discard a default recording when its owner goes away without
    /// a teardown (docs/game/film-format.md, "One film per game"). The boot
    /// loop holds one across its whole slice loop: an error exit or a panic
    /// that skips the final [`Session::finish`] still cuts the game in play
    /// and deletes the tape, where an abandoned unsealed tape would keep
    /// neither. The seal does nothing once the recording completed or
    /// failed on its own.
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
                if rec.meta.descriptor().match_start_tick == Some(tick) {
                    rec.game_started_ms = Some(autorecord::now_ms());
                }
            } else {
                film_play::tick_done(&mut s, hash);
                // The film tick the game reached, which a tick that applied
                // its update twice moves on by two.
                let at = s.tick;
                if let Some(v) = &mut s.verify {
                    v.tick(at);
                }
            }
            if s.failure.is_none() {
                Session::describe_tape(&mut s);
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
        if reached_target {
            s.seeking = None;
            if let Some(run) = &mut s.measurements {
                run.reached_target(tick);
            }
        }
        s.completed_transport_tick(reached_target);
    }

    /// Append a header naming the game on the tape (its map, variant and
    /// players) when that changes, and an empty one when the game is gone,
    /// so the tape names the game it held up to its last chunk. A session
    /// in menus appends none.
    fn describe_tape(s: &mut State) {
        if s.header.version < 3 {
            return;
        }
        let Mode::Record(rec) = &mut s.mode else { return };
        let now = rec.meta.description();
        if now == rec.described || (now.is_none() && rec.described.is_none()) {
            return;
        }
        rec.described = now.clone();
        let mut header = s.header.clone();
        if let Some(game) = now {
            header.meta.map = game.map;
            header.meta.variant = game.variant;
            header.meta.players = game.players;
        }
        if let Mode::Record(rec) = &mut s.mode
            && let Some(out) = rec.out.as_mut()
            && let Err(e) = out.chunk(film::HEAD, &header)
        {
            s.fail(format!("writing the tape's game header: {e:#}"));
        }
    }

    /// A recording reads the game's roster every 30 ticks.
    fn checkpoint_interval(s: &mut State, guest: &runtime::Guest) {
        s.roster(guest);
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
            // A default recording that failed is never listed half-written.
            if teardown && s.failure.is_some() {
                // Keep what was recorded: the game in play is cut up to the
                // tick the failure stopped the tape at.
                if s.header.player_features && matches!(&s.mode, Mode::Record(r) if r.out.is_some()) {
                    s.cut_game();
                }
                s.report_session_end();
                s.discard_auto();
            }
            return Ok(());
        }
        let Mode::Record(rec) = &s.mode else {
            unreachable!()
        };
        if !teardown && (!rec.match_over || !rec.frame.updates.is_empty()) {
            return Ok(());
        }
        // A default recording cuts each game into a film of its own and
        // keeps recording. At the session's end it cuts the game in play,
        // if any, and discards the tape (docs/game/film-format.md, "One
        // film per game").
        if s.header.player_features {
            s.cut_game();
            if teardown {
                s.complete = true;
                s.report_session_end();
                s.discard_auto();
            }
            return Ok(());
        }
        let matched = rec.match_over;
        let mut summary = rec.meta.summary(s.tick);
        rec.unified.summary_medals(&mut summary.players);
        if !summary.level.is_empty() {
            s.header.meta.map.clone_from(&summary.level);
            s.header.meta.variant.clone_from(&summary.variant_name);
            s.header.meta.players = summary.players.iter().map(|player| film::Player {
                name: player.name.clone(), team: player.team, score: player.score
            }).collect();
        }
        let tick = s.tick;
        s.header.meta.length_ticks = tick;
        let header = s.header.clone();
        let Mode::Record(rec) = &mut s.mode else {
            unreachable!()
        };
        let mut out = rec.out.take().context("film already closed")?;
        let path = rec.file.clone();
        let upload = rec.upload.take();
        let done = (|| -> Result<()> {
            rec.track.closing_events(std::mem::take(&mut rec.frame.events))?;
            rec.track.append_to_last(rec.unified.take_records(false, false)?)?;
            summary.dropped_samples = rec.unified.drop_counts();
            out.chunk(film::HEAD, &header)?;
            rec.meta.finish_map()?;
            rec.track.finish(&mut out, rec.meta.descriptor(), &summary)?;
            out.finish()?.sync_all()?;
            // A host's film file, sealed beside its stats stream.
            if let Some(film) = rec.film.take() {
                Arc::try_unwrap(film)
                    .map_err(|_| anyhow::anyhow!("the film file is still shared"))?
                    .into_inner()
                    .unwrap_or_else(|e| e.into_inner())
                    .finish()?
                    .sync_all()?;
            }
            Ok(())
        })();
        if let Err(e) = done {
            s.fail(format!("closing film: {e}"));
            return Ok(());
        }
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
impl State {
    fn fail(&mut self, why: String) {
        self.fail_at(self.tick, why)
    }

    /// Close and delete a default recording's tape and its film file.
    fn discard_auto(&mut self) {
        if !self.header.player_features {
            return;
        }
        let Mode::Record(rec) = &mut self.mode else { return };
        if rec.out.is_none() && !rec.file.exists() {
            return;
        }
        drop(rec.out.take());
        drop(rec.film.take());
        rec.frame = Default::default();
        if let Err(error) = autorecord::remove(&rec.file) {
            eprintln!("theater: could not delete {}: {error:#}", rec.file.display());
        }
    }

    /// Seal the game a default recording just played as a film of its own,
    /// cut from the running tape, and describe the next game from its
    /// start (docs/game/film-format.md, "One film per game"). A game that
    /// never started (menus, Theater, settings) has no film. A cut that
    /// cannot be written is logged and kept as the last discard reason; the
    /// tape keeps recording.
    fn cut_game(&mut self) {
        let tick = self.tick;
        let tape = self.header.clone();
        let Mode::Record(rec) = &mut self.mode else { return };
        rec.match_over = false;
        if rec.meta.descriptor().match_start_tick.is_some() {
            // The tape keeps the game's closing observations, as a film
            // sealed at its game's end does, so every later cut has them.
            let closed = (|| -> Result<()> {
                rec.track.closing_events(std::mem::take(&mut rec.frame.events))?;
                rec.track.append_to_last(rec.unified.take_records(false, false)?)?;
                rec.out.as_ref().context("film already closed")?;
                Ok(())
            })();
            if let Err(error) = closed {
                self.fail(format!("closing a game on the tape: {error:#}"));
                return;
            }
            match autorecord::cut(rec, &tape, tick) {
                Ok(path) => {
                    eprintln!("theater: this game's film is {}", path.display());
                    self.outcome.films.push(path);
                }
                Err(error) => {
                    eprintln!("theater: could not keep this game's film: {error:#}");
                    self.outcome.discard = Some(format!("could not keep this game's film: {error:#}"));
                }
            }
        }
        let Mode::Record(rec) = &mut self.mode else { return };
        rec.meta.next_game();
        rec.starts.next_game();
        rec.unified.next_game();
        rec.frame.next_game();
        rec.game_started_ms = None;
    }

    /// Forget the game in play without cutting it; its ticks stay on the
    /// tape. Returns whether a game was in play.
    fn drop_game(&mut self, why: String) -> bool {
        let Mode::Record(rec) = &mut self.mode else { return false };
        if !rec.in_game() {
            return false;
        }
        rec.match_over = false;
        rec.meta.next_game();
        rec.starts.next_game();
        rec.unified.next_game();
        rec.frame.next_game();
        rec.game_started_ms = None;
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
        let dir = rec.file.parent().unwrap_or(std::path::Path::new("."));
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
                &engine::points::PLAYER_KILLED,
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
