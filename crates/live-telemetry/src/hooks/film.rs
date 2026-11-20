//! Ordered film projection at the host's simulation boundary. Only legacy
//! telemetry delivery can run ahead or behind that clock.
use std::sync::{Arc, Mutex};

use super::{Telemetry, TelemetryFeature, signed_in};
use crate::config::{TelemetryConfig, TelemetrySinkConfig};
use crate::event::{GameEvent, Output};
use crate::pipeline::Work;
use crate::server::ServerSink;
use crate::session::Session;
use crate::sink::{JsonlSink, Sink};
use live_proto::{film::objects::Scene, game::Frame};

/// The film's local, ordered consumer. The engine and this observer share one
/// simulation clock; transport latency must never date or discard a film record.
pub trait FilmObserver: Send + Sync + 'static {
    fn active(&self) -> bool;
    fn event(&self, event: &GameEvent, clock: u64) -> Result<(), String>;
    fn frame(&self, frame: Frame) -> Result<(), String>;
    fn scene(&self, scene: Scene) -> Result<(), String>;
    fn record(&self, record: live_proto::film::events::Record) -> Result<(), String>;
    fn clock(&self, clock: Option<u64>);
    fn failed(&self, reason: &str);
}

pub(super) struct FilmProjection {
    pub(super) session: Mutex<Session>,
    pub(super) scene: Mutex<crate::FilmSceneReader>,
    pub(super) observer: Box<dyn FilmObserver>,
    /// The worker is also delivering projected records to a legacy sink.
    pub(super) transport: bool,
    transport_failure: Mutex<Option<String>>,
}

impl Telemetry {
    pub(super) fn submit(&self, work: Work) -> Result<(), String> {
        let Some(film) = &self.film else {
            return self.pipeline.push(work);
        };
        let mut session = film.session.lock().unwrap();
        if matches!(work, Work::Start(_)) {
            *film.scene.lock().unwrap() = Default::default();
        }
        let mut report = None;
        let snapshot = match work {
            Work::FinishSim(value) => {
                session.end_previous();
                report = Some(value);
                None
            }
            work => crate::pipeline::apply(&mut session, work),
        };
        let clock = session.match_clock();
        let legacy_frames = film.transport && session.live_frames();
        let active = film.observer.active();
        let projected = session.take_output();
        drop(session);
        if active {
            film.observer.clock(clock);
        }
        let mut output = Vec::new();
        for item in projected {
            match item {
                Output::Event(event) => {
                    if active {
                        film.observer.event(&event, clock.unwrap_or_default())?;
                    }
                    if film.transport {
                        output.push(Output::Event(event));
                    }
                }
                Output::Frame(frame) if active => {
                    if legacy_frames {
                        output.push(Output::Frame(frame.clone()));
                    }
                    film.observer.frame(frame)?;
                }
                Output::Frame(frame) if legacy_frames => output.push(Output::Frame(frame)),
                Output::Frame(_) => {}
            }
        }
        if let Some(snapshot) = snapshot {
            self.pipeline.recycle(snapshot);
        }
        if film.transport && !output.is_empty() {
            let mut failure = film.transport_failure.lock().unwrap();
            if failure.is_none()
                && let Err(error) = self.pipeline.forward(output)
            {
                self.pipeline.log_error(&error);
                *failure = Some(error);
            }
        }
        if let Some(report) = report {
            if let Some(reason) = film.transport_failure.lock().unwrap().as_ref() {
                return Err(reason.clone());
            }
            self.pipeline.settle();
            self.pipeline.push(Work::FinishSim(report))?;
        }
        Ok(())
    }

    pub(crate) fn film_observing(&self) -> bool {
        self.film.as_ref().is_some_and(|f| f.observer.active())
    }

    pub(crate) fn film_event(
        &self,
        mut record: live_proto::film::events::Record,
    ) -> Result<(), String> {
        if let Some(film) = &self.film
            && film.observer.active()
        {
            let session = film.session.lock().unwrap();
            let Some(clock) = session.match_clock_at(record.tick) else {
                return Ok(());
            };
            record.tick = clock;
            if let live_proto::film::events::Event::WeaponFire {
                weapon,
                barrel: Some(barrel),
                ..
            } = &mut record.event
            {
                *weapon = barrel
                    .definition
                    .and_then(|definition| session.film_weapon(definition));
            }
            drop(session);
            film.observer.record(record)?;
        }
        Ok(())
    }

    /// A local recording can close before the engine fires GAME_END.
    pub fn close_film(&self) {
        if self.film_observing() {
            self.protect(|| self.submit(Work::Configure(Box::new(Session::end_previous))));
        }
    }
}

