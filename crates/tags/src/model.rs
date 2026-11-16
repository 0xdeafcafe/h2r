//! Render-model (`mode`) geometry: regions/permutations, nodes, materials
//! (as tag references only) and sections. Sections are loaded by the shared
//! decoder in [`crate::geometry`] with [`MODEL_SECTION`]; this module adds
//! what is model-only: compression bounds, the compression flags, skinning
//! node maps and the material/node/permutation cross-checks.
//!
//! Wire records are in `schema.rs` (render_model section). Evidence is in
//! docs/formats/map-format.md, "Render-model geometry". Unknown vertex types are kept
//! as bytes, never guessed.
use crate::decode::Payload;
use crate::geometry::{
    self, Compression, GeometryBlockInfo, RawSource, SectionLayout, TagView, Vertices,
    decode_section, load_block, u32_at,
};
use crate::records::*;
use crate::wire::{BinaryRecord, TagBlock, TagReference};
use crate::{DecodeMode, Error, Result, Validation};
use map::{Map, RawPointer, TagId};
use std::io::{Read, Seek};

/// The 0x70-byte model section struct (`ModeSection` raw block).
///
/// Parts at 0 (E `0x000410da`..`0x000410e0`), descriptors at 0x38 (E
/// `0x00223ced`). Subparts at 8, indices at 0x20 and the node map at 0x64:
/// D (resource element sizes 8, 2 and 1; what the bytes decode to). Blocks
/// at 0x10, 0x44, 0x54 and 0x5c are patched but not interpreted. Strip parts
/// keep empty placeholder subparts, so subparts do not tile index ranges.
pub const MODEL_SECTION: SectionLayout = SectionLayout {
    section_size: 0x70,
    parts: Some(0),
    part_stride: 0x48,
    subparts: Some(8),
    subpart_stride: 8,
    subparts_tile_indices: false,
    indices: Some(0x20),
    vertex_buffers: 0x38,
    descriptor_stride: 0x20,
};
/// Node-map block field in [`MODEL_SECTION`] (u8 elements).
const NODE_MAP: u16 = 0x64;
/// Offset of the geometry block info inside a `ModeSection` record.
const BLOCK_INFO: usize = 0x38;
/// V: the block info's runtime block is the 8 bytes at section +0x30, whose
/// pointer the engine reads at +0x34 (E `0x00041076`..`0x00041086`).
const RUNTIME_BLOCK_OFFSET: i16 = 0x30 - BLOCK_INFO as i16;

/// A decoded render model. Materials are references; shaders are not read.
#[derive(Clone, Debug)]
pub struct RenderModelGeometry {
    pub id: TagId,
    pub name: Option<String>,
    pub header: ModeGeometryHeader,
    pub compression: Option<ModeCompression>,
    pub regions: Vec<Region>,
    pub nodes: Vec<Node>,
    pub materials: Vec<ModeMaterial>,
    pub sections: Vec<Section>,
    pub validation: Validation,
}
#[derive(Clone, Debug)]
pub struct Region {
    pub wire: ModeRegion,
    pub name: Option<String>,
    pub permutations: Vec<Permutation>,
}
#[derive(Clone, Debug)]
pub struct Permutation {
    pub wire: ModePermutation,
    pub name: Option<String>,
    /// Section per detail level, LOD 0 (lowest) to 5 (highest); `None` = -1.
    pub lod_sections: [Option<usize>; 6],
}
impl Permutation {
    /// The highest-detail section present.
    pub fn best_section(&self) -> Option<usize> {
        self.lod_sections.iter().rev().flatten().next().copied()
    }
}
#[derive(Clone, Debug)]
pub struct Node {
    pub wire: ModeNode,
    pub name: Option<String>,
}
/// One model section: its record, block info, the decoded geometry, and
/// its vertices expanded with the model's compression bounds.
#[derive(Clone, Debug)]
pub struct Section {
    pub wire: ModeSection,
    pub block: GeometryBlockInfo,
    /// Where the raw block came from.
    pub raw: RawPointer,
    pub geometry: geometry::Section,
    /// Section-local node index -> model node index (empty = identity).
    pub node_map: Vec<u8>,
    pub vertices: Vertices,
}

/// Compression flag bits of `ModeSection` (OpenH2 names; D: bit 0 is set
/// exactly when the position stream is s16, bit 1 when texcoords are).
const COMPRESSED_POSITIONS: u16 = 1;
const COMPRESSED_TEXCOORDS: u16 = 2;

