//! The converted structure BSP (`sbsp`) and lightmap (`ltmp`), written as
//! [`map::build`] block trees for a built BSP section.
//!
//! What every field is and why it may be empty is in
//! docs/game/h1-multiplayer.md, "The converted structure BSP"; the render
//! geometry layout is docs/formats/structure-bsp.md. In short: Halo 1's
//! rendered triangles become one or more clusters (a cluster's section
//! holds at most 65535 vertices and indices, so a large level takes
//! several), every cluster sees every other one, every collision leaf is in
//! cluster 0, and the lightmap has one group with no bitmap. Materials
//! name shaders the base map already has.
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
/// Part kind written: a drawn kind (0 and 5 are not drawn, E
/// `0x000410fc`); which drawn kind means what is not known.
pub const PART_KIND: u16 = 2;
/// Part flag bit 3: a triangle list.
pub const PART_TRIANGLE_LIST: u16 = 0x8;
/// Size of the element at `sbsp+0x21c` written (one, zeroed; the engine
/// reads its first word, E `0x00210f0f`).
pub const ELEMENT_21C: usize = 0x40;
/// Size of the element at `sbsp+0x234` written (one, zeroed; the engine
/// reads `+0x20`, E `0x0003f927`).
pub const ELEMENT_234: usize = 0x30;

/// Most vertices or indices one section holds (`u16` counts and indices).
pub const SECTION_MAX: usize = 0xffff;

/// Vertex stream types written, with their strides
/// (docs/formats/structure-bsp.md, "Stream types").
pub const STREAMS: [(u8, u8); 4] = [(0x01, 12), (0x18, 8), (0x1b, 12), (0x1f, 4)];

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
}

/// The triangles drawn with one Halo 1 shader.
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    /// The Halo 1 shader's tag name.
    pub shader: String,
    /// Vertices.
    pub vertices: Vec<Vertex>,
    /// Triangles, as indices into `vertices`.
    pub triangles: Vec<[u32; 3]>,
}

/// Halo 1's render geometry, one mesh per lightmap material.
/// `shader_name` names a Halo 1 shader reference's tag.
pub fn meshes(
    bsp: &StructureBsp,
    shader_name: impl Fn(h1map::TagRef) -> String,
) -> Result<Vec<Mesh>> {
    let surfaces = bsp.render_surfaces()?;
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
                })
                .collect();
            out.push(Mesh {
                shader: shader_name(m.shader),
                vertices,
                triangles: tris.iter().map(|t| t.map(u32::from)).collect(),
            });
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

/// Packs meshes into sections of at most [`SECTION_MAX`] vertices and
/// indices. `material[i]` is mesh `i`'s structure material. A mesh can
/// span sections; a part never does.
pub fn pack(meshes: &[Mesh], material: &[u16]) -> Vec<Section> {
    const MAX_INDICES: usize = SECTION_MAX / 3 * 3;
    let mut out = vec![Section::default()];
    for (m, &mat) in meshes.iter().zip(material) {
        let mut remap: Vec<Option<u16>> = vec![None; m.vertices.len()];
        for t in &m.triangles {
            let s = out.last_mut().expect("never empty");
            let new = t.iter().filter(|&&i| remap[i as usize].is_none()).count();
            if s.vertices.len() + new > SECTION_MAX || s.indices.len() + 3 > MAX_INDICES {
                out.push(Section::default());
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
pub fn raw_block(s: &Section) -> (Vec<u8>, Vec<[u8; 16]>) {
    let mut parts = Vec::with_capacity(s.parts.len() * PART);
    let mut subparts = Vec::with_capacity(s.parts.len() * 8);
    for (i, p) in s.parts.iter().enumerate() {
        let mut e = [0u8; PART];
        put16(&mut e, 0, PART_KIND);
        put16(&mut e, 2, PART_TRIANGLE_LIST);
        put16(&mut e, 4, p.material);
        put16(&mut e, 6, p.first);
        put16(&mut e, 8, p.count);
        put16(&mut e, 0xa, i as u16);
        put16(&mut e, 0xc, 1);
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

    // (kind, field, element size or descriptor index, bytes)
    let resources: [(u8, u16, u16, Vec<u8>); 8] = [
        (0, 0x00, PART as u16, parts),
        (0, 0x08, 8, subparts),
        (0, 0x20, 2, indices),
        (0, 0x38, 0x20, descriptors),
        (2, 0x38, 0, positions),
        (2, 0x38, 1, texcoords),
        (2, 0x38, 2, frames),
        (2, 0x38, 3, lightmap),
    ];
    let mut section = vec![0u8; SECTION];
    let mut data = Vec::new();
    let mut table = Vec::new();
    for (kind, field, secondary, bytes) in &resources {
        if *kind == 0 {
            put32(
                &mut section,
                usize::from(*field),
                (bytes.len() / usize::from(*secondary)) as u32,
            );
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
}

impl Templates {
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
                    .collect()
            },
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
            put16(b, 0, s.vertices.len() as u16);
            put16(b, 2, (s.indices.len() / 3) as u16);
            put16(b, 4, s.parts.len() as u16);
            put16(b, 0xc, s.vertices.len() as u16);
            for (k, v) in s.bounds().iter().enumerate() {
                b[0x54 + 4 * k..0x58 + 4 * k].copy_from_slice(&v.to_le_bytes());
            }
            b[0x6e] = 0; // sky: the scenario's first (E 0x0014b38d)
            b[0x6f] = 0xff;
            b[0x70] = 0xff;
            b[0x71] = 0xff;
            for o in [0x72, 0x74, 0x76, 0x7c] {
                put16(b, o, 0xffff);
            }
            put16(b, 0x7e, 0);
            if s.indices.is_empty() {
                // No raw block: pointer none, everything else zero.
                put32(b, 0x28, u32::MAX);
                put32(b, 0x40, u32::MAX);
                continue;
            }
            let (block, table) = raw_block(s);
            put32(b, 0x2c, block.len() as u32);
            put32(b, 0x30, SECTION as u32);
            put32(b, 0x34, (block.len() - 12 - SECTION) as u32);
            put16(b, 0x44, 0x24);
            b[0x46] = 0;
            b[0x47] = 1;
            put32(b, 0x48, u32::MAX);
            let raw = set.add_raw(block, 0x200);
            let base = at as u32;
            cluster_elements
                .raw(base + 0x28, RawRef::Local(raw))
                .block(base + 0x38, table.len() as u32, Block::new(table.concat()))
                .tag_id(base + 0x40, sbsp);
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
        for l in leaves.bytes.as_chunks_mut::<8>().0 {
            put16(l, 0, 0); // cluster 0
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
        root.block(0x234, 1, Block::zeroed(ELEMENT_234));

        let mut info = Vec::with_capacity(4 * clusters);
        for _ in 0..clusters {
            info.extend([0xff, 0xff, 0xff, 0x00]);
        }
        let mut group = Block::zeroed(LIGHTMAP_GROUP);
        group.put_u32(4, self.checksum);
        group.tag_ref(0x18, TagId::NULL);
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
