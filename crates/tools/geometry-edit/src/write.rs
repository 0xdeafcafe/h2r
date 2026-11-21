//! One section's in-place rewrite: the source block, the bytes of the
//! vertices an edit moved (and their re-derived normals and frames), and a
//! structure part's sphere and box, re-encoded with
//! `tags::geometry_write::encode_section` at the same size, so the block
//! differs from its source only in those bytes.

use map::RawPointer;
use mesh_edit::{Corner, dirty};
use tags::geometry::{
    Compression, GeometryBlockInfo, ResourceKind, Section, SectionLayout, VertexKind,
};
use tags::geometry_write::{
    self, SectionData, encode_section, pack_normpacked3, quantize, snorm16,
};

/// One vertex stream of the source section.
#[derive(Clone, Debug)]
pub(crate) struct Stream {
    pub slot: u16,
    pub kind: VertexKind,
    pub stride: usize,
}

/// What an in-place write of one section needs from its source.
#[derive(Clone, Debug)]
pub(crate) struct SectionSource {
    pub layout: SectionLayout,
    /// Where the raw block is, and its length.
    pub block: RawPointer,
    pub block_size: u32,
    /// The source block's bytes and its resources.
    pub bytes: Vec<u8>,
    pub data: SectionData,
    pub streams: Vec<Stream>,
    /// Decoded source positions (a box growth requantises them).
    pub positions: Vec<[f32; 3]>,
    /// Per part, the distinct vertices its indices name.
    pub part_vertices: Vec<Vec<u32>>,
    /// Whether parts carry a sphere and box this write keeps current
    /// (structure parts, `+0x10`, `+0x2c`, `+0x30`).
    pub part_bounds: bool,
}

impl SectionSource {
    pub fn new(
        s: &Section,
        info: &GeometryBlockInfo,
        layout: SectionLayout,
        positions: Vec<[f32; 3]>,
        part_bounds: bool,
    ) -> Option<Self> {
        let block = info.block?;
        let part_vertices = s
            .parts
            .iter()
            .map(|p| {
                let mut v: Vec<u32> = s.part_indices(p).iter().map(|&i| u32::from(i)).collect();
                v.sort_unstable();
                v.dedup();
                v
            })
            .collect();
        Some(Self {
            layout,
            block,
            block_size: info.block_size,
            bytes: s.block().bytes().to_vec(),
            data: SectionData::from_block(s.block()),
            streams: s
                .vertex_buffers
                .iter()
                .map(|b| Stream {
                    slot: b.slot as u16,
                    kind: b.kind,
                    stride: usize::from(b.stride),
                })
                .collect(),
            positions,
            part_vertices,
            part_bounds,
        })
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }
}

/// The bytes of `f`s, little-endian.
fn floats(f: &[f32]) -> Vec<u8> {
    f.iter().flat_map(|v| v.to_bits().to_le_bytes()).collect()
}

