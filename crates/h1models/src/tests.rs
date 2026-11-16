//! Conversion tests. Synthetic tests build invented Halo 1 tags in memory;
//! the `#[ignore]` tests read real maps from `$H1_MAPS` (a directory holding
//! `beavercreek.map`). Each converted tag is installed in a built v8 map and
//! decoded back through `crates/tags`, then compared with the source.

use std::io::Cursor;

use map::build::TagSet;
use map::{CacheType, Map, RawPointer, ResourceFile, TagId};
use tags::anim::AnimationGraph;
use tags::model::RenderModelGeometry;

use crate::h1::{self, Memory};
use crate::mode::{ModelOptions, convert_model, pack_normal, quantize};
use crate::{Converted, Error, Result, jmad};

// ---------------------------------------------------------------------------
// Building and reading back
// ---------------------------------------------------------------------------

/// Builds a v8 map holding just `tags` and returns its bytes and their datums.
fn build(tags: Vec<(Converted, &str)>) -> (Vec<u8>, Vec<TagId>) {
    let mut set = TagSet::new(CacheType::Multiplayer, "h1test", "invented\\h1test");
    let ids = tags
        .into_iter()
        .map(|(c, name)| c.install(&mut set, name).expect("install"))
        .collect();
    let (bytes, _) = set.build().expect("map builds");
    (bytes, ids)
}

struct Raw(Vec<u8>);
impl tags::geometry::RawSource for Raw {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        assert_eq!(p.file, ResourceFile::Local);
        Ok(self.0[p.offset as usize..(p.offset + size) as usize].to_vec())
    }
}
impl tags::sound::ResourceReader for Raw {
    fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
        assert!((p.offset + size) as usize <= self.0.len());
        Ok(())
    }
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        tags::geometry::RawSource::read_raw(self, p, size)
    }
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// Measured differences between a Halo 1 model and its decoded conversion.
#[derive(Debug, Default)]
struct ModelDiff {
    max_position: f32,
    max_texcoord: f32,
    max_normal: f32,
    max_weight: f32,
    vertices: usize,
    triangles: usize,
    /// Triangles whose winding disagrees with their first vertex's normal.
    winding_disagree: usize,
}

