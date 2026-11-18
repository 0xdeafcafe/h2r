//! Format constants and the records the reader returns.
//!
//! Field offsets, with their evidence, are in `docs/halo1/map-format.md`.

use std::fmt;

/// Bytes in the uncompressed file header.
pub const HEADER_LEN: usize = 0x800;
/// The Xbox cache version.
pub const VERSION: u32 = 5;
/// `head` as a little-endian u32: the file holds the bytes `daeh`.
pub const HEAD: u32 = u32::from_be_bytes(*b"head");
/// `foot`, at [`FOOT_OFFSET`].
pub const FOOT: u32 = u32::from_be_bytes(*b"foot");
/// Where the header's `foot` sits.
pub const FOOT_OFFSET: usize = 0x7fc;
/// `tags`, the tag index header's closing FourCC.
pub const TAGS: u32 = u32::from_be_bytes(*b"tags");
/// The address the Xbox engine loads tag data at: the tag index header's
/// address. The reader derives the base from each file (the tag array follows
/// the index header) and checks it against this value.
pub const XBOX_METADATA_BASE: u32 = 0x803a_6000;
/// Bytes per tag array entry.
pub const TAG_ENTRY_LEN: usize = 0x20;
/// Bytes per tag block field (count, address, unused).
pub const BLOCK_LEN: usize = 0xc;
/// Bytes per tag reference field (group, name address, name length, ID).
pub const TAG_REF_LEN: usize = 0x10;
/// Largest decompressed file accepted. Retail maps are far smaller.
pub const MAX_FILE_LEN: u32 = 512 << 20;
/// Largest tag count accepted.
pub const MAX_TAGS: u32 = 0x10000;

/// The scenario's structure BSP block, from the start of the `scnr` tag.
pub const SCNR_STRUCTURE_BSPS: usize = 0x5a4;
/// Bytes per scenario structure BSP element.
pub const SCNR_BSP_LEN: usize = 0x20;

/// The header's cache type (`0x60`, a u16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheType {
    /// A campaign level.
    Campaign,
    /// A multiplayer level.
    Multiplayer,
    /// The user interface map.
    UserInterface,
    /// Any other value, as stored.
    Other(u16),
}

impl CacheType {
    /// From the stored value.
    pub fn from_raw(v: u16) -> Self {
        match v {
            0 => Self::Campaign,
            1 => Self::Multiplayer,
            2 => Self::UserInterface,
            v => Self::Other(v),
        }
    }

    /// The stored value.
    pub fn raw(self) -> u16 {
        match self {
            Self::Campaign => 0,
            Self::Multiplayer => 1,
            Self::UserInterface => 2,
            Self::Other(v) => v,
        }
    }
}

impl fmt::Display for CacheType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Campaign => f.write_str("campaign"),
            Self::Multiplayer => f.write_str("multiplayer"),
            Self::UserInterface => f.write_str("ui"),
            Self::Other(v) => write!(f, "type {v}"),
        }
    }
}

/// The `0x800`-byte file header, which is stored uncompressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Cache version (`0x04`), 5.
    pub version: u32,
    /// Decompressed file length, header included (`0x08`).
    pub file_length: u32,
    /// The u32 at `0x0c`, kept as stored: a padding length in community
    /// docs (Xbox only), unconfirmed.
    pub field_0c: u32,
    /// File offset of the tag index header in the decompressed file (`0x10`).
    pub index_offset: u32,
    /// Bytes of tag data from the index header on (`0x14`).
    pub metadata_size: u32,
    /// Internal map name (`0x20`, 32 bytes).
    pub name: String,
    /// Build string (`0x40`, 32 bytes).
    pub build: String,
    /// Cache type (`0x60`).
    pub cache_type: CacheType,
    /// The u32 at `0x64` (a checksum in community docs).
    pub checksum: u32,
}

/// The tag index header, at [`Header::index_offset`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexHeader {
    /// Address of the tag array (`0x00`).
    pub tag_array_address: u32,
    /// The scenario tag (`0x04`).
    pub scenario: TagId,
    /// Checksum (`0x08`).
    pub checksum: u32,
    /// Tag count (`0x0c`).
    pub tag_count: u32,
    /// The u32 fields between the tag count and `tags`, as stored: model
    /// part count, vertex data, part count again, index data (and on PC a
    /// model data size). See the format doc for which are verified.
    pub model_fields: Vec<u32>,
    /// Bytes from the index header's start to the end of its `tags`.
    pub len: u32,
}

impl IndexHeader {
    /// Model part count (`0x10`).
    pub fn model_part_count(&self) -> u32 {
        self.model_fields.first().copied().unwrap_or(0)
    }
    /// Model vertex data (`0x14`): an address on Xbox, an offset on PC.
    pub fn vertex_data(&self) -> u32 {
        self.model_fields.get(1).copied().unwrap_or(0)
    }
    /// Model index data (`0x1c`).
    pub fn index_data(&self) -> u32 {
        self.model_fields.get(3).copied().unwrap_or(0)
    }
}

