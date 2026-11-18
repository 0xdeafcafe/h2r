//! The converted structure BSP (`sbsp`) and lightmap (`ltmp`), written as
//! [`map::build`] block trees for a built BSP section.
//!
//! What every field is and why it may be empty is in
//! docs/halo1/multiplayer.md, "The converted structure BSP"; the render
//! geometry layout is docs/formats/structure-bsp.md. In short: each of
//! Halo 1's clusters becomes one, with its rendered triangles, its portals
//! and its own lightmap page ([`crate::clusters`]), and each collision leaf
//! names its Halo 1 cluster. Materials name shaders the base map already
//! has.
//!
//! The structure's Havok MOPP code (`sbsp+0x1e0`) comes from outside, as
//! bytes: [`crate::convert::structure_mopp`].

use h1map::bsp::{StructureBsp, unpack_normal};
use map::TagId;
use map::build::{Block, RawRef, TagSet};
use tags::collision::{BSP_BLOCKS_SIZE, BSP_STRIDES, Bsp};

use crate::{Error, Result};

/// Bytes of the `sbsp` root written. The engine reads fields up to
/// `+0x238`; the rest is zero.
pub const SBSP_ROOT: usize = 0x280;
/// Bytes of the `ltmp` root written (the decoder reads `0x104`).
pub const LTMP_ROOT: usize = 0x200;
/// Cluster element size (E `0x000410ac`).
pub const CLUSTER: usize = 0xb0;
/// Render material element size.
pub const MATERIAL: usize = 0x20;
/// Collision material element size.
pub const COLLISION_MATERIAL: usize = 0x14;
/// Lightmap group element size.
pub const LIGHTMAP_GROUP: usize = 0x68;
/// The cluster section struct size (`STRUCTURE_SECTION`).
pub const SECTION: usize = 0x44;
/// Part element size.
pub const PART: usize = 0x48;
/// Bytes of a raw block's frame: `blkh`, the data size, and `blkf`.
pub const RAW_FRAME: usize = 12;
/// Part kind of an opaque part (0 and 5 are not drawn, E `0x000410fc`).
/// [`part_kind`] gives each material's.
pub const PART_KIND: u16 = 2;
/// Part kind of a decal drawn over opaque geometry.
pub const PART_KIND_DECAL: u16 = 3;
/// Part kind of a transparent part.
pub const PART_KIND_TRANSPARENT: u16 = 4;

/// [`part_kind`] of the base shader `id`, whose template `template`
/// names when it read ([`scenario_edit::scene::shader_bitmaps`]), else
/// as the shader library reads it (a shader that reader doesn't follow).
pub fn base_part_kind<R: std::io::Read + std::io::Seek>(
    base: &mut map::Map<R>,
    id: TagId,
    template: &str,
) -> u16 {
    if !template.is_empty() {
        return part_kind(template);
    }
    let read = tags::material::ShaderLibrary::read(base)
        .ok()
        .and_then(|lib| lib.shader(id).ok())
        .and_then(|s| base.tag(s.template.id).ok()?.name.clone());
    part_kind(read.as_deref().unwrap_or_default())
}

/// The part kind retail gives a structure part drawn with a shader of the
/// template `template` (a `stem` path). V (every structure part of every
/// retail Halo 2 map, by its material's template): every part under
/// `shader_templates\transparent\` and every `water_pool` and
/// `water_pool_static` part is kind 4; every `decal_simple` part and 85
/// of 98 `overlay` parts are kind 3; the other opaque templates are kind
/// 2 (kind 5, not drawn, on a few illuminated ones). Alpha-tested
/// templates are opaque: 155 of 157 `tex_bump_alpha_test_single_pass`
/// parts are kind 2.
pub fn part_kind(template: &str) -> u16 {
    let leaf = template.rsplit('\\').next().unwrap_or_default();
    if template.contains("\\transparent\\") || leaf.starts_with("water_pool") {
        PART_KIND_TRANSPARENT
    } else if matches!(leaf, "decal_simple" | "overlay") {
        PART_KIND_DECAL
    } else {
        PART_KIND
    }
}

/// Part flag bit 3: a triangle list.
pub const PART_TRIANGLE_LIST: u16 = 0x8;
/// Part flags written: every retail multiplayer structure part has 0xb,
/// the triangle list bit and bits 0 and 1 (not named).
pub const PART_FLAGS: u16 = PART_TRIANGLE_LIST | 0x3;
/// Size of the element at `sbsp+0x21c` written (one, zeroed; the engine
/// reads its first word, E `0x00210f0f`).
pub const ELEMENT_21C: usize = 0x40;
/// Size of the element at `sbsp+0x234` written (one, zeroed; the engine
/// reads `+0x20`, E `0x0003f927`).
pub const ELEMENT_234: usize = 0x30;
/// Size of an ambience (`sbsp+0x148`) or reverb (`sbsp+0x150`) sound
/// cluster (E `0x0018d7ba`: index * 0x14).
pub const SOUND_CLUSTER: usize = 0x14;
/// Size of the one element at `sbsp+0x94` (zeroed in every retail map).
pub const ELEMENT_94: usize = 0x24;

/// Size of the audibility element at `sbsp+0x224` (one in every retail
/// map; see [`Structure::audibility`]).
pub const AUDIBILITY: usize = 0x34;

/// Most vertices or indices one section holds (`u16` counts and indices).
pub const SECTION_MAX: usize = 0xffff;

/// Vertex stream types written, with their strides
/// (docs/formats/structure-bsp.md, "Stream types").
/// Every retail multiplayer structure section has these five (0x30 in all
/// but the unlit ones).
pub const STREAMS: [(u8, u8); 5] = [(0x01, 12), (0x18, 8), (0x1b, 12), (0x1f, 4), (0x30, 4)];

/// Section compression flags (cluster `+0x16`): bit 2, the secondary
/// (lightmap) texture coordinate is compressed, as stream 0x1f's `i16`s
/// are. Every retail multiplayer structure cluster has 4.
pub const COMPRESSION_FLAGS: u16 = 0x4;
/// Section lighting flags (cluster `+0x26`) of a lit section: bit 0 it has
/// lightmap texture coordinates, bit 1 per-vertex incident radiance
/// (stream 0x30). Every lit retail multiplayer structure cluster has 3.
pub const LIGHTING_FLAGS: u16 = 0x3;

/// A rendered vertex.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    /// Position.
    pub position: [f32; 3],
    /// Normal, binormal and tangent, packed 11:11:10 (the same packing in
    /// both games).
    pub frame: [u32; 3],
    /// Texture coordinate.
    pub texcoord: [f32; 2],
    /// Lightmap texture coordinate, in [0, 1].
    pub lightmap: [f32; 2],
    /// The lightmap's incident light direction, packed 11:11:10 (stream
    /// 0x30): Halo 1's lightmap vertex direction, or the normal when the
    /// vertex has none.
    pub incident: u32,
}

/// The triangles drawn with one Halo 1 shader.
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    /// The Halo 1 shader's tag name.
    pub shader: String,
    /// The lightmap page (image of the lightmap bitmap) its lightmap
    /// coordinates address; `None` when unlit.
    pub page: Option<u16>,
    /// The Halo 1 cluster its surfaces are in.
    pub cluster: u16,
    /// Vertices.
    pub vertices: Vec<Vertex>,
    /// Triangles, as indices into `vertices`.
    pub triangles: Vec<[u32; 3]>,
}

