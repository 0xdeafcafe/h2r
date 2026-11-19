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
//!   (`docs/graphics/e3-lighting.md`, phase 0).
//! - [`light_constants`]: `0x0004a1e0`'s per-light globals block, computed
//!   bit for bit without writing guest memory.
//! - [`shadow_constants`]: `0x00048ee0`'s shadow block, the same way.
//! - [`light_passes`]: the per-light pass flags and which implementation a
//!   lit material's layer-12 and layer-13 passes draw.
//! - [`screen_effects`]: which of the scenario's screen effects draw
//!   (`0x00134fc0`, the loop of `render_screen_effects`).
//! - [`function_eval`]: the tag function evaluator `0x0013b390` with the
//!   CRT's `fmod` and `pow`, bit for bit.
//! - [`points`]: the one table of named guest functions and globals.
//! - [`view`]: typed views (`record!`, datum arrays, tags, players,
//!   objects).
//! - [`entry`]: async entries, Rust bodies that await guest calls.
//! - [`testing`]: [`testing::FakeGuest`], sparse guest memory for tests.
//! - [`abi`]: per-function calling-convention adapters and the table of
//!   overridable addresses.
//!
//! Each function's recovered contract (registers, memory effects, failure
//! paths, quirks reproduced on purpose) is in `docs/engine/`.

pub mod abi;
pub mod colour_pass;
pub mod datum;
pub mod entry;
pub mod function_eval;
pub mod glow;
pub mod handle;
pub mod light_constants;
pub mod light_passes;
pub mod locale;
pub mod memory;
pub mod points;
pub mod render;
pub mod render_loop;
pub mod screen_effect;
pub mod screen_effects;
pub mod screen_grid;
pub mod shadow_constants;
pub mod tags;
pub mod testing;
pub mod view;

#[cfg(feature = "runtime-glue")]
pub mod runtime_glue;

pub use handle::DatumHandle;
pub use memory::{FlatMemory, GuestMemory, MemFault};

#[cfg(test)]
mod tests;
