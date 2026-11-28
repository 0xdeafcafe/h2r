//! The host perf line: where a device's frame time goes, one line every
//! 100 guest frames or 5 s (docs/runtime/xbox.md, "Where frame time
//! goes"). `H2_HOST_PERF=1` turns it on for any host (`h2 run`, `h2 play`,
//! the Apple apps); the Xbox app sets it itself, since a UWP app gets no
//! environment (`uwp_host::switches`).
//!
//! It reuses what is there: frame stats ([`timeline::phase`]) split each
//! frame on the game and render threads, and [`timeline::host`] adds the
//! UI thread's swap-chain waits, the pacer's sleeps and each thread's CPU
//! time. This module takes both once an interval, with the fallback
//! interpreter's counters, and prints:
//!
//! ```text
//! host perf: frames 100 in 13.91 s, 7.19 fps; frame ms 139.1/152.0 | game cpu 118.2/131.0 d3d 4.1/6.0 decode 0.3/0.6 render-wait 9.0/30.1 pace 0.0/0.0 input 0.05/0.30 | render encode 21.0/25.2 submit 2.1/3.0 gpu-wait 70.2/80.1 | ui 60.0/s acquire 15.1/16.9 missed 0 submit 0.1/0.4 present 0.4/1.2 | sleeps 0 0.0 ms over 0.0/0.0 | fallback 2.0/frame, 310 insns/frame | draws 300 submits 26 read 1.2 KB/frame | cpu game 97% (kernel 3%) render 61% ui 9% process 180% of 6
//! ```
//!
//! Pairs are mean/max in ms. Per frame for the phases (`frame`, `game`,
//! `render`; the most one frame had for max), per call for the UI and
//! sleep waits. CPU is each thread's CPU time over the interval's wall
//! time: a thread at 100% is busy, one well under is waiting somewhere.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use timeline::host::{self, Role, Taken};
use timeline::phase::{self, Counter, Interval, Phase};

/// The switch.
pub const ENV: &str = "H2_HOST_PERF";
/// A line at least this often in frames...
pub const EVERY_FRAMES: u64 = 100;
/// ...or in wall time, whichever comes first.
pub const EVERY: Duration = Duration::from_secs(5);

