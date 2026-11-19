//! Reading Halo 1 Xbox `mode` (model) and `antr` (animation graph) tags.
//!
//! Layouts are in `docs/halo1/models.md`, with the evidence for each
//! field. Everything here reads through [`Memory`], so the same code runs on a
//! real map ([`h1map::H1Map`]) and on invented bytes in tests.

use crate::{Error, Result};

/// Halo 1 tag data, addressed by virtual address.
pub trait Memory {
    /// `len` bytes at a tag-data address.
    fn read(&self, address: u32, len: usize) -> Result<&[u8]>;
}

impl Memory for h1map::H1Map {
    fn read(&self, address: u32, len: usize) -> Result<&[u8]> {
        h1map::H1Map::read(self, address, len).map_err(Error::H1)
    }
}

pub(crate) fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn i16_at(b: &[u8], o: usize) -> i16 {
    u16_at(b, o) as i16
}
pub(crate) fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(crate) fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}
pub(crate) fn f32s<const N: usize>(b: &[u8], o: usize) -> [f32; N] {
    std::array::from_fn(|i| f32_at(b, o + 4 * i))
}
/// A fixed `char[32]` name.
fn name32(b: &[u8], o: usize) -> String {
    let s = &b[o..o + 32];
    let end = s.iter().position(|&c| c == 0).unwrap_or(32);
    String::from_utf8_lossy(&s[..end]).into_owned()
}
fn need(b: &[u8], len: usize, what: &'static str) -> Result<()> {
    if b.len() < len {
        return Err(Error::Truncated(what));
    }
    Ok(())
}

/// The elements of the 12-byte tag block at `at` in `owner`.
fn block<'a, M: Memory + ?Sized>(
    m: &'a M,
    owner: &[u8],
    at: usize,
    stride: usize,
) -> Result<Vec<&'a [u8]>> {
    let count = u32_at(owner, at) as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    if count > 0x10000 {
        return Err(Error::Invalid(format!("block of {count} elements")));
    }
    let bytes = m.read(u32_at(owner, at + 4), count * stride)?;
    Ok(bytes.chunks_exact(stride).collect())
}

/// Decodes the Xbox 11:11:10 packed vector (x in the low 11 bits, signed).
pub fn unpack_normal(raw: u32) -> [f32; 3] {
    let s = |v: u32, bits: u32| {
        let shift = 32 - bits;
        let x = ((v << shift) as i32) >> shift;
        (x as f32 / ((1i32 << (bits - 1)) - 1) as f32).max(-1.0)
    };
    [
        s(raw & 0x7ff, 11),
        s((raw >> 11) & 0x7ff, 11),
        s(raw >> 22, 10),
    ]
}

