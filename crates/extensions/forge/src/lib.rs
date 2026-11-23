//! A local Forge host. The caller supplies the loaded map's palette, bounds,
//! teleporters and rows, and invokes edits at a game boundary after checking
//! session access.
#![forbid(unsafe_code)]

pub mod camera;
mod files;
mod links;
pub mod look;
pub mod net;
pub mod props;
mod scene;
mod session;
mod things;
pub mod tool;

pub use files::{load_saved, saved_names};
pub use forge_doc::{BlockKind, Endpoint, OpId, TagKey, Value};
pub use links::{Linkable, Teleporter};
pub use session::{Outgoing, SHARED_PROPOSE, SHARED_REDO, Session, SessionAdmit};
pub use things::{BaseRow, BaseRows, LEFT_OUT_SENTENCE, Thing, ThingKind};

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
///
/// The base calls edit the map's own entries
/// (docs/game/forge-map-variants.md, "Applying an edit in a session"). They
/// name an entry by its block and the index the base map has it at: a
/// loaded block is never reordered under a running game, so the index holds
/// for the session. A world without them refuses each by name.
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
    /// Move and turn own entry `index` of `block`: its position and
    /// orientation in the loaded scenario, and its live object when the
    /// block makes objects.
    fn base_move(&mut self, block: BlockKind, index: u32, frame: Frame) -> Result<(), String> {
        let _ = (block, index, frame);
        Err(NO_BASE.into())
    }
    /// Set one typed, editable field of the entry in the loaded scenario.
    fn base_field(
        &mut self,
        block: BlockKind,
        index: u32,
        field: &str,
        value: &Value,
    ) -> Result<(), String> {
        let _ = (block, index, field, value);
        Err(NO_BASE.into())
    }
    /// Make the entry's row inert and delete its live object.
    fn base_inert(&mut self, block: BlockKind, index: u32) -> Result<(), String> {
        let _ = (block, index);
        Err(NO_BASE.into())
    }
    /// Write the kept `row` back and make the entry's object again.
    fn base_restore(&mut self, block: BlockKind, index: u32, row: &[u8]) -> Result<(), String> {
        let _ = (block, index, row);
        Err(NO_BASE.into())
    }
}

const NO_BASE: &str = "This game cannot edit the map's own entries.";

#[cfg(test)]
mod tests;

#[cfg(test)]
mod base_tests;

#[cfg(test)]
mod props_tests;
