//! The one **geometry section** decoder, shared by render models (`mode`),
//! structure BSPs (`sbsp`) and lightmaps (`ltmp`).
//!
//! It covers the 0x24-byte geometry block info, the raw block it points to,
//! the resource fixups, and the parts, subparts, indices and vertex streams
//! inside a loaded section. The engine resolves every owner's sections with
//! one fixup routine (`0x00223c70`). Each owner describes its section struct
//! with a [`SectionLayout`]: field offsets, element strides and the struct
//! size. This module knows nothing about BSPs or models.
//!
//! The reader is hand-written. Per-field provenance, as engine addresses (E)
//! and disc checks (V/D), is in `docs/structure-bsp.md` and
//! `docs/map-format.md` ("Render-model geometry").
//!
//! Hostile-input rules, all enforced here so that every layout gets them:
//! - Tag-block reads are charged to a per-decode byte budget, so aliased
//!   blocks cannot amplify allocations.
//! - A raw block is at most [`MAX_BLOCK_SIZE`] bytes. Every present resource
//!   is marked, inside the resource area, and disjoint from every other.
//!   Resource keys are unique.
//! - Decoded tables borrow ranges of the one loaded block; nothing is copied
//!   per descriptor.
//! - Parts tile the index block in order. Subparts tile the subpart block and
//!   name their part. Indices stay below the vertex count, and every vertex
//!   stream agrees on that count.

use crate::{Error, Result};
use map::{FourCC, RawPointer, TagId};
use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;

pub(crate) fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(crate) fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

/// A tag payload and its virtual address, for following tag-block pointers.
///
/// Every [`TagView::block`] call is charged against a byte budget equal to
/// the payload length. Distinct blocks cannot exceed the payload, but aliased
/// pointers (many records naming one table) could otherwise multiply decoded
/// output without bound. This is the rule map-format.md applies to sprites.
pub struct TagView<'a> {
    bytes: &'a [u8],
    address: u32,
    budget: Cell<u64>,
}
impl<'a> TagView<'a> {
    /// View `bytes`, whose first byte is at virtual `address`.
    pub fn new(bytes: &'a [u8], address: u32) -> Self {
        Self {
            bytes,
            address,
            budget: Cell::new(bytes.len() as u64),
        }
    }
    /// Payload bytes.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
    /// Virtual address of `bytes()[0]`.
    pub fn address(&self) -> u32 {
        self.address
    }
    /// Fixed-size slice at a payload offset.
    pub fn at(&self, offset: usize, len: usize) -> Result<&'a [u8]> {
        offset
            .checked_add(len)
            .and_then(|end| self.bytes.get(offset..end))
            .ok_or(Error::Bounds {
                address: self.address.wrapping_add(offset as u32),
                size: len as u64,
            })
    }
    /// Translate a virtual range inside this payload.
    pub fn range(&self, address: u32, len: u64) -> Result<&'a [u8]> {
        if len == 0 {
            return Ok(&[]);
        }
        address
            .checked_sub(self.address)
            .map(u64::from)
            .and_then(|start| Some((start, start.checked_add(len)?)))
            .filter(|&(_, end)| end <= self.bytes.len() as u64)
            .map(|(start, end)| &self.bytes[start as usize..end as usize])
            .ok_or(Error::Bounds { address, size: len })
    }
    /// Elements of the tag block whose (count, pointer) pair is at `field`
    /// inside `record`, one slice per element. Charged to the budget.
    pub fn block(&self, record: &[u8], field: usize, stride: usize) -> Result<Vec<&'a [u8]>> {
        let pair = field
            .checked_add(8)
            .and_then(|end| record.get(field..end))
            .ok_or(Error::Invalid("tag block field outside its record"))?;
        self.elements(u32_at(pair, 0), u32_at(pair, 4), stride)
    }
    /// Elements of a tag block given its count and pointer. Charged to the
    /// budget. A zero `stride` is a caller error and is rejected.
    pub fn elements(&self, count: u32, address: u32, stride: usize) -> Result<Vec<&'a [u8]>> {
        if stride == 0 {
            return Err(Error::Invalid("tag block stride is zero"));
        }
        if count != 0 && !address.is_multiple_of(4) {
            return Err(Error::Invalid("unaligned geometry block pointer"));
        }
        let len = u64::from(count)
            .checked_mul(stride as u64)
            .ok_or(Error::Invalid("tag block size overflow"))?;
        if len > self.budget.get() {
            return Err(Error::Invalid("tag blocks exceed the payload budget"));
        }
        let bytes = self.range(address, len)?;
        self.charge(len)?;
        Ok(bytes.chunks_exact(stride).collect())
    }
    /// Charge `len` bytes of decoded output that did not come from
    /// [`Self::block`] (for example an alias read from another payload) to
    /// the same budget.
    pub fn charge(&self, len: u64) -> Result<()> {
        let left = self
            .budget
            .get()
            .checked_sub(len)
            .ok_or(Error::Invalid("tag blocks exceed the payload budget"))?;
        self.budget.set(left);
        Ok(())
    }
}

