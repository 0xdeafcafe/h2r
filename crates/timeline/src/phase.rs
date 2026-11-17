//! Frame-time stats: where each frame's wall time goes, split into a few
//! phases, with a rolling window (fps, p50, p99, max) and a whole-run
//! report of the worst frames. `H2_FRAME_STATS=1` turns it on for `h2 run`
//! and `h2 play` (docs/perf/frame-budget.md).
//!
//! Each thread has one current phase. [`enter`] switches it until the
//! returned guard drops, so nested phases are self time: a `Submit` inside
//! an `Encode` inside a `Decode` takes its time out of both. Time outside
//! any phase is [`Phase::Guest`]: generated code, the runtime, the kernel
//! and every HLE call that is not one of the others. [`frame_end`] (called
//! by [`crate::frame`], at every Swap) closes the frame on the calling
//! thread, which is the game thread. A queued [`WorkerFrame`] joins the
//! render thread's work to that same frame once both samples arrive.
//! Thread phases are self time locally; renderer work overlaps game-thread
//! time, so their combined durations can exceed the frame's wall time.
//!
//! Cost while off: one relaxed load and a branch per hook. While on: one
//! clock read per phase switch (a handful per draw) and, once a frame, a
//! push under a mutex. Nothing is recorded per event.
//!
//! A *guest scope* ([`scope_begin`], [`scope_end`]) is a phase that belongs
//! to one guest thread rather than to a Rust block: the `render-timing`
//! feature opens [`Phase::Render`] when the game enters its render
//! front-end and closes it when that returns. Every guest thread runs on the
//! one host thread, so the kernel says which guest thread is running
//! ([`guest_thread`], at each slice); while another thread (or a DPC, or
//! the scheduler) runs, the scope is suspended and its time goes back to
//! the phase it interrupted.

use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Where a frame's time goes. Self time within each thread; worker
/// contributions can overlap game-thread time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Everything else on the game thread: guest code, runtime accessors,
    /// the kernel and scheduler, HLE calls (XAPI, DirectSound, file I/O,
    /// the D3D API entries that only write the push buffer). Not the time
    /// in [`Phase::Render`] when that is being measured.
    Guest = 0,
    /// Push-buffer decode and device state (`pushbuf drain` self time).
    Decode = 1,
    /// wgpu work on the CPU: draw recording, shader prepare and uniform
    /// staging, render-pass encode, pipeline creation, texture uploads,
    /// surface writes and snapshots, the present copy.
    Encode = 2,
    /// `queue.submit`.
    Submit = 3,
    /// Waiting for the GPU: surface and pixel-count readbacks, the
    /// `D3DSWAP_FINISH` wait.
    Wait = 4,
    /// Pacing to the wall clock: `h2 play`'s tick sleep and the kernel
    /// pacer. Not a cost: a frame under budget spends its slack here.
    Pace = 5,
    /// Game-thread wall time waiting for render-thread answers or its
    /// frame-in-flight limit. Excluded from `Guest` self time.
    RenderWait = 6,
    /// The game's render front-end on the game thread: the guest thread
    /// that entered `render_frame_and_present`, from entry to return, minus
    /// the other phases inside it (push-buffer decode, render-wait) and
    /// minus slices of other guest threads. Zero unless the
    /// `render-timing` feature is on. `Guest + Render` is all guest time.
    Render = 7,
    /// The D3D API entries (our `d3d` layer's HLE overrides) on the game
    /// thread, minus the phases inside them (push-buffer decode,
    /// render-wait). Used to be part of `cpu` (or `render`).
    D3d = 8,
}

/// Number of [`Phase`]s.
pub const PHASES: usize = 9;

impl Phase {
    pub const ALL: [Phase; PHASES] = [
        Phase::Guest,
        Phase::Decode,
        Phase::Encode,
        Phase::Submit,
        Phase::Wait,
        Phase::Pace,
        Phase::RenderWait,
        Phase::Render,
        Phase::D3d,
    ];

    /// Short name for log lines and the overlay.
    pub fn label(self) -> &'static str {
        match self {
            Phase::Guest => "cpu",
            Phase::Decode => "decode",
            Phase::Encode => "encode",
            Phase::Submit => "submit",
            Phase::Wait => "gpu-wait",
            Phase::Pace => "pace",
            Phase::RenderWait => "render-wait",
            Phase::Render => "render",
            Phase::D3d => "d3d",
        }
    }
}

/// Things counted per frame, to explain a slow one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Counter {
    /// Draws through the wgpu backend.
    Draws = 0,
    /// `queue.submit` calls.
    Submits = 1,
    /// Render pipelines the encoding thread waited for or built when encoding
    /// a pass (a Metal compile on the critical path). Cache misses a
    /// compile worker finished in time are not counted.
    Pipelines = 2,
    /// nv2a programs translated to WGSL and compiled (shader cache misses).
    Shaders = 3,
    /// Guest textures decoded and uploaded.
    Uploads = 4,
    /// GPU round trips that block: surface and pixel-count readbacks.
    Readbacks = 5,
    /// Bytes the guest read from files (`NtReadFile`).
    ReadBytes = 6,
    /// Vertex-stream bytes the device copied out of guest memory for draws.
    VertexCopied = 7,
    /// Vertex-stream bytes draws took from the device's vertex cache
    /// without a copy.
    VertexReused = 8,
    /// Render and compute passes encoded (each is a Metal encoder and
    /// command buffer of its own in wgpu: the per-pass CPU cost).
    Passes = 9,
    /// Queued buffer writes from `gfx` (each is a staging buffer wgpu
    /// makes and frees).
    QueueWrites = 10,
    /// Of [`Passes`](Self::Passes): render passes started because the draw
    /// before, in the same pending pass, wrote some colour channels but not
    /// all, and this one writes others (the merge rule's mask split).
    PassMask = 11,
    /// Of [`Passes`](Self::Passes): the same split after a draw that wrote
    /// no colour channel at all.
    PassNoColor = 12,
    /// Of [`Passes`](Self::Passes): render passes started for a draw that
    /// can `discard` (after a partly masked draw with the same mask under
    /// the fast rule; every one under the exact rule).
    PassDiscard = 13,
    /// Of [`Passes`](Self::Passes): visibility-test passes, the count pass
    /// and the sum of each counted draw and the render pass the draws after
    /// it resume in.
    PassCounted = 14,
    /// Of [`Passes`](Self::Passes): side passes (alias mirror, depth
    /// aliases and levels, masked clears).
    PassSide = 15,
}