/// The section with `positions` and the dirty attributes of `corners`
/// (per source vertex) written into its streams. With `requantise`, every
/// compressed position is quantised again (the box changed). Refused, with
/// what could not be encoded, when an attribute has no in-place encoding.
pub(crate) fn rewrite(
    src: &SectionSource,
    positions: &[[f32; 3]],
    corners: Option<&[Corner]>,
    compression: Option<&Compression>,
    requantise: bool,
) -> Result<SectionData, String> {
    let n = src.vertex_count();
    if positions.len() != n || corners.is_some_and(|c| c.len() != n) {
        return Err("the edit changes the vertex count".into());
    }
    let dirt = |i: usize| corners.map_or(0, |c| c[i].dirty);
    if (0..n).any(|i| dirt(i) & dirty::SKIN != 0) {
        return Err("skin weights are not written in place".into());
    }
    let mut data = src.data.clone();
    let field = src.layout.vertex_buffers;
    for stream in &src.streams {
        let bytes = data
            .resource_mut(ResourceKind::VertexBuffer, field, stream.slot)
            .ok_or("a vertex stream lost its resource")?;
        if bytes.len() != n * stream.stride {
            return Err("a vertex stream disagrees with the vertex count".into());
        }
        for i in 0..n {
            let d = dirt(i);
            let row = &mut bytes[i * stream.stride..(i + 1) * stream.stride];
            let c = corners.map(|c| &c[i]);
            match stream.kind {
                VertexKind::Position if d & dirty::POSITION != 0 => {
                    row[..12].copy_from_slice(&floats(&positions[i]));
                }
                k if k.is_position() && k != VertexKind::Position => {
                    if d & dirty::POSITION == 0 && !requantise {
                        continue;
                    }
                    let b = compression
                        .ok_or("compressed positions without a box")?
                        .position;
                    for axis in 0..3 {
                        let q = quantize(positions[i][axis], b[2 * axis], b[2 * axis + 1])
                            .ok_or_else(|| format!("vertex {i} is outside the compression box"))?;
                        row[2 * axis..2 * axis + 2].copy_from_slice(&q.to_le_bytes());
                    }
                }
                VertexKind::TangentFrame => {
                    let Some(c) = c else { continue };
                    if d & dirty::NORMAL != 0 {
                        row[0..4].copy_from_slice(&pack_normpacked3(c.normal).to_le_bytes());
                    }
                    if d & dirty::FRAME != 0 {
                        row[4..8].copy_from_slice(&pack_normpacked3(c.binormal).to_le_bytes());
                        row[8..12].copy_from_slice(&pack_normpacked3(c.tangent).to_le_bytes());
                    }
                }
                VertexKind::Texcoord if d & dirty::UV != 0 => {
                    let c = c.expect("dirty corners exist");
                    row[..8].copy_from_slice(&floats(&c.uv));
                }
                VertexKind::CompressedTexcoord if d & dirty::UV != 0 => {
                    let c = c.expect("dirty corners exist");
                    let t = compression
                        .ok_or("compressed coordinates without a box")?
                        .texcoord;
                    for axis in 0..2 {
                        let q = quantize(c.uv[axis], t[2 * axis], t[2 * axis + 1]).ok_or_else(|| {
                            format!("vertex {i}'s texture coordinate is outside the model's coordinate box")
                        })?;
                        row[2 * axis..2 * axis + 2].copy_from_slice(&q.to_le_bytes());
                    }
                }
                VertexKind::LightmapTexcoord if d & dirty::LIGHTMAP != 0 => {
                    let c = c.expect("dirty corners exist");
                    row[0..2].copy_from_slice(&snorm16(c.lightmap[0]).to_le_bytes());
                    row[2..4].copy_from_slice(&snorm16(c.lightmap[1]).to_le_bytes());
                }
                VertexKind::Unknown(_) if d & dirty::LIGHT != 0 => {
                    return Err("the incident light direction is not written in place".into());
                }
                _ => {}
            }
        }
    }
    if src.part_bounds {
        write_part_bounds(src, &mut data, positions, corners)?;
    }
    Ok(data)
}

/// A touched part's box (`+0x30`) from its vertices, the sphere centre
/// (`+0x10`) at the box's centre (V: structure-bsp.md, "Part") and the
/// radius (`+0x2c`) to its farthest vertex (S: retail's radius rule is not
/// checked). A part with no moved vertex keeps its bytes.
fn write_part_bounds(
    src: &SectionSource,
    data: &mut SectionData,
    positions: &[[f32; 3]],
    corners: Option<&[Corner]>,
) -> Result<(), String> {
    let Some(field) = src.layout.parts else {
        return Ok(());
    };
    let stride = usize::from(src.layout.part_stride);
    let moved = |i: u32| corners.is_some_and(|c| c[i as usize].dirty & dirty::POSITION != 0);
    let parts = data
        .resource_mut(ResourceKind::Block, field, 0)
        .ok_or("the parts lost their resource")?;
    for (pi, verts) in src.part_vertices.iter().enumerate() {
        if verts.is_empty() || !verts.iter().any(|&v| moved(v)) {
            continue;
        }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for &v in verts {
            let p = positions[v as usize];
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) * 0.5);
        let radius = verts
            .iter()
            .map(|&v| {
                let p = positions[v as usize];
                ((p[0] - centre[0]).powi(2)
                    + (p[1] - centre[1]).powi(2)
                    + (p[2] - centre[2]).powi(2))
                .sqrt()
            })
            .fold(0.0f32, f32::max);
        let part = parts
            .get_mut(pi * stride..(pi + 1) * stride)
            .ok_or("a part lies outside its block")?;
        let boxed = [lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]];
        let old = &part[0x30..0x48];
        if floats(&boxed) == old {
            continue;
        }
        part[0x10..0x1c].copy_from_slice(&floats(&centre));
        // A radius of 0 is replaced by a constant in the engine
        // (`0x00045da1`): a part is never written with one.
        part[0x2c..0x30].copy_from_slice(&floats(&[radius.max(f32::MIN_POSITIVE)]));
        part[0x30..0x48].copy_from_slice(&floats(&boxed));
    }
    Ok(())
}

/// The block for `data` and the runs where it differs from the source:
/// `(offset in the block, bytes)`. Refused when the encoding is not the
/// source's size.
pub(crate) fn block_runs(
    src: &SectionSource,
    data: &SectionData,
) -> Result<Vec<(usize, Vec<u8>)>, String> {
    let e = encode_section(&src.layout, data).map_err(|e| e.to_string())?;
    if e.block.len() != src.bytes.len() {
        return Err("the section no longer fits its block".into());
    }
    Ok(geometry_write::changed_runs(&src.bytes, &e.block))
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