/// Install the same observer set for both Live and local films. The film
/// receives every projection before the scheduler crosses its tick boundary.
/// Only legacy file/network delivery uses the asynchronous best-effort path.
pub fn install_film(
    registry: &mut feature::Registry,
    config: &TelemetryConfig,
    observer: Option<Box<dyn FilmObserver>>,
) -> Result<Option<Arc<Telemetry>>, String> {
    if !config.enabled && observer.is_none() {
        return Ok(None);
    }
    let mut session =
        Session::new("h2recomp", Some("h2recomp".into())).with_signed_in(signed_in(config));
    if let TelemetrySinkConfig::Simulation { game, .. }
    | TelemetrySinkConfig::Host {
        game: Some(game), ..
    } = &config.sink
    {
        session = session.with_sim(game.clone());
    }
    let sink = transport_sink(config);
    let (sink, transport) = match sink {
        Ok(sink) => (sink, config.enabled),
        Err(error) if observer.is_some() => {
            eprintln!("live telemetry transport disabled: {error}");
            (Box::new(crate::sink::Discard) as Box<dyn Sink>, false)
        }
        Err(error) => return Err(error),
    };
    let mut telemetry = if let Some(observer) = observer {
        session.filming = true;
        let mut telemetry = Telemetry::new(Session::new("transport", None), sink);
        telemetry.film = Some(FilmProjection {
            session: Mutex::new(session),
            scene: Default::default(),
            observer,
            transport,
            transport_failure: Default::default(),
        });
        telemetry
    } else {
        Telemetry::new(session, sink)
    };
    telemetry.film_hooks = telemetry.film.is_some()
        || matches!(
            config.sink,
            TelemetrySinkConfig::Discard { film_effects: true }
        );
    // Replay needs the same hook addresses for scheduler parity. Its viewer
    // consumes the film: no second live telemetry session needs to observe it.
    if matches!(config.sink, TelemetrySinkConfig::Discard { .. }) {
        telemetry
            .disabled
            .store(true, std::sync::atomic::Ordering::Release);
    }
    let telemetry = Arc::new(telemetry);
    match registry.add(&TelemetryFeature(Arc::clone(&telemetry))) {
        Ok(true) => Ok(Some(telemetry)),
        Ok(false) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn transport_sink(config: &TelemetryConfig) -> Result<Box<dyn Sink>, String> {
    if !config.enabled {
        return Ok(Box::new(crate::sink::Discard));
    }
    Ok(match &config.sink {
        TelemetrySinkConfig::Discard { .. } => Box::new(crate::sink::Discard),
        TelemetrySinkConfig::File(path) => Box::new(JsonlSink::create(path)?),
        TelemetrySinkConfig::Server { base_url, account } => {
            Box::new(ServerSink::new(base_url.clone(), account.clone())?)
        }
        TelemetrySinkConfig::Host {
            base_url,
            token,
            game,
        } => {
            let sink = ServerSink::for_host(base_url.clone(), token.clone())?;
            Box::new(match game {
                Some(game) => sink.with_sim(game, None),
                None => sink,
            })
        }
        TelemetrySinkConfig::Simulation {
            base_url,
            account,
            game,
            deferred_lock,
        } => Box::new(
            ServerSink::new(base_url.clone(), account.clone())?
                .with_sim(game, deferred_lock.clone()),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::GameInfo;

    #[derive(Default)]
    struct CaptureLog {
        events: Mutex<Vec<String>>,
        frames: Mutex<Vec<u64>>,
        clock: Mutex<Option<u64>>,
    }

    impl FilmObserver for Arc<CaptureLog> {
        fn active(&self) -> bool {
            true
        }
        fn event(&self, event: &GameEvent, _: u64) -> Result<(), String> {
            self.events.lock().unwrap().push(event.kind.clone());
            Ok(())
        }
        fn frame(&self, frame: Frame) -> Result<(), String> {
            self.frames.lock().unwrap().push(frame.tick);
            Ok(())
        }
        fn clock(&self, value: Option<u64>) {
            *self.clock.lock().unwrap() = value;
        }
        fn scene(&self, _: Scene) -> Result<(), String> {
            Ok(())
        }
        fn record(&self, _: live_proto::film::events::Record) -> Result<(), String> {
            Ok(())
        }
        fn failed(&self, error: &str) {
            panic!("film capture failed: {error}")
        }
    }

    #[test]
    fn offline_film_keeps_each_sample_and_closes_without_an_engine_end_hook() {
        let log = Arc::new(CaptureLog::default());
        let mut registry = feature::Registry::new(feature::Switches::defaults());
        let telemetry = install_film(
            &mut registry,
            &TelemetryConfig::default(),
            Some(Box::new(log.clone())),
        )
        .unwrap()
        .unwrap();
        telemetry
            .submit(Work::Start(Box::new(crate::pipeline::Start {
                info: GameInfo {
                    map: "lockout".into(),
                    mode: "multiplayer".into(),
                    difficulty: None,
                    skulls: None,
                    variant: Some("Slayer".into()),
                    team_play: false,
                    score_limit: None,
                    time_limit_s: None,
                    tick_rate: 60,
                    tick: 0,
                },
                players: Vec::new(),
                teams: Vec::new(),
                map_id: None,
                placements: Default::default(),
                names: None,
            })))
            .unwrap();
        for tick in 1..=300 {
            telemetry
                .submit(Work::Tick(Box::new(crate::event::Snapshot {
                    tick,
                    ..Default::default()
                })))
                .unwrap();
        }
        // No settle: the film sees a completed projection at the same game-thread boundary.
        assert_eq!(
            *log.frames.lock().unwrap(),
            (4..=300).step_by(4).collect::<Vec<_>>()
        );
        assert_eq!(*log.clock.lock().unwrap(), Some(300));
        telemetry.close_film();
        assert_eq!(*log.clock.lock().unwrap(), None);
        assert_eq!(*log.events.lock().unwrap(), ["GameStarted", "GameEnded"]);
    }
}
