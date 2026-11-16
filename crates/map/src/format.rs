use crate::{Error, Result};
use std::fmt;

/// A tag class in human-readable order, e.g. `scnr` or `snd!`.
/// On disk its bytes are reversed because the FourCC is a little-endian u32.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct FourCC(pub [u8; 4]);

impl FourCC {
    /// Decode the numeric, little-endian on-disc value.
    pub fn from_u32(value: u32) -> Self {
        Self(value.to_be_bytes())
    }
    /// Encode the numeric value (write using `to_le_bytes`).
    pub fn as_u32(self) -> u32 {
        u32::from_be_bytes(self.0)
    }
}
impl fmt::Display for FourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            for c in std::ascii::escape_default(b) {
                write!(f, "{}", c as char)?;
            }
        }
        Ok(())
    }
}
impl std::str::FromStr for FourCC {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let bytes: [u8; 4] = s
            .as_bytes()
            .try_into()
            .map_err(|_| Error::Invalid("group must be four ASCII bytes"))?;
        if !bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
            return Err(Error::Invalid("group must be four printable ASCII bytes"));
        }
        Ok(Self(bytes))
    }
}

/// Salted datum handle. Its low 16 bits select a tag-index slot.
/// The complete value must match; using only the slot accepts stale handles.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct TagId(pub u32);
impl TagId {
    /// The null handle used by tag references.
    pub const NULL: Self = Self(u32::MAX);
    /// Slot in the tag index (not sufficient to identify a tag).
    pub fn index(self) -> usize {
        (self.0 & 0xffff) as usize
    }
    /// Generation/salt stored in the upper half.
    pub fn salt(self) -> u16 {
        (self.0 >> 16) as u16
    }
}
impl fmt::Display for TagId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:08x}", self.0)
    }
}

/// Cache purpose, retaining unknown values for inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheType {
    /// Campaign level.
    Campaign,
    /// Multiplayer level.
    Multiplayer,
    /// Main menu.
    MainMenu,
    /// Multiplayer shared resources.
    Shared,
    /// Campaign shared resources.
    SinglePlayerShared,
    /// An unrecognized type.
    Unknown(u32),
}
impl From<u32> for CacheType {
    fn from(n: u32) -> Self {
        match n {
            0 => Self::Campaign,
            1 => Self::Multiplayer,
            2 => Self::MainMenu,
            3 => Self::Shared,
            4 => Self::SinglePlayerShared,
            n => Self::Unknown(n),
        }
    }
}

/// Location of an indexed NUL-terminated string table.
#[derive(Clone, Copy, Debug)]
pub struct StringTableLocation {
    /// Number of u32 offsets.
    pub count: u32,
    /// Map-relative offset of the u32 index.
    pub index_offset: u32,
    /// Map-relative offset of the string blob.
    pub data_offset: u32,
    /// Size of the string blob in bytes.
    pub data_size: u32,
}

/// Selected fields of the 0x800-byte retail header. Unknown bytes are not interpreted.
#[derive(Clone, Debug)]
pub struct Header {
    /// Cache version, always 8 for accepted files.
    pub version: u32,
    /// Declared file size, excluding any trailing storage padding.
    pub file_size: u32,
    /// File offset of the tag-index header.
    pub index_offset: u32,
    /// Size from the index header to ordinary tag data (not an absolute offset).
    pub index_size: u32,
    /// Ordinary tag-data region length.
    pub tag_data_size: u32,
    /// Total in-memory metadata size including BSP space.
    pub meta_size: u32,
    /// Retail executable build string.
    pub build: String,
    /// Cache purpose.
    pub cache_type: CacheType,
    /// Internal map name.
    pub name: String,
    /// Scenario path supplied in the cache header.
    pub scenario_path: String,
    /// Indexed string-ID table (not localized UI text).
    pub strings: StringTableLocation,
    /// Indexed full tag paths, ordered like the tag index.
    pub tag_names: StringTableLocation,
    /// Offset of the auxiliary fixed-width, 128-byte string slots.
    pub fixed_strings_offset: u32,
    /// Map-relative start of the low-detail bitmap resource region.
    pub low_detail_offset: u32,
    /// Byte length of the low-detail bitmap resource region.
    pub low_detail_size: u32,
    /// Recorded XOR checksum; not verified by the reader.
    pub checksum: u32,
}

