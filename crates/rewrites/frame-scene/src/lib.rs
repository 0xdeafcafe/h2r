//! `FrameScene`: what the native renderer reads (docs/graphics/framescene.md).
//!
//! Owned, typed data with no guest addresses and no borrowed guest
//! memory: built on the sim thread during the frame step that follows the
//! frame's ticks ([`SceneBuilder`]), sealed, and published as an
//! `Arc<FrameScene>`. This crate knows neither the guest nor the GPU, so
//! the editor and the viewer can build scenes too. `scene_extract` fills
//! it from guest memory; `native_render` draws it.

pub mod builder;
pub mod camera;
pub mod decorator;
pub mod layer;
pub mod light;
pub mod node;
pub mod object;
pub mod scene;
pub mod summary;

pub use builder::{BuildError, FreeList, InstanceInput, SceneBuilder};
pub use camera::{Camera, D24_SCALE, Mat4, RenderCamera, mul, transform, w_depth};
pub use decorator::{DecoratorCounts, DecoratorDraw, DecoratorInstance};
pub use layer::{LayerInputs, LayerItem, LayerOwner, LayerPass};
pub use light::{LightStep, LightTarget, SceneLight};
pub use node::{NODE_MATRIX_SIZE, NodeMatrix};
pub use object::{Defaults, ItemKey, LIGHTING_BYTES, LIGHTING_WORDS, Lighting, ObjectFrame, Owner};
pub use scene::{
    AuxDepth, Cut, FrameScene, Instance, InstanceKey, PartDraw, PartFlags, Row, SceneId, Stamp, View, ViewCounts, ViewKey,
    Subparts, ViewKind, WorldState,
};
pub use summary::summary_hash;