/// Halo 1's render geometry, one mesh per lightmap material and cluster
/// ([`crate::clusters::surface_clusters`]; a surface listed by several
/// clusters is in each one's mesh, and one no cluster lists is dropped, as
/// Halo 1 never draws it). `shader_name` names a Halo 1 shader
/// reference's tag.
pub fn meshes(
    bsp: &StructureBsp,
    shader_name: impl Fn(h1map::TagRef) -> String,
) -> Result<Vec<Mesh>> {
    let surfaces = bsp.render_surfaces()?;
    let owner = crate::clusters::surface_clusters(bsp, surfaces.len())?;
    let mut out = Vec::new();
    for l in bsp.lightmaps()? {
        for m in l.materials {
            let first = m.first_surface as usize;
            let end = first.saturating_add(m.surface_count as usize);
            let Some(tris) = surfaces.get(first..end) else {
                return Err(Error::Invalid("a material's surfaces run past the block"));
            };
            let n = m.vertices.len();
            if tris.iter().flatten().any(|&i| usize::from(i) >= n) {
                return Err(Error::Invalid("a render surface indexes past its material"));
            }
            let vertices = m
                .vertices
                .iter()
                .enumerate()
                .map(|(i, v)| Vertex {
                    position: v.position,
                    frame: v.frame,
                    texcoord: v.texcoord,
                    lightmap: m
                        .lightmap_vertices
                        .get(i)
                        .map_or([0.0; 2], |l| l.texcoord.map(|c| c.clamp(0.0, 1.0))),
                    incident: m.lightmap_vertices.get(i).map_or(v.frame[0], |l| l.normal),
                })
                .collect();
            let triangles: Vec<[u32; 3]> = tris.iter().map(|t| t.map(u32::from)).collect();
            out.extend(crate::clusters::split_by_cluster(
                &shader_name(m.shader),
                u16::try_from(l.bitmap).ok(),
                vertices,
                &triangles,
                &owner[first..end],
            ));
        }
    }
    Ok(out)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Puts triangles in Halo 2's winding: counter-clockwise about the first
/// vertex's normal (docs/formats/structure-bsp.md, "Triangle lists"). The
/// order is decided once for the whole level, by majority, and every
/// triangle is flipped if most disagree. Returns whether it flipped.
pub fn fix_winding(meshes: &mut [Mesh]) -> bool {
    let (mut agree, mut disagree) = (0usize, 0usize);
    for m in meshes.iter() {
        for t in &m.triangles {
            let [a, b, c] = t.map(|i| m.vertices[i as usize].position);
            let n = unpack_normal(m.vertices[t[0] as usize].frame[0]);
            let d = dot(cross(sub(b, a), sub(c, a)), n);
            if d > 0.0 {
                agree += 1;
            } else if d < 0.0 {
                disagree += 1;
            }
        }
    }
    let flip = disagree > agree;
    if flip {
        for m in meshes.iter_mut() {
            for t in &mut m.triangles {
                t.swap(1, 2);
            }
        }
    }
    flip
}

/// One drawn run of a section's indices with one material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    /// Index into the structure's materials.
    pub material: u16,
    /// First index.
    pub first: u16,
    /// Index count (a multiple of 3).
    pub count: u16,
}

/// One cluster's render geometry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    /// The lightmap page every vertex's lightmap coordinates address (a
    /// cluster has one, its render info's); `None` when unlit.
    pub page: Option<u16>,
    /// Vertices.
    pub vertices: Vec<Vertex>,
    /// Triangle-list indices.
    pub indices: Vec<u16>,
    /// Parts, tiling `indices` in order.
    pub parts: Vec<Part>,
}

impl Section {
    /// `[x min, x max, y min, y max, z min, z max]` of the vertices; zero
    /// when there are none.
    pub fn bounds(&self) -> [f32; 6] {
        bounds(self.vertices.iter().map(|v| v.position))
    }
}

/// Bounds of points as `[x min, x max, y min, y max, z min, z max]`; zero
/// for none.
pub fn bounds(points: impl Iterator<Item = [f32; 3]>) -> [f32; 6] {
    let mut b = [f32::MAX, f32::MIN, f32::MAX, f32::MIN, f32::MAX, f32::MIN];
    let mut any = false;
    for p in points {
        any = true;
        for k in 0..3 {
            b[2 * k] = b[2 * k].min(p[k]);
            b[2 * k + 1] = b[2 * k + 1].max(p[k]);
        }
    }
    if any { b } else { [0.0; 6] }
}

/// A part's bounding sphere and box, as the part's `+0x10` centre, `+0x2c`
/// radius and `+0x30` box hold them: the centre is the box's, the radius
/// the farthest vertex from it (at least a millimetre, so never zero; the
/// draw at E `0x00045da1` swaps in a constant for a radius of 0), the box
/// `[x min, x max, y min, y max, z min, z max]`. Retail fills all three
/// for every structure part.
pub fn part_sphere(s: &Section, p: &Part) -> ([f32; 3], f32, [f32; 6]) {
    let first = usize::from(p.first);
    let end = (first + usize::from(p.count)).min(s.indices.len());
    let points = || {
        s.indices[first..end]
            .iter()
            .filter_map(|&i| s.vertices.get(usize::from(i)).map(|v| v.position))
    };
    let b = bounds(points());
    let centre = [
        (b[0] + b[1]) / 2.0,
        (b[2] + b[3]) / 2.0,
        (b[4] + b[5]) / 2.0,
    ];
    let radius = points()
        .map(|q| dot(sub(q, centre), sub(q, centre)).sqrt())
        .fold(0.001f32, f32::max);
    (centre, radius, b)
}

/// Packs meshes into sections of at most [`SECTION_MAX`] vertices and
/// indices, one lightmap page each (meshes are taken in page order,
/// unlit last, and otherwise in their order). `material[i]` is mesh `i`'s
/// structure material. A mesh can span sections; a part never does.
pub fn pack(meshes: &[Mesh], material: &[u16]) -> Vec<Section> {
    pack_order(meshes, material, (0..meshes.len()).collect())
}

