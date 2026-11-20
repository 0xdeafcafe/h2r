//! Halo 1 `mode` -> Halo 2 `mode` (render model).
//!
//! One Halo 2 section per Halo 1 geometry. Each Halo 1 part becomes a
//! triangle-list part (strips are unrolled, degenerate joins dropped), so
//! winding does not depend on strip parity; a sky's model keeps strips, in
//! Halo 2's parity ([`ModelOptions::strips`]). Vertices are written in Halo 2's
//! compressed form: s16 positions and texture coordinates against the model's
//! one compression record, packed tangent frames, and one or two node
//! indices (through a per-section node map) with u8 weights.
//!
//! Field layouts and their evidence are in `docs/halo1/models.md`.

use map::TagId;
use map::build::{Block, RawId, RawRef};

use crate::h1::{self, Model, Vertex};
use crate::out::{BlockExt, Converted, SELF_ID, array};
use crate::{Error, Result};

const HEADER_LEN: usize = 0x84;
const COMPRESSION_LEN: usize = 0x38;
const REGION_LEN: usize = 0x10;
const PERMUTATION_LEN: usize = 0x10;
const SECTION_LEN: usize = 0x5c;
const NODE_LEN: usize = 0x60;
const MARKER_GROUP_LEN: usize = 0x0c;
const MARKER_LEN: usize = 0x24;
const MATERIAL_LEN: usize = 0x20;
/// Raw section struct (`MODEL_SECTION` in crates/tags).
const SECTION_STRUCT: usize = 0x70;
const PART_LEN: usize = 0x48;
const SUBPART_LEN: usize = 8;
const DESCRIPTOR_LEN: usize = 0x20;
const BLOCK_INFO: usize = 0x38;
/// Part flag: indices are a triangle list.
pub const PART_TRIANGLE_LIST: u16 = 0x8;
/// Part kinds: opaque, transparent (kinds 0 and 5 are never drawn).
pub const PART_KIND_OPAQUE: u16 = 2;
pub const PART_KIND_TRANSPARENT: u16 = 4;

/// Vertex buffer types (docs/formats/map-format.md, "Vertex-buffer types").
const VB_ONE_NODE: u8 = 0x04;
const VB_TWO_NODES: u8 = 0x06;
const VB_TEXCOORD16: u8 = 0x19;
const VB_TANGENT_FRAME: u8 = 0x1b;

/// Per-model inputs the converter supplies.
#[derive(Clone, Debug, Default)]
pub struct ModelOptions {
    /// The Halo 2 `shad` tag for each Halo 1 shader block entry; missing or
    /// [`TagId::NULL`] leaves the material's shader null.
    pub shaders: Vec<TagId>,
    /// Write each part as a triangle strip in Halo 2's parity
    /// ([`h1::h2_strip`]) instead of a list: for a sky's model. The sky
    /// draw (`0x0004c690`, called from the sky pass `0x0003ede0`) draws
    /// every part as a strip whatever its flags: `0x0004c974` pushes
    /// primitive 6 (triangle strip) to `DrawIndexedVertices` with the
    /// part's first index (`+6`) and count (`+8`). A list drawn that way
    /// joins each triangle to the next two: Blood Gulch's ring drew as
    /// white slabs across the sky.
    pub strips: bool,
}

/// The marker names Halo 2's engine looks up by string IDs written in its
/// code, each through the marker lookup `0x000b8d30` (E, every use of the
/// string ID in the XBE; `docs/halo1/models.md`, "Marker names"):
/// `left_hand` is where a thrown grenade leaves the hand (`0x000e7730`);
/// the others' call sites are listed there.
pub const ENGINE_MARKERS: &[&str] = &[
    "body",
    "bumper",
    "drop",
    "drop_left",
    "drop_right",
    "flashlight",
    "fp_body_cam",
    "ground_point",
    "head",
    "hover_thrusters",
    "left_hand",
    "melee",
    "primary_trigger",
];

