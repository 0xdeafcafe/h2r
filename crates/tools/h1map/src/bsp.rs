//! A structure BSP's region and the parts of its `sbsp` tag the converter
//! reads: the region header, a few root fields and the collision BSP.
//!
//! Layouts and their evidence are in `docs/halo1/map-format.md`,
//! "Structure BSPs". Everything here was read off the data (strides from the
//! spacing of consecutive blocks, semantics from where retail spawn points
//! land); no community definition file was used.

use crate::format::*;
use crate::reader::H1Map;
use crate::space::{Space, f32_at, i32_at, vec3};
use crate::{Error, Result};

/// Bytes in a BSP region's header.
pub const REGION_HEADER_LEN: usize = 0x18;
/// `sbsp`, closing the region header.
pub const SBSP: u32 = u32::from_be_bytes(*b"sbsp");

/// `sbsp` root offsets.
pub mod root {
    /// Lightmap bitmap (`bitm` reference).
    pub const LIGHTMAPS_BITMAP: usize = 0x00;
    /// Collision materials block, [`super::MATERIAL_LEN`] bytes each.
    pub const COLLISION_MATERIALS: usize = 0xa4;
    /// Collision BSP block, [`super::COLLISION_BSP_LEN`] bytes each.
    pub const COLLISION_BSP: usize = 0xb0;
    /// World bounds: x, y and z (min, max) pairs.
    pub const WORLD_BOUNDS: usize = 0xc8;
    /// Lightmaps block (one per lightmap page, each with materials).
    pub const LIGHTMAPS: usize = 0x104;
    /// Clusters block.
    pub const CLUSTERS: usize = 0x134;
    /// Cluster-to-cluster visibility raw data.
    pub const CLUSTER_VISIBILITY: usize = 0x140;
    /// Bytes in the root struct.
    pub const LEN: usize = 0x288;
}

/// Bytes per collision material (a shader reference and padding).
pub const MATERIAL_LEN: usize = 0x14;
/// Bytes per collision BSP element: eight blocks.
pub const COLLISION_BSP_LEN: usize = 0x60;

/// The region header, at the start of each BSP region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionHeader {
    /// Address of the `sbsp` root struct, inside the region.
    pub sbsp_address: u32,
    /// The two count/address pairs that follow: lightmap material vertex
    /// data (rendered, then lightmap vertices), kept as stored.
    pub vertex_blocks: [Block; 2],
}

/// A structure BSP: its region, header and root struct.
#[derive(Debug, Clone)]
pub struct StructureBsp<'a> {
    /// The region, at its load address.
    pub space: Space<'a>,
    /// The region header.
    pub header: RegionHeader,
    /// The `sbsp` root struct's bytes.
    pub root: &'a [u8],
}

impl H1Map {
    /// A structure BSP's region, parsed down to its root struct.
    pub fn structure_bsp(&self, bsp: &BspRef) -> Result<StructureBsp<'_>> {
        StructureBsp::new(Space::new(self.bsp_bytes(bsp)?, bsp.address))
    }
}

impl<'a> StructureBsp<'a> {
    /// Parses a region loaded at `space.base`.
    pub fn new(space: Space<'a>) -> Result<Self> {
        let h = space.bytes;
        let u = |at| u32_at(h, at).unwrap_or(0);
        if h.len() < REGION_HEADER_LEN || u(0x14) != SBSP {
            return Err(Error::Invalid(
                "BSP region header has no 'sbsp' at 0x14".into(),
            ));
        }
        let header = RegionHeader {
            sbsp_address: u(0),
            vertex_blocks: [Block::read(h, 4).unwrap(), Block::read(h, 0xc).unwrap()],
        };
        let root = space.at(header.sbsp_address, root::LEN)?;
        Ok(Self {
            space,
            header,
            root,
        })
    }

    /// World bounds: `[x min, x max, y min, y max, z min, z max]`.
    pub fn world_bounds(&self) -> [f32; 6] {
        std::array::from_fn(|i| f32_at(self.root, root::WORLD_BOUNDS + 4 * i))
    }

    /// The lightmap bitmap reference.
    pub fn lightmaps_bitmap(&self) -> TagRef {
        TagRef::read(self.root, root::LIGHTMAPS_BITMAP).unwrap()
    }

    /// The count of a root block (clusters, lightmaps, ...).
    pub fn block_count(&self, at: usize) -> u32 {
        Block::read(self.root, at).map_or(0, |b| b.count)
    }