/// Number of [`Counter`]s.
pub const COUNTERS: usize = 16;

impl Counter {
    pub const ALL: [Counter; COUNTERS] = [
        Counter::Draws,
        Counter::Submits,
        Counter::Pipelines,
        Counter::Shaders,
        Counter::Uploads,
        Counter::Readbacks,
        Counter::ReadBytes,
        Counter::VertexCopied,
        Counter::VertexReused,
        Counter::Passes,
        Counter::QueueWrites,
        Counter::PassMask,
        Counter::PassNoColor,
        Counter::PassDiscard,
        Counter::PassCounted,
        Counter::PassSide,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Counter::Draws => "draws",
            Counter::Submits => "submits",
            Counter::Pipelines => "pipelines",
            Counter::Shaders => "shaders",
            Counter::Uploads => "uploads",
            Counter::Readbacks => "readbacks",
            Counter::ReadBytes => "read_bytes",
            Counter::VertexCopied => "vtx_copied",
            Counter::VertexReused => "vtx_reused",
            Counter::Passes => "passes",
            Counter::QueueWrites => "queue_writes",
            Counter::PassMask => "pass_mask",
            Counter::PassNoColor => "pass_nocolor",
            Counter::PassDiscard => "pass_discard",
            Counter::PassCounted => "pass_counted",
            Counter::PassSide => "pass_side",
        }
    }
}

static ON: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// True while frame stats are being collected. One relaxed load.
#[inline(always)]
pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

struct Local {
    generation: Cell<u64>,
    started: Cell<bool>,
    current: Cell<u8>,
    since: Cell<u64>,
    frame_start: Cell<u64>,
    acc: [Cell<u64>; PHASES],
    /// Like `acc`, but never taken by a frame: for [`thread_ns`].
    total: [Cell<u64>; PHASES],
    /// Like `total`, but only while a guest scope is open and running on
    /// its owner: for [`scope_ns`].
    scoped: [Cell<u64>; PHASES],
    /// Entries into each phase while a guest scope is open and running on
    /// its owner: for [`scope_entries`].
    entries: [Cell<u64>; PHASES],
    counts: [Cell<u64>; COUNTERS],
    /// The guest scope: its phase (`INERT` when none is open), nesting
    /// depth, owning guest thread, the phase it interrupted, and whether
    /// another guest thread is running now.
    scope: Cell<u8>,
    scope_depth: Cell<u32>,
    scope_owner: Cell<u32>,
    scope_prev: Cell<u8>,
    suspended: Cell<bool>,
    /// The guest thread the kernel says is running, or `NO_THREAD`.
    guest: Cell<u32>,
}

/// No guest thread is running (or the kernel never said).
const NO_THREAD: u32 = u32::MAX;

thread_local! {
    static LOCAL: Local = const {
        Local {
            generation: Cell::new(0),
            started: Cell::new(false),
            current: Cell::new(Phase::Guest as u8),
            since: Cell::new(0),
            frame_start: Cell::new(0),
            acc: [const { Cell::new(0) }; PHASES],
            total: [const { Cell::new(0) }; PHASES],
            scoped: [const { Cell::new(0) }; PHASES],
            entries: [const { Cell::new(0) }; PHASES],
            counts: [const { Cell::new(0) }; COUNTERS],
            scope: Cell::new(INERT),
            scope_depth: Cell::new(0),
            scope_owner: Cell::new(NO_THREAD),
            scope_prev: Cell::new(Phase::Guest as u8),
            suspended: Cell::new(false),
            guest: Cell::new(NO_THREAD),
        }
    };
}

const INERT: u8 = u8::MAX;

/// Restores the previous phase on drop. Inert when stats were off at
/// [`enter`].
#[must_use = "the phase ends when the guard drops"]
pub struct PhaseGuard {
    prev: u8,
}

impl Drop for PhaseGuard {
    #[inline]
    fn drop(&mut self) {
        if self.prev != INERT {
            switch(self.prev);
        }
    }
}

/// Charges the time from now until the guard drops to `phase`.
#[inline]
pub fn enter(phase: Phase) -> PhaseGuard {
    if !enabled() {
        return PhaseGuard { prev: INERT };
    }
    PhaseGuard {
        prev: switch(phase as u8),
    }
}

/// Makes `to` the current phase; returns the one it replaces.
#[inline(never)]
fn switch(to: u8) -> u8 {
    let now = crate::now_ns();
    LOCAL.with(|l| {
        start_local(l, now);
        let cur = charge(l, now);
        l.current.set(to);
        if l.scope_depth.get() != 0 && !l.suspended.get() {
            let e = &l.entries[to as usize];
            e.set(e.get() + 1);
        }
        cur as u8
    })
}

/// Charges the time since the last switch to the current phase; returns it.
fn charge(l: &Local, now: u64) -> usize {
    let cur = l.current.get() as usize;
    let d = now.saturating_sub(l.since.get());
    l.acc[cur].set(l.acc[cur].get() + d);
    l.total[cur].set(l.total[cur].get() + d);
    if l.scope_depth.get() != 0 && !l.suspended.get() {
        l.scoped[cur].set(l.scoped[cur].get() + d);
    }
    l.since.set(now);
    cur
}

