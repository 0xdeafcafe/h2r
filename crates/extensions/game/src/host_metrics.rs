//! A bounded rolling sample of game-thread and process use at the tick
//! boundary. A skipped render or a sleep does not change the unit of
//! measurement.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent::Access;
use agent_proto::{RpcError, code, method};
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use serde::Serialize;
use serde_json::json;
use timeline::host::{process_cpu_ns, thread_cpu_ns};

const WINDOW_TICKS: u64 = 30;

struct Sample {
    at: Instant,
    game_ns: Option<u64>,
    process_ns: Option<u64>,
    rss_bytes: Option<u64>,
}

impl Sample {
    fn now(at: Instant) -> Self {
        Self {
            at,
            game_ns: thread_cpu_ns(),
            process_ns: process_cpu_ns(),
            rss_bytes: resident_bytes(),
        }
    }

    fn since(&self, prev: &Self) -> Reading {
        let secs = self.at.duration_since(prev.at).as_secs_f64();
        let delta = |now: Option<u64>, then: Option<u64>| now?.checked_sub(then?);
        let ms_per_tick = |ns: u64| ns as f64 / 1e6 / WINDOW_TICKS as f64;
        Reading {
            ticks: WINDOW_TICKS,
            wall_ms_per_tick: secs * 1000.0 / WINDOW_TICKS as f64,
            game_cpu_ms_per_tick: delta(self.game_ns, prev.game_ns).map(ms_per_tick),
            process_cpu_ms_per_tick: delta(self.process_ns, prev.process_ns).map(ms_per_tick),
            cpu_percent: delta(self.process_ns, prev.process_ns)
                .filter(|_| secs > 0.0)
                .map(|ns| ns as f64 / 1e7 / secs),
            rss_bytes: self.rss_bytes,
        }
    }
}

#[derive(Serialize)]
struct Reading {
    ticks: u64,
    wall_ms_per_tick: f64,
    game_cpu_ms_per_tick: Option<f64>,
    process_cpu_ms_per_tick: Option<f64>,
    cpu_percent: Option<f64>,
    rss_bytes: Option<u64>,
}

const LATENCY_WINDOW: Duration = Duration::from_secs(30);
const LATE_TICK: Duration = Duration::from_millis(live_proto::host_pack::LATE_TICK_MS);

/// Exact nearest-rank p99 of individual tick intervals. Evict by wall
/// time so the horizon stays 30 seconds even when tick rate changes.
#[derive(Default)]
struct TickIntervals {
    last_tick: Option<Instant>,
    intervals: VecDeque<(Instant, Duration)>,
}

impl TickIntervals {
    fn tick(&mut self, now: Instant, playing: bool) {
        if !playing {
            self.last_tick = None;
            self.intervals.clear();
            return;
        }
        while self.intervals.front().is_some_and(|(at, _)| now.duration_since(*at) >= LATENCY_WINDOW) {
            self.intervals.pop_front();
        }
        if let Some(last) = self.last_tick.replace(now) {
            self.intervals.push_back((now, now.duration_since(last)));
        }
    }

    fn report(&self, now: Instant) -> (Option<f64>, usize) {
        let mut durations: Vec<_> = self.intervals.iter().rev()
            .take_while(|(at, _)| now.duration_since(*at) < LATENCY_WINDOW)
            .map(|(_, elapsed)| *elapsed)
            .collect();
        if durations.is_empty() {
            return (None, 0);
        }
        let late = durations.iter().filter(|&&d| d > LATE_TICK).count();
        let index = durations.len() - 1 - durations.len() / 100;
        let (_, p99, _) = durations.select_nth_unstable(index);
        (Some(p99.as_secs_f64() * 1000.0), late)
    }
}

#[derive(Default)]
struct State {
    intervals: TickIntervals,
    previous: Option<Sample>,
    last: Option<(Instant, Reading)>,
}

struct Sampler {
    ticks: AtomicU64,
    host_light: bool,
    state: Mutex<State>,
}

impl Sampler {
    fn tick(&self, is_multiplayer: impl FnOnce() -> bool) {
        let boundary = self.ticks.fetch_add(1, Ordering::Relaxed).is_multiple_of(WINDOW_TICKS);
        if !self.host_light && !boundary {
            return;
        }

        let now = Instant::now();
        let playing = self.host_light.then(is_multiplayer);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(playing) = playing {
            state.intervals.tick(now, playing);
        }
        if boundary {
            let sample = Sample::now(now);
            if let Some(prev) = &state.previous {
                state.last = Some((now, sample.since(prev)));
            }
            state.previous = Some(sample);
        }
    }
}

struct Metrics(Arc<Sampler>);

impl Feature for Metrics {
    fn manifest(&self) -> Manifest {
        Manifest::new("host-metrics", "Host resource use per game tick")
            .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        let shared = Arc::clone(&self.0);
        b.frame().on_tick("sample host metrics", move |o| {
            shared.tick(|| {
                crate::level_boot::active_mode(&*o.guest()) == Some(crate::level_boot::MODE_MULTIPLAYER)
            });
        });
        Ok(())
    }
}