fn compare_model(src: &h1::Model, bytes: Vec<u8>, id: TagId) -> ModelDiff {
    let mut map = Map::from_reader(Cursor::new(bytes.clone())).expect("map reads");
    let m = RenderModelGeometry::read(&mut map, id, &mut Raw(bytes)).expect("model decodes");
    let strings = map.strings().to_vec();
    let resolve = |s: tags::StringId| s.resolve(&strings).unwrap().unwrap_or("").to_owned();

    // Nodes: names, hierarchy, default pose.
    assert_eq!(m.nodes.len(), src.nodes.len());
    for (a, b) in m.nodes.iter().zip(&src.nodes) {
        assert_eq!(a.name.as_deref(), Some(b.name.as_str()));
        assert_eq!(a.wire.parent_index, b.parent);
        assert_eq!(a.wire.first_child_index, b.first_child);
        assert_eq!(a.wire.next_sibling_index, b.next_sibling);
        assert_eq!(a.wire.translation, b.translation);
        assert_eq!(a.wire.orientation, b.rotation);
    }
    // Regions, permutations, LODs.
    assert_eq!(m.regions.len(), src.regions.len());
    for (a, b) in m.regions.iter().zip(&src.regions) {
        assert_eq!(a.name.as_deref(), Some(b.name.as_str()));
        for (pa, pb) in a.permutations.iter().zip(&b.permutations) {
            assert_eq!(pa.name.as_deref(), Some(pb.name.as_str()));
            let want = pb.lods.map(|l| usize::try_from(l).ok());
            assert_eq!(pa.lod_sections[..5], want);
            assert_eq!(pa.lod_sections[5], want[4]);
        }
    }
    assert_eq!(m.materials.len(), src.shaders.len());

    // Markers: not decoded by crates/tags; read the marker-group block.
    let tag = map.read_tag_bytes(id).unwrap();
    let base = map.tag(id).unwrap().location.unwrap().address;
    let u32_at = |o: usize| u32::from_le_bytes(tag[o..o + 4].try_into().unwrap());
    let f32_at = |o: usize| f32::from_bits(u32_at(o));
    let (count, at) = (u32_at(0x58) as usize, (u32_at(0x5c) - base) as usize);
    assert_eq!(count, src.markers.len());
    for (k, mk) in src.markers.iter().enumerate() {
        let g = at + k * 0xc;
        assert_eq!(resolve(tags::StringId(u32_at(g))), mk.name);
        assert_eq!(u32_at(g + 4) as usize, mk.instances.len());
        let ma = (u32_at(g + 8) - base) as usize;
        for (j, inst) in mk.instances.iter().enumerate() {
            let e = ma + j * 0x24;
            assert_eq!(
                [tag[e] as i8, tag[e + 1] as i8, tag[e + 2] as i8],
                [inst.region, inst.permutation, inst.node]
            );
            for c in 0..3 {
                assert_eq!(f32_at(e + 4 + 4 * c), inst.translation[c]);
            }
            for c in 0..4 {
                assert_eq!(f32_at(e + 0x10 + 4 * c), inst.rotation[c]);
            }
        }
    }

    // Geometry: every vertex and triangle of every part.
    let mut d = ModelDiff::default();
    assert_eq!(m.sections.len(), src.geometries.len());
    for (s, g) in m.sections.iter().zip(&src.geometries) {
        assert_eq!(s.geometry.parts.len(), g.parts.len());
        let v = &s.vertices;
        let mut base_vertex = 0usize;
        for (p, sp) in s.geometry.parts.iter().zip(&g.parts) {
            let tris = s.geometry.part_triangles(p);
            let want = h1::strip_triangles(&sp.strip);
            assert_eq!(tris.len(), want.len(), "triangle count");
            for (a, b) in tris.iter().zip(&want) {
                assert_eq!(a.map(usize::from), b.map(|i| usize::from(i) + base_vertex));
                // Winding against the stored normal.
                let [p0, p1, p2] = b.map(|i| sp.vertices[usize::from(i)].position);
                let e1: [f32; 3] = std::array::from_fn(|k| p1[k] - p0[k]);
                let e2: [f32; 3] = std::array::from_fn(|k| p2[k] - p0[k]);
                let n = [
                    e1[1] * e2[2] - e1[2] * e2[1],
                    e1[2] * e2[0] - e1[0] * e2[2],
                    e1[0] * e2[1] - e1[1] * e2[0],
                ];
                let vn = sp.vertices[usize::from(b[0])].normal;
                if n.iter().zip(&vn).map(|(x, y)| x * y).sum::<f32>() < 0.0 {
                    d.winding_disagree += 1;
                }
            }
            d.triangles += tris.len();
            for (k, sv) in sp.vertices.iter().enumerate() {
                let i = base_vertex + k;
                for a in 0..3 {
                    d.max_position = d
                        .max_position
                        .max((v.positions[i][a] - sv.position[a]).abs());
                    d.max_normal = d.max_normal.max((v.normals[i][a] - sv.normal[a]).abs());
                }
                let scale = src.base_map_scale.map(|x| if x == 0.0 { 1.0 } else { x });
                for a in 0..2 {
                    d.max_texcoord = d
                        .max_texcoord
                        .max((v.texcoords[i][a] - sv.texcoord[a] * scale[a]).abs());
                }
                // Skinning: node through the section's node map, and weight.
                let node = |local: u8| {
                    if s.node_map.is_empty() {
                        local as i16
                    } else {
                        s.node_map[usize::from(local)] as i16
                    }
                };
                assert_eq!(
                    node(v.node_indices[i][0]),
                    sv.nodes[0].max(0),
                    "vertex node"
                );
                if sv.nodes[1] >= 0 && sv.nodes[1] != sv.nodes[0] && sv.weights[1] > 0.0 {
                    assert_eq!(node(v.node_indices[i][1]), sv.nodes[1]);
                    d.max_weight = d
                        .max_weight
                        .max((v.node_weights[i][0] - sv.weights[0]).abs());
                } else {
                    d.max_weight = d.max_weight.max((v.node_weights[i][0] - 1.0).abs());
                }
            }
            base_vertex += sp.vertices.len();
            d.vertices += sp.vertices.len();
        }
    }
    d
}

