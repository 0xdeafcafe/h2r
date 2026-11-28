//! Performance tooling for h2recomp (docs/perf/profiling.md):
//!
//! - [`sample`] and [`profile`]: a sampling profiler for the game thread
//!   that maps generated code back to guest functions and instructions
//!   ([`symbolize`]) and charges every sample to guest code, the runtime,
//!   the kernel, D3D, gfx, the fallback interpreter or other HLE. Writes
//!   folded stacks, a top-N table and a JSON summary.
//! - [`profile::Session`]: the profiler plus a `timeline` capture (Chrome
//!   trace JSON) around any run, from code or from `H2_PROFILE` /
//!   `H2_TIMELINE`.
//! - [`remote`]: profiles from a device that can't symbolise them (the
//!   Xbox app): a module-relative dump there, resolved on the Mac against
//!   the deployed binaries.
//! - [`bench`]: repeatable scenarios ([`hotloop`], [`boot`]),
//!   results as JSON, and a regression comparison.
//! - [`budget`]: a timeline trace split into per-frame self time per
//!   span, with logical time and scheduler counts (`h2 profile budget`).
//! - [`cli`]: `h2 profile` and `h2 bench`.

pub mod bench;
pub mod boot;
pub mod budget;
pub mod cli;
pub mod hotloop;
pub mod profile;
pub mod remote;
pub mod sample;
pub mod symbolize;

pub use profile::{Session, SessionConfig, register_current_thread};

/// Every scenario, in run order.
pub fn scenarios(disc: Option<std::path::PathBuf>) -> Vec<Box<dyn bench::Scenario>> {
    let mut v: Vec<Box<dyn bench::Scenario>> = Vec::new();
    for h in hotloop::HotLoop::all() {
        v.push(Box::new(h));
    }
    v.push(Box::new(boot::MenuHold::new(disc)));
    v
}

/// Scenarios whose name contains any of `patterns` (all when empty).
pub fn select(
    disc: Option<std::path::PathBuf>,
    patterns: &[String],
) -> Vec<Box<dyn bench::Scenario>> {
    scenarios(disc)
        .into_iter()
        .filter(|s| patterns.is_empty() || patterns.iter().any(|p| s.name().contains(p.as_str())))
        .collect()
}