/// Opens a guest scope of `phase` for the running guest thread: its time
/// goes to `phase` until [`scope_end`], except while the kernel runs another
/// guest thread ([`guest_thread`]). Nested opens count; only the outermost
/// switches. A no-op while stats are off.
pub fn scope_begin(phase: Phase) {
    if !enabled() {
        return;
    }
    let now = crate::now_ns();
    LOCAL.with(|l| {
        start_local(l, now);
        let depth = l.scope_depth.get();
        if depth != 0 {
            l.scope_depth.set(depth + 1);
            return;
        }
        // Charge the time before the scope while it is still closed.
        let prev = charge(l, now);
        l.scope_depth.set(1);
        l.scope.set(phase as u8);
        l.scope_owner.set(l.guest.get());
        l.suspended.set(false);
        l.scope_prev.set(prev as u8);
        l.current.set(phase as u8);
    });
}

/// Closes the guest scope [`scope_begin`] opened. Unmatched closes (stats
/// turned on inside a scope) are ignored.
pub fn scope_end() {
    if !enabled() {
        return;
    }
    let now = crate::now_ns();
    LOCAL.with(|l| {
        start_local(l, now);
        let depth = l.scope_depth.get();
        if depth == 0 {
            return;
        }
        if depth != 1 {
            l.scope_depth.set(depth - 1);
            return;
        }
        charge(l, now);
        if !l.suspended.get() {
            l.current.set(l.scope_prev.get());
        }
        l.scope_depth.set(0);
        l.scope.set(INERT);
        l.suspended.set(false);
    });
}

/// The kernel says which guest thread runs on this host thread from now
/// on: `Some(tid)` at the start of its slice, `None` when the slice ends.
/// An open guest scope is suspended while it is not its owner's slice.
#[inline]
pub fn guest_thread(tid: Option<u32>) {
    if enabled() {
        guest_thread_slow(tid.unwrap_or(NO_THREAD));
    }
}

#[inline(never)]
fn guest_thread_slow(tid: u32) {
    let now = crate::now_ns();
    LOCAL.with(|l| {
        start_local(l, now);
        l.guest.set(tid);
        if l.scope_depth.get() == 0 {
            return;
        }
        let mine = tid != NO_THREAD && tid == l.scope_owner.get();
        let to = match (mine, l.suspended.get()) {
            (false, false) => l.scope_prev.get(),
            (true, true) => l.scope.get(),
            _ => return,
        };
        // Charge up to now under the old state, then flip it.
        charge(l, now);
        l.suspended.set(!mine);
        l.current.set(to);
    });
}

/// Nanoseconds this host thread has spent in `phase` inside guest scopes
/// (open and on their owner's slices) since collection started, the
/// running interval included. `None` while stats are off. The
/// `render-timing` feature splits the render front-end with it: `Render`
/// is guest code there, `D3d` our D3D entries called from it.
pub fn scope_ns(phase: Phase) -> Option<u64> {
    if !enabled() {
        return None;
    }
    let now = crate::now_ns();
    Some(LOCAL.with(|l| {
        start_local(l, now);
        let p = phase as usize;
        let open =
            if l.current.get() as usize == p && l.scope_depth.get() != 0 && !l.suspended.get() {
                now.saturating_sub(l.since.get())
            } else {
                0
            };
        l.scoped[p].get() + open
    }))
}

/// How many times this host thread entered `phase` ([`enter`]) inside
/// guest scopes (open and on their owner's slices) since collection
/// started. `None` while stats are off. Every D3D entry enters
/// [`Phase::D3d`], so for that phase it counts D3D calls.
pub fn scope_entries(phase: Phase) -> Option<u64> {
    if !enabled() {
        return None;
    }
    let now = crate::now_ns();
    Some(LOCAL.with(|l| {
        start_local(l, now);
        l.entries[phase as usize].get()
    }))
}

/// Nanoseconds this host thread has spent in `phase` since collection
/// started, the running interval included. `None` while stats are off.
/// For splitting a phase further (the render stages): differences of two
/// readings are self time in `phase`.
pub fn thread_ns(phase: Phase) -> Option<u64> {
    if !enabled() {
        return None;
    }
    let now = crate::now_ns();
    Some(LOCAL.with(|l| {
        start_local(l, now);
        let p = phase as usize;
        let open = if l.current.get() as usize == p {
            now.saturating_sub(l.since.get())
        } else {
            0
        };
        l.total[p].get() + open
    }))
}

fn reset_local(l: &Local) {
    l.started.set(false);
    l.current.set(Phase::Guest as u8);
    for a in l
        .acc
        .iter()
        .chain(&l.total)
        .chain(&l.scoped)
        .chain(&l.entries)
    {
        a.set(0);
    }
    for c in &l.counts {
        c.set(0);
    }
    l.scope.set(INERT);
    l.scope_depth.set(0);
    l.scope_owner.set(NO_THREAD);
    l.suspended.set(false);
}

fn prepare_local(l: &Local) {
    let generation = GENERATION.load(Ordering::Relaxed);
    if l.generation.replace(generation) != generation {
        reset_local(l);
    }
}

fn start_local(l: &Local, now: u64) {
    prepare_local(l);
    if !l.started.get() {
        l.started.set(true);
        l.since.set(now);
        l.frame_start.set(now);
    }
}

/// Adds `n` to this frame's `counter`.
#[inline]
pub fn count(counter: Counter, n: u64) {
    if enabled() {
        LOCAL.with(|l| {
            prepare_local(l);
            let c = &l.counts[counter as usize];
            c.set(c.get() + n);
        });
    }
}

/// A queued render-thread boundary belonging to the next game-thread frame.
/// Create before `crate::frame`, then finish on the worker after all work
/// queued before that boundary. Neither side waits for the other.
pub struct WorkerFrame {
    generation: u64,
    index: u64,
}

