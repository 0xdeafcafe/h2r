//! Engine commands are sampled at the simulation-update envelope. A tick is
//! committed when GAME_TICK returns: its updates go to the film file and its
//! observations to the stats container beside it.
use super::{Mode, Recording, Session};
use anyhow::{Context, Result};
use film::{
    Header,
    blam::{Event, SeatInput, Summary, Tick},
};
use std::{fs::File, path::PathBuf};

/// One game's film being written: `<id>.h2film`, the film file, and
/// `<id>.h2stats`, its stats container (docs/game/film-format.md).
pub(super) struct GameFiles {
    /// The film file; the stats container is [`film::file::stats_of`] it.
    pub(super) path: PathBuf,
    /// The film's header: its ID and its start.
    pub(super) header: Header,
    pub(super) stats: film::Writer<File>,
    pub(super) film: film::file::FilmWriter<File>,
    pub(super) track: film::blam::TrackWriter,
    /// The session tick before the game's first update: the film's tick n
    /// is the session's `base + n`. `None` until the first update; a named
    /// recording, open from boot, has zero.
    pub(super) base: Option<u64>,
}

fn private_new(path: &std::path::Path) -> Result<File> {
    let mut options = File::options();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).with_context(|| path.display().to_string())
}

impl GameFiles {
    /// Create the film file at `path` and its stats container, both new.
    pub(super) fn create(path: PathBuf, header: Header, base: Option<u64>) -> Result<Self> {
        let stats_path = film::file::stats_of(&path);
        let stats = private_new(&stats_path)
            .and_then(|file| film::Writer::new(file, &header));
        let stats = match stats {
            Ok(stats) => stats,
            Err(error) => {
                let _ = std::fs::remove_file(&stats_path);
                return Err(error);
            }
        };
        let seed = film::file::Seed { header: header.clone(), descriptor: Default::default() };
        // The hold keeps the game in progress out of the library until the
        // film is sealed and closed, or the process ends.
        let film = private_new(&path).and_then(|file| {
            film::file::hold(&file)?;
            film::file::FilmWriter::new(file, &seed, 30)
        });
        let film = match film {
            Ok(film) => film,
            Err(error) => {
                drop(stats);
                let _ = std::fs::remove_file(&stats_path);
                let _ = std::fs::remove_file(&path);
                return Err(error);
            }
        };
        Ok(Self {
            path,
            header,
            stats,
            film,
            track: film::blam::TrackWriter::default().without_inputs(),
            base,
        })
    }

    /// Close and delete both files: a game with no film.
    pub(super) fn discard(self) {
        let path = self.path.clone();
        drop(self);
        for file in [film::file::stats_of(&path), path] {
            match std::fs::remove_file(&file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    eprintln!("theater: could not delete {}: {e}", file.display());
                }
                _ => {}
            }
        }
    }
}

/// The film's final header: its length, and the game's map, variant and
/// scoreboard when the game was described.
fn final_header(mut header: Header, summary: &Summary, tick: u64) -> Header {
    header.meta.length_ticks = tick;
    if !summary.level.is_empty() {
        header.meta.map.clone_from(&summary.level);
        header.meta.variant.clone_from(&summary.variant_name);
        header.meta.players = summary
            .players
            .iter()
            .map(|player| film::Player {
                name: player.name.clone(),
                team: player.team,
                score: player.score,
            })
            .collect();
    }
    header
}

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
pub(super) const MAX_UPDATES: usize = 16;

#[derive(Default)]
pub(super) struct Capture {
    /// The updates applied in the tick in progress, one for each
    /// application, in order.
    pub(super) updates: Vec<Update>,
    /// The tick stamp of the update applied last in this game.
    pub(super) last_stamp: Option<u32>,
    /// An update's stamp skipped, or a change the film cannot replay was
    /// made: the film ends at the tick before it, and nothing after is kept.
    pub(super) stamp_skipped: bool,
    /// The records features gave the game tick in progress
    /// ([`super::extension`]), in the order applied.
    pub(super) extensions: Vec<(film::Tag, Vec<u8>)>,
    pub(super) events: Vec<Event>,
    paused: bool,
}