impl RenderModelGeometry {
    /// Decode a `mode` tag's geometry, reading section raw blocks through `raw`.
    pub fn read<R: Read + Seek>(
        map: &mut Map<R>,
        id: TagId,
        raw: &mut dyn RawSource,
    ) -> Result<Self> {
        let tag = map.tag(id)?.clone();
        if tag.group.0 != *b"mode" {
            return Err(Error::UnsupportedGroup(tag.group));
        }
        let location = tag.location.ok_or(map::Error::NoTagData)?;
        let bytes = map.read_tag_bytes(id)?;
        let p = Payload {
            bytes: &bytes,
            location,
            mode: DecodeMode::Strict,
            warnings: Default::default(),
        };
        let view = TagView::new(&bytes, location.address);
        let mut v = Validation::default();
        let header: ModeGeometryHeader = p.record(0, &mut v)?;
        let strings = map.strings();
        let name = header.name.resolve(strings)?.map(str::to_owned);
        v.string_ids += 1;

        let compression = elements::<ModeCompression>(&p, &view, header.compression_infos, &mut v)?;
        if compression.len() > 1 {
            return Err(Error::Invalid("more than one compression record"));
        }
        let compression = compression.into_iter().next().map(|(c, _)| c);

        let section_count = header.sections.count as usize;
        let mut regions = Vec::new();
        for (wire, _) in elements::<ModeRegion>(&p, &view, header.regions, &mut v)? {
            let mut permutations = Vec::new();
            for (pw, _) in elements::<ModePermutation>(&p, &view, wire.permutations, &mut v)? {
                let raw = [
                    pw.lod0_section,
                    pw.lod1_section,
                    pw.lod2_section,
                    pw.lod3_section,
                    pw.lod4_section,
                    pw.lod5_section,
                ];
                let mut lod_sections = [None; 6];
                for (slot, &s) in lod_sections.iter_mut().zip(&raw) {
                    if s == -1 {
                        continue;
                    }
                    if s < 0 || s as usize >= section_count {
                        return Err(Error::Invalid("permutation section index"));
                    }
                    *slot = Some(s as usize);
                }
                v.string_ids += 1;
                permutations.push(Permutation {
                    name: pw.name.resolve(strings)?.map(str::to_owned),
                    wire: pw,
                    lod_sections,
                });
            }
            v.string_ids += 1;
            regions.push(Region {
                name: wire.name.resolve(strings)?.map(str::to_owned),
                wire,
                permutations,
            });
        }

        let mut nodes = Vec::new();
        let node_wires = elements::<ModeNode>(&p, &view, header.nodes, &mut v)?;
        let node_count = node_wires.len();
        for (wire, _) in node_wires {
            for link in [
                wire.parent_index,
                wire.first_child_index,
                wire.next_sibling_index,
            ] {
                if link != -1 && (link < 0 || link as usize >= node_count) {
                    return Err(Error::Invalid("node link out of range"));
                }
            }
            v.string_ids += 1;
            nodes.push(Node {
                name: wire.name.resolve(strings)?.map(str::to_owned),
                wire,
            });
        }

        let materials: Vec<ModeMaterial> = elements(&p, &view, header.materials, &mut v)?
            .into_iter()
            .map(|(m, _)| m)
            .collect();
        for m in &materials {
            check_reference(map, m.unknown_reference)?;
            check_reference(map, m.shader)?;
            v.tag_references += 2;
        }

        let ctx = SectionContext {
            owner: id,
            compression: compression.as_ref().map(|c| Compression {
                position: c.position_bounds,
                texcoord: c.texcoord_bounds,
            }),
            material_count: materials.len(),
            node_count,
        };
        let mut sections = Vec::new();
        for (index, (wire, record)) in elements::<ModeSection>(&p, &view, header.sections, &mut v)?
            .into_iter()
            .enumerate()
        {
            let section =
                read_section(&view, wire, record, &ctx, raw, &mut v).map_err(|source| {
                    Error::Section {
                        index,
                        source: Box::new(source),
                    }
                })?;
            sections.push(section);
        }
        Ok(Self {
            id,
            name,
            header,
            compression,
            regions,
            nodes,
            materials,
            sections,
            validation: v,
        })
    }

    /// The default draw set: for each region, its first permutation's
    /// highest-detail section. Duplicates are removed.
    pub fn default_sections(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for r in &self.regions {
            if let Some(s) = r.permutations.first().and_then(Permutation::best_section)
                && !out.contains(&s)
            {
                out.push(s);
            }
        }
        out
    }
}