/// Largest per-channel difference between Halo 1 frames and the decoded
/// Halo 2 clip, over every frame of the animation: (rotation as angle in
/// radians, translation, scale).
fn compare_animation(src: &h1::Animation, clip: &tags::anim_sample::AnimationClip) -> [f32; 3] {
    let mut e = [0f32; 3];
    assert_eq!(clip.frame_count(), usize::from(src.frame_count));
    for f in 0..clip.frame_count() {
        let pose = clip.sample_frame(f).expect("frame samples");
        for (n, (got, want)) in pose.nodes.iter().zip(&src.frames[f]).enumerate() {
            let carried = |c: usize| src.has(c, n) || src.kind == h1::AnimType::Base;
            if carried(0) {
                assert!(got.has_rotation);
                // Rotation angle between the two: 4 asin(|q1 - q2| / 2) on
                // the same hemisphere (acos of the dot product would turn
                // f32 rounding into ~1e-3 rad).
                let dot: f32 = got
                    .rotation
                    .iter()
                    .zip(&want.rotation)
                    .map(|(a, b)| a * b)
                    .sum();
                let s = if dot < 0.0 { -1.0 } else { 1.0 };
                let chord = (0..4)
                    .map(|k| (got.rotation[k] - s * want.rotation[k]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                e[0] = e[0].max(4.0 * (chord / 2.0).min(1.0).asin());
            }
            if carried(1) {
                assert!(got.has_translation);
                for a in 0..3 {
                    e[1] = e[1].max((got.translation[a] - want.translation[a]).abs());
                }
            }
            if carried(2) {
                assert!(got.has_scale);
                e[2] = e[2].max((got.scale - want.scale).abs());
            }
        }
    }
    e
}

/// Decodes a converted graph from a built map and compares every animation.
fn check_graph(
    src: &h1::Graph,
    skeleton: &[h1::AnimNode],
    bytes: Vec<u8>,
    id: TagId,
) -> Vec<(String, [f32; 3])> {
    let mut map = Map::from_reader(Cursor::new(bytes.clone())).expect("map reads");
    let g = AnimationGraph::read(&mut map, id).expect("graph decodes");
    let strings = map.strings().to_vec();
    let nodes = if src.nodes.is_empty() {
        skeleton
    } else {
        &src.nodes
    };
    assert_eq!(g.nodes.len(), nodes.len());
    for (a, b) in g.nodes.iter().zip(nodes) {
        assert_eq!(a.name.resolve(&strings).unwrap(), Some(b.name.as_str()));
        assert_eq!(
            (a.parent, a.first_child, a.next_sibling),
            (b.parent, b.first_child, b.next_sibling)
        );
    }
    assert_eq!(g.animations.len(), src.animations.len());
    let mut raw = Raw(bytes);
    let resource = g.read_resource(0, &mut raw).expect("resource");
    // Every action names an animation in range.
    for m in &g.modes {
        for c in &m.classes {
            for t in &c.types {
                for a in &t.actions {
                    assert!((a.animation_index as usize) < g.animations.len());
                }
            }
        }
    }
    src.animations
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let clip = g.clip_from_shared(i, &resource).expect("clip");
            (a.name.clone(), compare_animation(a, &clip))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Synthetic Halo 1 tags
// ---------------------------------------------------------------------------

const BASE: u32 = 0x8040_0000;

/// A bump-allocated image of Halo 1 tag data.
#[derive(Default)]
struct Image(Vec<u8>);
impl Memory for Image {
    fn read(&self, address: u32, len: usize) -> Result<&[u8]> {
        let at = address
            .checked_sub(BASE)
            .ok_or(Error::Truncated("address"))? as usize;
        self.0.get(at..at + len).ok_or(Error::Truncated("image"))
    }
}
impl Image {
    fn alloc(&mut self, bytes: &[u8]) -> u32 {
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0);
        }
        let at = BASE + self.0.len() as u32;
        self.0.extend(bytes);
        at
    }
}
fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn putf(b: &mut [u8], at: usize, v: &[f32]) {
    for (i, x) in v.iter().enumerate() {
        put(b, at + 4 * i, x.to_bits());
    }
}
fn put16(b: &mut [u8], at: usize, v: i16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn name(b: &mut [u8], at: usize, s: &str) {
    b[at..at + s.len()].copy_from_slice(s.as_bytes());
}
fn block(b: &mut [u8], at: usize, count: usize, address: u32) {
    put(b, at, count as u32);
    put(b, at + 4, address);
}

fn pack_h1_normal(v: [f32; 3]) -> u32 {
    pack_normal(v)
}

/// An invented two-node model: one region with one permutation, one
/// geometry with two parts (one skinned across both nodes), one marker.
fn synthetic_model() -> (Image, u32) {
    let mut img = Image::default();
    let mut nodes = vec![0u8; 2 * 0x9c];
    name(&mut nodes, 0, "root");
    put16(&mut nodes, 0x20, -1);
    put16(&mut nodes, 0x22, 1);
    put16(&mut nodes, 0x24, -1);
    putf(&mut nodes, 0x34, &[0.0, 0.0, 0.0, 1.0]);
    name(&mut nodes, 0x9c, "arm");
    put16(&mut nodes, 0x9c + 0x20, -1);
    put16(&mut nodes, 0x9c + 0x22, -1);
    put16(&mut nodes, 0x9c + 0x24, 0);
    putf(&mut nodes, 0x9c + 0x28, &[0.0, 0.0, 1.0]);
    putf(&mut nodes, 0x9c + 0x34, &[0.0, 0.0, 0.70710677, 0.70710677]);
    let nodes = img.alloc(&nodes);

    let mut inst = vec![0u8; 0x20];
    inst[2] = 1;
    putf(&mut inst, 4, &[0.25, 0.5, 0.75]);
    putf(&mut inst, 0x10, &[0.0, 0.0, 0.0, 1.0]);
    let inst = img.alloc(&inst);
    let mut marker = vec![0u8; 0x40];
    name(&mut marker, 0, "primary trigger");
    block(&mut marker, 0x34, 1, inst);
    let markers = img.alloc(&marker);

    let mut perm = vec![0u8; 0x58];
    name(&mut perm, 0, "__base");
    for i in 0..5 {
        put16(&mut perm, 0x40 + 2 * i, 0);
    }
    let perm = img.alloc(&perm);
    let mut region = vec![0u8; 0x4c];
    name(&mut region, 0, "body");
    block(&mut region, 0x40, 1, perm);
    let regions = img.alloc(&region);

    // Compressed vertices: a quad on node 0, a triangle fan skinned 0/1.
    let vertex = |p: [f32; 3], uv: [f32; 2], n0: i8, n1: i8, w: f32| {
        let mut v = vec![0u8; 0x20];
        putf(&mut v, 0, &p);
        put(&mut v, 0xc, pack_h1_normal([0.0, 0.0, 1.0]));
        put(&mut v, 0x10, pack_h1_normal([1.0, 0.0, 0.0]));
        put(&mut v, 0x14, pack_h1_normal([0.0, 1.0, 0.0]));
        put16(&mut v, 0x18, (uv[0] * 32767.0) as i16);
        put16(&mut v, 0x1a, (uv[1] * 32767.0) as i16);
        v[0x1c] = (n0 * 3) as u8;
        v[0x1d] = if n1 < 0 { (-3i8) as u8 } else { (n1 * 3) as u8 };
        put16(&mut v, 0x1e, (w * 32767.0) as i16);
        v
    };
    let quad: Vec<u8> = [
        vertex([0.0, 0.0, 0.0], [0.0, 0.0], 0, -1, 1.0),
        vertex([1.0, 0.0, 0.0], [1.0, 0.0], 0, -1, 1.0),
        vertex([0.0, 1.0, 0.0], [0.0, 1.0], 0, -1, 1.0),
        vertex([1.0, 1.0, 0.0], [1.0, 1.0], 0, -1, 1.0),
    ]
    .concat();
    let skinned: Vec<u8> = [
        vertex([0.0, 0.0, 1.0], [0.5, 0.5], 0, 1, 0.25),
        vertex([2.0, 0.0, 1.0], [0.25, 0.5], 1, -1, 1.0),
        vertex([0.0, -2.0, 1.5], [0.5, 0.125], 0, 1, 0.75),
    ]
    .concat();
    let quad_va = img.alloc(&quad);
    let skin_va = img.alloc(&skinned);
    // Cache vertex-table entries: the vertex address at +4.
    let mut e = vec![0u8; 0xc];
    put(&mut e, 4, quad_va);
    let quad_entry = img.alloc(&e);
    put(&mut e, 4, skin_va);
    let skin_entry = img.alloc(&e);
    // Strips: the quad as 0 1 2 3; the triangle with a degenerate join.
    let strip = |s: &[u16]| s.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<u8>>();
    let quad_ix = img.alloc(&strip(&[0, 1, 2, 3]));
    let skin_ix = img.alloc(&strip(&[0, 1, 2, 2]));
    let mut parts = vec![0u8; 2 * 0x68];
    for (k, (ix, n_ix, entry, nv, shader)) in [
        (quad_ix, 4u32, quad_entry, 4u32, 0i16),
        (skin_ix, 4, skin_entry, 3, 1),
    ]
    .into_iter()
    .enumerate()
    {
        let p = k * 0x68;
        put16(&mut parts, p + 4, shader);
        put16(&mut parts, p + 8, 0);
        put16(&mut parts, p + 0xa, -1);
        putf(&mut parts, p + 0xc, &[1.0, 0.0]);
        put(&mut parts, p + 0x44, 1);
        put(&mut parts, p + 0x48, n_ix - 2);
        put(&mut parts, p + 0x4c, ix);
        put(&mut parts, p + 0x54, 5);
        put(&mut parts, p + 0x58, nv);
        put(&mut parts, p + 0x64, entry);
    }
    let parts = img.alloc(&parts);
    let mut geo = vec![0u8; 0x30];
    block(&mut geo, 0x24, 2, parts);
    let geos = img.alloc(&geo);

    let mut shaders = vec![0u8; 0x40];
    shaders[0..4].copy_from_slice(&h1map::Group::new(b"soso").0.to_le_bytes());
    shaders[0x20..0x24].copy_from_slice(&h1map::Group::new(b"schi").0.to_le_bytes());
    let shaders = img.alloc(&shaders);

    let mut h = vec![0u8; h1::MODE_HEADER];
    put(&mut h, 4, 0x1234_5678);
    putf(&mut h, 0x30, &[2.0, 1.0]);
    block(&mut h, 0xac, 1, markers);
    block(&mut h, 0xb8, 2, nodes);
    block(&mut h, 0xc4, 1, regions);
    block(&mut h, 0xd0, 1, geos);
    block(&mut h, 0xdc, 2, shaders);
    let at = img.alloc(&h);
    (img, at)
}

/// An invented three-node graph: a base animation (node 1 rotates, node 2
/// translates and scales; the rest from default data) and an overlay.
fn synthetic_graph() -> (Image, u32) {
    let mut img = Image::default();
    let mut nodes = vec![0u8; 3 * 0x40];
    for (i, (n, parent, child, sib)) in [
        ("root", -1i16, 1i16, -1i16),
        ("a", 0, -1, 2),
        ("b", 0, -1, -1),
    ]
    .into_iter()
    .enumerate()
    {
        let o = i * 0x40;
        name(&mut nodes, o, n);
        put16(&mut nodes, o + 0x20, sib);
        put16(&mut nodes, o + 0x22, child);
        put16(&mut nodes, o + 0x24, parent);
    }
    let nodes = img.alloc(&nodes);
    let q = |q: [f32; 4]| {
        let l = q.iter().map(|x| x * x).sum::<f32>().sqrt();
        q.iter()
            .flat_map(|x| ((x / l * 32767.0).round() as i16).to_le_bytes())
            .collect::<Vec<u8>>()
    };
    let f = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let anim = |img: &mut Image,
                nm: &str,
                kind: u16,
                frames: u16,
                rot: u32,
                trans: u32,
                scale: u32,
                default: Vec<u8>,
                data: Vec<u8>| {
        let mut e = vec![0u8; 0xb4];
        name(&mut e, 0, nm);
        e[0x20..0x22].copy_from_slice(&kind.to_le_bytes());
        e[0x22..0x24].copy_from_slice(&frames.to_le_bytes());
        let fs = (data.len() / usize::from(frames)) as u16;
        e[0x24..0x26].copy_from_slice(&fs.to_le_bytes());
        put(&mut e, 0x28, 0xabcd);
        e[0x2c] = 3;
        put(&mut e, 0x5c, trans);
        put(&mut e, 0x6c, rot | 0xff00_0000); // junk above the node count
        put(&mut e, 0x7c, scale);
        let d = img.alloc(&default);
        put(&mut e, 0x8c, default.len() as u32);
        put(&mut e, 0x98, d);
        let fd = img.alloc(&data);
        put(&mut e, 0xa0, data.len() as u32);
        put(&mut e, 0xac, fd);
        e
    };
    // Base: rotation on node 1, translation + scale on node 2.
    let mut default = Vec::new();
    default.extend(q([0.0, 0.0, 0.0, 1.0])); // node 0 r
    default.extend(f(&[0.0, 0.0, 0.5])); // node 0 t
    default.extend(f(&[1.0])); // node 0 s
    default.extend(f(&[0.0, 1.0, 0.0])); // node 1 t
    default.extend(f(&[1.0])); // node 1 s
    default.extend(q([0.0, 0.3, 0.0, 0.9])); // node 2 r
    let mut data = Vec::new();
    for k in 0..5 {
        let t = k as f32 * 0.25;
        data.extend(q([t, 0.0, 0.0, 1.0])); // node 1 r
        data.extend(f(&[t, 2.0 * t, -t])); // node 2 t
        data.extend(f(&[1.0 + t])); // node 2 s
    }
    let base = anim(
        &mut img,
        "stand rifle move-front",
        0,
        5,
        0b010,
        0b100,
        0b100,
        default,
        data,
    );
    // Overlay: rotation on node 0 only.
    let mut default = Vec::new();
    default.extend(f(&[0.0; 3]));
    default.extend(f(&[1.0]));
    default.extend(q([0.0, 0.0, 0.0, 1.0]));
    default.extend(f(&[0.0; 3]));
    default.extend(f(&[1.0]));
    default.extend(q([0.0, 0.0, 0.0, 1.0]));
    default.extend(f(&[0.0; 3]));
    default.extend(f(&[1.0]));
    let mut data = Vec::new();
    for k in 0..3 {
        data.extend(q([0.0, 0.0, k as f32 * 0.1, 1.0]));
    }
    let overlay = anim(
        &mut img,
        "stand rifle aim-still",
        1,
        3,
        0b001,
        0,
        0,
        default,
        data,
    );
    let anims = img.alloc(&[base, overlay].concat());
    let mut h = vec![0u8; h1::ANTR_HEADER];
    block(&mut h, 0x68, 3, nodes);
    block(&mut h, 0x74, 2, anims);
    let at = img.alloc(&h);
    (img, at)
}

#[test]
fn packs_and_quantizes_round_trip() {
    for v in [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.6, 0.0, -0.8]] {
        let back = h1::unpack_normal(pack_normal(v));
        for a in 0..3 {
            assert!((back[a] - v[a]).abs() < 2.5e-3, "{v:?} -> {back:?}");
        }
    }
    let (lo, hi) = (-3.0f32, 5.0f32);
    for x in [-3.0f32, 0.0, 1.234, 5.0] {
        let s = quantize(x, lo, hi);
        let back = (f32::from(s) / 32767.0 + 1.0) / 2.0 * (hi - lo) + lo;
        assert!((back - x).abs() <= (hi - lo) / 65534.0 + 1e-6);
    }
}

#[test]
fn reads_synthetic_model() {
    let (img, at) = synthetic_model();
    let m = h1::Model::read(&img, at).unwrap();
    assert_eq!(m.nodes.len(), 2);
    assert_eq!(m.nodes[1].parent, 0);
    assert_eq!(m.markers[0].name, "primary trigger");
    assert_eq!(m.markers[0].instances[0].node, 1);
    let parts = &m.geometries[0].parts;
    assert_eq!(parts[0].vertices.len(), 4);
    assert_eq!(parts[1].vertices[0].nodes, [0, 1]);
    assert!((parts[1].vertices[0].weights[0] - 0.25).abs() < 1e-4);
    assert_eq!(parts[1].vertices[1].nodes, [1, -1]);
    assert_eq!(
        h1::strip_triangles(&parts[0].strip),
        vec![[0, 2, 1], [1, 2, 3]]
    );
    assert_eq!(h1::strip_triangles(&parts[1].strip), vec![[0, 2, 1]]);
}

#[test]
fn synthetic_model_round_trips_through_the_h2_decoder() {
    let (img, at) = synthetic_model();
    let m = h1::Model::read(&img, at).unwrap();
    let out = convert_model(&m, "thing", &ModelOptions::default()).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\thing")]);
    let d = compare_model(&m, bytes, ids[0]);
    assert_eq!(d.vertices, 7);
    assert_eq!(d.triangles, 3);
    assert!(d.max_position < 5e-5, "{d:?}");
    assert!(d.max_texcoord < 5e-5, "{d:?}");
    assert!(d.max_normal < 2.5e-3, "{d:?}");
    assert!(d.max_weight < 3e-3, "{d:?}");
}

#[test]
fn transparent_shaders_make_transparent_parts() {
    let (img, at) = synthetic_model();
    let m = h1::Model::read(&img, at).unwrap();
    let out = convert_model(&m, "thing", &ModelOptions::default()).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\thing")]);
    let mut map = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
    let g = RenderModelGeometry::read(&mut map, ids[0], &mut Raw(bytes)).unwrap();
    let kinds: Vec<u16> = g.sections[0]
        .geometry
        .parts
        .iter()
        .map(|p| p.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            crate::mode::PART_KIND_OPAQUE,
            crate::mode::PART_KIND_TRANSPARENT
        ]
    );
}

