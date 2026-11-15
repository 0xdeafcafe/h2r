//! Frame-time stats: where each frame's wall time goes, split into a few
//! phases, with a rolling window (fps, p50, p99, max) and a whole-run
//! report of the worst frames. `H2_FRAME_STATS=1` turns it on for `h2 run`
//! and `h2 play` (docs/frame-budget.md).
//!
//! Each thread has one current phase. [`enter`] switches it until the
//! returned guard drops, so nested phases are self time: a `Submit` inside
//! an `Encode` inside a `Decode` takes its time out of both. Time outside
//! any phase is [`Phase::Guest`]: generated code, the runtime, the kernel
//! and every HLE call that is not one of the others. [`frame_end`] (called
//! by [`crate::frame`], at every Swap) closes the frame on the calling
//! thread, which is the game thread: the D3D device, the push-buffer
//! decode and the wgpu backend all run there.
//!
//! Cost while off: one relaxed load and a branch per hook. While on: one
//! clock read per phase switch (a handful per draw) and, once a frame, a
//! push under a mutex. Nothing is recorded per event.

use std::cell::Cell;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Where a frame's time goes. Self time: nested phases do not overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Everything else on the game thread: guest code, runtime accessors,
    /// the kernel and scheduler, HLE calls (XAPI, DirectSound, file I/O,
    /// the D3D API entries that only write the push buffer).
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
}

/// Number of [`Phase`]s.
pub const PHASES: usize = 6;

impl Phase {
    pub const ALL: [Phase; PHASES] = [
        Phase::Guest,
        Phase::Decode,
        Phase::Encode,
        Phase::Submit,
        Phase::Wait,
        Phase::Pace,
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
    /// Render pipelines the game thread waited for or built when encoding
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
}

/// Number of [`Counter`]s.
pub const COUNTERS: usize = 7;

impl Counter {
    pub const ALL: [Counter; COUNTERS] = [
        Counter::Draws,
        Counter::Submits,
        Counter::Pipelines,
        Counter::Shaders,
        Counter::Uploads,
        Counter::Readbacks,
        Counter::ReadBytes,
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
        }
    }
}

static ON: AtomicBool = AtomicBool::new(false);

/// True while frame stats are being collected. One relaxed load.
#[inline(always)]
pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

struct Local {
    started: Cell<bool>,
    current: Cell<u8>,
    since: Cell<u64>,
    frame_start: Cell<u64>,
    acc: [Cell<u64>; PHASES],
    counts: [Cell<u64>; COUNTERS],
}

thread_local! {
    static LOCAL: Local = const {
        Local {
            started: Cell::new(false),
            current: Cell::new(Phase::Guest as u8),
            since: Cell::new(0),
            frame_start: Cell::new(0),
            acc: [const { Cell::new(0) }; PHASES],
            counts: [const { Cell::new(0) }; COUNTERS],
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
        let cur = l.current.get();
        let a = &l.acc[cur as usize];
        a.set(a.get() + now.saturating_sub(l.since.get()));
        l.since.set(now);
        l.current.set(to);
        cur
    })
}

fn start_local(l: &Local, now: u64) {
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
            let c = &l.counts[counter as usize];
            c.set(c.get() + n);
        });
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
        if !l.started.get() {
            start_local(l, now);
            return None;
        }
        let cur = l.current.get() as usize;
        l.acc[cur].set(l.acc[cur].get() + now.saturating_sub(l.since.get()));
        l.since.set(now);
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
            r.push(s);
            if let Some(every) = r.config.log_every_ns
                && now.saturating_sub(r.last_log_ns) >= every
            {
                r.last_log_ns = now;
                if let Some(sum) = r.window_summary() {
                    eprintln!("frame stats: {}", sum.line());
                }
            }
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
        })
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
    *global() = Some(Recorder::new(config, csv));
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

/// Stops collecting; returns the whole-run report.
pub fn stop() -> Option<String> {
    ON.store(false, Ordering::Relaxed);
    let mut r = global().take()?;
    if let Some(f) = r.csv.as_mut() {
        let _ = f.flush();
    }
    Some(r.report())
}

/// Summary of the rolling window, for an overlay.
pub fn window_summary() -> Option<Summary> {
    global().as_ref()?.window_summary()
}

static GLOBAL: Mutex<Option<Recorder>> = Mutex::new(None);

fn global() -> MutexGuard<'static, Option<Recorder>> {
    GLOBAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Frames collected: the rolling window and what the run report needs.
pub struct Recorder {
    config: Config,
    window: VecDeque<FrameSample>,
    csv: Option<std::io::BufWriter<std::fs::File>>,
    last_log_ns: u64,
    /// Whole run, from `report_from`: wall time of every frame, in us.
    totals_us: Vec<u32>,
    phase_sum: [u64; PHASES],
    count_sum: [u64; COUNTERS],
    worst: Vec<FrameSample>,
}

/// The window never holds more frames than this.
const WINDOW_MAX: usize = 4096;

impl Recorder {
    pub fn new(config: Config, csv: Option<std::io::BufWriter<std::fs::File>>) -> Self {
        Recorder {
            config,
            window: VecDeque::new(),
            csv,
            last_log_ns: 0,
            totals_us: Vec::new(),
            phase_sum: [0; PHASES],
            count_sum: [0; COUNTERS],
            worst: Vec::new(),
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
        for p in Phase::ALL {
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
            for p in Phase::ALL {
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
        for f in &frames {
            for (a, b) in phase_ms.iter_mut().zip(f.phase_ns) {
                *a += b as f64 / 1e6 / n;
            }
        }
        Some(Summary {
            frames: frames.len(),
            fps: n * 1e9 / wall.max(1) as f64,
            p50_ms: percentile(&totals, 50.0) as f64 / 1e6,
            p99_ms: percentile(&totals, 99.0) as f64 / 1e6,
            max_ms: *totals.last().unwrap() as f64 / 1e6,
            phase_ms,
        })
    }

    /// One line, under the overlay's 96 characters:
    /// `14.8 fps p50 61 p99 98 max 140 ms | cpu 40 dec 3 enc 8 sub 2 gpu 5 pace 0`.
    pub fn short_line(&self) -> String {
        let p = &self.phase_ms;
        format!(
            "{:.1} fps p50 {:.0} p99 {:.0} max {:.0} ms | cpu {:.0} dec {:.0} enc {:.0} sub {:.0} gpu {:.0} pace {:.0}",
            self.fps, self.p50_ms, self.p99_ms, self.max_ms, p[0], p[1], p[2], p[3], p[4], p[5]
        )
    }

    /// The log line.
    pub fn line(&self) -> String {
        let mut s = format!(
            "{:.2} fps over {} frames; frame ms p50 {:.1} p99 {:.1} max {:.1}; mean ms",
            self.fps, self.frames, self.p50_ms, self.p99_ms, self.max_ms
        );
        for p in Phase::ALL {
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
            phase_ns: [total_ms * 1_000_000, 0, 0, 0, 0, 0],
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
        let text = std::fs::read_to_string(&csv).unwrap();
        let _ = std::fs::remove_file(&csv);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].starts_with("frame,end_ms,total_us,cpu_us,decode_us"));
        assert!(lines[1].starts_with("1,"));
        // Off again: guards are inert and nothing is counted.
        let g = enter(Phase::Wait);
        assert_eq!(g.prev, INERT);
    }
}
