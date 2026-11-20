//! The read behind occlusion culling of objects
//! (docs/graphics/occlusion-culling.md): where the game chooses a view's
//! objects and where its camera, view index and frame counter live
//! ([`guest`]). The Hi-Z culling built on it was not adopted and is
//! deleted (docs/graphics/notes/not-adopted.md); the addresses stay for
//! the native renderer, the scene step and the camera readers.

pub mod guest;