/// Registers a worker contribution to the next frame, if stats are on.
/// A CSV row is published once both its game and worker samples arrive.
pub fn forward_frame() -> Option<WorkerFrame> {
    if !enabled() {
        return None;
    }
    LOCAL.with(|l| start_local(l, crate::now_ns()));
    let mut g = global();
    let r = g.as_mut()?;
    let index = crate::frames() + 1;
    r.pending.entry(index).or_default().expected += 1;
    Some(WorkerFrame {
        generation: r.generation,
        index,
    })
}

impl WorkerFrame {
    /// Takes this worker's phase/counter deltas and joins them to the game
    /// sample. Idle time between jobs (`Guest`) is deliberately excluded.
    pub fn finish(self) {
        crate::host::sample_thread(crate::host::Role::Render);
        if self.generation != GENERATION.load(Ordering::Relaxed) {
            // This marker precedes new-session work on the same FIFO.
            // Discard old work even if it executed after collection restarted.
            LOCAL.with(reset_local);
            return;
        }
        let mut g = global();
        let Some(r) = g.as_mut().filter(|r| r.generation == self.generation) else {
            LOCAL.with(reset_local);
            return;
        };
        let now = crate::now_ns();
        let sample = LOCAL.with(|l| {
            start_local(l, now);
            charge(l, now);
            let mut s = FrameSample::default();
            for (d, a) in s.phase_ns.iter_mut().zip(&l.acc) {
                *d = a.replace(0);
            }
            for (d, c) in s.counts.iter_mut().zip(&l.counts) {
                *d = c.replace(0);
            }
            s.phase_ns[Phase::Guest as usize] = 0;
            s
        });
        if let Some(p) = r.pending.get_mut(&self.index) {
            p.received += 1;
            for (a, n) in p.worker.phase_ns.iter_mut().zip(sample.phase_ns) {
                *a += n;
            }
            for (a, n) in p.worker.counts.iter_mut().zip(sample.counts) {
                *a += n;
            }
            r.publish_ready();
        }
    }
}

/// One finished frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameSample {
    /// Frame number (frames counted by [`crate::frame`]).
    pub index: u64,
    /// When it ended, [`crate::now_ns`].
    pub end_ns: u64,
    /// Wall time from the previous frame's end.
    pub total_ns: u64,
    /// Self time per phase, including parallel worker contributions.
    pub phase_ns: [u64; PHASES],
    pub counts: [u64; COUNTERS],
}

impl FrameSample {
    /// Wall time minus pacing: what the frame cost.
    pub fn busy_ns(&self) -> u64 {
        self.total_ns
            .saturating_sub(self.phase_ns[Phase::Pace as usize])
    }
}

/// Closes the current frame on this thread. Called by [`crate::frame`].
pub fn frame_end(index: u64) {
    if !enabled() {
        return;
    }
    let now = crate::now_ns();
    let sample = LOCAL.with(|l| {
        prepare_local(l);
        if !l.started.get() {
            start_local(l, now);
            return None;
        }
        charge(l, now);
        let mut s = FrameSample {
            index,
            end_ns: now,
            total_ns: now.saturating_sub(l.frame_start.get()),
            ..Default::default()
        };
        for (d, a) in s.phase_ns.iter_mut().zip(&l.acc) {
            *d = a.replace(0);
        }
        for (d, c) in s.counts.iter_mut().zip(&l.counts) {
            *d = c.replace(0);
        }
        l.frame_start.set(now);
        Some(s)
    });
    if let Some(s) = sample {
        let mut g = global();
        if let Some(r) = g.as_mut() {
            r.pending.entry(index).or_default().game = Some(s);
            r.publish_ready();
        }
    }
}

/// How stats are collected.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Print a window summary to stderr this often.
    pub log_every_ns: Option<u64>,
    /// Write one CSV row per frame here.
    pub csv: Option<std::path::PathBuf>,
    /// Leave frames before this one out of the whole-run report (the
    /// rolling window always has the latest frames).
    pub report_from: u64,
    /// Length of the rolling window.
    pub window_ns: u64,
    /// How many worst frames the run report keeps.
    pub worst: usize,
    /// Collect for a live display only (`h2 play`'s watermark): [`stop`]
    /// returns no report, and nothing is kept for one.
    pub quiet: bool,
}

impl Config {
    pub const DEFAULT_WINDOW_NS: u64 = 5_000_000_000;

    /// `H2_FRAME_STATS=1` (or any value but `0`) turns stats on and logs a
    /// line a second; `H2_FRAME_STATS_CSV=<file>` writes every frame;
    /// `H2_FRAME_STATS_FROM=N` starts the run report at frame N.
    pub fn from_env() -> Option<Config> {
        let on = std::env::var("H2_FRAME_STATS").is_ok_and(|v| !v.is_empty() && v != "0");
        let csv = std::env::var_os("H2_FRAME_STATS_CSV").map(Into::into);
        if !on && csv.is_none() {
            return None;
        }
        Some(Config {
            log_every_ns: on.then_some(1_000_000_000),
            csv,
            report_from: std::env::var("H2_FRAME_STATS_FROM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            window_ns: Self::DEFAULT_WINDOW_NS,
            worst: 10,
            quiet: false,
        })
    }

    /// Collection for a live display: a `window_ns` rolling window, no
    /// log, no CSV and no run report.
    pub fn live(window_ns: u64) -> Config {
        Config {
            report_from: u64::MAX,
            window_ns,
            worst: 1,
            quiet: true,
            ..Default::default()
        }
    }
}

/// Starts collecting with `config`, replacing any earlier collection.
pub fn start(config: Config) -> std::io::Result<()> {
    let csv = match &config.csv {
        Some(p) => {
            let mut f = std::io::BufWriter::new(std::fs::File::create(p)?);
            let phases: Vec<String> = Phase::ALL
                .iter()
                .map(|p| format!("{}_us", p.label()))
                .collect();
            let counts: Vec<&str> = Counter::ALL.iter().map(|c| c.label()).collect();
            writeln!(
                f,
                "frame,end_ms,total_us,{},{}",
                phases.join(","),
                counts.join(",")
            )?;
            Some(f)
        }
        None => None,
    };
    let mut recorder = Recorder::new(config, csv);
    let mut g = global();
    recorder.generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    *g = Some(recorder);
    ON.store(true, Ordering::Relaxed);
    Ok(())
}

/// Starts from the environment ([`Config::from_env`]); false when off.
pub fn start_from_env() -> bool {
    match Config::from_env() {
        Some(c) => match start(c) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("frame stats: {e}");
                false
            }
        },
        None => false,
    }
}

