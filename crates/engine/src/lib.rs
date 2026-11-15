//! Hand-written Rust replacements for Halo 2 engine functions.
//!
//! This is stage 2 of `docs/ARCHITECTURE.md`: readable code that takes over
//! from the generated scaffolding one guest function at a time, behind the
//! runtime's override table. Generated code keeps reading the same guest
//! structures directly, so every type here is a *view* over guest memory with
//! the original layout, reached through [`GuestMemory`].
//!
//! - [`tags`]: `tag_get_checked` and `tag_group_find` over the published
//!   cache index.
//! - [`datum`]: datum arrays (allocation, salts, lookup, iteration).
//! - [`locale`]: localized-string lookup by packed StringID.
//! - [`render`]: read-only views of the live lights and scene layers
//!   (`docs/e3-lighting.md`, phase 0).
//! - [`abi`]: per-function calling-convention adapters and the table of
//!   overridable addresses.
//!
//! Each function's recovered contract (registers, memory effects, failure
//! paths, quirks reproduced on purpose) is in `docs/engine/`.

pub mod abi;
pub mod datum;
pub mod handle;
pub mod locale;
pub mod memory;
pub mod render;
pub mod tags;

#[cfg(feature = "runtime-glue")]
pub mod runtime_glue;

pub use handle::DatumHandle;
pub use memory::{FlatMemory, GuestMemory, MemFault};

#[cfg(test)]
mod tests;
