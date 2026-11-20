//! Cost of the fault path: one first-write fault plus its re-arm.

use std::time::Instant;

use super::mapping::Mapping;
use super::ops::str64;
use super::server::WatchServer;
use super::sys::*;
use crate::Error;

/// Nanoseconds per page for each pattern.
#[derive(Clone, Copy, Debug)]
pub struct Costs {
    pub pages: usize,
    pub rounds: usize,
    /// Re-arm one page (`mprotect` read-only) then write it: one fault.
    pub single_rearm_and_fault_ns: Percentiles,
    /// Write one page after re-arm, isolated from the `mprotect`.
    pub fault_only_ns: Percentiles,
    /// `mprotect` read-only then read-write on one page, no fault.
    pub mprotect_pair_ns: f64,
    /// Arm the whole region with one `mprotect`, then fault every page;
    /// then `rearm_dirty` re-protects them in one coalesced run.
    pub bulk_fault_ns: f64,
    pub bulk_rearm_dirty_ns: f64,
    /// A store to a page that is already writable.
    pub plain_store_ns: f64,
}

/// Per-sample distribution in nanoseconds; medians resist the scheduling
/// noise of a loaded machine better than means.
#[derive(Clone, Copy, Debug)]
pub struct Percentiles {
    pub p10: u64,
    pub p50: u64,
    pub p90: u64,
    pub mean: f64,
}

impl Percentiles {
    fn of(mut v: Vec<u64>) -> Self {
        v.sort_unstable();
        let at = |q: usize| v[(v.len() - 1) * q / 100];
        let mean = v.iter().sum::<u64>() as f64 / v.len() as f64;
        Self {
            p10: at(10),
            p50: at(50),
            p90: at(90),
            mean,
        }
    }
}

/// Runs on the calling thread, which this attaches to a fresh server.
pub fn measure(pages: usize, rounds: usize) -> Result<Costs, Error> {
    let server = WatchServer::start()?;
    let map = Mapping::new(pages)?;
    let id = server.register(map.base(), map.len())?;
    let _attached = server.attach_current_thread()?;

    // Warm up: touch everything once through the fault path.
    server.arm(id)?;
    for p in 0..pages {
        // SAFETY: aligned address inside the mapping.
        unsafe { str64(map.page(p), p as u64) };
    }
    server.rearm_dirty(id, |_| ())?;

    let n = pages * rounds;
    let mut single = Vec::with_capacity(n);
    for r in 0..rounds {
        for p in 0..pages {
            let t = Instant::now();
            map.protect(p, 1, PROT_READ)?;
            // SAFETY: as above; faults once and is retried.
            unsafe { str64(map.page(p), (r + p) as u64) };
            single.push(t.elapsed().as_nanos() as u64);
        }
    }

    let mut fault_only = Vec::with_capacity(n);
    for r in 0..rounds {
        server.arm(id)?;
        for p in 0..pages {
            let t = Instant::now();
            // SAFETY: as above.
            unsafe { str64(map.page(p).wrapping_add(8), (r ^ p) as u64) };
            fault_only.push(t.elapsed().as_nanos() as u64);
        }
    }

    server.unregister(id)?;
    let t = Instant::now();
    for _ in 0..rounds {
        for p in 0..pages {
            map.protect(p, 1, PROT_READ)?;
            map.protect(p, 1, PROT_READ | PROT_WRITE)?;
        }
    }
    let pair = t.elapsed().as_nanos() as f64 / n as f64;
    let id = server.register(map.base(), map.len())?;

    let mut fault_ns = 0u128;
    let mut rearm_ns = 0u128;
    server.arm(id)?;
    for r in 0..rounds {
        let t = Instant::now();
        for p in 0..pages {
            // SAFETY: as above.
            unsafe { str64(map.page(p).wrapping_add(16), (r * p) as u64) };
        }
        fault_ns += t.elapsed().as_nanos();
        let t = Instant::now();
        let dirty = server.rearm_dirty(id, |_| ())?;
        rearm_ns += t.elapsed().as_nanos();
        assert_eq!(dirty, pages);
    }

    // Plain stores: open every page with one write first, then time.
    for p in 0..pages {
        // SAFETY: as above.
        unsafe { str64(map.page(p).wrapping_add(24), 0) };
    }
    let t = Instant::now();
    for r in 0..rounds {
        for p in 0..pages {
            // SAFETY: as above.
            unsafe { str64(map.page(p).wrapping_add(24), (r + p) as u64) };
        }
    }
    let plain = t.elapsed().as_nanos() as f64 / n as f64;

    Ok(Costs {
        pages,
        rounds,
        single_rearm_and_fault_ns: Percentiles::of(single),
        fault_only_ns: Percentiles::of(fault_only),
        mprotect_pair_ns: pair,
        bulk_fault_ns: fault_ns as f64 / n as f64,
        bulk_rearm_dirty_ns: rearm_ns as f64 / n as f64,
        plain_store_ns: plain,
    })
}