/// What a block resource patches inside the loaded section struct.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ResourceKind {
    /// 0: a tag block. Count is `size / element size`.
    Block,
    /// 1: a data reference (size, pointer).
    Data,
    /// 2: vertex data for one vertex-buffer descriptor.
    VertexBuffer,
}

/// One 16-byte block resource entry (E `0x00223c70`, see structure-bsp.md).
#[derive(Clone, Debug)]
pub struct BlockResource {
    /// Byte 0 (E `0x00223cc4`..`0x00223ccd`: only 0, 1 and 2 are handled).
    pub kind: ResourceKind,
    /// Byte 1; not read by the fixup routine (V: 2 for blocks and vertex
    /// data, 4 for data references).
    pub unknown_1: u8,
    /// u16 at +2; not read by the fixup. V: equals `field_offset` in every
    /// raw block (checked when one is loaded); in-tag sections differ.
    pub unknown_2: u16,
    /// u16 at +4: offset of the patched field inside the section struct.
    pub field_offset: u16,
    /// u16 at +6: element size (blocks), descriptor index (vertex data), or
    /// `0xffff` (data).
    pub secondary: u16,
    /// u32 at +8: byte length.
    pub size: u32,
    /// u32 at +0xc: offset in the resource area; `None` = skipped entry.
    pub offset: Option<u32>,
}
impl BlockResource {
    /// Fixed record size.
    pub const SIZE: usize = 0x10;
    fn decode(b: &[u8]) -> Result<Self> {
        let kind = match b[0] {
            0 => ResourceKind::Block,
            1 => ResourceKind::Data,
            2 => ResourceKind::VertexBuffer,
            _ => return Err(Error::Invalid("unknown block resource kind")),
        };
        let offset = u32_at(b, 0xc);
        Ok(Self {
            kind,
            unknown_1: b[1],
            unknown_2: u16_at(b, 2),
            field_offset: u16_at(b, 4),
            secondary: u16_at(b, 6),
            size: u32_at(b, 8),
            offset: (offset != u32::MAX).then_some(offset),
        })
    }
    /// Lookup key: blocks and data by field, vertex data by (field, index).
    fn key(&self) -> (ResourceKind, u16, u16) {
        let index = match self.kind {
            ResourceKind::VertexBuffer => self.secondary,
            _ => 0,
        };
        (self.kind, self.field_offset, index)
    }
}

/// The 0x24-byte geometry block info: where a section's raw block lives and
/// how to patch it after loading.
#[derive(Clone, Debug)]
pub struct GeometryBlockInfo {
    /// Raw pointer (two-bit file selector, 30-bit offset); `None` = no block.
    pub block: Option<RawPointer>,
    /// Whole block length including the `blkh` header and `blkf` footer.
    pub block_size: u32,
    /// Section struct length following the 8-byte `blkh` header.
    pub section_size: u32,
    /// Length of the resource area following the section struct.
    pub resource_size: u32,
    /// Fixups applied to the section struct.
    pub resources: Vec<BlockResource>,
    /// Owning tag.
    pub owner: TagId,
    /// i16 at +0x1c: offset from this record to the runtime section block
    /// the engine fills after loading (E `0x00223c73`).
    pub runtime_block_offset: i16,
    /// Byte at +0x1e: the runtime "loaded" flag (E `0x0012e31d`).
    pub runtime_loaded: u8,
    /// Byte at +0x1f (V: 1 when a block exists, 0 otherwise; meaning unknown).
    pub unknown_1f: u8,
    /// u32 at +0x20: the runtime cache handle (E `0x0012e380`).
    pub runtime_cache_handle: u32,
}
impl GeometryBlockInfo {
    /// Fixed record size.
    pub const SIZE: usize = 0x24;
    /// Decode the record at `offset` of `record`, following its resources.
    ///
    /// A present pointer must have a nonzero size, and an absent one size 0.
    /// Runtime fields are checked when a raw block is loaded (in-tag
    /// lightmap sections carry runtime state on disc).
    pub fn decode(view: &TagView<'_>, record: &[u8], offset: usize) -> Result<Self> {
        let b = offset
            .checked_add(Self::SIZE)
            .and_then(|end| record.get(offset..end))
            .ok_or(Error::Invalid("geometry block info truncated"))?;
        let resources = view
            .block(b, 0x10, BlockResource::SIZE)?
            .into_iter()
            .map(BlockResource::decode)
            .collect::<Result<Vec<_>>>()?;
        let raw = u32_at(b, 0);
        let block = if raw == u32::MAX {
            None
        } else {
            Some(RawPointer::decode(raw).ok_or(Error::Invalid("geometry raw pointer"))?)
        };
        let block_size = u32_at(b, 4);
        if block.is_some() != (block_size != 0) {
            return Err(Error::Invalid("geometry block pointer and size disagree"));
        }
        Ok(Self {
            block,
            block_size,
            section_size: u32_at(b, 8),
            resource_size: u32_at(b, 0xc),
            resources,
            owner: TagId(u32_at(b, 0x18)),
            runtime_block_offset: u16_at(b, 0x1c) as i16,
            runtime_loaded: b[0x1e],
            unknown_1f: b[0x1f],
            runtime_cache_handle: u32_at(b, 0x20),
        })
    }
    /// A present block must belong to `owner`; an absent one names no owner
    /// (`0xffffffff`, or 0 in an all-zero record).
    pub fn check_owner(&self, owner: TagId) -> Result<()> {
        let ok = match self.block {
            Some(_) => self.owner == owner,
            None => self.owner == TagId::NULL || self.owner == TagId(0),
        };
        if ok {
            Ok(())
        } else {
            Err(Error::Invalid("geometry block owner is not this tag"))
        }
    }
}

