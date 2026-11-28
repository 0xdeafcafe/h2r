//! Host waits and thread CPU time for the host perf line (`H2_HOST_PERF`,
//! `boot::host_perf`; docs/runtime/xbox.md "Where frame time goes").
//!
//! [`phase`](crate::phase) splits each guest frame on the game and render
//! threads. What it cannot see is the host around them: the shell's UI
//! thread acquiring and presenting the window's swap chain, the guest
//! thread's pacing sleeps (and how much longer than asked they took, which
//! is the host timer's granularity), and how much CPU each thread really
//! got against the wall time it took. These are process-wide atomics: each
//! [`Gauge`] keeps a count, a sum and a maximum, and a reader takes and
//! clears them once an interval.
//!
//! Cost while off: one relaxed load and a branch per hook. While on: two
//! clock reads per timed call and three relaxed atomic adds.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static ON: AtomicBool = AtomicBool::new(false);

/// True while the host perf line is being collected. One relaxed load.
#[inline(always)]
pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Turns collection on or off. Gauges keep what they hold; [`Gauge::take`]
/// clears them.
pub fn set_enabled(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// Count, sum and maximum of one kind of host wait, in nanoseconds.
#[derive(Debug)]
pub struct Gauge {
    n: AtomicU64,
    sum_ns: AtomicU64,
    max_ns: AtomicU64,
}

/// What a [`Gauge`] held when it was taken.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Taken {
    pub n: u64,
    pub sum_ns: u64,
    pub max_ns: u64,
}

impl Taken {
    /// Mean per event, in ms (0 with no events).
    pub fn avg_ms(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.sum_ns as f64 / self.n as f64 / 1e6
        }
    }
    pub fn max_ms(&self) -> f64 {
        self.max_ns as f64 / 1e6
    }
    pub fn sum_ms(&self) -> f64 {
        self.sum_ns as f64 / 1e6
    }
}

impl Gauge {
    pub const fn new() -> Self {
        Gauge {
            n: AtomicU64::new(0),
            sum_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
        }
    }

    /// Adds one event of `ns`, while collection is on.
    #[inline]
    pub fn record(&self, ns: u64) {
        if enabled() {
            self.record_enabled(ns);
        }
    }

    fn record_enabled(&self, ns: u64) {
        self.n.fetch_add(1, Ordering::Relaxed);
        self.sum_ns.fetch_add(ns, Ordering::Relaxed);
        self.max_ns.fetch_max(ns, Ordering::Relaxed);
    }

    fn reading(&self) -> Taken {
        Taken {
            n: self.n.load(Ordering::Relaxed),
            sum_ns: self.sum_ns.load(Ordering::Relaxed),
            max_ns: self.max_ns.load(Ordering::Relaxed),
        }
    }

    /// Runs `f`, recording its wall time while collection is on.
    #[inline]
    pub fn time<R>(&self, f: impl FnOnce() -> R) -> R {
        if !enabled() {
            return f();
        }
        let t = Instant::now();
        let r = f();
        self.record(t.elapsed().as_nanos() as u64);
        r
    }

    /// What it holds since the last take; clears it.
    pub fn take(&self) -> Taken {
        Taken {
            n: self.n.swap(0, Ordering::Relaxed),
            sum_ns: self.sum_ns.swap(0, Ordering::Relaxed),
            max_ns: self.max_ns.swap(0, Ordering::Relaxed),
        }
    }
}

impl Default for Gauge {
    fn default() -> Self {
        Self::new()
    }
}

static SUBSYSTEM_AUDIT: AtomicBool = AtomicBool::new(false);

/// Opt in independently of the frame-based host perf line.
pub fn set_subsystem_audit(enabled: bool) {
    SUBSYSTEM_AUDIT.store(enabled, Ordering::Relaxed);
}

#[derive(Clone, Copy)]
pub enum Subsystem {
    Bots,
    Telemetry,
    TelemetryWorker,
}

impl Subsystem {
    fn gauge(self) -> &'static Gauge {
        static BOTS: Gauge = Gauge::new();
        static TELEMETRY: Gauge = Gauge::new();
        static TELEMETRY_WORKER: Gauge = Gauge::new();
        match self {
            Self::Bots => &BOTS,
            Self::Telemetry => &TELEMETRY,
            Self::TelemetryWorker => &TELEMETRY_WORKER,
        }
    }

    pub fn span(self) -> SubsystemSpan {
        let start = if SUBSYSTEM_AUDIT.load(Ordering::Relaxed) {
            thread_cpu_ns()
        } else {
            None
        };
        SubsystemSpan { subsystem: self, start }
    }

    pub fn reading(self) -> Taken {
        self.gauge().reading()
    }
}

pub struct SubsystemSpan {
    subsystem: Subsystem,
    start: Option<u64>,
}

