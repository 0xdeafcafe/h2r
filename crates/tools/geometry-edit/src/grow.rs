//! One section's **grown** rewrite (docs/editor/modeling-targets.md,
//! "Write, grown"): an export whose vertex, index or part counts differ
//! from its source, encoded as a whole new section with
//! `tags::geometry_write::encode_section`.
//!
//! - Parts keep their source order, kind, material and every byte the
//!   writer does not set; a part left with no face is dropped. Each part is
//!   a triangle list (flag `0x8`) with one subpart, as
//!   `h1convert::structure::raw_block` and `h1_assets::models::mode` write
//!   them. A strip part of a sky's model is refused: the sky draw draws
//!   every part as a strip (docs/halo1/sky.md, "Strips").
//! - A source vertex keeps its stream bytes but for the attributes an edit
//!   made dirty; a new vertex starts from the bytes of a source vertex at
//!   the same mesh vertex (so streams the writer does not interpret, the
//!   incident light direction among them, carry over), else zeros, and
//!   then every attribute is encoded. A skin with more influences than the
//!   stream holds keeps the largest, renormalised to 255.
//! - A structure part whose vertices changed gets its sphere and box again;
//!   a structure section that has a visibility code gets a new code and
//!   table over the parts' boxes ([`crate::visibility`]), checked as the
//!   engine reads them.
//! - Every resource the writer does not rebuild is carried as the source's
//!   bytes.

use std::collections::HashMap;

use mesh_edit::{Export, Skin, dirty};
use tags::geometry::{Compression, PART_FLAG_TRIANGLE_LIST, ResourceKind, VertexKind};
use tags::geometry_write::{
    EncodedSection, SectionData, encode_section, pack_normpacked3, quantize, snorm16,
};

use crate::visibility;
use crate::write::{SectionSource, Stream};

/// Most vertices, or indices, a section holds (`u16` counts).
pub(crate) const SECTION_MAX: usize = 0xffff;
/// Part kind of opaque structure parts, and of decals (docs/formats/structure-bsp.md).
const PART_KIND_OPAQUE: u16 = 2;
const PART_KIND_DECAL: u16 = 3;

/// How to grow one section.
pub(crate) struct Options<'a> {
    /// The model's compression box (possibly grown).
    pub compression: Option<&'a Compression>,
    /// Every compressed position is quantised again (the box changed).
    pub requantise: bool,
    /// The section is a sky model's: strip parts are refused.
    pub sky: bool,
    /// Rebuild the visibility code and table when the section has them.
    pub visibility: bool,
}

/// One written part, for the owner's section counts.
#[derive(Clone, Debug)]
pub(crate) struct PartInfo {
    pub kind: u16,
    pub triangles: usize,
    /// The distinct vertices it indexes, sorted.
    pub vertices: Vec<u32>,
}

/// A grown section, encoded.
pub(crate) struct Grown {
    pub data: SectionData,
    pub encoded: EncodedSection,
    pub vertices: usize,
    pub indices: usize,
    pub parts: Vec<PartInfo>,
}

impl Grown {
    /// Whether the encoding has the source block's size and layout, so it
    /// can be written over the old block (fixed patches only).
    pub fn fits(&self, src: &SectionSource) -> bool {
        self.data.keeps_sizes() && self.encoded.block.len() == src.bytes.len()
    }

