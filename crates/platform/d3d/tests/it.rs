//! This crate's integration test files as one test binary: one link and one
//! executable instead of 37 (docs/dev/build-speed.md).
//!
//! Each file stays where it was and is a module here. Its tests gain the
//! file's name as a prefix, so `cargo test -p d3d --test it admission_scope::`
//! runs what `--test admission_scope` did. A new file needs a `mod` line below, or
//! a `[[test]]` of its own in Cargo.toml when it must have a process to
//! itself (the last test here fails until it has one or the other).

#[path = "admission_scope.rs"]
mod admission_scope;
#[path = "batching.rs"]
mod batching;
#[path = "depth_textures.rs"]
mod depth_textures;
#[path = "draw_sharing.rs"]
mod draw_sharing;
#[path = "external_pass.rs"]
mod external_pass;
#[path = "first_frame.rs"]
mod first_frame;
#[path = "fog_xbe.rs"]
mod fog_xbe;
#[path = "fragments.rs"]
mod fragments;
#[path = "glyph_text.rs"]
mod glyph_text;
#[path = "interp.rs"]
mod interp;
#[path = "interp_gpu.rs"]
mod interp_gpu;
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
#[path = "render_scale.rs"]
mod render_scale;
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
#[path = "shaders_render.rs"]
mod shaders_render;
#[path = "shadow_fault.rs"]
mod shadow_fault;
#[path = "shared_state.rs"]
mod shared_state;
#[path = "snapshot.rs"]
mod snapshot;
#[path = "stream_copy.rs"]
mod stream_copy;
#[path = "stream_reuse.rs"]
mod stream_reuse;
#[path = "swallow.rs"]
mod swallow;
#[path = "vblank.rs"]
mod vblank;
#[path = "vertex_cache.rs"]
mod vertex_cache;
#[path = "vertex_fetch.rs"]
mod vertex_fetch;
#[path = "xbe_tables.rs"]
mod xbe_tables;

include!("../../../../scripts/it_guard.rs");
