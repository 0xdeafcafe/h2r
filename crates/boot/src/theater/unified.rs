//! Game-thread observations waiting for the next completed simulation tick.
//! The final scoreboard (or an interrupted game's quit) can follow the last
//! GAME_TICK. In that case the recorder appends it to that tick before sealing.
use super::{Mode, Recording, Session, objectives};
use crate::telemetry::{FilmObserver, GameEvent};
use anyhow::Result;
use film::blam::RecordData;
use live_proto::{
    film::{events, objects::Scene},
    game::Frame,
};
use std::sync::{Arc, Weak};

enum Pending {
    Record(RecordData),
    Scene(Scene),
    Frame(Frame),
}

#[derive(Default)]
pub(super) struct Capture {
    stats: events::Tracker,
    presentation: film::presentation::Encoder,
    pub(super) objectives: objectives::Tracker,
    pending: Vec<Pending>,
    clock: Option<u64>,
    final_clock: Option<u64>,
    ended: bool,
}

impl Capture {
    fn records(&mut self, records: Vec<events::Record>) -> Result<(), String> {
        for record in records {
            self.pending
                .push(Pending::Record(RecordData::stats_event(record.encode()?)));
        }
        Ok(())
    }

    pub(super) fn marker(&mut self, map: &str) {
        self.pending.push(Pending::Record(RecordData::marker(map)));
    }

    pub(super) fn tick_clock(&mut self) -> Option<u64> {
        self.clock.or(self.final_clock.take())
    }

    pub(super) fn objectives(&mut self, guest: &runtime::Guest, clock: u64) -> Result<()> {
        let changes = self.objectives.observe(guest).map_err(anyhow::Error::msg)?;
        let records = changes
            .into_iter()
            .map(|event| events::Record { tick: clock, event })
            .collect();
        self.records(records).map_err(anyhow::Error::msg)
    }

    pub(super) fn take_records(&mut self, baseline: bool) -> Result<Vec<RecordData>> {
        let mut out = Vec::new();
        if baseline {
            out.push(RecordData::pose(self.presentation.baseline()?));
        }
        for item in self.pending.drain(..) {
            let pose = match item {
                Pending::Record(record) => {
                    out.push(record);
                    continue;
                }
                Pending::Scene(scene) => self.presentation.push(&scene)?,
                Pending::Frame(frame) => self.presentation.frame(&frame)?,
            };
            if let Some(bytes) = pose {
                out.push(RecordData::pose(bytes));
            }
        }
        Ok(out)
    }
}

pub(super) struct Observer(Weak<Session>);

impl Observer {
    fn write(
        &self,
        action: impl FnOnce(&mut Recording, Option<&str>) -> Result<(), String>,
    ) -> Result<(), String> {
        let Some(film) = self.0.upgrade() else {
            return Ok(());
        };
        let mut s = film.lock();
        if s.complete || s.failure.is_some() {
            return Ok(());
        }
        let super::State {
            mode: Mode::Record(rec),
            header,
            ..
        } = &mut *s
        else {
            return Ok(());
        };
        // A scenario change can close this game and announce the next game
        // in one telemetry batch, before the scheduler can seal the film.
        if rec.unified.ended {
            return Ok(());
        }
        action(rec, header.game_id.as_deref())
    }
}

impl FilmObserver for Observer {
    fn active(&self) -> bool {
        self.0.upgrade().is_some_and(|film| {
            let s = film.lock();
            !s.complete
                && s.failure.is_none()
                && matches!(&s.mode, Mode::Record(r) if !r.unified.ended)
        })
    }

    fn failed(&self, reason: &str) {
        if let Some(film) = self.0.upgrade() {
            film.lock().fail(format!("capturing film events: {reason}"));
        }
    }

    fn event(&self, event: &GameEvent, clock: u64) -> Result<(), String> {
        self.write(|rec, game_id| {
            let mut records = rec.unified.stats.event(&event.kind, &event.data, clock)?;
            for record in &mut records {
                if let events::Event::GameStarted(start) = &mut record.event {
                    if let Some(id) = game_id {
                        start.game_id = id.to_owned();
                    }
                    rec.unified.objectives.start_match();
                } else if let events::Event::GameEnded(end) = &record.event {
                    // An orderly disconnect may close the authoritative game
                    // without setting the retail GAME_ENDED latch.
                    rec.match_over = true;
                    rec.unified.ended = true;
                    rec.unified.clock = None;
                    rec.unified.final_clock = Some(end.tick);
                }
            }
            rec.unified.records(records)
        })
    }

    fn record(&self, record: events::Record) -> Result<(), String> {
        self.write(|rec, _| rec.unified.records(vec![record]))
    }

    fn clock(&self, clock: Option<u64>) {
        let _ = self.write(|rec, _| {
            rec.unified.clock = clock;
            Ok(())
        });
    }

    fn frame(&self, frame: Frame) -> Result<(), String> {
        self.write(|rec, _| {
            // The direct engine hooks already recorded the individual shots
            // and hits. Only changes requiring sampled state go in here.
            let events = rec.unified.stats.frame_changes(&frame);
            rec.unified.records(events)?;
            rec.unified.pending.push(Pending::Frame(frame));
            Ok(())
        })
    }

    fn scene(&self, scene: Scene) -> Result<(), String> {
        self.write(|rec, _| {
            rec.unified.pending.push(Pending::Scene(scene));
            Ok(())
        })
    }
}

impl Session {
    pub(crate) fn has_unified_track(&self) -> bool {
        self.version >= 3
    }

    pub(crate) fn film_observer(self: &Arc<Self>) -> Option<Box<dyn FilmObserver>> {
        (!self.replaying && self.has_unified_track())
            .then(|| Box::new(Observer(Arc::downgrade(self))) as Box<dyn FilmObserver>)
    }
}