/// Stops collecting; returns the whole-run report. Queued worker frames
/// that have not completed are omitted and reported explicitly, without
/// blocking shutdown or publishing misleading zero-valued CSV rows.
pub fn stop() -> Option<String> {
    let mut g = global();
    ON.store(false, Ordering::Relaxed);
    let mut r = g.take()?;
    drop(g);
    if let Some(f) = r.csv.as_mut() {
        let _ = f.flush();
    }
    if r.config.quiet {
        return None;
    }
    let mut report = r.report();
    let pending = r.pending.values().filter(|p| p.game.is_some()).count();
    if pending != 0 {
        if !report.ends_with('\n') {
            report.push('\n');
        }
        report +=
            &format!("frame stats: {pending} incomplete render-thread frames omitted at stop\n");
    }
    Some(report)
}

/// Summary of the rolling window, for an overlay.
pub fn window_summary() -> Option<Summary> {
    global().as_ref()?.window_summary()
}

/// The frames published since the last call (or since collection started),
/// with each phase's sum and maximum; clears them. `None` while stats are
/// off. The host perf line (`boot::host_perf`) reads it once an interval.
pub fn take_interval() -> Option<Interval> {
    global().as_mut().map(|r| std::mem::take(&mut r.interval))
}

/// Frames over an interval: sums and per-frame maxima, for averages and
/// worst cases of each phase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Interval {
    pub frames: u64,
    /// Sum and maximum of the frames' wall times.
    pub total_sum_ns: u64,
    pub total_max_ns: u64,
    /// Per phase: sum over the frames, and the largest one frame had.
    pub phase_sum_ns: [u64; PHASES],
    pub phase_max_ns: [u64; PHASES],
    pub count_sum: [u64; COUNTERS],
}

impl Interval {
    pub fn add(&mut self, s: &FrameSample) {
        self.frames += 1;
        self.total_sum_ns += s.total_ns;
        self.total_max_ns = self.total_max_ns.max(s.total_ns);
        for i in 0..PHASES {
            self.phase_sum_ns[i] += s.phase_ns[i];
            self.phase_max_ns[i] = self.phase_max_ns[i].max(s.phase_ns[i]);
        }
        for (a, b) in self.count_sum.iter_mut().zip(s.counts) {
            *a += b;
        }
    }

    /// Mean ms per frame in `phase` (0 with no frames).
    pub fn avg_ms(&self, phase: Phase) -> f64 {
        self.phase_sum_ns[phase as usize] as f64 / 1e6 / self.frames.max(1) as f64
    }

    /// The most one frame spent in `phase`, ms.
    pub fn max_ms(&self, phase: Phase) -> f64 {
        self.phase_max_ns[phase as usize] as f64 / 1e6
    }

    /// Mean of `counter` per frame.
    pub fn per_frame(&self, counter: Counter) -> f64 {
        self.count_sum[counter as usize] as f64 / self.frames.max(1) as f64
    }
}

static GLOBAL: Mutex<Option<Recorder>> = Mutex::new(None);

fn global() -> MutexGuard<'static, Option<Recorder>> {
    GLOBAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
struct PendingFrame {
    game: Option<FrameSample>,
    worker: FrameSample,
    expected: u64,
    received: u64,
}

/// Frames collected: the rolling window and what the run report needs.
pub struct Recorder {
    generation: u64,
    pending: BTreeMap<u64, PendingFrame>,
    config: Config,
    window: VecDeque<FrameSample>,
    csv: Option<std::io::BufWriter<std::fs::File>>,
    last_log_ns: u64,
    /// Whole run, from `report_from`: wall time of every frame, in us.
    totals_us: Vec<u32>,
    phase_sum: [u64; PHASES],
    count_sum: [u64; COUNTERS],
    worst: Vec<FrameSample>,
    /// Since the last [`take_interval`].
    interval: Interval,
}

/// The window never holds more frames than this.
const WINDOW_MAX: usize = 4096;

impl Recorder {
    pub fn new(config: Config, csv: Option<std::io::BufWriter<std::fs::File>>) -> Self {
        Recorder {
            generation: 0,
            pending: BTreeMap::new(),
            config,
            window: VecDeque::new(),
            csv,
            last_log_ns: 0,
            totals_us: Vec::new(),
            phase_sum: [0; PHASES],
            count_sum: [0; COUNTERS],
            worst: Vec::new(),
            interval: Interval::default(),
        }
    }

    fn publish_ready(&mut self) {
        while self
            .pending
            .first_key_value()
            .is_some_and(|(_, p)| p.game.is_some() && p.expected == p.received)
        {
            let (_, p) = self.pending.pop_first().unwrap();
            let mut s = p.game.unwrap();
            for (a, n) in s.phase_ns.iter_mut().zip(p.worker.phase_ns) {
                *a += n;
            }
            for (a, n) in s.counts.iter_mut().zip(p.worker.counts) {
                *a += n;
            }
            let now = s.end_ns;
            self.push(s);
            if let Some(every) = self.config.log_every_ns
                && now.saturating_sub(self.last_log_ns) >= every
            {
                self.last_log_ns = now;
                if let Some(sum) = self.window_summary() {
                    eprintln!("frame stats: {}", sum.line());
                }
            }
        }
    }