    /// One little-endian bit row per cluster, padded to whole words.
    pub fn cluster_visibility(&self) -> Result<&'a [u8]> {
        let at = root::CLUSTER_VISIBILITY;
        let len = u32_at(self.root, at).unwrap_or(0) as usize;
        if len == 0 {
            return Ok(&[]);
        }
        let n = self.block_count(root::CLUSTERS) as usize;
        if n.checked_mul(n.div_ceil(32) * 4) != Some(len) {
            return Err(Error::Invalid(format!(
                "BSP visibility has {len} bytes for {n} clusters"
            )));
        }
        self.space.at(u32_at(self.root, at + 0xc).unwrap_or(0), len)
    }

    /// The collision materials' shader references.
    pub fn collision_materials(&self) -> Result<Vec<TagRef>> {
        Ok(self
            .space
            .block_at(self.root, root::COLLISION_MATERIALS, MATERIAL_LEN)?
            .into_iter()
            .map(|e| TagRef::read(e, 0).unwrap())
            .collect())
    }

    /// The collision BSPs (retail maps have one).
    pub fn collision_bsps(&self) -> Result<Vec<CollisionBsp>> {
        self.space
            .block_at(self.root, root::COLLISION_BSP, COLLISION_BSP_LEN)?
            .into_iter()
            .map(|e| CollisionBsp::read(&self.space, e))
            .collect()
    }
}

/// A 3D node's child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Child {
    /// Another node.
    Node(u32),
    /// A leaf: open space.
    Leaf(u32),
    /// Nothing: solid.
    Solid,
}

impl Child {
    /// From the stored `i32`: `-1` solid, bit 31 a leaf, else a node.
    pub fn from_raw(v: i32) -> Self {
        if v == -1 {
            Self::Solid
        } else if v < 0 {
            Self::Leaf(v as u32 & 0x7fff_ffff)
        } else {
            Self::Node(v as u32)
        }
    }
}

/// A 3D node (12 bytes): plane, then the child on the plane's negative
/// side, then the child on its positive (or zero) side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node3d {
    /// Plane index.
    pub plane: u32,
    /// Child where `n.p - d < 0`.
    pub back: Child,
    /// Child where `n.p - d >= 0`.
    pub front: Child,
}

/// A leaf (8 bytes): flags, its 2D reference run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leaf {
    /// Flags (`u16`).
    pub flags: u16,
    /// Number of 2D references.
    pub ref_count: u16,
    /// First 2D reference.
    pub first_ref: u32,
}

/// A 2D reference (8 bytes): a plane (bit 31 flips it) and a 2D node or,
/// with bit 31 set, a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ref2d {
    /// Plane, as stored (bit 31 = flipped).
    pub plane: u32,
    /// 2D node or surface, as stored (bit 31 = surface).
    pub node: u32,
}

/// A 2D node (20 bytes): line `a*x + b*y - d`, then two children (bit 31 =
/// surface).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node2d {
    /// `a, b, d`.
    pub plane: [f32; 3],
    /// Child on the negative side, as stored.
    pub back: u32,
    /// Child on the positive side, as stored.
    pub front: u32,
}

/// A surface (12 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surface {
    /// Plane, as stored (bit 31 = flipped).
    pub plane: u32,
    /// First edge of its ring.
    pub first_edge: u32,
    /// Flags.
    pub flags: u8,
    /// Breakable surface index, `-1` none.
    pub breakable_surface: i8,
    /// Collision material index.
    pub material: u16,
}

/// An edge (24 bytes): six `i32`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    /// Start vertex.
    pub start_vertex: u32,
    /// End vertex.
    pub end_vertex: u32,
    /// Next edge round the left surface.
    pub forward_edge: u32,
    /// Next edge round the right surface.
    pub reverse_edge: u32,
    /// Left surface.
    pub left_surface: u32,
    /// Right surface (`u32::MAX` none).
    pub right_surface: u32,
}

/// A vertex (16 bytes): position and one edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    /// Position.
    pub point: [f32; 3],
    /// An edge that touches it.
    pub first_edge: u32,
}

/// A collision BSP: 3D tree, leaves, 2D trees and the surface mesh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollisionBsp {
    /// 3D nodes; node 0 is the root.
    pub nodes: Vec<Node3d>,
    /// Planes, `[nx, ny, nz, d]` with `n.p - d = 0`.
    pub planes: Vec<[f32; 4]>,
    /// Leaves.
    pub leaves: Vec<Leaf>,
    /// 2D references.
    pub refs: Vec<Ref2d>,
    /// 2D nodes.
    pub nodes_2d: Vec<Node2d>,
    /// Surfaces.
    pub surfaces: Vec<Surface>,
    /// Edges.
    pub edges: Vec<Edge>,
    /// Vertices.
    pub vertices: Vec<Vertex>,
}

