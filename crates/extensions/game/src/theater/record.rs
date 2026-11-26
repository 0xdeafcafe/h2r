//! Engine commands are sampled at the simulation-update envelope. A tick is
//! committed when GAME_TICK returns: its updates go to the film file and its
//! observations to the stats stream.
use super::{Mode, Recording, Session};
use anyhow::{Context, Result};
use film::blam::{Event, SeatInput, Tick};

/// Ticks between the film file's checkpoints: a recording that dies keeps
/// its ticks to the last one (`film::file::FilmWriter::checkpoint`).
const CHECKPOINT_TICKS: u64 = 150;

/// One application of the simulation update: the seats' actions and the
/// body the film stores beside them (`actions.rs`).
pub(super) struct Update {
    pub(super) inputs: Vec<SeatInput>,
    /// The update's bytes between its action block and its stamp, or `None`
    /// for an update that holds the one before's.
    pub(super) body: Option<Vec<u8>>,
}

/// The most simulation updates one tick can apply and the film keep.
const MAX_UPDATES: usize = 16;

#[derive(Default)]
pub(super) struct Capture {
    /// The updates applied in the tick in progress, one for each
    /// application, in order.
    pub(super) updates: Vec<Update>,
    /// The tick stamp of the update applied last in this game.
    pub(super) last_stamp: Option<u32>,
    /// An update's stamp skipped: the film ends at the tick before it, and
    /// nothing after is kept.
    pub(super) stamp_skipped: bool,
    pub(super) events: Vec<Event>,
    paused: bool,
}

impl Capture {
    /// After a game's film is cut: the next game's stamps start afresh.
    pub(super) fn next_game(&mut self) {
        self.last_stamp = None;
        self.stamp_skipped = false;
    }
}

impl Recording {
    /// Whether a game is on the tape (docs/game/film-format.md, "Recording
    /// every game"). The descriptor's match start appears when a game
    /// starts and is cleared when its film is cut or it is left.
    pub(super) fn in_game(&self) -> bool {
        self.meta.descriptor().match_start_tick.is_some()
    }

    pub(super) fn post_tick(
        &mut self,
        guest: &runtime::Guest,
        tick: u64,
        post_hash: Option<u64>,
    ) -> Result<()> {
        let mut events = std::mem::take(&mut self.frame.events);
        events.extend(
            self.meta
                .observe(guest, tick, |source| self.unified.dropped(source))?,
        );
        if self.meta.take_cut_left() {
            // The left game's film is cut at the next slice's check, as a
            // game's end is (docs/game/film-format.md, "One film per game").
            self.match_over = true;
        }
        let updates = std::mem::take(&mut self.frame.updates);
        let skipped = self.frame.stamp_skipped;
        // The machine tape takes one batch for a tick: the tick's first
        // update's. The film file has every one.
        let inputs = updates
            .first()
            .map(|update| update.inputs.clone())
            .unwrap_or_default();
        if let Some(film) = &self.film
            && !skipped
        {
            use film::file::{RECORD_JOIN, RECORD_LEAVE, TICK_DIGEST, TICK_EVENTS};
            let mut film = film.lock().unwrap_or_else(|e| e.into_inner());
            // A game's start sits just before its first update's tick.
            match self.starts.release(&mut self.meta) {
                Ok(Some(start)) => {
                    if let Err(error) = film.game_start(&start) {
                        eprintln!("theater: this game has no start in its film: {error:#}");
                    }
                }
                Ok(None) => {}
                Err(error) => eprintln!("theater: this game has no start in its film: {error:#}"),
            }
            // One film tick for each application of the update; a tick that
            // applied none is one with no actions. The tick's digest and
            // events are those of its end, so they sit on its last.
            let ticks = updates.len().max(1);
            for index in 0..ticks {
                let last = index + 1 == ticks;
                let update = updates.get(index);
                let mut seats = [None; film::columns::SEATS];
                if let Some(update) = update {
                    for input in &update.inputs {
                        if let Some(slot) = seats.get_mut(usize::from(input.seat)) {
                            *slot = input.action;
                        }
                    }
                }
                let flags = if last && post_hash.is_some() { TICK_DIGEST } else { 0 }
                    | if last && !events.is_empty() { TICK_EVENTS } else { 0 };
                let digest = if last { post_hash.unwrap_or(0) } else { 0 };
                film.push_tick(flags, digest, &seats)?;
                if let Some(body) = update.and_then(|update| update.body.as_deref()) {
                    film.tail(body)?;
                }
                if !last {
                    continue;
                }
                for event in &events {
                    match event {
                        Event::Join { player } => film.seat_record(
                            player.seat,
                            RECORD_JOIN,
                            &[player.seat, player.kind as u8],
                        )?,
                        Event::Leave { seat } | Event::Quit { seat } => {
                            film.seat_record(*seat, RECORD_LEAVE, &[*seat])?
                        }
                        _ => {}
                    }
                }
            }
            if tick.is_multiple_of(CHECKPOINT_TICKS) {
                film.checkpoint()?;
            }
        }
        let out = self.out.as_mut().context("engine tape is closed")?;
        let match_start = self.meta.descriptor().match_start_tick;
        if match_start == Some(tick) {
            self.unified.marker(&self.meta.descriptor().map.name);
        }
        let clock = self.unified.tick_clock();
        if let Some(clock) = clock {
            self.unified.objectives(guest, clock)?;
        }
        let records = self
            .unified
            .take_records(self.track.begins_chunk(), self.track.needs_pose_baseline())?;
        self.track.push_clocked(
            out,
            Tick {
                tick,
                inputs,
                events,
                closing_events: Vec::new(),
                post_hash,
                forge: Vec::new(),
                // The Firefight host log's entries applied this tick.
                firefight: crate::firefight::film::take(),
            },
            records,
            clock,
        )?;
        Ok(())
    }
}

