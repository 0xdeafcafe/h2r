//! Sim mode's pure parts (docs/runtime/sim.md): match, batch and bot
//! tuning files ([`config`]), pacing and speed-up ([`clock`]), the metrics
//! tally and the match report ([`metrics`]), and the A/B comparison of a
//! batch ([`compare`]).
//!
//! Nothing here touches the game: the boot glue (`boot::sim_glue`) reads
//! the game and feeds this crate plain values, and `h2 sim` (the CLI) runs
//! the processes. So all of it is tested without a run.

pub mod clock;
pub mod compare;
pub mod config;
pub mod metrics;

/// The environment variable that turns sim mode on: the match file's path.
pub const ENV: &str = "H2_SIM";
/// Where the boot writes the report at the end of the match.
pub const REPORT_ENV: &str = "H2_SIM_REPORT";
