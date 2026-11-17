//! Whole-cache builder: write a complete v8 map from a [`TagSet`].
//!
//! [`crate::writer::MapEditor`] patches an existing map in place. This module
//! instead lays out a new file: header, raw resources, BSP regions, the
//! string-ID and tag-name tables, the tag index and the ordinary metadata.
//!
//! v8 metadata holds absolute virtual addresses, so a tag laid out at a new
//! address only works if every pointer inside it is known. A tag therefore
//! enters the set in one of two forms ([`TagPayload`]):
//!
//! - **Built**: a tree of [`Block`]s. Each block is plain bytes plus explicit
//!   [`Fixup`]s for every field whose value depends on layout: child blocks,
//!   data references and bare pointers (addresses), tag references and datums
//!   (checked against the set), string IDs (interned into the string table),
//!   raw-resource pointers (file offset plus file selector) and BSP descriptor
//!   fields. The builder chooses addresses and patches them. Bytes without a
//!   fixup are copied as they are.
//! - **Pinned**: opaque bytes that must sit at one fixed virtual address,
//!   because they hold pointers nobody has described. Retail tags enter this
//!   way through [`TagSet::from_map`], which also pins the original file
//!   prefix (raw data, BSP regions) and the string-ID order, so the opaque
//!   bytes stay valid.
//!
//! A tag whose whole layout is known (for example a bitmap, through the
//! `tags` crate) can be converted from pinned to built and then moves freely.
//! See `docs/formats/map-builder.md` for the model, the layout rules and what
//! has been verified.
use crate::checksum::XorChecksum;
use crate::writer::{cache_size_limit, max_metadata_size};
use crate::{CacheType, Error, FourCC, Map, ResourceFile, Result, TagGroup, TagId};
use std::collections::HashMap;
use std::io::{Read, Seek, Write};

/// Virtual address the retail loader puts the tag index at (V: every retail
/// map on the owner's disc). BSP regions follow it, then the metadata.
pub const INDEX_VIRTUAL_BASE: u32 = 0x8006_1000;
/// The only build string the reader accepts.
pub const BUILD_STRING: &str = "02.09.27.09809";
/// Page size used for the index, BSP regions and the metadata region.
pub const PAGE: u32 = 0x1000;
const SECTOR: u64 = 0x200;
const HEADER: usize = 0x800;
/// Width of one fixed string-ID slot (header `+0x160`).
pub const FIXED_STRING_SLOT: usize = 0x80;

/// A bounded piece of tag data plus the fields the builder must fill in.
///
/// This is the tag builder: make a block from its bytes (fields with known
/// values already written), then record a fixup for every layout-dependent
/// field. Child blocks hang off fixups, so a tag is a tree rooted at one block.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// Block bytes. Fixup fields may hold anything; they are overwritten.
    pub bytes: Vec<u8>,
    /// Alignment of the block's virtual address, a power of two (default 4).
    pub align: u32,
    /// Layout-dependent fields, by offset into `bytes`.
    pub fixups: Vec<Fixup>,
    /// Sharing key: every block with the same key is placed once (in the
    /// same region), and each reference points at that one copy. The caller
    /// promises blocks with one key are identical. Retail caches share
    /// identical blocks between tags this way.
    pub share: Option<u64>,
}

/// One layout-dependent field of a [`Block`].
#[derive(Clone, Debug, PartialEq)]
pub struct Fixup {
    /// Byte offset of the field in its block.
    pub offset: u32,
    /// What to write there.
    pub kind: FixupKind,
}

/// Values the builder writes once addresses, IDs and offsets are known.
#[derive(Clone, Debug, PartialEq)]
pub enum FixupKind {
    /// An 8-byte tag block: element count, then the child's address
    /// (zero when the count is zero and the child is empty).
    Block {
        /// Element count written at `offset`.
        count: u32,
        /// Element bytes, laid out after the parent.
        child: Block,
    },
    /// An 8-byte data reference: the child's byte length, then its address.
    Data(Block),
    /// A 4-byte virtual address of the child.
    Pointer(Block),
    /// An 8-byte tag reference: the target's group and datum. Null writes
    /// `0xffffffff` twice. A non-null target must be in the set.
    TagRef(TagId),
    /// A 4-byte datum. A non-null target must be in the set.
    TagId(TagId),
    /// A 4-byte string ID for this string, interned into the set's table
    /// (existing entries keep their index). Encoding: index | length << 24.
    StringId(String),
    /// A 4-byte raw-resource pointer (file selector in the top two bits).
    Raw(RawRef),
    /// A 4-byte field of the region of built BSP section `section`.
    Bsp {
        /// Index into [`TagSet::bsps`].
        section: usize,
        /// Which value.
        field: BspField,
    },
}

/// Values of a built BSP region, for the scenario's BSP descriptor block
/// (`+0x210`, stride `0x44`: file offset `+0`, size `+4`, address `+8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BspField {
    /// File offset of the region.
    FileOffset,
    /// Region size in the file and in memory (page multiple).
    Size,
    /// Virtual base address of the region (its 16-byte header).
    Address,
}

/// A raw-resource pointer target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawRef {
    /// A resource added with [`TagSet::add_raw`], placed in this map.
    Local(RawId),
    /// Bytes at a fixed offset of another cache (`mainmenu.map`,
    /// `shared.map`, `single_player_shared.map`), or of this map's pinned
    /// prefix ([`ResourceFile::Local`]).
    External {
        /// Which cache.
        file: ResourceFile,
        /// Offset in that file, below 2^30.
        offset: u32,
    },
    /// `0xffffffff`.
    Null,
}

/// Handle of a raw resource in a [`TagSet`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RawId(pub usize);