/// Source of raw-resource bytes (`blkh` blocks), keyed by file selector.
/// Implementations must bound reads by the selected file's length.
pub trait RawSource {
    /// Read `size` bytes at a raw pointer.
    fn read_raw(&mut self, pointer: RawPointer, size: u32) -> Result<Vec<u8>>;
}

/// Largest raw block accepted, checked before reading.
pub const MAX_BLOCK_SIZE: u32 = 16 << 20;

const BLKH: FourCC = FourCC(*b"blkh");
const RSRC: FourCC = FourCC(*b"rsrc");
const BLKF: FourCC = FourCC(*b"blkf");

/// A loaded raw block with its validated resource table. Built only from the
/// block info it was read for, so the pair cannot be mismatched later.
#[derive(Clone, Debug)]
pub struct LoadedBlock {
    bytes: Vec<u8>,
    section_size: usize,
    resources: Vec<BlockResource>,
    /// Present resources: key -> (absolute byte range in `bytes`, secondary).
    ranges: HashMap<(ResourceKind, u16, u16), (Range<usize>, u16)>,
}
impl LoadedBlock {
    /// Validate a block's framing and resources against its block info.
    ///
    /// Every present resource must be preceded by an `rsrc` marker, fit the
    /// resource area, patch a field inside the section struct, and share no
    /// byte (marker included) or key with any other. A block resource's
    /// element size must divide its size, and the section struct's count at
    /// that field must equal size / element size.
    pub fn parse(info: &GeometryBlockInfo, bytes: Vec<u8>) -> Result<Self> {
        // V: a raw-block section is unloaded on disc.
        if info.runtime_loaded != 0 || info.runtime_cache_handle != u32::MAX {
            return Err(Error::Invalid("geometry block info has runtime state"));
        }
        let section = info.section_size as usize;
        let area = info.resource_size as usize;
        let expected = 8usize
            .checked_add(section)
            .and_then(|v| v.checked_add(area))
            .and_then(|v| v.checked_add(4))
            .ok_or(Error::Invalid("geometry block size overflow"))?;
        if bytes.len() != info.block_size as usize || expected != bytes.len() {
            return Err(Error::Invalid(
                "geometry block size disagrees with block info",
            ));
        }
        let fourcc = |o: usize| FourCC::from_u32(u32_at(&bytes, o));
        if fourcc(0) != BLKH || u32_at(&bytes, 4) as usize != area {
            return Err(Error::Invalid("geometry block lacks blkh header"));
        }
        if fourcc(bytes.len() - 4) != BLKF {
            return Err(Error::Invalid("geometry block lacks blkf footer"));
        }
        let base = 8 + section;
        let mut spans = Vec::with_capacity(info.resources.len());
        let mut ranges = HashMap::with_capacity(info.resources.len());
        for r in &info.resources {
            let field = usize::from(r.field_offset);
            if r.unknown_2 != r.field_offset {
                return Err(Error::Invalid("block resource field offsets differ"));
            }
            if field + 8 > section {
                return Err(Error::Invalid("block resource field outside section"));
            }
            let Some(offset) = r.offset else { continue };
            let start = offset as usize;
            let end = start
                .checked_add(r.size as usize)
                .filter(|&e| start >= 4 && e <= area)
                .ok_or(Error::Invalid("block resource outside resource area"))?;
            if fourcc(base + start - 4) != RSRC {
                return Err(Error::Invalid("block resource lacks rsrc marker"));
            }
            if r.kind == ResourceKind::Block {
                let element = usize::from(r.secondary);
                let count = u32_at(&bytes, 8 + field) as usize;
                // count * element == size also makes `element` divide it.
                if element == 0 || count * element != r.size as usize {
                    return Err(Error::Invalid(
                        "section count disagrees with its block resource",
                    ));
                }
            }
            if ranges
                .insert(r.key(), (base + start..base + end, r.secondary))
                .is_some()
            {
                return Err(Error::Invalid("duplicate block resource"));
            }
            spans.push((start - 4, end));
        }
        spans.sort_unstable();
        if spans.windows(2).any(|w| w[1].0 < w[0].1) {
            return Err(Error::Invalid("block resources overlap"));
        }
        Ok(Self {
            bytes,
            section_size: section,
            resources: info.resources.clone(),
            ranges,
        })
    }
    /// The section struct (counts filled, pointers zero on disc).
    pub fn section(&self) -> &[u8] {
        &self.bytes[8..8 + self.section_size]
    }
    /// The resource table this block was validated against.
    pub fn resources(&self) -> &[BlockResource] {
        &self.resources
    }
    /// Bytes of a present resource of this block (`None` if skipped or not
    /// one of this block's entries).
    pub fn resource(&self, r: &BlockResource) -> Option<&[u8]> {
        let (range, _) = self.ranges.get(&r.key())?;
        (range.len() == r.size as usize).then(|| &self.bytes[range.clone()])
    }
    fn range(&self, kind: ResourceKind, field: u16, index: u16) -> Option<Range<usize>> {
        self.ranges
            .get(&(kind, field, index))
            .map(|(r, _)| r.clone())
    }
    fn count_at(&self, field: u16) -> u32 {
        u32_at(self.section(), usize::from(field))
    }
}

