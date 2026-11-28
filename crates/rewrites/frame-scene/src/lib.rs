//! `FrameScene`: what the native renderer reads (docs/graphics/framescene.md).
//!
//! Owned, typed data with no guest addresses and no borrowed guest
//! memory: built on the sim thread during the frame step that follows the
//! frame's ticks ([`SceneBuilder`]), sealed, and published as an
//! `Arc<FrameScene>`. This crate knows neither the guest nor the GPU, so
//! the editor and the viewer can build scenes too. `scene_extract` fills
//! it from guest memory; `native_render` draws it. The camera, the carried
//! part and the view and instance keys are `frame_types`', re-exported.

pub mod transparent;
pub mod blend;
pub mod builder;
pub mod decal;
pub mod decorator;
pub mod display;
pub mod effect;
pub mod late;
pub mod layer;
pub mod light;
pub mod light_group;
pub mod mailbox;
pub mod node;
pub mod object;
pub mod pace;
pub mod overlay;
pub mod scene;
pub mod summary;
pub mod view_slot;
pub use view_slot::ViewSlot;

// The render camera and the carried part are shared with the guest's
// device (`d3d`), which sits below this crate: they live in `frame_types`
// and keep their paths here.
pub use frame_types::{camera, carried};

pub use builder::{BuildError, FreeList, InstanceInput, SceneBuilder};
pub use camera::{Camera, D24_SCALE, Mat4, RenderCamera, mul, transform, w_depth};
pub use carried::{
    AuxTarget, Carried, CarriedOp, CarriedSummary, GuestOp, OpCounts, Readback, SideTarget, Slot,
    SlotAt, SlotEvent, SlotTracker, TargetRef, TargetRole, TargetSample,
};
pub use decorator::{DecoratorCounts, DecoratorDraw, DecoratorInstance};
pub use late::{
    DeviceFog, FOG_WORDS, FogState, LateCounts, LateInput, LateLayer, LateMaterial, LatePart,
    PostEffect, PostParams, WaterState,
};
pub use layer::{
    LayerInputs, LayerItem, LayerObjectRefusal, LayerOwner, LayerPass, PRT_FIRST_CONSTANT,
    PrtStream,
};
pub use light::{
    Light, LightCandidate, LightConstants, LightProjection, LightStep, LightTarget, LightVariants,
    LightVolume, NO_DATUM, SceneLight,
};
pub use light_group::{
    EntryState, GroupSite, LightBinding, LightEntry, LightGroup, LightPose, LightStream, ObjectShadow,
    PROJECTION_ROWS, SHADOW_COMBINER_WORDS, ShadowFilter, ShadowFilterDraw, ShadowPass, Topology,
};
pub use node::{NODE_MATRIX_SIZE, NodeMatrix};
pub use object::{Defaults, ItemKey, LIGHTING_BYTES, LIGHTING_WORDS, Lighting, ObjectFrame, Owner};
pub use overlay::{
    ClipRect, Combiner, Overlay, OverlayBlend, OverlayBlur, OverlayCamera, OverlayCounts,
    OverlayError, OverlayGlyph, OverlayItem, OverlayKind, OverlayMaterial, OverlayModelView, OverlayMovie,
    OverlayTexture, OverlayVertex, Program, Sampler,
};
pub use scene::{
    AuxDepth, Cut, DisplayVisibility, FrameScene, Instance, InstanceKey, NearbyCaster, PartDraw, PartFlags, Row,
    SceneId, Stamp, StructureParameters, Subparts, View, ViewCounts, ViewKey, ViewKind, VisibilitySource, WorldState,
};
pub use summary::summary_hash;

#[cfg(test)]
pub(crate) mod test_camera {
    use crate::RenderCamera;

    /// A camera at (1, 2, 3) looking down +x, z up, 640x480, 70 degrees
    /// (the same as `frame_types::camera`'s tests use).
    pub fn camera() -> RenderCamera {
        RenderCamera {
            position: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 70f32.to_radians(),
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.0625,
            far: 1024.0,
        }
    }
}
