//! Halo 2 collision models (`coll`) and physics models (`phmo`).
//!
//! Hand-written fixed codecs, deliberately independent of `schema.rs`, so this
//! module merges without touching the generated records. Layouts and their
//! provenance are recorded in `docs/formats/collision.md`; engine addresses are in
//! `docs/engine/engine-map.md` ("Collision and physics model layouts").
//!
//! Both tags are one contiguous payload: a fixed root followed by every child
//! block. A block is `{count, address}` where the address is a virtual address
//! inside that payload (`address - tag base` is the payload offset).
//!
//! Collision BSPs here are the *object* collision model's BSPs. A structure
//! BSP's (`sbsp`) collision BSP has the same blocks without the leading word;
//! [`Bsp::decode_structure`] reads it (docs/formats/pathfinding.md).
use crate::wire::StringId;
use crate::{Error, Result};
use map::{FourCC, Map, TagQuery};
use std::cell::Cell;
use std::fmt::Write as _;
use std::io::{Read, Seek};

pub const COLL: FourCC = FourCC(*b"coll");
pub const PHMO: FourCC = FourCC(*b"phmo");

/// Decoding a hostile payload may alias blocks over each other. Total decoded
/// bytes are bounded by this multiple of the payload length.
const ALIAS_BUDGET_FACTOR: u64 = 64;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn i16_at(b: &[u8], o: usize) -> i16 {
    u16_at(b, o) as i16
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}
fn f32s<const N: usize>(b: &[u8], o: usize) -> [f32; N] {
    std::array::from_fn(|i| f32_at(b, o + i * 4))
}

/// A tag payload and the virtual address it was loaded at.
struct Payload<'a> {
    bytes: &'a [u8],
    base: u32,
    budget: Cell<u64>,
}
impl<'a> Payload<'a> {
    fn new(bytes: &'a [u8], base: u32) -> Self {
        let budget = Cell::new(bytes.len() as u64 * ALIAS_BUDGET_FACTOR + 4096);
        Self {
            bytes,
            base,
            budget,
        }
    }
    /// The `{count, address}` pair at `offset`, resolved to `count * stride`
    /// bytes. A zero count never dereferences its address.
    fn block(&self, offset: usize, stride: usize) -> Result<(usize, &'a [u8])> {
        let h = self
            .bytes
            .get(offset..offset + 8)
            .ok_or(Error::Invalid("block descriptor outside payload"))?;
        let (count, address) = (u32_at(h, 0), u32_at(h, 4));
        if count == 0 {
            return Ok((0, &[]));
        }
        let size = u64::from(count) * stride as u64;
        let bounds = Error::Bounds { address, size };
        let start = address
            .checked_sub(self.base)
            .ok_or(Error::Bounds { address, size })? as usize;
        let end = start
            .checked_add(size as usize)
            .ok_or(Error::Bounds { address, size })?;
        let s = self.bytes.get(start..end).ok_or(bounds)?;
        let left = self
            .budget
            .get()
            .checked_sub(size)
            .ok_or(Error::Invalid("aliased blocks exceed decode budget"))?;
        self.budget.set(left);
        Ok((count as usize, s))
    }
    /// A block's bytes as `stride`-sized chunks.
    fn chunks(&self, offset: usize, stride: usize) -> Result<std::slice::ChunksExact<'a, u8>> {
        Ok(self.block(offset, stride)?.1.chunks_exact(stride))
    }
}

// ---------------------------------------------------------------------------
// coll
// ---------------------------------------------------------------------------

/// A BSP 3D node child: the engine reads `i32 >> 8` of bytes `+1`/`+4`, so each
/// child is a signed 24-bit value, bit 23 marks a leaf and `-1` is "none".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Child {
    None,
    Node(u32),
    Leaf(u32),
}
impl Child {
    fn from_raw24(v: u32) -> Self {
        if v == 0xff_ffff {
            Self::None
        } else if v & 0x80_0000 != 0 {
            Self::Leaf(v & 0x7f_ffff)
        } else {
            Self::Node(v)
        }
    }
    pub fn raw24(self) -> u32 {
        match self {
            Self::None => 0xff_ffff,
            Self::Node(i) => i,
            Self::Leaf(i) => i | 0x80_0000,
        }
    }
}

/// 8 bytes: `+0` plane (u16), `+2` back child (24 bit), `+5` front child (24 bit).
/// The engine selects the child at `side * 3 + 1` where `side` is 1 when the
/// point is on the positive side of the plane, so `+2` is the back child.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node3d {
    pub plane: u16,
    pub back: Child,
    pub front: Child,
}
/// `n . p - d == 0`. Stored `[nx, ny, nz, d]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane(pub [f32; 4]);
/// 4 bytes: `+0` flags (bit 0 changes the engine's inside/outside classification),
/// `+1` reference count, `+2` first reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub flags: u8,
    pub ref_count: u8,
    pub first_ref: u16,
}
/// 4 bytes: a plane (bit 15 flips it) and either a 2D node (bit 15 clear) or a
/// surface (bit 15 set, low 15 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bsp2dRef {
    pub plane: u16,
    pub node: u16,
}
/// 2D node child: bit 15 set means "surface index in the low 15 bits".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Child2d {
    Node(u16),
    Surface(u16),
}
impl Child2d {
    /// The stored `u16`: bit 15 marks a surface.
    pub fn raw(self) -> u16 {
        match self {
            Self::Node(i) => i,
            Self::Surface(i) => i | 0x8000,
        }
    }
    fn from_raw(v: u16) -> Self {
        if v & 0x8000 != 0 {
            Self::Surface(v & 0x7fff)
        } else {
            Self::Node(v)
        }
    }
}
impl Bsp2dRef {
    pub fn target(self) -> Child2d {
        Child2d::from_raw(self.node)
    }
}
/// 16 bytes: a 2D plane `a*x + b*y - d` (floats `+0,+4,+8`), back child `+0xc`,
/// front child `+0xe`. The engine descends `+0xc` when the distance is below the
/// query radius and continues with `+0xe` unless the point is wholly behind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node2d {
    pub plane: [f32; 3],
    pub back: Child2d,
    pub front: Child2d,
}
/// 8 bytes: `+0` plane (bit 15 flips), `+2` first edge, `+4` flags (bit 3 is
/// tested against the breakable-surface mask), `+5` breakable surface (0xff =
/// none), `+6` material index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surface {
    pub plane: u16,
    pub first_edge: u16,
    pub flags: u8,
    pub breakable_surface: u8,
    pub material: u16,
}
/// 12 bytes. Walking a surface's ring (see [`Bsp::surface_ring`]): if
/// `right_surface == surface` use `end_vertex` and `reverse_edge`, otherwise
/// `start_vertex` and `forward_edge`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub start_vertex: u16,
    pub end_vertex: u16,
    pub forward_edge: u16,
    pub reverse_edge: u16,
    pub left_surface: u16,
    pub right_surface: u16,
}
/// 16 bytes: position (floats `+0,+4,+8`) and `+0xc` an edge index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub point: [f32; 3],
    pub edge: u32,
}

/// One collision BSP: a 0x44-byte element, a leading word followed by eight
/// blocks (the engine addresses it as the pointer to `+4`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bsp {
    /// `+0`: varies 0..=7 on retail data; meaning not established.
    pub unknown_00: u32,
    pub nodes: Vec<Node3d>,
    pub planes: Vec<Plane>,
    pub leaves: Vec<Leaf>,
    pub refs: Vec<Bsp2dRef>,
    pub nodes_2d: Vec<Node2d>,
    pub surfaces: Vec<Surface>,
    pub edges: Vec<Edge>,
    pub vertices: Vec<Vertex>,
}
pub const BSP_SIZE: usize = 0x44;

/// Region permutation: `{name, bsps, bsp_physics}`, 0x14 bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Permutation {
    pub name: StringId,
    pub bsps: Vec<Bsp>,
    /// `+0xc`: number of 0x70-byte runtime Havok slots the engine pairs with
    /// the first `n` BSPs (zero in nearly every tag; contents are not decoded).
    pub bsp_physics_count: u32,
}
/// 12 bytes: `{name, permutations}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub name: StringId,
    pub permutations: Vec<Permutation>,
}
/// 0x14 bytes. Layout inferred from data only (see docs/formats/collision.md).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathfindingSphere {
    pub node: i16,
    pub flags: u16,
    pub center: [f32; 3],
    pub radius: f32,
}
/// 12 bytes: `{name, region, parent, next sibling, first child}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollNode {
    pub name: StringId,
    pub region: i16,
    pub parent: i16,
    pub next_sibling: i16,
    pub first_child: i16,
}