/// Load and validate a section's raw block. `Ok(None)` when the info names
/// no block.
pub fn load_block(
    info: &GeometryBlockInfo,
    raw: &mut dyn RawSource,
) -> Result<Option<LoadedBlock>> {
    let Some(pointer) = info.block else {
        return Ok(None);
    };
    if info.block_size > MAX_BLOCK_SIZE {
        return Err(Error::Invalid("geometry block exceeds size budget"));
    }
    let bytes = raw.read_raw(pointer, info.block_size)?;
    LoadedBlock::parse(info, bytes).map(Some)
}

/// Where an owner's section struct keeps the blocks this decoder interprets,
/// and how large their elements are. `None` means the struct has no such
/// block. Other fields are left to the owner (see [`Section::block_bytes`]).
#[derive(Clone, Copy, Debug)]
pub struct SectionLayout {
    /// Section struct size; a block info with another size is rejected.
    pub section_size: u32,
    /// Parts block field.
    pub parts: Option<u16>,
    /// Part element size (at least 0xe).
    pub part_stride: u16,
    /// Subparts block field.
    pub subparts: Option<u16>,
    /// Subpart element size (at least 8).
    pub subpart_stride: u16,
    /// Whether each part's subparts tile its index range (V for structure
    /// sections). When false, a subpart must only be empty or lie inside its
    /// part (render models keep empty placeholder subparts on strip parts).
    pub subparts_tile_indices: bool,
    /// u16 index block field.
    pub indices: Option<u16>,
    /// Vertex-buffer descriptor block field.
    pub vertex_buffers: u16,
    /// Descriptor element size (E `0x00223ced`: `shl 5`, i.e. 0x20).
    pub descriptor_stride: u16,
}

/// Part flag bit 3: the part's indices are a triangle **list**; without it
/// they are a triangle **strip**. D: every flagged part (all structure parts,
/// and 377 model parts) has a multiple of 3 indices, and with this rule the
/// triangle count derived from the indices equals the stored count on every
/// structure and model section.
pub const PART_FLAG_TRIANGLE_LIST: u16 = 0x8;

/// One drawable range of indices.
#[derive(Clone, Debug)]
pub struct Part {
    /// u16 at +0 (E `0x000410fc`..`0x00041129`: kinds 0 and 5 are skipped).
    pub kind: u16,
    /// u16 at +2. Only [`PART_FLAG_TRIANGLE_LIST`] is interpreted.
    pub flags: u16,
    /// u16 at +4: material index into the owner's material block. V: below
    /// the owner's material count on every structure and model part;
    /// `0xffff` = none.
    pub material: u16,
    /// u16 at +6: first index.
    pub first_index: u16,
    /// u16 at +8: index count.
    pub index_count: u16,
    /// u16 at +0xa: first subpart.
    pub first_subpart: u16,
    /// u16 at +0xc: subpart count.
    pub subpart_count: u16,
    /// The whole element.
    pub raw: Vec<u8>,
}
impl Part {
    /// Engine predicate: part kinds 0 and 5 are never submitted.
    pub fn is_drawn(&self) -> bool {
        self.kind != 0 && self.kind != 5
    }
    /// See [`PART_FLAG_TRIANGLE_LIST`].
    pub fn is_triangle_list(&self) -> bool {
        self.flags & PART_FLAG_TRIANGLE_LIST != 0
    }
    /// The part's index range.
    pub fn index_range(&self) -> Range<usize> {
        let first = usize::from(self.first_index);
        first..first + usize::from(self.index_count)
    }
}

/// A sub-range of a part.
#[derive(Clone, Copy, Debug)]
pub struct Subpart {
    /// u16 at +0.
    pub first_index: u16,
    /// u16 at +2.
    pub index_count: u16,
    /// u16 at +4 (V: `0xffff` in structure sections, varies in models).
    pub unknown_4: u16,
    /// u16 at +6. V: always the index of the part whose range holds this
    /// subpart; the decoder requires it.
    pub part: u16,
}