/// Decode a tag block's elements, charging the view's byte budget (so
/// aliased blocks cannot amplify), and keep each element's bytes.
fn elements<'a, T: BinaryRecord>(
    p: &Payload<'_>,
    view: &TagView<'a>,
    block: TagBlock,
    v: &mut Validation,
) -> Result<Vec<(T, &'a [u8])>> {
    v.blocks += 1;
    let records = view.elements(block.count, block.address, T::SIZE)?;
    if block.count != 0 {
        v.nonempty_blocks += 1;
        v.block_elements += u64::from(block.count);
    }
    records
        .into_iter()
        .map(|b| Ok((p.nested(b, v)?, b)))
        .collect()
}

fn check_reference<R: Read + Seek>(map: &Map<R>, r: TagReference) -> Result<()> {
    if r.id == TagId::NULL {
        return Ok(());
    }
    let target = map.tag(r.id)?;
    // Some retail material references store group 0xffffffff with a live ID;
    // the ID must still resolve to a live tag.
    let compatible = r.group.as_u32() == u32::MAX
        || r.group == target.group
        || map
            .groups()
            .iter()
            .find(|g| g.class == target.group)
            .is_some_and(|g| g.parent == Some(r.group) || g.grandparent == Some(r.group));
    if !compatible {
        return Err(Error::Invalid("tag reference class mismatch"));
    }
    Ok(())
}

/// Model-wide facts a section is checked against.
struct SectionContext {
    owner: TagId,
    compression: Option<Compression>,
    material_count: usize,
    node_count: usize,
}

fn read_section(
    view: &TagView<'_>,
    wire: ModeSection,
    record: &[u8],
    ctx: &SectionContext,
    raw: &mut dyn RawSource,
    v: &mut Validation,
) -> Result<Section> {
    let block = GeometryBlockInfo::decode(view, record, BLOCK_INFO)?;
    block.check_owner(ctx.owner)?;
    let pointer = block
        .block
        .ok_or(Error::Invalid("model section has no raw block"))?;
    if block.runtime_block_offset != RUNTIME_BLOCK_OFFSET
        || u32_at(record, 0x30) != 0
        || wire.runtime_section_data != 0
    {
        return Err(Error::Invalid("model section runtime block on disc"));
    }
    let loaded = load_block(&block, raw)?.ok_or(Error::Invalid("model section block"))?;
    v.records += 1;
    let geometry = decode_section(loaded, &MODEL_SECTION, Some(usize::from(wire.vertex_count)))?;
    if geometry.parts.len() != usize::from(wire.part_count) {
        return Err(Error::Invalid("section part count"));
    }
    if geometry
        .parts
        .iter()
        .any(|p| p.material != u16::MAX && usize::from(p.material) >= ctx.material_count)
    {
        return Err(Error::Invalid("part material index"));
    }
    // Vertex node indices are not range-checked: first-person models
    // reference nodes beyond their own node list (a skeleton shared at run
    // time, presumably). `viewer --all` reports how many do.
    let node_map: Vec<u8> = geometry
        .extra_block(NODE_MAP, 1)?
        .into_iter()
        .map(|b| b[0])
        .collect();
    if node_map.iter().any(|&n| usize::from(n) >= ctx.node_count) {
        return Err(Error::Invalid("node map entry out of range"));
    }
    let flags = wire.compression_flags.0;
    let position = geometry
        .position_stream()
        .ok_or(Error::Invalid("section has no position stream"))?;
    if (position.kind != geometry::VertexKind::Position) != (flags & COMPRESSED_POSITIONS != 0) {
        return Err(Error::Invalid(
            "position stream disagrees with compression flags",
        ));
    }
    if let Some(t) = geometry.vertex_buffers.iter().find(|b| {
        matches!(
            b.kind,
            geometry::VertexKind::Texcoord | geometry::VertexKind::CompressedTexcoord
        )
    }) && (t.kind == geometry::VertexKind::CompressedTexcoord)
        != (flags & COMPRESSED_TEXCOORDS != 0)
    {
        return Err(Error::Invalid(
            "texcoord stream disagrees with compression flags",
        ));
    }
    let vertices = geometry.vertices(ctx.compression.as_ref())?;
    Ok(Section {
        wire,
        block,
        raw: pointer,
        geometry,
        node_map,
        vertices,
    })
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