/// A decoded `coll` tag (root size 0x34).
#[derive(Clone, Debug, PartialEq)]
pub struct Collision {
    /// Root `+0..+0x14`: five words, zero on every retail tag.
    pub reserved: [u32; 5],
    /// Block at `+0x14`, 4-byte elements holding a string ID each; surfaces'
    /// material indices refer to this list.
    pub materials: Vec<StringId>,
    pub regions: Vec<Region>,
    pub pathfinding_spheres: Vec<PathfindingSphere>,
    pub nodes: Vec<CollNode>,
}
pub const COLL_ROOT_SIZE: usize = 0x34;

fn decode_bsp(p: &Payload, at: usize) -> Result<Bsp> {
    // `at` is the element's payload offset; its blocks start at `+4`.
    let unknown_00 = u32_at(
        p.bytes
            .get(at..at + 4)
            .ok_or(Error::Invalid("collision BSP outside payload"))?,
        0,
    );
    decode_bsp_blocks(p, at + 4, unknown_00)
}

/// The eight blocks of a collision BSP, starting at payload offset `blocks`.
fn decode_bsp_blocks(p: &Payload, blocks: usize, unknown_00: u32) -> Result<Bsp> {
    let off = |i: usize| blocks + i * 8;
    let nodes = p
        .chunks(off(0), 8)?
        .map(|c| Node3d {
            plane: u16_at(c, 0),
            back: Child::from_raw24(u32_at(c, 1) >> 8),
            front: Child::from_raw24(u32_at(c, 4) >> 8),
        })
        .collect();
    let planes = p.chunks(off(1), 16)?.map(|c| Plane(f32s(c, 0))).collect();
    let leaves = p
        .chunks(off(2), 4)?
        .map(|c| Leaf {
            flags: c[0],
            ref_count: c[1],
            first_ref: u16_at(c, 2),
        })
        .collect();
    let refs = p
        .chunks(off(3), 4)?
        .map(|c| Bsp2dRef {
            plane: u16_at(c, 0),
            node: u16_at(c, 2),
        })
        .collect();
    let nodes_2d = p
        .chunks(off(4), 16)?
        .map(|c| Node2d {
            plane: f32s(c, 0),
            back: Child2d::from_raw(u16_at(c, 12)),
            front: Child2d::from_raw(u16_at(c, 14)),
        })
        .collect();
    let surfaces = p
        .chunks(off(5), 8)?
        .map(|c| Surface {
            plane: u16_at(c, 0),
            first_edge: u16_at(c, 2),
            flags: c[4],
            breakable_surface: c[5],
            material: u16_at(c, 6),
        })
        .collect();
    let edges = p
        .chunks(off(6), 12)?
        .map(|c| Edge {
            start_vertex: u16_at(c, 0),
            end_vertex: u16_at(c, 2),
            forward_edge: u16_at(c, 4),
            reverse_edge: u16_at(c, 6),
            left_surface: u16_at(c, 8),
            right_surface: u16_at(c, 10),
        })
        .collect();
    let vertices = p
        .chunks(off(7), 16)?
        .map(|c| Vertex {
            point: f32s(c, 0),
            edge: u32_at(c, 12),
        })
        .collect();
    Ok(Bsp {
        unknown_00,
        nodes,
        planes,
        leaves,
        refs,
        nodes_2d,
        surfaces,
        edges,
        vertices,
    })
}

impl Collision {
    /// Decode from a `coll` payload loaded at virtual address `base`.
    pub fn decode(bytes: &[u8], base: u32) -> Result<Self> {
        if bytes.len() < COLL_ROOT_SIZE {
            return Err(Error::RecordSize {
                actual: bytes.len(),
                expected: COLL_ROOT_SIZE,
            });
        }
        let p = Payload::new(bytes, base);
        let reserved = std::array::from_fn(|i| u32_at(bytes, i * 4));
        let materials = p.chunks(0x14, 4)?.map(|c| StringId(u32_at(c, 0))).collect();
        let mut regions = Vec::new();
        for r in p.chunks(0x1c, 12)? {
            let name = StringId(u32_at(r, 0));
            let (pc, pb) = block_of(&p, r, 4, 0x14)?;
            let mut permutations = Vec::with_capacity(pc);
            for pe in pb.as_chunks::<0x14>().0 {
                let name = StringId(u32_at(pe, 0));
                let (bc, bb) = block_of(&p, pe, 4, BSP_SIZE)?;
                let mut bsps = Vec::with_capacity(bc);
                for i in 0..bc {
                    // payload offset of this BSP element
                    let at = bb.as_ptr() as usize - bytes.as_ptr() as usize + i * BSP_SIZE;
                    bsps.push(decode_bsp(&p, at)?);
                }
                let phys = block_of(&p, pe, 0xc, 0x70)?.0 as u32;
                permutations.push(Permutation {
                    name,
                    bsps,
                    bsp_physics_count: phys,
                });
            }
            regions.push(Region { name, permutations });
        }
        let pathfinding_spheres = p
            .chunks(0x24, 0x14)?
            .map(|c| PathfindingSphere {
                node: i16_at(c, 0),
                flags: u16_at(c, 2),
                center: f32s(c, 4),
                radius: f32_at(c, 0x10),
            })
            .collect();
        let nodes = p
            .chunks(0x2c, 12)?
            .map(|c| CollNode {
                name: StringId(u32_at(c, 0)),
                region: i16_at(c, 4),
                parent: i16_at(c, 6),
                next_sibling: i16_at(c, 8),
                first_child: i16_at(c, 10),
            })
            .collect();
        Ok(Self {
            reserved,
            materials,
            regions,
            pathfinding_spheres,
            nodes,
        })
    }

    /// Read and decode a named `coll` tag from a map.
    pub fn read<'a, R: Read + Seek>(
        map: &mut Map<R>,
        query: impl Into<TagQuery<'a>>,
    ) -> Result<Self> {
        let tag = map.tag(query)?;
        if tag.group != COLL {
            return Err(Error::UnsupportedGroup(tag.group));
        }
        let (id, address) = (tag.id, tag.location.ok_or(map::Error::NoTagData)?.address);
        Self::decode(&map.read_tag_bytes(id)?, address)
    }

    pub fn bsps(&self) -> impl Iterator<Item = &Bsp> {
        self.regions
            .iter()
            .flat_map(|r| r.permutations.iter())
            .flat_map(|p| p.bsps.iter())
    }

    /// Structural checks: every index in range, plane normals unit length,
    /// BSP trees acyclic. Returns every problem found (empty means valid).
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (ri, r) in self.regions.iter().enumerate() {
            for (pi, p) in r.permutations.iter().enumerate() {
                if p.bsp_physics_count as usize > p.bsps.len() {
                    out.push(format!(
                        "region {ri} permutation {pi}: more bsp physics slots than bsps"
                    ));
                }
                for (bi, b) in p.bsps.iter().enumerate() {
                    for m in b.validate(self.materials.len()) {
                        out.push(format!("region {ri} permutation {pi} bsp {bi}: {m}"));
                    }
                }
            }
        }
        let n = self.nodes.len();
        let ok = |v: i16| v == -1 || (v >= 0 && (v as usize) < n);
        for (i, node) in self.nodes.iter().enumerate() {
            if !(ok(node.parent) && ok(node.next_sibling) && ok(node.first_child)) {
                out.push(format!("node {i}: parent/sibling/child out of range"));
            }
            if !(node.region == -1
                || (node.region >= 0 && (node.region as usize) < self.regions.len()))
            {
                out.push(format!("node {i}: region {} out of range", node.region));
            }
        }
        for (i, s) in self.pathfinding_spheres.iter().enumerate() {
            if !(s.node == -1 || (s.node >= 0 && (s.node as usize) < n)) {
                out.push(format!(
                    "pathfinding sphere {i}: node {} out of range",
                    s.node
                ));
            }
            if !(s.radius.is_finite() && s.radius >= 0.0 && s.center.iter().all(|c| c.is_finite()))
            {
                out.push(format!("pathfinding sphere {i}: non-finite or negative"));
            }
        }
        out
    }

    /// Wavefront OBJ of every BSP surface (fan-triangulated polygons), one object
    /// per region/permutation/BSP.
    ///
    /// Output is bounded: ring walking is budgeted per BSP (a legitimate BSP
    /// walks each edge at most twice) and the whole export is capped at
    /// [`MAX_OBJ_TRIANGLES`], returning an error instead of an unbounded file.
    pub fn to_obj(&self) -> Result<String> {
        let mut s = String::from("# h2 collision export (collision BSP surfaces)\n");
        let mut base = 1usize;
        let mut triangles = 0usize;
        for (ri, r) in self.regions.iter().enumerate() {
            for (pi, p) in r.permutations.iter().enumerate() {
                for (bi, b) in p.bsps.iter().enumerate() {
                    let _ = writeln!(s, "o region{ri}_perm{pi}_bsp{bi}");
                    for v in &b.vertices {
                        let _ = writeln!(s, "v {} {} {}", v.point[0], v.point[1], v.point[2]);
                    }
                    let mut budget = b.ring_budget();
                    for si in 0..b.surfaces.len() {
                        let Some((ring, _)) = b.walk_ring(si, &mut budget) else {
                            return Err(Error::Invalid("export exceeds the edge-walk budget"));
                        };
                        triangles += ring.len().saturating_sub(2);
                        if triangles > MAX_OBJ_TRIANGLES {
                            return Err(Error::Invalid("export exceeds the triangle budget"));
                        }
                        for k in 1..ring.len().saturating_sub(1) {
                            let _ = writeln!(
                                s,
                                "f {} {} {}",
                                base + ring[0],
                                base + ring[k],
                                base + ring[k + 1]
                            );
                        }
                    }
                    base += b.vertices.len();
                }
            }
        }
        Ok(s)
    }
}