/// [`pack`] over the meshes `order` names.
pub fn pack_order(meshes: &[Mesh], material: &[u16], mut order: Vec<usize>) -> Vec<Section> {
    const MAX_INDICES: usize = SECTION_MAX / 3 * 3;
    order.sort_by_key(|&i| meshes[i].page.map_or(u32::MAX, u32::from));
    let mut out = vec![Section::default()];
    for (m, &mat) in order.iter().map(|&i| (&meshes[i], &material[i])) {
        let mut remap: Vec<Option<u16>> = vec![None; m.vertices.len()];
        {
            let s = out.last_mut().expect("never empty");
            if s.indices.is_empty() {
                s.page = m.page;
            } else if s.page != m.page {
                out.push(Section {
                    page: m.page,
                    ..Section::default()
                });
            }
        }
        for t in &m.triangles {
            let s = out.last_mut().expect("never empty");
            let new = t.iter().filter(|&&i| remap[i as usize].is_none()).count();
            if s.vertices.len() + new > SECTION_MAX || s.indices.len() + 3 > MAX_INDICES {
                out.push(Section {
                    page: m.page,
                    ..Section::default()
                });
                remap.iter_mut().for_each(|r| *r = None);
            }
            let s = out.last_mut().expect("never empty");
            let first = s.indices.len() as u16;
            if s.parts.last().is_none_or(|p| {
                p.material != mat || usize::from(p.first) + usize::from(p.count) != s.indices.len()
            }) {
                s.parts.push(Part {
                    material: mat,
                    first,
                    count: 0,
                });
            }
            for &i in t {
                let i = i as usize;
                let at = *remap[i].get_or_insert_with(|| {
                    s.vertices.push(m.vertices[i]);
                    (s.vertices.len() - 1) as u16
                });
                s.indices.push(at);
            }
            s.parts.last_mut().expect("pushed").count += 3;
        }
    }
    out.retain(|s| !s.indices.is_empty());
    out
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn f32s(values: impl IntoIterator<Item = f32>) -> Vec<u8> {
    values.into_iter().flat_map(f32::to_le_bytes).collect()
}

/// A FourCC as the engine stores it: the tag-group byte order, reversed.
fn fourcc(s: &[u8; 4]) -> [u8; 4] {
    [s[3], s[2], s[1], s[0]]
}

/// A section's raw block (`blkh`, size, section struct, resources,
/// `blkf`) and its resource table (16-byte entries), as
/// docs/formats/structure-bsp.md, "Raw block" and "Block resource".
/// `kind` gives a part's kind from its material.
pub fn raw_block(s: &Section, kind: impl Fn(u16) -> u16) -> (Vec<u8>, Vec<[u8; 16]>) {
    if s.indices.is_empty() {
        let mut block = fourcc(b"blkh").to_vec();
        block.extend(0u32.to_le_bytes());
        block.extend([0u8; SECTION]);
        block.extend(fourcc(b"blkf"));
        return (block, Vec::new());
    }
    let mut parts = Vec::with_capacity(s.parts.len() * PART);
    let mut subparts = Vec::with_capacity(s.parts.len() * 8);
    for (i, p) in s.parts.iter().enumerate() {
        let mut e = [0u8; PART];
        put16(&mut e, 0, kind(p.material));
        put16(&mut e, 2, PART_FLAGS);
        put16(&mut e, 4, p.material);
        put16(&mut e, 6, p.first);
        put16(&mut e, 8, p.count);
        put16(&mut e, 0xa, i as u16);
        put16(&mut e, 0xc, 1);
        let (centre, radius, b) = part_sphere(s, p);
        e[0x10..0x1c].copy_from_slice(&f32s(centre));
        e[0x2c..0x30].copy_from_slice(&radius.to_le_bytes());
        e[0x30..0x48].copy_from_slice(&f32s(b));
        parts.extend(e);
        let mut sp = [0u8; 8];
        put16(&mut sp, 0, p.first);
        put16(&mut sp, 2, p.count);
        put16(&mut sp, 4, 0xffff);
        put16(&mut sp, 6, i as u16);
        subparts.extend(sp);
    }
    let indices: Vec<u8> = s.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let mut descriptors = vec![0u8; 0x20 * STREAMS.len()];
    for (i, (kind, stride)) in STREAMS.iter().enumerate() {
        descriptors[0x20 * i] = *kind;
        descriptors[0x20 * i + 1] = *stride;
    }
    let v = &s.vertices;
    let positions = f32s(v.iter().flat_map(|v| v.position));
    let texcoords = f32s(v.iter().flat_map(|v| v.texcoord));
    let frames: Vec<u8> = v
        .iter()
        .flat_map(|v| v.frame)
        .flat_map(u32::to_le_bytes)
        .collect();
    let lightmap: Vec<u8> = v
        .iter()
        .flat_map(|v| v.lightmap)
        .flat_map(|c| ((c.clamp(0.0, 1.0) * 32767.0).round() as i16).to_le_bytes())
        .collect();
    let incident: Vec<u8> = v.iter().flat_map(|v| v.incident.to_le_bytes()).collect();

    // (kind, field, element size or descriptor index, bytes)
    let (visibility, reorder) = visibility_mopp(s);
    let resources: [(u8, u16, u16, Vec<u8>); 11] = [
        (0, 0x00, PART as u16, parts),
        (0, 0x08, 8, subparts),
        (0, 0x20, 2, indices),
        (1, 0x28, 0xffff, visibility),
        (0, 0x30, 2, reorder),
        (0, 0x38, 0x20, descriptors),
        (2, 0x38, 0, positions),
        (2, 0x38, 1, texcoords),
        (2, 0x38, 2, frames),
        (2, 0x38, 3, lightmap),
        (2, 0x38, 4, incident),
    ];
    let mut section = vec![0u8; SECTION];
    let mut data = Vec::new();
    let mut table = Vec::new();
    for (kind, field, secondary, bytes) in &resources {
        match kind {
            0 => put32(
                &mut section,
                usize::from(*field),
                (bytes.len() / usize::from(*secondary)) as u32,
            ),
            // A data resource: its size, as retail sections hold it; the
            // code object starts 16-aligned in the loaded block (block
            // `+8`, then the section struct, then the resource area).
            1 => {
                put32(&mut section, usize::from(*field), bytes.len() as u32);
                while !(8 + SECTION + data.len() + 4).is_multiple_of(16) {
                    data.push(0);
                }
            }
            _ => {}
        }
        data.extend(fourcc(b"rsrc"));
        let mut e = [0u8; 16];
        e[0] = *kind;
        e[1] = 2;
        put16(&mut e, 2, *field);
        put16(&mut e, 4, *field);
        put16(&mut e, 6, *secondary);
        put32(&mut e, 8, bytes.len() as u32);
        put32(&mut e, 0xc, data.len() as u32);
        data.extend(bytes);
        table.push(e);
    }
    let mut block = fourcc(b"blkh").to_vec();
    block.extend((data.len() as u32).to_le_bytes());
    block.extend(&section);
    block.extend(&data);
    block.extend(fourcc(b"blkf"));
    (block, table)
}

/// A predicted resource (cluster `+0x84`, 8 bytes): `u16` type, `u16`
/// index, the tag's datum. The engine walks a cluster's list, and those of
/// the clusters it sees, through E `0x0016e5e0` (called from
/// `0x0003f495`), before drawing.
pub const PREDICTED_RESOURCE: usize = 8;
/// Predicted resource type 0: image `index` of bitmap `tag` (E
/// `0x0016e656`: the bitmap's `+0x48` images, 0x74 each, unchecked).
pub const PREDICT_BITMAP: u16 = 0;
/// Predicted resource type 3: cluster `index`'s geometry block of the
/// structure `tag` (E `0x0016e68e`). Types 6 to 8 name lightmap buckets,
/// which a converted lightmap has none of (E `0x0016e7b0` and
/// `0x0016e820` read the group's per-cluster tables unchecked), so they
/// are never written.
pub const PREDICT_CLUSTER_GEOMETRY: u16 = 3;

/// The bitmaps a shader's parameters name (`shad+0x20` postprocess, its
/// first element's `+0x04` parameters, 0xc each with the bitmap datum at
/// `+0x00`), in order and without repeats, as far as `set` holds them:
/// a built shader's tree, or a pinned shader's own bytes. Only datums of
/// `bitm` tags in the set count.
pub fn shader_bitmaps(set: &TagSet, shader: TagId) -> Vec<TagId> {
    use map::build::{FixupKind, TagPayload};
    let mut raw = Vec::new();
    match set.tag(shader).map(|t| &t.payload) {
        Some(TagPayload::Built(root)) => {
            let child = |b: &'_ Block, at: u32| {
                b.fixups.iter().find_map(|f| match &f.kind {
                    FixupKind::Block { count, child } if f.offset == at && *count > 0 => {
                        Some((*count, child.clone()))
                    }
                    _ => None,
                })
            };
            if let Some((_, pp)) = child(root, 0x20)
                && let Some((n, params)) = child(&pp, 0x04)
            {
                for i in 0..n as usize {
                    let at = 0xc * i;
                    let fixed = params.fixups.iter().find_map(|f| match f.kind {
                        FixupKind::TagId(id) | FixupKind::TagRef(id) if f.offset as usize == at => {
                            Some(id)
                        }
                        _ => None,
                    });
                    let bytes = params
                        .bytes
                        .get(at..at + 4)
                        .map(|b| TagId(u32::from_le_bytes(b.try_into().expect("4"))));
                    raw.extend(fixed.or(bytes));
                }
            }
        }
        Some(TagPayload::Pinned { address, bytes }) => {
            let word = |at: usize| {
                bytes
                    .get(at..at + 4)
                    .map(|b| u32::from_le_bytes(b.try_into().expect("4")))
            };
            let inside = |a: u32| a.checked_sub(*address).map(|o| o as usize);
            let params = (|| {
                if word(0x20)? == 0 {
                    return None;
                }
                let pp = inside(word(0x24)?)?;
                let n = word(pp + 4)? as usize;
                let at = inside(word(pp + 8)?)?;
                Some((n.min(64), at))
            })();
            if let Some((n, at)) = params {
                raw.extend((0..n).filter_map(|i| word(at + 0xc * i).map(TagId)));
            }
        }
        _ => {}
    }
    let mut out = Vec::new();
    for id in raw {
        let bitmap = set
            .tag(id)
            .is_some_and(|t| t.id == id && t.group == map::FourCC(*b"bitm"));
        if bitmap && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// A section's visibility MOPP code (section `+0x28`, a data resource)
/// and its reorder table (`+0x30`, `u16`s). Every retail structure section
/// has both.
///
/// The engine draws a cluster for a set of volumes (E `0x00044e90`, from
/// the pass at `0x00044720`) by querying this code with the volumes'
/// planes (`0x002dd930`). A leaf the volume cuts emits its key, and
/// `0x000458d0` sets bit *key* of the cluster's subpart mask
/// (`0x00045a34`). A subtree wholly inside emits the keys of its first and
/// last leaf instead (walks `0x002ff570`, `0x002ff210`), and the pair is
/// read through the table with `n` the subpart count (`+0x08`): positions
/// `table[n + first]..=table[n + last]`, and bit `table[p]` for each
/// (`0x000459cb`). So `table[..n]` is the keys in leaf order and
/// `table[n + key]` the key's position: V on every retail section that
/// has a code (4095 on the disc, `tests/render_fields.rs`). Retail's tables
/// run on past `2n` for a second code (read only when the table is longer
/// than `2n`, `0x00044f7a`); ours stop at `2n`.
///
/// So every subpart (one per part, keyed by the part's index) is one leaf,
/// over the part's box, and the table is the leaf order and its inverse.
/// Keyed per triangle (converter version 9) a key had many leaves, the
/// table was `n` long, and `table[n + key]` read past it: the bits set
/// from whatever lay there ran off the mask area and corrupted the view
/// (a30 and d20 faulted clearing it at `0x00134925`, a10, b30 and d40 on
/// a null pass object at `0x00147b75`). With no table at all (`+0x30`
/// count 0) the pass takes the object branch and reads the render entry's
/// null object (E `0x0004539c`, converter version 7).
pub fn visibility_mopp(s: &Section) -> (Vec<u8>, Vec<u8>) {
    let surfaces: Vec<mopp::Surface> = s
        .parts
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let (_, _, b) = part_sphere(s, p);
            mopp::Surface {
                key: k as u32,
                vertices: vec![[b[0], b[2], b[4]], [b[1], b[3], b[5]]],
            }
        })
        .collect();
    let code = mopp::build(&surfaces);
    let n = s.parts.len();
    let order = leaf_order(&code);
    debug_assert_eq!(order.len(), n, "one leaf per subpart");
    let mut table = vec![0u16; 2 * n];
    for (p, &k) in order.iter().enumerate().take(n) {
        table[p] = k as u16;
        if let Some(t) = table.get_mut(n + k as usize) {
            *t = p as u16;
        }
    }
    let table = table.into_iter().flat_map(u16::to_le_bytes).collect();
    (code.to_block(), table)
}

