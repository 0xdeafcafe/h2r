//! The render camera. The type and its view and projection are the
//! scene's ([`frame_scene::camera`]); the guest record and its read are
//! extraction's ([`scene_extract::layout::camera`]). [`GuestRead`] is
//! re-exported for the stages here that still read guest memory at their
//! hooks, until they move to `scene_extract` (slices 3 to 6).

pub use frame_scene::camera::*;
pub use scene_extract::layout::camera::{RENDER_CAMERA, read};
pub use scene_extract::{GuestRead, GuestWrite};

#[cfg(test)]
pub(crate) mod tests {
    pub use scene_extract::testing::Mem;
}
