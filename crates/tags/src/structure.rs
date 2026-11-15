//! Structure BSP (`sbsp`) render geometry and lightmap (`ltmp`) linkage.
//!
//! Reads clusters, instanced geometry definitions and placements from a BSP,
//! and lightmap groups (per-cluster/per-instance bitmap and palette indices,
//! palettes, geometry buckets) from its lightmap. Section loading is the
//! shared decoder in [`crate::geometry`].
//!
//! Hand-written reader; every offset's provenance (engine instruction address
//! or disc check) is in `docs/structure-bsp.md`. Unlisted bytes are preserved
//! in `raw` fields, not interpreted.

use crate::geometry::{
    GeometryBlockInfo, RawSource, Section, SectionLayout, TagView, decode_section, f32_at,
    load_block, u16_at, u32_at,
};
use crate::{Error, Result};
use map::{FourCC, Map, TagId};
use std::io::{Read, Seek};

/// Section struct of clusters and instanced definitions (0x44 bytes).
pub const STRUCTURE_SECTION: SectionLayout = SectionLayout {
    section_size: 0x44,
    parts: Some(0),
    part_stride: 0x48,
    subparts: Some(8),
    subpart_stride: 8,
    subparts_tile_indices: true,
    indices: Some(0x20),
    vertex_buffers: 0x38,
    descriptor_stride: 0x20,
};
/// Lightmap bucket section struct (8 bytes): vertex streams only.
pub const BUCKET_SECTION: SectionLayout = SectionLayout {
    section_size: 8,
    parts: None,
    part_stride: 0x48,
    subparts: None,
    subpart_stride: 8,
    subparts_tile_indices: true,
    indices: None,
    vertex_buffers: 0,
    descriptor_stride: 0x20,
};

/// The 0x28-byte section info preceding block info in clusters, instanced
/// definitions and lightmap geometry. Only counts checked against loaded data
/// are named; the rest is preserved.
#[derive(Clone, Debug)]
pub struct SectionInfo {
    /// Equals the vertex count of every vertex stream (V).
    pub total_vertex_count: u16,
    /// Equals index count / 3 (V).
    pub total_triangle_count: u16,
    /// Equals the loaded part count (V).
    pub total_part_count: u16,
    /// u16 at +0xc. No engine use found. V: at most `total_vertex_count`, and
    /// every instance's lightmap bucket range fits when sized by this count.
    pub unknown_0c: u16,
    /// All 0x28 bytes, for round trips and future fields.
    pub raw: [u8; SectionInfo::SIZE],
}
impl SectionInfo {
    /// Fixed record size.
    pub const SIZE: usize = 0x28;
    /// Decode exactly [`Self::SIZE`] bytes.
    pub fn decode(b: &[u8]) -> Result<Self> {
        let raw: [u8; Self::SIZE] = b.try_into().map_err(|_| Error::RecordSize {
            actual: b.len(),
            expected: Self::SIZE,
        })?;
        Ok(Self {
            total_vertex_count: u16_at(&raw, 0),
            total_triangle_count: u16_at(&raw, 2),
            total_part_count: u16_at(&raw, 4),
            unknown_0c: u16_at(&raw, 0xc),
            raw,
        })
    }
}