/// A tag group: a FourCC such as `scnr`, stored as a little-endian u32.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Group(pub u32);

impl Group {
    /// No group (all bits set): a tag with no parent group.
    pub const NONE: Group = Group(u32::MAX);
    /// `scnr`.
    pub const SCENARIO: Group = Group::new(b"scnr");
    /// `sbsp`.
    pub const STRUCTURE_BSP: Group = Group::new(b"sbsp");

    /// From the four characters, as written.
    pub const fn new(code: &[u8; 4]) -> Self {
        Group(u32::from_be_bytes(*code))
    }

    /// Whether this is [`Group::NONE`].
    pub fn is_none(self) -> bool {
        self == Self::NONE
    }

    /// Parses a group name of up to four characters (`hmt` is `hmt `).
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.is_empty() || b.len() > 4 || !s.is_ascii() {
            return None;
        }
        let mut code = [b' '; 4];
        code[..b.len()].copy_from_slice(b);
        Some(Self::new(&code))
    }
}

impl fmt::Display for Group {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_none() {
            return f.write_str("none");
        }
        let b = self.0.to_be_bytes();
        if b.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
            f.write_str(std::str::from_utf8(&b).unwrap_or("????"))
        } else {
            write!(f, "{:#010x}", self.0)
        }
    }
}

impl fmt::Debug for Group {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Group({self})")
    }
}

/// A tag datum: the tag array index in the low 16 bits, a salt above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TagId(pub u32);

impl TagId {
    /// The null ID.
    pub const NONE: TagId = TagId(u32::MAX);

    /// Index into the tag array.
    pub fn index(self) -> u16 {
        self.0 as u16
    }
    /// The salt.
    pub fn salt(self) -> u16 {
        (self.0 >> 16) as u16
    }
    /// Whether this is the null ID.
    pub fn is_none(self) -> bool {
        self == Self::NONE
    }
}

impl fmt::Display for TagId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08x}", self.0)
    }
}

/// One tag array entry, with its name resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Group (`0x00`).
    pub group: Group,
    /// Parent group (`0x04`), or [`Group::NONE`].
    pub parent: Group,
    /// Grandparent group (`0x08`), or [`Group::NONE`].
    pub grandparent: Group,
    /// ID (`0x0c`).
    pub id: TagId,
    /// Address of the name (`0x10`).
    pub name_address: u32,
    /// Path, without extension, read from [`Tag::name_address`].
    pub name: String,
    /// Address of the tag's data (`0x14`); a resource index when
    /// [`Tag::external`].
    pub data_address: u32,
    /// The indexed/external flag (`0x18`) is non-zero: the data is in a
    /// shared resource file, not here.
    pub external: bool,
    /// File range of the data in the decompressed file: from the data
    /// address up to the next tag's data, or the end of the tag data. `None`
    /// for external tags and for addresses outside the tag data.
    pub data: Option<std::ops::Range<usize>>,
}

/// A tag block field: `count` elements at `address`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    /// Element count.
    pub count: u32,
    /// Address of element 0.
    pub address: u32,
}

impl Block {
    /// Reads the block field at `at` in a tag's bytes.
    pub fn read(bytes: &[u8], at: usize) -> Option<Self> {
        Some(Self {
            count: u32_at(bytes, at)?,
            address: u32_at(bytes, at + 4)?,
        })
    }
}

/// A tag reference field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagRef {
    /// The group the field names.
    pub group: Group,
    /// Address of the path string.
    pub name_address: u32,
    /// The path's length.
    pub name_len: u32,
    /// The referenced tag.
    pub id: TagId,
}

impl TagRef {
    /// Reads the reference field at `at` in a tag's bytes.
    pub fn read(bytes: &[u8], at: usize) -> Option<Self> {
        Some(Self {
            group: Group(u32_at(bytes, at)?),
            name_address: u32_at(bytes, at + 4)?,
            name_len: u32_at(bytes, at + 8)?,
            id: TagId(u32_at(bytes, at + 12)?),
        })
    }
}

/// One element of the scenario's structure BSP block: where the BSP's own
/// region of the file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BspRef {
    /// File offset of the BSP region in the decompressed file (`0x00`).
    pub file_offset: u32,
    /// Bytes in the region (`0x04`).
    pub size: u32,
    /// The address the region is loaded at (`0x08`).
    pub address: u32,
    /// The `sbsp` tag (`0x10`, a tag reference).
    pub tag: TagRef,
    /// That tag's name.
    pub name: String,
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at.checked_add(4)?)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at.checked_add(2)?)
        .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
}

/// A NUL-terminated string in a fixed field (lossy; retail fields are ASCII).
pub(crate) fn fixed_str(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}