/// Upper bound on triangles in one OBJ export (real models are far below this).
pub const MAX_OBJ_TRIANGLES: usize = 2_000_000;
/// Upper bound on validation work counted in elementary checks.
pub const MAX_VALIDATE_WORK: u64 = 1 << 26;

/// A `{count, address}` pair at `offset` inside an element slice (not the root).
fn block_of<'a>(
    p: &Payload<'a>,
    element: &[u8],
    offset: usize,
    stride: usize,
) -> Result<(usize, &'a [u8])> {
    let at = element.as_ptr() as usize - p.bytes.as_ptr() as usize + offset;
    p.block(at, stride)
}

/// The eight block fields of a collision BSP, without the object-collision
/// element's leading word: the structure BSP (`sbsp+0x14`) stores elements
/// of exactly this size.
pub const BSP_BLOCKS_SIZE: usize = 0x40;
/// Element sizes of the eight blocks, in order.
pub const BSP_STRIDES: [usize; 8] = [8, 16, 4, 4, 16, 8, 12, 16];

impl Bsp {
    /// Decodes the eight block fields at `at` in `bytes` (loaded at `base`).
    /// The structure BSP's collision element is this; an object collision
    /// BSP element is a word followed by it. `unknown_00` is left zero.
    pub fn decode_blocks(bytes: &[u8], base: u32, at: usize) -> Result<Self> {
        if at < 4 {
            // decode_bsp reads the leading word at `at - 4`; give it room.
            let mut padded = vec![0u8; 4];
            padded.extend_from_slice(bytes);
            let mut bsp = decode_bsp(&Payload::new(&padded, base.wrapping_sub(4)), at)?;
            bsp.unknown_00 = 0;
            return Ok(bsp);
        }
        let mut bsp = decode_bsp(&Payload::new(bytes, base), at - 4)?;
        bsp.unknown_00 = 0;
        Ok(bsp)
    }

    /// Encodes the BSP as one payload loaded at `base`: the eight block
    /// fields ([`BSP_BLOCKS_SIZE`] bytes), then each block's elements in
    /// order. [`Bsp::decode_blocks`] at offset 0 reads it back.
    pub fn encode_blocks(&self, base: u32) -> Vec<u8> {
        let mut parts: [Vec<u8>; 8] = Default::default();
        for n in &self.nodes {
            parts[0].extend(n.plane.to_le_bytes());
            parts[0].extend(&n.back.raw24().to_le_bytes()[..3]);
            parts[0].extend(&n.front.raw24().to_le_bytes()[..3]);
        }
        for p in &self.planes {
            p.0.iter().for_each(|v| parts[1].extend(v.to_le_bytes()));
        }
        for l in &self.leaves {
            parts[2].extend([l.flags, l.ref_count]);
            parts[2].extend(l.first_ref.to_le_bytes());
        }
        for r in &self.refs {
            parts[3].extend(r.plane.to_le_bytes());
            parts[3].extend(r.node.to_le_bytes());
        }
        for n in &self.nodes_2d {
            n.plane
                .iter()
                .for_each(|v| parts[4].extend(v.to_le_bytes()));
            parts[4].extend(n.back.raw().to_le_bytes());
            parts[4].extend(n.front.raw().to_le_bytes());
        }
        for s in &self.surfaces {
            parts[5].extend(s.plane.to_le_bytes());
            parts[5].extend(s.first_edge.to_le_bytes());
            parts[5].extend([s.flags, s.breakable_surface]);
            parts[5].extend(s.material.to_le_bytes());
        }
        for e in &self.edges {
            for v in [
                e.start_vertex,
                e.end_vertex,
                e.forward_edge,
                e.reverse_edge,
                e.left_surface,
                e.right_surface,
            ] {
                parts[6].extend(v.to_le_bytes());
            }
        }
        for v in &self.vertices {
            v.point
                .iter()
                .for_each(|f| parts[7].extend(f.to_le_bytes()));
            parts[7].extend(v.edge.to_le_bytes());
        }
        let mut out = vec![0u8; BSP_BLOCKS_SIZE];
        for (i, part) in parts.iter().enumerate() {
            let count = (part.len() / BSP_STRIDES[i]) as u32;
            let address = if count == 0 {
                0
            } else {
                base + out.len() as u32
            };
            out[i * 8..i * 8 + 4].copy_from_slice(&count.to_le_bytes());
            out[i * 8 + 4..i * 8 + 8].copy_from_slice(&address.to_le_bytes());
            out.extend(part);
        }
        out
    }
}

impl Bsp {
    /// Element size of a structure BSP's collision BSP: the same eight blocks
    /// as a `coll` BSP without the leading word (V: on every disc BSP the
    /// first child block starts `0x40` bytes after the element).
    pub const STRUCTURE_SIZE: usize = 0x40;

    /// Decode a structure BSP's (`sbsp`) collision BSP: the eight blocks
    /// start at payload offset `at` of a payload loaded at `base`.
    /// `unknown_00` is 0, as there is no leading word.
    pub fn decode_structure(bytes: &[u8], base: u32, at: usize) -> Result<Self> {
        if at
            .checked_add(Self::STRUCTURE_SIZE)
            .is_none_or(|end| end > bytes.len())
        {
            return Err(Error::Invalid("collision BSP outside payload"));
        }
        decode_bsp_blocks(&Payload::new(bytes, base), at, 0)
    }

    /// Vertex indices around a surface, following the engine's edge walk.
    /// Bounded by the edge count, so a malformed ring cannot loop.
    pub fn surface_ring(&self, surface: usize) -> Vec<usize> {
        let mut ring = Vec::new();
        let Some(s) = self.surfaces.get(surface) else {
            return ring;
        };
        let (first, mut e) = (s.first_edge, s.first_edge);
        for _ in 0..=self.edges.len() {
            let Some(edge) = self.edges.get(usize::from(e)) else {
                break;
            };
            let right = usize::from(edge.right_surface) == surface;
            let v = if right {
                edge.end_vertex
            } else {
                edge.start_vertex
            };
            ring.push(usize::from(v).min(self.vertices.len().saturating_sub(1)));
            e = if right {
                edge.reverse_edge
            } else {
                edge.forward_edge
            };
            if e == first {
                break;
            }
        }
        ring
    }

