//! Host film lifetime: boot facts before guest entry, observer-only checkpoints,
//! then seal and optionally upload at the first slice after game over.
mod controls;
use anyhow::{Context, Result, bail, ensure};
pub(crate) use controls::library_methods;
use film::{
    FilmMeta, Header, Reader, Source, Writer,
    tape::{Batch, Call, Check, Mark, Playback, Tap},
};
use serde_json::Value;
use std::{
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
    /// Receiver for a requested restart, consumed by the CLI parent.
    pub restart: PathBuf,
    pub window_closed: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

struct Recording {
    out: Option<Writer<File>>,
    batch: Batch,
    file: PathBuf,
    match_over: bool,
}
enum Mode {
    Record(Recording),
    Replay(Playback, Replay),
}
struct State {
    mode: Mode,
    header: Header,
    tick: u64,
    calls: u64,
    paused: bool,
    speed: f64,
    seeking: Option<u64>,
    restart: Option<u64>,
    failure: Option<String>,
    complete: bool,
}
pub(crate) struct Facts {
    pub stamp: String,
    pub title_id: u32,
    pub epoch: u64,
    pub light: bool,
    pub bots: bool,
    pub address: String,
    pub live_service: bool,
}
pub(crate) struct Session {
    state: Mutex<State>,
    upload: Mutex<Option<std::thread::JoinHandle<()>>>,
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
            live_service,
        } = facts;
        let switches = feature::Switches::from_env()
            .map_err(anyhow::Error::msg)?
            .spec();
        let (mode, header, speed, seeking, paused) = if let Some(r) = replay {
            ensure!(record.is_none(), "cannot record and replay the same boot");
            ensure!((0.25..=4.).contains(&r.speed), "speed must be 0.25 to 4");
            let tape = Playback::load(Reader::open(&r.path)?)?;
            let mut h = tape.reader.header.clone();
            ensure!(h.meta.playable_here(&stamp), "film image stamp differs");
            ensure!(
                h.xbe_sha256 == film::xbe_digest(xbe)
                    && h.title_id == title_id
                    && h.features == switches
                    && h.epoch_100ns == epoch
                    && h.transfers_per_slice == opts.transfers_per_slice,
                "film boot facts differ"
            );
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
            let header = Header {
                version: 1,
                title_id,
                xbe_sha256: film::xbe_digest(xbe),
                epoch_100ns: epoch,
                lease_id: std::env::var("H2_LEASE").ok(),
                xnet_addr: address,
                lobby: std::env::var("H2_HOST_LOBBY").as_deref() == Ok("1"),
                lobby_machine_start: std::env::var("H2_HOST_LOBBY_MACHINE_START").as_deref()
                    != Ok("0"),
                host_bots: bots,
                telemetry: false,
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
                    source: if host_light {
                        Source::Host
                    } else {
                        Source::Local
                    },
                },
            };
            let manifest = film::manifest::Manifest::capture(&opts.data_root)?;
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
                }),
                header,
                1.,
                None,
                false,
            )
        } else {
            return Ok(None);
        };
        Ok(Some(Arc::new(Self {
            state: Mutex::new(State {
                mode,
                header,
                speed,
                seeking,
                paused,
                tick: 0,
                calls: 0,
                restart: None,
                failure: None,
                complete: false,
            }),
            upload: Mutex::default(),
        })))
    }

    pub fn record_telemetry(&self, enabled: bool) {
        let mut s = self.lock();
        if matches!(s.mode, Mode::Record(_)) {
            s.header.telemetry = enabled;
        }
    }
    pub fn has_telemetry(&self) -> bool {
        self.lock().header.telemetry
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
        let header = s.header.clone();
        match &mut s.mode {
            Mode::Record(rec) => {
                if let Some(out) = &mut rec.out {
                    let r = rec
                        .batch
                        .flush(out)
                        .and_then(|()| out.chunk(film::HASH, &actual))
                        .and_then(|()| out.chunk(film::HEAD, &header));
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
        mut window_open: impl FnMut() -> bool,
    ) -> Result<bool> {
        loop {
            {
                let mut s = self.lock();
                s.check_end(slices)?;
                let Mode::Replay(_, r) = &s.mode else {
                    return Ok(true);
                };
                if let Some(e) = &s.failure {
                    bail!("{e}")
                }
                if let Some(target) = s.restart {
                    std::fs::write(
                        &r.restart,
                        serde_json::to_vec(&(target, s.speed, s.paused))?,
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
            ensure!(window_open(), "film window closed");
            let a = agent.context("paused film needs the agent API")?;
            a.agent
                .poll_theater(&mut agent::GuestReader::new(|| rt.memory()));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    pub fn finish(
        &self,
        slices: u64,
        teardown: bool,
        telemetry: Option<&Arc<crate::telemetry::Telemetry>>,
    ) -> Result<()> {
        self.finish_output(slices, teardown, telemetry)?;
        if teardown
            && let Some(job) = self.upload.lock().unwrap_or_else(|e| e.into_inner()).take()
            && job.join().is_err()
        {
            eprintln!("theater: film upload worker stopped")
        }
        Ok(())
    }
    fn finish_output(
        &self,
        slices: u64,
        teardown: bool,
        telemetry: Option<&Arc<crate::telemetry::Telemetry>>,
    ) -> Result<()> {
        let mut s = self.lock();
        if let Mode::Replay(_, _) = &s.mode {
            s.check_end(slices)?;
            if let Some(e) = &s.failure {
                bail!("{e}")
            }
            if teardown && !s.complete && s.restart.is_none() {
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
        if !teardown && !rec.match_over {
            return Ok(());
        }
        let matched = rec.match_over;
        let (tick, calls) = (s.tick, s.calls);
        s.header.meta.length_ticks = tick;
        let header = s.header.clone();
        let Mode::Record(rec) = &mut s.mode else {
            unreachable!()
        };
        let mut out = rec.out.take().context("film already closed")?;
        let path = rec.file.clone();
        let done = (|| -> Result<()> {
            rec.batch.flush(&mut out)?;
            out.chunk(film::HEAD, &header)?;
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
        s.complete = true;
        drop(s);
        if matched
            && let (Ok(url), Ok(token)) = (
                std::env::var("H2_HOST_LIVE_URL"),
                std::env::var("H2_HOST_TOKEN"),
            )
        {
            let telemetry = telemetry.cloned();
            let worker = std::thread::Builder::new()
                .name("film-upload".into())
                .spawn(move || {
                    let game = std::env::var("H2_FILM_GAME_ID")
                        .ok()
                        .or_else(|| telemetry.and_then(|t| t.film_game_id()));
                    let sent = game
                        .context("no telemetry game ID for film")
                        .and_then(|id| film::upload(&url, &token, &id, &path));
                    if let Err(e) = sent {
                        eprintln!("theater: upload failed for {}: {e}", path.display())
                    }
                });
            match worker {
                Ok(job) => *self.upload.lock().unwrap_or_else(|e| e.into_inner()) = Some(job),
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
        if self.failure.is_some() {
            return;
        }
        let reason = match self.mode {
            Mode::Record(_) => format!("recording stopped at tick {}: {why}", self.tick),
            Mode::Replay(_, _) => format!("first divergence at tick {}: {why}", self.tick),
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
    fn replaying(&self) -> bool {
        matches!(self.lock().mode, Mode::Replay(_, _))
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

pub(crate) struct Observer(pub Arc<Session>, pub Option<agent::Agent>);
impl feature::Feature for Observer {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new("theater-film", "Exact host film").doc("docs/game/theater.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        let session = self.0.clone();
        b.snapshot(feature::SnapshotPolicy::forbid_while(move || {
            let state = session.lock();
            (!state.complete && state.failure.is_none()).then(|| "film cursor is live".into())
        }));
        let session = self.0.clone();
        let agent = self.1.clone();
        b.frame().on_tick("film checkpoint", move |o| {
            session.checkpoint(&o.guest());
            loop {
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
        });
        let session = self.0.clone();
        b.game().on_end("film scoreboard", move |o| {
            let mut s = session.lock();
            if !matches!(s.mode, Mode::Record(_)) || s.complete {
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
                if let Mode::Record(r) = &mut s.mode {
                    r.match_over = true
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
