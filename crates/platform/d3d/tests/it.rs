//! This crate's integration test files as one test binary: one link and one
//! executable instead of 37 (docs/dev/build-speed.md).
//!
//! Each file stays where it was and is a module here. Its tests gain the
//! file's name as a prefix, so `cargo test -p d3d --test it admission_scope::`
//! runs what `--test admission_scope` did. A new file needs a `mod` line below, or
//! a `[[test]]` of its own in Cargo.toml when it must have a process to
//! itself (the last test here fails until it has one or the other).

mod common;

#[path = "admission_scope.rs"]
mod admission_scope;
#[path = "depth_textures.rs"]
mod depth_textures;
#[path = "external_pass.rs"]
mod external_pass;
#[path = "first_frame.rs"]
mod first_frame;
#[path = "fog_xbe.rs"]
mod fog_xbe;
#[path = "fragments.rs"]
mod fragments;
#[path = "guest_draw_pass.rs"]
mod guest_draw_pass;
#[path = "light_tail.rs"]
mod light_tail;
#[path = "native_residual.rs"]
mod native_residual;
#[path = "nv2a_conformance.rs"]
mod nv2a_conformance;
#[path = "opaque_world.rs"]
mod opaque_world;
#[path = "passes.rs"]
mod passes;
#[path = "recheck_fixes.rs"]
mod recheck_fixes;
#[path = "registration.rs"]
mod registration;
#[path = "residual_tally.rs"]
mod residual_tally;
#[path = "render_scale.rs"]
mod render_scale;
#[cfg(feature = "gpu")]
#[path = "scene_camera.rs"]
mod scene_camera;
#[path = "scene_sink.rs"]
mod scene_sink;
#[path = "scene_stage.rs"]
mod scene_stage;
#[path = "review_fixes.rs"]
mod review_fixes;
#[path = "round5.rs"]
mod round5;
#[path = "round6.rs"]
mod round6;
#[path = "round7.rs"]
mod round7;
#[path = "rtt_direct.rs"]
mod rtt_direct;
#[path = "seams_gpu.rs"]
mod seams_gpu;
#[path = "shadow_fault.rs"]
mod shadow_fault;
#[path = "snapshot.rs"]
mod snapshot;
#[path = "stream_copy.rs"]
mod stream_copy;
#[path = "swallow.rs"]
mod swallow;
#[path = "swallow_carry.rs"]
mod swallow_carry;
#[path = "vblank.rs"]
mod vblank;
#[path = "vertex_fetch.rs"]
mod vertex_fetch;
#[path = "xbe_tables.rs"]
mod xbe_tables;

include!("../../../../scripts/it_guard.rs");