/// Vertex stream type, from byte 0 of the vertex-buffer descriptor. All
/// meanings are disc evidence (the engine's type table is not located); see
/// the docs for the checks behind each.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum VertexKind {
    /// 0x01, stride 12: f32×3 position.
    Position,
    /// 0x02, stride 6: s16×3 position in the model's compression bounds.
    CompressedPosition,
    /// 0x04, stride 8: s16×3 position, u8 node, u8 zero.
    OneNode,
    /// 0x06, stride 12: s16×3, 2 zero bytes, u8×2 nodes, u8×2 weights.
    TwoNodes,
    /// 0x08, stride 12: s16×3, u8×3 nodes, u8×3 weights.
    ThreeNodes,
    /// 0x0a, stride 16: s16×3, 2 zero bytes, u8×4 nodes, u8×4 weights.
    FourNodes,
    /// 0x18, stride 8: f32×2 texture coordinate. D: in models it occurs
    /// exactly when OpenH2's `compressed_tex_coords` flag is clear (0x19
    /// when set); in structure sections only stride and count are checked.
    Texcoord,
    /// 0x19, stride 4: s16×2 texture coordinate in the compression bounds.
    CompressedTexcoord,
    /// 0x1b, stride 12: three packed 11:11:10 vectors; the first is the
    /// normal (D: winding agreement).
    TangentFrame,
    /// 0x1f, stride 4: two normalized i16 lightmap coordinates (V: in
    /// [0, 1]).
    LightmapTexcoord,
    /// Any other type (0x2f stride 3 and 0x30 stride 4 occur): kept as bytes.
    Unknown(u8),
}
impl VertexKind {
    /// Map a descriptor type byte.
    pub fn from_type(t: u8) -> Self {
        match t {
            0x01 => Self::Position,
            0x02 => Self::CompressedPosition,
            0x04 => Self::OneNode,
            0x06 => Self::TwoNodes,
            0x08 => Self::ThreeNodes,
            0x0a => Self::FourNodes,
            0x18 => Self::Texcoord,
            0x19 => Self::CompressedTexcoord,
            0x1b => Self::TangentFrame,
            0x1f => Self::LightmapTexcoord,
            v => Self::Unknown(v),
        }
    }
    /// Stride this decoder requires for an interpreted kind.
    pub fn expected_stride(self) -> Option<u8> {
        Some(match self {
            Self::Position | Self::TwoNodes | Self::ThreeNodes | Self::TangentFrame => 12,
            Self::CompressedPosition => 6,
            Self::OneNode | Self::Texcoord => 8,
            Self::FourNodes => 16,
            Self::CompressedTexcoord | Self::LightmapTexcoord => 4,
            Self::Unknown(_) => return None,
        })
    }
    /// Whether this stream carries positions.
    pub fn is_position(self) -> bool {
        matches!(
            self,
            Self::Position
                | Self::CompressedPosition
                | Self::OneNode
                | Self::TwoNodes
                | Self::ThreeNodes
                | Self::FourNodes
        )
    }
    /// Streams with the same role may not repeat in one section.
    fn role(self) -> (u8, u8) {
        match self {
            k if k.is_position() => (0, 0),
            Self::Texcoord | Self::CompressedTexcoord => (1, 0),
            Self::TangentFrame => (2, 0),
            Self::LightmapTexcoord => (3, 0),
            Self::Unknown(t) => (4, t),
            _ => unreachable!("every kind has a role"),
        }
    }
}

/// One vertex stream: descriptor fields and where its data is in the block.
#[derive(Clone, Debug)]
pub struct VertexBuffer {
    /// Index of the descriptor in the section's descriptor block.
    pub slot: usize,
    /// Descriptor byte 0, decoded.
    pub kind: VertexKind,
    /// Descriptor byte 0.
    pub type_id: u8,
    /// Descriptor byte 1: bytes per vertex.
    pub stride: u8,
    range: Range<usize>,
}
impl VertexBuffer {
    /// Number of vertices.
    pub fn count(&self) -> usize {
        self.range.len() / usize::from(self.stride)
    }
}

/// Bounds that compressed (s16) positions and texcoords expand into:
/// x/y/z min,max then u/v min,max.
#[derive(Clone, Copy, Debug, Default)]
pub struct Compression {
    pub position: [f32; 6],
    pub texcoord: [f32; 4],
}

/// Decoded vertex attributes. Empty vectors mean the section has no such
/// stream.
#[derive(Clone, Debug, Default)]
pub struct Vertices {
    pub positions: Vec<[f32; 3]>,
    pub texcoords: Vec<[f32; 2]>,
    /// The three packed vectors of the tangent-frame stream, in order.
    pub normals: Vec<[f32; 3]>,
    pub binormals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    /// Lightmap coordinates (type 0x1f).
    pub lightmap_texcoords: Vec<[f32; 2]>,
    /// Node indices of skinned position streams; unused slots are 0.
    pub node_indices: Vec<[u8; 4]>,
    /// Weights in 0..=1; one-node vertices get `[1, 0, 0, 0]`.
    pub node_weights: Vec<[f32; 4]>,
}

