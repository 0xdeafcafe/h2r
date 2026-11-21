//! Actor presentation channel for host-replicated sessions. All identifiers
//! come from the peers' shared map or the replicated entity. The channel
//! has no guest, game-mode or transport dependency.

pub mod budget;
pub mod set;
pub mod wire;

pub use budget::{Budget, BudgetError};
pub use set::{ActorSet, Replica, SetError};
pub use wire::{Actor, ActorChange, Control, Packet, Record, WireError, decode, encode};

/// Host tick intervals at 30 Hz.
pub const KEY_PERIOD_TICKS: u32 = 150;
pub const DIGEST_PERIOD_TICKS: u32 = 30;

/// Serial-number ordering within a session, including the u32 wrap.
pub(crate) fn newer_tick(tick: u32, previous: u32) -> bool {
    let distance = tick.wrapping_sub(previous);
    distance != 0 && distance < (1 << 31)
}