    /// Check indices, plane normals and tree structure. `materials` is the owning
    /// tag's material count.
    pub fn validate(&self, materials: usize) -> Vec<String> {
        let mut out = Vec::new();
        let plane_ok = |v: u16| usize::from(v & 0x7fff) < self.planes.len();
        for (i, p) in self.planes.iter().enumerate() {
            let [x, y, z, d] = p.0;
            let len = (x * x + y * y + z * z).sqrt();
            if !(len.is_finite() && d.is_finite() && (len - 1.0).abs() <= PLANE_UNIT_TOLERANCE) {
                out.push(format!("plane {i}: normal length {len} is not unit"));
            }
        }
        for (i, n) in self.nodes.iter().enumerate() {
            if usize::from(n.plane) >= self.planes.len() {
                out.push(format!("node {i}: plane {} out of range", n.plane));
            }
            for c in [n.back, n.front] {
                match c {
                    Child::Node(j) if j as usize >= self.nodes.len() => {
                        out.push(format!("node {i}: child node {j} out of range"))
                    }
                    Child::Leaf(j) if j as usize >= self.leaves.len() => {
                        out.push(format!("node {i}: child leaf {j} out of range"))
                    }
                    _ => {}
                }
            }
        }
        if let Some(cycle) = tree_cycle(self.nodes.len(), |i| {
            let n = self.nodes[i];
            [n.back, n.front].into_iter().filter_map(|c| {
                if let Child::Node(j) = c {
                    Some(j as usize)
                } else {
                    None
                }
            })
        }) {
            out.push(format!("3d tree has a cycle through node {cycle}"));
        }
        for (i, l) in self.leaves.iter().enumerate() {
            // An empty leaf stores first_ref = 0xffff.
            if l.ref_count != 0
                && usize::from(l.first_ref) + usize::from(l.ref_count) > self.refs.len()
            {
                out.push(format!("leaf {i}: reference run outside refs"));
            }
        }
        for (i, r) in self.refs.iter().enumerate() {
            if !plane_ok(r.plane) {
                out.push(format!("ref {i}: plane {} out of range", r.plane & 0x7fff));
            }
            match r.target() {
                Child2d::Node(j) if usize::from(j) >= self.nodes_2d.len() => {
                    out.push(format!("ref {i}: 2d node {j} out of range"))
                }
                Child2d::Surface(j) if usize::from(j) >= self.surfaces.len() => {
                    out.push(format!("ref {i}: surface {j} out of range"))
                }
                _ => {}
            }
        }
        for (i, n) in self.nodes_2d.iter().enumerate() {
            if !n.plane.iter().all(|v| v.is_finite()) {
                out.push(format!("2d node {i}: non-finite plane"));
            }
            for c in [n.back, n.front] {
                match c {
                    Child2d::Node(j) if usize::from(j) >= self.nodes_2d.len() => {
                        out.push(format!("2d node {i}: child node {j} out of range"))
                    }
                    Child2d::Surface(j) if usize::from(j) >= self.surfaces.len() => {
                        out.push(format!("2d node {i}: surface {j} out of range"))
                    }
                    _ => {}
                }
            }
        }
        if let Some(cycle) = tree_cycle(self.nodes_2d.len(), |i| {
            let n = self.nodes_2d[i];
            [n.back, n.front].into_iter().filter_map(|c| {
                if let Child2d::Node(j) = c {
                    Some(usize::from(j))
                } else {
                    None
                }
            })
        }) {
            out.push(format!("2d tree has a cycle through node {cycle}"));
        }
        for (i, s) in self.surfaces.iter().enumerate() {
            if !plane_ok(s.plane) {
                out.push(format!(
                    "surface {i}: plane {} out of range",
                    s.plane & 0x7fff
                ));
            }
            if usize::from(s.first_edge) >= self.edges.len() {
                out.push(format!(
                    "surface {i}: first edge {} out of range",
                    s.first_edge
                ));
            }
            if usize::from(s.material) >= materials {
                out.push(format!("surface {i}: material {} out of range", s.material));
            }
        }
        for (i, e) in self.edges.iter().enumerate() {
            for v in [e.start_vertex, e.end_vertex] {
                if usize::from(v) >= self.vertices.len() {
                    out.push(format!("edge {i}: vertex {v} out of range"));
                }
            }
            for v in [e.forward_edge, e.reverse_edge] {
                if usize::from(v) >= self.edges.len() {
                    out.push(format!("edge {i}: edge {v} out of range"));
                }
            }
            for v in [e.left_surface, e.right_surface] {
                if v != 0xffff && usize::from(v) >= self.surfaces.len() {
                    out.push(format!("edge {i}: surface {v} out of range"));
                }
            }
        }
        for (i, v) in self.vertices.iter().enumerate() {
            if !v.point.iter().all(|c| c.is_finite()) {
                out.push(format!("vertex {i}: non-finite position"));
            }
            if v.edge as usize >= self.edges.len() {
                out.push(format!("vertex {i}: edge {} out of range", v.edge));
            }
        }
        // Every surface's edge ring must close within the edge count. All rings
        // share one step budget: legitimate data walks each edge at most twice.
        let mut budget = self.ring_budget();
        for si in 0..self.surfaces.len() {
            let Some((ring, closed)) = self.walk_ring(si, &mut budget) else {
                out.push("edge rings exceed the walk budget (malformed edges)".into());
                break;
            };
            if !(closed && ring.len() >= 3) {
                out.push(format!("surface {si}: edge ring does not close"));
            } else if let Some(err) = self.plane_error(si, &ring)
                && err > SURFACE_PLANAR_TOLERANCE
            {
                out.push(format!("surface {si}: vertices are {err} off its plane"));
            }
        }
        out
    }

    fn ring_budget(&self) -> usize {
        4 * self.edges.len() + self.surfaces.len() + 64
    }

    /// Walk one surface's ring, spending `budget` per step. `None` means the
    /// budget ran out; otherwise the vertices and whether the ring closed
    /// (returned to its first edge, every edge on it bordering the surface).
    fn walk_ring(&self, surface: usize, budget: &mut usize) -> Option<(Vec<usize>, bool)> {
        let mut ring = Vec::new();
        let Some(s) = self.surfaces.get(surface) else {
            return Some((ring, false));
        };
        let first = s.first_edge;
        let mut e = first;
        for _ in 0..=self.edges.len() {
            *budget = budget.checked_sub(1)?;
            let Some(edge) = self.edges.get(usize::from(e)) else {
                return Some((ring, false));
            };
            let right = usize::from(edge.right_surface) == surface;
            if !right && usize::from(edge.left_surface) != surface {
                return Some((ring, false));
            }
            let v = if right {
                edge.end_vertex
            } else {
                edge.start_vertex
            };
            ring.push(usize::from(v).min(self.vertices.len().saturating_sub(1)));
            e = if right {
                edge.reverse_edge
            } else {
                edge.forward_edge
            };
            if e == first {
                return Some((ring, true));
            }
        }
        Some((ring, false))
    }

    fn plane_error(&self, surface: usize, ring: &[usize]) -> Option<f32> {
        let s = self.surfaces.get(surface)?;
        let Plane([nx, ny, nz, d]) = *self.planes.get(usize::from(s.plane & 0x7fff))?;
        ring.iter()
            .map(|&v| {
                self.vertices
                    .get(v)
                    .map(|v| (nx * v.point[0] + ny * v.point[1] + nz * v.point[2] - d).abs())
            })
            .try_fold(0.0f32, |m, e| e.map(|e| m.max(e)))
    }

    /// Largest `|n . p - d|` over a surface's ring vertices against its plane
    /// (the plane's flip bit does not change the distance).
    pub fn surface_plane_error(&self, surface: usize) -> Option<f32> {
        let s = self.surfaces.get(surface)?;
        let Plane([nx, ny, nz, d]) = *self.planes.get(usize::from(s.plane & 0x7fff))?;
        let ring = self.surface_ring(surface);
        ring.iter()
            .map(|&v| {
                self.vertices
                    .get(v)
                    .map(|v| (nx * v.point[0] + ny * v.point[1] + nz * v.point[2] - d).abs())
            })
            .try_fold(0.0f32, |m, e| e.map(|e| m.max(e)))
    }
}

/// Tolerance on `| |n| - 1 |` for stored plane normals (f32 round-off).
pub const PLANE_UNIT_TOLERANCE: f32 = 1e-3;
/// Largest distance a surface's vertices may lie from its plane (world units).
pub const SURFACE_PLANAR_TOLERANCE: f32 = 1e-2;
/// Slack for polyhedron vertex/plane incidence (shape-local units).
pub const HULL_TOLERANCE: f32 = 1e-3;

