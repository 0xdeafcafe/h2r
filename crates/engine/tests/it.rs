//! This crate's integration test files as one test binary: one link and one
//! executable instead of 18 (docs/dev/build-speed.md).
//!
//! Each file stays where it was and is a module here. Its tests gain the
//! file's name as a prefix, so `cargo test -p engine --test it colour_pass_differential::`
//! runs what `--test colour_pass_differential` did. A new file needs a `mod` line below, or
//! a `[[test]]` of its own in Cargo.toml when it must have a process to
//! itself (the last test here fails until it has one or the other).

#[path = "colour_pass_differential.rs"]
mod colour_pass_differential;
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
#[path = "light_passes.rs"]
mod light_passes;
#[path = "light_passes_differential.rs"]
mod light_passes_differential;
#[path = "render_loop_differential.rs"]
mod render_loop_differential;
#[path = "scene_differential.rs"]
mod scene_differential;
#[path = "screen_effects_differential.rs"]
mod screen_effects_differential;
#[path = "screen_grid_differential.rs"]
mod screen_grid_differential;
#[path = "shadow_constants.rs"]
mod shadow_constants;
#[path = "shadow_constants_differential.rs"]
mod shadow_constants_differential;
#[path = "sim_native_differential.rs"]
mod sim_native_differential;
#[path = "sim_physics_differential.rs"]
mod sim_physics_differential;
#[path = "states_differential.rs"]
mod states_differential;
#[path = "views.rs"]
mod views;

include!("../../../scripts/it_guard.rs");
