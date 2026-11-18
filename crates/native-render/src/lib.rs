//! Native GPU renderer (docs/graphics/native-renderer.md).
//!
//! Stage 1, the static world: the structure BSP's opaque geometry is read
//! from the map's tags once ([`load`], [`world`]), uploaded once
//! ([`gpu`]), and drawn each frame from the game's render camera
//! ([`camera`]) with frustum-culled indirect draws ([`cull`]), while the
//! game skips its own layer-3 entries for it ([`skip`]). Nothing here
//! writes guest memory except [`skip`]'s mask bits, which it restores.

pub mod camera;
pub mod cull;
#[cfg(feature = "gpu")]
pub mod gpu;
#[cfg(feature = "gpu")]
pub mod load;
#[cfg(feature = "gpu")]
pub mod look;
#[cfg(feature = "gpu")]
pub mod program;
pub mod skip;
pub mod world;