impl Drop for SubsystemSpan {
    fn drop(&mut self) {
        if let Some(start) = self.start
            && let Some(end) = thread_cpu_ns()
        {
            self.subsystem.gauge().record_enabled(end.saturating_sub(start));
        }
    }
}

/// The UI thread's swap chain acquire (`get_current_texture`, with any
/// reconfigure). With a vsync'd swap chain this is where the UI thread
/// waits for the display.
pub static ACQUIRE: Gauge = Gauge::new();
/// Acquires that gave no texture (timeout, occluded, outdated): the UI
/// loop then draws nothing and goes round again.
pub static ACQUIRE_MISSED: Gauge = Gauge::new();
/// The UI thread's `queue.submit` of the compositor's pass.
pub static UI_SUBMIT: Gauge = Gauge::new();
/// The UI thread's `queue.present`. On DX12 the present itself can block
/// for a vblank while wgpu holds the queue's submit lock.
pub static PRESENT: Gauge = Gauge::new();
/// The guest thread's pacing sleeps (the shell's `Pacer`), as taken.
pub static SLEEP: Gauge = Gauge::new();
/// How much longer each pacing sleep took than it asked for: the host
/// timer's granularity (about 15.6 ms on a Windows timer at its default
/// rate, well under 1 ms on macOS).
pub static OVERSLEEP: Gauge = Gauge::new();
/// The guest thread's input pump between slices (`xapi::service_input`).
pub static INPUT: Gauge = Gauge::new();

/// A thread whose CPU time the line reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Role {
    /// The guest's runner thread (generated code, kernel, HLE, d3d).
    Game = 0,
    /// The render thread (`native_render::FrameOwner`).
    Render = 1,
    /// The shell's UI thread (window events, compositor, present).
    Ui = 2,
}

/// Number of [`Role`]s.
pub const ROLES: usize = 3;

/// Each role's thread CPU time at its last [`sample_thread`], ns, plus one
/// (zero: never sampled).
static THREAD_CPU: [AtomicU64; ROLES] = [const { AtomicU64::new(0) }; ROLES];

/// Records the calling thread's CPU time as `role`'s, while collection is
/// on. Called by each thread at a point it passes once a frame.
#[inline]
pub fn sample_thread(role: Role) {
    if enabled()
        && let Some(ns) = thread_cpu_ns()
    {
        THREAD_CPU[role as usize].store(ns + 1, Ordering::Relaxed);
    }
}

/// `role`'s thread CPU time at its last [`sample_thread`], if any.
pub fn thread_cpu(role: Role) -> Option<u64> {
    THREAD_CPU[role as usize]
        .load(Ordering::Relaxed)
        .checked_sub(1)
}

/// The calling thread's CPU time (user and kernel), in ns.
///
/// On Windows this is `GetThreadTimes`, which the scheduler updates at its
/// clock tick: read it over many frames, not one.
pub fn thread_cpu_ns() -> Option<u64> {
    sys::thread_cpu_ns()
}

/// The kernel-mode part of [`thread_cpu_ns`] (system calls, page faults),
/// where the host reports it apart: Windows only.
pub fn thread_kernel_ns() -> Option<u64> {
    sys::thread_kernel_ns()
}

/// The whole process's CPU time (user and kernel, every thread), in ns.
pub fn process_cpu_ns() -> Option<u64> {
    sys::process_cpu_ns()
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod sys {
    // kernel32, in OneCoreUAP.lib (docs/runtime/xbox.md, the API check).
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn GetCurrentProcess() -> isize;
        fn GetThreadTimes(h: isize, c: *mut u64, e: *mut u64, k: *mut u64, u: *mut u64) -> i32;
        fn GetProcessTimes(h: isize, c: *mut u64, e: *mut u64, k: *mut u64, u: *mut u64) -> i32;
    }

    pub fn thread_cpu_ns() -> Option<u64> {
        let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
        // SAFETY: the pseudo handle needs no closing; four valid FILETIMEs
        // (a FILETIME is two u32s, which a u64 holds with room to spare
        // for its alignment).
        let ok = unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) };
        (ok != 0).then(|| (k + u) * 100)
    }

    pub fn thread_kernel_ns() -> Option<u64> {
        let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
        // SAFETY: as above.
        let ok = unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) };
        (ok != 0).then(|| k * 100)
    }

    pub fn process_cpu_ns() -> Option<u64> {
        let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
        // SAFETY: as above.
        let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
        (ok != 0).then(|| (k + u) * 100)
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod sys {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }

    unsafe extern "C" {
        fn clock_gettime(clock: i32, t: *mut Timespec) -> i32;
    }

    #[cfg(target_vendor = "apple")]
    const THREAD: i32 = 16;
    #[cfg(target_vendor = "apple")]
    const PROCESS: i32 = 12;
    #[cfg(not(target_vendor = "apple"))]
    const THREAD: i32 = 3;
    #[cfg(not(target_vendor = "apple"))]
    const PROCESS: i32 = 2;

    fn read(clock: i32) -> Option<u64> {
        let mut t = Timespec { sec: 0, nsec: 0 };
        // SAFETY: a valid timespec for the call to fill.
        (unsafe { clock_gettime(clock, &mut t) } == 0)
            .then(|| t.sec as u64 * 1_000_000_000 + t.nsec as u64)
    }

    pub fn thread_cpu_ns() -> Option<u64> {
        read(THREAD)
    }

    pub fn process_cpu_ns() -> Option<u64> {
        read(PROCESS)
    }

    pub fn thread_kernel_ns() -> Option<u64> {
        None
    }
}

