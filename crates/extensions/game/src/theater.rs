//! Host film lifetime: boot facts before guest entry, observer-only checkpoints,
//! then seal and optionally upload at the first slice after game over.
mod actions;
mod camera;
mod compatibility;
#[cfg(feature = "gpu")]
mod host_view;
#[cfg(feature = "gpu")]
pub use host_view::HostView;
pub use camera::{FilmCamera, RenderHooks};
pub mod bot_tape;
mod rig;
mod controls;
mod metadata;
mod unified;
mod objectives;
mod record;
pub use record::in_keyframe;
mod startup;
pub use startup::Startup;
pub use startup::configure_replay_child;
mod replay;
mod local_pads;
mod video_menu;
use anyhow::{Context, Result, bail, ensure};
use rig::ReplayCamera;
pub use controls::library_methods;
pub use controls::{playable_entry, playback_images};
use film::{
    FilmMeta, Header, Reader, Source, Writer,
    tape::{Batch, Call, Check, Mark, Playback, Tap},
};
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
    pub path: PathBuf,
    pub render_every: u32,
    pub speed: f64,
    pub seek_tick: u64,
    pub paused_at_target: bool,
    pub presentation: Presentation,
    /// Receiver for a requested restart, consumed by the CLI parent.
    pub restart: PathBuf,
    pub camera: FilmCamera,
    pub window_closed: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// View and timeline state carried across an exact replay restart.
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

