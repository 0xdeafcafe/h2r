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

pub mod camera;
pub mod cull;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod instances;
#[cfg(feature = "gpu")]
pub mod load;
pub mod model;
#[cfg(feature = "gpu")]
pub mod model_gpu;
#[cfg(feature = "gpu")]
pub mod model_load;
pub mod objects;
pub mod skip;
pub mod world;
