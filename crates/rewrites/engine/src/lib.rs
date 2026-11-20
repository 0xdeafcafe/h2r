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
//!   (`docs/graphics/e3-lighting.md`, "Phase 0 findings").
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
//! - [`point_shapes`]: what each point expects at its address, checked
//!   against the user's image at install.
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
pub mod campaign_squad_pool;
pub mod entry;
pub mod function_eval;
pub mod gel;
pub mod glow;
pub mod light_constants;
pub mod light_passes;
pub mod locale;
pub mod objectives;
pub mod point_shapes;
pub mod render;
pub mod render_loop;
pub mod scene_list_math;
pub mod screen_effect;
pub mod screen_effects;
pub mod screen_grid;
pub mod shadow_constants;
pub mod hs_actions;
pub mod hs_pool;
pub mod hs_syntax;
// Historical fixed-size partition fixture; the runtime reserves campaign_squad_pool.
#[cfg(test)]
mod squad_pool;
pub mod ui_backdrop;
pub mod ui_model;
pub mod ui_sprite;

#[cfg(feature = "runtime-glue")]
pub mod runtime_glue;
#[cfg(feature = "runtime-glue")]
pub mod sim_anim;
#[cfg(feature = "runtime-glue")]
pub mod sim_collision;
#[cfg(feature = "runtime-glue")]
pub mod sim_native;
#[cfg(feature = "runtime-glue")]
pub mod sim_objects;

#[cfg(feature = "runtime-glue")]
pub mod sim_physics;

// The guest boundary and the named points live in guest-layout
// (docs/dev/crate-structure.md, S2); these keep `engine::` paths.
pub use guest_layout::{datum, handle, memory, points, tags, testing, view};
pub use guest_layout::{point_loc, point_ret, point_sig, record};

pub use handle::DatumHandle;
pub use memory::{FlatMemory, GuestMemory, MemFault};

#[cfg(test)]
mod tests;