impl Block {
    /// A block of these bytes, aligned to 4, with no fixups.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            align: 4,
            fixups: Vec::new(),
            share: None,
        }
    }
    /// A zero-filled block.
    pub fn zeroed(size: usize) -> Self {
        Self::new(vec![0; size])
    }
    /// Set the sharing key (see [`Block::share`]).
    pub fn shared(mut self, key: u64) -> Self {
        self.share = Some(key);
        self
    }
    /// Set the address alignment (a power of two).
    pub fn aligned(mut self, align: u32) -> Self {
        self.align = align;
        self
    }
    /// Write a little-endian u32 at `offset` (panics when out of range).
    pub fn put_u32(&mut self, offset: usize, value: u32) -> &mut Self {
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        self
    }
    /// Write a little-endian u16 at `offset` (panics when out of range).
    pub fn put_u16(&mut self, offset: usize, value: u16) -> &mut Self {
        self.bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        self
    }
    /// Write a little-endian f32 at `offset` (panics when out of range).
    pub fn put_f32(&mut self, offset: usize, value: f32) -> &mut Self {
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        self
    }
    /// Record a fixup.
    pub fn fixup(&mut self, offset: u32, kind: FixupKind) -> &mut Self {
        self.fixups.push(Fixup { offset, kind });
        self
    }
    /// Tag block at `offset` with `count` elements stored in `child`.
    pub fn block(&mut self, offset: u32, count: u32, child: Block) -> &mut Self {
        self.fixup(offset, FixupKind::Block { count, child })
    }
    /// Data reference at `offset`.
    pub fn data(&mut self, offset: u32, child: Block) -> &mut Self {
        self.fixup(offset, FixupKind::Data(child))
    }
    /// Tag reference at `offset`.
    pub fn tag_ref(&mut self, offset: u32, id: TagId) -> &mut Self {
        self.fixup(offset, FixupKind::TagRef(id))
    }
    /// Datum at `offset`.
    pub fn tag_id(&mut self, offset: u32, id: TagId) -> &mut Self {
        self.fixup(offset, FixupKind::TagId(id))
    }
    /// String ID at `offset`.
    pub fn string_id(&mut self, offset: u32, value: &str) -> &mut Self {
        self.fixup(offset, FixupKind::StringId(value.to_owned()))
    }
    /// Raw-resource pointer at `offset`.
    pub fn raw(&mut self, offset: u32, raw: RawRef) -> &mut Self {
        self.fixup(offset, FixupKind::Raw(raw))
    }
    /// Bytes of this block and every descendant, without padding.
    pub fn tree_len(&self) -> u64 {
        self.bytes.len() as u64
            + self
                .fixups
                .iter()
                .filter_map(|f| f.kind.child())
                .map(Block::tree_len)
                .sum::<u64>()
    }
    /// Largest alignment in the tree.
    fn max_align(&self) -> u32 {
        self.fixups
            .iter()
            .filter_map(|f| f.kind.child())
            .map(Block::max_align)
            .fold(self.align, u32::max)
    }
    fn visit<'a>(&'a self, f: &mut impl FnMut(&'a FixupKind)) {
        for fixup in &self.fixups {
            f(&fixup.kind);
            if let Some(child) = fixup.kind.child() {
                child.visit(f);
            }
        }
    }
}
impl FixupKind {
    fn child(&self) -> Option<&Block> {
        match self {
            Self::Block { child, .. } | Self::Data(child) | Self::Pointer(child) => Some(child),
            _ => None,
        }
    }
    fn width(&self) -> u32 {
        match self {
            Self::Block { .. } | Self::Data(_) | Self::TagRef(_) => 8,
            _ => 4,
        }
    }
}

/// What a tag-index slot's tag holds.
#[derive(Clone, Debug, PartialEq)]
pub enum TagPayload {
    /// No metadata in the ordinary region: index address and size are zero.
    /// Used for BSP/lightmap tags whose bytes ride in a pinned BSP region.
    Empty,
    /// Opaque bytes at a fixed ordinary-metadata address.
    Pinned {
        /// Virtual address of the first byte.
        address: u32,
        /// The tag's bytes.
        bytes: Vec<u8>,
    },
    /// A block tree the builder lays out.
    Built(Block),
}

/// A live tag in a [`TagSet`].
#[derive(Clone, Debug, PartialEq)]
pub struct TagEntry {
    /// Full salted datum; its low 16 bits equal the slot.
    pub id: TagId,
    /// Concrete group.
    pub group: FourCC,
    /// The tag's data.
    pub payload: TagPayload,
}

/// One tag-index slot: a tag or a null slot, and the slot's name.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    /// Full tag path (no extension); retail null slots can have names too.
    pub name: Option<String>,
    /// The tag, or `None` for a null slot.
    pub tag: Option<TagEntry>,
}

/// A built BSP region: one structure BSP and its optional lightmap, both
/// [`TagPayload::Built`], laid out in the shared BSP address space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BspSection {
    /// The `sbsp` tag.
    pub bsp: TagId,
    /// The `ltmp` tag, if any.
    pub lightmap: Option<TagId>,
}

/// A raw resource placed in this map's file.
#[derive(Clone, Debug, PartialEq)]
pub struct RawBlob {
    /// Bytes as stored.
    pub bytes: Vec<u8>,
    /// File-offset alignment (a power of two, at least 4).
    pub align: u32,
}

/// What [`TagSet::from_map`] keeps verbatim so that pinned tags stay valid.
#[derive(Clone, Debug, PartialEq)]
pub struct PinnedImage {
    /// The source header; fields the builder owns are overwritten.
    pub header: Vec<u8>,
    /// File bytes from `0x800` up to the index: raw data, BSP regions,
    /// low-detail bitmaps, locale text, and the old string/name tables.
    pub file: Vec<u8>,
    /// Where the old tables were, as (file offset, byte length), in the
    /// order fixed string slots, string offsets, string blob, name blob,
    /// name offsets. A rebuilt table that fits goes back in its old place
    /// (rest zeroed); one that grew goes after the prefix.
    pub tables: [(u32, u32); 5],
    /// The whole original index region (`+0x14` bytes). The builder rewrites
    /// its header, groups and entries; the uninterpreted bytes after the
    /// entries (retail maps have about 0x38000 nonzero bytes there) stay.
    pub index: Vec<u8>,
    /// End of the original tag entries in the index region.
    pub index_entries_end: u32,
    /// Index size (`+0x14`): BSP regions load at `INDEX_VIRTUAL_BASE + this`.
    pub index_size: u32,
    /// Largest BSP region (`+0x1c - +0x14 - +0x18`).
    pub bsp_size: u32,
    /// Ordinary metadata base address.
    pub metadata_base: u32,
    /// Original ordinary metadata length (`+0x18`).
    pub metadata_size: u32,
    /// Metadata bytes no tag owns (nonzero gaps between tags), by address.
    pub unowned: Vec<(u32, Vec<u8>)>,
}

/// Owned tags plus everything else a v8 cache needs. Build with
/// [`TagSet::write_to`] / [`TagSet::build`].
#[derive(Clone, Debug, PartialEq)]
pub struct TagSet {
    /// Header `+0x140`.
    pub cache_type: CacheType,
    /// Internal map name (header `+0x198`, under 32 bytes).
    pub name: String,
    /// Scenario path (header `+0x1bc`, under 256 bytes).
    pub scenario_path: String,
    /// Group hierarchy table. Every tag's group must be declared.
    pub groups: Vec<TagGroup>,
    /// String-ID table in index order; `None` keeps a missing entry.
    pub strings: Vec<Option<String>>,
    /// Tag-index slots in order.
    pub slots: Vec<Slot>,
    /// Root scenario datum (`scnr`).
    pub scenario: TagId,
    /// Root globals datum (`matg`).
    pub globals: TagId,
    /// Uninterpreted index-header word `+0x14`.
    pub index_word_14: u32,
    /// Raw resources placed in this map.
    pub raw: Vec<RawBlob>,
    /// Built BSP regions.
    pub bsps: Vec<BspSection>,
    /// Base for new datum salts: slot `n` gets salt `salt_base + n`.
    pub salt_base: u16,
    /// Verbatim parts of a decomposed map, if any.
    pub pinned: Option<PinnedImage>,
    /// Place built tags first-fit into metadata no pinned bytes use (default).
    /// When false, every built tag goes after the original region.
    pub reuse_free_space: bool,
    /// Place built tags in reverse slot order (default false). Used to move
    /// relocated retail tags away from their old addresses while reusing
    /// the space they freed.
    pub reverse_built: bool,
    /// Enforce [`max_metadata_size`] (default true). Off only for offline
    /// round trips of maps already at the bound: the bound is the largest
    /// retail map of the type, not a measured engine limit.
    pub check_tag_memory: bool,
}