/// Iterative three-colour DFS over every node: returns a node on a cycle.
/// Shared (DAG) children are permitted; only back edges are errors.
fn tree_cycle<I: Iterator<Item = usize>>(n: usize, children: impl Fn(usize) -> I) -> Option<usize> {
    // 0 = unvisited, 1 = on the current path, 2 = done
    let mut state = vec![0u8; n];
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, children(root))];
        state[root] = 1;
        while let Some((node, iter)) = stack.last_mut() {
            let node = *node;
            match iter.next() {
                Some(c) if c >= n => {} // range errors are reported separately
                Some(c) => match state[c] {
                    0 => {
                        state[c] = 1;
                        stack.push((c, children(c)));
                    }
                    1 => return Some(c),
                    _ => {}
                },
                None => {
                    state[node] = 2;
                    stack.pop();
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// phmo
// ---------------------------------------------------------------------------

/// Shape type of a `{type, index}` reference, as switched on by the engine's
/// shape resolver (`0x001eaf30`). Names for 5 and 6 are positional only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeType {
    Sphere,
    Pill,
    Box,
    Triangle,
    Polyhedron,
    /// Type 5, 0xb0-byte elements at root `+0x50`. Never present on the retail disc.
    Type5,
    /// Type 6, 0x20-byte elements at root `+0x108`.
    Type6,
    List,
    Mopp,
    Unknown(u16),
}
impl ShapeType {
    pub fn from_u16(v: u16) -> Self {
        match v {
            0 => Self::Sphere,
            1 => Self::Pill,
            2 => Self::Box,
            3 => Self::Triangle,
            4 => Self::Polyhedron,
            5 => Self::Type5,
            6 => Self::Type6,
            0xe => Self::List,
            0xf => Self::Mopp,
            n => Self::Unknown(n),
        }
    }
}
/// `{type u16, index u16}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShapeRef {
    pub kind: ShapeType,
    pub raw_type: u16,
    pub index: u16,
}
impl ShapeRef {
    fn at(b: &[u8], o: usize) -> Self {
        let raw_type = u16_at(b, o);
        Self {
            kind: ShapeType::from_u16(raw_type),
            raw_type,
            index: u16_at(b, o + 2),
        }
    }
}

/// Fields common to the 0x20-byte header of spheres, pills, boxes, triangles,
/// polyhedra (the Havok shape object follows at `+0x20`).
///
/// Only `name` and `material` have evidence (the latter: a valid index into the
/// model's materials on every retail shape). The remaining fields are named by
/// offset because nothing consumes them in the code examined: the floats hold
/// plausible values (`+8` is 1.0, `+0xc` 0.1..0.9, `+0x10` 0.2..0.6 and so on)
/// but which is which is a guess, so none is given a semantic name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeHeader {
    pub name: StringId,
    /// `+4`, index into the model's materials (checked on all retail shapes).
    pub material: u16,
    pub unknown_06: u16,
    pub unknown_08: f32,
    pub unknown_0c: f32,
    pub unknown_10: f32,
    pub unknown_14: f32,
    pub unknown_18: f32,
    pub unknown_1c: u16,
    pub unknown_1e: u8,
    pub unknown_1f: u8,
}
impl ShapeHeader {
    fn floats(&self) -> [f32; 5] {
        [
            self.unknown_08,
            self.unknown_0c,
            self.unknown_10,
            self.unknown_14,
            self.unknown_18,
        ]
    }
}
fn shape_header(c: &[u8]) -> ShapeHeader {
    ShapeHeader {
        name: StringId(u32_at(c, 0)),
        material: u16_at(c, 4),
        unknown_06: u16_at(c, 6),
        unknown_08: f32_at(c, 8),
        unknown_0c: f32_at(c, 0xc),
        unknown_10: f32_at(c, 0x10),
        unknown_14: f32_at(c, 0x14),
        unknown_18: f32_at(c, 0x18),
        unknown_1c: u16_at(c, 0x1c),
        unknown_1e: c[0x1e],
        unknown_1f: c[0x1f],
    }
}

/// 3x3 rotation (three 16-byte rows, `w` ignored) and a translation, as stored
/// by the transform shape that wraps spheres and boxes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub rotation: [[f32; 3]; 3],
    pub translation: [f32; 3],
}
fn transform(c: &[u8], o: usize) -> Transform {
    Transform {
        rotation: std::array::from_fn(|i| f32s(c, o + i * 16)),
        translation: f32s(c, o + 48),
    }
}