/// Keys of `code`'s leaves in the order the engine walks them: a query
/// that every leaf passes visits both children of a split, the first one
/// first (E `0x003003cd`), as the first- and last-leaf walks do.
pub fn leaf_order(code: &mopp::Code) -> Vec<u32> {
    let all = mopp::Aabb {
        min: [-1.0e9; 3],
        max: [1.0e9; 3],
    };
    mopp::query_aabb(code, &all).unwrap_or_default()
}

/// Whether a section's visibility `code` block and reorder `table` (raw
/// `u16`s) are laid out as the engine's volume pass reads them for `n`
/// subparts (see [`visibility_mopp`]): every key below `n`, one leaf per
/// key, `table[..n]` the keys in leaf order and `table[n + key]` each
/// key's position. Returns what is wrong.
pub fn check_visibility(n: usize, code: &[u8], table: &[u8]) -> std::result::Result<(), String> {
    let table: Vec<u16> = table
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| u16::from_le_bytes(*w))
        .collect();
    let code = mopp::Code::parse(code).map_err(|e| format!("code: {e:?}"))?;
    mopp::check::validate(&code.bytecode).map_err(|e| format!("code: {e:?}"))?;
    let leaves = leaf_order(&code);
    if table.len() < 2 * n {
        return Err(format!("table of {} for {n} subparts", table.len()));
    }
    if let Some(k) = leaves.iter().find(|&&k| k as usize >= n) {
        return Err(format!("key {k} of {n} subparts"));
    }
    let order: Vec<u32> = table[..n].iter().map(|&k| u32::from(k)).collect();
    if leaves != order {
        return Err(format!(
            "{} leaves, not the table's order of {n} keys",
            leaves.len()
        ));
    }
    for (p, &k) in table[..n].iter().enumerate() {
        if usize::from(table[n + usize::from(k)]) != p {
            return Err(format!("table[n + {k}] is not {p}"));
        }
    }
    Ok(())
}

/// How many images bitmap `id` has (`bitm+0x44`), as far as `set` holds
/// it: a built tree's block, or a pinned tag's own count; 0 otherwise. A
/// predicted image is indexed into that block unchecked (E `0x0016e663`),
/// so nothing past it is ever predicted.
pub fn bitmap_images(set: &TagSet, id: TagId) -> u32 {
    use map::build::{FixupKind, TagPayload};
    match set.tag(id).map(|t| &t.payload) {
        Some(TagPayload::Built(root)) => root
            .fixups
            .iter()
            .find_map(|f| match &f.kind {
                FixupKind::Block { count, .. } if f.offset == 0x44 => Some(*count),
                _ => None,
            })
            .unwrap_or(0),
        Some(TagPayload::Pinned { bytes, .. }) => bytes
            .get(0x44..0x48)
            .map_or(0, |b| u32::from_le_bytes(b.try_into().expect("4"))),
        _ => 0,
    }
}