/// A loaded section with its interpreted blocks.
#[derive(Clone, Debug)]
pub struct Section {
    /// Parts in index order (they tile the index block).
    pub parts: Vec<Part>,
    /// Subparts (each part's run tiles this block).
    pub subparts: Vec<Subpart>,
    /// u16 indices; each part is a list or strip (see [`Part`]).
    pub indices: Vec<u16>,
    /// Vertex streams in descriptor order. Zeroed descriptor slots without
    /// data (lightmap buckets reserve two) are omitted.
    pub vertex_buffers: Vec<VertexBuffer>,
    vertex_count: usize,
    block: LoadedBlock,
}

fn elements(block: &LoadedBlock, field: Option<u16>, stride: u16) -> Result<Vec<&[u8]>> {
    let Some(field) = field else {
        return Ok(Vec::new());
    };
    match block.ranges.get(&(ResourceKind::Block, field, 0)) {
        Some((range, element)) => {
            // parse() already checked that `element` divides the size.
            if *element != stride {
                return Err(Error::Invalid(
                    "geometry block resource has unexpected element size",
                ));
            }
            Ok(block.bytes[range.clone()]
                .chunks_exact(usize::from(stride))
                .collect())
        }
        None if block.count_at(field) == 0 => Ok(Vec::new()),
        None => Err(Error::Invalid("populated section block has no resource")),
    }
}

/// Decode a loaded block's parts, subparts, indices and vertex streams.
///
/// `vertex_count` is the owner's own count, when it has one: every stream
/// must hold exactly that many vertices. A section with no streams is only
/// accepted when that count is 0 and it has no parts or indices.
pub fn decode_section(
    block: LoadedBlock,
    layout: &SectionLayout,
    vertex_count: Option<usize>,
) -> Result<Section> {
    if block.section_size != layout.section_size as usize {
        return Err(Error::Invalid(
            "section struct size disagrees with its layout",
        ));
    }
    let parts = elements(&block, layout.parts, layout.part_stride)?
        .into_iter()
        .map(|b| Part {
            kind: u16_at(b, 0),
            flags: u16_at(b, 2),
            material: u16_at(b, 4),
            first_index: u16_at(b, 6),
            index_count: u16_at(b, 8),
            first_subpart: u16_at(b, 0xa),
            subpart_count: u16_at(b, 0xc),
            raw: b.to_vec(),
        })
        .collect::<Vec<_>>();
    let subparts = elements(&block, layout.subparts, layout.subpart_stride)?
        .into_iter()
        .map(|b| Subpart {
            first_index: u16_at(b, 0),
            index_count: u16_at(b, 2),
            unknown_4: u16_at(b, 4),
            part: u16_at(b, 6),
        })
        .collect::<Vec<_>>();
    let indices = elements(&block, layout.indices, 2)?
        .into_iter()
        .map(|b| u16_at(b, 0))
        .collect::<Vec<_>>();
    let descriptors: Vec<[u8; 2]> = elements(
        &block,
        Some(layout.vertex_buffers),
        layout.descriptor_stride,
    )?
    .into_iter()
    .map(|b| [b[0], b[1]])
    .collect();

    // Every vertex resource must belong to one descriptor of this block.
    for r in block.resources.iter().filter(|r| r.offset.is_some()) {
        if r.kind == ResourceKind::VertexBuffer
            && (r.field_offset != layout.vertex_buffers
                || usize::from(r.secondary) >= descriptors.len())
        {
            return Err(Error::Invalid("vertex resource has no descriptor"));
        }
    }
    let mut vertex_buffers = Vec::with_capacity(descriptors.len());
    let mut roles = Vec::with_capacity(descriptors.len());
    for (i, d) in descriptors.iter().enumerate() {
        let range = u16::try_from(i)
            .ok()
            .and_then(|i| block.range(ResourceKind::VertexBuffer, layout.vertex_buffers, i));
        let Some(range) = range else {
            // Fixed-size descriptor arrays leave unused slots zeroed.
            if *d == [0, 0] {
                continue;
            }
            return Err(Error::Invalid(
                "vertex buffer descriptor has no data resource",
            ));
        };
        let [type_id, stride] = *d;
        let kind = VertexKind::from_type(type_id);
        if stride == 0 || kind.expected_stride().is_some_and(|s| s != stride) {
            return Err(Error::Invalid(
                "vertex stream stride disagrees with its type",
            ));
        }
        if range.len() % usize::from(stride) != 0 {
            return Err(Error::Invalid(
                "vertex data is not a whole number of vertices",
            ));
        }
        if roles.contains(&kind.role()) {
            return Err(Error::Invalid("section repeats a vertex stream kind"));
        }
        roles.push(kind.role());
        vertex_buffers.push(VertexBuffer {
            slot: i,
            kind,
            type_id,
            stride,
            range,
        });
    }

    let vertices = match vertex_buffers.first() {
        Some(first) => {
            let n = first.count();
            if vertex_buffers.iter().any(|v| v.count() != n) {
                return Err(Error::Invalid("vertex streams disagree on vertex count"));
            }
            n
        }
        None if vertex_count == Some(0) && parts.is_empty() && indices.is_empty() => 0,
        None => return Err(Error::Invalid("section has no vertex streams")),
    };
    if vertex_count.is_some_and(|n| n != vertices) {
        return Err(Error::Invalid(
            "vertex streams disagree with the owner's vertex count",
        ));
    }
    if indices.iter().any(|&i| usize::from(i) >= vertices) {
        return Err(Error::Invalid("index outside vertex streams"));
    }
    check_tiling(&parts, &subparts, indices.len(), layout)?;
    Ok(Section {
        parts,
        subparts,
        indices,
        vertex_buffers,
        vertex_count: vertices,
        block,
    })
}

