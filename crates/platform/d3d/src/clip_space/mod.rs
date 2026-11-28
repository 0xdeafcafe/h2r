//! Where a vertex program puts its clip position, read from its microcode
//! and its constants: the program's own viewport epilogue ([`epilogue`]),
//! programs that pass window pixels straight through ([`pass_through`]),
//! and perspective matrices split into a projection and a rigid camera
//! ([`camera`], over [`math`]). The carried draws' camera classification
//! (`crate::scene_sink::camera`) and the device's projection checks
//! (`crate::device::projection`) read them.

pub use gfx::guest_projection::{camera, math};
pub mod epilogue;
pub mod pass_through;