/// Element sizes of the collision BSP's eight blocks, in order.
pub const COLLISION_STRIDES: [usize; 8] = [12, 16, 8, 8, 20, 12, 24, 16];

impl CollisionBsp {
    /// Reads the element `e` (eight block fields) in `space`.
    pub fn read(space: &Space, e: &[u8]) -> Result<Self> {
        let b = |i: usize| space.block_at(e, i * BLOCK_LEN, COLLISION_STRIDES[i]);
        let u = |c: &[u8], at| u32_at(c, at).unwrap();
        Ok(Self {
            nodes: b(0)?
                .into_iter()
                .map(|c| Node3d {
                    plane: u(c, 0),
                    back: Child::from_raw(i32_at(c, 4)),
                    front: Child::from_raw(i32_at(c, 8)),
                })
                .collect(),
            planes: b(1)?
                .into_iter()
                .map(|c| std::array::from_fn(|i| f32_at(c, 4 * i)))
                .collect(),
            leaves: b(2)?
                .into_iter()
                .map(|c| Leaf {
                    flags: u16_at(c, 0).unwrap(),
                    ref_count: u16_at(c, 2).unwrap(),
                    first_ref: u(c, 4),
                })
                .collect(),
            refs: b(3)?
                .into_iter()
                .map(|c| Ref2d {
                    plane: u(c, 0),
                    node: u(c, 4),
                })
                .collect(),
            nodes_2d: b(4)?
                .into_iter()
                .map(|c| Node2d {
                    plane: vec3(c, 0),
                    back: u(c, 12),
                    front: u(c, 16),
                })
                .collect(),
            surfaces: b(5)?
                .into_iter()
                .map(|c| Surface {
                    plane: u(c, 0),
                    first_edge: u(c, 4),
                    flags: c[8],
                    breakable_surface: c[9] as i8,
                    material: u16_at(c, 10).unwrap(),
                })
                .collect(),
            edges: b(6)?
                .into_iter()
                .map(|c| Edge {
                    start_vertex: u(c, 0),
                    end_vertex: u(c, 4),
                    forward_edge: u(c, 8),
                    reverse_edge: u(c, 12),
                    left_surface: u(c, 16),
                    right_surface: u(c, 20),
                })
                .collect(),
            vertices: b(7)?
                .into_iter()
                .map(|c| Vertex {
                    point: vec3(c, 0),
                    first_edge: u(c, 12),
                })
                .collect(),
        })
    }

    /// The leaf holding `p`, or `None` when `p` is in solid space.
    pub fn leaf_at(&self, p: [f32; 3]) -> Option<u32> {
        let mut child = if self.nodes.is_empty() {
            Child::Solid
        } else {
            Child::Node(0)
        };
        // A tree, so a walk is at most as long as the node count.
        for _ in 0..=self.nodes.len() {
            match child {
                Child::Solid => return None,
                Child::Leaf(l) => return Some(l),
                Child::Node(n) => {
                    let node = self.nodes.get(n as usize)?;
                    let [x, y, z, d] = *self.planes.get(node.plane as usize)?;
                    let s = x * p[0] + y * p[1] + z * p[2] - d;
                    child = if s >= 0.0 { node.front } else { node.back };
                }
            }
        }
        None
    }

    /// The vertices round a surface's ring. The rule: from the first edge,
    /// an edge whose right surface is this one gives its end vertex and goes
    /// on to its reverse edge; any other gives its start vertex and goes on
    /// to its forward edge, until the first edge comes round again. `None`
    /// when an index is out of range or the ring doesn't close.
    pub fn surface_ring(&self, surface: usize) -> Option<Vec<usize>> {
        let s = self.surfaces.get(surface)?;
        let first = s.first_edge as usize;
        let mut e = first;
        let mut out = Vec::new();
        for _ in 0..=self.edges.len() {
            let edge = self.edges.get(e)?;
            if edge.right_surface as usize == surface {
                out.push(edge.end_vertex as usize);
                e = edge.reverse_edge as usize;
            } else {
                out.push(edge.start_vertex as usize);
                e = edge.forward_edge as usize;
            }
            if e == first {
                return Some(out);
            }
        }
        None
    }

