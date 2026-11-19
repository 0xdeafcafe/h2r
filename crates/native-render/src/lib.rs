//! Native GPU renderer (docs/graphics/native-renderer.md).
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

pub mod camera;
pub mod cull;
pub mod gels;
#[cfg(feature = "gpu")]
pub mod gels_gpu;
#[cfg(feature = "gpu")]
pub mod glow;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod instances;
#[cfg(feature = "gpu")]
pub mod light_gpu;
#[cfg(feature = "gpu")]
pub mod light_look;
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
pub mod object_lighting;
pub mod object_nodes;
#[cfg(feature = "gpu")]
pub mod object_program;
#[cfg(feature = "gpu")]
pub mod object_program_gpu;
#[cfg(feature = "gpu")]
pub mod object_state;
pub mod object_streams;
pub mod objects;
pub mod particles;
#[cfg(feature = "gpu")]
pub mod particle_gpu;
pub mod post;
#[cfg(feature = "gpu")]
pub mod screen_gpu;
#[cfg(feature = "gpu")]
pub mod screen_look;
pub mod skip;
pub mod transparent;
#[cfg(feature = "gpu")]
pub mod transparent_streams;
pub mod world;
