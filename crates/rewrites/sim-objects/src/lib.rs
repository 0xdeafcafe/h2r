//! The object update for the simulation rewrite, phase 3
//! (docs/engine/sim-rewrite-objects.md; the guest contract is
//! docs/engine/object-update.md).
//!
//! Guest memory stays the only object table. This crate reads and writes
//! it through typed views over `guest_layout`'s layouts and any
//! [`guest_layout::Mem`], so the same code runs on the runtime's guest
//! memory (through `engine::sim_objects`, which owns the guest boundary:
//! bodies, the dispatcher, frames) and on synthetic memory in tests. It
//! has no `runtime` dependency and keeps no per-object state across a
//! guest call or a tick.
#![deny(clippy::disallowed_methods)]
