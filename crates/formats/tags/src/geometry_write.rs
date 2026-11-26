//! The one geometry **section encoder**, the inverse of
//! [`crate::geometry`]'s decoder, for every owner's layout
//! (`STRUCTURE_SECTION`, `MODEL_SECTION`): a section struct and its
//! resources framed as a raw block (`blkh`, struct, `rsrc` resources,
//! `blkf`), with its resource table. Design and the write classes it serves
//! are in docs/editor/modeling-targets.md ("The session and the save").
//!
//! The rule it keeps: a block read with [`SectionData::from_block`] and
//! encoded again is the same bytes, and a resource that keeps its size
//! stays at its offset, so an edit that changes no size changes only the
//! bytes it rewrote. A resource the caller does not touch is carried as the
//! source's bytes, whatever it holds. When a size changes, the resources are
//! laid out again in table order, each marker four-aligned (**Design**: the
//! engine's own alignment rule for the area is not located), and a data
//! resource (the visibility MOPP code) starts 16-aligned in the block with
//! its size in its struct field, as `h1convert::structure::raw_block` lays
//! out the code object Havok reads.
//!
//! The packers ([`pack_normpacked3`], [`quantize`], [`snorm16`]) are the
//! inverses of the decoder's expansions, for the streams an in-place edit
//! rewrites.

use crate::bytes::u32_at;
use crate::geometry::{BLKF, BLKH, BlockResource, LoadedBlock, RSRC, ResourceKind, SectionLayout};
use crate::{Error, Result};

/// One resource of a section: its table entry and, when present, its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceData {
    pub kind: ResourceKind,
    pub unknown_1: u8,
    /// Offset of the patched field in the section struct.
    pub field: u16,
    /// Element size (blocks), descriptor index (vertex data), or `0xffff`.
    pub secondary: u16,
    /// `None`: a skipped entry (offset `0xffffffff`), kept as one.
    pub bytes: Option<Vec<u8>>,
    /// The entry's size as read: a skipped entry keeps it, and a present
    /// one whose bytes still have it keeps its offset in the source block.
    pub declared_size: u32,
}

impl ResourceData {
    fn key(&self) -> (ResourceKind, u16, u16) {
        let index = match self.kind {
            ResourceKind::VertexBuffer => self.secondary,
            _ => 0,
        };
        (self.kind, self.field, index)
    }
}

/// A section as the encoder takes it: the struct, every resource in table
/// order, and the source block whose framing is kept while sizes are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionData {
    /// The section struct. Block counts are rewritten from the resources.
    pub section: Vec<u8>,
    pub resources: Vec<ResourceData>,
    /// The block this was read from, and each resource's offset in it.
    source: Option<(Vec<u8>, Vec<Option<u32>>)>,
}

/// An encoded section: the raw block and the resource table that patches
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedSection {
    pub block: Vec<u8>,
    pub resources: Vec<BlockResource>,
    /// The section struct's length (the block info's `+8`).
    pub section_size: u32,
    /// The resource area's length (the block info's `+0xc`).
    pub resource_size: u32,
}

impl SectionData {
    /// A section with no source block: every resource is laid out anew.
    pub fn new(section: Vec<u8>, resources: Vec<ResourceData>) -> Self {
        Self {
            section,
            resources,
            source: None,
        }
    }

    /// Everything a loaded block holds, interpreted or not.
    pub fn from_block(block: &LoadedBlock) -> Self {
        let resources = block
            .resources()
            .iter()
            .map(|r| ResourceData {
                kind: r.kind,
                unknown_1: r.unknown_1,
                field: r.field_offset,
                secondary: r.secondary,
                bytes: block.resource(r).map(<[u8]>::to_vec),
                declared_size: r.size,
            })
            .collect();
        let offsets = block.resources().iter().map(|r| r.offset).collect();
        Self {
            section: block.section().to_vec(),
            resources,
            source: Some((block.bytes().to_vec(), offsets)),
        }
    }