/// Geometry shared by clusters, instanced definitions and lightmap geometry:
/// section info, block info, then the runtime section block (8 bytes).
#[derive(Clone, Debug)]
pub struct GeometryHeader {
    /// Counts used to validate loaded data.
    pub info: SectionInfo,
    /// Where the raw block is and how to patch it.
    pub block: GeometryBlockInfo,
    /// The 8-byte block after block info is the engine's runtime pointer to
    /// the loaded section and is empty on disc, except for lightmap geometry
    /// without a raw block, where it points at a section struct inside the
    /// tag. Such sections are range-checked, not decoded.
    pub embedded_section: Option<crate::wire::TagBlock>,
}
impl GeometryHeader {
    /// Bytes covered: 0x28 info + 0x24 block info + 8 runtime block.
    pub const SIZE: usize = 0x54;
    /// Decode and check the block's owner.
    fn decode(view: &TagView<'_>, record: &[u8], owner: TagId) -> Result<Self> {
        let info = SectionInfo::decode(&record[..SectionInfo::SIZE])?;
        let block = GeometryBlockInfo::decode(view, record, SectionInfo::SIZE)?;
        block.check_owner(owner)?;
        if block.block.is_some() && block.runtime_block_offset as usize != GeometryBlockInfo::SIZE {
            return Err(Error::Invalid(
                "geometry block info runtime offset is not 0x24",
            ));
        }
        let embedded = crate::wire::TagBlock {
            count: u32_at(record, 0x4c),
            address: u32_at(record, 0x50),
        };
        let embedded_section = if embedded.count == 0 {
            None
        } else if block.block.is_none() && embedded.count == 1 {
            // Seen only in lightmap geometry: the section struct lives in tag
            // memory instead of a raw block. Bounds-checked, not interpreted.
            view.range(embedded.address, u64::from(block.section_size))?;
            Some(embedded)
        } else {
            return Err(Error::Invalid("runtime section block is populated on disc"));
        };
        // V: without a raw block, only in-tag sections keep resource entries.
        if block.block.is_none() && embedded_section.is_none() && !block.resources.is_empty() {
            return Err(Error::Invalid("absent geometry block has resources"));
        }
        Ok(Self {
            info,
            block,
            embedded_section,
        })
    }
    /// Load and decode the section; `Ok(None)` if there is no raw block.
    /// Checks the section info counts against the loaded data, and part
    /// materials against `material_count` when the owner has materials.
    pub fn load(
        &self,
        raw: &mut dyn RawSource,
        material_count: Option<usize>,
    ) -> Result<Option<Section>> {
        let Some(block) = load_block(&self.block, raw)? else {
            return Ok(None);
        };
        let s = decode_section(
            block,
            &STRUCTURE_SECTION,
            Some(usize::from(self.info.total_vertex_count)),
        )?;
        if s.indices.len() != 3 * usize::from(self.info.total_triangle_count)
            || s.parts.len() != usize::from(self.info.total_part_count)
        {
            return Err(Error::Invalid(
                "section info counts disagree with loaded section",
            ));
        }
        if let Some(n) = material_count
            && s.parts
                .iter()
                .any(|p| p.material != u16::MAX && usize::from(p.material) >= n)
        {
            return Err(Error::Invalid("part material outside the material block"));
        }
        Ok(Some(s))
    }
}

/// One BSP cluster (0xb0 bytes).
#[derive(Clone, Debug)]
pub struct Cluster {
    /// Render geometry.
    pub geometry: GeometryHeader,
    /// Axis bounds at +0x54: x min, x max, y min, y max, z min, z max.
    pub bounds: [f32; 6],
    /// All 0xb0 bytes.
    pub raw: Vec<u8>,
}
impl Cluster {
    /// Element stride (engine `imul 0B0h` at 0x000410ac).
    pub const SIZE: usize = 0xb0;
}

/// One instanced geometry definition (0xc8 bytes).
#[derive(Clone, Debug)]
pub struct InstancedGeometryDefinition {
    /// Render geometry, in definition-local space.
    pub geometry: GeometryHeader,
    /// All 0xc8 bytes.
    pub raw: Vec<u8>,
}
impl InstancedGeometryDefinition {
    /// Element stride (engine `imul 0C8h` at 0x000410d0).
    pub const SIZE: usize = 0xc8;
}

/// One placement of a definition (0x58 bytes).
#[derive(Clone, Debug)]
pub struct InstancedGeometryInstance {
    /// f32 at +0: uniform scale. V: positive on every placement.
    pub scale: f32,
    /// Rows at +4/+0x10/+0x1c: forward, left, up. V: orthonormal and
    /// right-handed on every placement.
    pub basis: [[f32; 3]; 3],
    /// Translation at +0x28 (V: rendered placements sit on their host).
    pub position: [f32; 3],
    /// i16 at +0x34 (engine `movsx` at 0x000410c5).
    pub definition: i16,
    /// All 0x58 bytes.
    pub raw: Vec<u8>,
}
impl InstancedGeometryInstance {
    /// Element stride (engine `imul 58h` at 0x000410c2).
    pub const SIZE: usize = 0x58;
    /// Local to world: `position + scale * (x*forward + y*left + z*up)`.
    pub fn transform(&self, p: [f32; 3]) -> [f32; 3] {
        let mut out = self.position;
        for (axis, row) in self.basis.iter().enumerate() {
            for k in 0..3 {
                out[k] += self.scale * p[axis] * row[k];
            }
        }
        out
    }
    /// Rotate (without scale/translation), for normals.
    pub fn rotate(&self, n: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0; 3];
        for (axis, row) in self.basis.iter().enumerate() {
            for k in 0..3 {
                out[k] += n[axis] * row[k];
            }
        }
        out
    }
    /// Whether the basis is orthonormal and right-handed (tolerance 1e-3).
    pub fn is_rigid(&self) -> bool {
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let [f, l, u] = self.basis;
        let cross = [
            f[1] * l[2] - f[2] * l[1],
            f[2] * l[0] - f[0] * l[2],
            f[0] * l[1] - f[1] * l[0],
        ];
        (0..3).all(|i| (dot(self.basis[i], self.basis[i]) - 1.0).abs() < 1e-3)
            && [dot(f, l), dot(f, u), dot(l, u)]
                .iter()
                .all(|d| d.abs() < 1e-3)
            && (dot(cross, u) - 1.0).abs() < 1e-3
    }
}