/// A Halo 1 marker name in Halo 2's form: lower case, words joined by
/// underscores (`left hand` is `left_hand`, `primary trigger` is
/// `primary_trigger`).
pub fn h2_marker_name(h1: &str) -> String {
    h1.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

/// The extra marker groups a converted model carries: each name of
/// [`ENGINE_MARKERS`] the model lacks but has a Halo 1 marker for (in
/// Halo 2's form, [`h2_marker_name`]), with that marker's index in `names`.
/// Halo 1's own names stay, so the names a converted tag writes (seats,
/// attachments, effects, unit hands) still resolve; the engine's search
/// (`0x001d8f00`) takes the first group whose name matches, so an extra
/// group changes no existing lookup.
pub fn marker_aliases(names: &[&str]) -> Vec<(&'static str, usize)> {
    ENGINE_MARKERS
        .iter()
        .filter(|h2| !names.iter().any(|n| n == *h2))
        .filter_map(|&h2| {
            names
                .iter()
                .position(|n| h2_marker_name(n) == h2)
                .map(|i| (h2, i))
        })
        .collect()
}

/// Packs a unit vector into Halo 2's 11:11:10 form (x in the low bits).
pub fn pack_normal(v: [f32; 3]) -> u32 {
    let q =
        |x: f32, scale: f32, mask: u32| ((x.clamp(-1.0, 1.0) * scale).round() as i32 as u32) & mask;
    q(v[0], 1023.0, 0x7ff) | q(v[1], 1023.0, 0x7ff) << 11 | q(v[2], 511.0, 0x3ff) << 22
}

/// Maps `v` in `[min, max]` to the s16 the decoder expands back:
/// `(s / 32767 + 1) / 2 * (max - min) + min`.
pub fn quantize(v: f32, min: f32, max: f32) -> i16 {
    let t = ((v - min) / (max - min)).clamp(0.0, 1.0) * 2.0 - 1.0;
    (t * 32767.0).round() as i16
}

/// Bounds covering every vertex, never empty on an axis.
fn bounds(model: &Model) -> ([f32; 6], [f32; 4]) {
    let mut p = [f32::MAX, f32::MIN, f32::MAX, f32::MIN, f32::MAX, f32::MIN];
    let mut t = [f32::MAX, f32::MIN, f32::MAX, f32::MIN];
    let mut any = false;
    for v in model
        .geometries
        .iter()
        .flat_map(|g| &g.parts)
        .flat_map(|p| &p.vertices)
    {
        any = true;
        for a in 0..3 {
            p[2 * a] = p[2 * a].min(v.position[a]);
            p[2 * a + 1] = p[2 * a + 1].max(v.position[a]);
        }
        let uv = texcoord(model, v);
        for a in 0..2 {
            t[2 * a] = t[2 * a].min(uv[a]);
            t[2 * a + 1] = t[2 * a + 1].max(uv[a]);
        }
    }
    if !any {
        return ([-1.0, 1.0, -1.0, 1.0, -1.0, 1.0], [0.0, 1.0, 0.0, 1.0]);
    }
    let widen = |b: &mut [f32]| {
        for pair in b.chunks_exact_mut(2) {
            if pair[1] - pair[0] < 1e-4 {
                pair[0] -= 1e-3;
                pair[1] += 1e-3;
            }
        }
    };
    widen(&mut p);
    widen(&mut t);
    (p, t)
}

fn texcoord(model: &Model, v: &Vertex) -> [f32; 2] {
    let s = model.base_map_scale;
    let s = [
        if s[0] == 0.0 { 1.0 } else { s[0] },
        if s[1] == 0.0 { 1.0 } else { s[1] },
    ];
    [v.texcoord[0] * s[0], v.texcoord[1] * s[1]]
}

/// Converts a Halo 1 model. `name` becomes the model's string-ID name.
pub fn convert_model(model: &Model, name: &str, opts: &ModelOptions) -> Result<Converted> {
    let (pos, uv) = bounds(model);
    let mut raw = Vec::new();
    let mut sections = Vec::new();
    for (gi, g) in model.geometries.iter().enumerate() {
        let (section, bytes) = build_section(model, gi, g, pos, uv, opts.strips)?;
        raw.push((bytes, 0x10));
        sections.push(section(RawId(raw.len() - 1)));
    }

    let mut compression = Block::zeroed(COMPRESSION_LEN);
    compression.put_f32s(0, &pos).put_f32s(0x18, &uv);

    let regions = model
        .regions
        .iter()
        .map(|r| {
            let perms = r
                .permutations
                .iter()
                .map(|p| {
                    let mut b = Block::zeroed(PERMUTATION_LEN);
                    b.string_id(0, &p.name);
                    // Halo 1: super low .. super high (5); Halo 2: LOD 0
                    // (lowest) .. 5 (highest). Super high fills both 4 and 5.
                    let l = p.lods;
                    for (i, s) in [l[0], l[1], l[2], l[3], l[4], l[4]].into_iter().enumerate() {
                        b.put_i16(4 + 2 * i, s);
                    }
                    b
                })
                .collect::<Vec<_>>();
            let mut b = Block::zeroed(REGION_LEN);
            b.string_id(0, &r.name).put_i16(4, -1).put_i16(6, 0);
            b.block(8, perms.len() as u32, array(perms, PERMUTATION_LEN));
            b
        })
        .collect::<Vec<_>>();

    let nodes = node_blocks(model);

    // Marker groups: one per Halo 1 marker name, then each Halo 2 name the
    // engine looks up for one of them ([`marker_aliases`]).
    let named = model
        .markers
        .iter()
        .map(|m| (m.name.as_str(), m))
        .chain(
            marker_aliases(&model.markers.iter().map(|m| m.name.as_str()).collect::<Vec<_>>())
                .into_iter()
                .map(|(alias, i)| (alias, &model.markers[i])),
        )
        .collect::<Vec<_>>();
    let groups = named
        .into_iter()
        .map(|(name, m)| {
            let markers = m
                .instances
                .iter()
                .map(|i| {
                    let mut b = Block::zeroed(MARKER_LEN);
                    b.put_u8(0, i.region as u8)
                        .put_u8(1, i.permutation as u8)
                        .put_u8(2, i.node as u8)
                        .put_f32s(4, &i.translation)
                        .put_f32s(0x10, &i.rotation);
                    b
                })
                .collect::<Vec<_>>();
            let mut b = Block::zeroed(MARKER_GROUP_LEN);
            b.string_id(0, name);
            b.block(4, markers.len() as u32, array(markers, MARKER_LEN));
            b
        })
        .collect::<Vec<_>>();

    let materials = (0..model.shaders.len())
        .map(|i| {
            let mut b = Block::zeroed(MATERIAL_LEN);
            b.tag_ref(0, TagId::NULL)
                .tag_ref(8, opts.shaders.get(i).copied().unwrap_or(TagId::NULL));
            b
        })
        .collect::<Vec<_>>();

    let mut h = Block::zeroed(HEADER_LEN);
    h.string_id(0, name);
    h.block(0x14, 1, compression);
    h.block(0x1c, regions.len() as u32, array(regions, REGION_LEN));
    h.block(0x24, sections.len() as u32, array(sections, SECTION_LEN));
    h.block(SECTION_GROUPS as u32, 1, section_group());
    // +0x44 stays zero, as on every retail mode. Halo 1's node list
    // checksum was written here; a value that happens to fall in the map's
    // address range reads as a stray pointer, and a fresh level refuses
    // the tag (c40's "cortana powerup fx").
    h.block(0x48, nodes.len() as u32, array(nodes, NODE_LEN));
    h.block(0x58, groups.len() as u32, array(groups, MARKER_GROUP_LEN));
    h.block(0x60, materials.len() as u32, array(materials, MATERIAL_LEN));
    Ok(Converted {
        group: map::FourCC(*b"mode"),
        root: h,
        raw,
    })
}

/// `mode +0x34`: section groups (`0xc` each); `+0x3c..+0x42` holds one
/// group index per detail level (zero: the first group).
pub const SECTION_GROUPS: usize = 0x34;

/// The one section group every converted model gets: all six detail
/// levels (`+0` `0x3f`), no compound nodes (`+4` block, empty).
///
/// **E** (`0x00137650`): the engine counts a model's node maps as the
/// compound nodes of `groups[lod_index[lod]]` plus the node count, with no
/// check on the group count; with none it read a null block (a30's second
/// fault, `0x0013767a`, on the converted ammo pack). **V** (05a, 181
/// retail `mode`s): every one has exactly this one group and zero indices.
fn section_group() -> Block {
    let mut g = Block::zeroed(0xc);
    g.put_u16(0, 0x3f);
    g
}

/// Default-pose node blocks, with the inverse of each node's model-space
/// transform.
fn node_blocks(model: &Model) -> Vec<Block> {
    default_pose(model, NODE_LEN)
}

/// Stride of an `hlmt` node (`hlmt +0x78`).
pub const HLMT_NODE_LEN: usize = 0x5c;

/// The `hlmt` nodes for a converted model, as one block-array child (count
/// = the model's node count, stride [`HLMT_NODE_LEN`]).
///
/// **E** (`object_new` `0x000b7ef3`, `0x000b800b`): an object reserves
/// `0x34` bytes of node matrices per `hlmt` node, at least one. The render
/// model and animation graph write one matrix per *their* node, so an
/// `hlmt` with fewer nodes than its `mode` lets every update run past the
/// object's memory into the next object's header. **V** (`05a`'s SMG,
/// warthog, banshee, Master Chief): a retail `hlmt` node is the `mode`
/// node's first `0x5c` bytes (no distance from parent), with `+0xa` zero.
pub fn hlmt_nodes(model: &Model) -> (u32, Block) {
    let nodes = default_pose(model, HLMT_NODE_LEN)
        .into_iter()
        .map(|mut b| {
            b.put_i16(0xa, 0);
            b
        })
        .collect::<Vec<_>>();
    (nodes.len() as u32, array(nodes, HLMT_NODE_LEN))
}

/// Node blocks of `len` bytes (`mode`'s `0x60`, or `hlmt`'s `0x5c`, which
/// stops before the distance from the parent).
fn default_pose(model: &Model, len: usize) -> Vec<Block> {
    let world = world_transforms(
        &model
            .nodes
            .iter()
            .map(|n| (n.parent, n.rotation, n.translation))
            .collect::<Vec<_>>(),
    );
    model
        .nodes
        .iter()
        .zip(&world)
        .map(|(n, (m, p))| {
            let mut b = Block::zeroed(len);
            b.string_id(0, &n.name)
                .put_i16(4, n.parent)
                .put_i16(6, n.first_child)
                .put_i16(8, n.next_sibling)
                .put_i16(0xa, -1)
                .put_f32s(0xc, &n.translation)
                .put_f32s(0x18, &n.rotation);
            // +0x28 default scale; +0x2c the inverse of the model-space
            // transform: its three columns (forward, left, up: the rows of
            // R, as every retail node stores them), then the position,
            // -R^T p.
            let inv_p = [0, 1, 2].map(|c| -(0..3).map(|r| m[r][c] * p[r]).sum::<f32>());
            b.put_f32(0x28, 1.0);
            for c in 0..3 {
                b.put_f32s(0x2c + 12 * c, &m[c]);
            }
            b.put_f32s(0x50, &inv_p);
            if len >= NODE_LEN {
                b.put_f32(0x5c, n.distance_from_parent);
            }
            b
        })
        .collect()
}

/// Row-major rotation matrix of a unit quaternion `[x, y, z, w]` acting on
/// column vectors.
pub fn quat_matrix(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [x, y, z, w] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Model-space (rotation, position) of each node from parent-relative
/// rotations and translations; nodes may come in any order.
pub fn world_transforms(nodes: &[(i16, [f32; 4], [f32; 3])]) -> Vec<([[f32; 3]; 3], [f32; 3])> {
    let mut out: Vec<Option<([[f32; 3]; 3], [f32; 3])>> = vec![None; nodes.len()];
    fn solve(
        i: usize,
        nodes: &[(i16, [f32; 4], [f32; 3])],
        out: &mut Vec<Option<([[f32; 3]; 3], [f32; 3])>>,
        depth: usize,
    ) -> ([[f32; 3]; 3], [f32; 3]) {
        if let Some(v) = out[i] {
            return v;
        }
        let (parent, q, t) = nodes[i];
        let local = quat_matrix(q);
        let v = if parent >= 0 && (parent as usize) < nodes.len() && depth < nodes.len() {
            let (pm, pp) = solve(parent as usize, nodes, out, depth + 1);
            let m = std::array::from_fn(|r| {
                std::array::from_fn(|c| (0..3).map(|k| pm[r][k] * local[k][c]).sum())
            });
            let p = std::array::from_fn(|r| pp[r] + (0..3).map(|k| pm[r][k] * t[k]).sum::<f32>());
            (m, p)
        } else {
            (local, t)
        };
        out[i] = Some(v);
        v
    }
    (0..nodes.len())
        .map(|i| solve(i, nodes, &mut out, 0))
        .collect()
}

type SectionFn = Box<dyn FnOnce(RawId) -> Block>;

/// Builds one section: returns its `ModeSection` record (given the raw
/// resource it will point at) and the raw block bytes.
fn build_section(
    model: &Model,
    index: usize,
    g: &h1::Geometry,
    pos: [f32; 6],
    uv: [f32; 4],
    strips: bool,
) -> Result<(SectionFn, Vec<u8>)> {
    // Node map: the nodes this geometry's vertices use, ascending.
    let mut used: Vec<i16> = g
        .parts
        .iter()
        .flat_map(|p| &p.vertices)
        .flat_map(|v| v.nodes)
        .filter(|&n| n >= 0)
        .collect();
    used.sort_unstable();
    used.dedup();
    if used.len() > 255 {
        return Err(Error::Unsupported(
            "more than 255 nodes in one section".into(),
        ));
    }
    let local = |n: i16| used.iter().position(|&u| u == n).unwrap_or(0) as u8;
    let weighted = g
        .parts
        .iter()
        .flat_map(|p| &p.vertices)
        .any(|v| v.nodes[1] >= 0 && v.weights[1] > 0.0 && v.nodes[1] != v.nodes[0]);

    let mut positions = Vec::new();
    let mut texcoords = Vec::new();
    let mut frames = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    let mut parts = Vec::new();
    let mut subparts = Vec::new();
    let mut triangles = 0usize;
    for (pi, p) in g.parts.iter().enumerate() {
        let first_vertex = positions.len() / if weighted { 12 } else { 8 };
        if first_vertex + p.vertices.len() > 0xffff {
            return Err(Error::Unsupported(format!(
                "geometry {index} has more than 65535 vertices"
            )));
        }
        for v in &p.vertices {
            for a in 0..3 {
                positions.extend(quantize(v.position[a], pos[2 * a], pos[2 * a + 1]).to_le_bytes());
            }
            if weighted {
                let w0 = if v.nodes[1] >= 0 && v.nodes[1] != v.nodes[0] {
                    (v.weights[0].clamp(0.0, 1.0) * 255.0).round() as u8
                } else {
                    255
                };
                let n1 = if v.nodes[1] >= 0 {
                    local(v.nodes[1])
                } else {
                    0
                };
                positions.extend([0, 0, local(v.nodes[0].max(0)), n1, w0, 255 - w0]);
            } else {
                positions.extend([local(v.nodes[0].max(0)), 0]);
            }
            let t = texcoord(model, v);
            texcoords.extend(quantize(t[0], uv[0], uv[1]).to_le_bytes());
            texcoords.extend(quantize(t[1], uv[2], uv[3]).to_le_bytes());
            for n in [v.normal, v.binormal, v.tangent] {
                frames.extend(pack_normal(n).to_le_bytes());
            }
        }
        let tris = h1::strip_triangles(&p.strip);
        triangles += tris.len();
        let first_index = indices.len();
        let base = first_vertex as u16;
        if strips {
            indices.extend(h1::h2_strip(&p.strip).into_iter().map(|i| i + base));
        } else {
            indices.extend(tris.iter().flatten().map(|&i| i + base));
        }
        let count = indices.len() - first_index;
        if indices.len() > 0xffff {
            return Err(Error::Unsupported(format!(
                "geometry {index} has more than 65535 indices"
            )));
        }
        // A part naming a shader past the model's list has no material to
        // draw with: a named error, not an opaque part.
        let shader = match usize::try_from(p.shader) {
            Ok(s) => Some(model.shaders.get(s).ok_or_else(|| {
                Error::Invalid(format!(
                    "geometry {index}: part shader {s} past the model's {} shaders",
                    model.shaders.len()
                ))
            })?),
            Err(_) => None,
        };
        let kind = match shader {
            Some(s) if s.group == h1map::Group::new(b"soso") => PART_KIND_OPAQUE,
            Some(_) => PART_KIND_TRANSPARENT,
            None => PART_KIND_OPAQUE,
        };
        let mut part = vec![0u8; PART_LEN];
        let put16 =
            |b: &mut [u8], at: usize, v: u16| b[at..at + 2].copy_from_slice(&v.to_le_bytes());
        put16(&mut part, 0, kind);
        put16(&mut part, 2, if strips { 0 } else { PART_TRIANGLE_LIST });
        put16(
            &mut part,
            4,
            if p.shader < 0 {
                u16::MAX
            } else {
                p.shader as u16
            },
        );
        put16(&mut part, 6, first_index as u16);
        put16(&mut part, 8, count as u16);
        put16(&mut part, 0xa, subparts.len() as u16 / SUBPART_LEN as u16);
        put16(&mut part, 0xc, 1);
        part[0xe] = if weighted { 2 } else { 1 };
        part[0xf] = 1;
        for (a, c) in p.centroid.iter().enumerate() {
            part[0x10 + 4 * a..0x14 + 4 * a].copy_from_slice(&c.to_le_bytes());
        }
        part[0x1c] = local(p.centroid_nodes[0].max(0));
        part[0x1d] = if p.centroid_nodes[1] >= 0 {
            local(p.centroid_nodes[1])
        } else {
            0
        };
        for (a, w) in p.centroid_weights.iter().enumerate() {
            part[0x20 + 4 * a..0x24 + 4 * a].copy_from_slice(&w.to_le_bytes());
        }
        parts.extend(part);
        // A strip part keeps an empty placeholder subpart, as retail's do
        // (docs/formats/map-format.md, `MODEL_SECTION`).
        let mut sub = vec![0u8; SUBPART_LEN];
        if !strips {
            put16(&mut sub, 0, first_index as u16);
            put16(&mut sub, 2, count as u16);
        }
        put16(&mut sub, 4, u16::MAX);
        put16(&mut sub, 6, pi as u16);
        subparts.extend(sub);
    }
    let vertex_count = positions.len() / if weighted { 12 } else { 8 };
    let position_type = if weighted { VB_TWO_NODES } else { VB_ONE_NODE };
    let position_stride = if weighted { 12 } else { 8 };
    let mut descriptors = vec![0u8; 3 * DESCRIPTOR_LEN];
    for (i, (t, s)) in [
        (position_type, position_stride),
        (VB_TEXCOORD16, 4),
        (VB_TANGENT_FRAME, 12),
    ]
    .into_iter()
    .enumerate()
    {
        descriptors[i * DESCRIPTOR_LEN] = t;
        descriptors[i * DESCRIPTOR_LEN + 1] = s;
    }
    let index_bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let node_map: Vec<u8> = used.iter().map(|&n| n as u8).collect();

    // Section struct: block counts; the engine's fixup writes the pointers.
    let mut section = vec![0u8; SECTION_STRUCT];
    let mut resources: Vec<(u8, u16, u16, Vec<u8>)> = Vec::new();
    let mut add_block = |section: &mut Vec<u8>, field: u16, stride: u16, data: Vec<u8>| {
        let count = (data.len() / usize::from(stride)) as u32;
        section[usize::from(field)..usize::from(field) + 4].copy_from_slice(&count.to_le_bytes());
        if count != 0 {
            resources.push((0, field, stride, data));
        }
    };
    add_block(&mut section, 0x00, PART_LEN as u16, parts);
    add_block(&mut section, 0x08, SUBPART_LEN as u16, subparts);
    add_block(&mut section, 0x20, 2, index_bytes);
    add_block(&mut section, 0x38, DESCRIPTOR_LEN as u16, descriptors);
    add_block(&mut section, 0x64, 1, node_map.clone());
    if vertex_count != 0 {
        for (i, data) in [positions, texcoords, frames].into_iter().enumerate() {
            resources.push((2, 0x38, i as u16, data));
        }
    }

    // Resource area: each resource behind an `rsrc` marker, 4-aligned.
    let mut area = Vec::new();
    let mut table = Vec::new();
    for (kind, field, secondary, data) in &resources {
        area.extend(fourcc(b"rsrc"));
        let offset = area.len() as u32;
        area.extend(data);
        while area.len() % 4 != 0 {
            area.push(0);
        }
        let mut r = vec![0u8; 0x10];
        r[0] = *kind;
        r[1] = 2;
        r[2..4].copy_from_slice(&field.to_le_bytes());
        r[4..6].copy_from_slice(&field.to_le_bytes());
        r[6..8].copy_from_slice(&secondary.to_le_bytes());
        r[8..0xc].copy_from_slice(&(data.len() as u32).to_le_bytes());
        r[0xc..0x10].copy_from_slice(&offset.to_le_bytes());
        table.push(Block::new(r));
    }
    let mut raw = Vec::with_capacity(8 + SECTION_STRUCT + area.len() + 4);
    raw.extend(fourcc(b"blkh"));
    raw.extend((area.len() as u32).to_le_bytes());
    raw.extend(&section);
    raw.extend(&area);
    raw.extend(fourcc(b"blkf"));
    let block_size = raw.len() as u32;
    let area_size = area.len() as u32;
    let classification: u16 = if weighted { 3 } else { 2 };
    let part_count = g.parts.len() as u16;
    let resource_count = table.len() as u32;
    let table = array(table, 0x10);
    let max_nodes = if weighted { 2 } else { 1 };
    let node_count = node_map.len() as u8;
    let record = move |raw_id: RawId| {
        let mut s = Block::zeroed(SECTION_LEN);
        s.put_u16(0, classification)
            .put_u16(4, vertex_count as u16)
            .put_u16(6, triangles as u16)
            .put_u16(8, part_count)
            .put_u8(0x14, max_nodes)
            .put_u16(0x18, classification)
            .put_u16(0x1a, 3) // compressed positions and texture coordinates
            .put_u8(0x24, node_count)
            .put_u8(0x25, node_count)
            .put_u16(0x28, part_count)
            .put_i16(0x2c, -1);
        let b = BLOCK_INFO;
        s.raw(b as u32, RawRef::Local(raw_id))
            .put_u32(b + 4, block_size)
            .put_u32(b + 8, SECTION_STRUCT as u32)
            .put_u32(b + 0xc, area_size)
            .block((b + 0x10) as u32, resource_count, table)
            .tag_id((b + 0x18) as u32, SELF_ID)
            .put_i16(b + 0x1c, 0x30i16 - BLOCK_INFO as i16)
            .put_u8(b + 0x1f, 1)
            .put_u32(b + 0x20, u32::MAX);
        s
    };
    Ok((Box::new(record), raw))
}

/// A FourCC as the engine stores it: its big-endian value written
/// little-endian (`blkh` is the bytes `hklb`).
fn fourcc(code: &[u8; 4]) -> [u8; 4] {
    map::FourCC(*code).as_u32().to_le_bytes()
}
