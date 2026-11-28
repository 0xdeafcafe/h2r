//! This crate's integration test files as one test binary: one link and one
//! executable instead of 16 (docs/dev/build-speed.md).
//!
//! Each file stays where it was and is a module here. Its tests gain the
//! file's name as a prefix, so `cargo test -p engine --test it colour_pass_differential::`
//! runs what `--test colour_pass_differential` did. A new file needs a `mod` line below, or
//! a `[[test]]` of its own in Cargo.toml when it must have a process to
//! itself (the last test here fails until it has one or the other).

#[path = "colour_pass_differential.rs"]
mod colour_pass_differential;
#[path = "contrail_ribbon_differential.rs"]
mod contrail_ribbon_differential;
#[path = "differential.rs"]
mod differential;
#[path = "glow_differential.rs"]
mod glow_differential;
#[path = "layer_differential.rs"]
mod layer_differential;
#[path = "light_constants.rs"]
mod light_constants;
#[path = "light_constants_differential.rs"]
mod light_constants_differential;
#[path = "light_frusta_differential.rs"]
mod light_frusta_differential;
#[path = "light_gather.rs"]
mod light_gather;
#[path = "light_gather_differential.rs"]
mod light_gather_differential;
#[path = "light_passes.rs"]
mod light_passes;
#[path = "light_passes_differential.rs"]
mod light_passes_differential;
#[path = "map_status_differential.rs"]
mod map_status_differential;
#[path = "ocean_grid_differential.rs"]
mod ocean_grid_differential;
#[path = "particle_emission_differential.rs"]
mod particle_emission_differential;
#[path = "particle_walk_differential.rs"]
mod particle_walk_differential;
#[path = "point_shapes.rs"]
mod point_shapes;
#[path = "screen_effects_differential.rs"]
mod screen_effects_differential;
#[path = "screen_grid_differential.rs"]
mod screen_grid_differential;
#[path = "screen_record_differential.rs"]
mod screen_record_differential;
#[path = "shadow_constants.rs"]
mod shadow_constants;
#[path = "shadow_constants_differential.rs"]
mod shadow_constants_differential;
#[path = "sim_anim_differential.rs"]
mod sim_anim_differential;
#[path = "sim_anim_graph_differential.rs"]
mod sim_anim_graph_differential;
#[path = "sim_collision_differential.rs"]
mod sim_collision_differential;
#[path = "sim_effects_differential.rs"]
mod sim_effects_differential;
#[path = "sim_objects_differential.rs"]
mod sim_objects_differential;
#[path = "sim_havok_differential.rs"]
mod sim_havok_differential;
#[path = "sim_lightmap_differential.rs"]
mod sim_lightmap_differential;
#[path = "sim_native_differential.rs"]
mod sim_native_differential;
#[path = "sim_physics_differential.rs"]
mod sim_physics_differential;
#[path = "sim_physics_kernel.rs"]
mod sim_physics_kernel;
#[path = "sim_seats_differential.rs"]
mod sim_seats_differential;
#[path = "sim_sound_differential.rs"]
mod sim_sound_differential;
#[path = "sim_tick_differential.rs"]
mod sim_tick_differential;
#[path = "sim_tier_differential.rs"]
mod sim_tier_differential;
#[path = "states_differential.rs"]
mod states_differential;
#[path = "variant_record_differential.rs"]
mod variant_record_differential;
#[path = "view_projection_differential.rs"]
mod view_projection_differential;
#[path = "view_records_differential.rs"]
mod view_records_differential;
#[path = "views.rs"]
mod views;

include!("../../../../scripts/it_guard.rs");