/// Parts tile the index block in order; list parts hold whole triangles;
/// each part's subparts tile the subpart block in order and name the part.
fn check_tiling(
    parts: &[Part],
    subparts: &[Subpart],
    index_count: usize,
    layout: &SectionLayout,
) -> Result<()> {
    let mut next_index = 0usize;
    let mut next_subpart = 0usize;
    for (pi, p) in parts.iter().enumerate() {
        let range = p.index_range();
        if range.start != next_index || range.end > index_count {
            return Err(Error::Invalid("parts do not tile the index block"));
        }
        next_index = range.end;
        if p.is_triangle_list() && range.len() % 3 != 0 {
            return Err(Error::Invalid("triangle-list part is not whole triangles"));
        }
        let first = usize::from(p.first_subpart);
        let end = first + usize::from(p.subpart_count);
        if first != next_subpart || end > subparts.len() {
            return Err(Error::Invalid("parts do not tile the subpart block"));
        }
        next_subpart = end;
        let mut cursor = range.start;
        for s in &subparts[first..end] {
            if usize::from(s.part) != pi {
                return Err(Error::Invalid("subpart names another part"));
            }
            let start = usize::from(s.first_index);
            let stop = start + usize::from(s.index_count);
            let fits = if layout.subparts_tile_indices {
                start == cursor && stop <= range.end
            } else {
                s.index_count == 0 || (start >= range.start && stop <= range.end)
            };
            if !fits {
                return Err(Error::Invalid("subpart outside its part"));
            }
            cursor = stop;
        }
        // Every structure part's indices must be covered, so a nonempty part
        // with an empty subpart run fails here (cursor stays at range.start).
        if layout.subparts_tile_indices && cursor != range.end {
            return Err(Error::Invalid("subparts do not tile their part"));
        }
    }
    if next_index != index_count || next_subpart != subparts.len() {
        return Err(Error::Invalid("parts do not tile the index block"));
    }
    Ok(())
}

/// Map a signed-normalised short to `[-1, 1]` (D3D NORMSHORT convention).
fn snorm16(b: &[u8], at: usize) -> f32 {
    (f32::from(u16_at(b, at) as i16) / 32767.0).max(-1.0)
}
/// Expand `[-1, 1]` into `[min, max]`.
fn expand(v: f32, min: f32, max: f32) -> f32 {
    (v + 1.0) * 0.5 * (max - min) + min
}
/// Xbox NORMPACKED3: signed 11:11:10, x in the low bits (as `gfx::repack`).
pub fn unpack_normpacked3(raw: u32) -> [f32; 3] {
    let signed = |v: u32, bits: u32| -> f32 {
        let shift = 32 - bits;
        let s = ((v << shift) as i32) >> shift;
        (s as f32 / ((1i32 << (bits - 1)) - 1) as f32).max(-1.0)
    };
    [
        signed(raw & 0x7ff, 11),
        signed((raw >> 11) & 0x7ff, 11),
        signed(raw >> 22, 10),
    ]
}

impl Section {
    /// Vertex count shared by every stream.
    pub fn vertex_count(&self) -> usize {
        self.vertex_count
    }
    /// The stream of a kind, if present (kinds never repeat).
    pub fn stream(&self, kind: VertexKind) -> Option<&VertexBuffer> {
        self.vertex_buffers.iter().find(|v| v.kind == kind)
    }
    /// The section's position stream, if any.
    pub fn position_stream(&self) -> Option<&VertexBuffer> {
        self.vertex_buffers.iter().find(|v| v.kind.is_position())
    }
    /// A stream's raw bytes.
    pub fn data(&self, v: &VertexBuffer) -> &[u8] {
        &self.block.bytes[v.range.clone()]
    }
    /// The section struct of the loaded block.
    pub fn section_struct(&self) -> &[u8] {
        self.block.section()
    }
    /// Elements of a block field the layout does not interpret (for example
    /// a model's node map), checked like the interpreted ones: the resource's
    /// element size must be `stride`, and an unpatched field must count 0.
    pub fn extra_block(&self, field: u16, stride: u16) -> Result<Vec<&[u8]>> {
        elements(&self.block, Some(field), stride)
    }
    /// A part's raw index range (validated at decode).
    pub fn part_indices(&self, part: &Part) -> &[u16] {
        &self.indices[part.index_range()]
    }
    /// Triangles of one part, degenerates dropped.
    pub fn part_triangles(&self, part: &Part) -> Vec<[u16; 3]> {
        let indices = self.part_indices(part);
        if part.is_triangle_list() {
            list_triangles(indices)
        } else {
            strip_triangles(indices)
        }
    }

