//! The session ring (docs/game/theater-design.md, "The session ring"):
//! snapshots of the film being played, taken as the player passes them, so
//! a seek behind the tick shown loads the latest one at or before its
//! target and runs forward from there instead of queueing the film's game
//! again.
//!
//! - **An entry** is the state at the end of a film tick: the game-state
//!   arena (zstd), the `SimExt` records and the arena hash of both
//!   (`start_point_state::capture`), the game ticks the film's game had
//!   ended by then, and the Forge monitors read so far
//!   (`forge_film::ring`). The film cursor is the tick itself: the film
//!   file's reader (`film::file::FilmActions`) keeps the decoder state of
//!   every chunk it has decoded and reads any tick the player has passed.
//! - **When.** One entry for each stretch of [`INTERVAL`] film ticks, at the
//!   first tick a game tick ends on in it; the first stretch has none, as
//!   the film's start is a map load away. A stretch passed again keeps its
//!   entry.
//! - **Cap.** The entries' stored bytes stay within the cap
//!   (`H2_THEATER_RING_MIB`, [`DEFAULT_CAP_MIB`]); past it, every second
//!   entry of the older half goes.
//! - **Load.** At the start of a frame, before its ticks (`pacing.rs`), the
//!   entry's records and arena are written back and the arena hashed; the
//!   hash must be the entry's. The film's cursor moves to the entry's tick
//!   (`Play::resume_at`) and the player runs forward to the target. No
//!   engine step runs after the write (slice 8's run says whether one is
//!   needed): the digests decide.
//! - **Failure.** An entry that does not load, or whose state's digests
//!   differ from the film's before the target or the next stretch's tick,
//!   whichever is later, drops the ring, which stays off for the film; the
//!   jump takes the start route.
//!
//! The ring is the film's: it goes with the film's state. `H2_THEATER_RING=0`
//! turns it off.
use super::start_point::StartPoint;
use super::start_point_state::{self as arena, ExtRecords, Global};
use super::{SeekProgress, Session, State, forge_film};
use agent_proto::theater as api;
use guest_layout::GuestMemory;
use std::time::Instant;

/// Film ticks between entries.
pub(super) const INTERVAL: u64 = 150;
/// The ring's byte cap without `H2_THEATER_RING_MIB`.
pub(super) const DEFAULT_CAP_MIB: u64 = 256;
/// zstd's level for an entry's arena: the fastest, as an entry is taken on
/// the game's thread.
const LEVEL: i32 = 1;

/// The state an entry keeps outside the arena. The process's is
/// [`Process`]; tests use a fake, so no test touches another's statics.
pub(super) trait Outside: Send {
    fn ext(&mut self) -> &mut dyn ExtRecords;
    fn monitors(&self) -> forge_film::ring::Kept;
    fn put_monitors(&mut self, kept: &forge_film::ring::Kept);
}

/// The process's `SimExt` records and Forge monitors.
struct Process(Global);

impl Outside for Process {
    fn ext(&mut self) -> &mut dyn ExtRecords {
        &mut self.0
    }
    fn monitors(&self) -> forge_film::ring::Kept {
        forge_film::ring::snapshot()
    }
    fn put_monitors(&mut self, kept: &forge_film::ring::Kept) {
        forge_film::ring::restore(kept);
    }
}

struct Entry {
    /// The film tick whose end the state is.
    tick: u64,
    /// The game ticks the film's game had ended by then.
    game_ticks: u64,
    arena_hash: u64,
    /// The arena, zstd.
    arena: Vec<u8>,
    arena_len: usize,
    ext: Vec<u8>,
    monitors: forge_film::ring::Kept,
}

impl Entry {
    fn bytes(&self) -> usize {
        self.arena.len() + self.ext.len()
    }
}

/// A jump the ring serves: its target, the tick it left and the route the
/// caller asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Jump {
    pub(super) target: u64,
    pub(super) from: u64,
    pub(super) requested: Option<api::SeekRoute>,
}

