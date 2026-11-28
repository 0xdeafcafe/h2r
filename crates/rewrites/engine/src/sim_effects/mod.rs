//! Effects, particles, contrails and decals: phase 6 of the simulation
//! rewrite (docs/engine/sim-rewrite.md, "Effects (phase 6)"; contracts in
//! docs/engine/sim-effects.md). Every port is installed, always, by
//! `sim_bridge::sim_effects`.
//!
//! A [`Port`] is a retail function or a stretch of one: an entry and the
//! return addresses of the guest calls it makes. At each, [`Port::run`]
//! works from the registers and the retail stack frame alone, so a
//! checkpoint restore between two segments resumes from the same state. The effect clock, the
//! particle step, the contrail clock and the decal clock are plain Rust
//! values gathered from the datums for one step and written back before
//! the next guest call; no handle, pointer or time survives a callout.
//!
//! The ports share one guarantee with the earlier phases: every RNG draw
//! (`[0x4e7408]` words) happens in retail order, on the game thread.

mod contrails;
mod decals;
mod events;
mod particle_birth;
mod particles;

use crate::points::Point;
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

type Result<T> = std::result::Result<T, GuestFault>;

/// The shadow-check group of these ports (`sim_bridge::shadow`).
pub const GROUP: &str = "sim.effects";

/// One ported function or stretch.
pub struct Port {
    /// Its entry.
    pub point: &'static Point,
    /// The retail return addresses of its guest calls, where `run` is
    /// entered again.
    pub sites: &'static [u32],
    /// Whether the call at `pc` (the entry or a site) is inside what the
    /// port reproduces exactly: float environment, bounded loops, no
    /// armed memory trap. False is counted by the installer; the port
    /// still runs.
    pub supported: fn(&CpuAbi, &Guest, u32) -> bool,
    /// Run from `pc` to the next transfer.
    pub run: fn(&mut CpuAbi, &mut Guest, u32) -> Result<TransferAbi>,
}

impl Port {
    /// Run the entry or a return continuation. A fault with no memory
    /// operand reports the selector that entered the port.
    pub fn invoke(&self, cpu: &mut CpuAbi, guest: &mut Guest, pc: u32) -> Result<TransferAbi> {
        (self.run)(cpu, guest, pc).map_err(|mut fault| {
            if fault.len == 0 && fault.reason == runtime::guest::FaultReason::InvalidOperation {
                fault.addr = pc;
            }
            fault
        })
    }

    /// Whether `pc` is this port's entry or one of its return sites.
    pub fn handles(&self, pc: u32) -> bool {
        pc == self.point.base || self.sites.contains(&pc)
    }
}

/// Every port, in a fixed order. The set of entries is disjoint.
pub fn ports() -> impl Iterator<Item = &'static Port> {
    [events::PORTS, particles::PORTS, particle_birth::PORTS, contrails::PORTS, decals::PORTS]
        .into_iter()
        .flatten()
}

#[cfg(test)]
mod tests;
