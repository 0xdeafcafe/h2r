//! Presents: one per frame the frame step seals
//! (`render_bridge::frame_wait::swapped`), or per frame sim mode skips.
//! Timelines keyed on frames (`H2_BOOT_INPUT`, `H2_BOOT_STOP_AT_PRESENT`,
//! the agent's stamps) count these, and the run report's frame rate comes
//! from their times.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static PRESENTS: AtomicU64 = AtomicU64::new(0);
static KEEP_HISTORY: AtomicBool = AtomicBool::new(true);
static TIMES: Mutex<History> = Mutex::new(History { recorded: 0, times: VecDeque::new() });

const HISTORY_LIMIT: usize = 65_536;

#[derive(Default)]
struct History {
    recorded: usize,
    times: VecDeque<Instant>,
}

impl History {
    fn push(&mut self, now: Instant) {
        if self.times.len() == HISTORY_LIMIT {
            self.times.pop_front();
        }
        self.times.push_back(now);
        self.recorded += 1;
    }

    fn hundred_frame_rate(&self, now: Instant) -> Option<f64> {
        let first = self.times.get(self.times.len().checked_sub(100)?)?;
        Some(100.0 / now.duration_since(*first).as_secs_f64())
    }
}

/// Counts presents from the frame step's swaps.
pub fn install() {
    render_bridge::frame_wait::on_swap(present);
}

/// Presents so far.
pub fn presents() -> u64 {
    PRESENTS.load(Ordering::Relaxed)
}

/// One present: counted, timed, and handed to what keys on it.
pub fn present() {
    let n = PRESENTS.fetch_add(1, Ordering::Relaxed) + 1;
    if KEEP_HISTORY.load(Ordering::Relaxed) {
        let mut times = TIMES.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        // A heartbeat for long runs: where the run is, and how fast the last 100 went.
        if n.is_multiple_of(100) && let Some(fps) = times.hundred_frame_rate(now) {
            eprintln!("present {n}: last 100 at {fps:.2} fps");
        }
        times.push(now);
    }
    // H2_HOST_PERF: where the frame time went (host_perf.rs).
    crate::host_perf::on_present(n);
    // render.capture: the present after the tick asked for is its frame.
    agent::render::CAPTURE.present(n);
    #[cfg(feature = "gpu")]
    agent::render::clip::CLIPS.present(n);
}

/// Counts a present sim mode skipped with the world render
/// (`sim_bridge::sim_frame`), so what counts presents moves on as in a
/// rendered run. No frame time is kept for it.
pub fn note_skipped_present() {
    PRESENTS.fetch_add(1, Ordering::Relaxed);
}

/// Whether to keep present times for [`frame_rate`] (a light host keeps none).
pub fn keep_present_history(keep: bool) {
    KEEP_HISTORY.store(keep, Ordering::Relaxed);
    if !keep {
        *TIMES.lock().unwrap_or_else(|e| e.into_inner()) = History::default();
    }
}

/// Wall-clock frame rate: presents per second over the run since `start`,
/// and over a window with its mean, median and worst frame time: from
/// present `from` (1-based) when given, else the last `tail` intervals.
/// `None` before two presents.
pub fn frame_rate(start: Instant, tail: usize, from: Option<usize>) -> Option<String> {
    let history = TIMES.lock().unwrap_or_else(|e| e.into_inner());
    let n = history.recorded;
    let times = &history.times;
    let len = times.len();
    if len < 2 {
        return None;
    }
    let total = times[len - 1].duration_since(start).as_secs_f64();
    let first = n - len + 1;
    let from = match from {
        Some(f) => f.saturating_sub(first).min(len - 2),
        None => len.saturating_sub(tail.max(1).saturating_add(1)),
    };
    let mut gaps: Vec<f64> = (from + 1..len)
        .map(|i| times[i].duration_since(times[i - 1]).as_secs_f64() * 1e3)
        .collect();
    let span: f64 = gaps.iter().sum();
    gaps.sort_by(f64::total_cmp);
    Some(format!(
        "frames: {n} presents in {total:.1}s ({:.2} fps); presents {}-{n}: {:.2} fps, frame ms mean {:.0} median {:.0} max {:.0}",
        n as f64 / total,
        first + from,
        gaps.len() as f64 * 1e3 / span,
        span / gaps.len() as f64,
        gaps[gaps.len() / 2],
        gaps[gaps.len() - 1],
    ))
}