/// An entry loaded into the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Loaded {
    pub(super) tick: u64,
    pub(super) game_ticks: u64,
}

pub(super) struct Ring {
    /// Oldest first.
    entries: Vec<Entry>,
    bytes: usize,
    cap: usize,
    /// Film ticks between entries: [`INTERVAL`].
    interval: u64,
    /// Why the ring is off for this film.
    off: Option<String>,
    /// A jump whose entry loads at the next frame's start.
    pending: Option<Jump>,
    /// The last load's jump and the last tick whose digest answers for it.
    checking: Option<(Jump, u64)>,
    /// How long the last load took, in milliseconds rounded up.
    restore_ms: Option<u64>,
    outside: Box<dyn Outside>,
}

impl Ring {
    /// The ring a film opens with, as the environment sets it; off with
    /// `H2_THEATER_RING=0`.
    pub(super) fn from_env() -> Self {
        Self::configured(|key| std::env::var(key).ok(), Box::new(Process(Global)))
    }

    fn configured(env: impl Fn(&str) -> Option<String>, outside: Box<dyn Outside>) -> Self {
        let mib = env("H2_THEATER_RING_MIB")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_CAP_MIB);
        let bytes = mib.saturating_mul(1 << 20);
        let mut ring = Self::new(usize::try_from(bytes).unwrap_or(usize::MAX), outside);
        if env("H2_THEATER_RING").as_deref() == Some("0") {
            ring.off = Some("H2_THEATER_RING=0".into());
        }
        ring
    }

    pub(super) fn new(cap: usize, outside: Box<dyn Outside>) -> Self {
        Self {
            entries: Vec::new(),
            bytes: 0,
            cap,
            interval: INTERVAL,
            off: None,
            pending: None,
            checking: None,
            restore_ms: None,
            outside,
        }
    }

    /// Drop every entry and keep the ring off for this film.
    pub(super) fn turn_off(&mut self, why: String) {
        eprintln!("theater: the session ring is off for this film: {why}");
        self.entries.clear();
        self.bytes = 0;
        self.pending = None;
        self.checking = None;
        self.off = Some(why);
    }

    /// The tick of the entry a jump to `target` loads: the latest at or
    /// before it.
    pub(super) fn nearest(&self, target: u64) -> Option<u64> {
        if self.off.is_some() {
            return None;
        }
        self.entries.iter().rev().find(|e| e.tick <= target).map(|e| e.tick)
    }

    /// A ring that takes an entry every `ticks` film ticks, so a test's
    /// film can be short.
    #[cfg(test)]
    pub(super) fn every(mut self, ticks: u64) -> Self {
        self.interval = ticks.max(1);
        self
    }

    fn due(&self, tick: u64) -> bool {
        let every = self.interval;
        self.off.is_none()
            && tick >= every
            && !self.entries.iter().any(|e| e.tick / every == tick / every)
    }

    /// A game tick ended on film tick `tick`, the film's game having ended
    /// `game_ticks`: take the stretch's entry if it has none. An entry that
    /// cannot be taken turns the ring off.
    pub(super) fn passed(&mut self, mem: &dyn GuestMemory, tick: u64, game_ticks: u64) {
        if self.checking.is_some_and(|(_, through)| tick > through) {
            self.checking = None;
        }
        if !self.due(tick) {
            return;
        }
        match self.capture(mem, tick, game_ticks) {
            Ok(entry) => self.insert(entry),
            Err(e) => self.turn_off(format!("taking the entry at tick {tick}: {e:#}")),
        }
    }

    fn capture(&mut self, mem: &dyn GuestMemory, tick: u64, game_ticks: u64) -> anyhow::Result<Entry> {
        let point = arena::capture(mem, self.outside.ext(), u32::try_from(tick)?, Vec::new())?;
        Ok(Entry {
            tick,
            game_ticks,
            arena_hash: point.arena_hash,
            arena_len: point.arena.len(),
            arena: zstd::bulk::compress(&point.arena, LEVEL)?,
            ext: point.ext,
            monitors: self.outside.monitors(),
        })
    }

    fn insert(&mut self, entry: Entry) {
        let at = self.entries.partition_point(|e| e.tick < entry.tick);
        self.bytes += entry.bytes();
        self.entries.insert(at, entry);
        self.thin();
    }

    /// Past the cap, every second entry of the older half goes, until the
    /// ring fits.
    fn thin(&mut self) {
        while self.bytes > self.cap && !self.entries.is_empty() {
            let older = self.entries.len() / 2;
            let before = self.entries.len();
            let mut index = 0;
            self.entries.retain(|_| {
                let keep = index >= older || index % 2 == 0;
                index += 1;
                keep
            });
            if self.entries.len() == before {
                // One or two entries: the oldest goes.
                self.entries.remove(0);
            }
            self.bytes = self.entries.iter().map(Entry::bytes).sum();
        }
    }

    /// A seek takes the ring route: its entry loads at the next frame's
    /// start.
    pub(super) fn ask(&mut self, jump: Jump) {
        self.pending = Some(jump);
        self.restore_ms = None;
    }

    /// A jump the ring was to serve is replaced or cancelled before its
    /// entry loaded.
    pub(super) fn cancel(&mut self) {
        self.pending = None;
    }

    /// The film starts over from its game start: no load is pending and no
    /// digest answers for one.
    pub(super) fn started_over(&mut self) {
        self.pending = None;
        self.checking = None;
    }

    pub(super) fn pending(&self) -> Option<Jump> {
        self.pending
    }

    fn take_pending(&mut self) -> Option<Jump> {
        self.pending.take()
    }

    /// Load the latest entry at or before the jump's target into the game:
    /// its records, its arena and its monitors. The loaded state's hash
    /// must be the entry's.
    pub(super) fn load(&mut self, jump: Jump, mem: &mut dyn GuestMemory) -> Result<Loaded, String> {
        if let Some(why) = &self.off {
            return Err(why.clone());
        }
        let started = Instant::now();
        let index = self
            .entries
            .iter()
            .rposition(|e| e.tick <= jump.target)
            .ok_or("the ring holds no entry at or before the target")?;
        let entry = &self.entries[index];
        let point = StartPoint {
            tick: u32::try_from(entry.tick).map_err(|e| e.to_string())?,
            arena_hash: entry.arena_hash,
            ids: Vec::new(),
            arena: zstd::bulk::decompress(&entry.arena, entry.arena_len)
                .map_err(|e| format!("the entry's arena does not inflate: {e}"))?,
            ext: entry.ext.clone(),
        };
        let after = arena::load(mem, self.outside.ext(), &point).map_err(|e| format!("{e:#}"))?;
        point.check_loaded(after).map_err(|e| format!("{e:#}"))?;
        self.outside.put_monitors(&entry.monitors);
        let loaded = Loaded { tick: entry.tick, game_ticks: entry.game_ticks };
        self.restore_ms = Some(started.elapsed().as_micros().div_ceil(1000) as u64);
        self.checking = Some((jump, jump.target.max(loaded.tick + self.interval)));
        Ok(loaded)
    }

    /// A digest failed at film tick `tick`: the jump to take by the start
    /// route when the failure answers for the last load, which turns the
    /// ring off.
    pub(super) fn diverged(&mut self, tick: u64, why: &str) -> Option<Jump> {
        let (jump, through) = self.checking?;
        if tick > through {
            return None;
        }
        self.turn_off(format!("the state loaded for the jump to tick {} diverged: {why}", jump.target));
        Some(jump)
    }

    pub(super) fn restore_ms(&self) -> Option<u64> {
        self.restore_ms
    }

    /// Change the arena hash the entry at `tick` holds, so its load fails
    /// its check.
    #[cfg(test)]
    pub(super) fn spoil(&mut self, tick: u64) {
        for entry in self.entries.iter_mut().filter(|e| e.tick == tick) {
            entry.arena_hash ^= 1;
        }
    }

    pub(super) fn keyframes(&self) -> api::Keyframes {
        api::Keyframes {
            ring: self.entries.iter().map(|e| e.tick).collect(),
            ring_bytes: self.bytes as u64,
            ring_cap: self.cap as u64,
            off: self.off.clone(),
        }
    }
}

