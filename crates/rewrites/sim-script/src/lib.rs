//! HaloScript threads and natives on the world's pools
//! (docs/engine/hs_vm.md, "Threads"; docs/engine/sim-world.md, "Store
//! slots"). The VM is `hs`'s; this crate holds its state as the world's
//! scripts section and runs the per-tick update over it.
#![deny(clippy::disallowed_methods)]

mod natives;
mod runtime;
mod table;

pub use natives::{Native, Natives};
pub use runtime::ScriptRuntime;
pub use table::{ScriptStore, ScriptTable, Shared, VERSION, lock};

#[cfg(test)]
mod tests;