/// Element templates taken from the base map's own structure BSP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Templates {
    /// A collision material: global material index at `+0x08`, shader
    /// reference at `+0x0c`. No pointers, so it copies as is.
    pub collision_material: [u8; COLLISION_MATERIAL],
    /// Each base render material's first 16 bytes (shader reference at
    /// `+0x08`, its datum at `+0x0c`). A written material copies one; the
    /// rest is written here: its block at `+0x10` empty, `+0x1c` 0xff.
    pub material_heads: Vec<[u8; 16]>,
    /// Each material head's part kind ([`part_kind`] of its shader's
    /// template; [`PART_KIND`] until set).
    pub part_kinds: Vec<u16>,
}

impl Templates {
    /// The shader datum of the collision material template (`+0x10`).
    pub fn collision_shader(&self) -> map::TagId {
        let m = &self.collision_material;
        map::TagId(u32::from_le_bytes(m[0x10..0x14].try_into().expect("4")))
    }

    /// The shader datum of base material `i`.
    pub fn shader(&self, i: usize) -> map::TagId {
        let h = &self.material_heads[i];
        map::TagId(u32::from_le_bytes(h[0xc..0x10].try_into().expect("4")))
    }
}

impl Templates {
    /// The first collision material and every render material of a base
    /// `sbsp` payload loaded at `address`.
    pub fn from_base(sbsp: &[u8], address: u32) -> Result<Self> {
        let word = |at: usize| -> Result<u32> {
            sbsp.get(at..at + 4)
                .map(|b| u32::from_le_bytes(b.try_into().expect("4")))
                .ok_or(Error::Invalid("base sbsp root is short"))
        };
        let first = |field: usize, size: usize| -> Result<&[u8]> {
            if word(field)? == 0 {
                return Err(Error::Invalid("base sbsp has an empty material block"));
            }
            let at = word(field + 4)?.wrapping_sub(address) as usize;
            sbsp.get(at..at + size)
                .ok_or(Error::Invalid("base sbsp material outside the tag"))
        };
        Ok(Self {
            collision_material: first(0x0c, COLLISION_MATERIAL)?.try_into().expect("size"),
            material_heads: {
                let n = word(0xa4)? as usize;
                let at = word(0xa8)?.wrapping_sub(address) as usize;
                let all = at
                    .checked_add(n * MATERIAL)
                    .and_then(|end| sbsp.get(at..end))
                    .ok_or(Error::Invalid("base sbsp material outside the tag"))?;
                first(0xa4, 16)?;
                all.as_chunks::<MATERIAL>()
                    .0
                    .iter()
                    .map(|m| m[..16].try_into().expect("16"))
                    .collect::<Vec<_>>()
            },
            part_kinds: Vec::new(),
        })
        .map(|mut t| {
            t.part_kinds = vec![PART_KIND; t.material_heads.len()];
            t
        })
    }
}

/// Everything a converted structure is made of.
#[derive(Debug, Clone)]
pub struct Structure<'a> {
    /// The collision BSP, converted.
    pub collision: &'a Bsp,
    /// Collision materials to write (at least one past the largest surface
    /// material).
    pub collision_materials: usize,
    /// Render sections, one cluster each.
    pub sections: &'a [Section],
    /// Render materials to write (one past the largest part material):
    /// for each, the base material (index into
    /// [`Templates::material_heads`]) whose shader it draws with.
    pub materials: &'a [usize],
    /// World bounds, `[x min, x max, y min, y max, z min, z max]`.
    pub bounds: [f32; 6],
    /// The Havok MOPP code block (`sbsp+0x1e0`), as the block holds it.
    pub mopp: &'a [u8],
    /// Structure checksum (`sbsp+0x08`, repeated by the lightmap group).
    pub checksum: u32,
    /// Base element templates.
    pub templates: &'a Templates,
    /// The lightmap bitmap (`bitm`) whose images the sections' pages
    /// index, or [`TagId::NULL`]: every cluster unlit.
    pub lightmap_bitmap: TagId,
    /// What each cluster plays and which cluster each collision leaf is
    /// in ([`Sound`]); the default plays nothing, every leaf in cluster 0.
    pub sound: &'a Sound,
    /// The portals and each cluster's portal list
    /// ([`crate::clusters::portals`]); the default has none.
    pub visibility: &'a crate::clusters::Visibility,
}

/// Ambient sound in a converted structure. The engine plays the
/// background sound and sound environment of the listener's cluster:
/// cluster `+0x72` and `+0x74` (i16, -1 none) index the structure's own
/// palettes, `sbsp +0xd4` (`0x64` each) and `+0xdc` (`0x48` each) (E
/// `0x0011bf46`, `0x0018d730`..`0x0018d8a5`, `0x0018ddf0`..`0x0018de0d`),
/// which on every retail map hold the same tags as the scenario's
/// (`scnr +0x250`, `+0x258`; `tests/h2_sbsp.rs`). The listener's cluster
/// is its collision leaf's (`sbsp +0x2c`, 8 bytes, i16 cluster at `+0`).
#[derive(Debug, Clone, Default)]
pub struct Sound {
    /// Palette entries: the name and the `lsnd`, then the `snde`.
    pub backgrounds: Vec<(String, TagId)>,
    pub environments: Vec<(String, TagId)>,
    /// Per cluster: background sound and sound environment (-1 none).
    pub clusters: Vec<[i16; 2]>,
    /// Per collision leaf: its cluster.
    pub leaves: Vec<u16>,
}

/// Checks a structure BSP's audibility the way the engine reads it:
/// `bytes` is the `sbsp` tag as loaded at `address`. The engine compares
/// clusters `i < j` through the element at `+0x224` when there is one,
/// reading byte `(n-1)i - i(i+1)/2 + j - 1` of its `+0x24` table and the
/// same bit of its `+0x1c` words (E `0x00249d60`, `0x00249c20`), and
/// otherwise through the sound PAS data at `+0xe4` (E `0x00249ed0`), which
/// no retail map has. So: one element, both tables covering every pair.
pub fn check_audibility(bytes: &[u8], address: u32) -> std::result::Result<(), String> {
    let word = |at: usize| -> std::result::Result<u32, String> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4")))
            .ok_or(format!("{at:#x} is past the tag"))
    };
    let n = word(0x9c)? as usize;
    if word(0x224)? < 1 {
        return Err(format!("no audibility element at +0x224 ({n} clusters)"));
    }
    let e = word(0x228)?
        .checked_sub(address)
        .ok_or("+0x228 points before the tag")? as usize;
    let pairs = n * n.saturating_sub(1) / 2;
    let distances = word(e + 0x24)? as usize;
    let deafening = word(e + 0x1c)? as usize;
    if distances < pairs {
        return Err(format!(
            "{distances} distance bytes for {pairs} cluster pairs"
        ));
    }
    if deafening * 32 < pairs {
        return Err(format!(
            "{deafening} deafening words for {pairs} cluster pairs"
        ));
    }
    for (count, pointer, len) in [(0x24, 0x28, distances), (0x1c, 0x20, 4 * deafening)] {
        if word(e + count)? == 0 {
            continue;
        }
        let at = word(e + pointer)?
            .checked_sub(address)
            .ok_or(format!("audibility +{pointer:#x} points before the tag"))?
            as usize;
        if at + len > bytes.len() {
            return Err(format!("audibility +{count:#x} runs past the tag"));
        }
    }
    Ok(())
}

