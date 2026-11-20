//! What a packed match costs, from Linux `/proc` and the container's cgroup.
//! Elsewhere (a Mac running local Live) nothing is measured and admission
//! falls back to the configured per-match defaults.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::time::Instant;

use live_proto::host_pack::{MatchLoad, TICK_MS};

/// One sample of a match's process group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Usage {
    /// User plus system CPU time of every process in the group, in ms.
    pub cpu_ms: f64,
    /// Proportional resident memory (shared pages split between sharers).
    pub pss_mib: u64,
}

/// Samples before a match's cost is trusted over the default. The first
/// seconds are provisioning and boot, not play.
const WARMUP: u32 = 3;
/// Weight of the newest per-tick cost.
const ALPHA: f64 = 0.3;

#[derive(Clone, Debug, Default)]
pub(super) struct Meter {
    last: Option<(Instant, f64)>,
    ewma: Option<f64>,
    /// The higher of the smoothed and the latest cost, so a burst counts.
    recent: Option<f64>,
    samples: u32,
    pss_mib: Option<u64>,
}

impl Meter {
    pub(super) fn observe(&mut self, now: Instant, usage: Usage) {
        self.pss_mib = Some(usage.pss_mib);
        if let Some((then, cpu_then)) = self.last {
            let wall_ms = now.duration_since(then).as_secs_f64() * 1000.0;
            if wall_ms > 0.0 {
                let per_tick = (usage.cpu_ms - cpu_then).max(0.0) / (wall_ms / TICK_MS);
                let ewma = self
                    .ewma
                    .map_or(per_tick, |old| old + ALPHA * (per_tick - old));
                self.ewma = Some(ewma);
                self.recent = Some(ewma.max(per_tick));
                self.samples += 1;
            }
        }
        self.last = Some((now, usage.cpu_ms));
    }

    pub(super) fn load(&self) -> MatchLoad {
        MatchLoad {
            ms_per_tick: self.recent.filter(|_| self.samples >= WARMUP),
            rss_mib: self.pss_mib,
        }
    }
}

/// `utime + stime` (clock ticks) and process group from `/proc/PID/stat`.
pub(super) fn parse_stat(stat: &str) -> Option<(i64, u64)> {
    // The command name can hold spaces and parentheses; fields resume
    // after the last ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let pgrp = fields.get(2)?.parse().ok()?;
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((pgrp, utime + stime))
}

/// `Pss:` from `/proc/PID/smaps_rollup`, in KiB.
pub(super) fn parse_pss_kib(rollup: &str) -> Option<u64> {
    rollup.lines().find_map(|line| {
        let value = line.strip_prefix("Pss:")?;
        value.trim().strip_suffix("kB")?.trim().parse().ok()
    })
}

/// vCPUs from cgroup v2 `cpu.max` (`quota period`, or `max period`).
pub(super) fn parse_cpu_max(text: &str) -> Option<f64> {
    let mut words = text.split_whitespace();
    let quota: f64 = words.next()?.parse().ok()?;
    let period: f64 = words.next()?.parse().ok()?;
    (quota > 0.0 && period > 0.0).then(|| quota / period)
}

/// MiB from cgroup v2 `memory.max` (`max` means none).
pub(super) fn parse_memory_max(text: &str) -> Option<u64> {
    let bytes: u64 = text.trim().parse().ok()?;
    Some(bytes / (1024 * 1024))
}

/// Container memory that reclaim cannot drop: `memory.current` less the
/// inactive file pages in `memory.stat`, in MiB.
pub(super) fn used_mib(current: &str, stat: &str) -> Option<u64> {
    let current: u64 = current.trim().parse().ok()?;
    let inactive = stat
        .lines()
        .find_map(|line| line.strip_prefix("inactive_file ")?.trim().parse::<u64>().ok())
        .unwrap_or(0);
    Some(current.saturating_sub(inactive) / (1024 * 1024))
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs;

    const CGROUP: &str = "/sys/fs/cgroup";

    pub(in super::super) fn group_usage(leader: u32) -> Option<Usage> {
        // SAFETY: sysconf has no preconditions.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let hz = if hz > 0 { hz as f64 } else { 100.0 };
        let mut ticks = 0u64;
        let mut pss_kib = 0u64;
        let mut found = false;
        for entry in fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            let Some((pgrp, cpu)) = parse_stat(&stat) else {
                continue;
            };
            if pgrp != i64::from(leader) {
                continue;
            }
            found = true;
            ticks += cpu;
            if let Ok(rollup) = fs::read_to_string(format!("/proc/{pid}/smaps_rollup")) {
                pss_kib += parse_pss_kib(&rollup).unwrap_or(0);
            }
        }
        found.then(|| Usage {
            cpu_ms: ticks as f64 * 1000.0 / hz,
            pss_mib: pss_kib / 1024,
        })
    }

    pub(in super::super) fn cgroup_vcpus() -> Option<f64> {
        parse_cpu_max(&fs::read_to_string(format!("{CGROUP}/cpu.max")).ok()?)
    }

    pub(in super::super) fn cgroup_memory_max_mib() -> Option<u64> {
        parse_memory_max(&fs::read_to_string(format!("{CGROUP}/memory.max")).ok()?)
    }

    pub(in super::super) fn cgroup_used_mib() -> Option<u64> {
        used_mib(
            &fs::read_to_string(format!("{CGROUP}/memory.current")).ok()?,
            &fs::read_to_string(format!("{CGROUP}/memory.stat")).unwrap_or_default(),
        )
    }
}

#[cfg(target_os = "linux")]
pub(super) use linux::{cgroup_memory_max_mib, cgroup_used_mib, cgroup_vcpus, group_usage};

#[cfg(not(target_os = "linux"))]
pub(super) fn group_usage(_leader: u32) -> Option<Usage> {
    None
}

#[cfg(not(target_os = "linux"))]
pub(super) fn cgroup_vcpus() -> Option<f64> {
    None
}

#[cfg(not(target_os = "linux"))]
pub(super) fn cgroup_memory_max_mib() -> Option<u64> {
    None
}

#[cfg(not(target_os = "linux"))]
pub(super) fn cgroup_used_mib() -> Option<u64> {
    None
}