#[cfg(not(any(windows, unix)))]
mod sys {
    pub fn thread_cpu_ns() -> Option<u64> {
        None
    }
    pub fn process_cpu_ns() -> Option<u64> {
        None
    }
    pub fn thread_kernel_ns() -> Option<u64> {
        None
    }
}

/// How long the host's timed waits really take when asked for 1 ms: the
/// smallest and largest of `rounds` condition-variable timeouts (the
/// pacer's sleep) and `rounds` `thread::sleep`s. A Windows timer at its
/// default rate rounds both up to its 15.6 ms tick (std's `thread::sleep`
/// uses a high-resolution waitable timer where Windows has one, so it may
/// not); macOS and Linux take about 1.1 ms.
pub fn timer_probe(rounds: usize) -> TimerProbe {
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;
    let ask = Duration::from_millis(1);
    let pair = (Mutex::new(()), Condvar::new());
    let mut condvar = (f64::MAX, 0.0f64);
    let mut sleep = (f64::MAX, 0.0f64);
    for _ in 0..rounds.max(1) {
        let g = pair.0.lock().unwrap_or_else(|e| e.into_inner());
        let t = Instant::now();
        // Spurious wakes are waited out: the whole timeout is measured.
        let _ = pair.1.wait_timeout_while(g, ask, |_| true);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        condvar = (condvar.0.min(ms), condvar.1.max(ms));
        let t = Instant::now();
        std::thread::sleep(ask);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        sleep = (sleep.0.min(ms), sleep.1.max(ms));
    }
    TimerProbe {
        condvar_ms: condvar,
        sleep_ms: sleep,
    }
}

/// What [`timer_probe`] measured: (min, max) ms for a 1 ms wait.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimerProbe {
    pub condvar_ms: (f64, f64),
    pub sleep_ms: (f64, f64),
}

impl std::fmt::Display for TimerProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "a 1 ms condvar wait took {:.2}-{:.2} ms, a 1 ms thread::sleep {:.2}-{:.2} ms",
            self.condvar_ms.0, self.condvar_ms.1, self.sleep_ms.0, self.sleep_ms.1
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test touches the process-wide switch, so they cannot race.
    #[test]
    fn gauges_count_only_while_on_and_clear_when_taken() {
        let g = Gauge::new();
        set_enabled(false);
        g.record(5);
        assert_eq!(g.time(|| 7), 7);
        assert_eq!(g.take(), Taken::default());

        set_enabled(true);
        g.record(2_000_000);
        g.record(6_000_000);
        g.record(1_000_000);
        let t = g.take();
        assert_eq!(
            t,
            Taken {
                n: 3,
                sum_ns: 9_000_000,
                max_ns: 6_000_000
            }
        );
        assert!((t.avg_ms() - 3.0).abs() < 1e-9);
        assert!((t.max_ms() - 6.0).abs() < 1e-9);
        assert_eq!(g.take(), Taken::default(), "taking clears");
        assert_eq!(g.time(|| 1), 1);
        assert_eq!(g.take().n, 1);

        // A thread's CPU time is recorded under its role while on.
        sample_thread(Role::Ui);
        assert!(thread_cpu(Role::Ui).is_some());
        set_enabled(false);
        assert_eq!(Taken::default().avg_ms(), 0.0);
    }

    #[test]
    fn thread_cpu_time_moves_with_work_and_not_with_sleep() {
        let Some(a) = thread_cpu_ns() else { return };
        let mut x = 0u64;
        let t = Instant::now();
        while t.elapsed().as_millis() < 30 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(x);
        let b = thread_cpu_ns().unwrap();
        assert!(
            b - a >= 10_000_000,
            "30 ms of spinning is mostly CPU: {}",
            b - a
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
        let c = thread_cpu_ns().unwrap();
        assert!(c - b < 15_000_000, "a sleep is not CPU: {}", c - b);
        assert!(process_cpu_ns().unwrap() >= c - a);
    }

    #[test]
    fn the_timer_probe_measures_at_least_what_it_asks() {
        let p = timer_probe(2);
        assert!(p.condvar_ms.0 >= 0.9 && p.sleep_ms.0 >= 0.9, "{p}");
        assert!(p.condvar_ms.0 <= p.condvar_ms.1);
        assert!(p.to_string().starts_with("a 1 ms condvar wait took "));
    }
}