/// Checks the reads the engine makes with no count check, beyond
/// [`check_audibility`]: each cluster's ambience (`+0x78`) and reverb
/// (`+0x7a`) sound cluster indexes `+0x148` / `+0x150` (E `0x0018d7c0`,
/// `0x0018dd20`), and each sound cluster's portal list (i16, low 15 bits a
/// portal index into `+0x5c`) lies in the tag and names real portals.
pub fn check_sound_clusters(bytes: &[u8], address: u32) -> std::result::Result<(), String> {
    let word = |at: usize| -> std::result::Result<u32, String> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4")))
            .ok_or(format!("{at:#x} is past the tag"))
    };
    let at = |a: u32, what: &str| -> std::result::Result<usize, String> {
        a.checked_sub(address)
            .map(|o| o as usize)
            .ok_or(format!("{what} points before the tag"))
    };
    let n = word(0x9c)? as usize;
    let clusters = at(word(0xa0)?, "clusters")?;
    let portals = word(0x5c)? as usize;
    for (field, index) in [(0x148usize, 0x78usize), (0x150, 0x7a)] {
        let count = word(field)? as usize;
        let base = if count > 0 {
            at(word(field + 4)?, "sound clusters")?
        } else {
            0
        };
        if base + count * SOUND_CLUSTER > bytes.len() {
            return Err(format!("+{field:#x} runs past the tag"));
        }
        for c in 0..n {
            let e = clusters + c * CLUSTER + index;
            let i = bytes
                .get(e..e + 2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .ok_or("cluster past the tag")?;
            if i < 0 || i as usize >= count {
                return Err(format!(
                    "cluster {c} names sound cluster {i} of +{field:#x}, which has {count}"
                ));
            }
        }
        for k in 0..count {
            let e = base + k * SOUND_CLUSTER;
            let list = word(e + 4)? as usize;
            if list == 0 {
                continue;
            }
            let p = at(word(e + 8)?, "portal list")?;
            for j in 0..list {
                let v = bytes
                    .get(p + 2 * j..p + 2 * j + 2)
                    .ok_or("portal list past the tag")?;
                let portal = (u16::from_le_bytes([v[0], v[1]]) & 0x7fff) as usize;
                if portal >= portals {
                    return Err(format!(
                        "+{field:#x}[{k}] names portal {portal} of {portals}"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The eight block fields of a collision BSP as a [`Block`] whose blocks
/// are fixups.
pub fn collision_block(bsp: &Bsp) -> Block {
    let bytes = bsp.encode_blocks(0);
    let mut out = Block::zeroed(BSP_BLOCKS_SIZE);
    let mut at = BSP_BLOCKS_SIZE;
    for (i, stride) in BSP_STRIDES.iter().enumerate() {
        let count = u32::from_le_bytes(bytes[8 * i..8 * i + 4].try_into().expect("4"));
        let len = count as usize * stride;
        out.block(
            (8 * i) as u32,
            count,
            Block::new(bytes[at..at + len].to_vec()),
        );
        at += len;
    }
    out
}

impl Structure<'_> {
    /// The part kind of structure material `material`: its base material
    /// head's ([`Templates::part_kinds`]).
    pub fn part_kind(&self, material: u16) -> u16 {
        let from = self
            .materials
            .get(usize::from(material))
            .copied()
            .unwrap_or(0);
        self.templates
            .part_kinds
            .get(from)
            .copied()
            .unwrap_or(PART_KIND)
    }

    /// Clusters (and so sections), at least one.
    pub fn clusters(&self) -> usize {
        self.sections.len().max(1)
    }

    /// Visibility rows: every cluster sees every cluster.
    pub fn visibility(&self) -> Vec<u8> {
        let n = self.clusters();
        let words = n.div_ceil(32);
        let mut row = vec![0u32; words];
        for c in 0..n {
            row[c / 32] |= 1 << (c % 32);
        }
        (0..n)
            .flat_map(|_| row.iter().flat_map(|w| w.to_le_bytes()))
            .collect()
    }

    /// The audibility element (`sbsp+0x224`, one in every retail map):
    /// no doors, and for each pair of clusters `i < j` a distance byte
    /// (`+0x24`, `n(n-1)/2` of them) and an AI-deafening bit (`+0x1c`,
    /// `ceil(n(n-1)/2 / 32)` words). Every distance is 0 (adjacent: every
    /// cluster hears and is near every other) and no pair is deafened.
    ///
    /// Without it the engine falls back to the sound PAS data at
    /// `sbsp+0xe4`, empty in every retail map, and reads through its null
    /// pointer as soon as two clusters are compared (E `0x00249dfa` ->
    /// `0x00249f03`, called every tick from `0x0018cb77`).
    pub fn audibility(&self) -> Block {
        let n = self.clusters();
        let pairs = n * (n - 1) / 2;
        let mut e = Block::zeroed(AUDIBILITY);
        // +0x00 door count 0; +0x04/+0x08 the distance range a byte of
        // 1..=254 spans (unused here: byte 0 reads as 0.0 directly).
        let [x0, x1, y0, y1, z0, z1] = self.bounds;
        let diagonal = ((x1 - x0).powi(2) + (y1 - y0).powi(2) + (z1 - z0).powi(2)).sqrt();
        e.put_f32(0x04, 0.0)
            .put_f32(0x08, if diagonal.is_finite() { diagonal } else { 0.0 });
        if pairs > 0 {
            e.block(
                0x1c,
                pairs.div_ceil(32) as u32,
                Block::zeroed(4 * pairs.div_ceil(32)),
            );
            e.block(0x24, pairs as u32, Block::zeroed(pairs));
        }
        e
    }

    /// The background sound palette (`sbsp +0xd4`), the scenario's
    /// elements ([`crate::ambience::background_entry`]).
    fn background_palette(&self) -> Option<(u32, Block)> {
        palette(
            &self.sound.backgrounds,
            crate::ambience::BACKGROUND_SOUND,
            crate::ambience::background_entry,
        )
    }

    /// The sound environment palette (`sbsp +0xdc`).
    fn environment_palette(&self) -> Option<(u32, Block)> {
        palette(
            &self.sound.environments,
            crate::ambience::SOUND_ENVIRONMENT,
            crate::ambience::environment_entry,
        )
    }

    /// One ambience (`sbsp+0x148`) or reverb (`sbsp+0x150`) sound
    /// cluster holding every cluster, which every cluster names at `+0x78`
    /// and `+0x7a`. The engine reads `[sbsp+0x14c] + 0x14 * cluster[+0x78]`
    /// with no check (E `0x0018d7b4..0x0018d7c0`; the reverb twin at
    /// `0x0018dd10`), so an empty block faults at address 4. Every retail
    /// map has at least one of each.
    ///
    /// `+0x00` the palette entry (`+0xd4` background sounds, `+0xdc` sound
    /// environments): none, -1 whether read as 16 or 32 bits (both
    /// palettes are empty here); `+0x04` the enclosing portals (i16, top
    /// bit the portal's side): none, so the walk at `0x0018d7c3` is
    /// skipped; `+0x0c` the interior clusters (i16): all of them.
    pub fn sound_cluster(&self) -> Block {
        let n = self.clusters();
        let mut e = Block::zeroed(SOUND_CLUSTER);
        e.put_u32(0, u32::MAX);
        let interior: Vec<u8> = (0..n as u16).flat_map(u16::to_le_bytes).collect();
        e.block(0x0c, n as u32, Block::new(interior));
        e
    }

    /// Cluster `c`'s predicted resources (`+0x84`), as retail lists
    /// them: its own geometry block, the lightmap image its render info
    /// names, and every bitmap its parts' shaders draw with (image 0).
    pub fn predicted(&self, set: &TagSet, c: usize, sbsp: TagId) -> Vec<(u16, u16, TagId)> {
        let Some(s) = self.sections.get(c) else {
            return Vec::new();
        };
        let mut out = vec![(PREDICT_CLUSTER_GEOMETRY, c as u16, sbsp)];
        if s.indices.is_empty() {
            return out;
        }
        if let Some(page) = s.page
            && self.lightmap_bitmap != TagId::NULL
            && u32::from(page) < bitmap_images(set, self.lightmap_bitmap)
        {
            out.push((PREDICT_BITMAP, page, self.lightmap_bitmap));
        }
        let mut shaders: Vec<TagId> = Vec::new();
        for p in &s.parts {
            let from = self
                .materials
                .get(usize::from(p.material))
                .copied()
                .unwrap_or(0);
            if from < self.templates.material_heads.len() {
                let id = self.templates.shader(from);
                if !shaders.contains(&id) {
                    shaders.push(id);
                }
            }
        }
        for id in shaders {
            for b in shader_bitmaps(set, id) {
                if bitmap_images(set, b) == 0 {
                    continue;
                }
                if !out
                    .iter()
                    .any(|&(k, i, t)| k == PREDICT_BITMAP && i == 0 && t == b)
                {
                    out.push((PREDICT_BITMAP, 0, b));
                }
            }
        }
        out
    }

    /// Writes the BSP and lightmap into `set` as built payloads of the
    /// tags `sbsp` and `ltmp` (existing slots), each section's raw block as
    /// a new raw resource, and adds the BSP section. Returns its index in
    /// [`TagSet::bsps`].
    pub fn write(&self, set: &mut TagSet, sbsp: TagId, ltmp: TagId) -> Result<usize> {
        let clusters = self.clusters();
        if clusters > 512 {
            return Err(Error::TooBig {
                what: "cluster count",
                value: clusters as u32,
                max: 512,
            });
        }
        let mut cluster_elements = Block::zeroed(CLUSTER * clusters);
        let empty = Section::default();
        for c in 0..clusters {
            let s = self.sections.get(c).unwrap_or(&empty);
            let at = CLUSTER * c;
            let b = &mut cluster_elements.bytes[at..at + CLUSTER];
            // Section info, as retail fills it. Shadow-casting triangles
            // and parts are the opaque (kind 2) ones, the opaque parts
            // those of kind 2 and 3 (decals); transparent parts are
            // neither. V (every retail cluster with a kind 3 or 4 part,
            // 961): `+0x06` and `+0x12` are kind 2's triangles, `+0x08`
            // kind 2's parts, `+0x0e` kind 2 and 3's parts. Rigid,
            // worldspace (+0x14 0); one subpart per part.
            let (vertices, triangles, parts) = (
                s.vertices.len() as u16,
                (s.indices.len() / 3) as u16,
                s.parts.len() as u16,
            );
            let kinds: Vec<u16> = s.parts.iter().map(|p| self.part_kind(p.material)).collect();
            let of = |want: &'static [u16]| {
                s.parts
                    .iter()
                    .zip(&kinds)
                    .filter(move |(_, k)| want.contains(k))
            };
            let shadow_triangles: usize = of(&[PART_KIND])
                .map(|(p, _)| usize::from(p.count) / 3)
                .sum();
            let shadow_parts = of(&[PART_KIND]).count() as u16;
            let opaque_parts = of(&[PART_KIND, PART_KIND_DECAL]).count() as u16;
            // Opaque vertices (`+0x0c`, no engine use found): every vertex
            // when every part is opaque, else those opaque parts index.
            let opaque_vertices = if opaque_parts == parts {
                vertices
            } else {
                let mut v: Vec<u16> = of(&[PART_KIND, PART_KIND_DECAL])
                    .flat_map(|(p, _)| {
                        s.indices[usize::from(p.first)..usize::from(p.first) + usize::from(p.count)]
                            .iter()
                            .copied()
                    })
                    .collect();
                v.sort_unstable();
                v.dedup();
                v.len() as u16
            };
            put16(b, 0, vertices);
            put16(b, 2, triangles);
            put16(b, 4, parts);
            put16(b, 6, shadow_triangles as u16); // shadow-casting triangles
            put16(b, 8, shadow_parts); // shadow-casting parts
            put16(b, 0xc, opaque_vertices); // opaque vertices
            put16(b, 0xe, opaque_parts); // opaque parts
            put16(b, 0x12, shadow_triangles as u16); // shadow-casting rigid triangles
            if !s.indices.is_empty() {
                put16(b, 0x16, COMPRESSION_FLAGS);
            }
            put16(b, 0x24, parts); // subparts
            if s.page.is_some() && self.lightmap_bitmap != TagId::NULL {
                put16(b, 0x26, LIGHTING_FLAGS);
            }
            // An empty cluster's bounds are inverted, as retail's are.
            let bounds = if s.vertices.is_empty() {
                [f32::MAX, -f32::MAX].repeat(3).try_into().expect("6")
            } else {
                s.bounds()
            };
            for (k, v) in bounds.iter().enumerate() {
                b[0x54 + 4 * k..0x58 + 4 * k].copy_from_slice(&v.to_le_bytes());
            }
            // +0x6c: the cluster's first sky, 0 (E 0x00130f60 reads it
            // first; -1 falls back to +0x6e through a sky-flag check that
            // can pick the BSP's own sky). +0x6d none, as retail.
            b[0x6c] = 0;
            b[0x6d] = 0xff;
            // The sky the camera sees from this cluster (E 0x0014b38d):
            // Halo 1's ([`crate::clusters::cluster_skies`]), sky 0 without.
            b[0x6e] = self.visibility.skies.get(c).copied().unwrap_or(0) as u8;
            b[0x6f] = 0xff;
            b[0x70] = 0xff;
            b[0x71] = 0xff;
            for o in [0x72, 0x74, 0x76, 0x7c] {
                put16(b, o, 0xffff);
            }
            // Background sound and sound environment ([`Sound`]).
            if let Some(&[bg, env]) = self.sound.clusters.get(c) {
                put16(b, 0x72, bg as u16);
                put16(b, 0x74, env as u16);
            }
            put16(b, 0x7e, 0);
            // Ambience and reverb sound cluster 0 (E 0x0018d7b0,
            // 0x0018dd10: indexed unchecked).
            put16(b, 0x78, 0);
            put16(b, 0x7a, 0);
            // The structure checksum, as every retail cluster repeats it.
            put32(b, 0x94, self.checksum);
            // The portals on its boundary (u16 each), which the flood
            // crosses to the clusters it draws (E 0x001728ba).
            if let Some(list) = self.visibility.clusters.get(c).filter(|l| !l.is_empty()) {
                let bytes: Vec<u8> = list.iter().flat_map(|p| p.to_le_bytes()).collect();
                cluster_elements.block((at + 0x8c) as u32, list.len() as u32, Block::new(bytes));
            }
            let b = &mut cluster_elements.bytes[at..at + CLUSTER];
            // A cluster with no geometry (a Halo 1 cluster the portals pass
            // through) gets a raw block that is only the header and an
            // empty section struct, as retail's do (V: 95 of the disc's
            // 4,191 clusters, each 0x50 bytes with no resources).
            let predicted = self.predicted(set, c, sbsp);
            let mut list = Block::zeroed(PREDICTED_RESOURCE * predicted.len());
            for (k, &(kind, index, tag)) in predicted.iter().enumerate() {
                put16(&mut list.bytes, PREDICTED_RESOURCE * k, kind);
                put16(&mut list.bytes, PREDICTED_RESOURCE * k + 2, index);
                list.tag_id((PREDICTED_RESOURCE * k + 4) as u32, tag);
            }
            let (block, table) = raw_block(s, |m| {
                let from = self.materials.get(usize::from(m)).copied().unwrap_or(0);
                self.templates
                    .part_kinds
                    .get(from)
                    .copied()
                    .unwrap_or(PART_KIND)
            });
            put32(b, 0x2c, block.len() as u32);
            put32(b, 0x30, SECTION as u32);
            put32(b, 0x34, (block.len() - RAW_FRAME - SECTION) as u32);
            put16(b, 0x44, 0x24);
            b[0x46] = 0;
            b[0x47] = 1;
            put32(b, 0x48, u32::MAX);
            let raw = set.add_raw(block, 0x200);
            let base = at as u32;
            cluster_elements
                .raw(base + 0x28, RawRef::Local(raw))
                .tag_id(base + 0x40, sbsp);
            if !table.is_empty() {
                cluster_elements.block(base + 0x38, table.len() as u32, Block::new(table.concat()));
            }
            if !predicted.is_empty() {
                cluster_elements.block(base + 0x84, predicted.len() as u32, list);
            }
        }

        let count = self.materials.len().max(1);
        let mut materials = Block::zeroed(MATERIAL * count);
        for (k, m) in materials
            .bytes
            .as_chunks_mut::<MATERIAL>()
            .0
            .iter_mut()
            .enumerate()
        {
            let from = self.materials.get(k).copied().unwrap_or(0);
            let head = self
                .templates
                .material_heads
                .get(from)
                .ok_or(Error::Invalid("no such base material"))?;
            m[..16].copy_from_slice(head);
            m[0x1c] = 0xff;
        }
        let mut collision_materials =
            Block::zeroed(COLLISION_MATERIAL * self.collision_materials.max(1));
        for m in collision_materials
            .bytes
            .as_chunks_mut::<COLLISION_MATERIAL>()
            .0
        {
            m.copy_from_slice(&self.templates.collision_material);
            put16(m, 0xa, 0xffff);
        }
        let mut leaves = Block::zeroed(8 * self.collision.leaves.len());
        for (k, l) in leaves.bytes.as_chunks_mut::<8>().0.iter_mut().enumerate() {
            let c = self.sound.leaves.get(k).copied().unwrap_or(0);
            put16(l, 0, if usize::from(c) < clusters { c } else { 0 });
        }

        let mut root = Block::zeroed(SBSP_ROOT);
        root.put_u32(0x08, self.checksum);
        root.block(
            0x0c,
            self.collision_materials.max(1) as u32,
            collision_materials,
        );
        root.block(0x14, 1, collision_block(self.collision));
        root.put_f32(0x1c, 0.0)
            .put_f32(0x20, 0.0)
            .put_f32(0x1c0, 0.0);
        root.block(0x2c, self.collision.leaves.len() as u32, leaves);
        for (k, v) in self.bounds.iter().enumerate() {
            root.put_f32(0x34 + 4 * k, *v);
        }
        let vis = self.visibility();
        root.block(0x54, vis.len() as u32, Block::new(vis));
        root.block(0x9c, clusters as u32, cluster_elements);
        let portals = &self.visibility.portals;
        if !portals.is_empty() {
            let mut block = Block::new(self.visibility.portal_bytes());
            for (k, p) in portals.iter().enumerate() {
                if !p.vertices.is_empty() {
                    block.block(
                        (crate::clusters::PORTAL * k + 0x1c) as u32,
                        p.vertices.len() as u32,
                        Block::new(f32s(p.vertices.iter().flatten().copied())),
                    );
                }
            }
            root.block(0x5c, portals.len() as u32, block);
        }
        root.block(0xa4, count as u32, materials);
        root.block(
            0x1e0,
            self.mopp.len() as u32,
            Block::new(self.mopp.to_vec()).aligned(16),
        );
        let [x0, x1, y0, y1, z0, z1] = self.bounds;
        for (k, v) in [x0, y0, z0, x1, y1, z1].iter().enumerate() {
            root.put_f32(0x1ec + 4 * k, *v);
        }
        root.block(0x21c, 1, Block::zeroed(ELEMENT_21C));
        root.block(0x224, 1, self.audibility());
        root.block(0x94, 1, Block::zeroed(ELEMENT_94));
        for (field, block) in [
            (0xd4, self.background_palette()),
            (0xdc, self.environment_palette()),
        ] {
            if let Some((n, b)) = block {
                root.block(field, n, b);
            }
        }
        root.block(0x148, 1, self.sound_cluster());
        root.block(0x150, 1, self.sound_cluster());
        root.block(0x234, 1, Block::zeroed(ELEMENT_234));

        // Render info per cluster: i16 page, i8 palette -1 (the pages are
        // colour, not palette indices), a zero byte.
        let mut info = Vec::with_capacity(4 * clusters);
        for c in 0..clusters {
            let page = self
                .sections
                .get(c)
                .and_then(|s| s.page)
                .filter(|_| self.lightmap_bitmap != TagId::NULL);
            let page = page.unwrap_or(u16::MAX);
            info.extend(page.to_le_bytes());
            info.extend([0xff, 0x00]);
        }
        let mut group = Block::zeroed(LIGHTMAP_GROUP);
        group.put_u32(4, self.checksum);
        group.tag_ref(0x18, self.lightmap_bitmap);
        group.block(0x28, clusters as u32, Block::new(info));
        let mut light = Block::zeroed(LTMP_ROOT);
        light.put_f32(0x34, 0.0);
        light.block(0x80, 1, group);

        for (id, payload) in [(sbsp, root), (ltmp, light)] {
            let tag = set
                .tag_mut(id)
                .ok_or(Error::Invalid("the base has no tag in the BSP's slot"))?;
            tag.payload = map::build::TagPayload::Built(payload);
        }
        Ok(set.add_bsp(sbsp, Some(ltmp)))
    }
}

/// A palette block of `entries`, `len` bytes each, made by `entry`, with
/// each element's tag reference (`+0x20`) a fixup; `None` for none.
fn palette(
    entries: &[(String, TagId)],
    len: usize,
    entry: fn(&str, TagId) -> Vec<u8>,
) -> Option<(u32, Block)> {
    if entries.is_empty() {
        return None;
    }
    let bytes: Vec<u8> = entries.iter().flat_map(|(n, id)| entry(n, *id)).collect();
    let mut b = Block::new(bytes);
    for (k, (_, id)) in entries.iter().enumerate() {
        b.tag_ref((k * len + 0x20) as u32, *id);
    }
    Some((entries.len() as u32, b))
}