/// Structure BSP render data.
#[derive(Clone, Debug)]
pub struct StructureRender {
    /// The BSP tag.
    pub tag: TagId,
    /// u32 at +8; lightmap groups store the same value.
    pub checksum: u32,
    /// Count of the material block at +0xa4 (elements not decoded). V: every
    /// part's material index is below it.
    pub material_count: u32,
    /// Block at +0x9c.
    pub clusters: Vec<Cluster>,
    /// Block at +0x138.
    pub instance_definitions: Vec<InstancedGeometryDefinition>,
    /// Block at +0x140.
    pub instances: Vec<InstancedGeometryInstance>,
}

fn read_payload<R: Read + Seek>(
    map: &mut Map<R>,
    id: TagId,
    group: FourCC,
) -> Result<(Vec<u8>, u32)> {
    let tag = map.tag(id)?.clone();
    if tag.group != group {
        return Err(Error::Invalid("tag has the wrong group for this reader"));
    }
    let location = tag.location.ok_or(map::Error::NoTagData)?;
    Ok((map.read_tag_bytes(id)?, location.address))
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

impl StructureRender {
    /// Header bytes this reader needs.
    const HEADER: usize = 0x148;
    /// Read the render-related blocks of a BSP tag.
    pub fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Self> {
        let (bytes, address) = read_payload(map, id, FourCC(*b"sbsp"))?;
        Self::decode(id, &TagView::new(&bytes, address))
    }
    /// Decode from a payload view.
    pub fn decode(tag: TagId, view: &TagView<'_>) -> Result<Self> {
        let h = view.at(0, Self::HEADER)?;
        let clusters = view
            .block(h, 0x9c, Cluster::SIZE)?
            .into_iter()
            .map(|b| {
                let bounds: [f32; 6] = std::array::from_fn(|i| f32_at(b, 0x54 + 4 * i));
                if !finite(&bounds) {
                    return Err(Error::Invalid("non-finite cluster bounds"));
                }
                Ok(Cluster {
                    geometry: GeometryHeader::decode(view, b, tag)?,
                    bounds,
                    raw: b.to_vec(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let instance_definitions = view
            .block(h, 0x138, InstancedGeometryDefinition::SIZE)?
            .into_iter()
            .map(|b| {
                Ok(InstancedGeometryDefinition {
                    geometry: GeometryHeader::decode(view, b, tag)?,
                    raw: b.to_vec(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let instances = view
            .block(h, 0x140, InstancedGeometryInstance::SIZE)?
            .into_iter()
            .map(|b| {
                let v = |o| [f32_at(b, o), f32_at(b, o + 4), f32_at(b, o + 8)];
                let i = InstancedGeometryInstance {
                    scale: f32_at(b, 0),
                    basis: [v(4), v(0x10), v(0x1c)],
                    position: v(0x28),
                    definition: u16_at(b, 0x34) as i16,
                    raw: b.to_vec(),
                };
                if !finite(&[i.scale]) || !finite(i.basis.as_flattened()) || !finite(&i.position) {
                    return Err(Error::Invalid("non-finite instance placement"));
                }
                if i.definition < 0 || i.definition as usize >= instance_definitions.len() {
                    return Err(Error::Invalid("instance refers to a missing definition"));
                }
                Ok(i)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            tag,
            checksum: u32_at(h, 8),
            material_count: u32_at(h, 0xa4),
            clusters,
            instance_definitions,
            instances,
        })
    }
}

/// Lightmap bitmap selection for one cluster or instance (4 bytes).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LightmapRenderInfo {
    /// i16 at +0: bitmap image index in the group's lightmap bitmap; -1 = none.
    pub bitmap_index: i16,
    /// i8 at +2: palette index; -1 = none (engine `movsx byte` at 0x0004d3d4).
    pub palette_index: i8,
    /// Byte at +3.
    pub unknown_3: u8,
}

/// Bucket flag bit 0: the engine patches the bucket's first vertex
/// descriptor with a bucket reference's offset only when it is set (E
/// `0x0004d326`, `0x0004d396`); otherwise no lightmap stream is bound.
pub const BUCKET_FLAG_PATCHED: u16 = 1;

/// A geometry bucket (0x38 bytes): extra per-vertex lightmap streams.
#[derive(Clone, Debug)]
pub struct LightmapBucket {
    /// u16 at +0. Bit 0: [`BUCKET_FLAG_PATCHED`]. Bit 1 (V): set exactly on
    /// the buckets that carry a type-0x2f stream.
    pub flags: u16,
    /// Block info at +0xc.
    pub block: GeometryBlockInfo,
}
impl LightmapBucket {
    /// Element stride (engine `imul 38h` at 0x0004d389).
    pub const SIZE: usize = 0x38;
    /// See [`BUCKET_FLAG_PATCHED`].
    pub fn is_patched(&self) -> bool {
        self.flags & BUCKET_FLAG_PATCHED != 0
    }
}

/// Where one cluster's or instance's vertices sit in a bucket (0xc bytes).
#[derive(Clone, Debug)]
pub struct BucketReference {
    /// u16 at +0 (V: always 0; no engine use found).
    pub unknown_0: u16,
    /// u16 at +2: bucket index (E `0x0004d382`, `0x0004d315`).
    pub bucket: u16,
    /// u16 block at +4: first-vertex offsets into the bucket's streams. The
    /// engine reads the first entry for instances (E `0x0004d386`) and the
    /// entry selected by a 9-bit render-key field for clusters
    /// (E `0x0004d309`..`0x0004d311`), then multiplies it by 4. It never
    /// tests for `0xffff`; `None` (0xffff) is a disc observation only (V:
    /// only on references whose geometry has no lightmap vertices).
    pub section_offsets: Vec<Option<u16>>,
}

/// One lightmap group (0x68 bytes).
#[derive(Clone, Debug)]
pub struct LightmapGroup {
    /// u32 at +4; equals the BSP checksum it lights.
    pub structure_checksum: u32,
    /// 256-entry 32-bit palettes, block at +8 (0x400-byte elements).
    pub palettes: Vec<[u32; 256]>,
    /// Bitmap reference at +0x18 (`bitm`).
    pub bitmap: Option<TagId>,
    /// Block at +0x20.
    pub cluster_geometry: Vec<GeometryHeader>,
    /// Block at +0x28, one per BSP cluster.
    pub cluster_render_info: Vec<LightmapRenderInfo>,
    /// Block at +0x30, one per instanced definition.
    pub instance_geometry: Vec<GeometryHeader>,
    /// Block at +0x40.
    pub buckets: Vec<LightmapBucket>,
    /// Block at +0x48, one per BSP instance.
    pub instance_render_info: Vec<LightmapRenderInfo>,
    /// Block at +0x50, one per BSP instance.
    pub instance_bucket_refs: Vec<BucketReference>,
    /// Block at +0x60.
    pub cluster_bucket_refs: Vec<BucketReference>,
    /// Counts of the blocks at +0x10, +0x38 and +0x58 (not followed here;
    /// +0x38 and +0x58 are nonempty in some disc maps). `lighting` decodes
    /// +0x38 as light probes through the same budgeted view.
    pub unsupported_counts: [u32; 3],
    /// All 0x68 bytes.
    pub raw: Vec<u8>,
}
impl LightmapGroup {
    /// Element stride. E: the engine's per-group loop advances its group
    /// offset with `add esi, 68h` at 0x00015c6b and compares the iteration
    /// against `[ltmp+80h]`; the inner palette loop steps `0x400` at
    /// 0x00015c51.
    pub const SIZE: usize = 0x68;
}

/// Lightmap tag render linkage.
#[derive(Clone, Debug)]
pub struct Lightmap {
    /// The lightmap tag.
    pub tag: TagId,
    /// Block at +0x80 (engine reads the first element via `+84h`).
    pub groups: Vec<LightmapGroup>,
}

impl Lightmap {
    const HEADER: usize = 0x104;
    /// Read a lightmap tag.
    pub fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Self> {
        let (bytes, address) = read_payload(map, id, FourCC(*b"ltmp"))?;
        Self::decode(id, &TagView::new(&bytes, address))
    }
    /// Decode from a payload view.
    pub fn decode(tag: TagId, view: &TagView<'_>) -> Result<Self> {
        let h = view.at(0, Self::HEADER)?;
        let groups = view
            .block(h, 0x80, LightmapGroup::SIZE)?
            .into_iter()
            .map(|g| Self::group(tag, view, g))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { tag, groups })
    }
    fn group(tag: TagId, view: &TagView<'_>, g: &[u8]) -> Result<LightmapGroup> {
        let render_info = |field| -> Result<Vec<LightmapRenderInfo>> {
            Ok(view
                .block(g, field, 4)?
                .into_iter()
                .map(|b| LightmapRenderInfo {
                    bitmap_index: u16_at(b, 0) as i16,
                    palette_index: b[2] as i8,
                    unknown_3: b[3],
                })
                .collect())
        };
        let geometry = |field| -> Result<Vec<GeometryHeader>> {
            view.block(g, field, GeometryHeader::SIZE)?
                .into_iter()
                .map(|b| GeometryHeader::decode(view, b, tag))
                .collect()
        };
        let bucket_refs = |field| -> Result<Vec<BucketReference>> {
            view.block(g, field, 0xc)?
                .into_iter()
                .map(|b| {
                    let section_offsets: Vec<Option<u16>> = view
                        .block(b, 4, 2)?
                        .into_iter()
                        .map(|o| Some(u16_at(o, 0)).filter(|&v| v != u16::MAX))
                        .collect();
                    // The engine reads an entry unconditionally.
                    if section_offsets.is_empty() {
                        return Err(Error::Invalid("bucket reference has no offsets"));
                    }
                    Ok(BucketReference {
                        unknown_0: u16_at(b, 0),
                        bucket: u16_at(b, 2),
                        section_offsets,
                    })
                })
                .collect()
        };
        let palettes = view
            .block(g, 8, 0x400)?
            .into_iter()
            .map(|b| std::array::from_fn(|i| u32_at(b, 4 * i)))
            .collect();
        let buckets = view
            .block(g, 0x40, LightmapBucket::SIZE)?
            .into_iter()
            .map(|b| {
                let block = GeometryBlockInfo::decode(view, b, 0xc)?;
                block.check_owner(tag)?;
                if block.block.is_some() && block.runtime_block_offset != 0x24 {
                    return Err(Error::Invalid("lightmap bucket runtime offset is not 0x24"));
                }
                // V: the runtime section block at +0x30 is empty on disc.
                if u32_at(b, 0x30) != 0 {
                    return Err(Error::Invalid(
                        "lightmap bucket runtime block is populated on disc",
                    ));
                }
                Ok(LightmapBucket {
                    flags: u16_at(b, 0),
                    block,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let bitmap_group = FourCC::from_u32(u32_at(g, 0x18));
        let bitmap_id = u32_at(g, 0x1c);
        let bitmap = if bitmap_id == u32::MAX {
            None
        } else if bitmap_group == FourCC(*b"bitm") {
            Some(TagId(bitmap_id))
        } else {
            return Err(Error::Invalid("lightmap bitmap reference is not a bitmap"));
        };
        let group = LightmapGroup {
            structure_checksum: u32_at(g, 4),
            palettes,
            bitmap,
            cluster_geometry: geometry(0x20)?,
            cluster_render_info: render_info(0x28)?,
            instance_geometry: geometry(0x30)?,
            buckets,
            instance_render_info: render_info(0x48)?,
            instance_bucket_refs: bucket_refs(0x50)?,
            cluster_bucket_refs: bucket_refs(0x60)?,
            unsupported_counts: [u32_at(g, 0x10), u32_at(g, 0x38), u32_at(g, 0x58)],
            raw: g.to_vec(),
        };
        for r in group
            .instance_bucket_refs
            .iter()
            .chain(&group.cluster_bucket_refs)
        {
            if usize::from(r.bucket) >= group.buckets.len() {
                return Err(Error::Invalid("bucket reference outside bucket block"));
            }
        }
        Ok(group)
    }
}

/// Load a lightmap bucket's section. Bucket sections hold only vertex
/// streams: an 8-byte struct whose single block field (offset 0) carries the
/// descriptors.
pub fn load_bucket(bucket: &LightmapBucket, raw: &mut dyn RawSource) -> Result<Option<Section>> {
    let Some(block) = load_block(&bucket.block, raw)? else {
        return Ok(None);
    };
    decode_section(block, &BUCKET_SECTION, None).map(Some)
}