/// Where things went in a built map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildLayout {
    /// File length.
    pub file_size: u32,
    /// Header `+0x10`.
    pub index_offset: u32,
    /// Header `+0x14`.
    pub index_size: u32,
    /// Header `+0x18`.
    pub metadata_size: u32,
    /// Header `+0x1c`.
    pub memory_size: u32,
    /// Ordinary metadata base address.
    pub metadata_base: u32,
    /// Virtual address of every BSP region.
    pub bsp_base: u32,
    /// (address, size) of each slot's ordinary metadata; (0, 0) when none.
    pub tags: Vec<(u32, u32)>,
    /// File offset of each raw resource.
    pub raw: Vec<u32>,
    /// (file offset, region size) of each built BSP.
    pub bsps: Vec<(u32, u32)>,
    /// String-ID table length after interning.
    pub string_count: u32,
    /// XOR checksum written at `+0x2d0`.
    pub checksum: u32,
}

/// Header bytes a map built from scratch still needs from a pinned source:
/// the low-detail bitmap count, offset and size (`+0x154..+0x160`), which
/// pinned bitmaps index into.
pub const LOW_DETAIL_HEADER: std::ops::Range<usize> = 0x154..0x160;

impl PinnedImage {
    /// Zero the uninterpreted index bytes after the tag entries, as a map
    /// built from scratch has them. The index size, and so the BSP base,
    /// stay the same. Returns how many nonzero bytes were cleared.
    pub fn clear_index_tail(&mut self) -> usize {
        let start = (self.index_entries_end as usize).min(self.index.len());
        let tail = &mut self.index[start..];
        let cleared = tail.iter().filter(|&&b| b != 0).count();
        tail.fill(0);
        cleared
    }
    /// Replace the header template with zeros, keeping only
    /// [`LOW_DETAIL_HEADER`]: every field the builder does not write (the
    /// `+0x144` CRC field, build and dependency identities `+0x174..+0x198`,
    /// build date, and all other uninterpreted bytes) is left zero, as in a
    /// map built from scratch.
    pub fn clear_header(&mut self) {
        let mut header = vec![0; HEADER];
        header[LOW_DETAIL_HEADER].copy_from_slice(&self.header[LOW_DETAIL_HEADER]);
        self.header = header;
    }
}

impl TagSet {
    /// An empty set. Add groups, tags and strings, then build.
    pub fn new(cache_type: CacheType, name: &str, scenario_path: &str) -> Self {
        Self {
            cache_type,
            name: name.to_owned(),
            scenario_path: scenario_path.to_owned(),
            groups: Vec::new(),
            strings: Vec::new(),
            slots: Vec::new(),
            scenario: TagId::NULL,
            globals: TagId::NULL,
            index_word_14: 0,
            raw: Vec::new(),
            bsps: Vec::new(),
            salt_base: 0xe174,
            pinned: None,
            reuse_free_space: true,
            reverse_built: false,
            check_tag_memory: true,
        }
    }
    /// Declare a group and its ancestors (no-op if already declared).
    pub fn add_group(
        &mut self,
        class: FourCC,
        parent: Option<FourCC>,
        grandparent: Option<FourCC>,
    ) {
        if !self.groups.iter().any(|g| g.class == class) {
            self.groups.push(TagGroup {
                class,
                parent,
                grandparent,
            });
        }
    }
    /// Add a tag in the next slot and return its datum.
    pub fn add_tag(&mut self, group: FourCC, name: &str, payload: TagPayload) -> Result<TagId> {
        let slot = self.slots.len();
        if slot >= 0xffff {
            return Err(Error::Limit);
        }
        let salt = self.salt_base.wrapping_add(slot as u16);
        let id = TagId(u32::from(salt) << 16 | slot as u32);
        self.slots.push(Slot {
            name: Some(name.to_owned()),
            tag: Some(TagEntry { id, group, payload }),
        });
        Ok(id)
    }
    /// The live tag with this full datum.
    pub fn tag(&self, id: TagId) -> Option<&TagEntry> {
        self.slots
            .get(id.index())?
            .tag
            .as_ref()
            .filter(|t| t.id == id)
    }
    /// Mutable access to the live tag with this full datum.
    pub fn tag_mut(&mut self, id: TagId) -> Option<&mut TagEntry> {
        self.slots
            .get_mut(id.index())?
            .tag
            .as_mut()
            .filter(|t| t.id == id)
    }
    /// The string ID for `value`, adding it to the table if needed.
    pub fn intern(&mut self, value: &str) -> Result<u32> {
        let (index, _) = intern(&mut self.strings, &mut HashMap::new(), value)?;
        string_id(index, value)
    }
    /// Add a raw resource to be placed in this map.
    pub fn add_raw(&mut self, bytes: Vec<u8>, align: u32) -> RawId {
        self.raw.push(RawBlob { bytes, align });
        RawId(self.raw.len() - 1)
    }
    /// Add a built BSP region and return its section index (for
    /// [`FixupKind::Bsp`]).
    pub fn add_bsp(&mut self, bsp: TagId, lightmap: Option<TagId>) -> usize {
        self.bsps.push(BspSection { bsp, lightmap });
        self.bsps.len() - 1
    }

    /// Build the map in memory.
    pub fn build(&self) -> Result<(Vec<u8>, BuildLayout)> {
        Builder::new(self)?.run()
    }
    /// Build and write the map; returns its layout.
    pub fn write_to(&self, out: &mut impl Write) -> Result<BuildLayout> {
        let (bytes, layout) = self.build()?;
        out.write_all(&bytes)?;
        Ok(layout)
    }

