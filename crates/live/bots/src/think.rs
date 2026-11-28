//! Bot brains off the game thread (docs/game/bots.md §5.5).
//!
//! A phantom's pad used to be sampled whole, read and think, when the
//! simulation gathered its slot. In a batched [`Mode`] the first gather of
//! a tick samples every phantom at once, in three phases:
//!
//! 1. **Read**, on the game thread in seat order: each source's
//!    [`PadSource::observe`] takes what it needs from the guest and from
//!    host-wide state (kill feed, shots, the loaded map, the radio) into
//!    its own staged observation.
//! 2. **Think**, on a small pool of worker threads: each source's
//!    [`PadSource::think`] touches only that source, the shared map data
//!    (read-only) and thread-local scratch. Which thread runs which source
//!    cannot change a pad.
//! 3. **Conclude**, on the game thread in seat order: each source's
//!    [`PadSource::conclude`] publishes its radio calls, placed as the
//!    serial order would have placed them (`decisions::radio::Seat`).
//!
//! **Pipelined**, the thinks staged at tick N run while the guest
//! simulates tick N, and their pads are tick N+1's: a bot acts on what it
//! saw one tick ago. The uplink credits this tick toward any configured
//! transport delay (`phantom::action`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

use guest_layout::memory::GuestMemory;
use input::host::HostPadState;

use crate::decisions::radio;
use crate::pad_source::{Frame, PadSource, Sampled};

/// A pad source shared between the books and a think in flight.
pub type Source = Arc<Mutex<Box<dyn PadSource>>>;

/// The source, waiting for a think in flight on it to finish.
pub fn lock_source(s: &Source) -> MutexGuard<'_, Box<dyn PadSource>> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// How phantom pads are sampled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Each phantom sampled whole at its own slot's gather, on the game
    /// thread: the original order, and the reference.
    #[default]
    Serial,
    /// Every phantom read at the tick's first gather and thought across
    /// `threads` workers (0: on the game thread, still batched). With
    /// `pipeline`, the thinks overlap the guest's tick and their pads are
    /// the next tick's.
    Batched { threads: usize, pipeline: bool },
}

/// The worker count: `0`, `off` or unset for [`Mode::Serial`], a number,
/// or `auto` (one fewer than the cores, at most 8).
pub const THREADS_ENV: &str = "H2_BOT_THREADS";
/// `1`, `on`, `true` or `yes` pipelines the thinks ([`Mode::Batched`]);
/// with no thread count it takes `auto`.
pub const PIPELINE_ENV: &str = "H2_BOT_PIPELINE";

/// The most workers asked for by number.
pub const MAX_THREADS: usize = 32;

/// Nanoseconds the game thread has spent sampling phantom pads, in any
/// [`Mode`], since the process began: the read, think and conclude phases
/// that run on it, and the wait for workers. A think that runs on a worker
/// is not in it. `h2 sim` reports the delta over a match as
/// `wall.bots_ms_per_tick`.
static GAME_THREAD_NS: AtomicU64 = AtomicU64::new(0);

/// [`GAME_THREAD_NS`]'s value now.
pub fn game_thread_ns() -> u64 {
    GAME_THREAD_NS.load(Ordering::Relaxed)
}

/// Adds the time from its creation to its drop to [`GAME_THREAD_NS`].
pub(crate) struct GameThreadTimer(Instant);

impl GameThreadTimer {
    pub(crate) fn start() -> Self {
        Self(Instant::now())
    }
}

impl Drop for GameThreadTimer {
    fn drop(&mut self) {
        let ns = u64::try_from(self.0.elapsed().as_nanos()).unwrap_or(u64::MAX);
        GAME_THREAD_NS.fetch_add(ns, Ordering::Relaxed);
    }
}

fn auto_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get().saturating_sub(1))
        .clamp(1, 8)
}