/// An i16 quaternion as stored in models and animations, normalised.
/// All-zero words come back as identity, counted by the caller.
pub fn quat_i16(b: &[u8], o: usize) -> [f32; 4] {
    let q: [f32; 4] = std::array::from_fn(|i| f32::from(i16_at(b, o + 2 * i)));
    normalize(q)
}
/// A Halo 1 rotation in Halo 2's convention: its conjugate.
///
/// Halo 1 stores node, marker and animation rotations as the conjugate of
/// the quaternion Halo 2 (and [`crate::mode::quat_matrix`]) means. On the
/// disc's bipeds, composing the nodes by Halo 2's rule puts vertices 3x
/// farther from their own node than the conjugate does (cyborg 0.27 against
/// 0.084 units), and only the conjugate reproduces the inverse rest frames
/// Halo 1's cache stores with each node (to 5e-7;
/// `skin_tests::h1_disc_bipeds_skin`). Everything this module returns is
/// already converted, so callers compose with Halo 2's rule.
pub fn h2_rotation(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

pub(crate) fn normalize(q: [f32; 4]) -> [f32; 4] {
    let l = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    if l > 0.0 && l.is_finite() {
        q.map(|x| x / l)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// Model header size; blocks below are offsets into it.
pub const MODE_HEADER: usize = 0xe8;
const MODE_MARKERS: usize = 0xac;
const MODE_NODES: usize = 0xb8;
const MODE_REGIONS: usize = 0xc4;
const MODE_GEOMETRIES: usize = 0xd0;
const MODE_SHADERS: usize = 0xdc;
const MARKER_LEN: usize = 0x40;
const MARKER_INSTANCE_LEN: usize = 0x20;
const NODE_LEN: usize = 0x9c;
const REGION_LEN: usize = 0x4c;
const PERMUTATION_LEN: usize = 0x58;
const GEOMETRY_LEN: usize = 0x30;
const PART_LEN: usize = 0x68;
const SHADER_LEN: usize = 0x20;
/// Bytes per compressed (Xbox) vertex.
pub const COMPRESSED_VERTEX_LEN: usize = 0x20;
/// Bytes per uncompressed vertex.
pub const UNCOMPRESSED_VERTEX_LEN: usize = 0x44;

/// A Halo 1 render model.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// Header flags (`+0x00`).
    pub flags: u32,
    /// Node list checksum (`+0x04`); animations carry the same value.
    pub node_list_checksum: u32,
    /// LOD cutoffs, super high to super low (`+0x08`).
    pub lod_cutoffs: [f32; 5],
    /// Base map u/v scale (`+0x30`); texture coordinates are multiplied by it.
    pub base_map_scale: [f32; 2],
    pub markers: Vec<Marker>,
    pub nodes: Vec<Node>,
    pub regions: Vec<Region>,
    pub geometries: Vec<Geometry>,
    pub shaders: Vec<ShaderRef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub name: String,
    pub instances: Vec<MarkerInstance>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkerInstance {
    pub region: i8,
    pub permutation: i8,
    pub node: i8,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub next_sibling: i16,
    pub first_child: i16,
    pub parent: i16,
    /// Default translation relative to the parent.
    pub translation: [f32; 3],
    /// Default rotation relative to the parent, `i, j, k, w`.
    pub rotation: [f32; 4],
    pub distance_from_parent: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub name: String,
    pub permutations: Vec<Permutation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Permutation {
    pub name: String,
    pub flags: u32,
    /// Geometry per LOD: super low, low, medium, high, super high.
    pub lods: [i16; 5],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub flags: u32,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub flags: u32,
    pub shader: i16,
    /// Centroid nodes and weights.
    pub centroid_nodes: [i16; 2],
    pub centroid_weights: [f32; 2],
    pub centroid: [f32; 3],
    pub vertices: Vec<Vertex>,
    /// Triangle strip, D3D order (odd triangles flip).
    pub strip: Vec<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub binormal: [f32; 3],
    pub tangent: [f32; 3],
    /// Texture coordinate, before the base map scale.
    pub texcoord: [f32; 2],
    /// Node indices; -1 = none.
    pub nodes: [i16; 2],
    pub weights: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShaderRef {
    /// Group of the referenced shader (`soso`, `schi`, ...).
    pub group: h1map::Group,
    pub tag: h1map::TagId,
    pub permutation: i16,
}

impl Model {
    /// Reads a `mode` tag whose header is at `address`.
    pub fn read<M: Memory + ?Sized>(m: &M, address: u32) -> Result<Self> {
        let h = m.read(address, MODE_HEADER)?;
        let markers = block(m, h, MODE_MARKERS, MARKER_LEN)?
            .into_iter()
            .map(|e| {
                Ok(Marker {
                    name: name32(e, 0),
                    instances: block(m, e, 0x34, MARKER_INSTANCE_LEN)?
                        .into_iter()
                        .map(|i| MarkerInstance {
                            region: i[0] as i8,
                            permutation: i[1] as i8,
                            node: i[2] as i8,
                            translation: f32s(i, 4),
                            rotation: h2_rotation(normalize(f32s(i, 0x10))),
                        })
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let nodes = block(m, h, MODE_NODES, NODE_LEN)?
            .into_iter()
            .map(|e| Node {
                name: name32(e, 0),
                next_sibling: i16_at(e, 0x20),
                first_child: i16_at(e, 0x22),
                parent: i16_at(e, 0x24),
                translation: f32s(e, 0x28),
                rotation: h2_rotation(normalize(f32s(e, 0x34))),
                distance_from_parent: f32_at(e, 0x44),
            })
            .collect::<Vec<_>>();
        let regions = block(m, h, MODE_REGIONS, REGION_LEN)?
            .into_iter()
            .map(|e| {
                Ok(Region {
                    name: name32(e, 0),
                    permutations: block(m, e, 0x40, PERMUTATION_LEN)?
                        .into_iter()
                        .map(|p| Permutation {
                            name: name32(p, 0),
                            flags: u32_at(p, 0x20),
                            lods: std::array::from_fn(|i| i16_at(p, 0x40 + 2 * i)),
                        })
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let geometries = block(m, h, MODE_GEOMETRIES, GEOMETRY_LEN)?
            .into_iter()
            .map(|g| {
                Ok(Geometry {
                    flags: u32_at(g, 0),
                    parts: block(m, g, 0x24, PART_LEN)?
                        .into_iter()
                        .map(|p| read_part(m, p))
                        .collect::<Result<Vec<_>>>()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let shaders = block(m, h, MODE_SHADERS, SHADER_LEN)?
            .into_iter()
            .map(|e| ShaderRef {
                group: h1map::Group(u32_at(e, 0)),
                tag: h1map::TagId(u32_at(e, 0xc)),
                permutation: i16_at(e, 0x10),
            })
            .collect();
        let model = Self {
            flags: u32_at(h, 0),
            node_list_checksum: u32_at(h, 4),
            lod_cutoffs: f32s(h, 8),
            base_map_scale: f32s(h, 0x30),
            markers,
            nodes,
            regions,
            geometries,
            shaders,
        };
        model.check()?;
        Ok(model)
    }

    fn check(&self) -> Result<()> {
        let n = self.nodes.len() as i16;
        let link = |v: i16| v == -1 || (0..n).contains(&v);
        if !self
            .nodes
            .iter()
            .all(|x| link(x.parent) && link(x.first_child) && link(x.next_sibling))
        {
            return Err(Error::Invalid("node link out of range".into()));
        }
        let g = self.geometries.len() as i16;
        for r in &self.regions {
            for p in &r.permutations {
                if p.lods.iter().any(|&l| !(l == -1 || (0..g).contains(&l))) {
                    return Err(Error::Invalid("permutation LOD index out of range".into()));
                }
            }
        }
        for geo in &self.geometries {
            for part in &geo.parts {
                if part
                    .strip
                    .iter()
                    .any(|&i| usize::from(i) >= part.vertices.len())
                {
                    return Err(Error::Invalid("strip index outside its part".into()));
                }
                if part
                    .vertices
                    .iter()
                    .flat_map(|v| v.nodes)
                    .any(|i| !(i == -1 || (0..n).contains(&i)))
                {
                    return Err(Error::Invalid("vertex node out of range".into()));
                }
                if part.shader != -1 && !(0..self.shaders.len() as i16).contains(&part.shader) {
                    return Err(Error::Invalid("part shader out of range".into()));
                }
            }
        }
        Ok(())
    }
}

fn read_part<M: Memory + ?Sized>(m: &M, p: &[u8]) -> Result<Part> {
    // Vertices: the tag's own blocks when present (as in source tags),
    // otherwise the cache's vertex buffer (Xbox: compressed, 32 bytes).
    let unc = block(m, p, 0x20, UNCOMPRESSED_VERTEX_LEN)?;
    let comp = block(m, p, 0x2c, COMPRESSED_VERTEX_LEN)?;
    let vertices = if !unc.is_empty() {
        unc.into_iter().map(uncompressed_vertex).collect()
    } else if !comp.is_empty() {
        comp.into_iter().map(compressed_vertex).collect()
    } else {
        let count = u32_at(p, 0x58) as usize;
        let kind = u32_at(p, 0x54);
        if count > 0xffff {
            return Err(Error::Invalid(format!("part with {count} vertices")));
        }
        if count == 0 {
            Vec::new()
        } else {
            if kind != 5 {
                return Err(Error::Unsupported(format!("cache vertex type {kind}")));
            }
            let entry = m.read(u32_at(p, 0x64), 8)?;
            let bytes = m.read(u32_at(entry, 4), count * COMPRESSED_VERTEX_LEN)?;
            bytes
                .chunks_exact(COMPRESSED_VERTEX_LEN)
                .map(compressed_vertex)
                .collect()
        }
    };
    let triangles = block(m, p, 0x38, 6)?;
    let strip = if !triangles.is_empty() {
        // Source tags: the strip in 3-index rows, padded with -1.
        triangles
            .into_iter()
            .flat_map(|t| [u16_at(t, 0), u16_at(t, 2), u16_at(t, 4)])
            .filter(|&i| i != u16::MAX)
            .collect()
    } else {
        // Cache: +0x48 holds the strip length less two, +0x4c its address.
        let n = u32_at(p, 0x48) as usize;
        if n == 0 {
            Vec::new()
        } else {
            if n > 0x20000 {
                return Err(Error::Invalid(format!("strip of {n} triangles")));
            }
            m.read(u32_at(p, 0x4c), (n + 2) * 2)?
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect()
        }
    };
    Ok(Part {
        flags: u32_at(p, 0),
        shader: i16_at(p, 4),
        centroid_nodes: [i16_at(p, 8), i16_at(p, 0xa)],
        centroid_weights: f32s(p, 0xc),
        centroid: f32s(p, 0x14),
        vertices,
        strip,
    })
}

/// The 32-byte Xbox vertex: position, three packed vectors, i16 texcoords,
/// two node indices stored times three, and the first node's i16 weight.
pub fn compressed_vertex(b: &[u8]) -> Vertex {
    let node = |x: u8| {
        let v = x as i8;
        if v < 0 { -1 } else { i16::from(v) / 3 }
    };
    let w0 = f32::from(i16_at(b, 0x1e)) / 32767.0;
    let nodes = [node(b[0x1c]), node(b[0x1d])];
    Vertex {
        position: f32s(b, 0),
        normal: unpack_normal(u32_at(b, 0xc)),
        binormal: unpack_normal(u32_at(b, 0x10)),
        tangent: unpack_normal(u32_at(b, 0x14)),
        texcoord: [
            f32::from(i16_at(b, 0x18)) / 32767.0,
            f32::from(i16_at(b, 0x1a)) / 32767.0,
        ],
        nodes,
        weights: if nodes[1] == -1 {
            [1.0, 0.0]
        } else {
            [w0, 1.0 - w0]
        },
    }
}

/// The 68-byte uncompressed vertex.
pub fn uncompressed_vertex(b: &[u8]) -> Vertex {
    Vertex {
        position: f32s(b, 0),
        normal: f32s(b, 0xc),
        binormal: f32s(b, 0x18),
        tangent: f32s(b, 0x24),
        texcoord: f32s(b, 0x30),
        nodes: [i16_at(b, 0x38), i16_at(b, 0x3a)],
        weights: f32s(b, 0x3c),
    }
}

/// Triangles of a Halo 1 strip, skipping degenerate joins, wound so that
/// they agree with their vertex normals in Halo 2's convention (the order
/// the Halo 2 decoder gives list triangles). Halo 1 strips have the opposite
/// parity to Halo 2's: even triangles are `(a, c, b)`, odd ones `(a, b, c)`.
/// On beavercreek's cyborg, assault rifle and warthog, Halo 2's parity rule
/// disagrees with the normals on 99% of triangles and this one on under 1%.
pub fn strip_triangles(strip: &[u16]) -> Vec<[u16; 3]> {
    strip
        .windows(3)
        .enumerate()
        .filter(|(_, w)| w[0] != w[1] && w[1] != w[2] && w[0] != w[2])
        .map(|(i, w)| {
            if i % 2 == 0 {
                [w[0], w[2], w[1]]
            } else {
                [w[0], w[1], w[2]]
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Animation graphs
// ---------------------------------------------------------------------------

/// Animation graph header size.
pub const ANTR_HEADER: usize = 0x80;
const ANTR_UNITS: usize = 0x0c;
const ANTR_WEAPONS: usize = 0x18;
const ANTR_VEHICLES: usize = 0x24;
/// The object overlays block (`0x14` each): `+0` i16 animation, `+2` u16
/// function (0 to 3: the object's A to D output), `+4` u16 function
/// controls (0 frame, 1 scale). V (Blood Gulch): the warthog's one
/// element names its `firing` overlay with function 3 and controls 0;
/// the scorpion's names no animation (-1).
const ANTR_OBJECTS: usize = 0x00;
const OBJECT_OVERLAY_LEN: usize = 0x14;
/// A vehicles block element (`0x74`): the steering screen ([`AimScreen`])
/// at `+0`, the vehicle slots (`i16` animation indices,
/// [`VEHICLE_SLOTS`]) at `+0x5c`, the suspension block
/// ([`SUSPENSION_LEN`]) at `+0x68`. V (Blood Gulch's warthog, ghost and
/// scorpion): the warthog's slots name `vehicle steering` (0) and
/// `vehicle ground-speed` (5), the ghost's `vehicle steering` (0); the
/// warthog's four suspension elements name its four `suspension ...`
/// animations and its four tire mass points, the scorpion's eight its
/// `tread ...` animations and its eight tread mass points.
const VEHICLE_LEN: usize = 0x74;
const VEHICLE_SLOT_LIST: usize = 0x5c;
const VEHICLE_SUSPENSION: usize = 0x68;
/// A suspension element: `+0` i16 mass point (the `phys` mass point
/// index), `+2` i16 animation, `+4` f32 full extension ground depth, `+8`
/// f32 full compression ground depth, 8 bytes of padding.
const SUSPENSION_LEN: usize = 0x14;
/// A unit's aiming screen (`+0x20`) and unit slots (`+0x40`).
const UNIT_SCREEN: usize = 0x20;
const UNIT_SLOT_LIST: usize = 0x40;
/// A weapon class's aiming screen (`+0x60`, after the label and two
/// marker names).
const CLASS_SCREEN: usize = 0x60;
/// The first-person weapons block (`0x1c` each): 16 bytes, then a block of
/// `i16` animation indices at `+0x10`, one per first-person slot
/// ([`FIRST_PERSON_SLOTS`]).
const ANTR_FIRST_PERSON: usize = 0x48;
const FIRST_PERSON_LEN: usize = 0x1c;
const FIRST_PERSON_SLOT_LIST: usize = 0x10;
const ANTR_NODES: usize = 0x68;
const ANTR_ANIMATIONS: usize = 0x74;
const ANTR_NODE_LEN: usize = 0x40;
/// A unit (mode) element: label at +0, weapon classes at `UNIT_WEAPONS`.
const UNIT_LEN: usize = 0x64;
const UNIT_WEAPONS: usize = 0x58;
/// A weapon class element: label at +0, animation slots (`i16` animation
/// indices) at `CLASS_SLOTS`, weapon types at `CLASS_TYPES`.
const CLASS_LEN: usize = 0xbc;
const CLASS_SLOTS: usize = 0x98;
const CLASS_TYPES: usize = 0xb0;
/// A weapon type element: label at +0, animation slots at `TYPE_SLOTS`.
const TYPE_LEN: usize = 0x3c;
const TYPE_SLOTS: usize = 0x30;
const ANIMATION_LEN: usize = 0xb4;

/// Animation type (`+0x20`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimType {
    Base,
    Overlay,
    Replacement,
}

/// A Halo 1 animation graph.
#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    /// The graph's own nodes; empty when it animates its model's skeleton
    /// ([`AnimNode::from_model`]).
    pub nodes: Vec<AnimNode>,
    pub animations: Vec<Animation>,
    /// Element counts of the units, weapons and vehicles blocks.
    pub unit_count: u32,
    pub weapon_count: u32,
    pub vehicle_count: u32,
    /// Halo 1's lookup tables: per unit, per weapon class (and type), the
    /// animation index of each slot (-1 for none). A slot may name another
    /// class's animation: every class's throw-grenade slot names
    /// `stand rifle throw-grenade` on the cyborg.
    pub units: Vec<UnitSlots>,
    /// A first-person graph's slot table (the first element of `+0x48`):
    /// per slot of [`FIRST_PERSON_SLOTS`], the animation it plays, -1 for
    /// none. Empty on other graphs.
    pub first_person_slots: Vec<i16>,
    /// A vehicle graph's first vehicles element: steering screen, slots
    /// and suspension. `None` without one.
    pub vehicle: Option<VehicleSlots>,
    /// The object overlays block: animations an object function drives.
    pub objects: Vec<ObjectOverlay>,
}

/// Halo 1's vehicle slots, by position ([`VehicleSlots::slots`]): the
/// words Halo 2 asks a vehicle's graph for as overlays (E: the vehicle
/// animation update `0x000f0760` looks up `steering`, `roll`, `throttle`,
/// `velocity`, `ground_speed` and `ground_speed_rear` through
/// `0x001cb9d0`; Halo 2's string table holds `steering` .. `unoccupied`
/// in this order, IDs `0x78` to `0x7f`). V that slot 0 is steering and
/// slot 5 ground speed: the warthog's slots hold `vehicle steering` and
/// `vehicle ground-speed` there, the ghost's slot 0 its `vehicle
/// steering`. The others are S, by their order in Halo 2's string table.
pub const VEHICLE_SLOTS: &[&str] = &[
    "steering",
    "roll",
    "throttle",
    "velocity",
    "braking",
    "ground_speed",
    "occupied",
    "unoccupied",
];

/// An aiming or steering screen (`0x18` bytes): the yaw and pitch each
/// frame of a screen animation stands for. Halo 2's blend screen holds the
/// same six fields after its label (V: the retail chaingun turret's
/// `aiming` screen and the Halo 1 warthog's `fixed` class screen are the
/// same 24 bytes; the retail ghost's two screens have the Halo 1 ghost's
/// shape).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimScreen {
    pub right_yaw_per_frame: f32,
    pub left_yaw_per_frame: f32,
    pub right_frames: i16,
    pub left_frames: i16,
    pub down_pitch_per_frame: f32,
    pub up_pitch_per_frame: f32,
    pub down_frames: i16,
    pub up_frames: i16,
}

impl AimScreen {
    fn read(b: &[u8], at: usize) -> Self {
        Self {
            right_yaw_per_frame: f32_at(b, at),
            left_yaw_per_frame: f32_at(b, at + 4),
            right_frames: i16_at(b, at + 8),
            left_frames: i16_at(b, at + 0xa),
            down_pitch_per_frame: f32_at(b, at + 0xc),
            up_pitch_per_frame: f32_at(b, at + 0x10),
            down_frames: i16_at(b, at + 0x14),
            up_frames: i16_at(b, at + 0x16),
        }
    }

    /// The 24 bytes as Halo 1 (and Halo 2's blend screen) lay them out.
    pub fn bytes(&self) -> [u8; 0x18] {
        let mut b = [0u8; 0x18];
        b[0..4].copy_from_slice(&self.right_yaw_per_frame.to_le_bytes());
        b[4..8].copy_from_slice(&self.left_yaw_per_frame.to_le_bytes());
        b[8..0xa].copy_from_slice(&self.right_frames.to_le_bytes());
        b[0xa..0xc].copy_from_slice(&self.left_frames.to_le_bytes());
        b[0xc..0x10].copy_from_slice(&self.down_pitch_per_frame.to_le_bytes());
        b[0x10..0x14].copy_from_slice(&self.up_pitch_per_frame.to_le_bytes());
        b[0x14..0x16].copy_from_slice(&self.down_frames.to_le_bytes());
        b[0x16..0x18].copy_from_slice(&self.up_frames.to_le_bytes());
        b
    }

    /// Whether the screen spans anything (an all-zero screen is none).
    pub fn is_set(&self) -> bool {
        self.right_frames != 0
            || self.left_frames != 0
            || self.down_frames != 0
            || self.up_frames != 0
    }
}

/// A graph's vehicles element ([`VEHICLE_LEN`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleSlots {
    pub screen: AimScreen,
    /// Per [`VEHICLE_SLOTS`] word, the animation index, -1 for none.
    pub slots: Vec<i16>,
    pub suspension: Vec<Suspension>,
}

/// One suspension element ([`SUSPENSION_LEN`]). Halo 1 writes the ground
/// depths negative (V: the warthog's -0.314 and -0.135, the scorpion's
/// -0.38 to -0.11).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Suspension {
    pub mass_point: i16,
    pub animation: i16,
    pub full_extension_ground_depth: f32,
    pub full_compression_ground_depth: f32,
}

/// One object overlay ([`OBJECT_OVERLAY_LEN`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectOverlay {
    pub animation: i16,
    pub function: u16,
    pub controls: u16,
}

/// What each first-person slot is, in Halo 1's own words (the name half
/// after `first-person`), by position. Halo 1's engine plays a weapon's
/// first-person animations through this table, not by name, and a slot
/// may name an animation labelled for another (V, every first-person graph
/// on Blood Gulch: the assault rifle's `reload-empty` slot plays its
/// `reload-full`, the shotgun's `reload-full` its `reload-empty`, its
/// `light-on` its `light-off`; its `firing` fills no slot).
///
/// Each position's word is the label of the animation it holds on a graph
/// of the disc where the two agree (V, Blood Gulch's 11 first-person
/// graphs), except `light-on` (6) and `misfire-2` (19), which no Blood
/// Gulch graph labels: S, from their neighbours `light-off` (5) and
/// `misfire-1` (18).
pub const FIRST_PERSON_SLOTS: &[&str] = &[
    "idle",
    "posing",
    "fire-1",
    "moving",
    "overlays",
    "light-off",
    "light-on",
    "reload-empty",
    "reload-full",
    "overheated",
    "ready",
    "put-away",
    "overcharged",
    "melee",
    "fire-2",
    "overcharged-jitter",
    "throw-grenade",
    "ammunition",
    "misfire-1",
    "misfire-2",
    "throw-overheated",
    "overheating",
    "overheating-again",
    "enter",
    "exit-empty",
    "exit-full",
    "o-h-exit",
    "o-h-s-enter",
];

/// One unit (mode) of a graph's lookup tables ([`Graph::units`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitSlots {
    pub label: String,
    /// The unit's own aiming screen (`+0x20`).
    pub screen: AimScreen,
    /// The unit's own slots (`+0x40`; V: the warthog's 2 to 4 hold its
    /// `stand acc-front-back`, `acc-left-right`, `acc-up-down`).
    pub slots: Vec<i16>,
    pub classes: Vec<ClassSlots>,
}

/// One weapon class of a unit: its slots, and each weapon type's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClassSlots {
    pub label: String,
    /// The class's aiming screen (`+0x60`), the yaw and pitch of each
    /// frame of its aim animation (V: the warthog's and scorpion's
    /// `fixed` screens are 2 + 1 + 2 yaw by 1 + 1 + 1 pitch frames, their
    /// 15-frame `stand fixed aim-still`; the ghost's 1 by 3, its 3-frame
    /// one).
    pub screen: AimScreen,
    pub slots: Vec<i16>,
    pub types: Vec<(String, Vec<i16>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnimNode {
    pub name: String,
    pub next_sibling: i16,
    pub first_child: i16,
    pub parent: i16,
    pub joint_flags: u32,
    pub base_vector: [f32; 3],
    pub vector_range: f32,
}

impl AnimNode {
    /// Animation nodes for a graph without its own node block: the model's.
    pub fn from_model(model: &Model) -> Vec<Self> {
        model
            .nodes
            .iter()
            .map(|n| Self {
                name: n.name.clone(),
                next_sibling: n.next_sibling,
                first_child: n.first_child,
                parent: n.parent,
                joint_flags: 0,
                base_vector: [0.0; 3],
                vector_range: 0.0,
            })
            .collect()
    }
}

/// One node's transform at one frame, in its parent's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: f32,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [0.0; 3],
            scale: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    pub name: String,
    pub kind: AnimType,
    pub frame_count: u16,
    /// Root movement type, 0 to 3 (none, dx/dy, dx/dy/dyaw, dx/dy/dz/dyaw).
    pub frame_info_type: u16,
    pub node_list_checksum: u32,
    pub node_count: u16,
    pub loop_frame: u16,
    pub flags: u16,
    /// Per-node channel flags: rotation, translation, scale.
    pub animated: [u64; 3],
    /// `frames[f][n]`: every channel, animated or from the default data.
    pub frames: Vec<Vec<Transform>>,
    /// Zero quaternions replaced by identity while reading.
    pub zero_rotations: usize,
    /// For compressed (keyframed) animations, what decoding found.
    pub compressed: Option<crate::compressed::Stats>,
}

impl Animation {
    pub fn has(&self, channel: usize, node: usize) -> bool {
        self.animated[channel] >> node & 1 != 0
    }
}

impl Graph {
    /// Reads an `antr` tag whose header is at `address`. Compressed
    /// animations are decoded to per-frame transforms
    /// ([`crate::compressed`]).
    pub fn read<M: Memory + ?Sized>(m: &M, address: u32) -> Result<Self> {
        let h = m.read(address, ANTR_HEADER)?;
        let nodes = block(m, h, ANTR_NODES, ANTR_NODE_LEN)?
            .into_iter()
            .map(|e| AnimNode {
                name: name32(e, 0),
                next_sibling: i16_at(e, 0x20),
                first_child: i16_at(e, 0x22),
                parent: i16_at(e, 0x24),
                joint_flags: u32_at(e, 0x28),
                base_vector: f32s(e, 0x2c),
                vector_range: f32_at(e, 0x38),
            })
            .collect::<Vec<_>>();
        if nodes.len() > 64 {
            return Err(Error::Unsupported(format!("{} nodes", nodes.len())));
        }
        let elements = block(m, h, ANTR_ANIMATIONS, ANIMATION_LEN)?;
        // Some graphs (vehicles, scenery) have no node block and animate the
        // model's skeleton; their animations must agree with each other.
        let count = match (nodes.len(), elements.first()) {
            (0, Some(e)) => usize::from(u16_at(e, 0x2c)),
            (n, _) => n,
        };
        let animations = elements
            .into_iter()
            .map(|e| read_animation(m, e, count))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            nodes,
            animations,
            unit_count: u32_at(h, ANTR_UNITS),
            weapon_count: u32_at(h, ANTR_WEAPONS),
            vehicle_count: u32_at(h, ANTR_VEHICLES),
            units: read_units(m, h)?,
            first_person_slots: match block(m, h, ANTR_FIRST_PERSON, FIRST_PERSON_LEN)?.first() {
                Some(e) => block(m, e, FIRST_PERSON_SLOT_LIST, 2)?
                    .into_iter()
                    .map(|s| i16_at(s, 0))
                    .collect(),
                None => Vec::new(),
            },
            vehicle: match block(m, h, ANTR_VEHICLES, VEHICLE_LEN)?.first() {
                Some(v) => Some(VehicleSlots {
                    screen: AimScreen::read(v, 0),
                    slots: block(m, v, VEHICLE_SLOT_LIST, 2)?
                        .into_iter()
                        .map(|s| i16_at(s, 0))
                        .collect(),
                    suspension: block(m, v, VEHICLE_SUSPENSION, SUSPENSION_LEN)?
                        .into_iter()
                        .map(|s| Suspension {
                            mass_point: i16_at(s, 0),
                            animation: i16_at(s, 2),
                            full_extension_ground_depth: f32_at(s, 4),
                            full_compression_ground_depth: f32_at(s, 8),
                        })
                        .collect(),
                }),
                None => None,
            },
            objects: block(m, h, ANTR_OBJECTS, OBJECT_OVERLAY_LEN)?
                .into_iter()
                .map(|o| ObjectOverlay {
                    animation: i16_at(o, 0),
                    function: u16_at(o, 2),
                    controls: u16_at(o, 4),
                })
                .collect(),
        })
    }
}

fn read_units<M: Memory + ?Sized>(m: &M, h: &[u8]) -> Result<Vec<UnitSlots>> {
    let slots = |e: &[u8], at: usize| -> Result<Vec<i16>> {
        Ok(block(m, e, at, 2)?
            .into_iter()
            .map(|s| i16_at(s, 0))
            .collect())
    };
    block(m, h, ANTR_UNITS, UNIT_LEN)?
        .into_iter()
        .map(|u| {
            let classes = block(m, u, UNIT_WEAPONS, CLASS_LEN)?
                .into_iter()
                .map(|c| {
                    let types = block(m, c, CLASS_TYPES, TYPE_LEN)?
                        .into_iter()
                        .map(|t| Ok((name32(t, 0), slots(t, TYPE_SLOTS)?)))
                        .collect::<Result<Vec<_>>>()?;
                    Ok(ClassSlots {
                        label: name32(c, 0),
                        screen: AimScreen::read(c, CLASS_SCREEN),
                        slots: slots(c, CLASS_SLOTS)?,
                        types,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(UnitSlots {
                label: name32(u, 0),
                screen: AimScreen::read(u, UNIT_SCREEN),
                slots: slots(u, UNIT_SLOT_LIST)?,
                classes,
            })
        })
        .collect()
}

/// Every weapon type the units' classes of the graph at `address` list,
/// `(type label, class label)`, without the animations.
pub fn weapon_types<M: Memory + ?Sized>(m: &M, address: u32) -> Result<Vec<(String, String)>> {
    let h = m.read(address, ANTR_HEADER)?;
    let mut out = Vec::new();
    for u in read_units(m, h)? {
        for c in u.classes {
            out.extend(c.types.into_iter().map(|(t, _)| (t, c.label.clone())));
        }
    }
    Ok(out)
}

/// The animation names of the graph at `address`, without their data.
pub fn animation_names<M: Memory + ?Sized>(m: &M, address: u32) -> Result<Vec<String>> {
    let h = m.read(address, ANTR_HEADER)?;
    Ok(block(m, h, ANTR_ANIMATIONS, ANIMATION_LEN)?
        .into_iter()
        .map(|e| name32(e, 0))
        .collect())
}

fn data_ref<'a, M: Memory + ?Sized>(m: &'a M, e: &[u8], at: usize) -> Result<&'a [u8]> {
    let size = u32_at(e, at) as usize;
    if size == 0 {
        return Ok(&[]);
    }
    m.read(u32_at(e, at + 0xc), size)
}

/// Every rotation of `frames` in Halo 2's convention ([`h2_rotation`]).
fn to_h2_rotations(frames: &mut [Vec<Transform>]) {
    for t in frames.iter_mut().flatten() {
        t.rotation = h2_rotation(t.rotation);
    }
}

fn read_animation<M: Memory + ?Sized>(m: &M, e: &[u8], graph_nodes: usize) -> Result<Animation> {
    need(e, ANIMATION_LEN, "animation")?;
    let name = name32(e, 0);
    let kind = match u16_at(e, 0x20) {
        0 => AnimType::Base,
        1 => AnimType::Overlay,
        2 => AnimType::Replacement,
        t => return Err(Error::Invalid(format!("{name}: animation type {t}"))),
    };
    let frame_count = u16_at(e, 0x22);
    let frame_size = usize::from(u16_at(e, 0x24));
    let node_count = u16_at(e, 0x2c);
    let flags = u16_at(e, 0x3a);
    let n = usize::from(node_count);
    if n != graph_nodes {
        return Err(Error::Invalid(format!(
            "{name}: {n} nodes, the graph has {graph_nodes}"
        )));
    }
    // Bits past the node count hold junk on real data; mask them.
    let mask = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
    let bits = |at: usize| (u64::from(u32_at(e, at)) | u64::from(u32_at(e, at + 4)) << 32) & mask;
    let animated = [bits(0x6c), bits(0x5c), bits(0x7c)];
    let [r, t, s] = animated.map(|b| b.count_ones() as usize);
    if frame_size != 8 * r + 12 * t + 4 * s {
        return Err(Error::Invalid(format!(
            "{name}: frame size {frame_size} for {r}/{t}/{s} channels"
        )));
    }
    if flags & 1 != 0 {
        // Keyframed data at frame data + [+0x88]; no default data.
        let data = data_ref(m, e, 0xa0)?;
        let at = u32_at(e, 0x88) as usize;
        let blob = data
            .get(at..)
            .ok_or_else(|| Error::Invalid(format!("{name}: compressed data offset {at:#x}")))?;
        let (mut frames, stats) = crate::compressed::decode(blob, frame_count, n, animated)
            .map_err(|err| Error::Invalid(format!("{name}: {err}")))?;
        to_h2_rotations(&mut frames);
        return Ok(Animation {
            name,
            kind,
            frame_count,
            frame_info_type: u16_at(e, 0x26),
            node_list_checksum: u32_at(e, 0x28),
            node_count,
            loop_frame: u16_at(e, 0x2e),
            flags,
            animated,
            frames,
            zero_rotations: stats.zero_rotations,
            compressed: Some(stats),
        });
    }
    let default = data_ref(m, e, 0x8c)?;
    let want_default = 8 * (n - r) + 12 * (n - t) + 4 * (n - s);
    if default.len() < want_default {
        return Err(Error::Invalid(format!("{name}: default data too short")));
    }
    let frames_bytes = data_ref(m, e, 0xa0)?;
    if frames_bytes.len() < frame_size * usize::from(frame_count) {
        return Err(Error::Invalid(format!("{name}: frame data too short")));
    }
    let mut zero_rotations = 0usize;
    let mut rot = |b: &[u8], o: usize| {
        if b[o..o + 8].iter().all(|&x| x == 0) {
            zero_rotations += 1;
        }
        quat_i16(b, o)
    };
    // Default data: per node, the channels it does not animate, in the
    // order rotation, translation, scale.
    let mut base = vec![Transform::default(); n];
    let mut at = 0;
    for (node, out) in base.iter_mut().enumerate() {
        if animated[0] >> node & 1 == 0 {
            out.rotation = rot(default, at);
            at += 8;
        }
        if animated[1] >> node & 1 == 0 {
            out.translation = f32s(default, at);
            at += 12;
        }
        if animated[2] >> node & 1 == 0 {
            out.scale = f32_at(default, at);
            at += 4;
        }
    }
    // Frames: per node, the channels it animates, same order.
    let mut frames = Vec::with_capacity(usize::from(frame_count));
    for f in 0..usize::from(frame_count) {
        let b = &frames_bytes[f * frame_size..(f + 1) * frame_size];
        let mut pose = base.clone();
        let mut at = 0;
        for (node, out) in pose.iter_mut().enumerate() {
            if animated[0] >> node & 1 != 0 {
                out.rotation = rot(b, at);
                at += 8;
            }
            if animated[1] >> node & 1 != 0 {
                out.translation = f32s(b, at);
                at += 12;
            }
            if animated[2] >> node & 1 != 0 {
                out.scale = f32_at(b, at);
                at += 4;
            }
        }
        frames.push(pose);
    }
    to_h2_rotations(&mut frames);
    Ok(Animation {
        name,
        kind,
        frame_count,
        frame_info_type: u16_at(e, 0x26),
        node_list_checksum: u32_at(e, 0x28),
        node_count,
        loop_frame: u16_at(e, 0x2e),
        flags,
        animated,
        frames,
        zero_rotations,
        compressed: None,
    })
}