    /// Decompose a retail-layout map into a set whose tags are all pinned
    /// at their original addresses. Building it reproduces the map: same
    /// header values, file prefix, tables, index and metadata.
    ///
    /// Requires the retail order: everything from the first string or name
    /// table to EOF is tables, then the index, then the metadata, and BSP
    /// regions lie before the tables.
    pub fn from_map<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        let h = map.header().clone();
        if map.index_space().virtual_base != INDEX_VIRTUAL_BASE {
            return Err(Error::Unsupported("index not at the retail base".into()));
        }
        let space = map
            .metadata_space()
            .ok_or(Error::Invalid("map has no metadata region"))?;
        let prefix_end = u64::from(h.index_offset);
        let tables = [
            (
                h.fixed_strings_offset,
                h.strings.count * FIXED_STRING_SLOT as u32,
            ),
            (h.strings.index_offset, h.strings.count * 4),
            (h.strings.data_offset, h.strings.data_size),
            (h.tag_names.data_offset, h.tag_names.data_size),
            (h.tag_names.index_offset, h.tag_names.count * 4),
        ];
        for (offset, len) in tables {
            if len != 0
                && (u64::from(offset) < HEADER as u64
                    || u64::from(offset) + u64::from(len) > prefix_end)
            {
                return Err(Error::Unsupported(
                    "string or name table outside the prefix".into(),
                ));
            }
        }
        let end = u64::from(h.index_offset) + u64::from(h.index_size) + u64::from(h.tag_data_size);
        if end != u64::from(h.file_size) || map.storage_length() != end {
            return Err(Error::Unsupported("metadata does not end the file".into()));
        }
        for t in map.tags() {
            if let Some(l) = t.location
                && l.space != space
                && l.space.file_offset + u64::from(l.space.size) > prefix_end
            {
                return Err(Error::Unsupported("BSP region after the index".into()));
            }
        }
        let header = map.read_file_range(0, HEADER as u64)?;
        let mut file = Vec::new();
        let mut at = HEADER as u64;
        while at < prefix_end {
            let n = (prefix_end - at).min(16 << 20);
            file.extend(map.read_file_range(at, n)?);
            at += n;
        }
        let index = map.read_file_range(h.index_offset.into(), h.index_size.into())?;
        let index_word = |at: usize| u32::from_le_bytes(index[at..at + 4].try_into().expect("4"));
        let tags_rel = index_word(8).wrapping_sub(INDEX_VIRTUAL_BASE);
        let index_entries_end = if tags_rel < h.index_size {
            tags_rel
                .checked_add(16 * map.index_slots())
                .ok_or(Error::Invalid("tag entry table"))?
        } else {
            // The entry table sits in the metadata (a grown index): the
            // index region holds only its header and groups.
            0x20 + 12 * map.groups().len() as u32
        };
        let metadata = {
            let mut m = Vec::new();
            let mut at = space.file_offset;
            while at < space.file_offset + u64::from(space.size) {
                let n = (space.file_offset + u64::from(space.size) - at).min(16 << 20);
                m.extend(map.read_file_range(at, n)?);
                at += n;
            }
            m
        };
        let names = read_names(map)?;
        let slot_count = map.index_slots() as usize;
        let mut slots: Vec<Slot> = (0..slot_count)
            .map(|i| Slot {
                name: names.get(i).cloned().flatten(),
                tag: None,
            })
            .collect();
        let mut owned = Vec::new();
        for t in map.tags() {
            let payload = match t.location {
                Some(l) if l.space == space => {
                    let start = (l.address - space.virtual_base) as usize;
                    owned.push((start, start + l.size as usize));
                    TagPayload::Pinned {
                        address: l.address,
                        bytes: metadata[start..start + l.size as usize].to_vec(),
                    }
                }
                _ => TagPayload::Empty,
            };
            slots[t.id.index()].tag = Some(TagEntry {
                id: t.id,
                group: t.group,
                payload,
            });
        }
        owned.sort_unstable();
        let mut unowned = Vec::new();
        let mut cursor = 0;
        for (lo, hi) in owned.into_iter().chain([(metadata.len(), metadata.len())]) {
            if lo > cursor {
                let gap = &metadata[cursor..lo];
                if gap.iter().any(|&b| b != 0) {
                    unowned.push((space.virtual_base + cursor as u32, gap.to_vec()));
                }
            }
            cursor = cursor.max(hi);
        }
        let groups = map.groups().to_vec();
        let scenario = map.scenario().map_or(TagId::NULL, |t| t.id);
        let globals = map.globals().map_or(TagId::NULL, |t| t.id);
        Ok(Self {
            cache_type: h.cache_type,
            name: h.name.clone(),
            scenario_path: h.scenario_path.clone(),
            groups,
            strings: map.strings().to_vec(),
            slots,
            scenario,
            globals,
            index_word_14: index_word(0x14),
            raw: Vec::new(),
            bsps: Vec::new(),
            salt_base: 0xe174,
            reuse_free_space: true,
            reverse_built: false,
            check_tag_memory: true,
            pinned: Some(PinnedImage {
                header,
                file,
                tables,
                index_entries_end,
                index,
                index_size: h.index_size,
                bsp_size: h
                    .meta_size
                    .checked_sub(h.index_size)
                    .and_then(|n| n.checked_sub(h.tag_data_size))
                    .ok_or(Error::Invalid("metadata memory size"))?,
                metadata_base: space.virtual_base,
                metadata_size: h.tag_data_size,
                unowned,
            }),
        })
    }
}

fn read_names<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<Option<String>>> {
    let t = map.header().tag_names;
    if t.count == 0 {
        return Ok(Vec::new());
    }
    let index = map.read_file_range(t.index_offset.into(), u64::from(t.count) * 4)?;
    let blob = map.read_file_range(t.data_offset.into(), t.data_size.into())?;
    index
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| {
            let at = u32::from_le_bytes(*w);
            if at == u32::MAX {
                return Ok(None);
            }
            let tail = blob
                .get(at as usize..)
                .ok_or(Error::Invalid("tag-name offset"))?;
            let n = tail
                .iter()
                .position(|&b| b == 0)
                .ok_or(Error::Invalid("tag-name terminator"))?;
            Ok(Some(
                std::str::from_utf8(&tail[..n])
                    .map_err(|_| Error::Encoding("tag-name table"))?
                    .to_owned(),
            ))
        })
        .collect()
}

/// Encode a string ID: index in the low 24 bits, byte length in the top 8.
pub fn string_id(index: usize, value: &str) -> Result<u32> {
    if index >= 1 << 24 {
        return Err(Error::Limit);
    }
    let len = u8::try_from(value.len()).map_err(|_| Error::Invalid("string ID over 255 bytes"))?;
    Ok(u32::from(len) << 24 | index as u32)
}
fn intern(
    strings: &mut Vec<Option<String>>,
    lookup: &mut HashMap<String, usize>,
    value: &str,
) -> Result<(usize, bool)> {
    if value.as_bytes().contains(&0) {
        return Err(Error::Invalid("NUL in string ID"));
    }
    if value.len() >= FIXED_STRING_SLOT {
        return Err(Error::Invalid("string ID too long for its fixed slot"));
    }
    if lookup.is_empty() {
        for (i, s) in strings.iter().enumerate() {
            if let Some(s) = s {
                lookup.entry(s.clone()).or_insert(i);
            }
        }
    }
    if let Some(&i) = lookup.get(value) {
        return Ok((i, false));
    }
    strings.push(Some(value.to_owned()));
    lookup.insert(value.to_owned(), strings.len() - 1);
    Ok((strings.len() - 1, true))
}
fn round_up(n: u64, align: u64) -> u64 {
    n.div_ceil(align) * align
}
fn check_align(align: u32) -> Result<()> {
    if !align.is_power_of_two() || align > PAGE {
        return Err(Error::Invalid(
            "alignment must be a power of two up to 0x1000",
        ));
    }
    Ok(())
}
fn put(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_str(b: &mut [u8], at: usize, width: usize, s: &str, what: &'static str) -> Result<()> {
    if s.len() >= width || s.as_bytes().contains(&0) {
        return Err(Error::Invalid(what));
    }
    b[at..at + width].fill(0);
    b[at..at + s.len()].copy_from_slice(s.as_bytes());
    Ok(())
}

/// Values fixups resolve against, fixed before any block is emitted.
struct Resolve<'a> {
    groups: HashMap<TagId, FourCC>,
    strings: HashMap<String, usize>,
    raw: &'a [u32],
    bsps: Vec<(u32, u32, u32)>,
}