impl Mode {
    /// The mode from [`THREADS_ENV`] and [`PIPELINE_ENV`]'s values.
    pub fn from_values(threads: Option<&str>, pipeline: Option<&str>) -> Mode {
        let pipeline = matches!(
            pipeline.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
            Some("1" | "on" | "true" | "yes")
        );
        let threads = match threads.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("" | "0" | "off" | "serial" | "no" | "false") => None,
            Some("auto") => Some(auto_threads()),
            Some(n) => n.parse::<usize>().ok().map(|n| n.min(MAX_THREADS)),
        };
        match (threads, pipeline) {
            (None, false) => Mode::Serial,
            (None, true) => Mode::Batched {
                threads: auto_threads(),
                pipeline,
            },
            (Some(threads), pipeline) => Mode::Batched { threads, pipeline },
        }
    }

    /// The mode the environment asks for.
    pub fn from_env() -> Mode {
        Mode::from_values(
            std::env::var(THREADS_ENV).ok().as_deref(),
            std::env::var(PIPELINE_ENV).ok().as_deref(),
        )
    }

    pub fn pipelined(self) -> bool {
        matches!(self, Mode::Batched { pipeline: true, .. })
    }

    pub fn batched(self) -> bool {
        matches!(self, Mode::Batched { .. })
    }

    fn threads(self) -> usize {
        match self {
            Mode::Serial => 0,
            Mode::Batched { threads, .. } => threads,
        }
    }
}

type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// Queued or running.
    unfinished: usize,
    closed: bool,
    /// A job's panic, for the waiting game thread to carry on.
    panic: Option<Box<dyn std::any::Any + Send>>,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    work: Condvar,
    idle: Condvar,
}

impl Shared {
    fn finish(&self, outcome: std::thread::Result<()>) {
        let mut q = lock(&self.queue);
        q.unfinished -= 1;
        if let Err(p) = outcome
            && q.panic.is_none()
        {
            q.panic = Some(p);
        }
        if q.unfinished == 0 {
            self.idle.notify_all();
        }
    }
}

fn run(job: Job) -> std::thread::Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
}

/// A few long-lived worker threads. The thread that waits runs queued jobs
/// too, so a pool of none runs everything on the caller.
pub(crate) struct Pool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl Pool {
    pub fn new(threads: usize) -> Pool {
        let shared = Arc::new(Shared::default());
        let workers = (0..threads)
            .filter_map(|i| {
                let shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("bots-think-{i}"))
                    .spawn(move || work(&shared))
                    .ok()
            })
            .collect();
        Pool { shared, workers }
    }

    #[cfg(test)]
    pub fn threads(&self) -> usize {
        self.workers.len()
    }

    pub fn submit(&self, jobs: Vec<Job>) {
        if jobs.is_empty() {
            return;
        }
        let mut q = lock(&self.shared.queue);
        q.unfinished += jobs.len();
        q.jobs.extend(jobs);
        drop(q);
        self.shared.work.notify_all();
    }

    /// Retire the previous map's work without helping or waiting. A job
    /// already popped by a worker has its own source and output slot.
    fn cancel_queued(&self) {
        let mut q = lock(&self.shared.queue);
        let queued = q.jobs.len();
        q.jobs.clear();
        q.unfinished -= queued;
        if q.unfinished == 0 {
            self.shared.idle.notify_all();
        }
    }

    fn busy(&self) -> bool {
        lock(&self.shared.queue).unfinished != 0
    }

    /// Returns once every submitted job is done, running queued ones on
    /// this thread meanwhile. A job's panic goes on here.
    pub fn wait(&self) {
        let mut q = lock(&self.shared.queue);
        loop {
            if let Some(job) = q.jobs.pop_front() {
                drop(q);
                self.shared.finish(run(job));
                q = lock(&self.shared.queue);
                continue;
            }
            if q.unfinished == 0 {
                break;
            }
            q = self.shared.idle.wait(q).unwrap_or_else(|e| e.into_inner());
        }
        if let Some(p) = q.panic.take() {
            drop(q);
            std::panic::resume_unwind(p);
        }
    }
}

