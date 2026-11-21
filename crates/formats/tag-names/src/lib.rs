//! Developer comparison of retail Xbox plugin layouts and the checked-in tag definitions.
#![forbid(unsafe_code)]

pub mod assembly;
mod merge;
pub use merge::*;

/// Groups use the same case-sensitive spelling as `tags::defs::fourcc_text`.
pub trait Source {
    fn groups(&self) -> Vec<String>;
    fn load(&self, group: &str) -> Result<Option<Plugin>, String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plugin {
    pub group: String,
    pub game: String,
    pub base_size: u32,
    pub fields: Vec<Field>,
}

/// Offsets in a block are relative to one element. An unrecognised type
/// keeps its spelling and an unknown width until there is layout evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub offset: u32,
    pub kind: Kind,
    pub size: Option<u32>,
    pub visible: bool,
    pub comment: String,
    pub options: Vec<(i64, String)>,
    pub bits: Vec<(u32, String)>,
    pub children: Vec<Field>,
    pub element_size: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Int { signed: bool },
    /// Packed arrays of signed 16-bit components.
    Int16Array,
    Float,
    Bounds,
    Degree,
    Vector,
    Colour,
    PackedColour,
    Enum,
    Bits,
    StringId,
    TagRef,
    Datum,
    DataRef,
    Block,
    Ascii,
    Undefined,
    Unknown(String),
}
