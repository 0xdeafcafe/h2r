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
/// The `hlmt` nodes are the converted `mode`'s, one for one: the engine
/// sizes an object's node matrices by the `hlmt` count, and the model
/// writes one per its own node (a30 crashed on a converted `hlmt` with none).
#[test]
fn hlmt_nodes_match_the_render_model() {
    use map::build::FixupKind;
    let (img, at) = synthetic_model();
    let m = h1::Model::read(&img, at).unwrap();
    let out = convert_model(&m, "thing", &ModelOptions::default()).unwrap();
    let (mode_count, mode_nodes) = out
        .root
        .fixups
        .iter()
        .find_map(|f| match &f.kind {
            FixupKind::Block { count, child } if f.offset == 0x48 => Some((*count, child)),
            _ => None,
        })
        .expect("mode nodes");
    let (count, nodes) = crate::mode::hlmt_nodes(&m);
    assert_eq!(count, 2);
    assert_eq!(count, mode_count);
    let n = crate::mode::HLMT_NODE_LEN;
    assert_eq!(nodes.bytes.len(), count as usize * n);
    for i in 0..count as usize {
        let h = &nodes.bytes[i * n..(i + 1) * n];
        let mut want = mode_nodes.bytes[i * 0x60..i * 0x60 + n].to_vec();
        want[0xa..0xc].copy_from_slice(&[0, 0]);
        assert_eq!(h, &want[..], "node {i}");
    }
    let names = |b: &map::build::Block, stride: usize| {
        b.fixups
            .iter()
            .map(|f| {
                (
                    f.offset as usize / stride,
                    f.offset as usize % stride,
                    f.kind.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&nodes, n), names(mode_nodes, 0x60));
}

/// Each node's inverse rest frame is stored as retail stores it: at `+0x2c`,
/// `+0x38`, `+0x44` the inverse's columns (the rows of the model-space
/// rotation R), at `+0x50` its position, so that the rest frame times it is
/// the identity (`skin_tests::retail_h2_bipeds_pose_sanely` holds every
/// retail biped node to the same reading). Writing R's columns there
/// instead skins every vertex of a rotated node by R twice, which the
/// engine drew as twisted shards.
#[test]
fn node_inverse_frames_are_stored_as_columns() {
    let (img, at) = synthetic_model();
    let m = h1::Model::read(&img, at).unwrap();
    let out = convert_model(&m, "thing", &ModelOptions::default()).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\thing")]);
    let mut map = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
    let g = RenderModelGeometry::read(&mut map, ids[0], &mut Raw(bytes)).unwrap();
    let tag = map.read_tag_bytes(ids[0]).unwrap();
    let base = map.tag(ids[0]).unwrap().location.unwrap().address;
    let at = (u32::from_le_bytes(tag[0x4c..0x50].try_into().unwrap()) - base) as usize;
    let world = crate::mode::world_transforms(
        &g.nodes
            .iter()
            .map(|n| (n.wire.parent_index, n.wire.orientation, n.wire.translation))
            .collect::<Vec<_>>(),
    );
    let mut rotated = 0;
    for (i, (r, p)) in world.iter().enumerate() {
        let e = &tag[at + i * 0x60..];
        // inv[row][col] = vector col, component row.
        let inv: [[f32; 3]; 3] = std::array::from_fn(|row| std::array::from_fn(|col| h1::f32_at(e, 0x2c + 12 * col + 4 * row)));
        let ip: [f32; 3] = std::array::from_fn(|k| h1::f32_at(e, 0x50 + 4 * k));
        for row in 0..3 {
            for col in 0..3 {
                let v: f32 = (0..3).map(|k| r[row][k] * inv[k][col]).sum();
                assert!((v - f32::from(u8::from(row == col))).abs() < 1e-6, "node {i}: R inv [{row}][{col}] {v}");
            }
            let t: f32 = (0..3).map(|k| r[row][k] * ip[k]).sum::<f32>() + p[row];
            assert!(t.abs() < 1e-6, "node {i}: position {t}");
        }
        rotated += usize::from((0..3).any(|a| (0..3).any(|b| a != b && (r[a][b] - r[b][a]).abs() > 0.5)));
    }
    // The synthetic arm turns 90 degrees, so the transpose would fail.
    assert_eq!(rotated, 1);
}

/// Every converted model has the one section group retail models have:
/// the engine indexes `mode +0x34` by the detail level's byte at `+0x3c`
/// with no count check (a30's second fault, on an empty block).
#[test]
fn converted_models_have_one_section_group() {
    use map::build::FixupKind;
    let (img, at) = synthetic_model();
    let m = h1::Model::read(&img, at).unwrap();
    let out = convert_model(&m, "thing", &ModelOptions::default()).unwrap();
    let (count, child) = out
        .root
        .fixups
        .iter()
        .find_map(|f| match &f.kind {
            FixupKind::Block { count, child }
                if f.offset as usize == crate::mode::SECTION_GROUPS =>
            {
                Some((*count, child))
            }
            _ => None,
        })
        .expect("section groups");
    assert_eq!(count, 1);
    assert_eq!(child.bytes, [0x3f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(child.fixups.is_empty());
    assert_eq!(&out.root.bytes[0x3c..0x42], [0; 6]);
}

pub(crate) fn build(tags: Vec<(Converted, &str)>) -> (Vec<u8>, Vec<TagId>) {
    let mut set = TagSet::new(CacheType::Multiplayer, "h1test", "invented\\h1test");
    let ids = tags
        .into_iter()
        .map(|(c, name)| c.install(&mut set, name).expect("install"))
        .collect();
    let (bytes, _) = set.build().expect("map builds");
    (bytes, ids)
}

pub(crate) struct Raw(pub(crate) Vec<u8>);
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
    // Halo 1's markers, then the engine's name for `primary trigger`
    // (`mode::marker_aliases`) with the same instances.
    assert_eq!(count, src.markers.len() + 1);
    let alias = at + src.markers.len() * 0xc;
    assert_eq!(resolve(tags::StringId(u32_at(alias))), "primary_trigger");
    let n = u32_at(at + 4) as usize;
    assert_eq!(u32_at(alias + 4) as usize, n);
    let insts = |g: usize| {
        let a = (u32_at(g + 8) - base) as usize;
        tag[a..a + n * 0x24].to_vec()
    };
    assert_eq!(insts(alias), insts(at));
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
    // Every animation reads as the engine reads it.
    for (i, a) in src.animations.iter().enumerate() {
        let record = &g.animations[i];
        assert_eq!(u16::from(record.frame_info_type), a.frame_info_type, "{}", a.name);
        let sizes = record.sizes;
        let len = u16::from_le_bytes(sizes.unknown) as usize;
        let at = record.data_offset as usize
            + usize::from(sizes.static_size)
            + sizes.animated_size as usize
            + usize::from(sizes.static_flags)
            + usize::from(sizes.animated_flags);
        assert_eq!(&resource[at..at + len], a.frame_info.as_slice(), "{}", a.name);
        let p = g.engine_problems(i, &resource);
        assert!(p.is_empty(), "{}: {p:?}", a.name);
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

/// A Halo 1 strip behind one repeat of its first index, read with Halo 2's
/// parity, is the triangles Halo 1's parity reads, in order and wound
/// alike: across degenerate joins, on a strip of either length parity, and
/// on the strips `triangles_strip` rebuilds after edge-on triangles drop.
#[test]
fn a_halo2_strip_draws_the_halo1_strips_triangles() {
    for strip in [
        vec![0u16, 1, 2, 3],
        vec![0, 1, 2, 3, 4],
        vec![0, 1, 2, 3, 3, 4, 4, 5, 6, 7],
        vec![5, 6, 7, 7, 8, 8, 9, 10],
    ] {
        let want = h1::strip_triangles(&strip);
        assert!(!want.is_empty());
        assert_eq!(
            tags::geometry::strip_triangles(&h1::h2_strip(&strip)),
            want,
            "{strip:?}"
        );
        // Halo 2's own parity on the bare strip winds every one the other
        // way: why the repeat is there.
        let bare = tags::geometry::strip_triangles(&strip);
        assert!(bare.iter().zip(&want).all(|(a, b)| a != b), "{strip:?}");
        let rebuilt = h1::triangles_strip(&want);
        assert_eq!(
            tags::geometry::strip_triangles(&h1::h2_strip(&rebuilt)),
            want
        );
    }
    assert!(h1::h2_strip(&[0, 1]).is_empty());
    assert!(h1::h2_strip(&[]).is_empty());
}

/// A sky's model (`ModelOptions::strips`; the sky draw `0x0004c974` draws
/// every part as a strip whatever its flags) carries no list flag on any
/// part, keeps retail's empty placeholder subparts, and its strips are
/// Halo 1's triangles, wound alike (`compare_model`). The list form read as
/// a strip, as the sky draw reads it, is not.
#[test]
fn a_sky_model_draws_its_parts_as_strips() {
    let (img, at) = synthetic_model();
    let m = h1::Model::read(&img, at).unwrap();
    let opts = ModelOptions {
        strips: true,
        ..ModelOptions::default()
    };
    let out = convert_model(&m, "thing", &opts).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\thing")]);
    let d = compare_model(&m, bytes.clone(), ids[0]);
    assert_eq!(d.vertices, 7);
    assert_eq!(d.triangles, 3);
    assert!(d.max_position < 5e-5, "{d:?}");
    let mut map = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
    let g = RenderModelGeometry::read(&mut map, ids[0], &mut Raw(bytes)).unwrap();
    let mut parts = 0;
    for s in &g.sections {
        for p in &s.geometry.parts {
            assert!(!p.is_triangle_list(), "a strip part");
            parts += 1;
        }
        for sp in &s.geometry.subparts {
            assert_eq!((sp.first_index, sp.index_count), (0, 0), "a placeholder");
        }
    }
    assert_eq!(parts, 2);

    // The list form, drawn as the sky draws it.
    let list = convert_model(&m, "thing", &ModelOptions::default()).unwrap();
    let (bytes, ids) = build(vec![(list, "invented\\thing")]);
    let mut map = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
    let g = RenderModelGeometry::read(&mut map, ids[0], &mut Raw(bytes)).unwrap();
    let s = &g.sections[0];
    let p = &s.geometry.parts[0];
    assert!(p.is_triangle_list());
    assert_ne!(
        tags::geometry::strip_triangles(s.geometry.part_indices(p)),
        s.geometry.part_triangles(p)
    );
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
    // Stored j = 0.3; read back in Halo 2's convention, the conjugate.
    assert!((a.frames[0][2].rotation[1] + 0.3 / (0.9f32 * 0.9 + 0.09).sqrt()).abs() < 1e-4);
}

#[test]
fn movement_records_follow_the_two_channel_masks_for_each_encoding() {
    for (kind, axes) in [(1, 2), (2, 3), (3, 4)] {
        let (mut img, graph) = synthetic_graph();
        let animations = h1::u32_at(&img.0, (graph - BASE) as usize + 0x78);
        let entry = (animations - BASE) as usize;
        let movement: Vec<u8> = (0..5 * axes)
            .flat_map(|n| ((n as f32 - 3.0) / 8.0).to_le_bytes())
            .collect();
        let addr = img.alloc(&movement);
        put16(&mut img.0, entry + 0x26, kind);
        put(&mut img.0, entry + 0x48, movement.len() as u32);
        put(&mut img.0, entry + 0x54, addr);
        let g = h1::Graph::read(&img, graph).unwrap();
        assert_eq!(g.animations[0].frame_info, movement);
        let (out, report) = jmad::convert_graph(&g, None).unwrap();
        assert_eq!(report.movement, [g.animations[0].name.clone()]);
        let (bytes, ids) = build(vec![(out, "invented\\graph")]);
        assert_eq!(check_graph(&g, &[], bytes, ids[0]).len(), g.animations.len());

        put(&mut img.0, entry + 0x48, movement.len() as u32 - 1);
        assert!(h1::Graph::read(&img, graph).is_err());
    }
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

/// Every level of a converted graph's tree is laid out in ascending
/// string-ID order, as the engine's binary search over it needs (E
/// `0x001dd560`, from `0x001db170`). Here name order and string-ID order
/// disagree at two levels: class `pistol` sorts before `rifle` by name, but
/// `rifle` (5 bytes) has the smaller string ID; action `aim_still` before
/// `idle` by name, `idle` (4 bytes) by ID.
#[test]
fn a_converted_graph_tree_is_in_string_id_order() {
    let (img, at) = synthetic_graph();
    let mut g = h1::Graph::read(&img, at).unwrap();
    g.animations[0].name = "stand pistol idle".into();
    g.animations[1].name = "stand pistol aim-still".into();
    let mut third = g.animations[0].clone();
    third.name = "stand rifle idle".into();
    g.animations.push(third);
    let (out, _) = jmad::convert_graph(&g, None).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\graph")]);
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let base = map.tag(ids[0]).unwrap().location.unwrap().address;
    let b = map.read_tag_bytes(ids[0]).unwrap();
    let strings = map.strings().to_vec();
    let w = |a: u32| u32::from_le_bytes(b[(a - base) as usize..][..4].try_into().unwrap());
    let name = |sid: u32| strings[(sid & 0xff_ffff) as usize].clone().unwrap();
    // (count, address) at `a`; the names of its elements of `stride`.
    let level = |a: u32, stride: u32| -> Vec<(u32, u32)> {
        (0..w(a)).map(|i| (w(w(a + 4) + i * stride), w(a + 4) + i * stride)).collect()
    };
    let ascending = |l: &[(u32, u32)]| l.windows(2).all(|p| (p[0].0 as i32) < (p[1].0 as i32));
    let modes = level(base + 0x34, 0x14);
    assert!(ascending(&modes));
    let classes = level(modes[0].1 + 4, 0x14);
    assert!(ascending(&classes));
    assert_eq!(
        classes.iter().map(|c| name(c.0)).collect::<Vec<_>>(),
        ["rifle", "pistol"]
    );
    let pistol = classes[1].1;
    let types = level(pistol + 4, 0x34);
    let actions = level(types[0].1 + 4, 8);
    assert!(ascending(&actions));
    assert_eq!(
        actions.iter().map(|a| name(a.0)).collect::<Vec<_>>(),
        ["idle", "aim_still"]
    );
}

/// Halo 1 finds a unit's animations through slot tables, and a class's slot
/// may name another class's animation: the cyborg's pistol class throws with
/// `stand rifle throw-grenade`. The converted tree names that animation under
/// the pistol too, so Halo 2's search for `combat:pistol:any:throw_grenade`
/// (`0x000e8220`) finds it; the rifle keeps its own, and nothing is added
/// for a slot that names its own class's animation.
#[test]
fn a_class_that_borrows_a_slot_finds_it_by_name() {
    let (img, at) = synthetic_graph();
    let mut g = h1::Graph::read(&img, at).unwrap();
    g.animations[0].name = "stand rifle throw-grenade".into();
    g.animations[1].name = "stand pistol idle".into();
    g.units = vec![h1::UnitSlots {
        label: "stand".into(),
        classes: vec![
            h1::ClassSlots {
                label: "rifle".into(),
                slots: vec![-1, 0],
                ..Default::default()
            },
            h1::ClassSlots {
                label: "pistol".into(),
                slots: vec![1, 0],
                ..Default::default()
            },
        ],
        ..Default::default()
    }];
    let (out, _) = jmad::convert_graph(&g, None).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\graph")]);
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let base = map.tag(ids[0]).unwrap().location.unwrap().address;
    let b = map.read_tag_bytes(ids[0]).unwrap();
    let strings = map.strings().to_vec();
    let w = |a: u32| u32::from_le_bytes(b[(a - base) as usize..][..4].try_into().unwrap());
    let name = |sid: u32| strings[(sid & 0xff_ffff) as usize].clone().unwrap();
    let level = |a: u32, stride: u32| -> Vec<(String, u32)> {
        (0..w(a)).map(|i| (name(w(w(a + 4) + i * stride)), w(a + 4) + i * stride)).collect()
    };
    let modes = level(base + 0x34, 0x14);
    assert_eq!(modes.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), ["combat"]);
    let actions = |class: &str| -> Vec<(String, i16)> {
        let c = level(modes[0].1 + 4, 0x14).into_iter().find(|c| c.0 == class).unwrap();
        let t = level(c.1 + 4, 0x34);
        assert_eq!(t.len(), 1);
        level(t[0].1 + 4, 8)
            .into_iter()
            .map(|(n, a)| (n, (w(a + 4) >> 16) as i16))
            .collect()
    };
    let mut pistol = actions("pistol");
    pistol.sort();
    assert_eq!(pistol, [("idle".to_owned(), 1), ("throw_grenade".to_owned(), 0)]);
    assert_eq!(actions("rifle"), [("throw_grenade".to_owned(), 0)]);
}

/// Halo 1 plays a weapon's first-person animations by slot (`antr +0x48`),
/// and a slot may hold an animation labelled for another: the assault
/// rifle's `reload-empty` slot holds its `reload-full`. The reader takes
/// the table, and the converted tree names each slot's action
/// (`reload_empty`) for the animation it holds, beside the labels' own;
/// a label's own action is not replaced, and a slot past the animations
/// is skipped.
#[test]
fn a_first_person_slot_names_its_action_for_the_animation_it_holds() {
    let (mut img, at) = synthetic_graph();
    // Slots: idle 0, posing -1, fire-1 -1, ..., reload-empty (7) 1,
    // reload-full (8) 1, ..., ready (10) 0, put-away (11) 9.
    let mut slots = vec![-1i16; 12];
    slots[0] = 0;
    slots[7] = 1;
    slots[8] = 1;
    slots[10] = 0;
    slots[11] = 9;
    let list: Vec<u8> = slots.iter().flat_map(|s| s.to_le_bytes()).collect();
    let list = img.alloc(&list);
    let mut element = vec![0u8; 0x1c];
    block(&mut element, 0x10, slots.len(), list);
    let element = img.alloc(&element);
    let h = (at - BASE) as usize;
    block(&mut img.0[h..h + h1::ANTR_HEADER], 0x48, 1, element);
    let mut g = h1::Graph::read(&img, at).unwrap();
    assert_eq!(g.first_person_slots, slots);
    g.animations[0].name = "first-person idle".into();
    g.animations[1].name = "first-person reload-full".into();
    // A base animation, as Halo 1's reloads are (an overlay would go to
    // the type's overlays).
    g.animations[1].kind = h1::AnimType::Base;
    let (out, _) = jmad::convert_graph(&g, None).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\graph")]);
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let base = map.tag(ids[0]).unwrap().location.unwrap().address;
    let b = map.read_tag_bytes(ids[0]).unwrap();
    let strings = map.strings().to_vec();
    let w = |a: u32| u32::from_le_bytes(b[(a - base) as usize..][..4].try_into().unwrap());
    let name = |sid: u32| strings[(sid & 0xff_ffff) as usize].clone().unwrap();
    let level = |a: u32, stride: u32| -> Vec<(String, u32)> {
        (0..w(a)).map(|i| (name(w(w(a + 4) + i * stride)), w(a + 4) + i * stride)).collect()
    };
    let modes = level(base + 0x34, 0x14);
    assert_eq!(modes.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), ["any"]);
    let classes = level(modes[0].1 + 4, 0x14);
    let types = level(classes[0].1 + 4, 0x34);
    let mut actions: Vec<(String, i16)> = level(types[0].1 + 4, 8)
        .into_iter()
        .map(|(n, a)| (n, (w(a + 4) >> 16) as i16))
        .collect();
    actions.sort();
    assert_eq!(
        actions,
        [
            ("idle".to_owned(), 0),
            ("ready".to_owned(), 0),
            ("reload_empty".to_owned(), 1),
            ("reload_full".to_owned(), 1),
        ]
    );
}

/// The converted first-person graph's one type (`any`, `any`, `any`): its
/// actions (`+0x4`) and its overlays (`+0xc`), `(name, animation index)`
/// each, sorted.
fn first_person_type(g: &h1::Graph) -> (Vec<(String, i16)>, Vec<(String, i16)>) {
    let (out, _) = jmad::convert_graph(g, None).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\graph")]);
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let base = map.tag(ids[0]).unwrap().location.unwrap().address;
    let b = map.read_tag_bytes(ids[0]).unwrap();
    let strings = map.strings().to_vec();
    let w = |a: u32| u32::from_le_bytes(b[(a - base) as usize..][..4].try_into().unwrap());
    let name = |sid: u32| strings[(sid & 0xff_ffff) as usize].clone().unwrap();
    let level = |a: u32, stride: u32| -> Vec<(String, u32)> {
        (0..w(a)).map(|i| (name(w(w(a + 4) + i * stride)), w(a + 4) + i * stride)).collect()
    };
    let modes = level(base + 0x34, 0x14);
    assert_eq!(modes.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), ["any"]);
    let classes = level(modes[0].1 + 4, 0x14);
    let types = level(classes[0].1 + 4, 0x34);
    assert_eq!(types.len(), 1);
    let entries = |at: u32| {
        let mut e: Vec<(String, i16)> = level(at, 8)
            .into_iter()
            .map(|(n, a)| {
                assert_eq!(w(a + 4) as i16, -1, "{n}: this graph");
                (n, (w(a + 4) >> 16) as i16)
            })
            .collect();
        e.sort();
        e
    };
    (entries(types[0].1 + 4), entries(types[0].1 + 0xc))
}

/// The assault rifle's `fire-1` slot is empty and its firing animation is
/// labelled `firing`: the graph gets `fire_1` naming it, beside `firing`.
/// Its overlay (`moving`) goes to the type's overlays, not its actions:
/// the engine finds a first-person overlay there (`0x001dbc80`).
#[test]
fn a_first_person_graph_fires_its_firing_animation_and_keeps_overlays_apart() {
    let (img, at) = synthetic_graph();
    let mut g = h1::Graph::read(&img, at).unwrap();
    assert_eq!(g.animations[1].kind, h1::AnimType::Overlay);
    g.animations[0].name = "first-person firing".into();
    g.animations[1].name = "first-person moving".into();
    g.first_person_slots = vec![-1, -1, -1, 1];
    let (actions, overlays) = first_person_type(&g);
    assert_eq!(actions, [("fire_1".to_owned(), 0), ("firing".to_owned(), 0)]);
    assert_eq!(overlays, [("moving".to_owned(), 1)]);
}

/// A filled `fire-1` slot keeps its animation: `firing` does not take
/// `fire_1` from it.
#[test]
fn a_filled_fire_slot_keeps_its_animation() {
    let (img, at) = synthetic_graph();
    let mut g = h1::Graph::read(&img, at).unwrap();
    g.animations[0].name = "first-person fire-1".into();
    g.animations[1].name = "first-person firing".into();
    g.animations[1].kind = h1::AnimType::Base;
    g.first_person_slots = vec![-1, -1, 0];
    let (actions, overlays) = first_person_type(&g);
    assert_eq!(actions, [("fire_1".to_owned(), 0), ("firing".to_owned(), 1)]);
    assert!(overlays.is_empty());
}

/// Third-person graphs keep their overlays among the actions, as before:
/// only first-person overlays are moved.
#[test]
fn a_third_person_graph_keeps_its_overlays_as_actions() {
    let (img, at) = synthetic_graph();
    let g = h1::Graph::read(&img, at).unwrap();
    let (out, _) = jmad::convert_graph(&g, None).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\graph")]);
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let base = map.tag(ids[0]).unwrap().location.unwrap().address;
    let b = map.read_tag_bytes(ids[0]).unwrap();
    let w = |a: u32| u32::from_le_bytes(b[(a - base) as usize..][..4].try_into().unwrap());
    let el = |a: u32, i: u32, stride: u32| w(a + 4) + i * stride;
    let mode = el(base + 0x34, 0, 0x14);
    let class = el(mode + 4, 0, 0x14);
    let ty = el(class + 4, 0, 0x34);
    assert!(w(ty + 4) >= 2, "move_front and aim_still among the actions");
    assert_eq!(w(ty + 0xc), 0, "no overlays block");
}

/// No converted animation names a blend screen: the graph has none, and
/// the engine indexes `blend_screens` (`+0x24`) by an animation's `+0x12`
/// unless it is -1 (E `0x001daf30`). Written 0, the player's aim update
/// (`0x00112a51`) read the empty block's element 0 and faulted at `0xc`
/// on every converted map once the sorted trees let a graph find its
/// animations.
#[test]
fn a_converted_animation_names_no_blend_screen() {
    let (img, at) = synthetic_graph();
    let g = h1::Graph::read(&img, at).unwrap();
    let (out, _) = jmad::convert_graph(&g, None).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\graph")]);
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let base = map.tag(ids[0]).unwrap().location.unwrap().address;
    let b = map.read_tag_bytes(ids[0]).unwrap();
    let w = |a: u32| u32::from_le_bytes(b[(a - base) as usize..][..4].try_into().unwrap());
    // The graph holds no blend screens.
    assert_eq!(w(base + 0x24), 0);
    let (count, at) = (w(base + 0x2c), w(base + 0x30));
    assert!(count > 0);
    for i in 0..count {
        let e = (at + i * 0x6c - base) as usize;
        assert_eq!(b[e + 0x12] as i8, -1, "animation {i}");
    }
}

/// The synthetic graph's base animation, re-encoded as compressed data with
/// a key on every frame, reads back as the uncompressed original (within
/// the 12-bit rotation quantisation) and converts.
#[test]
fn compressed_animations_decode_like_their_uncompressed_form() {
    let (mut img, at) = synthetic_graph();
    let plain = h1::Graph::read(&img, at).unwrap();
    let src = &plain.animations[0];
    let n = src.frames[0].len();
    let f0 = &src.frames[0];
    // The reader returns Halo 2's convention; the stored form is Halo 1's
    // (the conjugate, h1::h2_rotation, its own inverse).
    let keys = |node: usize| -> Vec<(u16, [f32; 4])> {
        (1..src.frames.len())
            .map(|f| (f as u16, h1::h2_rotation(src.frames[f][node].rotation)))
            .collect()
    };
    let blob = crate::compressed::encode(
        &f0.iter().map(|t| h1::h2_rotation(t.rotation)).collect::<Vec<_>>(),
        &f0.iter().map(|t| t.translation).collect::<Vec<_>>(),
        &[keys(1)],
        &[(1..5)
            .map(|f| (f as u16, src.frames[f][2].translation))
            .collect()],
        &[(
            f0[2].scale,
            (1..5).map(|f| (f as u16, src.frames[f][2].scale)).collect(),
        )],
    );
    let data = img.alloc(&blob);
    let anims = u32::from_le_bytes(
        img.0[(at - BASE) as usize + 0x78..][..4]
            .try_into()
            .unwrap(),
    );
    let e = (anims - BASE) as usize;
    img.0[e + 0x3a] |= 1;
    put(&mut img.0, e + 0x8c, 0); // no default data
    put(&mut img.0, e + 0xa0, blob.len() as u32);
    put(&mut img.0, e + 0xac, data);
    let g = h1::Graph::read(&img, at).unwrap();
    let a = &g.animations[0];
    let stats = a.compressed.expect("decoded as compressed");
    assert_eq!(stats.keys, 12);
    assert_eq!(a.frames.len(), src.frames.len());
    for (f, (got, want)) in a.frames.iter().zip(&src.frames).enumerate() {
        for node in 0..n {
            let (g, w) = (got[node], want[node]);
            let dot: f32 = (0..4).map(|i| g.rotation[i] * w.rotation[i]).sum();
            assert!(
                1.0 - dot.abs() < 1e-5,
                "frame {f} node {node}: {g:?} vs {w:?}"
            );
            assert_eq!(g.translation, w.translation, "frame {f} node {node}");
            assert_eq!(g.scale, w.scale, "frame {f} node {node}");
        }
    }
    let (out, _) = jmad::convert_graph(&g, None).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\graph")]);
    for (name, e) in check_graph(&g, &[], bytes, ids[0]) {
        assert!(e[0] < 2e-4 && e[1] == 0.0 && e[2] == 0.0, "{name}: {e:?}");
    }
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
            "{path}: {} animations, {} frames; max error rotation {:.2e} rad, translation {:.2e}, scale {:.2e}; variants {}, root movement {}, dropped overlay defaults {}, zero rotations {}",
            g.animations.len(),
            g.animations
                .iter()
                .map(|a| usize::from(a.frame_count))
                .sum::<usize>(),
            worst[0],
            worst[1],
            worst[2],
            report.variants.len(),
            report.movement.len(),
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

/// Checks the two Halo 2 `mode` layouts the converter writes but
/// `crates/tags` does not decode, against every retail model of one map:
/// marker groups at header `+0x58` (stride `0xc`; markers `0x24`) and each
/// node's inverse model-space transform at `+0x28`.
///
/// `H2_DISC=disc/halo2.iso cargo test -p h1models --release -- --ignored retail_h2 --nocapture`
#[test]
#[ignore = "needs $H2_DISC"]
fn retail_h2_mode_layouts() {
    use std::io::BufReader;
    let Some(disc) = std::env::var_os("H2_DISC") else {
        return;
    };
    let stem = std::env::var("H2_MAP").unwrap_or_else(|_| "lockout".into());
    let mut iso = xiso::Xiso::open(BufReader::new(std::fs::File::open(&disc).unwrap())).unwrap();
    let entry = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| !e.is_dir && e.path.ends_with(&format!("{stem}.map")))
        .unwrap();
    let base = iso.partition_offset() + u64::from(entry.sector) * xiso::SECTOR;
    let mut map = Map::from_region(
        BufReader::new(std::fs::File::open(&disc).unwrap()),
        base,
        entry.size.into(),
    )
    .unwrap();
    let ids: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group.0 == *b"mode" && t.location.is_some())
        .map(|t| t.id)
        .collect();
    let (mut models, mut groups, mut markers, mut bad_markers) = (0, 0, 0, 0);
    // Failing markers by reason: unnamed group, node, rotation, +0x20 not finite.
    let mut why = [0usize; 4];
    let mut nonzero_20 = 0;
    let (mut nodes, mut inverse_ok, mut transposed_ok, mut scale_one) = (0, 0, 0, 0);
    for id in ids {
        let tag = map.read_tag_bytes(id).unwrap();
        let addr = map.tag(id).unwrap().location.unwrap().address;
        let strings = map.strings().to_vec();
        let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let f32_at = |b: &[u8], o: usize| f32::from_bits(u32_at(b, o));
        let elems = |count: u32, at: u32, stride: usize| -> Vec<&[u8]> {
            if count == 0 {
                return Vec::new();
            }
            let s = (at - addr) as usize;
            tag[s..s + count as usize * stride].chunks(stride).collect()
        };
        models += 1;
        let node_list = elems(u32_at(&tag, 0x48), u32_at(&tag, 0x4c), 0x60);
        for g in elems(u32_at(&tag, 0x58), u32_at(&tag, 0x5c), 0xc) {
            groups += 1;
            let named = tags::StringId(u32_at(g, 0))
                .resolve(&strings)
                .ok()
                .flatten()
                .is_some_and(|s| !s.is_empty());
            for m in elems(u32_at(g, 4), u32_at(g, 8), 0x24) {
                markers += 1;
                let q: f32 = (0..4).map(|k| f32_at(m, 0x10 + 4 * k).powi(2)).sum();
                let reasons = [
                    !named,
                    usize::from(m[2]) >= node_list.len(),
                    (q - 1.0).abs() >= 1e-2,
                    !f32_at(m, 0x20).is_finite(),
                ];
                for (k, r) in reasons.iter().enumerate() {
                    why[k] += usize::from(*r);
                }
                let ok = !reasons.iter().any(|&r| r);
                nonzero_20 += usize::from(f32_at(m, 0x20) != 0.0);
                bad_markers += usize::from(!ok);
            }
        }
        // Inverse transform: composing the default pose's model-space
        // transform with +0x28.. must give identity.
        let pose: Vec<(i16, [f32; 4], [f32; 3])> = node_list
            .iter()
            .map(|n| {
                (
                    i16::from_le_bytes([n[4], n[5]]),
                    std::array::from_fn(|k| f32_at(n, 0x18 + 4 * k)),
                    std::array::from_fn(|k| f32_at(n, 0xc + 4 * k)),
                )
            })
            .collect();
        let world = crate::mode::world_transforms(&pose);
        for (n, (m, p)) in node_list.iter().zip(&world) {
            nodes += 1;
            let inv: [[f32; 3]; 3] =
                std::array::from_fn(|r| std::array::from_fn(|c| f32_at(n, 0x2c + 12 * r + 4 * c)));
            let ip: [f32; 3] = std::array::from_fn(|k| f32_at(n, 0x50 + 4 * k));
            // Either the stored vectors are the columns of R^T (ours) or its rows.
            let check = |get: &dyn Fn(usize, usize) -> f32| {
                (0..3).all(|r| {
                    (0..3).all(|c| {
                        let v: f32 = (0..3).map(|k| get(r, k) * m[k][c]).sum();
                        (v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-2
                    }) && {
                        let v: f32 = (0..3).map(|k| get(r, k) * p[k]).sum::<f32>() + ip[r];
                        v.abs() < 1e-2
                    }
                })
            };
            let ok = check(&|r, k| inv[k][r]);
            transposed_ok += usize::from(check(&|r, k| inv[r][k]));
            scale_one += usize::from(f32_at(n, 0x28) == 1.0);
            inverse_ok += usize::from(ok);
        }
    }
    println!(
        "{stem}: {models} models, {groups} marker groups, {markers} markers ({bad_markers} failing: {why:?}; {nonzero_20} with +0x20 nonzero), {nodes} nodes ({inverse_ok} match our inverse layout, {transposed_ok} its transpose, {scale_one} with scale 1)"
    );
    assert!(markers > 0 && bad_markers == 0);
    assert!(inverse_ok * 100 >= nodes * 99, "inverse layout");
}

#[test]
fn install_at_takes_an_existing_slot_and_keeps_its_datum() {
    use map::FourCC;
    use map::build::{Block, TagPayload};
    let mut set = TagSet::new(CacheType::Multiplayer, "h1test", "invented\\h1test");
    set.add_group(FourCC(*b"shad"), None, None);
    let old = set
        .add_tag(
            FourCC(*b"shad"),
            "invented\\old",
            TagPayload::Built(Block::zeroed(8)),
        )
        .unwrap();
    let slots = set.slots.len();
    let mut root = Block::zeroed(0x10);
    root.put_u32(0, 7).tag_id(4, crate::SELF_ID);
    let c = Converted {
        group: FourCC(*b"bitm"),
        root,
        raw: vec![(vec![1, 2, 3], 0x80)],
    };
    assert_eq!(
        c.clone()
            .install_at(&mut set, old, "invented\\new")
            .unwrap(),
        old
    );
    assert_eq!(set.slots.len(), slots, "no new slot");
    assert_eq!(
        set.slots[old.index()].name.as_deref(),
        Some("invented\\new")
    );
    let tag = set.tag(old).unwrap();
    assert_eq!(tag.group, FourCC(*b"bitm"));
    let TagPayload::Built(b) = &tag.payload else {
        panic!("built")
    };
    assert_eq!(b.bytes[0], 7);
    assert!(matches!(
        c.install_at(&mut set, TagId(0x1234_0099), "x"),
        Err(Error::Invalid(_))
    ));
}

// ---------------------------------------------------------------------------
// The Halo 1 disc: compressed animations
// ---------------------------------------------------------------------------

/// Every map on the Halo 1 disc (`$H1_ISO`), in disc order.
fn h1_disc_maps() -> Option<Vec<(String, h1map::H1Map)>> {
    let path = std::env::var_os("H1_ISO")?;
    let file = std::io::BufReader::new(std::fs::File::open(path).expect("H1_ISO opens"));
    let mut iso = xiso::Xiso::open(file).expect("disc reads");
    let disc = h1map::disc::Disc::open(&mut iso).expect("a Halo 1 disc");
    Some(
        disc.maps
            .iter()
            .map(|e| {
                let bytes = iso.read(e).expect("map reads");
                let map = h1map::H1Map::from_reader(Cursor::new(bytes)).expect("map parses");
                (h1map::disc::map_stem(e).to_string(), map)
            })
            .collect(),
    )
}

/// Rotation angle between two unit quaternions, in radians.
fn angle(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot: f32 = (0..4).map(|i| a[i] * b[i]).sum();
    2.0 * dot.abs().min(1.0).acos()
}

/// Largest rotation (radians) and translation differences between two
/// animations' frames, over the channels `a` animates.
fn frame_error(a: &h1::Animation, b: &h1::Animation) -> (f32, f32) {
    assert_eq!(a.frames.len(), b.frames.len(), "{}", a.name);
    let (mut r, mut t) = (0f32, 0f32);
    for (fa, fb) in a.frames.iter().zip(&b.frames) {
        for (n, (x, y)) in fa.iter().zip(fb).enumerate() {
            if a.has(0, n) {
                r = r.max(angle(x.rotation, y.rotation));
            }
            if a.has(1, n) {
                for i in 0..3 {
                    t = t.max((x.translation[i] - y.translation[i]).abs());
                }
            }
        }
    }
    (r, t)
}

/// Every animation graph on every map of the Halo 1 disc reads and
/// converts; every compressed animation decodes to `frame count` frames of
/// unit rotations and finite translations and scales, round-trips through
/// the Halo 2 decoder, and agrees with the uncompressed copies and model
/// poses that exist for it.
///
/// `H1_ISO=disc/halo1.iso cargo test -p h1models --release -- --ignored h1_disc --nocapture`
#[test]
#[ignore = "needs $H1_ISO"]
fn h1_disc_compressed_animations_decode() {
    let Some(maps) = h1_disc_maps() else {
        return;
    };
    assert_eq!(maps.len(), 24, "maps on the disc");
    let antr = h1map::Group::new(b"antr");
    let mode = h1map::Group::new(b"mode");
    let (mut graphs, mut compressed, mut frames, mut keys) = (0, 0, 0usize, 0usize);
    let mut failures = Vec::new();
    let mut no_skeleton = Vec::new();
    // Undriven rotation channels, and how many sit at the rest pose of the
    // model with the same node list.
    let (mut pose_channels, mut pose_matching) = (0usize, 0usize);
    let mut worst_roundtrip = 0f32;
    // (map, graph, animation) of each copy, compressed then uncompressed.
    let pairs = [
        (
            (
                "a50",
                "cinematics\\animations\\chief\\level_specific\\a50\\a50",
            ),
            (
                "a30",
                "cinematics\\animations\\chief\\level_specific\\b30\\b30",
            ),
            "B30HoloMapShort",
        ),
        (
            ("a10", "cinematics\\animations\\chief\\x30\\x30"),
            ("a10", "characters\\cyborg\\cyborg"),
            "stand rifle idle",
        ),
    ];
    let mut copies: Vec<[Option<h1::Animation>; 2]> = vec![[None, None]; pairs.len()];
    for (stem, map) in &maps {
        // Models by node list checksum, for the base pose check.
        let models: Vec<h1::Model> = map
            .tags()
            .iter()
            .filter(|t| t.group == mode)
            .filter_map(|t| h1::Model::read(map, t.data_address).ok())
            .collect();
        for t in map.tags().iter().filter(|t| t.group == antr) {
            graphs += 1;
            let model = map.find(mode, &t.name);
            let (g, out) = match crate::graph_from_map(map, t, model) {
                Ok((g, _, out)) => (g, Some(out)),
                // A graph without nodes whose model has another name (d40's
                // airlock door) reads but cannot convert on its own.
                Err(e) => match h1::Graph::read(map, t.data_address) {
                    Ok(g) if g.nodes.is_empty() && model.is_none() => {
                        no_skeleton.push(format!("{stem} {}", t.name));
                        (g, None)
                    }
                    _ => {
                        failures.push(format!("{stem} {}: {e}", t.name));
                        continue;
                    }
                },
            };
            for (k, (c, u, name)) in pairs.iter().enumerate() {
                for (slot, (m, path)) in [c, u].into_iter().enumerate() {
                    if (*m, *path) == (stem.as_str(), t.name.as_str()) {
                        copies[k][slot] = g.animations.iter().find(|a| a.name == *name).cloned();
                    }
                }
            }
            if g.animations.iter().all(|a| a.compressed.is_none()) {
                continue;
            }
            for a in g.animations.iter().filter(|a| a.compressed.is_some()) {
                let stats = a.compressed.unwrap();
                compressed += 1;
                frames += a.frames.len();
                keys += stats.keys;
                let who = format!("{stem} {} / {}", t.name, a.name);
                if a.frames.len() != usize::from(a.frame_count) {
                    failures.push(format!("{who}: {} frames", a.frames.len()));
                }
                if stats.short_channels != 0 || stats.zero_rotations != 0 {
                    failures.push(format!("{who}: {stats:?}"));
                }
                for (f, pose) in a.frames.iter().enumerate() {
                    for (n, x) in pose.iter().enumerate() {
                        let l: f32 = x.rotation.iter().map(|v| v * v).sum();
                        let finite = x.translation.iter().all(|v| v.is_finite() && v.abs() < 1e4)
                            && x.scale.is_finite()
                            && x.scale > 0.0;
                        if (l - 1.0).abs() > 1e-4 || !finite {
                            failures.push(format!("{who}: frame {f} node {n}: {x:?}"));
                        }
                    }
                }
                // Undriven channels hold the per-node statics. Cinematic
                // graphs often hold a pose there rather than the model's rest
                // pose (their uncompressed animations do the same), so this
                // is reported, not asserted.
                let nodes = g.nodes.len().max(usize::from(a.node_count));
                if let Some(m) = models.iter().find(|m| {
                    m.node_list_checksum == a.node_list_checksum && m.nodes.len() == nodes
                }) {
                    for (n, node) in m.nodes.iter().enumerate() {
                        if !a.has(0, n) {
                            pose_channels += 1;
                            let e = angle(a.frames[0][n].rotation, h1::normalize(node.rotation));
                            pose_matching += usize::from(e < 0.01);
                        }
                    }
                }
            }
            // Through the Halo 2 decoder.
            let Some(out) = out else {
                continue;
            };
            let skeleton = if g.nodes.is_empty() {
                model
                    .map(|m| {
                        h1::AnimNode::from_model(&h1::Model::read(map, m.data_address).unwrap())
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let (bytes, ids) = build(vec![(out, t.name.as_str())]);
            for (name, e) in check_graph(&g, &skeleton, bytes, ids[0]) {
                worst_roundtrip = worst_roundtrip.max(e[0]);
                if e[0] > 2e-4 || e[1] != 0.0 || e[2] != 0.0 {
                    failures.push(format!("{stem} {} / {name}: round trip {e:?}", t.name));
                }
            }
        }
    }
    println!(
        "{} maps, {graphs} graphs, {compressed} compressed animations, {frames} frames, {keys} keys",
        maps.len()
    );
    println!(
        "rest pose: {pose_matching} of {pose_channels} undriven rotation channels within 0.01 rad of the model's"
    );
    println!("read, not converted (no model of the same name): {no_skeleton:?}");
    println!("halo 2 round trip: max rotation {worst_roundtrip:.2e} rad");
    for ((c, u, name), [a, b]) in pairs.iter().zip(&copies) {
        let (a, b) = (
            a.as_ref().expect("compressed copy"),
            b.as_ref().expect("plain copy"),
        );
        assert!(a.compressed.is_some() && b.compressed.is_none());
        let (r, t) = frame_error(a, b);
        println!("{name}: {c:?} vs {u:?}: max rotation {r:.4} rad, translation {t:.2e}");
        assert_eq!(a.animated, b.animated, "{name}: channels");
        assert!(r < 0.05 && t < 0.01, "{name}: {r} rad, {t}");
    }
    println!("failures: {failures:#?}");
    assert!(failures.is_empty());
    assert_eq!(compressed, 186);
}

/// Halo 2's engine looks some markers up by names written in its code
/// (`left_hand` for a thrown grenade); a converted model carries each under
/// that name too when it has the Halo 1 marker (`left hand`), keeping the
/// Halo 1 name the converted tags write.
#[test]
fn a_converted_model_carries_the_engine_s_marker_names() {
    let names = ["left hand", "Primary Trigger", "head", "antenna", "driver enter"];
    assert_eq!(
        crate::mode::marker_aliases(&names),
        [("left_hand", 0), ("primary_trigger", 1)]
    );
    assert!(crate::mode::marker_aliases(&["left_hand", "head"]).is_empty());
}

/// A strip built from a triangle list reads back as that list, in order
/// and winding, whatever the triangles' parity in the strip they came from.
#[test]
fn a_triangle_list_strip_reads_back_as_the_list() {
    let strip = [0u16, 1, 2, 3, 4, 5, 5, 6, 6, 7, 8];
    let tris = h1::strip_triangles(&strip);
    for keep in [
        tris.clone(),
        vec![tris[1]],
        vec![tris[0], tris[2], tris[3]],
        Vec::new(),
    ] {
        assert_eq!(h1::strip_triangles(&h1::triangles_strip(&keep)), keep);
    }
    // Triangles sharing vertices across the join still read back whole.
    let shared = vec![[0, 1, 2], [2, 1, 0], [2, 3, 1]];
    assert_eq!(h1::strip_triangles(&h1::triangles_strip(&shared)), shared);
}

// ---------------------------------------------------------------------------
// Vehicle graphs
// ---------------------------------------------------------------------------

/// The warthog's steering screen and its `fixed` class's aiming screen, as
/// Halo 1 holds them (35 degrees a frame, one frame each way; 90 degrees a
/// yaw frame, two each way, 15 and 35 degrees a pitch frame, one each way).
fn steering_screen() -> h1::AimScreen {
    h1::AimScreen {
        right_yaw_per_frame: 0.6109,
        left_yaw_per_frame: 0.6109,
        right_frames: 1,
        left_frames: 1,
        ..Default::default()
    }
}
fn aiming_screen() -> h1::AimScreen {
    h1::AimScreen {
        right_yaw_per_frame: std::f32::consts::FRAC_PI_2,
        left_yaw_per_frame: std::f32::consts::FRAC_PI_2,
        right_frames: 2,
        left_frames: 2,
        down_pitch_per_frame: 0.2618,
        up_pitch_per_frame: 0.6109,
        down_frames: 1,
        up_frames: 1,
    }
}

/// The synthetic graph's header with Halo 1's vehicle tables added as the
/// warthog lays them out: a `stand` unit (acc slot 2 naming animation 4)
/// whose `fixed` class (aiming screen, slot 25 naming animation 2) lists
/// the type `cg`; a vehicles element (steering screen, slot 0 naming
/// animation 0, two suspension elements naming animation 1 and mass
/// points 1 and 0); an object overlay naming animation 3 by function 3.
fn vehicle_image() -> (Image, u32) {
    let (mut img, at) = synthetic_graph();
    let o = (at - BASE) as usize;
    let mut h = img.0[o..o + h1::ANTR_HEADER].to_vec();
    let slots = |img: &mut Image, s: &[i16]| {
        img.alloc(&s.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>())
    };
    // Type `cg`, no slots.
    let mut ty = vec![0u8; 0x3c];
    name(&mut ty, 0, "cg");
    let ty = img.alloc(&ty);
    let mut class_slots = vec![-1i16; 32];
    class_slots[25] = 2;
    let cs = slots(&mut img, &class_slots);
    let mut class = vec![0u8; 0xbc];
    name(&mut class, 0, "fixed");
    name(&mut class, 0x40, "gun");
    class[0x60..0x78].copy_from_slice(&aiming_screen().bytes());
    block(&mut class, 0x98, 32, cs);
    block(&mut class, 0xb0, 1, ty);
    let class = img.alloc(&class);
    let us = slots(&mut img, &[-1, -1, 4]);
    let mut unit = vec![0u8; 0x64];
    name(&mut unit, 0, "stand");
    block(&mut unit, 0x40, 3, us);
    block(&mut unit, 0x58, 1, class);
    let unit = img.alloc(&unit);
    block(&mut h, 0x0c, 1, unit);
    let vs = slots(&mut img, &[0, -1, -1, -1, -1, -1]);
    let mut susp = vec![0u8; 2 * 0x14];
    put16(&mut susp, 0, 1);
    put16(&mut susp, 2, 1);
    putf(&mut susp, 4, &[-0.31445, -0.13452]);
    put16(&mut susp, 0x14, 0);
    put16(&mut susp, 0x16, 1);
    putf(&mut susp, 0x18, &[-0.3, -0.1]);
    let susp = img.alloc(&susp);
    let mut v = vec![0u8; 0x74];
    v[0..0x18].copy_from_slice(&steering_screen().bytes());
    block(&mut v, 0x5c, 6, vs);
    block(&mut v, 0x68, 2, susp);
    let v = img.alloc(&v);
    block(&mut h, 0x24, 1, v);
    let mut obj = vec![0u8; 0x14];
    put16(&mut obj, 0, 3);
    put16(&mut obj, 2, 3);
    let obj = img.alloc(&obj);
    block(&mut h, 0x00, 1, obj);
    let at = img.alloc(&h);
    (img, at)
}

/// Halo 1's vehicle tables read as laid out: the vehicles element's
/// steering screen, slots and suspension, the object overlays, and each
/// unit's and class's screen and slots.
#[test]
fn reads_a_vehicle_graph_s_tables() {
    let (img, at) = vehicle_image();
    let g = h1::Graph::read(&img, at).unwrap();
    let v = g.vehicle.as_ref().expect("vehicles element");
    assert_eq!(v.screen, steering_screen());
    assert_eq!(v.slots, [0, -1, -1, -1, -1, -1]);
    assert_eq!(
        v.suspension[0],
        h1::Suspension {
            mass_point: 1,
            animation: 1,
            full_extension_ground_depth: -0.31445,
            full_compression_ground_depth: -0.13452,
        }
    );
    assert_eq!(v.suspension.len(), 2);
    assert_eq!(
        g.objects,
        [h1::ObjectOverlay {
            animation: 3,
            function: 3,
            controls: 0
        }]
    );
    let u = &g.units[0];
    assert_eq!(
        (u.label.as_str(), &u.slots[..]),
        ("stand", &[-1, -1, 4][..])
    );
    assert!(!u.screen.is_set());
    let c = &u.classes[0];
    assert_eq!(c.label, "fixed");
    assert_eq!(c.screen, aiming_screen());
    assert_eq!(c.slots[25], 2);
    assert_eq!(c.types, [("cg".to_owned(), vec![])]);
    assert_eq!(
        h1::weapon_types(&img, at).unwrap(),
        [("cg".to_owned(), "fixed".to_owned())]
    );
    // The same classes in a graph without a vehicles element (a biped's,
    // whose converted graph holds only the types its animations name).
    let (mut img, at) = vehicle_image();
    let o = (at - BASE) as usize;
    img.0[o + 0x24..o + 0x28].fill(0);
    assert!(h1::weapon_types(&img, at).unwrap().is_empty());
    // A graph without the tables has none.
    let (img, at) = synthetic_graph();
    let g = h1::Graph::read(&img, at).unwrap();
    assert!(g.vehicle.is_none() && g.objects.is_empty());
}

/// The vehicle graph with six animations named as the warthog's: 0
/// `vehicle steering`, 1 `suspension left back`, 2 `stand fixed
/// aim-still`, 3 `firing`, 4 `stand acc-front-back` (overlays), 5 `stand
/// opening` (base).
fn vehicle_graph() -> h1::Graph {
    let (img, at) = vehicle_image();
    let mut g = h1::Graph::read(&img, at).unwrap();
    let base = g.animations[0].clone();
    let overlay = g.animations[1].clone();
    g.animations = [
        "vehicle steering",
        "suspension left back",
        "stand fixed aim-still",
        "firing",
        "stand acc-front-back",
    ]
    .iter()
    .map(|n| h1::Animation {
        name: (*n).into(),
        ..overlay.clone()
    })
    .chain([h1::Animation {
        name: "stand opening".into(),
        ..base
    }])
    .collect();
    g
}

/// A converted graph read back raw: `w(address)` and string names.
struct Raw32 {
    b: Vec<u8>,
    base: u32,
    strings: Vec<Option<String>>,
    map: Vec<u8>,
}
impl Raw32 {
    fn new(out: Converted) -> Self {
        let (bytes, ids) = build(vec![(out, "invented\\graph")]);
        let mut map = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
        let base = map.tag(ids[0]).unwrap().location.unwrap().address;
        let b = map.read_tag_bytes(ids[0]).unwrap();
        let strings = map.strings().to_vec();
        Self {
            b,
            base,
            strings,
            map: bytes,
        }
    }
    fn w(&self, a: u32) -> u32 {
        u32::from_le_bytes(self.b[(a - self.base) as usize..][..4].try_into().unwrap())
    }
    fn f(&self, a: u32) -> f32 {
        f32::from_bits(self.w(a))
    }
    fn name(&self, sid: u32) -> String {
        if sid == 0 {
            return String::new();
        }
        self.strings[(sid & 0xff_ffff) as usize].clone().unwrap()
    }
    /// The elements of the block at `a`: (name at +0, address).
    fn level(&self, a: u32, stride: u32) -> Vec<(String, u32)> {
        (0..self.w(a))
            .map(|i| {
                let e = self.w(a + 4) + i * stride;
                (self.name(self.w(e)), e)
            })
            .collect()
    }
    /// A type's actions (`+4`) or overlays (`+0xc`): (name, animation).
    fn entries(&self, at: u32) -> Vec<(String, i16)> {
        let mut e: Vec<_> = self
            .level(at, 8)
            .into_iter()
            .map(|(n, a)| (n, (self.w(a + 4) >> 16) as i16))
            .collect();
        e.sort();
        e
    }
    /// The type at mode, class, type names.
    fn ty(&self, path: [&str; 3]) -> Option<u32> {
        let m = self
            .level(self.base + 0x34, 0x14)
            .into_iter()
            .find(|m| m.0 == path[0])?;
        let c = self
            .level(m.1 + 4, 0x14)
            .into_iter()
            .find(|c| c.0 == path[1])?;
        Some(
            self.level(c.1 + 4, 0x34)
                .into_iter()
                .find(|t| t.0 == path[2])?
                .1,
        )
    }
}

/// A converted vehicle graph is laid out as Halo 2's vehicle graphs are
/// (V: the retail warthog, ghost, chaingun and scorpion graphs): the
/// steering slot an overlay `steering` of `any`, the aim overlay
/// `aim_still_up` and the acceleration overlay among their types'
/// overlays, only base animations among the actions; the steering and
/// aiming screens as blend screens on those two animations, which are
/// written with the blend-screen codec; the suspension element whose
/// mass point has a marker in the suspension block, naming the marker,
/// with Halo 1's depths negated; the object overlay naming
/// `barrel_spin`; the weapon pairs. No `vehicle`, `suspension` or
/// `firing` mode is left.
#[test]
fn a_vehicle_graph_takes_halo_2s_vehicle_layout() {
    let g = vehicle_graph();
    let opts = jmad::GraphOptions {
        mass_point_markers: vec![None, Some("tire_back_left".into())],
    };
    let (out, report) = jmad::convert_graph_with(&g, None, &opts).unwrap();
    assert_eq!(report.suspension_without_marker, ["suspension left back"]);
    assert_eq!(report.names[2].1, "combat:fixed:aim_still_up");
    let r = Raw32::new(out.clone());
    let modes: Vec<String> = r
        .level(r.base + 0x34, 0x14)
        .into_iter()
        .map(|m| m.0)
        .collect();
    let mut sorted = modes.clone();
    sorted.sort();
    assert_eq!(sorted, ["any", "combat"]);
    let any = r.ty(["any", "any", "any"]).unwrap();
    assert!(r.entries(any + 4).is_empty());
    assert_eq!(r.entries(any + 0xc), [("steering".to_owned(), 0)]);
    let combat = r.ty(["combat", "any", "any"]).unwrap();
    assert_eq!(r.entries(combat + 4), [("opening".to_owned(), 5)]);
    assert_eq!(r.entries(combat + 0xc), [("acc_front_back".to_owned(), 4)]);
    let fixed = r.ty(["combat", "fixed", "any"]).unwrap();
    assert!(r.entries(fixed + 4).is_empty());
    assert_eq!(r.entries(fixed + 0xc), [("aim_still_up".to_owned(), 2)]);

    // Blend screens: steering, then aiming, each Halo 1's 24 bytes.
    let screens = r.level(r.base + 0x24, 0x1c);
    assert_eq!(
        screens.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
        ["steering", "aiming"]
    );
    for ((_, a), want) in screens.iter().zip([steering_screen(), aiming_screen()]) {
        let at = (a + 4 - r.base) as usize;
        assert_eq!(r.b[at..at + 0x18], want.bytes());
    }
    let anims = r.w(r.base + 0x30);
    let screen_of = |i: u32| r.b[(anims + i * 0x6c + 0x12 - r.base) as usize] as i8;
    assert_eq!(
        (0..6).map(screen_of).collect::<Vec<_>>(),
        [0, -1, 1, -1, -1, -1]
    );
    // Their data is codec 8, the others' codec 3 (the stream's first
    // byte; the data resource is the graph's one, `+0xac`).
    let res_ptr = r.w(r.base + 0xb0);
    let raw = RawPointer::decode(r.w(res_ptr + 8)).unwrap();
    let codec = |i: u32| {
        let off = r.w(anims + i * 0x6c + 0x28);
        r.map[raw.offset as usize + off as usize]
    };
    assert_eq!((0..5).map(codec).collect::<Vec<_>>(), [8, 3, 8, 3, 3]);
    // Every blob starts 16-byte aligned: codec 8's rotation reads are
    // `movaps` (E `0x0028c589`). Animation 2 followed 0 and 1 at a
    // 4-aligned offset through converter version 88.
    for i in 0..6 {
        assert_eq!(r.w(anims + i * 0x6c + 0x28) % 16, 0, "animation {i}");
    }

    // Suspension: the one element whose mass point has a marker.
    let susp = r.level(r.base + 0x3c, 0x28);
    assert_eq!(susp.len(), 1);
    let a = susp[0].1;
    assert_eq!(susp[0].0, "suspension:left_back");
    assert_eq!(r.w(a + 4), 1 << 16 | 0xffff);
    assert_eq!(r.name(r.w(a + 8)), "tire_back_left");
    assert_eq!(r.f(a + 0xc), 0.0);
    // The kept element is Halo 1's first (mass point 1, the one with a
    // marker; depths -0.31445 and -0.13452), negated; the second (mass
    // point 0, -0.3 and -0.1) has no marker and is dropped.
    assert_eq!((r.f(a + 0x10), r.f(a + 0x14)), (0.31445, 0.13452));
    assert_eq!(r.w(a + 0x18), 0);
    assert_eq!(
        (r.f(a + 0x1c), r.f(a + 0x20), r.f(a + 0x24)),
        (0.0, 0.31445, 0.13452)
    );

    // The object overlay.
    let objects = r.level(r.base + 0x44, 0x14);
    assert_eq!(objects.len(), 1);
    let a = objects[0].1;
    assert_eq!(objects[0].0, "firing");
    assert_eq!(r.w(a + 4), 3 << 16 | 0xffff);
    assert_eq!(r.w(a + 8), 0);
    assert_eq!(r.name(r.w(a + 0xc)), "barrel_spin");

    // Weapon pairs.
    let mut pairs: Vec<(String, String)> = (0..r.w(r.base + 0x54))
        .map(|i| {
            let e = r.w(r.base + 0x58) + 8 * i;
            (r.name(r.w(e)), r.name(r.w(e + 4)))
        })
        .collect();
    pairs.sort();
    assert_eq!(
        pairs,
        [
            ("any".to_owned(), "any".to_owned()),
            ("cg".to_owned(), "fixed".to_owned())
        ]
    );

    // Every animation still decodes to Halo 1's frames, the codec 8 ones
    // exactly up to float rounding.
    let (bytes, ids) = build(vec![(out, "invented\\graph")]);
    for (name, e) in check_graph(&g, &[], bytes, ids[0]) {
        assert!(e[0] < 2e-4 && e[1] == 0.0 && e[2] == 0.0, "{name}: {e:?}");
    }
}

/// A base animation with a blend screen keeps its static stream, padded
/// so its codec 8 stream after it starts 16-byte aligned (E: `movaps` at
/// `0x0028c589`); the static size covers the padding and the clip decodes
/// to Halo 1's frames.
#[test]
fn a_blend_screen_after_a_static_stream_is_aligned() {
    let mut g = vehicle_graph();
    // Steering and aim as base animations whose static stream holds one
    // scale (node 0's, a default channel of the source): 0x24 bytes, so
    // codec 8 would follow at +0x24. The other channels are animated.
    let base = g.animations[5].clone();
    for i in [0, 2] {
        g.animations[i] = h1::Animation {
            name: g.animations[i].name.clone(),
            animated: [0b111, 0b111, 0b110],
            ..base.clone()
        };
    }
    let (out, _) = jmad::convert_graph(&g, None).unwrap();
    let r = Raw32::new(out.clone());
    let anims = r.w(r.base + 0x30);
    let res_ptr = r.w(r.base + 0xb0);
    let raw = RawPointer::decode(r.w(res_ptr + 8)).unwrap();
    for i in [0u32, 2] {
        let a = anims + i * 0x6c;
        let off = r.w(a + 0x28) as usize;
        let static_size = (r.w(a + 0x40) >> 16) as usize;
        assert_eq!(r.map[raw.offset as usize + off], 1, "codec 1 first");
        assert_eq!(static_size, 0x30, "0x24 padded to 16");
        assert_eq!((off + static_size) % 16, 0);
        assert_eq!(r.map[raw.offset as usize + off + static_size], 8);
    }
    let (bytes, ids) = build(vec![(out, "invented\\graph")]);
    for (name, e) in check_graph(&g, &[], bytes, ids[0]) {
        assert!(e[0] < 2e-4 && e[1] == 0.0 && e[2] == 0.0, "{name}: {e:?}");
    }
}

/// A graph without Halo 1's vehicle tables converts as before: no blend
/// screens, suspension, object overlays or weapon pairs, and its aim
/// animation keeps its name.
#[test]
fn a_graph_without_vehicle_tables_is_unchanged() {
    let (img, at) = synthetic_graph();
    let g = h1::Graph::read(&img, at).unwrap();
    let (out, report) = jmad::convert_graph(&g, None).unwrap();
    assert_eq!(report.names[1].1, "combat:rifle:aim_still");
    let r = Raw32::new(out);
    for at in [0x24, 0x3c, 0x44, 0x54] {
        assert_eq!(r.w(r.base + at), 0, "+{at:#x}");
    }
}

#[test]
fn sentinel_fixed_slots_build_a_runtime_idle_and_aim_overlay() {
    let (img, at) = synthetic_graph();
    let mut g = h1::Graph::read(&img, at).unwrap();
    g.animations[0].name = "stand fixed idle%0".into();
    g.animations[1].name = "stand fixed aim-still".into();
    let mut slots = vec![-1; 26];
    slots[0] = 0;
    slots[25] = 1;
    g.units = vec![h1::UnitSlots {
        label: "stand".into(),
        classes: vec![h1::ClassSlots {
            label: "fixed".into(),
            slots,
            screen: h1::AimScreen {
                right_frames: 1,
                right_yaw_per_frame: 0.35,
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    }];
    let (out, _) = jmad::convert_graph_for(&g, None, &Default::default(), true).unwrap();
    let (bytes, ids) = build(vec![(out, "invented\\sentinel")]);
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let graph = tags::anim::AnimationGraph::read(&mut map, ids[0]).unwrap();
    let strings = map.strings();
    let mode = graph
        .modes
        .iter()
        .find(|m| m.name.resolve(strings).unwrap() == Some("combat"))
        .unwrap();
    let class = mode
        .classes
        .iter()
        .find(|c| c.name.resolve(strings).unwrap() == Some("any"))
        .unwrap();
    let ty = &class.types[0];
    assert!(
        ty.actions
            .iter()
            .any(|a| a.name.resolve(strings).unwrap() == Some("idle") && a.animation_index == 0)
    );
    for name in [
        "aim_still_up",
        "aim_still_down",
        "aim_move_up",
        "aim_move_down",
    ] {
        assert!(
            ty.overlays
                .iter()
                .any(|a| a.name.resolve(strings).unwrap() == Some(name) && a.animation_index == 1)
        );
    }
    assert_eq!(graph.animations[1].unknown_12, 0);
    assert_eq!(graph.blend_screens.len(), 1);
}