    /// Decode every interpreted stream. Compressed (s16) streams need
    /// `compression`; non-finite bounds or float attributes are rejected.
    pub fn vertices(&self, compression: Option<&Compression>) -> Result<Vertices> {
        let mut out = Vertices::default();
        if let Some(c) = compression
            && !c.position.iter().chain(&c.texcoord).all(|x| x.is_finite())
        {
            return Err(Error::Invalid("non-finite compression bounds"));
        }
        let bounds = || compression.ok_or(Error::Invalid("compressed stream without bounds"));
        for vb in &self.vertex_buffers {
            #[allow(clippy::chunks_exact_to_as_chunks)] // stride is a runtime value
            let rows = self.data(vb).chunks_exact(usize::from(vb.stride));
            match vb.kind {
                VertexKind::Position => {
                    out.positions = rows
                        .map(|r| [f32_at(r, 0), f32_at(r, 4), f32_at(r, 8)])
                        .collect();
                }
                k if k.is_position() => {
                    let p = bounds()?.position;
                    let skin: (&[usize], &[usize]) = match k {
                        VertexKind::OneNode => (&[6], &[]),
                        VertexKind::TwoNodes => (&[8, 9], &[10, 11]),
                        VertexKind::ThreeNodes => (&[6, 7, 8], &[9, 10, 11]),
                        VertexKind::FourNodes => (&[8, 9, 10, 11], &[12, 13, 14, 15]),
                        _ => (&[], &[]),
                    };
                    for r in rows {
                        out.positions.push([
                            expand(snorm16(r, 0), p[0], p[1]),
                            expand(snorm16(r, 2), p[2], p[3]),
                            expand(snorm16(r, 4), p[4], p[5]),
                        ]);
                        let (ni, wi) = skin;
                        if ni.is_empty() {
                            continue;
                        }
                        let mut nodes = [0u8; 4];
                        let mut weights = [0f32; 4];
                        for (k, &at) in ni.iter().enumerate() {
                            nodes[k] = r[at];
                            weights[k] = wi.get(k).map_or(1.0, |&w| f32::from(r[w]) / 255.0);
                        }
                        out.node_indices.push(nodes);
                        out.node_weights.push(weights);
                    }
                }
                VertexKind::Texcoord => {
                    out.texcoords = rows.map(|r| [f32_at(r, 0), f32_at(r, 4)]).collect();
                }
                VertexKind::CompressedTexcoord => {
                    let t = bounds()?.texcoord;
                    out.texcoords = rows
                        .map(|r| {
                            [
                                expand(snorm16(r, 0), t[0], t[1]),
                                expand(snorm16(r, 2), t[2], t[3]),
                            ]
                        })
                        .collect();
                }
                VertexKind::TangentFrame => {
                    for r in rows {
                        out.normals.push(unpack_normpacked3(u32_at(r, 0)));
                        out.binormals.push(unpack_normpacked3(u32_at(r, 4)));
                        out.tangents.push(unpack_normpacked3(u32_at(r, 8)));
                    }
                }
                VertexKind::LightmapTexcoord => {
                    out.lightmap_texcoords = rows.map(|r| [snorm16(r, 0), snorm16(r, 2)]).collect();
                }
                _ => {}
            }
        }
        let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
        if !out.positions.iter().all(|p| finite(p)) || !out.texcoords.iter().all(|t| finite(t)) {
            return Err(Error::Invalid("non-finite vertex attribute"));
        }
        Ok(out)
    }
}

/// Expand a triangle strip into triangles, alternating winding and dropping
/// degenerate triangles (strips join runs with repeated indices).
pub fn strip_triangles(strip: &[u16]) -> Vec<[u16; 3]> {
    let mut out = Vec::with_capacity(strip.len().saturating_sub(2));
    for (i, w) in strip.windows(3).enumerate() {
        let (a, b, c) = (w[0], w[1], w[2]);
        if a == b || b == c || a == c {
            continue;
        }
        out.push(if i % 2 == 0 { [a, b, c] } else { [a, c, b] });
    }
    out
}

/// Split an index list into triangles, dropping degenerate ones.
pub fn list_triangles(list: &[u16]) -> Vec<[u16; 3]> {
    list.as_chunks::<3>()
        .0
        .iter()
        .copied()
        .filter(|[a, b, c]| a != b && b != c && a != c)
        .collect()
}