/// Whether `value` (of [`ENV`]) asks for the line: set, and not `0`.
pub fn requested_by(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

/// One interval's readings at its end, for the next.
struct Mark {
    at: Instant,
    present: u64,
    game_cpu: Option<u64>,
    game_kernel: Option<u64>,
    render_cpu: Option<u64>,
    ui_cpu: Option<u64>,
    process_cpu: Option<u64>,
    fallback: (u64, u64),
}

struct State {
    render_thread: bool,
    cpus: usize,
    last: Mark,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn fallback_counts() -> (u64, u64) {
    crate::fallback::STATS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map_or((0, 0), |s| (s.fallbacks, s.interpreted))
}

fn mark(present: u64) -> Mark {
    Mark {
        at: Instant::now(),
        present,
        game_cpu: host::thread_cpu_ns(),
        game_kernel: host::thread_kernel_ns(),
        render_cpu: host::thread_cpu(Role::Render),
        ui_cpu: host::thread_cpu(Role::Ui),
        process_cpu: host::process_cpu_ns(),
        fallback: fallback_counts(),
    }
}

/// Starts the line if [`ENV`] asks for it, on the game thread before the
/// guest runs; frame stats too if nothing started them (quietly: no line
/// a second, but the exit report). Logs what the host runs with once.
pub fn start_from_env(render_thread: bool) -> bool {
    if !requested_by(std::env::var(ENV).ok().as_deref()) {
        return false;
    }
    start(render_thread);
    true
}

/// The frame stats host perf starts when nothing has: the environment's
/// (`H2_FRAME_STATS`, `H2_FRAME_STATS_CSV`, `H2_FRAME_STATS_FROM`) when it
/// asks for any, else quiet ones (no line a second, the exit report).
/// Host perf starts before the boot reads those variables itself, so a
/// default here used to drop the CSV and the report's first frame.
pub fn frame_stats_config(env: Option<phase::Config>) -> phase::Config {
    env.unwrap_or(phase::Config {
        report_from: 0,
        window_ns: phase::Config::DEFAULT_WINDOW_NS,
        worst: 10,
        ..Default::default()
    })
}

/// [`start_from_env`] without the switch.
pub fn start(render_thread: bool) {
    if !phase::enabled() {
        let config = frame_stats_config(phase::Config::from_env());
        if let Err(e) = phase::start(config) {
            eprintln!("host perf: frame stats: {e}");
        }
    }
    // Clear whatever an earlier run left.
    let _ = phase::take_interval();
    for g in gauges() {
        g.take();
    }
    host::set_enabled(true);
    let cpus = std::thread::available_parallelism().map_or(0, |n| n.get());
    eprintln!(
        "host perf: on ({ENV}), a line every {EVERY_FRAMES} frames or {} s; {cpus} logical CPUs; render thread {}; guest waits (kernel sleeps, timers, vblank) run on the logical clock, not host timers; host timer: {}",
        EVERY.as_secs(),
        if render_thread { "on" } else { "off" },
        host::timer_probe(5)
    );
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(State {
        render_thread,
        cpus,
        last: mark(crate::presents::presents()),
    });
}

/// Stops the line.
pub fn stop() {
    host::set_enabled(false);
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn gauges() -> [&'static host::Gauge; 7] {
    [
        &host::ACQUIRE,
        &host::ACQUIRE_MISSED,
        &host::UI_SUBMIT,
        &host::PRESENT,
        &host::SLEEP,
        &host::OVERSLEEP,
        &host::INPUT,
    ]
}

/// At guest present `n`, on the game thread: prints the line when an
/// interval is due. One relaxed load while off.
pub fn on_present(n: u64) {
    if !host::enabled() {
        return;
    }
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(st) = g.as_mut() else { return };
    if n.saturating_sub(st.last.present) < EVERY_FRAMES && st.last.at.elapsed() < EVERY {
        return;
    }
    let now = mark(n);
    let delta = |a: Option<u64>, b: Option<u64>| Some(a?.saturating_sub(b?));
    let r = Reading {
        secs: now.at.duration_since(st.last.at).as_secs_f64(),
        presents: n.saturating_sub(st.last.present),
        frames: phase::take_interval().unwrap_or_default(),
        acquire: host::ACQUIRE.take(),
        acquire_missed: host::ACQUIRE_MISSED.take(),
        ui_submit: host::UI_SUBMIT.take(),
        present: host::PRESENT.take(),
        sleep: host::SLEEP.take(),
        oversleep: host::OVERSLEEP.take(),
        input: host::INPUT.take(),
        fallbacks: now.fallback.0.saturating_sub(st.last.fallback.0),
        interpreted: now.fallback.1.saturating_sub(st.last.fallback.1),
        game_cpu_ns: delta(now.game_cpu, st.last.game_cpu),
        game_kernel_ns: delta(now.game_kernel, st.last.game_kernel),
        render_cpu_ns: delta(now.render_cpu, st.last.render_cpu),
        ui_cpu_ns: delta(now.ui_cpu, st.last.ui_cpu),
        process_cpu_ns: delta(now.process_cpu, st.last.process_cpu),
        render_thread: st.render_thread,
        cpus: st.cpus,
    };
    st.last = now;
    drop(g);
    let line = r.line();
    eprintln!("{line}");
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), line));
}

/// The newest line and when it was printed.
static LAST: Mutex<Option<(Instant, String)>> = Mutex::new(None);

/// The newest host perf line and how long ago it was printed (the agent
/// API's `host.perf`).
pub fn last_line() -> Option<(std::time::Duration, String)> {
    LAST.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|(at, l)| (at.elapsed(), l.clone()))
}

/// One interval, as read.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    pub secs: f64,
    /// Guest presents in the interval (frame stats publish render-thread
    /// frames a little later, so `frames.frames` may differ by one or two).
    pub presents: u64,
    pub frames: Interval,
    pub acquire: Taken,
    pub acquire_missed: Taken,
    pub ui_submit: Taken,
    pub present: Taken,
    pub sleep: Taken,
    pub oversleep: Taken,
    pub input: Taken,
    pub fallbacks: u64,
    pub interpreted: u64,
    pub game_cpu_ns: Option<u64>,
    /// Of `game_cpu_ns`, kernel mode (Windows only).
    pub game_kernel_ns: Option<u64>,
    pub render_cpu_ns: Option<u64>,
    pub ui_cpu_ns: Option<u64>,
    pub process_cpu_ns: Option<u64>,
    pub render_thread: bool,
    pub cpus: usize,
}