#[test]
fn reads_synthetic_graph() {
    let (img, at) = synthetic_graph();
    let g = h1::Graph::read(&img, at).unwrap();
    assert_eq!(g.nodes.len(), 3);
    let a = &g.animations[0];
    assert_eq!(a.animated, [0b010, 0b100, 0b100]);
    assert_eq!(a.frames[4][2].translation, [1.0, 2.0, -1.0]);
    assert_eq!(a.frames[4][2].scale, 2.0);
    assert_eq!(a.frames[3][1].translation, [0.0, 1.0, 0.0]);
    assert!((a.frames[0][2].rotation[1] - 0.3 / (0.9f32 * 0.9 + 0.09).sqrt()).abs() < 1e-4);
}

#[test]
fn synthetic_graph_round_trips_through_the_h2_decoder() {
    let (img, at) = synthetic_graph();
    let g = h1::Graph::read(&img, at).unwrap();
    let (out, report) = jmad::convert_graph(&g, None).unwrap();
    assert_eq!(report.names[0].1, "combat:rifle:move_front");
    assert_eq!(report.names[1].1, "combat:rifle:aim_still");
    let (bytes, ids) = build(vec![(out, "invented\\graph")]);
    for (name, e) in check_graph(&g, &[], bytes, ids[0]) {
        assert!(e[0] < 2e-4 && e[1] == 0.0 && e[2] == 0.0, "{name}: {e:?}");
    }
}

