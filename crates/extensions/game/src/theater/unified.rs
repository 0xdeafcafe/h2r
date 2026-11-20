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
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};

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
    /// Reserve room for the score stream regardless of a burst of visual effects.
    stats_bytes: usize,
    highlight_bytes: usize,
    drops: BTreeMap<&'static str, u64>,
    clock: Option<u64>,
    final_clock: Option<u64>,
    ended: bool,
}

impl Capture {
    pub(super) fn summary_medals(&self, rows: &mut Vec<live_proto::film::PlayerSummary>) {
        self.stats.summary_medals(rows);
    }

    pub(super) fn dropped(&mut self, source: &'static str) {
        self.count_drops(source, 1);
    }

    fn count_drops(&mut self, source: &'static str, n: u64) {
        if n == 0 {
            return;
        }
        let count = self.drops.entry(source).or_default();
        if *count == 0 {
            eprintln!("theater: dropped {source} observation (counts in film summary)");
        }
        *count = count.saturating_add(n);
    }

    pub(super) fn drop_counts(&self) -> BTreeMap<String, u64> {
        self.drops
            .iter()
            .map(|(&source, &count)| (source.to_owned(), count))
            .collect()
    }

    fn records(&mut self, records: impl IntoIterator<Item = events::Record>) {
        // A chunk rolls at 4 MiB and can grow to 8 MiB. Bound each tick
        // independently, including the separate batch after the final tick.
        const BUDGET: usize = live_proto::film::stream::MAX_BYTES / 16;
        for record in records {
            let canonical = matches!(
                record.event,
                events::Event::GameStarted(_)
                    | events::Event::PlayerJoined(_)
                    | events::Event::PlayerLeft(_)
                    | events::Event::PlayerKilled(_)
                    | events::Event::ScoreChanged(_)
                    | events::Event::GameEnded(_)
            );
            let (source, used) = if canonical {
                ("stats_budget", &mut self.stats_bytes)
            } else {
                ("highlight_budget", &mut self.highlight_bytes)
            };
            if *used >= BUDGET {
                self.dropped(source);
                continue;
            }
            let Ok(bytes) = record.encode() else {
                self.dropped(if canonical {
                    "stats_encoding"
                } else {
                    "highlight_encoding"
                });
                continue;
            };
            let size = bytes.len() + 13; // subtype and worst-case stream envelope
            if size > BUDGET - *used {
                self.dropped(source);
                continue;
            }
            *used += size;
            self.pending
                .push(Pending::Record(RecordData::stats_event(bytes)));
        }
    }

    pub(super) fn marker(&mut self, map: &str) {
        self.pending.push(Pending::Record(RecordData::marker(map)));
    }

    pub(super) fn tick_clock(&mut self) -> Option<u64> {
        self.clock.or(self.final_clock.take())
    }

    pub(super) fn objectives(&mut self, guest: &runtime::Guest, clock: u64) -> Result<()> {
        // A successful pickup hook belongs to this tick even if reading
        // the objective registry fails during the same simulation boundary.
        let mut changes = self.objectives.take_pickups();
        match self.objectives.observe(guest) {
            Ok(sample) => changes.extend(sample),
            Err(_) => self.dropped("objective"),
        }
        let records = changes
            .into_iter()
            .map(|event| events::Record { tick: clock, event });
        self.records(records);
        Ok(())
    }

    pub(super) fn take_records(&mut self, starts_chunk: bool, baseline: bool) -> Result<Vec<RecordData>> {
        let mut out = Vec::new();
        if starts_chunk {
            self.presentation.begin_chunk();
        }
        if baseline {
            let (checkpoint, dropped) = self.presentation.baseline_observed()?;
            self.count_drops("frame", u64::from(dropped));
            out.push(RecordData::pose(checkpoint));
        }
        let mut pending = std::mem::take(&mut self.pending);
        for item in pending.drain(..) {
            let (source, result) = match item {
                Pending::Record(record) => {
                    out.push(record);
                    continue;
                }
                Pending::Scene(scene) => match self.presentation.push_observed(&scene) {
                    Ok((record, drops)) => {
                        self.count_drops("object_pose", drops.objects);
                        self.count_drops("hill", u64::from(drops.hill));
                        self.count_drops("objective_registry", u64::from(drops.objectives));
                        ("scene_census", Ok(record))
                    }
                    Err(error) => ("scene_census", Err(error)),
                },
                Pending::Frame(frame) => match self.presentation.frame_observed(&frame) {
                    Ok((record, dropped)) => {
                        self.count_drops("unit_frame", dropped);
                        ("frame", Ok(record))
                    }
                    Err(error) => ("frame", Err(error)),
                },
            };
            match result {
                Ok(Some(bytes)) => out.push(RecordData::pose(bytes)),
                Ok(None) => {}
                Err(_) => self.dropped(source),
            }
        }
        self.pending = pending;
        self.stats_bytes = 0;
        self.highlight_bytes = 0;
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

    fn dropped(&self, source: &'static str) {
        let _ = self.write(|rec, _| {
            rec.unified.dropped(source);
            Ok(())
        });
    }

    fn event(&self, event: &GameEvent, clock: u64) -> Result<(), String> {
        self.write(|rec, game_id| {
            let mut records = match rec.unified.stats.event(&event.kind, &event.data, clock) {
                Ok(records) => records,
                Err(_) => {
                    rec.unified.dropped("stats_event");
                    return Ok(());
                }
            };
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
            rec.unified.records(records);
            Ok(())
        })
    }

    fn record(&self, record: events::Record) -> Result<(), String> {
        self.write(|rec, _| {
            rec.unified.records([record]);
            Ok(())
        })
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
            rec.unified.records(events);
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
    pub fn has_unified_track(&self) -> bool {
        self.version >= 3
    }

    pub fn film_observer(self: &Arc<Self>) -> Option<Box<dyn FilmObserver>> {
        (!self.replaying && self.has_unified_track())
            .then(|| Box::new(Observer(Arc::downgrade(self))) as Box<dyn FilmObserver>)
    }
}