/// 0x80 bytes: header, a sphere shape (`+0x20`, radius `+0x2c`) and its transform
/// shape (`+0x30`, rotation `+0x40`, translation `+0x70`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sphere {
    pub header: ShapeHeader,
    pub radius: f32,
    pub transform: Transform,
}
/// 0x50 bytes: header and a capsule shape: radius `+0x2c`, end points `+0x30`/`+0x40`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pill {
    pub header: ShapeHeader,
    pub radius: f32,
    pub bottom: [f32; 3],
    pub top: [f32; 3],
}
/// 0x90 bytes: header, a box shape (`+0x20`, half extents `+0x30`, convex radius
/// `+0x3c`) and its transform shape (`+0x40`, rotation `+0x50`, translation `+0x80`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxShape {
    pub header: ShapeHeader,
    pub half_extents: [f32; 3],
    pub radius: f32,
    pub transform: Transform,
}
/// 0x100 bytes: header and a convex-vertices shape (see docs/formats/collision.md).
#[derive(Clone, Debug, PartialEq)]
pub struct Polyhedron {
    pub header: ShapeHeader,
    /// `+0x30`.
    pub aabb_half_extents: [f32; 3],
    /// `+0x40`.
    pub aabb_center: [f32; 3],
    /// `+0x54`: groups of four vertices (`hkFourTransposedPoints`).
    pub group_count: u32,
    /// `+0x58` without bit 31: capacity; above 3 the groups live in the
    /// model-wide four-vector block, otherwise inline at `+0x60`.
    pub group_capacity: u32,
    /// `+0xf0`.
    pub vertex_count: u32,
    /// `+0xf8`: planes, consumed from the model-wide plane block in element order.
    pub plane_count: u32,
    /// Resolved vertices (`group_count * 4` slots, trimmed to `vertex_count`).
    pub vertices: Vec<[f32; 3]>,
    /// Resolved plane equations `[nx, ny, nz, w]` with `n.p + w <= 0` inside.
    pub planes: Vec<[f32; 4]>,
}
/// A list child: `{type, index}` plus a runtime word.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListChild {
    pub shape: ShapeRef,
    pub runtime: u32,
}
/// 0x38 bytes: `+0xc` runtime pointer, `+0x10` child count, `+0x14` capacity
/// (above 4 children live in the model-wide child block, else inline at `+0x18`).
#[derive(Clone, Debug, PartialEq)]
pub struct List {
    pub capacity: u32,
    pub children: Vec<ListChild>,
}
/// 0x14 bytes: `+0xc` child `{type, index}` (a list), `+0x10` runtime code pointer.
/// The code bytes come from the model-wide mopp-code block in element order.
#[derive(Clone, Debug, PartialEq)]
pub struct Mopp {
    pub child: ShapeRef,
    /// Code size recorded at `+0x20` of the code header.
    pub code_size: u32,
    pub code_offset: usize,
}
/// 0x90 bytes. Only fields established by data and engine use are named.
#[derive(Clone, Debug, PartialEq)]
pub struct RigidBody {
    /// `+0`: node index, -1 for none.
    pub node: i16,
    /// `+8`.
    pub bounding_sphere_offset: [f32; 3],
    /// `+0x14`.
    pub bounding_sphere_radius: f32,
    /// `+0x38`: the engine overwrites this with the resolved shape pointer.
    pub shape: ShapeRef,
    /// `+0x3c`.
    pub mass: f32,
    /// `+0x40`.
    pub center_of_mass: [f32; 3],
    /// `+0x50`, `+0x60`, `+0x70` (symmetric on all retail data).
    pub inertia_tensor: [[f32; 3]; 3],
    /// Everything else, unmodelled: the element verbatim.
    pub raw: Vec<u8>,
}
/// Materials block element (12 bytes decoded; see docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysMaterial {
    pub name: StringId,
    pub unknown_04: u32,
    pub unknown_08: i16,
    pub unknown_0a: u16,
}
/// An element whose layout is known only as far as its size and name.
#[derive(Clone, Debug, PartialEq)]
pub struct RawElement {
    pub name: StringId,
    pub bytes: Vec<u8>,
}
/// 0x20 bytes at root `+0x108`: the child `{type, index}` at `+0xc` is resolved
/// by the engine through the shape resolver (`0x001eaeb6`..`0x001eaed3`).
#[derive(Clone, Debug, PartialEq)]
pub struct Type6Shape {
    pub child: ShapeRef,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PhysRegion {
    pub name: StringId,
    pub permutations: Vec<PhysPermutation>,
}
/// Physics region permutation, 12 bytes (a name and eight zero bytes on retail data).
#[derive(Clone, Debug, PartialEq)]
pub struct PhysPermutation {
    pub name: StringId,
    pub raw: [u32; 2],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysNode {
    pub name: StringId,
    pub raw: [u16; 4],
}

pub const PHMO_ROOT_SIZE: usize = 0x110;
/// Smallest mopp code run: the 0x20-byte header plus the size word.
pub const MOPP_MIN_RUN: u32 = 0x24;
/// Largest vertex or plane count validated or exported per polyhedron.
pub const MAX_HULL_ELEMENTS: usize = 4096;

/// A decoded `phmo` tag.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicsModel {
    /// Root `+0`.
    pub flags: u32,
    /// Root `+4`.
    pub mass: f32,
    /// Root `+8`, `+0xc` (1.0/1.0 or 2.0/2.0 on retail data).
    pub mass_scales: [f32; 2],
    pub rigid_bodies: Vec<RigidBody>,
    pub materials: Vec<PhysMaterial>,
    pub spheres: Vec<Sphere>,
    pub pills: Vec<Pill>,
    pub boxes: Vec<BoxShape>,
    pub polyhedra: Vec<Polyhedron>,
    pub lists: Vec<List>,
    pub mopps: Vec<Mopp>,
    pub mopp_code: Vec<u8>,
    /// Elements of the type-5 (0xb0) and triangle (0x60) blocks: no retail instance.
    pub type5_count: usize,
    pub triangle_count: usize,
    pub type6: Vec<Type6Shape>,
    pub regions: Vec<PhysRegion>,
    pub nodes: Vec<PhysNode>,
    /// Constraint-like blocks, kept as raw elements: root `+0xb8` (0x94 bytes),
    /// `+0xe8` (0x84) and `+0xf0` (0x78). The first word is a string ID.
    pub constraints_b8: Vec<RawElement>,
    pub constraints_e8: Vec<RawElement>,
    pub constraints_f0: Vec<RawElement>,
    /// Counts of blocks whose element layout is not established (root `+0x28`, `+0x30`).
    pub block_28_count: u32,
    pub block_30_count: u32,
    /// Model-wide block sizes, retained to check cursor consumption.
    pub four_vector_groups: usize,
    pub plane_equations: usize,
    pub list_children: usize,
}

fn raw_elements(p: &Payload, at: usize, stride: usize) -> Result<Vec<RawElement>> {
    Ok(p.chunks(at, stride)?
        .map(|c| RawElement {
            name: StringId(u32_at(c, 0)),
            bytes: c.to_vec(),
        })
        .collect())
}

impl PhysicsModel {
    pub fn decode(bytes: &[u8], base: u32) -> Result<Self> {
        if bytes.len() < PHMO_ROOT_SIZE {
            return Err(Error::RecordSize {
                actual: bytes.len(),
                expected: PHMO_ROOT_SIZE,
            });
        }
        let p = Payload::new(bytes, base);
        let rigid_bodies = p
            .chunks(0x38, 0x90)?
            .map(|c| RigidBody {
                node: i16_at(c, 0),
                bounding_sphere_offset: f32s(c, 8),
                bounding_sphere_radius: f32_at(c, 0x14),
                shape: ShapeRef::at(c, 0x38),
                mass: f32_at(c, 0x3c),
                center_of_mass: f32s(c, 0x40),
                inertia_tensor: std::array::from_fn(|i| f32s(c, 0x50 + i * 16)),
                raw: c.to_vec(),
            })
            .collect();
        // 12-byte elements. A block is followed by fill (0xaaaaaaaa) up to a
        // 16-byte multiple, which is why spans between blocks read 16 or 12.
        let materials = p
            .chunks(0x40, 12)?
            .map(|c| PhysMaterial {
                name: StringId(u32_at(c, 0)),
                unknown_04: u32_at(c, 4),
                unknown_08: i16_at(c, 8),
                unknown_0a: u16_at(c, 10),
            })
            .collect();
        let spheres = p
            .chunks(0x48, 0x80)?
            .map(|c| Sphere {
                header: shape_header(c),
                radius: f32_at(c, 0x2c),
                transform: transform(c, 0x40),
            })
            .collect();
        let pills = p
            .chunks(0x58, 0x50)?
            .map(|c| Pill {
                header: shape_header(c),
                radius: f32_at(c, 0x2c),
                bottom: f32s(c, 0x30),
                top: f32s(c, 0x40),
            })
            .collect();
        let boxes = p
            .chunks(0x60, 0x90)?
            .map(|c| BoxShape {
                header: shape_header(c),
                half_extents: f32s(c, 0x30),
                radius: f32_at(c, 0x3c),
                transform: transform(c, 0x50),
            })
            .collect();

        // Model-wide arrays consumed by element order (engine 0x001ea9c0).
        let (_, four) = p.block(0x78, 0x30)?;
        let (_, plane_bytes) = p.block(0x80, 0x10)?;
        let mut group_cursor = 0usize;
        let mut plane_cursor = 0usize;
        let mut polyhedra = Vec::new();
        for c in p.chunks(0x70, 0x100)? {
            let group_count = u32_at(c, 0x54);
            let capacity = u32_at(c, 0x58) & 0x7fff_ffff;
            let vertex_count = u32_at(c, 0xf0);
            let plane_count = u32_at(c, 0xf8);
            let groups: &[u8] = if capacity > 3 {
                let (s, e) = (
                    group_cursor * 0x30,
                    (group_cursor + group_count as usize) * 0x30,
                );
                group_cursor += group_count as usize;
                four.get(s..e)
                    .ok_or(Error::Invalid("polyhedron four-vector run outside block"))?
            } else {
                let n = group_count as usize * 0x30;
                c.get(0x60..0x60 + n)
                    .ok_or(Error::Invalid("inline four-vector run too long"))?
            };
            let mut vertices = Vec::with_capacity(groups.len() / 12);
            for g in groups.as_chunks::<0x30>().0 {
                for lane in 0..4 {
                    vertices.push([
                        f32_at(g, lane * 4),
                        f32_at(g, 16 + lane * 4),
                        f32_at(g, 32 + lane * 4),
                    ]);
                }
            }
            vertices.truncate(vertex_count as usize);
            let (s, e) = (
                plane_cursor * 16,
                (plane_cursor + plane_count as usize) * 16,
            );
            plane_cursor += plane_count as usize;
            let planes = plane_bytes
                .get(s..e)
                .ok_or(Error::Invalid("polyhedron plane run outside block"))?
                .as_chunks::<16>()
                .0
                .iter()
                .map(|q| f32s(q, 0))
                .collect();
            polyhedra.push(Polyhedron {
                header: shape_header(c),
                aabb_half_extents: f32s(c, 0x30),
                aabb_center: f32s(c, 0x40),
                group_count,
                group_capacity: capacity,
                vertex_count,
                plane_count,
                vertices,
                planes,
            });
        }

        let (_, child_bytes) = p.block(0x98, 8)?;
        let mut child_cursor = 0usize;
        let mut lists = Vec::new();
        for c in p.chunks(0x90, 0x38)? {
            let count = u32_at(c, 0x10) as usize;
            let capacity = u32_at(c, 0x14) & 0x7fff_ffff;
            let src: &[u8] = if capacity > 4 {
                let (s, e) = (child_cursor * 8, (child_cursor + count) * 8);
                child_cursor += count;
                child_bytes
                    .get(s..e)
                    .ok_or(Error::Invalid("list child run outside block"))?
            } else {
                c.get(0x18..0x18 + count * 8)
                    .ok_or(Error::Invalid("inline list children too long"))?
            };
            lists.push(List {
                capacity,
                children: src
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|e| ListChild {
                        shape: ShapeRef::at(e, 0),
                        runtime: u32_at(e, 4),
                    })
                    .collect(),
            });
        }

        let (_, code) = p.block(0xa8, 1)?;
        let mut cursor = 0usize;
        let mut mopps = Vec::new();
        for c in p.chunks(0xa0, 0x14)? {
            let size = code
                .get(cursor + 0x20..cursor + 0x24)
                .map(|s| u32_at(s, 0))
                .ok_or(Error::Invalid("mopp code header outside block"))?;
            // A run is its 0x20-byte header, the size word and code; the size
            // covers all of it, so it must hold at least the header and word, and
            // the (16-aligned) run must lie wholly inside the remaining bytes.
            let aligned = (size as usize)
                .checked_add(0xf)
                .ok_or(Error::Invalid("mopp size"))?
                & !0xf;
            if size < MOPP_MIN_RUN || cursor + aligned > code.len() {
                return Err(Error::Invalid("mopp code run outside its block"));
            }
            mopps.push(Mopp {
                child: ShapeRef::at(c, 0xc),
                code_size: size,
                code_offset: cursor,
            });
            cursor += aligned;
        }

        let regions = {
            let mut v = Vec::new();
            for r in p.chunks(0xc0, 12)? {
                let (n, pb) = block_of(&p, r, 4, 12)?;
                let permutations = pb
                    .as_chunks::<12>()
                    .0
                    .iter()
                    .take(n)
                    .map(|q| PhysPermutation {
                        name: StringId(u32_at(q, 0)),
                        raw: [u32_at(q, 4), u32_at(q, 8)],
                    })
                    .collect();
                v.push(PhysRegion {
                    name: StringId(u32_at(r, 0)),
                    permutations,
                });
            }
            v
        };
        let nodes = p
            .chunks(0xc8, 12)?
            .map(|c| PhysNode {
                name: StringId(u32_at(c, 0)),
                raw: [u16_at(c, 4), u16_at(c, 6), u16_at(c, 8), u16_at(c, 10)],
            })
            .collect();
        Ok(Self {
            flags: u32_at(bytes, 0),
            mass: f32_at(bytes, 4),
            mass_scales: [f32_at(bytes, 8), f32_at(bytes, 12)],
            rigid_bodies,
            materials,
            spheres,
            pills,
            boxes,
            polyhedra,
            lists,
            mopps,
            mopp_code: code.to_vec(),
            type5_count: p.block(0x50, 0xb0)?.0,
            triangle_count: p.block(0x68, 0x60)?.0,
            type6: p
                .chunks(0x108, 0x20)?
                .map(|c| Type6Shape {
                    child: ShapeRef::at(c, 0xc),
                    bytes: c.to_vec(),
                })
                .collect(),
            regions,
            nodes,
            constraints_b8: raw_elements(&p, 0xb8, 0x94)?,
            constraints_e8: raw_elements(&p, 0xe8, 0x84)?,
            constraints_f0: raw_elements(&p, 0xf0, 0x78)?,
            block_28_count: u32_at(bytes, 0x28),
            block_30_count: u32_at(bytes, 0x30),
            four_vector_groups: four.len() / 0x30,
            plane_equations: plane_bytes.len() / 16,
            list_children: child_bytes.len() / 8,
        })
    }