    pub fn push(&mut self, s: FrameSample) {
        if let Some(f) = self.csv.as_mut() {
            let phases: Vec<String> = s.phase_ns.iter().map(|n| (n / 1000).to_string()).collect();
            let counts: Vec<String> = s.counts.iter().map(u64::to_string).collect();
            let _ = writeln!(
                f,
                "{},{:.3},{},{},{}",
                s.index,
                s.end_ns as f64 / 1e6,
                s.total_ns / 1000,
                phases.join(","),
                counts.join(",")
            );
        }
        self.window.push_back(s);
        self.interval.add(&s);
        let window_ns = if self.config.window_ns == 0 {
            Config::DEFAULT_WINDOW_NS
        } else {
            self.config.window_ns
        };
        while self.window.len() > WINDOW_MAX
            || self
                .window
                .front()
                .is_some_and(|f| s.end_ns.saturating_sub(f.end_ns) > window_ns)
        {
            self.window.pop_front();
        }
        if s.index < self.config.report_from {
            return;
        }
        self.totals_us
            .push((s.total_ns / 1000).min(u32::MAX as u64) as u32);
        for (a, b) in self.phase_sum.iter_mut().zip(s.phase_ns) {
            *a += b;
        }
        for (a, b) in self.count_sum.iter_mut().zip(s.counts) {
            *a += b;
        }
        let keep = self.config.worst.max(1);
        if self.worst.len() < keep || self.worst.last().is_some_and(|w| s.total_ns > w.total_ns) {
            let at = self.worst.partition_point(|w| w.total_ns >= s.total_ns);
            self.worst.insert(at, s);
            self.worst.truncate(keep);
        }
    }

    pub fn window_summary(&self) -> Option<Summary> {
        Summary::of(self.window.iter())
    }

    /// The whole run from `report_from`: percentiles, mean phases and the
    /// worst frames with what happened in them.
    pub fn report(&self) -> String {
        let n = self.totals_us.len();
        if n == 0 {
            return "frame stats: no frames".into();
        }
        let mut sorted = self.totals_us.clone();
        sorted.sort_unstable();
        let wall: u64 = sorted.iter().map(|&v| v as u64).sum();
        let ms = |us: u64| us as f64 / 1000.0;
        let mut out = format!(
            "frame stats: {n} frames from {}, {:.2} fps; frame ms p50 {:.1} p90 {:.1} p99 {:.1} max {:.1}; over 33.3 ms: {}\n",
            self.config.report_from,
            n as f64 * 1e6 / wall.max(1) as f64,
            ms(percentile(&sorted, 50.0) as u64),
            ms(percentile(&sorted, 90.0) as u64),
            ms(percentile(&sorted, 99.0) as u64),
            ms(*sorted.last().unwrap() as u64),
            sorted.iter().filter(|&&v| v > 33_333).count(),
        );
        out += "  mean ms/frame:";
        let shown = |p: Phase| p != Phase::Render || self.phase_sum[p as usize] != 0;
        for p in Phase::ALL.into_iter().filter(|&p| shown(p)) {
            out += &format!(
                " {} {:.2}",
                p.label(),
                self.phase_sum[p as usize] as f64 / 1e6 / n as f64
            );
        }
        out += "\n  mean per frame:";
        for c in Counter::ALL {
            out += &format!(
                " {} {:.1}",
                c.label(),
                self.count_sum[c as usize] as f64 / n as f64
            );
        }
        out += "\n  worst frames:\n";
        for w in &self.worst {
            out += &format!("    #{} {:.1} ms:", w.index, w.total_ns as f64 / 1e6);
            for p in Phase::ALL.into_iter().filter(|&p| shown(p)) {
                out += &format!(" {} {:.1}", p.label(), w.phase_ns[p as usize] as f64 / 1e6);
            }
            out += " |";
            for c in Counter::ALL {
                out += &format!(" {} {}", c.label(), w.counts[c as usize]);
            }
            out += "\n";
        }
        out
    }
}

/// Nearest-rank percentile of sorted values.
fn percentile<T: Copy>(sorted: &[T], p: f64) -> T {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// A set of frames summarised.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub frames: usize,
    /// Frames per wall-clock second over the window.
    pub fps: f64,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    /// Mean ms per frame in each phase.
    pub phase_ms: [f64; PHASES],
    /// Mean per frame of each [`Counter`].
    pub count_mean: [f64; COUNTERS],
}