    /// The bytes of the present resource with this key (vertex data by
    /// descriptor index, blocks and data by field).
    pub fn resource(&self, kind: ResourceKind, field: u16, index: u16) -> Option<&[u8]> {
        self.find(kind, field, index)
            .and_then(|i| self.resources[i].bytes.as_deref())
    }

    /// The same, to change in place (keep its length for an in-place
    /// write).
    pub fn resource_mut(
        &mut self,
        kind: ResourceKind,
        field: u16,
        index: u16,
    ) -> Option<&mut Vec<u8>> {
        let i = self.find(kind, field, index)?;
        self.resources[i].bytes.as_mut()
    }

    fn find(&self, kind: ResourceKind, field: u16, index: u16) -> Option<usize> {
        let index = if kind == ResourceKind::VertexBuffer {
            index
        } else {
            0
        };
        self.resources
            .iter()
            .position(|r| r.key() == (kind, field, index))
    }

    /// Whether every resource keeps the size it had in the source block,
    /// and the struct its length: the encoding is then in place.
    pub fn keeps_sizes(&self) -> bool {
        let Some((block, offsets)) = &self.source else {
            return false;
        };
        let area = if block.len() >= 12 {
            u32_at(block, 4) as usize
        } else {
            0
        };
        block.len() >= 12
            && 8 + self.section.len() + area + 4 == block.len()
            && offsets.len() == self.resources.len()
            && self
                .resources
                .iter()
                .zip(offsets)
                .all(|(r, o)| match (&r.bytes, o) {
                    (Some(b), Some(_)) => b.len() == r.declared_size as usize,
                    (None, None) => true,
                    _ => false,
                })
    }
}

/// Encode a section for `layout`. Refused when the struct is not the
/// layout's size, a block resource's size is not whole elements, or the
/// block would exceed [`crate::geometry::MAX_BLOCK_SIZE`].
pub fn encode_section(layout: &SectionLayout, data: &SectionData) -> Result<EncodedSection> {
    if data.section.len() != layout.section_size as usize {
        return Err(Error::Invalid(
            "section struct size disagrees with its layout",
        ));
    }
    let mut section = data.section.clone();
    for r in &data.resources {
        let field = usize::from(r.field);
        if field + 8 > section.len() {
            return Err(Error::Invalid("block resource field outside section"));
        }
        if r.kind == ResourceKind::Block
            && let Some(b) = &r.bytes
        {
            let element = usize::from(r.secondary);
            if element == 0 || b.len() % element != 0 {
                return Err(Error::Invalid("block resource is not whole elements"));
            }
            let count = u32::try_from(b.len() / element)
                .map_err(|_| Error::Invalid("block resource count overflow"))?;
            section[field..field + 4].copy_from_slice(&count.to_le_bytes());
        }
    }
    let entry = |r: &ResourceData, offset: Option<u32>| BlockResource {
        kind: r.kind,
        unknown_1: r.unknown_1,
        unknown_2: r.field,
        field_offset: r.field,
        secondary: r.secondary,
        size: r.bytes.as_ref().map_or(r.declared_size, |b| b.len() as u32),
        offset,
    };
    let section_size = section.len() as u32;
    if data.keeps_sizes() {
        let (source, offsets) = data.source.as_ref().expect("keeps_sizes has a source");
        let mut block = source.clone();
        block[8..8 + section.len()].copy_from_slice(&section);
        let base = 8 + section.len();
        let mut resources = Vec::with_capacity(data.resources.len());
        for (r, &offset) in data.resources.iter().zip(offsets) {
            if let (Some(b), Some(o)) = (&r.bytes, offset) {
                let at = base + o as usize;
                block[at..at + b.len()].copy_from_slice(b);
            }
            resources.push(entry(r, offset));
        }
        let resource_size = (block.len() - base - 4) as u32;
        return Ok(EncodedSection {
            block,
            resources,
            section_size,
            resource_size,
        });
    }
    let mut area: Vec<u8> = Vec::new();
    let mut resources = Vec::with_capacity(data.resources.len());
    // Block offset of the resource area's first byte.
    let base = 8 + section.len();
    for r in &data.resources {
        let Some(b) = &r.bytes else {
            resources.push(entry(r, None));
            continue;
        };
        area.resize(area.len().next_multiple_of(4), 0);
        if r.kind == ResourceKind::Data {
            let field = usize::from(r.field);
            let size = u32::try_from(b.len())
                .map_err(|_| Error::Invalid("data resource size overflow"))?;
            section[field..field + 4].copy_from_slice(&size.to_le_bytes());
            while !(base + area.len() + 4).is_multiple_of(16) {
                area.push(0);
            }
        }
        area.extend_from_slice(&RSRC.as_u32().to_le_bytes());
        let offset = area.len() as u32;
        area.extend_from_slice(b);
        resources.push(entry(r, Some(offset)));
    }
    let total = 8 + section.len() + area.len() + 4;
    if total > crate::geometry::MAX_BLOCK_SIZE as usize {
        return Err(Error::Invalid("geometry block exceeds size budget"));
    }
    let mut block = Vec::with_capacity(total);
    block.extend_from_slice(&BLKH.as_u32().to_le_bytes());
    block.extend_from_slice(&(area.len() as u32).to_le_bytes());
    block.extend_from_slice(&section);
    block.extend_from_slice(&area);
    block.extend_from_slice(&BLKF.as_u32().to_le_bytes());
    Ok(EncodedSection {
        block,
        resources,
        section_size,
        resource_size: area.len() as u32,
    })
}

