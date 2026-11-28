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
//! - [`render`]: read-only views of the live lights and scene layers
//!   (`docs/graphics/e3-lighting.md`, "Phase 0 findings").
//! - [`light_constants`]: `0x0004a1e0`'s per-light globals block, computed
//!   bit for bit without writing guest memory.
//! - [`shadow_constants`]: `0x00048ee0`'s shadow block, the same way.
//! - [`light_passes`]: the per-light pass flags and which implementation a
//!   lit material's layer-12 and layer-13 passes draw.
//! - [`screen_effects`]: which of the scenario's screen effects draw
//!   (`0x00134fc0`, the loop of `render_screen_effects`).
//! - [`hud_effects`]: a HUD widget's effects on its basis (`0x0022bcaf`).
//! - [`hud_sensor`]: the motion sensor's place and its draw calls
//!   (`0x001a2823`).
//! - [`function_eval`]: the tag function evaluator `0x0013b390` with the
//!   CRT's `fmod` and `pow`, bit for bit.
//! - [`map_status`]: the map cache status `0x213f10`, answered from its
//!   last result while nothing it reads has changed.
//! - [`view_projection`]: the view projection builder `0x0002fb70`, bit
//!   for bit.
//!
//! Re-exported from `guest_layout` (docs/dev/crate-structure.md, S2):
//! [`points`], the one table of named guest functions and globals;
//! [`point_shapes`], what each point expects at its address; [`view`],
//! typed views; [`entry`], async entries, Rust bodies that await guest
//! calls; [`testing`], sparse guest memory for tests; [`locale`],
//! localized-string lookup; and [`abi`], the per-function
//! calling-convention adapters and the table of overridable addresses.
//!
//! Each function's recovered contract (registers, memory effects, failure
//! paths, quirks reproduced on purpose) is in `docs/engine/`.

pub mod colour_pass;
pub mod contrail_ribbon;
pub mod campaign_squad_pool;
pub mod fog_fans;
pub mod function_eval;
pub mod hud_effects;
pub mod hud_sensor;
pub mod gel;
pub mod glow;
pub mod light_constants;
pub mod light_passes;
pub mod map_status;
pub mod objectives;
pub mod ocean_grid;
pub mod particle_emission;
pub mod particle_seen;
pub mod particle_walk;
pub mod planar_fog;
pub mod render;
pub mod render_census;
pub mod render_loop;
pub mod round_state;
pub mod scene_list_math;
pub mod screen_effect;
pub mod screen_effects;
pub mod screen_grid;
pub mod shadow_arm;
pub mod shadow_constants;
pub mod skull_gate;
pub mod hs_actions;
pub mod hs_object_lists;
pub mod hs_pool;
pub mod hs_syntax;
// Historical fixed-size partition fixture; the runtime reserves campaign_squad_pool.
#[cfg(test)]
mod squad_pool;
pub mod ui_backdrop;
pub mod view_projection;
pub mod light_frusta;
pub mod view_records;
pub mod visibility_setup;
pub mod ui_model;
pub mod ui_sprite;

#[cfg(feature = "runtime-glue")]
pub mod sim_anim;
#[cfg(feature = "runtime-glue")]
pub mod sim_collision;
#[cfg(feature = "runtime-glue")]
pub mod sim_effects;
#[cfg(feature = "runtime-glue")]
pub mod effect_update;
#[cfg(feature = "runtime-glue")]
pub mod sim_native;
#[cfg(feature = "runtime-glue")]
pub mod sim_lightmap;
#[cfg(feature = "runtime-glue")]
pub mod sim_objects;
#[cfg(feature = "runtime-glue")]
pub mod sim_seats;
pub mod sim_tick;
pub mod countdown_requests;
pub mod hs_update;
pub mod ai_update;
pub mod post_game;
pub mod effects_phase;
pub mod tick_poses;

#[cfg(feature = "runtime-glue")]
pub mod native_sound_ambience;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_channel;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_environment;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_impulse;
#[cfg(feature = "runtime-glue")]
pub mod native_cache_flush;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_listener;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_loops;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_mix;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_objects;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_scheduling;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_segments;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_sources;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_spatial;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_submix;
#[cfg(feature = "runtime-glue")]
pub mod native_sound_tick;
#[cfg(feature = "runtime-glue")]
pub mod sim_sound;
#[cfg(feature = "runtime-glue")]
pub mod sim_physics;

// The guest boundary and the named points live in guest-layout
// (docs/dev/crate-structure.md, S2), with the async entries, the override
// adapters, the locale table and the point shapes; these keep `engine::`
// paths.
pub use guest_layout::{abi, datum, entry, handle, locale, memory, point_shapes, points, sound, tags, testing, view};
#[cfg(feature = "runtime-glue")]
pub use guest_layout::runtime_glue;
pub use guest_layout::{point_loc, point_ret, point_sig, record};

pub use handle::DatumHandle;
pub use memory::{FlatMemory, GuestMemory, MemFault};

#[cfg(test)]
mod tests;
