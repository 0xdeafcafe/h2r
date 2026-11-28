//! The effects phase's per-effect callees as Rust bodies
//! (docs/engine/effects-parts.md, "The per-effect bodies"). The effect
//! walk of `0x176e40` (`crate::effects_phase`) calls four functions for
//! each live effect; each is a [`Body`] here, and the Rust answers every
//! segment ([`arm`] counts them):
//!
//! | Entry | Body | What it does |
//! |---|---|---|
//! | `1794e0` | [`alive`] | the effect's object bookkeeping; true when it is gone |
//! | `179730` | [`culled`] | the visibility bit; true when the walk skips it |
//! | `1774f0` | [`delete`] | free the datum and its links |
//! | `1792b0` | [`update`] | the event clock: delays, events, successors |
//!
//! Every call they make that is not one of these four is a guest call
//! from the retail frame, so the runtime can stop at any of them and
//! resume at its site. No RNG draw moves: the draw `259d0` in the update
//! is a guest call where retail makes it.

use guest_layout::points::{self, Point};
use crate::sim_objects::{Body, Table};
use runtime::GuestFault;
use std::sync::LazyLock;

mod alive;
pub mod arm;
mod culled;
mod delete;
mod update;

#[cfg(test)]
mod tests;

/// The bodies' points, in the order the walk calls them.
pub const POINTS: [&Point; 4] =
    [&points::EFFECT_ALIVE, &points::EFFECT_CULLED, &points::EFFECT_DELETE, &points::EFFECT_UPDATE];

/// The four bodies.
pub static BODIES: [Body; 4] = [
    Body { point: &points::EFFECT_ALIVE, sites: &alive::SITES, run: alive::alive },
    Body { point: &points::EFFECT_CULLED, sites: &culled::SITES, run: culled::culled },
    Body { point: &points::EFFECT_DELETE, sites: &delete::SITES, run: delete::delete },
    Body { point: &points::EFFECT_UPDATE, sites: &update::SITES, run: update::update },
];

/// [`BODIES`], indexed. `1794e0` fuses into `1774f0`.
pub static TABLE: LazyLock<Table> = LazyLock::new(|| Table::new(BODIES.iter()).expect("BODIES is consistent"));

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}

fn unowned(pc: u32) -> GuestFault {
    GuestFault {
        addr: pc,
        len: 1,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

/// The entries the bodies' constants name, for the points' tests.
#[cfg(test)]
const ENTRIES: [u32; 4] = [alive::ENTRY, culled::ENTRY, delete::ENTRY, update::ENTRY];
