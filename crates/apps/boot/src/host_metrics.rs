//! A bounded rolling sample of game-thread and process use at the tick
//! boundary. A skipped render or a sleep does not change the unit of
//! measurement.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use agent::Access;
use agent_proto::{RpcError, code, method};
use feature::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
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
    fn now() -> Self {
        Self {
            at: Instant::now(),
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

#[derive(Default)]
struct State {
    previous: Option<Sample>,
    last: Option<(Instant, Reading)>,
}

#[derive(Default)]
struct Shared {
    ticks: AtomicU64,
    state: Mutex<State>,
}

struct Metrics(Arc<Shared>);

impl Feature for Metrics {
    fn manifest(&self) -> Manifest {
        Manifest::new("host-metrics", "Host resource use per game tick")
            .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        let shared = Arc::clone(&self.0);
        b.frame().on_tick("sample host metrics", move |_| {
            if !shared.ticks.fetch_add(1, Ordering::Relaxed).is_multiple_of(WINDOW_TICKS) {
                return;
            }
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let now = Sample::now();
            if let Some(prev) = &state.previous {
                state.last = Some((now.at, now.since(prev)));
            }
            state.previous = Some(now);
        });
        Ok(())
    }
}

pub(crate) fn install(
    features: &mut feature::Registry,
    agent: &agent::Agent,
    host_light: bool,
) -> anyhow::Result<()> {
    let shared = Arc::new(Shared::default());
    if features
        .add(&Metrics(Arc::clone(&shared)))
        .map_err(anyhow::Error::msg)?
    {
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
            reply["age_ms"] = json!(at.elapsed().as_millis() as u64);
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