impl Capture {
    /// After a game's film is sealed: the next game's stamps start afresh.
    pub(super) fn next_game(&mut self) {
        self.last_stamp = None;
        self.stamp_skipped = false;
        self.extensions.clear();
    }
}

impl Recording {
    /// Whether a game is being recorded (docs/game/film-format.md,
    /// "Recording every game"). The descriptor's match start appears when
    /// a game starts and is cleared when its film is sealed or it is left.
    pub(super) fn in_game(&self) -> bool {
        self.meta.descriptor().match_start_tick.is_some()
    }

    /// Forget the game just sealed or dropped: the next one is described
    /// from its activation.
    pub(super) fn next_game(&mut self) {
        self.meta.next_game();
        self.starts.next_game();
        self.unified.next_game();
        self.frame.next_game();
        self.frame.events.clear();
        self.described = None;
    }

    /// Seal `files` at film tick `tick`: the game's closing observations,
    /// its final header (from `header`, or the film's own) and its
    /// scoreboard, then the film file's trailer. Returns the film file's
    /// path and the final header.
    /// Seal `files` at film tick `tick`. A client's game also takes its
    /// machine tape (`super::machine`), and may hold no tick: what it
    /// observed before a first tick has no tick to close, and is dropped.
    pub(super) fn seal(
        &mut self,
        mut files: GameFiles,
        header: Option<Header>,
        tape: Option<film::tape::Tape>,
        tick: u64,
    ) -> Result<(PathBuf, Header)> {
        let sealed = (|| -> Result<Header> {
            let events = std::mem::take(&mut self.frame.events);
            let records = self.unified.take_records(false, false)?;
            if files.track.ticks() > 0 || tape.is_none() {
                files.track.closing_events(events)?;
                files.track.append_to_last(records)?;
            }
            if let Some(tape) = &tape {
                files.film.tape(tape)?;
            }
            self.meta.finish_map()?;
            let mut summary = self.meta.summary(tick);
            self.unified.summary_medals(&mut summary.players);
            summary.dropped_samples = self.unified.drop_counts();
            let header = final_header(header.unwrap_or_else(|| files.header.clone()), &summary, tick);
            files.stats.chunk(film::HEAD, &header)?;
            files.track.finish(&mut files.stats, self.meta.descriptor(), &summary)?;
            Ok(header)
        })();
        let header = match sealed {
            Ok(header) => header,
            Err(error) => {
                files.discard();
                return Err(error);
            }
        };
        let GameFiles { path, stats: stats_out, film: film_out, .. } = files;
        let closed = (|| -> Result<()> {
            stats_out.finish()?.sync_all()?;
            film_out.finish()?.sync_all()?;
            Ok(())
        })();
        if let Err(error) = closed {
            for file in [film::file::stats_of(&path), path.clone()] {
                let _ = std::fs::remove_file(file);
            }
            return Err(error);
        }
        Ok((path, header))
    }

    /// Write the game as it stands into the public front of its stats
    /// container: its roster, map and scoreboard so far. The seal writes the
    /// final front; this one is what a crash or kill leaves.
    pub(super) fn refresh_front(&mut self, session_tick: u64) -> Result<()> {
        let Some(game) = self.game.as_mut() else { return Ok(()) };
        let tick = session_tick.saturating_sub(game.base.unwrap_or(0));
        let mut summary = self.meta.summary(tick);
        if summary.level.is_empty() {
            return Ok(());
        }
        self.unified.summary_medals(&mut summary.players);
        game.stats.refresh_front(self.meta.descriptor(), &summary)
    }

