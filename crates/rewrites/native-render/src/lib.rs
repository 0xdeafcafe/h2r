//! Native GPU renderer (docs/graphics/native-renderer.md).
//!
//! It draws from [`frame_scene::FrameScene`] (docs/graphics/framescene.md).
//! The guest offsets the world and object stages read moved to
//! `scene_extract::layout` in slice 1; they are re-exported below at
//! their old paths for the hooks in `feature` and `boot`, and for the
//! stages here that still read guest memory at their hooks (slices 3 to
//! 6 move them).
//!
//! Stage 1, the static world: the structure BSP's opaque geometry is read
//! from the map's tags once ([`load`], [`world`]), uploaded once
//! ([`gpu`]), and drawn each frame from the game's render camera
//! ([`camera`]) with frustum-culled indirect draws ([`cull`]), while the
//! game skips its own layer-3 entries for it ([`skip`]). Nothing here
//! writes guest memory except [`skip`]'s mask bits, which it restores.
//!
//! Stage 2, objects and skinning: render models read once per tag
//! ([`model_load`], [`model`]), uploaded once ([`model_gpu`]), and drawn
//! per view from the game's own node matrices ([`objects`],
//! [`instances`]), skinned on the GPU, while the game skips their layer-3
//! entries ([`objects::clear_objects`]).
//!
//! Stage 3, transparents: sorted geometry entries ([`transparent`])
//! use native vertex bindings at their original indexed draws.
//!
//! Stage 4b, planar fog: the world's materials drawn again with their own
//! layer-16 passes, the fog unit resolved per view from the device's
//! state ([`fog`], [`fog_gpu`]), and the water edge blend, layer 6, with
//! its scene-copy and reflection inputs supplied per view
//! ([`water_gpu`]).

pub mod camera;
pub mod cull;
pub mod decal;
#[cfg(feature = "gpu")]
pub mod decal_gpu;
pub mod decorators;
#[cfg(feature = "gpu")]
pub mod decorators_gpu;
pub mod effect_geometry;
pub mod effect_immediate;
pub mod fog;
#[cfg(feature = "gpu")]
pub mod fog_gpu;
pub mod gels;
#[cfg(feature = "gpu")]
pub mod gels_gpu;
#[cfg(feature = "gpu")]
pub mod glow;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod hud;
#[cfg(feature = "gpu")]
pub mod layer_pass;
#[cfg(feature = "gpu")]
pub mod ui_gpu;
pub mod instances;
#[cfg(feature = "gpu")]
pub mod light_gpu;
pub mod light_list;
pub mod light_scene;
#[cfg(feature = "gpu")]
pub mod light_look;
#[cfg(feature = "gpu")]
pub mod light_models;
pub mod lights;
#[cfg(feature = "gpu")]
pub mod load;
#[cfg(feature = "gpu")]
pub mod look;
#[cfg(feature = "gpu")]
pub mod pass_state;
#[cfg(feature = "gpu")]
pub mod program;
pub mod model;
#[cfg(feature = "gpu")]
pub mod model_gpu;
#[cfg(feature = "gpu")]
pub mod model_load;
#[cfg(feature = "gpu")]
pub mod object_program;
#[cfg(feature = "gpu")]
pub mod object_program_gpu;
#[cfg(feature = "gpu")]
pub mod object_state;
pub mod object_streams;
pub mod particles;
#[cfg(feature = "gpu")]
pub mod particle_gpu;
pub mod post;
#[cfg(feature = "gpu")]
pub mod screen_gpu;
#[cfg(feature = "gpu")]
pub mod screen_look;
#[cfg(feature = "gpu")]
pub mod shadow_gpu;
pub mod shadows;
pub mod transparent;
#[cfg(feature = "gpu")]
pub mod transparent_streams;
#[cfg(feature = "gpu")]
pub mod water_gpu;
pub mod world;
#[cfg(feature = "gpu")]
pub mod world_layers_gpu;

pub use scene_extract::layout::{aux_depth, light_passes, object_lighting, object_nodes, object_walk, objects, skip, world_layers, world_ranges};
