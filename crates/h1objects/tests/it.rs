//! This crate's integration test files as one test binary: one link and one
//! executable instead of 27 (docs/dev/build-speed.md).
//!
//! Each file stays where it was and is a module here. Its tests gain the
//! file's name as a prefix, so `cargo test -p h1objects --test it animation_codec_disc::`
//! runs what `--test animation_codec_disc` did. A new file needs a `mod` line below, or
//! a `[[test]]` of its own in Cargo.toml when it must have a process to
//! itself (the last test here fails until it has one or the other).

#[path = "animation_codec_disc.rs"]
mod animation_codec_disc;
#[path = "biped_physics_disc.rs"]
mod biped_physics_disc;
#[path = "collision_disc.rs"]
mod collision_disc;
#[path = "disc.rs"]
mod disc;
#[path = "effects_disc.rs"]
mod effects_disc;
#[path = "flares_disc.rs"]
mod flares_disc;
#[path = "hitreg_disc.rs"]
mod hitreg_disc;
#[path = "hlmt_materials_disc.rs"]
mod hlmt_materials_disc;
#[path = "hud_disc.rs"]
mod hud_disc;
#[path = "hud_layout_disc.rs"]
mod hud_layout_disc;
#[path = "model_shaders_disc.rs"]
mod model_shaders_disc;
#[path = "object_functions_disc.rs"]
mod object_functions_disc;
#[path = "particles_disc.rs"]
mod particles_disc;
#[path = "ragdoll_disc.rs"]
mod ragdoll_disc;
#[path = "rebuild_disc.rs"]
mod rebuild_disc;
#[path = "role_audit_disc.rs"]
mod role_audit_disc;
#[path = "sandbox_compare_disc.rs"]
mod sandbox_compare_disc;
#[path = "sandbox_disc.rs"]
mod sandbox_disc;
#[path = "scenery_collision_disc.rs"]
mod scenery_collision_disc;
#[path = "unit_rebuild_disc.rs"]
mod unit_rebuild_disc;
#[path = "vehicle_animation_disc.rs"]
mod vehicle_animation_disc;
#[path = "vehicle_camera_disc.rs"]
mod vehicle_camera_disc;
#[path = "vehicle_ground_disc.rs"]
mod vehicle_ground_disc;
#[path = "vehicle_rebuild_disc.rs"]
mod vehicle_rebuild_disc;
#[path = "weapon_counter.rs"]
mod weapon_counter;
#[path = "weapon_rebuild_disc.rs"]
mod weapon_rebuild_disc;
#[path = "weapon_sounds_disc.rs"]
mod weapon_sounds_disc;

include!("../../../scripts/it_guard.rs");