pub fn install(
    features: &mut hooks::Registry,
    agent: &agent::Agent,
    host_light: bool,
) -> anyhow::Result<()> {
    let shared = Arc::new(Sampler {
        ticks: AtomicU64::new(0),
        host_light,
        state: Mutex::new(State::default()),
    });
    if features
        .add(&Metrics(Arc::clone(&shared)))
        .map_err(anyhow::Error::msg)?
    {
        let mut peak_pss_bytes = None;
        agent.add_method(method::HOST_METRICS, Access::Read, move |params| {
            if !(params.is_null() || params.as_object().is_some_and(|p| p.is_empty())) {
                return Err(RpcError::new(
                    code::INVALID_PARAMS,
                    "host.metrics takes no parameters",
                ));
            }
            let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some((at, reading)) = &state.last else {
                return Err(RpcError::new(
                    code::UNAVAILABLE,
                    "waiting for 30 completed tick intervals",
                ));
            };
            let mut reply = json!(reading);
            let now = Instant::now();
            let timing = host_light.then(|| state.intervals.report(now));
            reply["wall_ms_per_tick_p99"] = json!(timing.and_then(|(p99, _)| p99));
            reply["late_ticks"] = json!(timing.map(|(_, late)| late));
            reply["age_ms"] = json!(now.duration_since(*at).as_millis() as u64);
            drop(state);
            let split = crate::host_mem::resident_split();
            let pss = split.map(|m| m.pss);
            peak_pss_bytes = peak_pss_bytes.max(pss);
            reply["cpu_seconds"] = json!(process_cpu_ns().map(|ns| ns as f64 / 1e9));
            reply["private_bytes"] = json!(split.map(|m| m.private));
            reply["shared_bytes"] = json!(split.map(|m| m.shared));
            reply["private_dirty_bytes"] = json!(split.map(|m| m.private_dirty));
            reply["shared_clean_bytes"] = json!(split.map(|m| m.shared_clean));
            reply["pss_bytes"] = json!(pss);
            reply["peak_pss_bytes"] = json!(peak_pss_bytes);
            reply["host_light"] = json!(host_light);
            Ok(reply)
        });
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn resident_bytes() -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: the buffer has the alignment and size for PROC_PIDTASKINFO.
    // Read it only after the OS reports writing the whole object.
    let n = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    (n == size).then(|| unsafe { info.assume_init() }.pti_resident_size)
}

#[cfg(target_os = "linux")]
fn resident_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    // SAFETY: querying the page size requires no pointers.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    (page_size > 0).then_some(page_size as u64)?.checked_mul(resident_pages)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn resident_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn ordinary_players_get_window_metrics_without_a_guest_mode_read() {
        let sampler = Sampler {
            ticks: AtomicU64::new(0),
            host_light: false,
            state: Mutex::new(State::default()),
        };
        for _ in 0..=WINDOW_TICKS {
            sampler.tick(|| panic!("ordinary player sampled the game's mode"));
        }
        let state = sampler.state.lock().unwrap();
        assert_eq!(state.last.as_ref().unwrap().1.ticks, WINDOW_TICKS);
        assert!(state.intervals.last_tick.is_none());
        assert!(state.intervals.intervals.is_empty());
    }

    #[test]
    fn rolling_p99_counts_individual_gameplay_intervals_and_expires_at_30s() {
        let mut w = TickIntervals::default();
        let mut at = Instant::now();
        w.tick(at, true);
        // Two late ticks of 50 ms, 97 regular ticks and one at the threshold.
        for ms in [50, 50, 36].into_iter().chain(std::iter::repeat_n(33, 97)) {
            at += Duration::from_millis(ms);
            w.tick(at, true);
        }
        assert_eq!(w.report(at), (Some(50.0), 2));
        // When only the first 50 ms tick expires, the second is the maximum.
        // With 99 intervals, nearest-rank p99 is still the maximum.
        assert_eq!(
            w.report(at + LATENCY_WINDOW - Duration::from_millis(3250)),
            (Some(50.0), 1)
        );
        assert_eq!(w.report(at + LATENCY_WINDOW), (None, 0));
        w.tick(at, false);
        w.tick(at + Duration::from_secs(5), true);
        assert_eq!(w.report(at + Duration::from_secs(5)), (None, 0));
    }

    #[test]
    fn normalizes_cpu_time_by_ticks_and_wall_interval() {
        let now = Instant::now();
        let prev = Sample {
            at: now,
            game_ns: Some(123_000_000),
            process_ns: Some(255_000_000),
            rss_bytes: Some(10),
        };
        let next = Sample {
            at: now + Duration::from_secs(1),
            game_ns: Some(423_000_000),
            process_ns: Some(705_000_000),
            rss_bytes: Some(100),
        };
        let r = next.since(&prev);
        assert_eq!(r.ticks, 30);
        assert_eq!(r.wall_ms_per_tick, 1000.0 / 30.0);
        assert_eq!(r.game_cpu_ms_per_tick, Some(10.0));
        assert_eq!(r.process_cpu_ms_per_tick, Some(15.0));
        assert_eq!(r.cpu_percent, Some(45.0));
        assert_eq!(r.rss_bytes, Some(100));
    }
}