/// A growing virtual region (BSP or ordinary metadata).
struct Region {
    base: u32,
    bytes: Vec<u8>,
    /// Where the next allocation starts, relative to `base`.
    cursor: usize,
    /// Addresses of shared blocks placed so far, by key.
    shared: HashMap<u64, u32>,
}
impl Region {
    fn new(base: u32, bytes: Vec<u8>) -> Self {
        let cursor = bytes.len();
        Self {
            base,
            bytes,
            cursor,
            shared: HashMap::new(),
        }
    }
    fn alloc(&mut self, len: usize, align: u32) -> Result<u32> {
        check_align(align)?;
        let at = u64::from(self.base) + self.cursor as u64;
        let start = round_up(at, align.into());
        let end = start + len as u64;
        if end > 1 << 32 {
            return Err(Error::Limit);
        }
        self.cursor = (end - u64::from(self.base)) as usize;
        if self.bytes.len() < self.cursor {
            self.bytes.resize(self.cursor, 0);
        }
        Ok(start as u32)
    }
    fn place(&mut self, at: u32, bytes: &[u8]) -> Result<()> {
        let start = at
            .checked_sub(self.base)
            .ok_or(Error::Invalid("pinned bytes below the metadata base"))?
            as usize;
        let end = start + bytes.len();
        if self.bytes.len() < end {
            self.bytes.resize(end, 0);
        }
        self.bytes[start..end].copy_from_slice(bytes);
        Ok(())
    }
    fn emit(&mut self, block: &Block, r: &Resolve) -> Result<u32> {
        self.emit_in(block, r, None)
    }
    /// Emit `block`; a shared block already placed here or in `outer` (a
    /// dry run's view of the real region) is not placed again.
    fn emit_in(
        &mut self,
        block: &Block,
        r: &Resolve,
        outer: Option<&HashMap<u64, u32>>,
    ) -> Result<u32> {
        if let Some(k) = block.share
            && let Some(&a) = self
                .shared
                .get(&k)
                .or_else(|| outer.and_then(|o| o.get(&k)))
        {
            return Ok(a);
        }
        let addr = self.alloc(block.bytes.len(), block.align)?;
        if let Some(k) = block.share {
            self.shared.insert(k, addr);
        }
        let start = (addr - self.base) as usize;
        self.bytes[start..start + block.bytes.len()].copy_from_slice(&block.bytes);
        let mut spans: Vec<_> = block
            .fixups
            .iter()
            .map(|f| (f.offset, f.offset.saturating_add(f.kind.width())))
            .collect();
        spans.sort_unstable();
        if spans.windows(2).any(|w| w[1].0 < w[0].1) {
            return Err(Error::Invalid("overlapping fixups"));
        }
        for f in &block.fixups {
            let at = f.offset as usize;
            if at + f.kind.width() as usize > block.bytes.len() {
                return Err(Error::Invalid("fixup outside its block"));
            }
            let mut words = [0u32; 2];
            match &f.kind {
                FixupKind::Block { count, child } => {
                    if (*count == 0) != child.bytes.is_empty()
                        || (*count != 0 && child.bytes.len() % *count as usize != 0)
                    {
                        return Err(Error::Invalid("block count does not match its bytes"));
                    }
                    words[0] = *count;
                    words[1] = if *count == 0 {
                        0
                    } else {
                        self.emit_in(child, r, outer)?
                    };
                }
                FixupKind::Data(child) => {
                    words[0] = u32::try_from(child.bytes.len()).map_err(|_| Error::Limit)?;
                    words[1] = if child.bytes.is_empty() {
                        0
                    } else {
                        self.emit_in(child, r, outer)?
                    };
                }
                FixupKind::Pointer(child) => words[0] = self.emit_in(child, r, outer)?,
                FixupKind::TagRef(id) | FixupKind::TagId(id) => {
                    let group = if *id == TagId::NULL {
                        u32::MAX
                    } else {
                        r.groups
                            .get(id)
                            .ok_or(Error::Invalid("reference to a tag not in the set"))?
                            .as_u32()
                    };
                    words = if matches!(f.kind, FixupKind::TagRef(_)) {
                        [group, id.0]
                    } else {
                        [id.0, 0]
                    };
                }
                FixupKind::StringId(s) => {
                    words[0] = string_id(r.strings[s.as_str()], s)?;
                }
                FixupKind::Raw(raw) => {
                    words[0] = match *raw {
                        RawRef::Null => u32::MAX,
                        RawRef::Local(RawId(i)) => {
                            *r.raw.get(i).ok_or(Error::Invalid("unknown raw resource"))?
                        }
                        RawRef::External { file, offset } => {
                            if offset >= 1 << 30 {
                                return Err(Error::Invalid("raw offset needs more than 30 bits"));
                            }
                            let selector = match file {
                                ResourceFile::Local => 0,
                                ResourceFile::MainMenu => 1,
                                ResourceFile::Shared => 2,
                                ResourceFile::SinglePlayerShared => 3,
                            };
                            selector << 30 | offset
                        }
                    };
                }
                FixupKind::Bsp { section, field } => {
                    let (offset, size, address) = *r
                        .bsps
                        .get(*section)
                        .ok_or(Error::Invalid("unknown BSP section"))?;
                    words[0] = match field {
                        BspField::FileOffset => offset,
                        BspField::Size => size,
                        BspField::Address => address,
                    };
                }
            }
            let start = start + at;
            put(&mut self.bytes, start, words[0]);
            if f.kind.width() == 8 {
                put(&mut self.bytes, start + 4, words[1]);
            }
        }
        Ok(addr)
    }
}

struct Builder<'a> {
    set: &'a TagSet,
    strings: Vec<Option<String>>,
    lookup: HashMap<String, usize>,
}

