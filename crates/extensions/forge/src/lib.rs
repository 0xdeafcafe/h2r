//! A local Forge host. The caller supplies the loaded map's palette, bounds
//! and teleporters, and invokes edits at a game boundary after checking
//! session access.
#![forbid(unsafe_code)]

pub mod camera;
mod files;
mod links;
mod scene;
mod session;
pub mod tool;

pub use files::saved_names;
pub use forge_doc::{BlockKind, OpId, TagKey};
pub use links::{Linkable, Teleporter};
pub use session::Session;

/// Position in world units, and yaw, pitch and roll in radians, in scenario order.
#[derive(Clone, Copy, Debug, Default)]
pub struct Frame {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
}

// Forge's canonical floats distinguish positive and negative zero. The
// current frame is also the source of the next typed field precondition.
impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.position.map(f32::to_bits) == other.position.map(f32::to_bits)
            && self.rotation.map(f32::to_bits) == other.rotation.map(f32::to_bits)
    }
}

/// A tag already referenced by the named scenario block's palette.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaletteItem {
    pub block: BlockKind,
    pub tag: TagKey,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedObject {
    /// Stable across moves, undo and reload. The engine handle can change.
    pub id: OpId,
    pub handle: u32,
    pub item: PaletteItem,
    pub frame: Frame,
}

/// Game-boundary operations. A refused call must leave the world untouched.
/// Handles are local to the current map and never enter the document or log.
pub trait World {
    fn place(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String>;
    fn move_object(&mut self, handle: u32, frame: Frame) -> Result<(), String>;
    fn delete(&mut self, handle: u32) -> Result<(), String>;
    fn respawn(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String>;
    /// Set a teleporter netgame flag's identifier, its channel. Flags are
    /// named by their index in the loaded scenario.
    fn channel(&mut self, flag: u32, channel: i16) -> Result<(), String> {
        let _ = (flag, channel);
        Err("This game cannot change teleporter channels.".into())
    }
}

#[cfg(test)]
mod tests;
