//! The frame types the guest's Direct3D device and `FrameScene` share
//! (docs/graphics/framescene.md): the carried part of a frame by slot
//! ([`carried`]), the render camera ([`camera`]) and the view and object
//! keys ([`keys`]).
//!
//! `d3d` records the carried part and classifies its draws against the
//! scene's cameras; `frame_scene` holds them in the sealed scene and
//! re-exports this crate's modules under its own paths. Plain data: no
//! guest addresses, no GPU, no dependency on either side.

pub mod camera;
pub mod carried;
pub mod keys;

pub use camera::{Camera, D24_SCALE, Mat4, RenderCamera, mul, transform, w_depth};
pub use carried::{
    AuxTarget, CameraUse, Carried, CarriedOp, CarriedSummary, GuestOp, OpCounts, Readback, SideTarget, Slot, SlotAt,
    SlotEvent, SlotTracker, TargetRef, TargetRole, TargetSample,
};
pub use keys::{InstanceKey, ViewKey, ViewKind};
