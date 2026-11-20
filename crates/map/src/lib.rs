//! Halo 2 retail Xbox (cache v8) reader and writer, with no game data or unsafe code.
//!
//! [`Map::open`] opens a file; [`Map::from_reader`] accepts a seekable stream,
//! and [`Map::from_region`] reads directly from a bounded region of a disc.
//! Only indexes and strings are loaded eagerly. Tag payloads are read on demand.
//! All file offsets are relative to the map, even when it lives inside a disc.
//!
//! See `docs/formats/map-format.md` for provenance, addressing rules and limitations.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Whole-cache builder: write a complete map from a set of tags.
pub mod build;
/// Engine checksum and streaming checksum state.
pub mod checksum;
mod format;
mod reader;
/// Converted campaign recording controls shared by the encoder and playback.
pub mod recording;
/// Pass-through cache writing (not a rebuild) and bounded edits.
pub mod writer;

pub use format::*;
pub use reader::Map;

/// A malformed, unsupported or unreadable cache file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The backing stream failed.
    #[error("map I/O: {0}")]
    Io(#[from] std::io::Error),
    /// A structural invariant was violated.
    #[error("invalid map: {0}")]
    Invalid(&'static str),
    /// Cache version or platform is outside this reader's scope.
    #[error("unsupported cache: {0}")]
    Unsupported(String),
    /// A file range falls outside the map or a metadata pointer's region.
    #[error("{what} range {offset:#x}+{size:#x} exceeds {limit:#x}")]
    Bounds {
        /// The range's purpose.
        what: &'static str,
        /// Requested offset.
        offset: u64,
        /// Requested byte count.
        size: u64,
        /// Exclusive upper bound.
        limit: u64,
    },
    /// A built map's tag memory (its index, BSP region and metadata,
    /// header `+0x1c`) is over the engine's for its cache type; says by how
    /// much and which tag groups take the most.
    #[error("metadata exceeds the engine's tag memory for this cache type: {0}")]
    TagMemory(String),
    /// An allocation or configured table-size limit was exceeded.
    #[error("map allocation exceeds supported limits")]
    Limit,
    /// A string is not UTF-8 (retail tables are ASCII).
    #[error("invalid UTF-8 in {0}")]
    Encoding(&'static str),
    /// No live tag matches a query, including its datum salt.
    #[error("tag not found")]
    TagNotFound,
    /// Several groups share the requested name; qualify with a group or ID.
    #[error("ambiguous tag name; use TagQuery::NameInGroup or a TagId")]
    AmbiguousName,
    /// A tag has no local payload, e.g. an unresolved null index address.
    #[error("tag has no payload in this map")]
    NoTagData,
}

/// Result type for this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod build_tests;

/// Marker in a converted hlmt voice variant's word at +0x24. Its low
/// word is the signed Halo 1 selector. Ordinary model variants opt out.
pub const H1_VOICE_VARIANT: u32 = 0x4831_0000;

/// Marker in a converted udlg source row's word at +0x00, outside the
/// native indexed vocabulary. Its low word is the Halo 1 slot index.
pub const H1_DIALOGUE_SLOT: u32 = 0x4831_0000;

/// The original Halo 1 damage category, kept at jpt! +0x34 for dialogue.
/// Halo 2's own damage category at +0x12 uses Halo 2's numbering.
pub const H1_DAMAGE_CATEGORY: u32 = 0x4831_0000;

/// Unit death line for the authored Halo 1 damage impulse threshold.
pub const H1_DEATH_FLYING_LINE: usize = 0x140;

/// Halo 1 kill stimulus 0's attacker lines for damage categories that
/// have no distinct Halo 2 vocalization. The second word is a source
/// udlg byte offset.
pub const H1_WEAPON_KILL_LINES: &[(u16, usize)] = &[
    (5, 0x450),  // sniper rifle
    (8, 0x4c0),  // mounted weapon
    (11, 0x430), // needler
    (12, 0x4a0), // shotgun
];

/// Speaker-independent victim classes used by Halo 1's automatic dialogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum H1DialogueClass {
    Other = 0,
    Player = 1,
    Human = 2,
    Covenant = 4,
    CombatForm = 8,
    Carrier = 16,
    InfectionForm = 32,
    Sentinel = 64,
}

impl H1DialogueClass {
    /// H1's actor-kind classification at 0x146089. Halo 2 preserves these
    /// actor type values when installing the converted actor's char.
    pub const fn actor(kind: u16) -> Self {
        match kind {
            0..=5 | 14 => Self::Covenant,
            6..=8 => Self::Human,
            9 => Self::CombatForm,
            10 => Self::InfectionForm,
            11 => Self::Carrier,
            12 | 13 => Self::Sentinel,
            _ => Self::Other,
        }
    }

    pub const fn hostile_kill_line(self) -> Option<usize> {
        match self {
            Self::Covenant => Some(0x3a0),
            Self::CombatForm => Some(0x3c0),
            Self::Carrier => Some(0x3e0),
            Self::Sentinel => Some(0x400),
            _ => None,
        }
    }
}

/// Victim-specific attacker cheers that use the generic H2 kill candidate.
pub const fn h1_species_kill_line(offset: usize) -> bool {
    matches!(offset, 0x3a0 | 0x3c0 | 0x3e0 | 0x400)
}