    /// The structure section info words a grown section rewrites (offset
    /// in the 0x28-byte info, value), filled as
    /// `h1convert::structure::Structure::write` fills them: vertices,
    /// triangles, parts, shadow-casting (opaque) triangles and parts,
    /// opaque vertices, opaque-and-decal parts, shadow-casting rigid
    /// triangles and subparts (one per part).
    pub fn structure_info(&self) -> [(usize, u16); 9] {
        let parts = self.parts.len() as u16;
        let opaque = |p: &&PartInfo| p.kind == PART_KIND_OPAQUE;
        let lit = |p: &&PartInfo| p.kind == PART_KIND_OPAQUE || p.kind == PART_KIND_DECAL;
        let shadow_triangles: usize = self.parts.iter().filter(opaque).map(|p| p.triangles).sum();
        let shadow_parts = self.parts.iter().filter(opaque).count() as u16;
        let opaque_parts = self.parts.iter().filter(lit).count() as u16;
        let opaque_vertices = if opaque_parts == parts {
            self.vertices as u16
        } else {
            let mut v: Vec<u32> = self
                .parts
                .iter()
                .filter(lit)
                .flat_map(|p| p.vertices.iter().copied())
                .collect();
            v.sort_unstable();
            v.dedup();
            v.len() as u16
        };
        [
            (0, self.vertices as u16),
            (2, (self.indices / 3) as u16),
            (4, parts),
            (6, shadow_triangles as u16),
            (8, shadow_parts),
            (0xc, opaque_vertices),
            (0xe, opaque_parts),
            (0x12, shadow_triangles as u16),
            (0x24, parts),
        ]
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn floats(f: &[f32]) -> Vec<u8> {
    f.iter().flat_map(|v| v.to_bits().to_le_bytes()).collect()
}

/// The section for export `e` of source `src`. Refused, naming why, when
/// the format cannot hold it.
pub(crate) fn grow(src: &SectionSource, e: &Export, o: &Options<'_>) -> Result<Grown, String> {
    let n = e.positions.len();
    if n > SECTION_MAX {
        return Err(format!(
            "{n} vertices: a section holds at most 65,535"
        ));
    }
    let layout = &src.layout;
    let (Some(parts_field), Some(subparts_field), Some(indices_field)) =
        (layout.parts, layout.subparts, layout.indices)
    else {
        return Err("the section has no parts, subparts or indices to grow".into());
    };
    let part_stride = usize::from(layout.part_stride);
    let subpart_stride = usize::from(layout.subpart_stride);
    let old_parts: Vec<&[u8]> = src
        .data
        .resource(ResourceKind::Block, parts_field, 0)
        .unwrap_or(&[])
        .chunks_exact(part_stride)
        .collect();
    let old_subparts: Vec<&[u8]> = src
        .data
        .resource(ResourceKind::Block, subparts_field, 0)
        .unwrap_or(&[])
        .chunks_exact(subpart_stride)
        .collect();

    let mut by_part: Vec<Vec<[u32; 3]>> = vec![Vec::new(); old_parts.len()];
    for (t, tri) in e.triangles.iter().enumerate() {
        let g = usize::from(e.groups.get(t).copied().unwrap_or(0));
        let part = old_parts.get(g).ok_or_else(|| {
            format!("a face is in part {g}, which the section does not have: new parts are not written")
        })?;
        if e.materials.get(t).copied().unwrap_or(0) != u16_at(part, 4) {
            return Err(
                "a face's material changed: materials are per part, and new parts are not written"
                    .into(),
            );
        }
        by_part[g].push(*tri);
    }

    let moved = |v: u32| e.corners[v as usize].dirty & dirty::POSITION != 0;
    let mut parts = Vec::new();
    let mut subparts = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    let mut boxes = Vec::new();
    let mut infos = Vec::new();
    for (g, tris) in by_part.iter().enumerate() {
        if tris.is_empty() {
            continue;
        }
        let old = old_parts[g];
        let flags = u16_at(old, 2);
        if flags & PART_FLAG_TRIANGLE_LIST == 0 && o.sky {
            return Err(
                "a sky's model draws every part as a strip, and a grown part is a list".into(),
            );
        }
        let first = indices.len();
        indices.extend(tris.iter().flatten().map(|&i| i as u16));
        if indices.len() > SECTION_MAX {
            return Err(format!(
                "{} indices: a section holds at most 65,535",
                indices.len()
            ));
        }
        let count = indices.len() - first;
        let k = parts.len() as u16;
        let mut p = old.to_vec();
        put16(&mut p, 2, flags | PART_FLAG_TRIANGLE_LIST);
        put16(&mut p, 6, first as u16);
        put16(&mut p, 8, count as u16);
        put16(&mut p, 0xa, k);
        put16(&mut p, 0xc, 1);
        let mut verts: Vec<u32> = tris.iter().flatten().copied().collect();
        verts.sort_unstable();
        verts.dedup();
        let b = bounds(verts.iter().map(|&v| e.positions[v as usize]));
        if src.part_bounds {
            let same = src.part_vertices.get(g) == Some(&verts) && !verts.iter().any(|&v| moved(v));
            if !same {
                write_bounds(&mut p, b, verts.iter().map(|&v| e.positions[v as usize]));
            }
        }
        let (old_first, old_count) = (usize::from(u16_at(old, 0xa)), u16_at(old, 0xc));
        let mut s = match old_subparts.get(old_first) {
            Some(s) if old_count > 0 => s.to_vec(),
            _ => {
                let mut s = vec![0u8; subpart_stride];
                put16(&mut s, 4, 0xffff);
                s
            }
        };
        put16(&mut s, 0, first as u16);
        put16(&mut s, 2, count as u16);
        put16(&mut s, 6, k);
        infos.push(PartInfo {
            kind: u16_at(&p, 0),
            triangles: count / 3,
            vertices: verts,
        });
        parts.push(p);
        subparts.push(s);
        boxes.push(b);
    }
    if parts.is_empty() {
        return Err("the edit leaves the section with no faces".into());
    }

    let mut data = src.data.clone();
    set(&mut data, ResourceKind::Block, parts_field, 0, parts.concat())?;
    set(&mut data, ResourceKind::Block, subparts_field, 0, subparts.concat())?;
    set(
        &mut data,
        ResourceKind::Block,
        indices_field,
        0,
        indices.iter().flat_map(|i| i.to_le_bytes()).collect(),
    )?;
    let templates = templates(e, src.vertex_count());
    for stream in &src.streams {
        let old = src
            .data
            .resource(ResourceKind::VertexBuffer, layout.vertex_buffers, stream.slot)
            .ok_or("a vertex stream lost its resource")?;
        if old.len() != src.vertex_count() * stream.stride {
            return Err("a vertex stream disagrees with the vertex count".into());
        }
        let mut rows = Vec::with_capacity(n * stream.stride);
        for i in 0..n {
            rows.extend(row(src, stream, old, e, i, &templates, o)?);
        }
        set(
            &mut data,
            ResourceKind::VertexBuffer,
            layout.vertex_buffers,
            stream.slot,
            rows,
        )?;
    }
    let has_visibility = o.visibility
        && data
            .resource(ResourceKind::Data, visibility::CODE_FIELD, 0)
            .is_some()
        && data
            .resource(ResourceKind::Block, visibility::TABLE_FIELD, 0)
            .is_some();
    if has_visibility {
        let (code, table) = visibility::build(&boxes);
        visibility::check(boxes.len(), &code, &table)?;
        set(&mut data, ResourceKind::Data, visibility::CODE_FIELD, 0, code)?;
        set(&mut data, ResourceKind::Block, visibility::TABLE_FIELD, 0, table)?;
    }
    let encoded = encode_section(layout, &data).map_err(|e| e.to_string())?;
    Ok(Grown {
        data,
        encoded,
        vertices: n,
        indices: indices.len(),
        parts: infos,
    })
}

/// Replace the bytes of a present resource.
fn set(
    data: &mut SectionData,
    kind: ResourceKind,
    field: u16,
    index: u16,
    bytes: Vec<u8>,
) -> Result<(), String> {
    *data
        .resource_mut(kind, field, index)
        .ok_or_else(|| format!("the section has no resource at {field:#x} to grow"))? = bytes;
    Ok(())
}

/// Per mesh vertex, the first source vertex made from it: a new vertex's
/// template.
fn templates(e: &Export, n_src: usize) -> HashMap<u32, usize> {
    let mut out = HashMap::new();
    for (i, v) in e.vert_ids.iter().enumerate().take(n_src) {
        if let Some(v) = v {
            out.entry(v.0).or_insert(i);
        }
    }
    out
}

/// `[x min, x max, y min, y max, z min, z max]` of `points`.
fn bounds(points: impl Iterator<Item = [f32; 3]>) -> [f32; 6] {
    let mut b = [f32::INFINITY, f32::NEG_INFINITY].repeat(3);
    for p in points {
        for k in 0..3 {
            b[2 * k] = b[2 * k].min(p[k]);
            b[2 * k + 1] = b[2 * k + 1].max(p[k]);
        }
    }
    b.try_into().expect("6")
}

/// A structure part's sphere centre (`+0x10`) at its box's centre, the
/// radius (`+0x2c`) to its farthest vertex, and the box (`+0x30`), as the
/// in-place write sets them.
fn write_bounds(part: &mut [u8], b: [f32; 6], points: impl Iterator<Item = [f32; 3]>) {
    let centre = [0, 1, 2].map(|k| (b[2 * k] + b[2 * k + 1]) * 0.5);
    let radius = points
        .map(|p| {
            ((p[0] - centre[0]).powi(2) + (p[1] - centre[1]).powi(2) + (p[2] - centre[2]).powi(2))
                .sqrt()
        })
        .fold(0.0f32, f32::max);
    part[0x10..0x1c].copy_from_slice(&floats(&centre));
    part[0x2c..0x30].copy_from_slice(&floats(&[radius.max(f32::MIN_POSITIVE)]));
    part[0x30..0x48].copy_from_slice(&floats(&b));
}

/// Vertex `i`'s bytes in `stream`.
fn row(
    src: &SectionSource,
    stream: &Stream,
    old: &[u8],
    e: &Export,
    i: usize,
    templates: &HashMap<u32, usize>,
    o: &Options<'_>,
) -> Result<Vec<u8>, String> {
    let st = stream.stride;
    let n_src = src.vertex_count();
    let (base, d) = if i < n_src {
        (Some(i), e.corners[i].dirty)
    } else {
        let t = e.vert_ids[i].and_then(|v| templates.get(&v.0).copied());
        (t, dirty::ALL)
    };
    let mut row = match base {
        Some(j) => old[j * st..(j + 1) * st].to_vec(),
        None => vec![0; st],
    };
    let c = &e.corners[i];
    let p = e.positions[i];
    match stream.kind {
        VertexKind::Position => {
            if d & dirty::POSITION != 0 {
                row[..12].copy_from_slice(&floats(&p));
            }
        }
        k if k.is_position() => {
            if d & dirty::POSITION != 0 || o.requantise {
                let b = o
                    .compression
                    .ok_or("compressed positions without a box")?
                    .position;
                for axis in 0..3 {
                    let q = quantize(p[axis], b[2 * axis], b[2 * axis + 1])
                        .ok_or_else(|| format!("vertex {i} is outside the compression box"))?;
                    row[2 * axis..2 * axis + 2].copy_from_slice(&q.to_le_bytes());
                }
            }
            if d & dirty::SKIN != 0
                && let Some(skin) = e.skins.get(i).copied().flatten()
            {
                write_skin(k, &mut row, skin);
            }
        }
        VertexKind::TangentFrame => {
            if d & dirty::NORMAL != 0 {
                row[0..4].copy_from_slice(&pack_normpacked3(c.normal).to_le_bytes());
            }
            if d & dirty::FRAME != 0 {
                row[4..8].copy_from_slice(&pack_normpacked3(c.binormal).to_le_bytes());
                row[8..12].copy_from_slice(&pack_normpacked3(c.tangent).to_le_bytes());
            }
        }
        VertexKind::Texcoord => {
            if d & dirty::UV != 0 {
                row[..8].copy_from_slice(&floats(&c.uv));
            }
        }
        VertexKind::CompressedTexcoord => {
            if d & dirty::UV != 0 {
                let t = o
                    .compression
                    .ok_or("compressed coordinates without a box")?
                    .texcoord;
                for axis in 0..2 {
                    let q = quantize(c.uv[axis], t[2 * axis], t[2 * axis + 1]).ok_or_else(|| {
                        format!(
                            "vertex {i}'s texture coordinate is outside the model's coordinate box"
                        )
                    })?;
                    row[2 * axis..2 * axis + 2].copy_from_slice(&q.to_le_bytes());
                }
            }
        }
        VertexKind::LightmapTexcoord if d & dirty::LIGHTMAP != 0 => {
            row[0..2].copy_from_slice(&snorm16(c.lightmap[0]).to_le_bytes());
            row[2..4].copy_from_slice(&snorm16(c.lightmap[1]).to_le_bytes());
        }
        // Streams the writer does not interpret keep the template's bytes.
        _ => {}
    }
    Ok(row)
}

/// The skin's nodes and weights in a skinned position row (the decoder's
/// layout, docs/formats/map-format.md "Render-model geometry").
fn write_skin(kind: VertexKind, row: &mut [u8], skin: Skin) {
    let slots = match kind {
        VertexKind::OneNode => 1,
        VertexKind::TwoNodes => 2,
        VertexKind::ThreeNodes => 3,
        VertexKind::FourNodes => 4,
        _ => return,
    };
    let (nodes, weights) = fit(skin, slots);
    match kind {
        VertexKind::OneNode => {
            row[6] = nodes[0];
            row[7] = 0;
        }
        VertexKind::TwoNodes => {
            row[6..8].fill(0);
            row[8..10].copy_from_slice(&nodes[..2]);
            row[10..12].copy_from_slice(&weights[..2]);
        }
        VertexKind::ThreeNodes => {
            row[6..9].copy_from_slice(&nodes[..3]);
            row[9..12].copy_from_slice(&weights[..3]);
        }
        _ => {
            row[6..8].fill(0);
            row[8..12].copy_from_slice(&nodes);
            row[12..16].copy_from_slice(&weights);
        }
    }
}

/// The `slots` strongest influences of `skin` (ties to the earlier slot),
/// weights renormalised to sum to 255 (the remainder to the strongest).
fn fit(skin: Skin, slots: usize) -> ([u8; 4], [u8; 4]) {
    let mut order: Vec<usize> = (0..4).filter(|&k| skin.weights[k] > 0).collect();
    order.sort_by(|&a, &b| skin.weights[b].cmp(&skin.weights[a]).then(a.cmp(&b)));
    order.truncate(slots);
    let mut nodes = [0u8; 4];
    let mut weights = [0u8; 4];
    let total: u32 = order.iter().map(|&k| u32::from(skin.weights[k])).sum();
    if total == 0 {
        weights[0] = 255;
        return (nodes, weights);
    }
    let mut sum = 0u32;
    for (slot, &k) in order.iter().enumerate() {
        nodes[slot] = skin.nodes[k];
        let w = u32::from(skin.weights[k]) * 255 / total;
        weights[slot] = w as u8;
        sum += w;
    }
    weights[0] = weights[0].saturating_add((255 - sum) as u8);
    (nodes, weights)
}

#[cfg(test)]
#[path = "grow_tests.rs"]
mod tests;