impl Summary {
    pub fn of<'a>(frames: impl Iterator<Item = &'a FrameSample>) -> Option<Summary> {
        let frames: Vec<&FrameSample> = frames.collect();
        if frames.is_empty() {
            return None;
        }
        let mut totals: Vec<u64> = frames.iter().map(|f| f.total_ns).collect();
        totals.sort_unstable();
        let wall: u64 = totals.iter().sum();
        let n = frames.len() as f64;
        let mut phase_ms = [0.0; PHASES];
        let mut count_mean = [0.0; COUNTERS];
        for f in &frames {
            for (a, b) in phase_ms.iter_mut().zip(f.phase_ns) {
                *a += b as f64 / 1e6 / n;
            }
            for (a, b) in count_mean.iter_mut().zip(f.counts) {
                *a += b as f64 / n;
            }
        }
        Some(Summary {
            frames: frames.len(),
            fps: n * 1e9 / wall.max(1) as f64,
            p50_ms: percentile(&totals, 50.0) as f64 / 1e6,
            p99_ms: percentile(&totals, 99.0) as f64 / 1e6,
            max_ms: *totals.last().unwrap() as f64 / 1e6,
            phase_ms,
            count_mean,
        })
    }

    /// One line, under the overlay's 96 characters:
    /// `14.8 fps p50 61 p99 98 max 140 ms | cpu 40 dec 3 enc 8 sub 2 gpu 5 pace 0 rt 0`,
    /// with `ren` and `d3d` after `cpu` while the render front-end is being
    /// timed (otherwise `cpu` includes `d3d`).
    pub fn short_line(&self) -> String {
        let p = &self.phase_ms;
        let render = p[Phase::Render as usize];
        let d3d = p[Phase::D3d as usize];
        // `cpu` keeps its old meaning (guest code and every HLE call)
        // unless the render front-end is being timed.
        let (cpu, ren) = if render > 0.0 {
            (p[0], format!(" ren {render:.0} d3d {d3d:.0}"))
        } else {
            (p[0] + d3d, String::new())
        };
        format!(
            "{:.1} fps p50 {:.0} p99 {:.0} max {:.0} ms | cpu {:.0}{ren} dec {:.0} enc {:.0} sub {:.0} gpu {:.0} pace {:.0} rt {:.0}",
            self.fps,
            self.p50_ms,
            self.p99_ms,
            self.max_ms,
            cpu,
            p[1],
            p[2],
            p[3],
            p[4],
            p[5],
            p[6]
        )
    }

    /// The log line.
    pub fn line(&self) -> String {
        let mut s = format!(
            "{:.2} fps over {} frames; frame ms p50 {:.1} p99 {:.1} max {:.1}; mean ms",
            self.fps, self.frames, self.p50_ms, self.p99_ms, self.max_ms
        );
        // `render` only while the render front-end is being timed.
        for p in Phase::ALL
            .into_iter()
            .filter(|&p| p != Phase::Render || self.phase_ms[p as usize] > 0.0)
        {
            s += &format!(" {} {:.1}", p.label(), self.phase_ms[p as usize]);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(index: u64, end_ms: u64, total_ms: u64) -> FrameSample {
        FrameSample {
            index,
            end_ns: end_ms * 1_000_000,
            total_ns: total_ms * 1_000_000,
            phase_ns: [total_ms * 1_000_000, 0, 0, 0, 0, 0, 0, 0, 0],
            counts: [0; COUNTERS],
        }
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let v: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&v, 50.0), 50);
        assert_eq!(percentile(&v, 99.0), 99);
        assert_eq!(percentile(&v, 100.0), 100);
        assert_eq!(percentile(&[7u64], 50.0), 7);
    }

    #[test]
    fn summary_of_steady_frames_with_one_hitch() {
        let mut frames: Vec<FrameSample> = (0..99).map(|i| frame(i, i * 20, 20)).collect();
        frames.push(frame(99, 2100, 120));
        let s = Summary::of(frames.iter()).unwrap();
        assert_eq!(s.frames, 100);
        assert!((s.fps - 100.0 * 1000.0 / 2100.0).abs() < 1e-9, "{}", s.fps);
        assert_eq!(s.p50_ms, 20.0);
        assert_eq!(s.p99_ms, 20.0);
        assert_eq!(s.max_ms, 120.0);
        assert!((s.phase_ms[0] - 21.0).abs() < 1e-9);
        assert!(s.short_line().len() <= 96, "{}", s.short_line());
        assert!(Summary::of([].iter()).is_none());
    }

    #[test]
    fn the_interval_keeps_sums_and_the_worst_frame_of_each_phase() {
        let mut r = Recorder::new(Config::default(), None);
        for (i, (cpu, wait)) in [(100u64, 5u64), (140, 1), (120, 30)]
            .into_iter()
            .enumerate()
        {
            let mut f = frame(i as u64, i as u64 * 150, 150);
            f.phase_ns[Phase::Guest as usize] = cpu * 1_000_000;
            f.phase_ns[Phase::RenderWait as usize] = wait * 1_000_000;
            f.counts[Counter::Draws as usize] = 300;
            r.push(f);
        }
        let iv = std::mem::take(&mut r.interval);
        assert_eq!(iv.frames, 3);
        assert!((iv.avg_ms(Phase::Guest) - 120.0).abs() < 1e-9);
        assert_eq!(iv.max_ms(Phase::Guest), 140.0);
        assert_eq!(iv.max_ms(Phase::RenderWait), 30.0);
        assert!((iv.avg_ms(Phase::RenderWait) - 12.0).abs() < 1e-9);
        assert_eq!(iv.per_frame(Counter::Draws), 300.0);
        assert_eq!(iv.total_max_ns, 150_000_000);
        assert_eq!(r.interval, Interval::default(), "taken");
        assert_eq!(Interval::default().avg_ms(Phase::Guest), 0.0);
    }

    #[test]
    fn window_drops_old_frames_and_report_keeps_the_worst() {
        let config = Config {
            window_ns: 1_000_000_000,
            worst: 3,
            report_from: 2,
            ..Default::default()
        };
        let mut r = Recorder::new(config, None);
        for i in 0..100u64 {
            // Frame 50 is the hitch; frames 0 and 1 are before the report.
            let total = if i == 50 {
                200
            } else if i < 2 {
                500
            } else {
                30 + i % 3
            };
            r.push(frame(i, i * 40, total));
        }
        let w = r.window_summary().unwrap();
        // Frames ending within 1 s of the last (3960 ms): 2960..=3960.
        assert_eq!(w.frames, 26);
        assert_eq!(w.max_ms, 32.0);
        let worst: Vec<u64> = r.worst.iter().map(|f| f.index).collect();
        assert_eq!(worst[0], 50);
        assert_eq!(worst.len(), 3);
        assert!(r.worst.iter().all(|f| f.index >= 2));
        let report = r.report();
        assert!(report.contains("98 frames from 2"), "{report}");
        assert!(report.contains("#50 200.0 ms"), "{report}");
        assert!(report.contains("over 33.3 ms: 1"), "{report}");
    }

    #[test]
    fn delayed_worker_samples_preserve_frame_order_and_wall_time() {
        let mut r = Recorder::new(Config::default(), None);
        let mut game = frame(1, 20, 20);
        game.phase_ns[Phase::Guest as usize] = 15_000_000;
        game.phase_ns[Phase::RenderWait as usize] = 5_000_000;
        r.pending.insert(
            1,
            PendingFrame {
                game: Some(game),
                expected: 1,
                ..Default::default()
            },
        );
        let mut worker = FrameSample::default();
        worker.phase_ns[Phase::Encode as usize] = 8_000_000;
        worker.phase_ns[Phase::Submit as usize] = 2_000_000;
        worker.phase_ns[Phase::Wait as usize] = 6_000_000;
        worker.counts[Counter::Draws as usize] = 1500;
        r.pending.insert(
            2,
            PendingFrame {
                game: Some(frame(2, 50, 30)),
                worker,
                expected: 1,
                received: 1,
            },
        );
        r.publish_ready();
        assert!(
            r.window.is_empty(),
            "later frames wait for the earlier sample"
        );
        let first = r.pending.get_mut(&1).unwrap();
        first.worker = worker;
        first.received = 1;
        r.publish_ready();
        assert_eq!(r.window.iter().map(|s| s.index).collect::<Vec<_>>(), [1, 2]);
        let first = r.window.front().unwrap();
        assert_eq!(first.total_ns, 20_000_000);
        assert_eq!(first.phase_ns[Phase::Guest as usize], 15_000_000);
        assert_eq!(first.phase_ns[Phase::RenderWait as usize], 5_000_000);
        assert_eq!(first.phase_ns[Phase::Encode as usize], 8_000_000);
        assert_eq!(first.counts[Counter::Draws as usize], 1500);
        assert_eq!(r.window_summary().unwrap().fps, 40.0);
        r.publish_ready();
        assert_eq!(r.window.len(), 2, "publish each frame once");
    }

    #[test]
    fn worker_sample_can_arrive_before_the_game_frame() {
        let mut r = Recorder::new(Config::default(), None);
        let mut worker = FrameSample::default();
        worker.counts[Counter::Uploads as usize] = 3;
        r.pending.insert(
            1,
            PendingFrame {
                worker,
                expected: 1,
                received: 1,
                ..Default::default()
            },
        );
        r.publish_ready();
        assert!(r.window.is_empty());
        let mut game = frame(1, 20, 20);
        game.counts[Counter::ReadBytes as usize] = 123;
        r.pending.get_mut(&1).unwrap().game = Some(game);
        r.publish_ready();
        let first = r.window.front().unwrap();
        assert_eq!(first.counts[Counter::Uploads as usize], 3);
        assert_eq!(first.counts[Counter::ReadBytes as usize], 123);
        assert!(r.pending.is_empty());
    }

    #[test]
    fn nested_phases_are_self_time_and_frames_add_up() {
        // The only test that turns the global collection on.
        let csv = std::env::temp_dir().join(format!("h2-frame-stats-{}.csv", std::process::id()));
        start(Config {
            csv: Some(csv.clone()),
            worst: 4,
            ..Default::default()
        })
        .unwrap();
        frame_end(0); // starts this thread's clock
        let spin = |ms: u64| {
            let t = std::time::Instant::now();
            while t.elapsed() < std::time::Duration::from_millis(ms) {}
        };
        spin(2);
        {
            let _d = enter(Phase::Decode);
            spin(3);
            {
                let _e = enter(Phase::Encode);
                count(Counter::Draws, 2);
                spin(4);
                let _s = enter(Phase::Submit);
                count(Counter::Submits, 1);
                spin(1);
            }
            spin(1);
        }
        frame_end(1);
        let w = window_summary().unwrap();
        let report = stop().unwrap();
        assert!(!enabled());
        assert_eq!(w.frames, 1);
        let p = w.phase_ms;
        let ms = |v: f64, want: f64| v >= want && v < want + 3.0;
        assert!(
            ms(p[0], 2.0) && ms(p[1], 4.0) && ms(p[2], 4.0) && ms(p[3], 1.0),
            "{p:?}"
        );
        let sum: f64 = p.iter().sum();
        assert!(
            (sum - w.max_ms).abs() < 0.01,
            "phases {sum} vs frame {}",
            w.max_ms
        );
        assert!(report.contains("draws 2 submits 1"), "{report}");
        assert_eq!(w.count_mean[Counter::Draws as usize], 2.0);
        let text = std::fs::read_to_string(&csv).unwrap();
        let _ = std::fs::remove_file(&csv);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].starts_with("frame,end_ms,total_us,cpu_us,decode_us"));
        assert!(lines[1].starts_with("1,"));
        // Off again: guards are inert and nothing is counted.
        let g = enter(Phase::Wait);
        assert_eq!(g.prev, INERT);
        drop(g);
        // A live collection (the watermark) keeps a window but no report.
        start(Config::live(1_000_000_000)).unwrap();
        frame_end(2); // a new collection restarts this thread's clock
        count(Counter::Draws, 7);
        frame_end(3);
        let w = window_summary().unwrap();
        assert_eq!(w.frames, 1);
        assert_eq!(w.count_mean[Counter::Draws as usize], 7.0);
        // Guest scopes: D3D entries inside the render scope are scoped D3d
        // time, outside it or on another guest thread they are not.
        guest_thread(Some(1));
        {
            let _d = enter(Phase::D3d);
            spin(2);
        }
        assert_eq!(scope_ns(Phase::D3d), Some(0));
        assert_eq!(scope_entries(Phase::D3d), Some(0));
        scope_begin(Phase::Render);
        spin(2);
        {
            let _d = enter(Phase::D3d);
            spin(3);
        }
        guest_thread(Some(2));
        {
            let _d = enter(Phase::D3d);
            spin(2);
        }
        guest_thread(Some(1));
        scope_end();
        let (render, d3d) = (
            scope_ns(Phase::Render).unwrap(),
            scope_ns(Phase::D3d).unwrap(),
        );
        assert!((2_000_000..4_000_000).contains(&render), "{render}");
        assert!((3_000_000..5_000_000).contains(&d3d), "{d3d}");
        assert!(thread_ns(Phase::D3d).unwrap() >= 7_000_000);
        assert_eq!(scope_entries(Phase::D3d), Some(1), "only the scoped entry");
        guest_thread(None);
        assert_eq!(stop(), None);
        assert!(!enabled());
    }
}
