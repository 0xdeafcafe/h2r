//! This process's memory, for the sim report's `wall` block
//! (docs/runtime/sim.md §6): peak resident set everywhere, and the
//! physical footprint on macOS, where the two differ by the clean mapped
//! pages (the shared dylib's code, the disc image's resident pages).

use sim::metrics::MemoryUsage;

/// The process's memory now. `ru_maxrss` is a high-water mark, so this is
/// safe to sample wherever a report is built.
pub fn usage() -> MemoryUsage {
    MemoryUsage {
        peak_rss_mib: peak_rss_mib(),
        footprint_mib: footprint_mib(),
    }
}

/// `getrusage`'s `ru_maxrss`: KiB on Linux and bytes on macOS, by host;
/// MiB out.
fn peak_rss_mib() -> u64 {
    #[cfg(unix)]
    {
        // SAFETY: rusage has no preconditions; the result is by value.
        let mut rusage = unsafe { std::mem::zeroed::<libc::rusage>() };
        // SAFETY: a valid pointer to that struct.
        let ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut rusage) } == 0;
        if !ok {
            return 0;
        }
        let raw = rusage.ru_maxrss.max(0) as u64;
        let bytes = if cfg!(target_os = "macos") {
            raw
        } else {
            raw * 1024
        };
        bytes / (1 << 20)
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// `proc_pid_rusage`'s `ri_phys_footprint`, in MiB. `None` off Apple
/// hosts, where there is no footprint number, only the RSS.
fn footprint_mib() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        // SAFETY: a pointer to that struct, as the flavor's buffer.
        let ok = unsafe {
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V4,
                &mut info as *mut _ as libc::rusage_info_t,
            )
        } == 0;
        ok.then(|| info.ri_phys_footprint / (1 << 20))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_process_has_a_peak_rss() {
        let m = usage();
        assert!(m.peak_rss_mib > 0, "{m:?}");
    }

    #[test]
    fn the_footprint_is_reported_where_the_host_has_one() {
        let m = usage();
        if cfg!(target_os = "macos") {
            assert!(m.footprint_mib.is_some_and(|f| f > 0), "{m:?}");
        } else {
            assert_eq!(m.footprint_mib, None);
        }
    }
}