/// The runs where `new` differs from `old` (same length): `(offset,
/// bytes)`, one per contiguous run, so a write touches only changed bytes.
pub fn changed_runs(old: &[u8], new: &[u8]) -> Vec<(usize, Vec<u8>)> {
    debug_assert_eq!(old.len(), new.len());
    let mut out = Vec::new();
    let mut i = 0;
    let n = old.len().min(new.len());
    while i < n {
        if old[i] == new[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && old[i] != new[i] {
            i += 1;
        }
        out.push((start, new[start..i].to_vec()));
    }
    out
}

/// A value in `[-1, 1]` as a signed-normalised short (the inverse of the
/// decoder's `s16 / 32767`, clamped to -1).
pub fn snorm16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// `v` in `[min, max]` quantised as the decoder expands it
/// (`(s + 1) / 2 * (max - min) + min`). `None` when `v` is outside the
/// bounds by more than a step, or the bounds are not finite: the caller
/// grows the box first.
pub fn quantize(v: f32, min: f32, max: f32) -> Option<i16> {
    if !(v.is_finite() && min.is_finite() && max.is_finite()) || max < min {
        return None;
    }
    let span = max - min;
    if span == 0.0 {
        return (v == min).then_some(0);
    }
    let unit = (v - min) / span * 2.0 - 1.0;
    let step = 1.0 / 32767.0;
    if !(-1.0 - step..=1.0 + step).contains(&unit) {
        return None;
    }
    Some(snorm16(unit))
}

/// Xbox NORMPACKED3 (signed 11:11:10, x in the low bits), the inverse of
/// [`crate::geometry::unpack_normpacked3`].
pub fn pack_normpacked3(v: [f32; 3]) -> u32 {
    let field = |x: f32, bits: u32| -> u32 {
        let max = ((1i32 << (bits - 1)) - 1) as f32;
        let s = (x.clamp(-1.0, 1.0) * max).round() as i32;
        (s as u32) & ((1u32 << bits) - 1)
    };
    field(v[0], 11) | (field(v[1], 11) << 11) | (field(v[2], 10) << 22)
}

#[cfg(test)]
#[path = "geometry_write_tests.rs"]
mod tests;