#[test]
fn compressed_animations_are_refused() {
    let (mut img, at) = synthetic_graph();
    let anims = u32::from_le_bytes(
        img.0[(at - BASE) as usize + 0x78..][..4]
            .try_into()
            .unwrap(),
    );
    let o = (anims - BASE) as usize + 0x3a;
    img.0[o] |= 1;
    assert!(matches!(
        h1::Graph::read(&img, at),
        Err(Error::Unsupported(_))
    ));
}

// ---------------------------------------------------------------------------
// Real maps
// ---------------------------------------------------------------------------

fn real_map(name: &str) -> Option<h1map::H1Map> {
    let dir = std::env::var_os("H1_MAPS")?;
    let path = std::path::Path::new(&dir).join(name);
    let f = std::fs::File::open(&path).expect("H1_MAPS map opens");
    Some(h1map::H1Map::from_reader(std::io::BufReader::new(f)).expect("map reads"))
}

const MODELS: &[&str] = &[
    "characters\\cyborg\\cyborg",
    "weapons\\assault rifle\\assault rifle",
    "vehicles\\warthog\\warthog",
];

/// `H1_MAPS=<dir> cargo test -p h1models -- --ignored --nocapture`
#[test]
#[ignore = "needs $H1_MAPS with beavercreek.map"]
fn real_models_round_trip() {
    let Some(map) = real_map("beavercreek.map") else {
        return;
    };
    for &path in MODELS {
        let tag = map
            .find(h1map::Group::new(b"mode"), path)
            .expect("model present");
        let (m, out) = crate::model_from_map(&map, tag, &ModelOptions::default()).unwrap();
        let (bytes, ids) = build(vec![(out, path)]);
        let d = compare_model(&m, bytes, ids[0]);
        println!(
            "{path}: {} nodes, {} markers, {} geometries, {} vertices, {} triangles; max error position {:.2e}, texcoord {:.2e}, normal {:.2e}, weight {:.2e}; winding disagrees {} of {}",
            m.nodes.len(),
            m.markers.iter().map(|k| k.instances.len()).sum::<usize>(),
            m.geometries.len(),
            d.vertices,
            d.triangles,
            d.max_position,
            d.max_texcoord,
            d.max_normal,
            d.max_weight,
            d.winding_disagree,
            d.triangles
        );
        assert!(d.max_position < 2e-3, "{path}: {d:?}");
        assert!(d.max_normal < 3e-3, "{path}: {d:?}");
        assert!(d.max_weight < 3e-3, "{path}: {d:?}");
        assert!(d.winding_disagree * 50 < d.triangles, "{path}: {d:?}");
    }
}