    /// A surface's plane with its flip applied.
    pub fn surface_plane(&self, surface: usize) -> Option<[f32; 4]> {
        let raw = self.surfaces.get(surface)?.plane;
        let p = *self.planes.get((raw & 0x7fff_ffff) as usize)?;
        Some(if raw & 0x8000_0000 != 0 {
            p.map(|v| -v)
        } else {
            p
        })
    }

    /// Structural checks: every index in range, planes unit length, every
    /// surface ring closes and lies on its plane. Returns the problems.
    pub fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        let (nodes, leaves) = (self.nodes.len() as u32, self.leaves.len() as u32);
        let (planes, refs) = (self.planes.len() as u32, self.refs.len() as u32);
        let (n2, surfaces) = (self.nodes_2d.len() as u32, self.surfaces.len() as u32);
        let (edges, verts) = (self.edges.len() as u32, self.vertices.len() as u32);
        let child_ok = |c: Child| match c {
            Child::Node(n) => n < nodes,
            Child::Leaf(l) => l < leaves,
            Child::Solid => true,
        };
        let child2_ok = |v: u32| {
            if v & 0x8000_0000 != 0 {
                v & 0x7fff_ffff < surfaces
            } else {
                v < n2
            }
        };
        for (i, n) in self.nodes.iter().enumerate() {
            if n.plane >= planes || !child_ok(n.back) || !child_ok(n.front) {
                bad.push(format!("node {i} out of range"));
            }
        }
        for (i, p) in self.planes.iter().enumerate() {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            if (len - 1.0).abs() > 1e-3 || !p[3].is_finite() {
                bad.push(format!("plane {i} not unit"));
            }
        }
        for (i, l) in self.leaves.iter().enumerate() {
            if l.ref_count > 0 && l.first_ref + l.ref_count as u32 > refs {
                bad.push(format!("leaf {i} refs out of range"));
            }
        }
        for (i, r) in self.refs.iter().enumerate() {
            if r.plane & 0x7fff_ffff >= planes || !child2_ok(r.node) {
                bad.push(format!("2d ref {i} out of range"));
            }
        }
        for (i, n) in self.nodes_2d.iter().enumerate() {
            if !child2_ok(n.back) || !child2_ok(n.front) {
                bad.push(format!("2d node {i} out of range"));
            }
        }
        for (i, e) in self.edges.iter().enumerate() {
            let s_ok = |s: u32| s == u32::MAX || s < surfaces;
            if e.start_vertex >= verts
                || e.end_vertex >= verts
                || e.forward_edge >= edges
                || e.reverse_edge >= edges
                || !s_ok(e.left_surface)
                || !s_ok(e.right_surface)
            {
                bad.push(format!("edge {i} out of range"));
            }
        }
        if !bad.is_empty() {
            return bad;
        }
        for i in 0..self.surfaces.len() {
            let (Some(ring), Some(p)) = (self.surface_ring(i), self.surface_plane(i)) else {
                bad.push(format!("surface {i}: ring does not close"));
                continue;
            };
            for v in ring {
                let q = self.vertices[v].point;
                let dist = p[0] * q[0] + p[1] * q[1] + p[2] * q[2] - p[3];
                if dist.abs() > 1e-2 {
                    bad.push(format!("surface {i}: vertex {v} is {dist} off its plane"));
                    break;
                }
            }
        }
        bad
    }
}

/// Bytes per lightmap element in the root's lightmaps block.
pub const LIGHTMAP_LEN: usize = 0x20;
/// Bytes per lightmap material.
pub const MATERIAL_ELEM_LEN: usize = 0x100;
/// Bytes per render surface (three `u16` vertex indices).
pub const RENDER_SURFACE_LEN: usize = 6;
/// Bytes per compressed rendered vertex.
pub const RENDER_VERTEX_LEN: usize = 32;
/// Bytes per compressed lightmap vertex.
pub const LIGHTMAP_VERTEX_LEN: usize = 8;
/// Root offset of the render surfaces block.
pub const RENDER_SURFACES: usize = 0xf8;

/// A unit vector packed as signed 11:11:10 (x bits 0-10, y 11-21, z
/// 22-31), each over its field's largest magnitude.
pub fn unpack_normal(v: u32) -> [f32; 3] {
    let field = |shift: u32, bits: u32| {
        let raw = (v >> shift) & ((1 << bits) - 1);
        // Sign-extend.
        let s = ((raw << (32 - bits)) as i32) >> (32 - bits);
        s as f32 / ((1 << (bits - 1)) - 1) as f32
    };
    [field(0, 11), field(11, 11), field(22, 10)]
}