fn work(shared: &Shared) {
    let mut q = lock(&shared.queue);
    loop {
        if let Some(job) = q.jobs.pop_front() {
            drop(q);
            shared.finish(run(job));
            q = lock(&shared.queue);
            continue;
        }
        if q.closed {
            return;
        }
        q = shared.work.wait(q).unwrap_or_else(|e| e.into_inner());
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        lock(&self.shared.queue).closed = true;
        self.shared.work.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

/// One phantom in a batch. Seats are given in the order the simulation
/// gathers their slots.
pub struct Seat<'a> {
    /// Who it is (the phantom's handle), to match a pipelined think.
    pub key: u32,
    /// Bumped on every input reset: a think from before one is dropped.
    pub generation: u64,
    pub source: &'a Source,
    pub frame: u64,
    pub machine: [u8; 6],
}

/// The pad and the observation it came from travel together, including
/// while the next tick's think is running on another thread.
#[derive(Clone, Copy, Default)]
pub(crate) struct Report {
    pub(crate) pad: HostPadState,
    pub(crate) latency: crate::tuning::PhantomLatency,
    pub(crate) think_ticks: u32,
    /// A brain report can have no current target. Scripts have no decision.
    pub(crate) shot_range: Option<Option<f32>>,
}

impl Report {
    pub(crate) fn from_source(pad: HostPadState, source: &dyn PadSource) -> Self {
        Self {
            pad,
            latency: source.phantom_latency(),
            think_ticks: 0,
            shot_range: source.combat_stats().map(|s| s.range),
        }
    }
}

type Slot = Arc<Mutex<Option<Report>>>;

struct Thought {
    key: u32,
    generation: u64,
    source: Source,
    pad: Slot,
}

/// Thinks staged at `tick`, running while the guest simulates it.
struct InFlight {
    tick: u32,
    thoughts: Vec<Thought>,
}

/// Samples the phantoms' pad sources in a [`Mode`].
pub struct Thinker {
    mode: Mode,
    pool: Option<Pool>,
    radio: &'static Mutex<radio::Radio>,
    in_flight: Option<InFlight>,
    retiring: bool,
}

impl Default for Thinker {
    fn default() -> Self {
        Thinker::new(Mode::Serial)
    }
}

impl Thinker {
    pub fn new(mode: Mode) -> Self {
        Thinker {
            mode,
            pool: None,
            radio: radio::host(),
            in_flight: None,
            retiring: false,
        }
    }

    /// A thinker on its own radio, for tests that run beside each other.
    #[cfg(test)]
    pub(crate) fn on_radio(mode: Mode, board: &'static Mutex<radio::Radio>) -> Self {
        let mut t = Thinker::new(mode);
        t.radio = board;
        t
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Switches mode, finishing any think in flight first.
    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.settle();
        if mode.threads() != self.mode.threads() {
            self.pool = None;
        }
        self.mode = mode;
    }

    fn pool(&mut self) -> &Pool {
        let threads = self.mode.threads();
        self.pool.get_or_insert_with(|| Pool::new(threads))
    }

    /// The action stream ended on a render frame. Drop its unpublished pads
    /// and radio messages. The next session may sample only after any worker
    /// that already took a job releases the old source.
    pub(crate) fn end_game(&mut self) {
        if self.in_flight.take().is_some() {
            if let Some(pool) = &self.pool {
                pool.cancel_queued();
                self.retiring = true;
            }
            radio::lock(self.radio).end_batch();
        }
    }

    pub(crate) fn ready_for_game(&mut self) -> bool {
        if self.retiring {
            self.retiring = self.pool.as_ref().is_some_and(Pool::busy);
        }
        !self.retiring
    }

    /// Finishes and publishes pipelined decisions before changing modes,
    /// dropping their pads. A game transition uses `end_game` instead.
    pub fn settle(&mut self) {
        if let Some(f) = self.in_flight.take() {
            self.conclude(&f);
        }
    }

    fn conclude(&mut self, f: &InFlight) {
        if let Some(pool) = &self.pool {
            pool.wait();
        }
        for t in &f.thoughts {
            lock_source(&t.source).conclude();
        }
        radio::lock(self.radio).end_batch();
    }

    /// The read phase for every seat. A seat's pad is filled in when its
    /// source gave one outright; the others have a think staged.
    fn observe(
        &mut self,
        tick: u32,
        guest: &dyn GuestMemory,
        seats: &[Seat<'_>],
    ) -> (Vec<Report>, Vec<(usize, Thought)>) {
        radio::lock(self.radio).begin_batch();
        let mut pads = vec![Report::default(); seats.len()];
        let mut thoughts = Vec::new();
        for (i, seat) in seats.iter().enumerate() {
            let frame = Frame {
                frame: seat.frame,
                tick,
                machine: seat.machine,
                guest,
            };
            let mut source = lock_source(seat.source);
            match source.observe(&frame) {
                Sampled::Pad(pad) => pads[i] = Report::from_source(pad, &**source),
                Sampled::Think => thoughts.push((
                    i,
                    Thought {
                        key: seat.key,
                        generation: seat.generation,
                        source: Arc::clone(seat.source),
                        pad: Slot::default(),
                    },
                )),
            }
        }
        let jobs = thoughts
            .iter()
            .map(|(_, t)| {
                let (source, pad) = (Arc::clone(&t.source), Arc::clone(&t.pad));
                Box::new(move || {
                    let mut source = lock_source(&source);
                    let thought = source.think();
                    *lock(&pad) = Some(Report::from_source(thought, &**source));
                }) as Job
            })
            .collect();
        self.pool().submit(jobs);
        (pads, thoughts)
    }

    /// One pad per seat for `tick`, in seat order.
    pub fn sample(
        &mut self,
        tick: u32,
        guest: &dyn GuestMemory,
        seats: &[Seat<'_>],
    ) -> Vec<HostPadState> {
        self.sample_reports(tick, guest, seats)
            .into_iter()
            .map(|report| report.pad)
            .collect()
    }

    pub(crate) fn sample_reports(
        &mut self,
        tick: u32,
        guest: &dyn GuestMemory,
        seats: &[Seat<'_>],
    ) -> Vec<Report> {
        let _timer = GameThreadTimer::start();
        // The seats read one object scan between them (`arms::share_ground`).
        let mut ground = crate::arms::SharedGround::default();
        crate::arms::share_ground(&mut ground, u64::from(tick), || {
            self.sample_seats(tick, guest, seats)
        })
    }

    fn sample_seats(
        &mut self,
        tick: u32,
        guest: &dyn GuestMemory,
        seats: &[Seat<'_>],
    ) -> Vec<Report> {
        match self.mode {
            Mode::Serial => {
                self.settle();
                seats
                    .iter()
                    .map(|s| {
                        let mut source = lock_source(s.source);
                        let pad = source.pad(&Frame {
                            frame: s.frame,
                            tick,
                            machine: s.machine,
                            guest,
                        });
                        Report::from_source(pad, &**source)
                    })
                    .collect()
            }
            Mode::Batched {
                pipeline: false, ..
            } => {
                self.settle();
                let (mut pads, thoughts) = self.observe(tick, guest, seats);
                if let Some(pool) = &self.pool {
                    pool.wait();
                }
                for (i, t) in &thoughts {
                    pads[*i] = lock(&t.pad).take().unwrap_or_default();
                    lock_source(&t.source).conclude();
                }
                radio::lock(self.radio).end_batch();
                pads
            }
            Mode::Batched { pipeline: true, .. } => {
                // Last tick's thinks: finished, published, and this tick's
                // pads when they follow on without a gap or a reset.
                let previous = self.in_flight.take();
                if let Some(f) = &previous {
                    self.conclude(f);
                }
                let (mut pads, thoughts) = self.observe(tick, guest, seats);
                for (i, t) in &thoughts {
                    pads[*i] = previous
                        .as_ref()
                        .filter(|f| f.tick.wrapping_add(1) == tick)
                        .and_then(|f| {
                            f.thoughts
                                .iter()
                                .find(|p| p.key == t.key && p.generation == t.generation)
                        })
                        .and_then(|p| *lock(&p.pad))
                        .map(|report| Report {
                            think_ticks: 1,
                            ..report
                        })
                        .unwrap_or_default();
                }
                self.in_flight = Some(InFlight {
                    tick,
                    thoughts: thoughts.into_iter().map(|(_, t)| t).collect(),
                });
                pads
            }
        }
    }
}

impl Drop for Thinker {
    fn drop(&mut self) {
        self.settle();
    }
}

#[cfg(test)]
pub(crate) mod tests;