impl State {
    /// The entry a seek to `target` would load: the ring's latest at or
    /// before it, when the film's game runs, the caller did not ask for the
    /// start, and, for a target ahead, the entry is past the tick shown.
    pub(super) fn ring_entry_for(&self, target: u64, requested: Option<api::SeekRoute>) -> Option<u64> {
        if requested == Some(api::SeekRoute::Boot)
            || !self.play.as_ref().is_some_and(|play| play.begun() && !play.awaiting())
        {
            return None;
        }
        let at = self.ring.as_ref()?.nearest(target)?;
        (target <= self.tick || at > self.tick).then_some(at)
    }

    /// A ring jump: the entry loads at the next frame's start
    /// ([`Session::ring_frame`]) and the player runs forward to `target`.
    pub(super) fn ring_jump(&mut self, target: u64, from: u64, requested: Option<api::SeekRoute>) {
        let Some(ring) = &mut self.ring else { return };
        ring.ask(Jump { target, from, requested });
        self.seek = Some(SeekProgress {
            target,
            from_tick: from,
            source: api::SeekSource::Ring,
            requested_source: requested,
        });
        self.seeking = Some(target);
        self.restart = None;
        self.step_until = None;
        self.rate = Default::default();
    }

    /// A ring jump waits for its entry to load.
    pub(super) fn ring_pending(&self) -> bool {
        self.ring.as_ref().is_some_and(|ring| ring.pending().is_some())
    }