impl<'a> Builder<'a> {
    fn new(set: &'a TagSet) -> Result<Self> {
        let mut b = Self {
            set,
            strings: set.strings.clone(),
            lookup: HashMap::new(),
        };
        for s in set.strings.iter().flatten() {
            if s.len() > 255 || s.as_bytes().contains(&0) {
                return Err(Error::Invalid("string-ID table entry"));
            }
        }
        b.validate()?;
        // Intern in a fixed order (slot order, depth first) so output is
        // deterministic.
        let mut wanted = Vec::new();
        for slot in &set.slots {
            if let Some(TagEntry {
                payload: TagPayload::Built(root),
                ..
            }) = &slot.tag
            {
                root.visit(&mut |k| {
                    if let FixupKind::StringId(s) = k {
                        wanted.push(s);
                    }
                });
            }
        }
        for s in wanted {
            intern(&mut b.strings, &mut b.lookup, s)?;
        }
        if b.lookup.is_empty() {
            for (i, s) in b.strings.iter().enumerate() {
                if let Some(s) = s {
                    b.lookup.entry(s.clone()).or_insert(i);
                }
            }
        }
        Ok(b)
    }

    fn validate(&self) -> Result<()> {
        let set = self.set;
        if set.slots.len() > 0xffff {
            return Err(Error::Limit);
        }
        for (i, slot) in set.slots.iter().enumerate() {
            let Some(t) = &slot.tag else { continue };
            if t.id == TagId::NULL || t.id.index() != i {
                return Err(Error::Invalid("tag datum does not match its slot"));
            }
            if !set.groups.iter().any(|g| g.class == t.group) {
                return Err(Error::Invalid("tag uses undeclared class"));
            }
            if let Some(name) = &slot.name
                && name.as_bytes().contains(&0)
            {
                return Err(Error::Invalid("NUL in tag name"));
            }
        }
        for (root, class) in [(set.scenario, *b"scnr"), (set.globals, *b"matg")] {
            if root != TagId::NULL && set.tag(root).is_none_or(|t| t.group.0 != class) {
                return Err(Error::Invalid("root tag missing or of the wrong class"));
            }
        }
        for s in &set.bsps {
            for (id, class) in [(Some(s.bsp), *b"sbsp"), (s.lightmap, *b"ltmp")] {
                let Some(id) = id else { continue };
                match set.tag(id) {
                    Some(TagEntry {
                        group,
                        payload: TagPayload::Built(_),
                        ..
                    }) if group.0 == class => {}
                    _ => return Err(Error::Invalid("BSP section tag must be a built sbsp/ltmp")),
                }
            }
        }
        for r in &set.raw {
            check_align(r.align.max(4))?;
        }
        Ok(())
    }