struct Recording {
    out: Option<Writer<File>>,
    batch: Batch,
    file: PathBuf,
    match_over: bool,
    track: film::blam::TrackWriter,
    meta: metadata::Capture,
    frame: record::Capture,
    unified: unified::Capture,
    disk: Arc<Mutex<film::manifest::DiskTracker>>,
    upload: Option<UploadTarget>,
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
    Replay(Playback, Replay),
}
struct State {
    mode: Mode,
    engine: Option<replay::EngineReplay>,
    header: Header,
    tick: u64,
    calls: u64,
    paused: bool,
    speed: f64,
    seeking: Option<u64>,
    presentation: Presentation,
    restart: Option<u64>,
    failure: Option<String>,
    complete: bool,
}
/// Check an installed, current module against the recording. The launcher also
/// uses this result to choose the disk state matching the replay's starting RAM.
pub fn image_match(
    version: u32,
    original: &str,
    runtime: &str,
    guest: Option<&film::image::GuestImage>,
    candidate: &str,
    xbe_sha256: &str,
) -> Result<film::image::ImageMatch> {
    ensure!(version == 1 || runtime == compatibility::SIMULATION_ABI, "film simulation ABI differs (recorded {runtime}, installed {})", compatibility::SIMULATION_ABI);
    if version >= 3 && let Some(guest) = guest {
        return guest.compare(original, candidate, recomp::host_triple(), xbe_sha256)
            .map_err(|reason| anyhow::anyhow!("film image stamp differs: {reason}"));
    }
    ensure!(!candidate.is_empty() && original == candidate, "film requires its exact native image stamp");
    Ok(film::image::ImageMatch::Native)
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
}
pub struct Session {
    version: u32,
    replaying: bool,
    state: Mutex<State>,
    video_menu: std::sync::OnceLock<Arc<settings::Store>>,
    #[cfg(feature = "gpu")]
    view: std::sync::OnceLock<Mutex<host_view::MountedView>>,
    bots: Mutex<bot_tape::BotTape>,
    upload: Mutex<Option<std::thread::JoinHandle<Result<()>>>>,
    require_upload: bool,
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
        } = facts;
        let switches = hooks::Switches::from_env()
            .map_err(anyhow::Error::msg)?
            .spec();
        let mut image = film::image::ImageMatch::Native;
        let (mode, header, speed, seeking, paused) = if let Some(r) = replay {
            ensure!(record.is_none(), "cannot record and replay the same boot");
            ensure!((0.25..=4.).contains(&r.speed), "speed must be 0.25 to 4");
            let tape = Playback::load(Reader::open(&r.path)?)?;
            let mut h = tape.reader.header.clone();
            image = image_match(h.version, &h.meta.image_stamp, &h.runtime_stamp,
                h.guest_image.as_ref(), &stamp, &h.xbe_sha256)?;
            ensure!(
                h.xbe_sha256 == film::xbe_digest(xbe)
                    && h.title_id == title_id
                    && h.features == switches
                    && h.epoch_100ns == epoch
                    && h.transfers_per_slice == opts.transfers_per_slice,
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
            ensure!(r.seek_tick <= tape.length, "seek beyond film");
            h.meta.length_ticks = tape.length;
            let (speed, target, paused) = (
                r.speed,
                (r.seek_tick > 0).then_some(r.seek_tick),
                r.paused_at_target && r.seek_tick == 0,
            );
            (Mode::Replay(tape, r), h, speed, target, paused)
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
                features: switches,
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
            let (manifest, disk) = film::manifest::Manifest::capture_for_recording(&opts.data_root)?;
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
            out.chunk(film::DATA, &manifest)?;
            (
                Mode::Record(Recording {
                    out: Some(out),
                    batch: Batch::default(),
                    file: path,
                    match_over: false,
                    track: Default::default(),
                    meta: metadata::Capture::new(&header),
                    frame: Default::default(),
                    unified: Default::default(),
                    disk: Arc::new(Mutex::new(disk)),
                    upload,
                }),
                header,
                1.,
                None,
                false,
            )
        } else {
            return Ok(None);
        };
        let engine = match &mode {
            Mode::Replay(_, r) if header.version >= 2 => Some(replay::EngineReplay::open(r, image)?),
            _ => None,
        };
        let bot_tape = bot_tape::BotTape::new(&mode, header.version);
        let mut presentation = match &mode {
            Mode::Replay(_, opts) => opts.presentation.clone(),
            Mode::Record(_) => Presentation::default(),
        };
        if let Mode::Replay(_, opts) = &mode {
            if !presentation.camera.initialized {
                if opts.render_every != 0 && header.meta.source == Source::Host {
                    presentation.camera.mode = agent_proto::theater::CameraMode::Free;
                }
                presentation.camera.initialized = true;
            }
            opts.camera.restore(&presentation);
        }
        Ok(Some(Arc::new(Self {
            version: header.version,
            video_menu: Default::default(),
            replaying: matches!(&mode, Mode::Replay(_, _)),
            require_upload: matches!(&mode, Mode::Record(rec) if rec.upload.is_some())
                && std::env::var("H2_HOST_ARCHIVE").as_deref() == Ok("film"),
            bots: Mutex::new(bot_tape),
            #[cfg(feature = "gpu")]
            view: Default::default(),
            state: Mutex::new(State {
                mode,
                engine,
                header,
                speed,
                seeking,
                paused,
                presentation,
                tick: 0,
                calls: 0,
                restart: None,
                failure: None,
                complete: false,
            }),
            upload: Mutex::default(),
        })))
    }

    pub fn has_engine_track(&self) -> bool {
        self.version >= 2
    }

    pub fn render_hooks(&self) -> film::RenderHooks {
        self.lock().header.render_hooks
            .unwrap_or_else(crate::run_plan::compiled_render_hooks)
    }

    pub fn telemetry_plan(&self) -> live_proto::film::TelemetryHooks {
        self.lock().header.telemetry_plan()
    }

    fn checkpoint(&self, guest: &runtime::Guest) {
        let mut s = self.lock();
        if s.complete || s.failure.is_some() {
            return;
        }
        s.tick += 1;
        let tick = s.tick;
        if s.seeking == Some(tick) {
            s.seeking = None;
            s.paused = matches!(&s.mode, Mode::Replay(_,r) if r.paused_at_target);
        }
        if s.header.version >= 2 {
            let hash = crate::sim_glue::film_gameplay_hash(guest);
            if let Mode::Record(rec) = &mut s.mode {
                if let Err(e) = rec.post_tick(guest, tick, hash) {
                    s.fail(format!("writing engine tick: {e:#}"));
                    return;
                }
            } else {
                Session::check_replay_tick(&mut s, guest, hash);
            }
        }
        if !tick.is_multiple_of(30) {
            return;
        }
        let actual = Check {
            tick,
            sim: lockstep::hash_sim(guest, lockstep::SimRegion::HALO2),
            calls: s.calls,
        };
        if matches!(s.mode, Mode::Record(_)) {
            s.roster(guest);
        }
        match &mut s.mode {
            Mode::Record(rec) => {
                if let Some(out) = &mut rec.out {
                    let r = rec
                        .batch
                        .flush(out)
                        .and_then(|()| out.chunk(film::HASH, &actual));
                    if let Err(e) = r {
                        s.fail(e.to_string());
                    }
                }
            }
            Mode::Replay(tape, _) => {
                let wanted = tape.checks.pop_front();
                if wanted.as_ref() != Some(&actual) {
                    s.fail(format!("expected {wanted:?}, got {actual:?}"));
                } else if tape.end.is_none() && tape.checks.is_empty() {
                    s.complete = true;
                }
            }
        }
    }

    /// Poll before executing another slice. The logical clock remains still
    /// during transport pauses. Seeking just removes the host wall-time wait.
    pub fn before_slice(
        &self,
        slices: u64,
        m: &mut kernel::Machine,
        agent: Option<&crate::agent_glue::AgentGlue>,
        rt: &runtime::Runtime,
    ) -> Result<bool> {
        self.bot_boundary()?;
        loop {
            #[cfg(feature = "gpu")]
            self.draw_host_view(rt);
            {
                let mut s = self.lock();
                s.check_end(slices)?;
                let Mode::Replay(_, r) = &s.mode else {
                    return Ok(true);
                };
                if let Some(e) = &s.failure {
                    bail!("{e}")
                }
                if r.window_closed.as_ref().is_some_and(|closed| closed()) {
                    return Ok(false);
                }
                if let Some(target) = s.restart {
                    let mut presentation = s.presentation.clone();
                    r.camera.save(&mut presentation);
                    std::fs::write(
                        &r.restart,
                        serde_json::to_vec(&(target, s.speed, s.paused, &presentation))?,
                    )?;
                    return Ok(false);
                }
                if s.complete {
                    return Ok(false);
                }
                if !s.paused {
                    let pace = s
                        .seeking
                        .is_none()
                        .then(|| (kernel::clock::Clock::FREQUENCY as f64 * s.speed) as u64);
                    if m.pacer.as_ref().map(|p| p.ticks_per_second) != pace {
                        m.pacer = pace.map(|rate| {
                            let mut p = kernel::pacer::Pacer::real_time();
                            p.ticks_per_second = rate;
                            p
                        });
                    }
                    return Ok(true);
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
    pub fn finish_uploaded_sim(&self, slices: u64, published: bool) -> Result<()> {
        if let Mode::Record(r) = &mut self.lock().mode {
            r.match_over = published;
        }
        self.finish(slices, true)
    }

    pub fn finish(&self, slices: u64, teardown: bool) -> Result<()> {
        self.finish_output(slices, teardown)?;
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

    fn finish_output(&self, slices: u64, teardown: bool) -> Result<()> {
        let mut s = self.lock();
        if let Mode::Replay(_, replay) = &s.mode {
            let window_closed = replay.window_closed.as_ref().is_some_and(|closed| closed());
            s.check_end(slices)?;
            if let Some(e) = &s.failure {
                bail!("{e}")
            }
            if teardown && !s.complete && s.restart.is_none() && !window_closed {
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
            return Ok(());
        }
        let Mode::Record(rec) = &s.mode else {
            unreachable!()
        };
        if !teardown && (!rec.match_over || rec.frame.actions.is_some()) {
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
        let (tick, calls) = (s.tick, s.calls);
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
            rec.batch.flush(&mut out)?;
            out.chunk(film::HEAD, &header)?;
            rec.meta.finish_map()?;
            rec.track.finish(&mut out, rec.meta.descriptor(), &summary)?;
            out.chunk(
                film::MARK,
                &Mark::End {
                    slices,
                    tick,
                    calls,
                },
            )?;
            out.finish()?.sync_all()?;
            Ok(())
        })();
        if let Err(e) = done {
            s.fail(format!("closing film: {e}"));
            return Ok(());
        }
        // Release the retained physical RAM anchor when a local game returns to the lobby.
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
    fn check_end(&mut self, slices: u64) -> Result<()> {
        if self.complete || self.failure.is_some() || self.restart.is_some() {
            return Ok(());
        }
        if let Mode::Replay(tape, _) = &self.mode
            && let Some((end, tick, calls)) = tape.end
            && slices >= end
        {
            ensure!(
                self.tick == tick && self.calls == calls && tape.exhausted(),
                "first divergence at tick {}: end of film expected tick {tick}, {calls} calls; got {}",
                self.tick,
                self.calls
            );
            self.complete = true;
        }
        Ok(())
    }
    fn fail(&mut self, why: String) {
        self.fail_at(self.tick, why)
    }

    fn fail_at(&mut self, tick: u64, why: String) {
        if self.failure.is_some() {
            return;
        }
        let reason = match &mut self.mode {
            Mode::Record(rec) => {
                rec.frame = Default::default();
                format!("recording stopped at tick {tick}: {why}")
            }
            Mode::Replay(_, _) => format!("first divergence at tick {tick}: {why}"),
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
impl Tap for Session {
    fn service_calls(&self) -> bool {
        self.version >= 2
    }
    fn replaying(&self) -> bool {
        self.replaying
    }
    fn next(&self, op: &str, args: &[u32], thread: u32) -> Option<Call> {
        let mut s = self.lock();
        if s.failure.is_some() || s.complete {
            return None;
        }
        let i = s.calls;
        let Mode::Replay(tape, _) = &mut s.mode else {
            return None;
        };
        match tape.call(i, op, args, thread) {
            Ok(c) => {
                s.calls += 1;
                Some(c)
            }
            Err(e) => {
                s.fail(e.to_string());
                None
            }
        }
    }
    fn record(&self, tag: film::Tag, mut c: Call) {
        let mut s = self.lock();
        if s.complete || s.failure.is_some() {
            return;
        }
        c.index = s.calls;
        let Mode::Record(rec) = &mut s.mode else {
            return;
        };
        if let Some(out) = &mut rec.out {
            match rec.batch.append(out, tag, c) {
                Ok(()) => s.calls += 1,
                Err(e) => s.fail(e.to_string()),
            }
        }
    }
    fn fault(&self, reason: String) {
        self.lock().fail(reason)
    }
}
impl agent::LobbyTape for Session {
    fn record_lobby(&self, boundary: u64, name: &str, params: &Value) {
        let mut s = self.lock();
        if s.complete || s.failure.is_some() {
            return;
        }
        if let Mode::Record(rec) = &mut s.mode
            && let Some(out) = &mut rec.out
            && let Err(e) = out.chunk(
                film::MARK,
                &Mark::Lobby {
                    boundary,
                    method: name.into(),
                    params: params.clone(),
                },
            )
        {
            s.fail(e.to_string())
        }
    }
    fn lobby_due(&self, boundary: u64) -> Vec<(String, Value)> {
        let mut s = self.lock();
        let Mode::Replay(t, _) = &mut s.mode else {
            return vec![];
        };
        let mut actions = vec![];
        while t.lobby.front().is_some_and(|(at, _, _)| *at == boundary) {
            let (_, method, p) = t.lobby.pop_front().unwrap();
            actions.push((method, p));
        }
        actions
    }
    fn lobby_error(&self, message: String) {
        self.lock().fail(message)
    }
}

pub struct Observer(pub Arc<Session>, pub Option<agent::Agent>);
impl hooks::Feature for Observer {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("theater-film", "Exact host film").doc("docs/game/theater.md")
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
        }
        let session = self.0.clone();
        let live = move || {
            let state = session.lock();
            (!in_keyframe() && !state.complete && state.failure.is_none()).then(|| "film cursor is live".into())
        };
        b.snapshot(video_menu::snapshot(self.0.video_menu.get(), live));
        let session = self.0.clone();
        let agent = self.1.clone();
        b.frame().on_tick("film checkpoint", move |o| {
            session.checkpoint(&o.guest());
            loop {
                #[cfg(feature = "gpu")]
                session.draw_host_view(o.runtime);
                {
                    let s = session.lock();
                    let Mode::Replay(_, r) = &s.mode else { break };
                    if s.failure.is_some()
                        || s.complete
                        || s.restart.is_some()
                        || r.window_closed.as_ref().is_some_and(|closed| closed())
                    {
                        o.runtime.request_preempt(runtime::preempt::REQUEST);
                        break;
                    }
                    if !s.paused {
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
        if (!self.0.has_engine_track() && self.0.replaying())
            || (self.0.has_unified_track() && self.0.telemetry_plan().enabled())
        {
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