    /// Load the entry a ring jump asked for, at a frame's start. `Err` with
    /// the jump's target when the ring was dropped: the jump is queued for
    /// the start route ([`State::queue_jump`]) and the session queues the
    /// film's game again.
    pub(super) fn ring_restore(&mut self, mem: &mut dyn GuestMemory) -> Result<(), u64> {
        let Some(jump) = self.ring.as_mut().and_then(Ring::take_pending) else {
            return Ok(());
        };
        let running = self.play.as_ref().is_some_and(|play| play.begun() && !play.awaiting());
        let loaded = match (&mut self.ring, &mut self.play) {
            (Some(ring), Some(play)) if running => ring.load(jump, mem).and_then(|loaded| {
                play.resume_at(loaded.tick, loaded.game_ticks)
                    .map(|()| loaded)
                    .map_err(|e| format!("{e:#}"))
            }),
            _ => Err("the film's game is not running".into()),
        };
        match loaded {
            Ok(loaded) => {
                self.tick = loaded.tick;
                self.complete = false;
                self.seeking = (loaded.tick < jump.target).then_some(jump.target);
                if self.seeking.is_none()
                    && let Some(run) = &mut self.measurements
                {
                    run.reached_target(loaded.tick);
                }
                self.rate = Default::default();
                if let Some(clock) = &mut self.pacing {
                    clock.debt = 0.;
                }
                eprintln!(
                    "theater: loaded the ring entry at tick {} in {} ms for the jump to tick {}",
                    loaded.tick,
                    self.ring.as_ref().and_then(Ring::restore_ms).unwrap_or(0),
                    jump.target
                );
                Ok(())
            }
            Err(why) => {
                if let Some(ring) = &mut self.ring {
                    ring.turn_off(format!("loading the entry for the jump to tick {}: {why}", jump.target));
                }
                self.queue_jump(jump.target, jump.from, jump.requested);
                Err(jump.target)
            }
        }
    }

    /// After a game tick: a divergence that answers for the last ring load
    /// is not the film's. The ring goes, the failure with it, and the jump
    /// is queued for the start route; its target is returned for the
    /// session to queue the film's game again.
    pub(super) fn ring_diverged(&mut self) -> Option<u64> {
        let why = self.failure.clone()?;
        let jump = self.ring.as_mut()?.diverged(self.tick, &why)?;
        self.failure = None;
        self.queue_jump(jump.target, jump.from, jump.requested);
        Some(jump.target)
    }

