//! This crate's integration test files as one test binary: one link and one
//! executable instead of 16 (docs/dev/build-speed.md).
//!
//! Each file stays where it was and is a module here. Its tests gain the
//! file's name as a prefix, so `cargo test -p h1convert --test it bloodgulch_ramp::`
//! runs what `--test bloodgulch_ramp` did. A new file needs a `mod` line below, or
//! a `[[test]]` of its own in Cargo.toml when it must have a process to
//! itself (the last test here fails until it has one or the other).

#[path = "bloodgulch_ramp.rs"]
mod bloodgulch_ramp;
#[path = "bloodgulch_ring.rs"]
mod bloodgulch_ring;
#[path = "collision_engine.rs"]
mod collision_engine;
#[path = "disc_maps.rs"]
mod disc_maps;
#[path = "fresh.rs"]
mod fresh;
#[path = "h1_clusters.rs"]
mod h1_clusters;
#[path = "h2_globals.rs"]
mod h2_globals;
#[path = "h2_sbsp.rs"]
mod h2_sbsp;
#[path = "h2_scenario.rs"]
mod h2_scenario;
#[path = "lightmap_light.rs"]
mod lightmap_light;
#[path = "portal_flood.rs"]
mod portal_flood;
#[path = "ramp_contacts.rs"]
mod ramp_contacts;
#[path = "real_maps.rs"]
mod real_maps;
#[path = "render_fields.rs"]
mod render_fields;
#[path = "shad_rebuild.rs"]
mod shad_rebuild;
#[path = "sky_pictures_disc.rs"]
mod sky_pictures_disc;

include!("../../../../scripts/it_guard.rs");