    pub(super) fn post_tick(
        &mut self,
        guest: &runtime::Guest,
        session_tick: u64,
        post_hash: Option<u64>,
    ) -> Result<()> {
        // The film's tick: tick 1 is the game's first update.
        let game_tick = self
            .game
            .as_ref()
            .and_then(|game| game.base)
            .map(|base| session_tick - base);
        let mut events = std::mem::take(&mut self.frame.events);
        events.extend(
            self.meta
                .observe(guest, game_tick.unwrap_or(0), |source| self.unified.dropped(source))?,
        );
        if self.meta.take_left() {
            // The left game's film is sealed at the next slice's check, as
            // a game's end is (docs/game/film-format.md, "Recording every
            // game").
            self.match_over = true;
        }
        let updates = std::mem::take(&mut self.frame.updates);
        let skipped = self.frame.stamp_skipped;
        let Some(tick) = game_tick else {
            // Before the game's first update nothing is the film's: what
            // the observer saw of the game waits for its first tick.
            if self.game.is_some() {
                self.frame.events = events;
            }
            return Ok(());
        };
        let game = self.game.as_mut().context("a game's first update without its film")?;
        // The stats track takes one batch for a tick: the tick's first
        // update's. The film file has every one.
        let inputs = updates
            .first()
            .map(|update| update.inputs.clone())
            .unwrap_or_default();
        if !skipped {
            use film::file::{RECORD_JOIN, RECORD_LEAVE, TICK_DIGEST, TICK_EVENTS};
            let out = &mut game.film;
            // The game's start sits just before its first update's tick.
            match self.starts.release(&mut self.meta) {
                Ok(Some(start)) => {
                    if let Err(error) = out.game_start(&start) {
                        eprintln!("theater: this game has no start in its film: {error:#}");
                    }
                }
                Ok(None) => {}
                Err(error) => eprintln!("theater: this game has no start in its film: {error:#}"),
            }
            // The records features gave this game tick come before its first
            // film tick. One that cannot be kept ends the film at the tick
            // before: the game's change has no record.
            let mut outside = std::mem::take(&mut self.frame.extensions);
            outside.extend(super::extension::outside());
            let first_update = u32::try_from(out.ticks() + 1).unwrap_or(u32::MAX);
            let mut cut = false;
            for (tag, bytes) in outside {
                if let Err(error) = out.extension(first_update, tag, &bytes) {
                    eprintln!(
                        "theater: a record of {} cannot be kept: {error:#}; this game's film ends at the tick before",
                        String::from_utf8_lossy(&tag)
                    );
                    self.frame.stamp_skipped = true;
                    self.match_over = true;
                    cut = true;
                    break;
                }
            }
            // One film tick for each application of the update; a tick that
            // applied none is one with no actions. The tick's digest and
            // events are those of its end, so they sit on its last.
            let ticks = if cut { 0 } else { updates.len().max(1) };
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
                out.push_tick(flags, digest, &seats)?;
                if let Some(body) = update.and_then(|update| update.body.as_deref()) {
                    out.tail(body)?;
                }
                if !last {
                    continue;
                }
                for event in &events {
                    match event {
                        Event::Join { player } => out.seat_record(
                            player.seat,
                            RECORD_JOIN,
                            &[player.seat, player.kind as u8],
                        )?,
                        Event::Leave { seat } | Event::Quit { seat } => {
                            out.seat_record(*seat, RECORD_LEAVE, &[*seat])?
                        }
                        _ => {}
                    }
                }
            }
            if tick.is_multiple_of(CHECKPOINT_TICKS) {
                out.checkpoint()?;
            }
        }
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
            .take_records(game.track.begins_chunk(), game.track.needs_pose_baseline())?;
        game.track.push_clocked(
            &mut game.stats,
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
    /// An update with no body, as a test makes one.
    #[cfg(test)]
    pub(super) fn record_actions(&self, inputs: Vec<SeatInput>) {
        self.record_update(inputs, None, None);
    }

    /// The simulation update was applied: keep its actions and body for
    /// the tick in progress. A tick can apply it more than once (a client
    /// catching up after a stall), and each application is its own record,
    /// in order. An update whose stamp is further on than the next tick
    /// means a tick the film does not hold: the game's film is sealed at
    /// the tick before it (`finish` seals it, as at the game's end).
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