    fn run(self) -> Result<(Vec<u8>, BuildLayout)> {
        let set = self.set;
        let pin = set.pinned.as_ref();
        let in_bsp: std::collections::HashSet<TagId> = set
            .bsps
            .iter()
            .flat_map(|s| [Some(s.bsp), s.lightmap])
            .flatten()
            .collect();

        // Index: header, groups, tag entries.
        let groups_end = 0x20 + 12 * set.groups.len() as u64;
        let entries_bytes = 16 * set.slots.len() as u64;
        let mut index_bytes = groups_end + entries_bytes;
        // A pinned map's index region has uninterpreted bytes after its
        // entries (the "tail"). Entries that outgrow the old ones go into
        // the tail where it is zero (e.g. after `clear_index_tail`);
        // otherwise the entry table moves to the end of the ordinary
        // metadata (the index header points at it), and the tail stays.
        let entries_in_meta = pin.is_some_and(|p| {
            let old = u64::from(p.index_entries_end);
            index_bytes > old
                && (index_bytes > p.index.len() as u64
                    || p.index[old as usize..index_bytes as usize]
                        .iter()
                        .any(|b| *b != 0))
        });
        if entries_in_meta {
            if groups_end > pin.map_or(0, |p| u64::from(p.index_entries_end)) {
                return Err(Error::Invalid(
                    "a pinned map's group table cannot grow over its index tail",
                ));
            }
            index_bytes = groups_end;
        }
        let mut index_size = round_up(index_bytes, PAGE.into());
        if let Some(p) = pin {
            if index_size > u64::from(p.index_size) {
                return Err(Error::Invalid("index outgrew the pinned BSP base"));
            }
            index_size = p.index_size.into();
        }
        let index_size = u32::try_from(index_size).map_err(|_| Error::Limit)?;
        let bsp_base = INDEX_VIRTUAL_BASE
            .checked_add(index_size)
            .ok_or(Error::Limit)?;

        // File prefix: header, pinned bytes, raw resources, BSP regions.
        let mut file = vec![0u8; HEADER];
        if let Some(p) = pin {
            file.extend_from_slice(&p.file);
        }
        let mut raw_offsets = Vec::new();
        for r in &set.raw {
            let at = round_up(file.len() as u64, u64::from(r.align.max(4)).max(SECTOR));
            if at + r.bytes.len() as u64 >= 1 << 30 {
                return Err(Error::Invalid("raw resource beyond a 30-bit offset"));
            }
            file.resize(at as usize, 0);
            file.extend_from_slice(&r.bytes);
            raw_offsets.push(at as u32);
        }
        let resolve_pre = Resolve {
            groups: self.groups(),
            strings: self.lookup.clone(),
            raw: &raw_offsets,
            bsps: Vec::new(),
        };
        let mut bsp_layout = Vec::new();
        let mut bsp_tags: HashMap<TagId, (u32, u32)> = HashMap::new();
        let mut max_bsp = pin.map_or(0, |p| p.bsp_size);
        for s in &set.bsps {
            let mut region = Region::new(bsp_base, vec![0; 16]);
            let built = |id: TagId| match &set.tag(id).expect("validated").payload {
                TagPayload::Built(b) => b,
                _ => unreachable!("validated"),
            };
            let bsp_addr = region.emit(built(s.bsp), &resolve_pre)?;
            if bsp_addr != bsp_base + 16 {
                return Err(Error::Invalid("BSP root must follow its region header"));
            }
            let bsp_end = bsp_base + region.bytes.len() as u32;
            bsp_tags.insert(s.bsp, (bsp_addr, bsp_end - bsp_addr));
            let light_addr = match s.lightmap {
                Some(l) => {
                    let a = region.emit(built(l), &resolve_pre)?;
                    let end = bsp_base + region.bytes.len() as u32;
                    // The reader sizes the BSP as lightmap address minus BSP
                    // address, so no padding may sit between them.
                    if a != bsp_end {
                        return Err(Error::Invalid("lightmap must follow its BSP unpadded"));
                    }
                    bsp_tags.insert(l, (a, end - a));
                    a
                }
                None => 0,
            };
            let used = region.bytes.len() as u32;
            put(&mut region.bytes, 0, used);
            put(&mut region.bytes, 4, bsp_addr);
            put(&mut region.bytes, 8, light_addr);
            region.bytes[12..16].copy_from_slice(b"psbs");
            let size = round_up(used.into(), PAGE.into()) as u32;
            region.bytes.resize(size as usize, 0);
            let at = round_up(file.len() as u64, PAGE.into()) as usize;
            file.resize(at, 0);
            file.extend_from_slice(&region.bytes);
            bsp_layout.push((u32::try_from(at).map_err(|_| Error::Limit)?, size, bsp_base));
            max_bsp = max_bsp.max(size);
        }

        // Ordinary metadata.
        let computed_base = u64::from(bsp_base) + u64::from(max_bsp);
        let metadata_base = match pin {
            Some(p) => {
                if computed_base > u64::from(p.metadata_base) {
                    return Err(Error::Invalid("BSP space outgrew the pinned metadata base"));
                }
                p.metadata_base
            }
            None => u32::try_from(computed_base).map_err(|_| Error::Limit)?,
        };
        let resolve = Resolve {
            bsps: bsp_layout.clone(),
            ..resolve_pre
        };
        let mut meta = Region::new(metadata_base, Vec::new());
        let mut tag_layout = vec![(0u32, 0u32); set.slots.len()];
        if let Some(p) = pin {
            for (at, bytes) in &p.unowned {
                meta.place(*at, bytes)?;
            }
        }
        for (i, slot) in set.slots.iter().enumerate() {
            if let Some(TagEntry {
                payload: TagPayload::Pinned { address, bytes },
                ..
            }) = &slot.tag
            {
                meta.place(*address, bytes)?;
                tag_layout[i] = (*address, bytes.len() as u32);
            }
        }
        // Built tags go first-fit into space no pinned bytes use (ranges of
        // tags that became built, padding, the region's slack), else after
        // everything. A dry run at an aligned base gives each tree's size.
        let mut holes = Vec::new();
        if let Some(p) = pin.filter(|_| set.reuse_free_space) {
            let mut used: Vec<(usize, usize)> = p
                .unowned
                .iter()
                .map(|(a, b)| (a, b.len()))
                .chain(set.slots.iter().filter_map(|s| match &s.tag {
                    Some(TagEntry {
                        payload: TagPayload::Pinned { address, bytes },
                        ..
                    }) => Some((address, bytes.len())),
                    _ => None,
                }))
                .map(|(a, n)| {
                    let lo = (a - metadata_base) as usize;
                    (lo, lo + n)
                })
                .collect();
            used.sort_unstable();
            let mut cursor = 0;
            for (lo, hi) in used.into_iter().chain([(p.metadata_size as usize, 0)]) {
                if lo > cursor {
                    holes.push((cursor, lo));
                }
                cursor = cursor.max(hi);
            }
        }
        // Appends start after the pinned region's original length, whose
        // slack is handed out through `holes` only.
        let mut append = meta
            .bytes
            .len()
            .max(pin.map_or(0, |p| p.metadata_size as usize));
        let order: Vec<usize> = if set.reverse_built {
            (0..set.slots.len()).rev().collect()
        } else {
            (0..set.slots.len()).collect()
        };
        for i in order {
            let Some(t) = &set.slots[i].tag else { continue };
            if let TagPayload::Built(root) = &t.payload {
                if in_bsp.contains(&t.id) {
                    continue;
                }
                let align = root.max_align();
                let size = {
                    let mut dry = Region::new(0, Vec::new());
                    dry.emit_in(root, &resolve, Some(&meta.shared))?;
                    dry.bytes.len()
                };
                let fit = holes.iter().enumerate().find_map(|(h, &(lo, hi))| {
                    let start = round_up((metadata_base as usize + lo) as u64, align.into())
                        as usize
                        - metadata_base as usize;
                    (start + size <= hi).then_some((h, start))
                });
                let end = append;
                meta.cursor = match fit {
                    Some((h, start)) => {
                        let (lo, hi) = holes[h];
                        holes.splice(h..=h, [(lo, start), (start + size, hi)]);
                        start
                    }
                    // Align the root to the tree's largest alignment so the
                    // layout matches the dry run.
                    None => {
                        round_up((metadata_base as usize + end) as u64, align.into()) as usize
                            - metadata_base as usize
                    }
                };
                let addr = meta.emit(root, &resolve)?;
                if meta.cursor - (addr - metadata_base) as usize != size {
                    return Err(Error::Invalid("tag layout differs from its dry run"));
                }
                if fit.is_none() {
                    append = meta.cursor;
                }
                tag_layout[i] = (addr, size as u32);
            }
        }
        // The reader anchors the metadata region on slot 0's address.
        match tag_layout.first() {
            Some(&(a, _)) if a == metadata_base => {}
            Some(_) => {
                return Err(Error::Invalid(
                    "slot 0 must hold the first ordinary-metadata tag",
                ));
            }
            None => {}
        }
        let groups_addr = INDEX_VIRTUAL_BASE + 0x20;
        let mut tags_addr = groups_addr + 12 * set.groups.len() as u32;
        let entry = |i: usize, slot: &Slot| -> [u32; 4] {
            let (class, id) = slot
                .tag
                .as_ref()
                .map_or((u32::MAX, u32::MAX), |t| (t.group.as_u32(), t.id.0));
            [class, id, tag_layout[i].0, tag_layout[i].1]
        };
        if entries_in_meta {
            // After every tag (and the old region's length), 16-aligned.
            let end = meta
                .bytes
                .len()
                .max(pin.map_or(0, |p| p.metadata_size as usize));
            let at = round_up(end as u64, 16) as usize;
            meta.bytes.resize(at + entries_bytes as usize, 0);
            tags_addr = metadata_base + u32::try_from(at).map_err(|_| Error::Limit)?;
            for (i, slot) in set.slots.iter().enumerate() {
                for (k, v) in entry(i, slot).into_iter().enumerate() {
                    put(&mut meta.bytes, at + i * 16 + k * 4, v);
                }
            }
        }
        let mut metadata_size = round_up(meta.bytes.len() as u64, PAGE.into());
        if let Some(p) = pin {
            metadata_size = metadata_size.max(p.metadata_size.into());
        }
        meta.bytes.resize(metadata_size as usize, 0);
        let metadata_size = u32::try_from(metadata_size).map_err(|_| Error::Limit)?;
        let memory_size = (u64::from(metadata_base) - u64::from(INDEX_VIRTUAL_BASE))
            .checked_add(metadata_size.into())
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(Error::Limit)?;
        if set.check_tag_memory && memory_size > max_metadata_size(set.cache_type)? {
            return Err(Error::Invalid(
                "metadata exceeds the engine's tag memory for this cache type",
            ));
        }

        // String-ID tables: fixed 128-byte slots, offsets, blob.
        let mut header = match pin {
            Some(p) => p.header.clone(),
            None => vec![0; HEADER],
        };
        let mut string_blob = Vec::new();
        let mut string_index = Vec::new();
        let mut fixed = vec![0u8; self.strings.len() * FIXED_STRING_SLOT];
        for (i, s) in self.strings.iter().enumerate() {
            match s {
                Some(s) => {
                    if s.len() >= FIXED_STRING_SLOT {
                        return Err(Error::Invalid("string ID too long for its fixed slot"));
                    }
                    fixed[i * FIXED_STRING_SLOT..][..s.len()].copy_from_slice(s.as_bytes());
                    string_index.extend((string_blob.len() as u32).to_le_bytes());
                    string_blob.extend_from_slice(s.as_bytes());
                    string_blob.push(0);
                }
                None => string_index.extend(u32::MAX.to_le_bytes()),
            }
        }
        let mut name_blob = Vec::new();
        let mut name_index = Vec::new();
        for slot in &set.slots {
            match &slot.name {
                Some(n) => {
                    name_index.extend((name_blob.len() as u32).to_le_bytes());
                    name_blob.extend_from_slice(n.as_bytes());
                    name_blob.push(0);
                }
                None => name_index.extend(u32::MAX.to_le_bytes()),
            }
        }
        // A table that fits goes back where the pinned map had it; anything
        // else follows the prefix, sector aligned.
        let place = |file: &mut Vec<u8>, slot: usize, bytes: &[u8]| -> Result<u32> {
            if let Some((at, len)) = pin.map(|p| p.tables[slot])
                && bytes.len() <= len as usize
            {
                let at = at as usize;
                file[at..at + len as usize].fill(0);
                file[at..at + bytes.len()].copy_from_slice(bytes);
                return Ok(at as u32);
            }
            let at = round_up(file.len() as u64, SECTOR) as usize;
            file.resize(at, 0);
            file.extend_from_slice(bytes);
            u32::try_from(at).map_err(|_| Error::Limit)
        };
        let fixed_at = place(&mut file, 0, &fixed)?;
        let string_index_at = place(&mut file, 1, &string_index)?;
        let string_blob_at = place(&mut file, 2, &string_blob)?;
        let name_blob_at = place(&mut file, 3, &name_blob)?;
        let name_index_at = place(&mut file, 4, &name_index)?;

        // Tag index.
        let index_at = round_up(file.len() as u64, SECTOR) as usize;
        file.resize(index_at, 0);
        let mut index = match pin {
            Some(p) => {
                // Clear the old header, groups and entries; nonzero tail
                // bytes past them are never overwritten (checked above).
                let mut index = p.index.clone();
                let clear = index_bytes.max(p.index_entries_end.into()) as usize;
                let clear = clear.min(index.len());
                index[..clear].fill(0);
                index
            }
            None => vec![0u8; index_size as usize],
        };
        put(&mut index, 0, groups_addr);
        put(&mut index, 4, set.groups.len() as u32);
        put(&mut index, 8, tags_addr);
        put(&mut index, 0xc, set.scenario.0);
        put(&mut index, 0x10, set.globals.0);
        put(&mut index, 0x14, set.index_word_14);
        put(&mut index, 0x18, set.slots.len() as u32);
        index[0x1c..0x20].copy_from_slice(b"sgat");
        for (i, g) in set.groups.iter().enumerate() {
            let at = 0x20 + i * 12;
            put(&mut index, at, g.class.as_u32());
            put(
                &mut index,
                at + 4,
                g.parent.map_or(u32::MAX, FourCC::as_u32),
            );
            put(
                &mut index,
                at + 8,
                g.grandparent.map_or(u32::MAX, FourCC::as_u32),
            );
        }
        if !entries_in_meta {
            for (i, slot) in set.slots.iter().enumerate() {
                let at = (tags_addr - INDEX_VIRTUAL_BASE) as usize + i * 16;
                for (k, v) in entry(i, slot).into_iter().enumerate() {
                    put(&mut index, at + k * 4, v);
                }
            }
        }
        file.extend_from_slice(&index);
        let metadata_at = file.len();
        file.extend_from_slice(&meta.bytes);
        let file_size = u32::try_from(file.len()).map_err(|_| Error::Limit)?;
        if u64::from(file_size) > cache_size_limit(set.cache_type)? {
            return Err(Error::Invalid("map exceeds the engine's cache size limit"));
        }
        debug_assert_eq!(metadata_at, index_at + index_size as usize);

        // Header.
        header[..4].copy_from_slice(b"daeh");
        header[0x7fc..].copy_from_slice(b"toof");
        let cache_type = match set.cache_type {
            CacheType::Campaign => 0,
            CacheType::Multiplayer => 1,
            CacheType::MainMenu => 2,
            CacheType::Shared => 3,
            CacheType::SinglePlayerShared => 4,
            CacheType::Unknown(_) => return Err(Error::Invalid("unknown cache type")),
        };
        for (at, value) in [
            (4, 8),
            (8, file_size),
            (0x10, index_at as u32),
            (0x14, index_size),
            (0x18, metadata_size),
            (0x1c, memory_size),
            (0x140, cache_type),
            (0x160, fixed_at),
            (0x164, self.strings.len() as u32),
            (0x168, string_blob.len() as u32),
            (0x16c, string_index_at),
            (0x170, string_blob_at),
            (0x2c0, set.slots.len() as u32),
            (0x2c4, name_blob_at),
            (0x2c8, name_blob.len() as u32),
            (0x2cc, name_index_at),
        ] {
            put(&mut header, at, value);
        }
        put_str(&mut header, 0x120, 32, BUILD_STRING, "build string")?;
        put_str(
            &mut header,
            0x198,
            32,
            &set.name,
            "map name must be under 32 bytes",
        )?;
        put_str(
            &mut header,
            0x1bc,
            256,
            &set.scenario_path,
            "scenario path must be under 256 bytes",
        )?;
        let mut checksum = XorChecksum::default();
        checksum.update(&file[HEADER..]);
        let checksum = checksum.finish();
        put(&mut header, 0x2d0, checksum);
        file[..HEADER].copy_from_slice(&header);

        let tags = set
            .slots
            .iter()
            .enumerate()
            .map(|(i, s)| match &s.tag {
                Some(t) if bsp_tags.contains_key(&t.id) => bsp_tags[&t.id],
                _ => tag_layout[i],
            })
            .collect();
        let layout = BuildLayout {
            file_size,
            index_offset: index_at as u32,
            index_size,
            metadata_size,
            memory_size,
            metadata_base,
            bsp_base,
            tags,
            raw: raw_offsets,
            bsps: bsp_layout.iter().map(|&(o, s, _)| (o, s)).collect(),
            string_count: self.strings.len() as u32,
            checksum,
        };
        Ok((file, layout))
    }

    fn groups(&self) -> HashMap<TagId, FourCC> {
        self.set
            .slots
            .iter()
            .filter_map(|s| s.tag.as_ref().map(|t| (t.id, t.group)))
            .collect()
    }
}