    pub fn read<'a, R: Read + Seek>(
        map: &mut Map<R>,
        query: impl Into<TagQuery<'a>>,
    ) -> Result<Self> {
        let tag = map.tag(query)?;
        if tag.group != PHMO {
            return Err(Error::UnsupportedGroup(tag.group));
        }
        let (id, address) = (tag.id, tag.location.ok_or(map::Error::NoTagData)?.address);
        Self::decode(&map.read_tag_bytes(id)?, address)
    }

    /// Structural checks. Returns every problem found (empty means valid).
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        let shapes_of = |t: ShapeType| -> usize {
            match t {
                ShapeType::Sphere => self.spheres.len(),
                ShapeType::Pill => self.pills.len(),
                ShapeType::Box => self.boxes.len(),
                ShapeType::Triangle => self.triangle_count,
                ShapeType::Polyhedron => self.polyhedra.len(),
                ShapeType::Type5 => self.type5_count,
                ShapeType::Type6 => self.type6.len(),
                ShapeType::List => self.lists.len(),
                ShapeType::Mopp => self.mopps.len(),
                ShapeType::Unknown(_) => 0,
            }
        };
        let check_ref = |what: String, r: ShapeRef, out: &mut Vec<String>| {
            if matches!(r.kind, ShapeType::Unknown(_)) {
                out.push(format!("{what}: unknown shape type {}", r.raw_type));
            } else if usize::from(r.index) >= shapes_of(r.kind) {
                out.push(format!(
                    "{what}: {:?} index {} out of range",
                    r.kind, r.index
                ));
            }
        };
        let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
        if !(self.mass.is_finite() && self.mass >= 0.0 && finite(&self.mass_scales)) {
            out.push("root mass or mass scales are not finite and non-negative".into());
        }
        for (i, b) in self.rigid_bodies.iter().enumerate() {
            check_ref(format!("rigid body {i}"), b.shape, &mut out);
            if !(b.mass.is_finite() && b.mass >= 0.0) {
                out.push(format!("rigid body {i}: bad mass {}", b.mass));
            }
            if !(finite(&b.bounding_sphere_offset)
                && b.bounding_sphere_radius.is_finite()
                && finite(&b.center_of_mass)
                && b.inertia_tensor.iter().all(|r| finite(r)))
            {
                out.push(format!("rigid body {i}: non-finite mass properties"));
            }
            // A physical inertia tensor is symmetric.
            let t = b.inertia_tensor;
            let scale = t
                .iter()
                .flatten()
                .fold(0.0f32, |m, v| m.max(v.abs()))
                .max(1e-6);
            for (r, c) in [(0, 1), (0, 2), (1, 2)] {
                if (t[r][c] - t[c][r]).abs() > 1e-3 * scale {
                    out.push(format!("rigid body {i}: inertia tensor is not symmetric"));
                    break;
                }
            }
            if !(b.node == -1 || (b.node >= 0 && (b.node as usize) < self.nodes.len())) {
                out.push(format!("rigid body {i}: node {} out of range", b.node));
            }
        }
        let mats = self.materials.len();
        let hdr = |name: &str, i: usize, h: &ShapeHeader, out: &mut Vec<String>| {
            if usize::from(h.material) >= mats {
                out.push(format!("{name} {i}: material {} out of range", h.material));
            }
            if !finite(&h.floats()) {
                out.push(format!("{name} {i}: non-finite header value"));
            }
        };
        let valid_transform =
            |t: &Transform| t.rotation.iter().all(|r| finite(r)) && finite(&t.translation);
        for (i, s) in self.spheres.iter().enumerate() {
            hdr("sphere", i, &s.header, &mut out);
            if !(s.radius.is_finite() && s.radius >= 0.0) {
                out.push(format!("sphere {i}: bad radius"));
            }
            if !valid_transform(&s.transform) {
                out.push(format!("sphere {i}: non-finite transform"));
            }
        }
        for (i, s) in self.pills.iter().enumerate() {
            hdr("pill", i, &s.header, &mut out);
            if !(s.radius.is_finite() && s.radius >= 0.0 && finite(&s.bottom) && finite(&s.top)) {
                out.push(format!("pill {i}: bad radius or end points"));
            }
        }
        for (i, s) in self.boxes.iter().enumerate() {
            hdr("box", i, &s.header, &mut out);
            if !(finite(&s.half_extents) && s.half_extents.iter().all(|v| *v >= 0.0)) {
                out.push(format!("box {i}: bad half extents"));
            }
            if !valid_transform(&s.transform) || !s.radius.is_finite() {
                out.push(format!("box {i}: non-finite transform or radius"));
            }
        }
        for (i, t) in self.type6.iter().enumerate() {
            check_ref(format!("type-6 shape {i} child"), t.child, &mut out);
        }
        let (mut group_total, mut plane_total) = (0usize, 0usize);
        let mut work = 0u64;
        for (i, s) in self.polyhedra.iter().enumerate() {
            hdr("polyhedron", i, &s.header, &mut out);
            if !(finite(&s.aabb_half_extents) && finite(&s.aabb_center)) {
                out.push(format!("polyhedron {i}: non-finite bounds"));
            }
            work += s.planes.len() as u64 * s.vertices.len() as u64;
            if s.planes.len() > MAX_HULL_ELEMENTS
                || s.vertices.len() > MAX_HULL_ELEMENTS
                || work > MAX_VALIDATE_WORK
            {
                out.push(format!(
                    "polyhedron {i}: {} planes x {} vertices exceed the validation budget",
                    s.planes.len(),
                    s.vertices.len()
                ));
                group_total += if s.group_capacity > 3 {
                    s.group_count as usize
                } else {
                    0
                };
                plane_total += s.plane_count as usize;
                continue;
            }
            if s.group_capacity > 3 {
                group_total += s.group_count as usize;
            }
            plane_total += s.plane_count as usize;
            let slots = s.group_count as usize * 4;
            if !(s.vertex_count as usize <= slots && s.vertex_count as usize + 4 > slots) {
                out.push(format!(
                    "polyhedron {i}: vertex count {} vs {} groups",
                    s.vertex_count, s.group_count
                ));
            }
            if s.vertices.iter().any(|v| !finite(v)) {
                out.push(format!("polyhedron {i}: non-finite vertex"));
            }
            for (k, pl) in s.planes.iter().enumerate() {
                let len = (pl[0] * pl[0] + pl[1] * pl[1] + pl[2] * pl[2]).sqrt();
                if !(len.is_finite()
                    && pl[3].is_finite()
                    && (len - 1.0).abs() <= PLANE_UNIT_TOLERANCE)
                {
                    out.push(format!(
                        "polyhedron {i}: plane {k} normal length {len} is not unit"
                    ));
                    continue;
                }
                // Convexity: every vertex is on or inside every plane (n.p + w <= 0)
                // and at least one vertex touches it.
                let d: Vec<f32> = s
                    .vertices
                    .iter()
                    .map(|v| pl[0] * v[0] + pl[1] * v[1] + pl[2] * v[2] + pl[3])
                    .collect();
                if d.iter().any(|&x| x > HULL_TOLERANCE) {
                    out.push(format!("polyhedron {i}: a vertex lies outside plane {k}"));
                } else if !d.iter().any(|&x| x.abs() <= HULL_TOLERANCE) {
                    out.push(format!("polyhedron {i}: plane {k} touches no vertex"));
                }
            }
        }
        if group_total != self.four_vector_groups {
            out.push(format!(
                "four-vector block holds {} groups but polyhedra consume {group_total}",
                self.four_vector_groups
            ));
        }
        if plane_total != self.plane_equations {
            out.push(format!(
                "plane block holds {} planes but polyhedra consume {plane_total}",
                self.plane_equations
            ));
        }
        let mut child_total = 0usize;
        for (i, l) in self.lists.iter().enumerate() {
            if l.capacity > 4 {
                child_total += l.children.len();
            }
            for (k, c) in l.children.iter().enumerate() {
                check_ref(format!("list {i} child {k}"), c.shape, &mut out);
                if matches!(c.shape.kind, ShapeType::List) && usize::from(c.shape.index) == i {
                    out.push(format!("list {i} child {k}: list contains itself"));
                }
            }
        }
        if child_total != self.list_children {
            out.push(format!(
                "child block holds {} entries but lists consume {child_total}",
                self.list_children
            ));
        }
        // Compound shapes: lists and mopps form one graph (a mopp wraps a list,
        // a list may hold lists and mopps); it must be acyclic.
        let (nl, nm) = (self.lists.len(), self.mopps.len());
        let edges = |i: usize| -> Vec<usize> {
            let target = |r: ShapeRef| match r.kind {
                ShapeType::List => Some(usize::from(r.index)),
                ShapeType::Mopp => Some(nl + usize::from(r.index)),
                ShapeType::Type6 => Some(nl + nm + usize::from(r.index)),
                _ => None,
            };
            if i < nl {
                self.lists[i]
                    .children
                    .iter()
                    .filter_map(|c| target(c.shape))
                    .collect()
            } else if i < nl + nm {
                target(self.mopps[i - nl].child).into_iter().collect()
            } else {
                target(self.type6[i - nl - nm].child).into_iter().collect()
            }
        };
        if let Some(c) = tree_cycle(nl + nm + self.type6.len(), |i| edges(i).into_iter()) {
            out.push(format!("compound shape graph has a cycle through node {c}"));
        }
        let mut code_total = 0usize;
        for (i, m) in self.mopps.iter().enumerate() {
            // The engine indexes the list block with the child's index
            // unconditionally (`0x001eae11`..`0x001eae24`).
            if m.child.kind != ShapeType::List || usize::from(m.child.index) >= self.lists.len() {
                out.push(format!("mopp {i}: child is not a valid list"));
            }
            code_total = m.code_offset + ((m.code_size as usize + 0xf) & !0xf);
        }
        if code_total != self.mopp_code.len() {
            out.push(format!(
                "mopp code block is {} bytes but mopps consume {code_total}",
                self.mopp_code.len()
            ));
        }
        out
    }

    /// Wavefront OBJ of the convex shapes, in each shape's local space (rigid-body
    /// node placement is not applied). Spheres and pills are UV-tessellated.
    pub fn to_obj(&self) -> Result<String> {
        let mut o = Obj::default();
        for (i, s) in self.polyhedra.iter().enumerate() {
            o.object(format!("polyhedron{i}"));
            o.hull(&s.vertices, &s.planes)?;
        }
        for (i, b) in self.boxes.iter().enumerate() {
            o.object(format!("box{i}"));
            let [hx, hy, hz] = b.half_extents;
            let t = &b.transform;
            let mut pts = Vec::new();
            for k in 0..8 {
                let l = [
                    if k & 1 == 0 { -hx } else { hx },
                    if k & 2 == 0 { -hy } else { hy },
                    if k & 4 == 0 { -hz } else { hz },
                ];
                // Rows of the stored rotation are the transformed axes; the engine
                // composes a column vector, so rows act on `l` as `R * l`.
                pts.push(std::array::from_fn(|a| {
                    t.rotation[a][0] * l[0]
                        + t.rotation[a][1] * l[1]
                        + t.rotation[a][2] * l[2]
                        + t.translation[a]
                }));
            }
            o.cuboid(&pts);
            o.check()?;
        }
        for (i, s) in self.spheres.iter().enumerate() {
            o.object(format!("sphere{i}"));
            o.capsule(s.transform.translation, s.transform.translation, s.radius);
            o.check()?;
        }
        for (i, s) in self.pills.iter().enumerate() {
            o.object(format!("pill{i}"));
            o.capsule(s.bottom, s.top, s.radius);
            o.check()?;
        }
        Ok(o.text)
    }
}