    /// A game tick of the film ended well: the ring takes its entry when
    /// one is due.
    pub(super) fn ring_passed(&mut self, mem: &dyn GuestMemory) {
        let Some(play) = self.play.as_ref().filter(|play| play.begun() && !play.awaiting()) else {
            return;
        };
        let game_ticks = play.game_ticks();
        if let Some(ring) = &mut self.ring {
            ring.passed(mem, self.tick, game_ticks);
        }
    }

    /// `theater.keyframes`.
    pub(super) fn keyframes(&self) -> api::Keyframes {
        self.ring.as_ref().map_or_else(
            || api::Keyframes { off: Some("this player keeps no ring".into()), ..Default::default() },
            Ring::keyframes,
        )
    }

    /// How long the ring jump being reported took to load its entry.
    pub(super) fn ring_restore_ms(&self, source: api::SeekSource) -> Option<u64> {
        (source == api::SeekSource::Ring)
            .then(|| self.ring.as_ref().and_then(Ring::restore_ms))
            .flatten()
    }
}

impl Session {
    /// A film played from its game start keeps a ring, unless it is a
    /// verification, which plays straight through.
    pub(super) fn open_ring(&self) {
        let mut s = self.lock();
        if s.verify.is_none() {
            s.ring = Some(Ring::from_env());
        }
    }

    /// Whether a ring jump waits for its entry to load.
    pub(super) fn ring_due(&self) -> bool {
        self.lock().ring_pending()
    }

    /// A frame starts (`pacing.rs`): load the entry a ring jump asked for.
    /// The target of a jump whose entry did not load, which the caller
    /// sends by the start route ([`Self::start_over`]) once it has let go
    /// of guest memory.
    pub(super) fn ring_frame(&self, mem: &mut dyn GuestMemory) -> Option<u64> {
        self.lock().ring_restore(mem).err()
    }

    /// The ring could not serve a jump to `target`: the film's game is
    /// queued again and plays to it from its first update.
    pub(super) fn start_over(&self, target: u64) {
        if let Err(e) = self.requeue(target) {
            self.lock().fail(format!(
                "the jump to tick {target} cannot start the film again: {}",
                e.message
            ));
        }
    }
}

#[cfg(test)]
pub(super) mod fakes {
    use super::*;
    use crate::theater::start_point_state::fakes::Ext;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Extension records a test reads while the ring holds them.
    #[derive(Clone, Default)]
    pub struct SharedExt(pub Arc<Mutex<Ext>>);

    impl ExtRecords for SharedExt {
        fn save(&self) -> Vec<u8> {
            self.0.lock().unwrap().save()
        }
        fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().load(bytes)
        }
        fn digest(&self) -> Option<u64> {
            self.0.lock().unwrap().digest()
        }
    }

    /// The ring's outside state in a test: shared records and a count of
    /// monitor loads. The process's statics are left alone, but for the
    /// Forge monitors when `forge` is set (the test holds
    /// `forge_film::tests::turn`).
    #[derive(Clone, Default)]
    pub struct Fake {
        pub ext: SharedExt,
        pub monitors_put: Arc<AtomicUsize>,
        pub forge: bool,
    }

    impl Fake {
        pub fn puts(&self) -> usize {
            self.monitors_put.load(Ordering::SeqCst)
        }
    }

    impl Outside for Fake {
        fn ext(&mut self) -> &mut dyn ExtRecords {
            &mut self.ext
        }
        fn monitors(&self) -> forge_film::ring::Kept {
            if self.forge { forge_film::ring::snapshot() } else { Default::default() }
        }
        fn put_monitors(&mut self, kept: &forge_film::ring::Kept) {
            if self.forge {
                forge_film::ring::restore(kept);
            }
            self.monitors_put.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests;