/// Every model and graph in the map reads and converts.
#[test]
#[ignore = "needs $H1_MAPS with beavercreek.map"]
fn real_every_model_and_graph_converts() {
    for file in ["beavercreek.map", "carousel.map"] {
        let Some(map) = real_map(file) else {
            return;
        };
        let (mut models, mut graphs, mut failures) = (0, 0, Vec::new());
        for t in map.tags() {
            if t.group == h1map::Group::new(b"mode") {
                models += 1;
                if let Err(e) = crate::model_from_map(&map, t, &ModelOptions::default()) {
                    failures.push(format!("{}: {e}", t.name));
                }
            } else if t.group == h1map::Group::new(b"antr") {
                graphs += 1;
                let model = map.find(h1map::Group::new(b"mode"), &t.name);
                if let Err(e) = crate::graph_from_map(&map, t, model) {
                    failures.push(format!("{}: {e}", t.name));
                }
            }
        }
        println!("{file}: {models} models, {graphs} graphs, failures {failures:#?}");
        assert!(failures.is_empty());
    }
}

const GRAPHS: &[&str] = &["characters\\cyborg\\cyborg", "vehicles\\warthog\\warthog"];

#[test]
#[ignore = "needs $H1_MAPS with beavercreek.map"]
fn real_graphs_round_trip() {
    let Some(map) = real_map("beavercreek.map") else {
        return;
    };
    for &path in GRAPHS {
        let tag = map
            .find(h1map::Group::new(b"antr"), path)
            .expect("graph present");
        let model = map.find(h1map::Group::new(b"mode"), path);
        let (g, report, out) = crate::graph_from_map(&map, tag, model).unwrap();
        let skeleton = match model {
            Some(m) => h1::AnimNode::from_model(&h1::Model::read(&map, m.data_address).unwrap()),
            None => Vec::new(),
        };
        let (bytes, ids) = build(vec![(out, path)]);
        let errors = check_graph(&g, &skeleton, bytes, ids[0]);
        println!("  variants: {:?}", report.variants);
        let worst = errors.iter().fold([0f32; 3], |w, (_, e)| {
            std::array::from_fn(|k| w[k].max(e[k]))
        });
        println!(
            "{path}: {} animations, {} frames; max error rotation {:.2e} rad, translation {:.2e}, scale {:.2e}; variants {}, dropped movement {}, dropped overlay defaults {}, zero rotations {}",
            g.animations.len(),
            g.animations
                .iter()
                .map(|a| usize::from(a.frame_count))
                .sum::<usize>(),
            worst[0],
            worst[1],
            worst[2],
            report.variants.len(),
            report.dropped_movement.len(),
            report.dropped_default_channels,
            report.zero_rotations
        );
        for (name, e) in errors.iter().take(4) {
            println!("  {name}: {e:?}");
        }
        assert!(
            worst[0] < 2e-4 && worst[1] < 1e-6 && worst[2] < 1e-6,
            "{path}: {worst:?}"
        );
    }
}