/// Class inheritance entry from the index's group table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagGroup {
    /// Concrete class.
    pub class: FourCC,
    /// Parent, unless the file stores 0xffffffff.
    pub parent: Option<FourCC>,
    /// Grandparent, unless the file stores 0xffffffff.
    pub grandparent: Option<FourCC>,
}

/// A contiguous file region mapped into tag virtual memory.
/// BSPs can reuse virtual addresses, so retain the region belonging to the tag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressSpace {
    /// Virtual address corresponding to `file_offset`.
    pub virtual_base: u32,
    /// Map-relative start.
    pub file_offset: u64,
    /// Region size in bytes.
    pub size: u32,
}
impl AddressSpace {
    /// Virtual-address minus file-offset, traditionally called "magic".
    pub fn magic(self) -> i64 {
        i64::from(self.virtual_base) - self.file_offset as i64
    }
    /// Translate a complete range, rejecting pointers outside this region.
    pub fn translate(self, address: u32, size: u32) -> Result<u64> {
        let relative = address.checked_sub(self.virtual_base).map(u64::from);
        if let Some(r) = relative
            && r + u64::from(size) <= u64::from(self.size)
        {
            return self
                .file_offset
                .checked_add(r)
                .ok_or(Error::Invalid("file offset overflow"));
        }
        Err(Error::Bounds {
            what: "virtual address",
            offset: u64::from(address),
            size: u64::from(size),
            limit: u64::from(self.virtual_base) + u64::from(self.size),
        })
    }
}

/// Resolved tag payload, including BSP-specific address translation.
#[derive(Clone, Copy, Debug)]
pub struct TagLocation {
    /// Payload's virtual address.
    pub address: u32,
    /// Contiguous payload size; does not include external raw resources.
    pub size: u32,
    /// Region to use for this tag's embedded metadata pointers.
    pub space: AddressSpace,
}
impl TagLocation {
    /// Map-relative start of the payload.
    pub fn file_offset(self) -> Result<u64> {
        self.space.translate(self.address, self.size)
    }
}

/// One live tag, retaining its original index address and size as well as its
/// resolved payload. Null index slots are omitted from [`crate::Map::tags`].
#[derive(Clone, Debug)]
pub struct Tag {
    /// Full salted identity.
    pub id: TagId,
    /// Concrete class, e.g. `matg`.
    pub group: FourCC,
    /// Full path without a class extension; may be absent in stripped maps.
    pub name: Option<String>,
    /// Original index address; BSP/lightmap entries usually store zero.
    pub index_address: u32,
    /// Original index size; BSP/lightmap entries usually store zero.
    pub index_size: u32,
    /// Resolved local payload, if one exists.
    pub location: Option<TagLocation>,
}

/// Query by full identity or name. Names are case-sensitive and use the cache's
/// backslash separators. Shared names across classes require qualification.
#[derive(Clone, Copy, Debug)]
pub enum TagQuery<'a> {
    /// Full salted datum.
    Id(TagId),
    /// Unique full tag path.
    Name(&'a str),
    /// Full tag path and class.
    NameInGroup(&'a str, FourCC),
}
impl From<TagId> for TagQuery<'_> {
    fn from(id: TagId) -> Self {
        Self::Id(id)
    }
}
impl From<u32> for TagQuery<'_> {
    fn from(id: u32) -> Self {
        Self::Id(TagId(id))
    }
}
impl<'a> From<&'a str> for TagQuery<'a> {
    fn from(name: &'a str) -> Self {
        Self::Name(name)
    }
}

/// Which cache a raw-resource pointer addresses. These are not virtual pointers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceFile {
    /// The map containing the pointer.
    Local,
    /// `mainmenu.map`.
    MainMenu,
    /// `shared.map`.
    Shared,
    /// `single_player_shared.map`.
    SinglePlayerShared,
}
/// Decoded top-two-bit file selector and low-30-bit file offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawPointer {
    /// File to open; the caller supplies its own disc resources.
    pub file: ResourceFile,
    /// Pure file offset, with no magic applied.
    pub offset: u32,
}
impl RawPointer {
    /// Decode a raw pointer. `0xffffffff` is treated as absent, not a huge offset.
    pub fn decode(value: u32) -> Option<Self> {
        if value == u32::MAX {
            return None;
        }
        let file = match value >> 30 {
            0 => ResourceFile::Local,
            1 => ResourceFile::MainMenu,
            2 => ResourceFile::Shared,
            _ => ResourceFile::SinglePlayerShared,
        };
        Some(Self {
            file,
            offset: value & 0x3fff_ffff,
        })
    }
}