impl Session {
    /// An update with no body, as a test or a machine tape makes one.
    pub(super) fn record_actions(&self, inputs: Vec<SeatInput>) {
        self.record_update(inputs, None, None);
    }

    /// The simulation update was applied: keep its actions and body for
    /// the tick in progress. A tick can apply it more than once (a client
    /// catching up after a stall), and each application is its own record,
    /// in order. An update whose stamp is further on than the next tick
    /// means a tick the film does not hold: the game's film is cut at the
    /// tick before it (`finish` cuts it, as at the game's end).
    pub(super) fn record_update(&self, inputs: Vec<SeatInput>, body: Option<Vec<u8>>, stamp: Option<u32>) {
        let mut s = self.lock();
        if s.complete || s.failure.is_some() {
            return;
        }
        let tick = s.tick + 1;
        let Mode::Record(r) = &mut s.mode else {
            return;
        };
        if let Some(stamp) = stamp {
            let last = r.frame.last_stamp.replace(stamp);
            if !super::actions::stamp_follows(last, stamp) && !r.frame.stamp_skipped {
                eprintln!(
                    "theater: the update at tick {tick} is stamped {stamp} after {}: this game's film ends at the tick before",
                    last.unwrap_or_default()
                );
                r.frame.stamp_skipped = true;
                r.match_over = true;
            }
        }
        if r.frame.stamp_skipped {
            return;
        }
        if r.frame.updates.len() >= MAX_UPDATES {
            s.fail_at(
                tick,
                format!("more than {MAX_UPDATES} simulation updates in one tick"),
            );
            return;
        }
        r.frame.updates.push(Update { inputs, body });
    }

    /// The campaign timer stops ticking while paused. Sample the pause
    /// latch between scheduler slices so both edges reach the resumed tick.
    pub fn observe_pause(&self, guest: &runtime::Guest) {
        let Ok(time) = guest.u32(crate::telemetry::layout::GAME_TIME_PTR) else {
            return;
        };
        if time == 0 || guest.u8(time).ok() != Some(1) {
            return;
        }
        let Some(paused) = time.checked_add(1).and_then(|p| guest.u8(p).ok()) else {
            return;
        };
        let mut s = self.lock();
        if s.complete || s.failure.is_some() {
            return;
        }
        let paused = paused != 0;
        if let Mode::Record(r) = &mut s.mode
            && r.frame.paused != paused
        {
            r.frame.events.push(Event::Pause { paused });
            r.frame.paused = paused;
        }
    }
}