#[derive(Default)]
struct Obj {
    text: String,
    next: usize,
    tris: usize,
}
impl Obj {
    fn object(&mut self, name: String) {
        let _ = writeln!(self.text, "o {name}");
    }
    fn v(&mut self, p: [f32; 3]) -> usize {
        let _ = writeln!(self.text, "v {} {} {}", p[0], p[1], p[2]);
        self.next += 1;
        self.next
    }
    fn tri(&mut self, a: usize, b: usize, c: usize) {
        self.tris += 1;
        let _ = writeln!(self.text, "f {a} {b} {c}");
    }
    fn check(&self) -> Result<()> {
        if self.tris > MAX_OBJ_TRIANGLES {
            return Err(Error::Invalid("export exceeds the triangle budget"));
        }
        Ok(())
    }
    fn cuboid(&mut self, pts: &[[f32; 3]]) {
        let base = self.next;
        for p in pts {
            self.v(*p);
        }
        for q in [
            [0, 1, 3, 2],
            [4, 6, 7, 5],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 5, 7, 3],
        ] {
            self.tri(base + q[0] + 1, base + q[1] + 1, base + q[2] + 1);
            self.tri(base + q[0] + 1, base + q[2] + 1, base + q[3] + 1);
        }
    }
    /// One polygon per distinct face (the vertices lying on a plane, ordered
    /// around their centroid). Planes naming an already-emitted face are skipped
    /// (retail hulls have near-parallel planes that pick overlapping vertex sets,
    /// so no tighter per-hull triangle bound holds), and the running triangle
    /// count is checked against the export budget after every face.
    fn hull(&mut self, verts: &[[f32; 3]], planes: &[[f32; 4]]) -> Result<()> {
        if verts.len() > MAX_HULL_ELEMENTS || planes.len() > MAX_HULL_ELEMENTS {
            return Err(Error::Invalid("polyhedron exceeds the export budget"));
        }
        let base = self.next;
        let mut seen = std::collections::HashSet::new();
        for v in verts {
            self.v(*v);
        }
        for pl in planes {
            let n = [pl[0], pl[1], pl[2]];
            let mut on: Vec<usize> = (0..verts.len())
                .filter(|&i| {
                    (n[0] * verts[i][0] + n[1] * verts[i][1] + n[2] * verts[i][2] + pl[3]).abs()
                        < 1e-3
                })
                .collect();
            if on.len() < 3 || !seen.insert(on.clone()) {
                continue;
            }
            let c: [f32; 3] = std::array::from_fn(|a| {
                on.iter().map(|&i| verts[i][a]).sum::<f32>() / on.len() as f32
            });
            // Basis in the plane.
            let ref_axis = if n[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            let u = cross(n, ref_axis);
            let ul = dot(u, u).sqrt().max(1e-12);
            let u = [u[0] / ul, u[1] / ul, u[2] / ul];
            let w = cross(n, u);
            on.sort_by(|&a, &b| {
                let ang = |i: usize| {
                    let d = [verts[i][0] - c[0], verts[i][1] - c[1], verts[i][2] - c[2]];
                    dot(d, w).atan2(dot(d, u))
                };
                ang(a).total_cmp(&ang(b))
            });
            for k in 1..on.len() - 1 {
                self.tri(base + on[0] + 1, base + on[k] + 1, base + on[k + 1] + 1);
            }
            self.check()?;
        }
        self.check()
    }
    /// A capsule between `a` and `b` (a sphere when they coincide), 12x6 UV rings,
    /// built in a basis whose pole axis runs from `a` to `b`.
    fn capsule(&mut self, a: [f32; 3], b: [f32; 3], r: f32) {
        let base = self.next;
        let (seg, rings) = (12usize, 6usize);
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = dot(d, d).sqrt();
        let axis = if len > 1e-9 {
            [d[0] / len, d[1] / len, d[2] / len]
        } else {
            [0.0, 0.0, 1.0]
        };
        let helper = if axis[2].abs() < 0.9 {
            [0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let u = cross(axis, helper);
        let ul = dot(u, u).sqrt();
        let u = [u[0] / ul, u[1] / ul, u[2] / ul];
        let v = cross(axis, u);
        for ring in 0..=rings {
            let phi = std::f32::consts::PI * ring as f32 / rings as f32;
            let (c, s) = (phi.cos(), phi.sin());
            // The first half-rings cap the `b` end, the rest the `a` end.
            let from = if ring <= rings / 2 { b } else { a };
            for k in 0..seg {
                let th = 2.0 * std::f32::consts::PI * k as f32 / seg as f32;
                let (ct, st) = (th.cos(), th.sin());
                self.v(std::array::from_fn(|i| {
                    from[i] + r * (s * ct * u[i] + s * st * v[i] + c * axis[i])
                }));
            }
        }
        for ring in 0..rings {
            for k in 0..seg {
                let (i0, i1) = (
                    base + ring * seg + k + 1,
                    base + ring * seg + (k + 1) % seg + 1,
                );
                let (j0, j1) = (i0 + seg, i1 + seg);
                self.tri(i0, j0, i1);
                self.tri(i1, j0, j1);
            }
        }
    }
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

#[cfg(test)]
#[path = "collision_tests.rs"]
mod tests;
