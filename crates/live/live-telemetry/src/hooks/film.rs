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
    fn dropped(&self, source: &'static str);
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

    pub(crate) fn dropped_film_sample(&self, source: &'static str) {
        if let Some(film) = &self.film {
            film.observer.dropped(source);
        }
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
    registry: &mut ::hooks::Registry,
    config: &TelemetryConfig,
    observer: Option<Box<dyn FilmObserver>>,
    hooks: live_proto::film::TelemetryHooks,
) -> Result<Option<Arc<Telemetry>>, String> {
    if !hooks.enabled() {
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
    let combat = super::CombatFeed::default();
    let sink = transport_sink(config, &combat);
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
    telemetry.combat = combat;
    telemetry.hooks = hooks;
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

fn transport_sink(config: &TelemetryConfig, combat: &super::CombatFeed) -> Result<Box<dyn Sink>, String> {
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
                Some(game) => sink.with_sim(game, None, combat.clone()),
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
                .with_sim(game, deferred_lock.clone(), combat.clone()),
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
        actor_kills: Mutex<Vec<GameEvent>>,
        frames: Mutex<Vec<u64>>,
        clock: Mutex<Option<u64>>,
        dropped: Mutex<Vec<&'static str>>,
        records: Mutex<Vec<live_proto::film::events::Record>>,
    }

    impl FilmObserver for Arc<CaptureLog> {
        fn active(&self) -> bool {
            true
        }
        fn event(&self, event: &GameEvent, _: u64) -> Result<(), String> {
            self.events.lock().unwrap().push(event.kind.clone());
            if event.kind == live_proto::game::ACTOR_KILLED
                || event.kind == live_proto::game::FIREFIGHT_SCORE
            {
                self.actor_kills.lock().unwrap().push(event.clone());
            }
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
        fn record(&self, record: live_proto::film::events::Record) -> Result<(), String> {
            self.records.lock().unwrap().push(record);
            Ok(())
        }
        fn dropped(&self, source: &'static str) {
            self.dropped.lock().unwrap().push(source);
        }
    }

    #[test]
    fn bad_event_observations_are_counted_and_the_next_tick_recovers() {
        use crate::{layout, memory::fixture};
        use live_proto::film::{TelemetryHooks, events::Event};

        let log = Arc::new(CaptureLog::default());
        let mut registry = ::hooks::Registry::new(::hooks::Switches::defaults());
        let t = install_film(
            &mut registry,
            &TelemetryConfig::default(),
            Some(Box::new(log.clone())),
            TelemetryHooks::UNIFIED,
        )
        .unwrap()
        .unwrap();
        t.protect(|| t.start(&runtime::Guest::new()));
        assert!(!t.disabled());

        let mut g = fixture::memory();
        g.set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2)
            .unwrap();
        g.set_u16(
            fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE,
            30,
        )
        .unwrap();
        g.write_bytes(layout::CACHE_MAP_NAME, b"lockout\0", 1)
            .unwrap();
        // Tick retries the failed start after loading the roster.
        t.protect(|| t.sample(&g));
        assert_eq!(*log.events.lock().unwrap(), ["GameStarted"]);
        g.set_u32(fixture::TIME + layout::TIME_TICK, 1).unwrap();
        let runtime = runtime::Runtime::new(g, runtime::Dispatch::default());
        let cpu = runtime::CpuAbi::default();
        let o = ::hooks::Obs {
            thread_id: 1,
            cpu: &cpu,
            runtime: &runtime,
            addr: guest_layout::points::DAMAGE_REPORT.base,
            point: &guest_layout::points::DAMAGE_REPORT,
        };
        for source in ["weapon_fire", "impact", "projectile_end"] {
            crate::impacts::observe(&t, source, &o, |guest| {
                guest_layout::GuestMemory::u32(guest, 0x7000_0000).map(|_| None)
            });
        }
        crate::impacts::observe(&t, "impact", &o, |_| {
            Ok(Some(Event::Pickup { unit: "p0".into() }))
        });
        assert_eq!(log.records.lock().unwrap().len(), 1);
        t.protect(|| t.sample(&runtime::Guest::new()));
        t.protect(|| t.sample(&runtime.memory()));
        assert_eq!(*log.clock.lock().unwrap(), Some(1));
        {
            let mut g = runtime.memory();
            g.set_u8(fixture::GLOBALS + layout::GAME_ENDED, 1).unwrap();
            g.set_u32(layout::PLAYERS_PTR, 0x7000_0000).unwrap();
        }
        t.protect(|| t.end(&runtime.memory()));
        assert_eq!(
            *log.dropped.lock().unwrap(),
            [
                "match_start",
                "weapon_fire",
                "impact",
                "projectile_end",
                "tick_clock",
                "final_scoreboard"
            ]
        );
        t.close_film();
        assert_eq!(*log.events.lock().unwrap(), ["GameStarted", "GameEnded"]);
        assert!(!t.disabled());
    }

    #[test]
    fn actor_kill_telemetry_projects_one_stats_record_with_the_credited_seat() {
        use crate::{layout, memory::fixture};
        use guest_layout::{DamageReport, DatumArray, actor::Actor, damage::DEPLETING, global};
        use live_proto::film::events::{Event, Tracker};
        use live_proto::game::ActorKilled;

        let log = Arc::new(CaptureLog::default());
        let mut registry = ::hooks::Registry::new(::hooks::Switches::defaults());
        let telemetry = install_film(
            &mut registry,
            &TelemetryConfig::default(),
            Some(Box::new(log.clone())),
            live_proto::film::TelemetryHooks::UNIFIED,
        )
        .unwrap()
        .unwrap();

        let mut guest = fixture::memory();
        guest.set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2).unwrap();
        guest.set_u16(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE, 30).unwrap();
        guest.write_bytes(layout::CACHE_MAP_NAME, b"synthetic\0", 1).unwrap();
        fixture::put_player(&mut guest, 0, "Killer", 0, true, u32::MAX);
        fixture::put_actor(&mut guest, 1, 1, [1.0, 2.0, 3.0]);
        let unit = fixture::object_datum(1);

        let bitmap = 0x160000;
        let actors = 0x161000;
        let data = 0x162000;
        let report = 0x164000;
        let character = 0xe200_0003;
        guest.set_u32(fixture::OBJECTS + DatumArray::BITMAP.offset, bitmap).unwrap();
        guest.set_u32(bitmap, 1 << 1).unwrap();
        guest.set_u32(global::ACTORS.base, actors).unwrap();
        guest.set_u32(actors + DatumArray::ELEMENT_SIZE.offset, Actor::SIZE).unwrap();
        guest.set_u32(actors + DatumArray::HIGH_WATER.offset, 2).unwrap();
        guest.set_u32(actors + DatumArray::DATA.offset, data).unwrap();
        let actor = data + Actor::SIZE;
        guest.set_u16(actor, 0xa000).unwrap();
        guest.set_u32(actor + Actor::UNIT, unit).unwrap();
        guest.set_u32(actor + Actor::CHARACTER, character).unwrap();
        guest.set_u32(report + DamageReport::OWNER_PLAYER.offset, 0x8001_0000).unwrap();
        guest.set_u8(report + DamageReport::CODE.offset, 13).unwrap();

        telemetry.start(&guest).unwrap();
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 12).unwrap();
        telemetry.actor_killed(&guest, report, unit).unwrap();
        guest.set_u32(report + DamageReport::FLAGS.offset, DEPLETING).unwrap();
        telemetry.actor_killed(&guest, report, unit).unwrap();

        let events = log.actor_kills.lock().unwrap();
        assert_eq!(events.len(), 1);
        let records = Tracker::default().event(&events[0].kind, &events[0].data, 13).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tick, 12);
        assert_eq!(records[0].event, Event::ActorKilled(ActorKilled {
            tick: 12,
            unit,
            character: Some(character),
            killer: Some(0),
            weapon: Some("sniper_rifle".into()),
        }));
    }

    #[test]
    fn a_firefight_score_is_a_game_event_and_a_running_objective_score_in_the_film() {
        use crate::{layout, memory::fixture};
        use live_proto::film::events::{Event, Tracker};
        use live_proto::firefight::{FirefightEnded, FirefightScore};

        let log = Arc::new(CaptureLog::default());
        let mut registry = ::hooks::Registry::new(::hooks::Switches::defaults());
        let telemetry = install_film(
            &mut registry,
            &TelemetryConfig::default(),
            Some(Box::new(log.clone())),
            live_proto::film::TelemetryHooks::UNIFIED,
        )
        .unwrap()
        .unwrap();
        let mut guest = fixture::memory();
        guest.set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2).unwrap();
        guest.set_u16(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE, 30).unwrap();
        guest.write_bytes(layout::CACHE_MAP_NAME, b"synthetic\0", 1).unwrap();
        fixture::put_player(&mut guest, 0, "Killer", 0, true, u32::MAX);
        // Before a game runs, a score is dropped.
        telemetry.firefight_score(FirefightScore {
            tick: 1,
            player: 0,
            points: 5,
            reason: "kill".into(),
            class: None,
        });
        telemetry.start(&guest).unwrap();
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 12).unwrap();
        for points in [100, 50] {
            telemetry.firefight_score(FirefightScore {
                tick: 12,
                player: 0,
                points,
                reason: "kill".into(),
                class: Some("grunt".into()),
            });
        }
        telemetry.firefight_ended(FirefightEnded {
            tick: 12,
            set: "Holdout".into(),
            teams: 1,
            outcome: "won".into(),
            team: None,
            scores: vec![150],
            rounds_cleared: 3,
            sets_cleared: 1,
            lives_used: 1,
        });
        let kinds = log.events.lock().unwrap().clone();
        assert_eq!(kinds, ["GameStarted", "FirefightScore", "FirefightScore", "FirefightEnded"]);
        let scores = log.actor_kills.lock().unwrap();
        let mut tracker = Tracker::default();
        let totals: Vec<i64> = scores
            .iter()
            .flat_map(|e| tracker.event(&e.kind, &e.data, 13).unwrap())
            .filter_map(|r| match r.event {
                Event::ObjectiveScore { game_type, player: Some(0), score, .. } if game_type == "firefight" => {
                    Some(score)
                }
                _ => None,
            })
            .collect();
        assert_eq!(totals, [100, 150]);
    }

    #[test]
    fn offline_film_keeps_each_sample_and_closes_without_an_engine_end_hook() {
        let log = Arc::new(CaptureLog::default());
        let mut registry = ::hooks::Registry::new(::hooks::Switches::defaults());
        let telemetry = install_film(
            &mut registry,
            &TelemetryConfig::default(),
            Some(Box::new(log.clone())),
            live_proto::film::TelemetryHooks::UNIFIED,
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