/// A rendered vertex (32 bytes compressed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderVertex {
    /// Position.
    pub position: [f32; 3],
    /// Normal, binormal and tangent, packed 11:11:10 (see [`unpack_normal`]).
    pub frame: [u32; 3],
    /// Texture coordinate.
    pub texcoord: [f32; 2],
}

/// A lightmap vertex (8 bytes compressed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightmapVertex {
    /// Incident light direction, packed 11:11:10.
    pub normal: u32,
    /// Lightmap texture coordinate, `i16 / 32767`.
    pub texcoord: [f32; 2],
}

/// One lightmap material: a shader and the triangles drawn with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    /// The shader.
    pub shader: TagRef,
    /// First render surface (triangle).
    pub first_surface: u32,
    /// Triangle count.
    pub surface_count: u32,
    /// Vertices; triangles index into these.
    pub vertices: Vec<RenderVertex>,
    /// Lightmap vertices, one per vertex; empty when the lightmap has no
    /// bitmap.
    pub lightmap_vertices: Vec<LightmapVertex>,
}

/// One lightmap: an index into the lightmap bitmap's images, and its
/// materials.
#[derive(Debug, Clone, PartialEq)]
pub struct Lightmap {
    /// Image in the lightmap bitmap, `-1` none (unlit surfaces).
    pub bitmap: i16,
    /// Materials.
    pub materials: Vec<Material>,
}

impl<'a> StructureBsp<'a> {
    /// The render surfaces: three vertex indices into their material's
    /// vertices.
    pub fn render_surfaces(&self) -> Result<Vec<[u16; 3]>> {
        Ok(self
            .space
            .block_at(self.root, RENDER_SURFACES, RENDER_SURFACE_LEN)?
            .into_iter()
            .map(|c| std::array::from_fn(|i| u16_at(c, 2 * i).unwrap()))
            .collect())
    }

    /// The lightmaps and their materials, with each material's vertices.
    pub fn lightmaps(&self) -> Result<Vec<Lightmap>> {
        let mut out = Vec::new();
        for l in self
            .space
            .block_at(self.root, root::LIGHTMAPS, LIGHTMAP_LEN)?
        {
            let mut materials = Vec::new();
            for m in self.space.block_at(l, 0x14, MATERIAL_ELEM_LEN)? {
                materials.push(self.material(m)?);
            }
            out.push(Lightmap {
                bitmap: u16_at(l, 0).unwrap() as i16,
                materials,
            });
        }
        Ok(out)
    }

    fn material(&self, m: &[u8]) -> Result<Material> {
        let u = |at| u32_at(m, at).unwrap();
        let count = u(0xb4) as usize;
        // The compressed vertices: a data reference at 0xec (size, flags,
        // file offset, address): rendered vertices, then lightmap vertices
        // when the size has room for them.
        let (size, address) = (u(0xec) as usize, u(0xf8));
        let lit = count * (RENDER_VERTEX_LEN + LIGHTMAP_VERTEX_LEN);
        if size != lit && size != count * RENDER_VERTEX_LEN {
            return Err(Error::Invalid(format!(
                "material vertex data is {size:#x} bytes for {count} vertices"
            )));
        }
        let data = self.space.at(address, size)?;
        let (rendered, lightmap) = data.split_at(count * RENDER_VERTEX_LEN);
        Ok(Material {
            shader: TagRef::read(m, 0).unwrap(),
            first_surface: u(0x14),
            surface_count: u(0x18),
            vertices: rendered
                .as_chunks::<RENDER_VERTEX_LEN>()
                .0
                .iter()
                .map(|c| RenderVertex {
                    position: vec3(c, 0),
                    frame: std::array::from_fn(|i| u32_at(c, 12 + 4 * i).unwrap()),
                    texcoord: [f32_at(c, 24), f32_at(c, 28)],
                })
                .collect(),
            lightmap_vertices: lightmap
                .as_chunks::<LIGHTMAP_VERTEX_LEN>()
                .0
                .iter()
                .map(|c| LightmapVertex {
                    normal: u32_at(c, 0).unwrap(),
                    texcoord: std::array::from_fn(|i| {
                        u16_at(c, 4 + 2 * i).unwrap() as i16 as f32 / 32767.0
                    }),
                })
                .collect(),
        })
    }
}