impl Reading {
    /// The line (see the module docs).
    pub fn line(&self) -> String {
        let f = &self.frames;
        let n = f.frames.max(1) as f64;
        let ph = |p: Phase| format!("{} {:.1}/{:.1}", p.label(), f.avg_ms(p), f.max_ms(p));
        let per_call =
            |name: &str, t: &Taken| format!("{name} {:.1}/{:.1}", t.avg_ms(), t.max_ms());
        let pct = |ns: Option<u64>| match ns {
            Some(ns) if self.secs > 0.0 => format!("{:.0}%", ns as f64 / 1e9 / self.secs * 100.0),
            _ => "?".into(),
        };
        let mut s = format!(
            "host perf: frames {} in {:.2} s, {:.2} fps; frame ms {:.1}/{:.1}",
            self.presents,
            self.secs,
            self.presents as f64 / self.secs.max(1e-9),
            f.total_sum_ns as f64 / 1e6 / n,
            f.total_max_ns as f64 / 1e6,
        );
        // The game thread's own phases; with the render thread the
        // backend's are that thread's, overlapping these.
        s += &format!(
            " | game {} {} {} {} {} input {:.2}/{:.2}",
            ph(Phase::Guest),
            ph(Phase::D3d),
            ph(Phase::Decode),
            ph(Phase::RenderWait),
            ph(Phase::Pace),
            self.input.sum_ms() / n,
            self.input.max_ms(),
        );
        if f.phase_sum_ns[Phase::Render as usize] != 0 {
            s += &format!(" {}", ph(Phase::Render));
        }
        s += &format!(
            " | {} {} {} {}",
            if self.render_thread {
                "render"
            } else {
                "backend (game thread)"
            },
            ph(Phase::Encode),
            ph(Phase::Submit),
            ph(Phase::Wait),
        );
        s += &format!(
            " | ui {:.1}/s {} missed {} {} {}",
            self.acquire.n as f64 / self.secs.max(1e-9),
            per_call("acquire", &self.acquire),
            self.acquire_missed.n,
            per_call("submit", &self.ui_submit),
            per_call("present", &self.present),
        );
        s += &format!(
            " | sleeps {} {:.1} ms over {:.1}/{:.1}",
            self.sleep.n,
            self.sleep.sum_ms(),
            self.oversleep.avg_ms(),
            self.oversleep.max_ms(),
        );
        s += &format!(
            " | fallback {:.1}/frame, {:.0} insns/frame",
            self.fallbacks as f64 / n,
            self.interpreted as f64 / n
        );
        s += &format!(
            " | draws {:.0} submits {:.0} read {:.1} KB/frame",
            f.per_frame(Counter::Draws),
            f.per_frame(Counter::Submits),
            f.per_frame(Counter::ReadBytes) / 1024.0
        );
        s += &format!(
            " | cpu game {}{} render {} ui {} process {} of {}",
            pct(self.game_cpu_ns),
            match self.game_kernel_ns {
                Some(_) => format!(" (kernel {})", pct(self.game_kernel_ns)),
                None => String::new(),
            },
            if self.render_thread {
                pct(self.render_cpu_ns)
            } else {
                "-".into()
            },
            pct(self.ui_cpu_ns),
            pct(self.process_cpu_ns),
            self.cpus
        );
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_perf_keeps_the_frame_stats_csv_and_first_frame() {
        let env = phase::Config {
            log_every_ns: Some(1_000_000_000),
            csv: Some("/tmp/frames.csv".into()),
            report_from: 4200,
            window_ns: phase::Config::DEFAULT_WINDOW_NS,
            worst: 10,
            quiet: false,
        };
        let c = frame_stats_config(Some(env));
        assert_eq!(c.csv.as_deref(), Some(std::path::Path::new("/tmp/frames.csv")));
        assert_eq!((c.report_from, c.log_every_ns), (4200, Some(1_000_000_000)));
        let quiet = frame_stats_config(None);
        assert_eq!((quiet.csv, quiet.report_from, quiet.log_every_ns), (None, 0, None));
    }

    #[test]
    fn the_switch_is_set_and_not_zero() {
        assert!(requested_by(Some("1")));
        assert!(requested_by(Some("yes")));
        assert!(!requested_by(Some("0")));
        assert!(!requested_by(Some("")));
        assert!(!requested_by(None));
    }

    fn ms(v: u64) -> u64 {
        v * 1_000_000
    }

    #[test]
    fn the_line_reads_phases_per_frame_waits_per_call_and_cpu_against_wall_time() {
        let mut frames = Interval::default();
        for (cpu, gpu) in [(100, 60), (140, 80)] {
            let mut s = phase::FrameSample {
                total_ns: ms(150),
                ..Default::default()
            };
            s.phase_ns[Phase::Guest as usize] = ms(cpu);
            s.phase_ns[Phase::Wait as usize] = ms(gpu);
            s.counts[Counter::Draws as usize] = 300;
            s.counts[Counter::ReadBytes as usize] = 2048;
            frames.add(&s);
        }
        let r = Reading {
            secs: 2.0,
            presents: 2,
            frames,
            acquire: Taken {
                n: 120,
                sum_ns: ms(1800),
                max_ns: ms(17),
            },
            present: Taken {
                n: 120,
                sum_ns: ms(60),
                max_ns: ms(2),
            },
            sleep: Taken {
                n: 2,
                sum_ns: ms(31),
                max_ns: ms(16),
            },
            oversleep: Taken {
                n: 2,
                sum_ns: ms(29),
                max_ns: ms(15),
            },
            input: Taken {
                n: 50,
                sum_ns: ms(1),
                max_ns: 100_000,
            },
            fallbacks: 6,
            interpreted: 900,
            game_cpu_ns: Some(ms(1900)),
            render_cpu_ns: Some(ms(1000)),
            ui_cpu_ns: None,
            process_cpu_ns: Some(ms(6000)),
            render_thread: true,
            cpus: 6,
            ..Default::default()
        };
        let line = r.line();
        for want in [
            "host perf: frames 2 in 2.00 s, 1.00 fps; frame ms 150.0/150.0",
            "| game cpu 120.0/140.0 d3d 0.0/0.0 decode 0.0/0.0 render-wait 0.0/0.0 pace 0.0/0.0 input 0.50/0.10 |",
            "| render encode 0.0/0.0 submit 0.0/0.0 gpu-wait 70.0/80.0 |",
            "| ui 60.0/s acquire 15.0/17.0 missed 0 submit 0.0/0.0 present 0.5/2.0 |",
            "| sleeps 2 31.0 ms over 14.5/15.0 |",
            "| fallback 3.0/frame, 450 insns/frame |",
            "| draws 300 submits 0 read 2.0 KB/frame |",
            "| cpu game 95% render 50% ui ? process 300% of 6",
        ] {
            assert!(line.contains(want), "{want}\n{line}");
        }
        assert!(
            !line.contains(" render 0.0/0.0"),
            "the render front-end only when timed: {line}"
        );
        let solo = Reading {
            render_thread: false,
            ..r
        }
        .line();
        let windows = Reading {
            game_kernel_ns: Some(ms(200)),
            ..Reading::default()
        };
        let windows = Reading { secs: 2.0, game_cpu_ns: Some(ms(1900)), ..windows }.line();
        assert!(windows.contains("| cpu game 95% (kernel 10%) render - "), "{windows}");
        assert!(solo.contains("| backend (game thread) encode"), "{solo}");
        assert!(solo.contains("cpu game 95% render - "), "{solo}");
    }

    #[test]
    fn an_empty_interval_prints_without_dividing_by_zero() {
        let line = Reading::default().line();
        assert!(
            line.starts_with("host perf: frames 0 in 0.00 s, 0.00 fps; frame ms 0.0/0.0"),
            "{line}"
        );
        assert!(!line.contains("NaN") && !line.contains("inf"), "{line}");
    }
}
