//! What an object draws with, by value: its lighting copy, the inputs'
//! defaults, the item key the guest picked, and its pose. Read from guest
//! memory by `scene_extract::layout` (`objects`, `object_lighting`); the
//! offsets live there, not here.

use serde::Serialize;

use crate::node::NodeMatrix;

/// Bytes of an object's lighting the inputs read (`0x78`).
pub const LIGHTING_BYTES: u32 = 0x78;
/// Its words.
pub const LIGHTING_WORDS: usize = (LIGHTING_BYTES / 4) as usize;

/// One object's lighting: a copy of the `0x78` bytes the inputs read,
/// taken when the hook meets the object (the cache record is relit and
/// reused after the frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Lighting {
    pub words: [u32; LIGHTING_WORDS],
    /// The root value (`scene_extract::layout::object_lighting::root_value`)
    /// of the object `+0` names, read with the words; `None` when the walk
    /// did not finish (unreadable memory, a cycle): inputs 40 and 37 are
    /// then not given.
    pub root: Option<u32>,
}

impl Lighting {
    /// The word at byte `offset`.
    pub fn word(&self, offset: u32) -> u32 {
        self.words[(offset / 4) as usize]
    }

    /// Three words from byte `offset`.
    pub fn vector(&self, offset: u32) -> [u32; 3] {
        [self.word(offset), self.word(offset + 4), self.word(offset + 8)]
    }

    /// The colour slots in use (`+0x74`).
    pub fn colour_count(&self) -> u8 {
        self.word(0x74) as u8
    }

    /// The byte `+0x76` the build picks the implementation by.
    pub fn implementation_byte(&self) -> u8 {
        (self.word(0x74) >> 16) as u8
    }
}

/// What the inputs fall back on without a lighting (or past the colour
/// count), read from the globals at the same time as the lighting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Defaults {
    pub vector: [u32; 3],
    pub colour: [u32; 3],
}

/// What one layer-3 item draws, as the guest picked it: its render record,
/// the shader the item names (`+8`), the record key's section (bits 9..17)
/// and part (bits 0..3, the index into the section's parts), and the
/// layer-3 implementation the record's `+0` picks
/// (`scene_extract::layout::object_nodes::record_implementation`). The
/// native pass draws exactly this part with exactly this implementation,
/// or the item keeps its D3D draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct ItemKey {
    pub record: u16,
    pub shader: u32,
    pub section: u16,
    pub part: u8,
    pub implementation: usize,
}

/// One object as the native pass draws it this view.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ObjectFrame {
    pub handle: u32,
    /// The definition tag datum.
    pub definition: u32,
    /// World-space node matrices, in model node order.
    pub nodes: Vec<NodeMatrix>,
    /// World bounding sphere: centre, radius.
    pub sphere: ([f32; 3], f32),
    /// Its lighting, copied from its render-state cache record; `None`
    /// without one.
    pub lighting: Option<Lighting>,
    /// The inputs' defaults, read with it; `None` when they do not read.
    pub defaults: Option<Defaults>,
}

/// What a draw belongs to in the BSP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Owner {
    /// A cluster, by index in `sbsp+0x9c`.
    Cluster(u32),
    /// An instanced-geometry placement, by index in `sbsp+0x140`.
    Instance(u32),
}
