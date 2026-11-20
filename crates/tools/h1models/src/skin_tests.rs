//! Biped skinning round trips on the Halo 1 disc (`$H1_ISO`) and a retail
//! Halo 2 biped (`$H2_DISC`). Each test prints only counts and error
//! figures.

use crate::h1;
use crate::mode::world_transforms;

type Mat = ([[f32; 3]; 3], [f32; 3]);

fn conj(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

fn apply(m: &Mat, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m.1[r] + (0..3).map(|k| m.0[r][k] * v[k]).sum::<f32>())
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

fn h1_disc_map(stem: &str) -> Option<h1map::H1Map> {
    let path = std::env::var_os("H1_ISO")?;
    let file = std::io::BufReader::new(std::fs::File::open(path).expect("H1_ISO opens"));
    let mut iso = xiso::Xiso::open(file).expect("disc reads");
    let disc = h1map::disc::Disc::open(&mut iso).expect("a Halo 1 disc");
    let e = disc.maps.iter().find(|e| h1map::disc::map_stem(e) == stem)?;
    let bytes = iso.read(e).expect("map reads");
    Some(h1map::H1Map::from_reader(std::io::Cursor::new(bytes)).expect("map parses"))
}

/// Halo 1's rotation convention, from its own data. The reader returns
/// rotations in Halo 2's convention (`h1::h2_rotation`); composing the
/// nodes with them must put each vertex nearer its own node than the
/// stored (conjugate) form does. Also prints how each graph's nodes line up
/// with its model's, and the first-person models' and graphs' node lists.
///
/// `H1_ISO=disc/halo1.iso cargo test -p h1models --release -- --ignored h1_disc_biped_conventions --nocapture`
#[test]
#[ignore = "needs $H1_ISO"]
fn h1_disc_biped_conventions() {
    let mut checked = 0;
    for stem in ["a10", "bloodgulch"] {
        let Some(map) = h1_disc_map(stem) else {
            return;
        };
        for t in map.tags() {
            if t.group != h1map::Group::new(b"mode") {
                continue;
            }
            let model = h1::Model::read(&map, t.data_address).unwrap();
            if t.name.contains("\\fp\\") {
                println!(
                    "{stem} fp mode {}: {:?}",
                    t.name,
                    model.nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>()
                );
            }
            if !t.name.starts_with("characters\\") || t.name.contains("\\fp\\") {
                continue;
            }
            let pose = |stored: bool| {
                world_transforms(
                    &model
                        .nodes
                        .iter()
                        .map(|n| (n.parent, if stored { conj(n.rotation) } else { n.rotation }, n.translation))
                        .collect::<Vec<_>>(),
                )
            };
            let (converted, stored) = (pose(false), pose(true));
            let mut sum = [0f64; 2];
            let mut count = 0usize;
            for p in &model.geometries[0].parts {
                for v in &p.vertices {
                    let n = v.nodes[0].max(0) as usize;
                    sum[0] += f64::from(dist(v.position, converted[n].1));
                    sum[1] += f64::from(dist(v.position, stored[n].1));
                    count += 1;
                }
            }
            let (c, s) = (sum[0] / count as f64, sum[1] / count as f64);
            println!(
                "{stem} {}: {} nodes, {count} vertices; mean vertex-node distance converted {c:.3}, as stored {s:.3}",
                t.name,
                model.nodes.len(),
            );
            assert!(c * 2.0 < s, "{}: converted {c}, stored {s}", t.name);
            checked += 1;
        }
        for t in map.tags() {
            if t.group != h1map::Group::new(b"antr")
                || !(t.name.starts_with("characters\\") || t.name.contains("\\fp\\"))
            {
                continue;
            }
            let g = h1::Graph::read(&map, t.data_address).unwrap();
            let names: Vec<&str> = g.nodes.iter().map(|n| n.name.as_str()).collect();
            let same = map.find(h1map::Group::new(b"mode"), &t.name).map(|m| {
                let m = h1::Model::read(&map, m.data_address).unwrap();
                m.nodes.iter().map(|n| n.name.as_str()).eq(names.iter().copied())
            });
            println!("{stem} antr {}: {} nodes, same order as its mode: {same:?}", t.name, names.len());
            if t.name.contains("\\fp\\") {
                println!("  nodes {names:?}");
            }
        }
    }
    assert!(checked > 0 || std::env::var_os("H1_ISO").is_none());
}

/// Retail Halo 2 map bytes and its shared maps, from `$H2_DISC`.
struct H2Disc {
    local: Vec<u8>,
    shared: Vec<u8>,
    sp_shared: Vec<u8>,
}
impl tags::geometry::RawSource for H2Disc {
    fn read_raw(&mut self, p: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        let b = match p.file {
            map::ResourceFile::Local => &self.local,
            map::ResourceFile::Shared => &self.shared,
            map::ResourceFile::SinglePlayerShared => &self.sp_shared,
            map::ResourceFile::MainMenu => return Err(tags::Error::Invalid("mainmenu")),
        };
        let s = p.offset as usize;
        b.get(s..s + size as usize)
            .map(<[u8]>::to_vec)
            .ok_or(tags::Error::Invalid("raw range"))
    }
}
impl tags::sound::ResourceReader for H2Disc {
    fn check(&self, _: map::RawPointer, _: u32) -> tags::Result<()> {
        Ok(())
    }
    fn read_raw(&mut self, p: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        tags::geometry::RawSource::read_raw(self, p, size)
    }
}

fn h2_disc(stem: &str) -> Option<H2Disc> {
    let path = std::env::var_os("H2_DISC")?;
    let file = std::io::BufReader::new(std::fs::File::open(path).expect("H2_DISC opens"));
    let mut iso = xiso::Xiso::open(file).expect("disc reads");
    let entries = iso.entries().unwrap();
    let mut get = |name: &str| {
        let e = entries
            .iter()
            .find(|e| !e.is_dir && e.path.ends_with(&format!("/{name}.map")) || e.path == format!("{name}.map"))
            .unwrap_or_else(|| panic!("{name}.map on the disc"));
        iso.read(e).unwrap()
    };
    Some(H2Disc {
        local: get(stem),
        shared: get("shared"),
        sp_shared: get("single_player_shared"),
    })
}

/// The model-space rest frame of every node of a decoded Halo 2 model,
/// from `+0xc`/`+0x18`.
fn h2_rest(m: &tags::model::RenderModelGeometry) -> Vec<Mat> {
    world_transforms(
        &m.nodes
            .iter()
            .map(|n| (n.wire.parent_index, n.wire.orientation, n.wire.translation))
            .collect::<Vec<_>>(),
    )
}

/// Mean distance from each vertex to its first node's rest position, with
/// the node index through the section's node map (`mapped`) and taken as a
/// model node index directly.
fn h2_vertex_node_distance(m: &tags::model::RenderModelGeometry, section: usize) -> (f64, f64, usize) {
    let rest = h2_rest(m);
    let s = &m.sections[section];
    let v = &s.vertices;
    let (mut a, mut b, mut n) = (0f64, 0f64, 0usize);
    for i in 0..v.positions.len() {
        let Some(ix) = v.node_indices.get(i) else { break };
        let local = usize::from(ix[0]);
        let mapped = s.node_map.get(local).map_or(local, |&g| usize::from(g));
        if mapped < rest.len() && local < rest.len() {
            a += f64::from(dist(v.positions[i], rest[mapped].1));
            b += f64::from(dist(v.positions[i], rest[local].1));
            n += 1;
        }
    }
    (a / n.max(1) as f64, b / n.max(1) as f64, n)
}

/// Retail Halo 2 bipeds against themselves: which node-index reading puts
/// vertices next to their nodes, the section record words the converter
/// writes, and whether the jmad's node order is the mode's.
#[test]
#[ignore = "needs $H2_DISC"]
fn retail_h2_biped_skinning() {
    let stem = std::env::var("H2_MAP").unwrap_or_else(|_| "05a_deltaapproach".into());
    let Some(mut disc) = h2_disc(&stem) else {
        return;
    };
    let mut map = map::Map::from_reader(std::io::Cursor::new(disc.local.clone())).unwrap();
    let tags_list: Vec<(map::FourCC, String, map::TagId)> = map
        .tags()
        .iter()
        .filter(|t| t.location.is_some())
        .map(|t| (t.group, t.name.clone().unwrap_or_default(), t.id))
        .collect();
    // First-person: how a weapon's fp graph's nodes line up with the fp
    // arms model's and the weapon's fp model's.
    let strings = map.strings().to_vec();
    let node_names = |map: &mut map::Map<std::io::Cursor<Vec<u8>>>, id: map::TagId, at: usize, stride: usize| -> Vec<String> {
        let tag = map.read_tag_bytes(id).unwrap();
        let base = map.tag(id).unwrap().location.unwrap().address;
        let (count, ptr) = (h1::u32_at(&tag, at) as usize, h1::u32_at(&tag, at + 4));
        (0..count)
            .map(|i| {
                let o = (ptr - base) as usize + i * stride;
                tags::StringId(h1::u32_at(&tag, o)).resolve(&strings).ok().flatten().unwrap_or("").to_owned()
            })
            .collect()
    };
    let arms: Vec<String> = tags_list
        .iter()
        .find(|(g, n, _)| g.0 == *b"mode" && n.ends_with("masterchief\\fp\\fp"))
        .map(|(_, _, id)| node_names(&mut map, *id, 0x48, 0x60))
        .unwrap_or_default();
    println!("fp arms nodes {}: {arms:?}", arms.len());
    // The first-person body is drawn with the third-person unit's node
    // matrices, only when the two `mode`s agree on the node count (+0x48)
    // and +0x44 (E 0x001661d8, 0x001661e0): the Chief's do.
    let header = |map: &mut map::Map<std::io::Cursor<Vec<u8>>>, suffix: &str| {
        tags_list.iter().find(|(g, n, _)| g.0 == *b"mode" && n.ends_with(suffix)).map(|(_, _, id)| {
            let tag = map.read_tag_bytes(*id).unwrap();
            (h1::u32_at(&tag, 0x48), h1::u32_at(&tag, 0x44))
        })
    };
    if let (Some(body), Some(chief)) = (
        header(&mut map, "masterchief\\fp_body\\fp_body"),
        header(&mut map, "masterchief\\masterchief"),
    ) {
        println!("fp_body (nodes, +0x44) {body:?}, the Chief's {chief:?}");
        assert_eq!(body, chief);
    }
    for (g, name, id) in &tags_list {
        if g.0 == *b"mode" && name.contains("\\fp\\") {
            let n = node_names(&mut map, *id, 0x48, 0x60);
            println!("fp mode {name}: {n:?}");
        }
    }
    for (g, name, id) in &tags_list {
        if g.0 != *b"jmad" || !name.contains("\\fp\\") {
            continue;
        }
        let jn = node_names(&mut map, *id, 0x0c, 0x20);
        let weapon: Vec<String> = tags_list
            .iter()
            .find(|(g, n, _)| g.0 == *b"mode" && n == name)
            .map(|(_, _, id)| node_names(&mut map, *id, 0x48, 0x60))
            .unwrap_or_default();
        let arms_at: Vec<Option<usize>> = arms.iter().map(|a| jn.iter().position(|j| j == a)).collect();
        let weapon_at: Vec<Option<usize>> = weapon.iter().map(|a| jn.iter().position(|j| j == a)).collect();
        let prefix = jn.len() >= arms.len() && jn[..arms.len()] == arms[..];
        println!(
            "fp jmad {name}: {} nodes; the arms' {} nodes first, in order: {prefix}; weapon mode {} nodes at {weapon_at:?}\n  {jn:?}",
            jn.len(),
            arms.len(),
            weapon.len()
        );
        let _ = arms_at;
        // Every retail first-person graph lists the arms model's nodes
        // first, in the arms model's order, then the weapon's. That is the
        // tool's order: the engine binds by name (h1_disc_first_person_skin).
        assert!(arms.is_empty() || prefix, "{name}");
    }
    for (g, name, id) in &tags_list {
        if g.0 != *b"mode" || !name.contains("characters") {
            continue;
        }
        let m = match tags::model::RenderModelGeometry::read(&mut map, *id, &mut disc) {
            Ok(m) => m,
            Err(e) => {
                println!("{name}: {e}");
                continue;
            }
        };
        let tag = map.read_tag_bytes(*id).unwrap();
        let base = map.tag(*id).unwrap().location.unwrap().address;
        let u32_at = |o: usize| h1::u32_at(&tag, o);
        let sections_at = (u32_at(0x28) - base) as usize;
        println!("{name}: {} nodes, {} sections", m.nodes.len(), m.sections.len());
        for (si, s) in m.sections.iter().enumerate().take(6) {
            let rec = &tag[sections_at + si * 0x5c..sections_at + si * 0x5c + 0x30];
            let (a, b, n) = h2_vertex_node_distance(&m, si);
            // Node indices are section-local, through the node map.
            assert!(s.node_map.len() <= 1 || a <= b + 1e-6, "{name} section {si}: {a} {b}");
            println!(
                "  section {si}: class {} vertices {} node map {} rec[10..30] {:02x?}; mean vertex-node distance mapped {a:.3}, unmapped {b:.3} over {n}",
                h1::u16_at(rec, 0),
                s.vertices.positions.len(),
                s.node_map.len(),
                &rec[0x10..0x30]
            );
        }
        // The jmad of the same name.
        if let Some((_, _, jid)) = tags_list.iter().find(|(g, n, _)| g.0 == *b"jmad" && n == name) {
            let j = tags::anim::AnimationGraph::read(&mut map, *jid).unwrap();
            let strings = map.strings().to_vec();
            let jn: Vec<String> = j
                .nodes
                .iter()
                .map(|n| n.name.resolve(&strings).ok().flatten().unwrap_or("").to_owned())
                .collect();
            let mn: Vec<String> = m.nodes.iter().map(|n| n.name.clone().unwrap_or_default()).collect();
            println!("  jmad nodes {} same order as mode: {}", jn.len(), jn == mn);
            if jn != mn {
                println!("    jmad {jn:?}\n    mode {mn:?}");
            }
        }
    }
}

/// Model-space frames from parent-relative transforms whose rotations the
/// reader returned (Halo 1's stored rotations, converted).
fn h1_world(parents: &[i16], local: &[([f32; 4], [f32; 3])]) -> Vec<Mat> {
    world_transforms(
        &parents
            .iter()
            .zip(local)
            .map(|(&p, (r, t))| (p, *r, *t))
            .collect::<Vec<_>>(),
    )
}

/// Halo 1's own cache-computed inverse rest frame of each node (node
/// `+0x68`: scale, then three vectors that are the inverse's columns, then
/// the inverse's position).
fn h1_cached_inverse(map: &h1map::H1Map, address: u32, nodes: usize) -> Vec<Mat> {
    let h = map.read(address, h1::MODE_HEADER).unwrap();
    let raw = map.read(h1::u32_at(h, 0xb8 + 4), nodes * 0x9c).unwrap();
    (0..nodes)
        .map(|i| {
            let e = &raw[i * 0x9c..];
            let r = std::array::from_fn(|row| std::array::from_fn(|c| h1::f32_at(e, 0x6c + 12 * c + 4 * row)));
            (r, h1::f32s(e, 0x90))
        })
        .collect()
}

fn mul(a: &Mat, b: &Mat) -> Mat {
    let r = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a.0[i][k] * b.0[k][j]).sum()));
    (r, apply(a, b.1))
}

/// Linear blend skinning of one vertex.
fn skin(m: &[Mat], v: [f32; 3], nodes: &[(usize, f32)]) -> [f32; 3] {
    let mut out = [0f32; 3];
    let total: f32 = nodes.iter().map(|n| n.1).sum();
    for &(n, w) in nodes {
        let p = apply(&m[n], v);
        for k in 0..3 {
            out[k] += p[k] * w / total;
        }
    }
    out
}

/// Halo 1 vertex skin nodes: `(node, weight)`.
fn h1_nodes(v: &h1::Vertex) -> Vec<(usize, f32)> {
    let mut out = vec![(v.nodes[0].max(0) as usize, v.weights[0])];
    if v.nodes[1] >= 0 && v.weights[1] > 0.0 {
        out.push((v.nodes[1] as usize, v.weights[1]));
    }
    out
}

/// What one model's round trip measured.
#[derive(Debug, Default)]
struct Skinned {
    /// Halo 1's rule against Halo 1's own cached inverse frames.
    h1_inverse: f32,
    /// Our node inverse frames against Halo 1's cached ones.
    our_inverse: f32,
    vertices: usize,
    /// Bind pose: Halo 2 decode and skin against the Halo 1 vertex.
    bind: f32,
    /// Bind pose: the farthest vertex from the model's centre, over its
    /// Halo 1 bounding radius.
    bind_radius: f32,
    /// Posed: `(animation, frame, max error, max vertex travel)`.
    posed: Vec<(String, usize, f32, f32)>,
}

/// Converts `model` (and `graph`), builds a map, decodes both through
/// `crates/formats/tags` and skins every vertex in the bind pose and in sampled
/// frames, the Halo 2 way (decoded vertex, node map, weights, decoded
/// frame, the inverse frames the `mode` stores), against Halo 1's own
/// vertices skinned with Halo 1's own cached inverse frames. The reader's
/// rotations are checked against those cached frames first
/// (`h1_inverse`), so the Halo 1 side does not rest on the conversion.
fn skin_round_trip(map: &h1map::H1Map, mode_tag: &h1map::Tag, graph: Option<&h1map::Tag>) -> Skinned {
    use crate::tests::{Raw, build};
    let (src, conv) = crate::model_from_map(map, mode_tag, &crate::mode::ModelOptions::default()).unwrap();
    let n = src.nodes.len();
    let parents: Vec<i16> = src.nodes.iter().map(|x| x.parent).collect();
    let rest_h1 = h1_world(&parents, &src.nodes.iter().map(|x| (x.rotation, x.translation)).collect::<Vec<_>>());
    let cached = h1_cached_inverse(map, mode_tag.data_address, n);
    let mut out = Skinned::default();
    for (w, c) in rest_h1.iter().zip(&cached) {
        let id = mul(c, w);
        for i in 0..3 {
            for j in 0..3 {
                out.h1_inverse = out.h1_inverse.max((id.0[i][j] - f32::from(u8::from(i == j))).abs());
            }
            out.h1_inverse = out.h1_inverse.max(id.1[i].abs());
        }
    }
    let mut tags_in = vec![(conv, mode_tag.name.as_str())];
    let h1_graph = graph.map(|g| {
        let (hg, _, gc) = crate::graph_from_map(map, g, Some(mode_tag)).unwrap();
        tags_in.push((gc, g.name.as_str()));
        hg
    });
    let (bytes, ids) = build(tags_in);
    let mut m2 = map::Map::from_reader(std::io::Cursor::new(bytes.clone())).unwrap();
    let dm = tags::model::RenderModelGeometry::read(&mut m2, ids[0], &mut Raw(bytes.clone())).unwrap();
    // The converted inverse frames, as stored.
    let inverse = h2_inverse(&mut m2, ids[0]);
    for (a, b) in inverse.iter().zip(&cached) {
        for i in 0..3 {
            for j in 0..3 {
                out.our_inverse = out.our_inverse.max((a.0[i][j] - b.0[i][j]).abs());
            }
            out.our_inverse = out.our_inverse.max((a.1[i] - b.1[i]).abs());
        }
    }
    let rest_h2 = world_transforms(
        &dm.nodes
            .iter()
            .map(|x| (x.wire.parent_index, x.wire.orientation, x.wire.translation))
            .collect::<Vec<_>>(),
    );
    // Each H1 vertex with its decoded H2 counterpart.
    let mut pairs: Vec<(h1::Vertex, [f32; 3], Vec<(usize, f32)>)> = Vec::new();
    for (s, g) in dm.sections.iter().zip(&src.geometries) {
        let mut i = 0usize;
        for p in &g.parts {
            for v in &p.vertices {
                let ix = s.vertices.node_indices[i];
                let w = s.vertices.node_weights[i];
                let nodes = (0..4)
                    .filter(|&k| w[k] > 0.0)
                    .map(|k| {
                        let local = usize::from(ix[k]);
                        (s.node_map.get(local).map_or(local, |&g| usize::from(g)), w[k])
                    })
                    .collect();
                pairs.push((*v, s.vertices.positions[i], nodes));
                i += 1;
            }
        }
    }
    out.vertices = pairs.len();
    let skin_h2 = |world: &[Mat]| -> Vec<Mat> { world.iter().zip(&inverse).map(|(w, i)| mul(w, i)).collect() };
    let skin_h1 = |world: &[Mat]| -> Vec<Mat> { world.iter().zip(&cached).map(|(w, i)| mul(w, i)).collect() };
    let (bind2, bind1) = (skin_h2(&rest_h2), skin_h1(&rest_h1));
    let centre: [f32; 3] = std::array::from_fn(|k| {
        pairs.iter().map(|p| p.0.position[k]).sum::<f32>() / pairs.len().max(1) as f32
    });
    let radius = pairs.iter().map(|p| dist(p.0.position, centre)).fold(0f32, f32::max);
    for (v, p2, nodes) in &pairs {
        let a = skin(&bind1, v.position, &h1_nodes(v));
        let b = skin(&bind2, *p2, nodes);
        out.bind = out.bind.max(dist(a, b));
        out.bind_radius = out.bind_radius.max(dist(b, centre) / radius);
    }
    // Posed: frames of base animations, the Halo 1 frame against the
    // decoded jmad clip.
    if let (Some(hg), Some(gid)) = (h1_graph, ids.get(1)) {
        let g2 = tags::anim::AnimationGraph::read(&mut m2, *gid).unwrap();
        let resource = g2.read_resource(0, &mut Raw(bytes.clone())).unwrap();
        let picks: Vec<usize> = hg
            .animations
            .iter()
            .enumerate()
            .filter(|(_, a)| a.kind == h1::AnimType::Base)
            .map(|(i, _)| i)
            .collect();
        for &ai in picks.iter().step_by((picks.len() / 6).max(1)) {
            let a = &hg.animations[ai];
            let clip = g2.clip_from_shared(ai, &resource).unwrap();
            let f = usize::from(a.frame_count) / 2;
            let w1 = h1_world(&parents, &a.frames[f].iter().map(|t| (t.rotation, t.translation)).collect::<Vec<_>>());
            let pose = clip.sample_frame(f).unwrap();
            let w2 = world_transforms(
                &dm.nodes
                    .iter()
                    .zip(&pose.nodes)
                    .map(|(x, t)| {
                        (
                            x.wire.parent_index,
                            if t.has_rotation { t.rotation } else { x.wire.orientation },
                            if t.has_translation { t.translation } else { x.wire.translation },
                        )
                    })
                    .collect::<Vec<_>>(),
            );
            let (s1, s2) = (skin_h1(&w1), skin_h2(&w2));
            let (mut err, mut travel) = (0f32, 0f32);
            for (v, p2, nodes) in &pairs {
                let a1 = skin(&s1, v.position, &h1_nodes(v));
                let b = skin(&s2, *p2, nodes);
                err = err.max(dist(a1, b));
                travel = travel.max(dist(a1, v.position));
            }
            out.posed.push((a.name.clone(), f, err, travel));
        }
    }
    out
}

/// Every biped and first-person model on a10 and Blood Gulch, converted,
/// decoded and skinned in the bind pose and in sampled animation frames,
/// against Halo 1's own vertices skinned by Halo 1's rule and its own
/// cached inverse frames.
///
/// `H1_ISO=disc/halo1.iso cargo test -p h1models --release -- --ignored h1_disc_bipeds_skin --nocapture`
#[test]
#[ignore = "needs $H1_ISO"]
fn h1_disc_bipeds_skin() {
    let mut failures = Vec::new();
    for stem in ["a10", "bloodgulch"] {
        let Some(map) = h1_disc_map(stem) else {
            return;
        };
        let models: Vec<&h1map::Tag> = map
            .tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"mode") && t.name.starts_with("characters\\"))
            .collect();
        for t in models {
            let model = h1::Model::read(&map, t.data_address).unwrap();
            let names: Vec<&str> = model.nodes.iter().map(|x| x.name.as_str()).collect();
            // Its graph: the first unit graph with this skeleton.
            let graph = map.tags().iter().find(|g| {
                g.group == h1map::Group::new(b"antr")
                    && h1::Graph::read(&map, g.data_address)
                        .is_ok_and(|hg| hg.nodes.iter().map(|x| x.name.as_str()).eq(names.iter().copied()))
            });
            let r = skin_round_trip(&map, t, graph);
            println!(
                "{stem} {} ({}): {} vertices; H1 rule vs H1 inverse {:.1e}, our inverse vs H1's {:.1e}; bind max {:.2e}, radius ratio {:.3}",
                t.name,
                graph.map_or("no graph", |g| g.name.as_str()),
                r.vertices,
                r.h1_inverse,
                r.our_inverse,
                r.bind,
                r.bind_radius
            );
            let worst = r.posed.iter().map(|p| p.2).fold(0f32, f32::max);
            for (name, f, e, travel) in &r.posed {
                println!("    {name} frame {f}: max error {e:.2e} (vertices travel up to {travel:.3})");
            }
            if r.h1_inverse > 1e-3 || r.our_inverse > 1e-3 || r.bind > 1e-3 || r.bind_radius > 1.001 || worst > 2e-3 {
                failures.push(t.name.clone());
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

/// Halo 1's globals name the first-person arms at `matg +0x17c` (a block
/// of one element), element `+0x0` (16-byte reference, datum at `+0xc`):
/// `characters\cyborg\fp\fp` on a10 and Blood Gulch
/// (`h1convert::globals::h1_first_person_hands` reads it).
#[test]
#[ignore = "needs $H1_ISO"]
fn h1_disc_globals_name_the_first_person_arms() {
    for stem in ["a10", "bloodgulch"] {
        let Some(map) = h1_disc_map(stem) else {
            return;
        };
        let matg = map.tags().iter().find(|t| t.group == h1map::Group::new(b"matg")).unwrap();
        let root = map.tag_data(matg).unwrap();
        assert_eq!(h1::u32_at(root, 0x17c), 1, "{stem}");
        let e = map.read(h1::u32_at(root, 0x180), 16).unwrap();
        assert_eq!(&h1::u32_at(e, 0).to_be_bytes(), b"mode");
        let arms = map.tag(h1map::TagId(h1::u32_at(e, 0xc))).unwrap();
        println!("{stem}: first-person arms {}", arms.name);
        assert_eq!(arms.name, "characters\\cyborg\\fp\\fp");
    }
}

/// First person, posed: Halo 1's arms and a weapon's first-person model,
/// both converted, animated by the weapon's converted first-person graph.
/// Each model node is bound to the graph node of the same name, as the
/// engine does (E `0x001dd4c0`: per `mode` node, the `jmad` node whose
/// string-ID name is equal, -1 for none; `0x001676d7` uses the table for
/// the arms and the weapon model alike). The graph frame is composed along
/// the graph's own tree, the Halo 2 way from the decoded `jmad`, and the
/// Halo 1 way from Halo 1's frame; each model's vertices are skinned with
/// its stored (Halo 2) or cached (Halo 1) inverse frames.
///
/// `H1_ISO=disc/halo1.iso cargo test -p h1models --release -- --ignored h1_disc_first_person_skin --nocapture`
#[test]
#[ignore = "needs $H1_ISO"]
fn h1_disc_first_person_skin() {
    use crate::tests::{Raw, build};
    let Some(map) = h1_disc_map("bloodgulch") else {
        return;
    };
    let find = |g: &[u8; 4], n: &str| map.find(h1map::Group::new(g), n).unwrap_or_else(|| panic!("{n}"));
    let arms = find(b"mode", "characters\\cyborg\\fp\\fp");
    let mut worst = 0f32;
    for weapon in ["assault rifle", "pistol", "plasma rifle", "needler", "rocket launcher"] {
        let path = format!("weapons\\{weapon}\\fp\\fp");
        let (gun_tag, graph_tag) = (find(b"mode", &path), find(b"antr", &path));
        let (arms_src, arms_conv) = crate::model_from_map(&map, arms, &Default::default()).unwrap();
        let (gun_src, gun_conv) = crate::model_from_map(&map, gun_tag, &Default::default()).unwrap();
        let (hg, _, graph_conv) = crate::graph_from_map(&map, graph_tag, None).unwrap();
        let (bytes, ids) = build(vec![(arms_conv, "fp\\arms"), (gun_conv, "fp\\gun"), (graph_conv, "fp\\graph")]);
        let mut m2 = map::Map::from_reader(std::io::Cursor::new(bytes.clone())).unwrap();
        let g2 = tags::anim::AnimationGraph::read(&mut m2, ids[2]).unwrap();
        let strings = m2.strings().to_vec();
        let graph_names: Vec<String> = g2
            .nodes
            .iter()
            .map(|n| n.name.resolve(&strings).ok().flatten().unwrap_or("").to_owned())
            .collect();
        let graph_parents: Vec<i16> = g2.nodes.iter().map(|n| n.parent).collect();
        let resource = g2.read_resource(0, &mut Raw(bytes.clone())).unwrap();
        // Per model: (H1 model, decoded model, stored inverse, H1 cached
        // inverse, model node -> graph node).
        let mut models = Vec::new();
        for (src, id, tag) in [(&arms_src, ids[0], arms), (&gun_src, ids[1], gun_tag)] {
            let dm = tags::model::RenderModelGeometry::read(&mut m2, id, &mut Raw(bytes.clone())).unwrap();
            let table: Vec<usize> = dm
                .nodes
                .iter()
                .map(|n| {
                    let name = n.name.as_deref().unwrap_or("");
                    graph_names.iter().position(|g| g == name).unwrap_or_else(|| panic!("{path}: {name} not in the graph"))
                })
                .collect();
            let inverse = h2_inverse(&mut m2, id);
            let cached = h1_cached_inverse(&map, tag.data_address, src.nodes.len());
            models.push((src, dm, inverse, cached, table));
        }
        let picks: Vec<usize> = (0..hg.animations.len())
            .filter(|&i| hg.animations[i].kind == h1::AnimType::Base)
            .collect();
        let mut line = Vec::new();
        for &ai in picks.iter().step_by((picks.len() / 5).max(1)) {
            let a = &hg.animations[ai];
            let f = usize::from(a.frame_count) / 2;
            let w1 = h1_world(&graph_parents, &a.frames[f].iter().map(|t| (t.rotation, t.translation)).collect::<Vec<_>>());
            let pose = g2.clip_from_shared(ai, &resource).unwrap().sample_frame(f).unwrap();
            assert!(pose.nodes.iter().all(|t| t.has_rotation && t.has_translation), "{}", a.name);
            let w2 = world_transforms(
                &graph_parents
                    .iter()
                    .zip(&pose.nodes)
                    .map(|(&p, t)| (p, t.rotation, t.translation))
                    .collect::<Vec<_>>(),
            );
            let mut err = 0f32;
            for (src, dm, inverse, cached, table) in &models {
                let s1: Vec<Mat> = table.iter().zip(cached.iter()).map(|(&g, i)| mul(&w1[g], i)).collect();
                let s2: Vec<Mat> = table.iter().zip(inverse.iter()).map(|(&g, i)| mul(&w2[g], i)).collect();
                for (s, geo) in dm.sections.iter().zip(&src.geometries) {
                    let vs = geo.parts.iter().flat_map(|p| &p.vertices);
                    for (i, v) in vs.enumerate() {
                        let ix = s.vertices.node_indices[i];
                        let w = s.vertices.node_weights[i];
                        let nodes: Vec<(usize, f32)> = (0..4)
                            .filter(|&k| w[k] > 0.0)
                            .map(|k| {
                                let l = usize::from(ix[k]);
                                (s.node_map.get(l).map_or(l, |&g| usize::from(g)), w[k])
                            })
                            .collect();
                        let a1 = skin(&s1, v.position, &h1_nodes(v));
                        let b = skin(&s2, s.vertices.positions[i], &nodes);
                        err = err.max(dist(a1, b));
                    }
                }
            }
            worst = worst.max(err);
            line.push(format!("{} {err:.1e}", a.name));
        }
        println!("{path}: arms {} + weapon {} nodes in a {}-node graph; {}", arms_src.nodes.len(), gun_src.nodes.len(), graph_names.len(), line.join(", "));
    }
    println!("worst {worst:.2e}");
    assert!(worst < 2e-3, "{worst}");
}


/// The inverse rest frame each node of a `mode` stores: `+0x2c`, `+0x38`,
/// `+0x44` its forward, left and up vectors (the matrix's columns), `+0x50`
/// its position. Every retail node's frame inverts its model-space rest
/// transform read that way (`retail_h2_bipeds_pose_sanely`).
fn h2_inverse<R: std::io::Read + std::io::Seek>(m2: &mut map::Map<R>, id: map::TagId) -> Vec<Mat> {
    let tag = m2.read_tag_bytes(id).unwrap();
    let base = m2.tag(id).unwrap().location.unwrap().address;
    let n = h1::u32_at(&tag, 0x48) as usize;
    let nodes_at = (h1::u32_at(&tag, 0x4c) - base) as usize;
    (0..n)
        .map(|i| {
            let e = &tag[nodes_at + i * 0x60..];
            let r = std::array::from_fn(|row| std::array::from_fn(|c| h1::f32_at(e, 0x2c + 12 * c + 4 * row)));
            (r, h1::f32s(e, 0x50))
        })
        .collect()
}

/// How much a pose stretches the mesh: each triangle edge's skinned
/// length over its bind length, `(median, 99th percentile, max)`. One in
/// the bind pose; near one in any sane pose (skin moves with its bones),
/// far from it when node frames and inverse frames disagree.
fn edge_stretch(
    dm: &tags::model::RenderModelGeometry,
    inverse: &[Mat],
    pose: Option<&tags::anim_sample::Pose>,
    flip: bool,
) -> (f32, f32, f32) {
    let world = world_transforms(
        &dm.nodes
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let t = pose.map(|p| &p.nodes[i]);
                let r = match t {
                    Some(t) if t.has_rotation => {
                        if flip { conj(t.rotation) } else { t.rotation }
                    }
                    _ => x.wire.orientation,
                };
                let p = match t {
                    Some(t) if t.has_translation => t.translation,
                    _ => x.wire.translation,
                };
                (x.wire.parent_index, r, p)
            })
            .collect::<Vec<_>>(),
    );
    let skin_m: Vec<Mat> = world.iter().zip(inverse).map(|(w, i)| mul(w, i)).collect();
    let mut ratios = Vec::new();
    for s in &dm.sections {
        let v = &s.vertices;
        let posed: Vec<[f32; 3]> = (0..v.positions.len())
            .map(|i| {
                let ix = v.node_indices.get(i).copied().unwrap_or([0; 4]);
                let w = v.node_weights.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]);
                let nodes: Vec<(usize, f32)> = (0..4)
                    .filter(|&k| w[k] > 0.0)
                    .map(|k| {
                        let l = usize::from(ix[k]);
                        (s.node_map.get(l).map_or(l, |&g| usize::from(g)), w[k])
                    })
                    .collect();
                skin(&skin_m, v.positions[i], &nodes)
            })
            .collect();
        for part in &s.geometry.parts {
            let idx = &s.geometry.indices[part.index_range()];
            let tris: Vec<[u16; 3]> = if part.is_triangle_list() {
                idx.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect()
            } else {
                idx.windows(3).map(|t| [t[0], t[1], t[2]]).collect()
            };
            for t in tris {
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    let (a, b) = (usize::from(a), usize::from(b));
                    if a >= posed.len() || b >= posed.len() {
                        continue;
                    }
                    let bind = dist(v.positions[a], v.positions[b]);
                    if bind > 1e-3 {
                        ratios.push(dist(posed[a], posed[b]) / bind);
                    }
                }
            }
        }
    }
    ratios.sort_by(f32::total_cmp);
    let at = |q: f32| ratios.get(((ratios.len() as f32 - 1.0) * q) as usize).copied().unwrap_or(0.0);
    (at(0.5), at(0.99), at(1.0))
}

/// The control for the posed round trips: retail Halo 2 bipeds, their
/// stored inverse frames read by [`h2_inverse`], and their own retail
/// animations decoded by `crates/formats/tags` and composed as the converted ones
/// are. The rest pose times the stored inverse is the identity on every
/// node, and posed frames stretch the mesh's edges little (median 1, 99th
/// percentile under 3), less than the same frames with conjugated
/// rotations do.
///
/// `H2_DISC=disc/halo2.iso cargo test -p h1models --release -- --ignored retail_h2_bipeds_pose_sanely --nocapture`
#[test]
#[ignore = "needs $H2_DISC"]
fn retail_h2_bipeds_pose_sanely() {
    let Some(mut disc) = h2_disc("05a_deltaapproach") else {
        return;
    };
    let mut map = map::Map::from_reader(std::io::Cursor::new(disc.local.clone())).unwrap();
    let strings = map.strings().to_vec();
    let mut checked = 0;
    for name in ["objects\\characters\\marine\\marine", "objects\\characters\\elite\\elite", "objects\\characters\\grunt\\grunt"] {
        let find = |g: &[u8; 4]| map.tags().iter().find(|t| t.group.0 == *g && t.name.as_deref() == Some(name)).map(|t| t.id);
        let (Some(mid), Some(jid)) = (find(b"mode"), find(b"jmad")) else {
            println!("{name}: not on 05a");
            continue;
        };
        let dm = match tags::model::RenderModelGeometry::read(&mut map, mid, &mut disc) {
            Ok(m) => m,
            Err(e) => {
                println!("{name}: {e}");
                continue;
            }
        };
        let inverse = h2_inverse(&mut map, mid);
        let mut rest_error = 0f32;
        for (w, inv) in h2_rest(&dm).iter().zip(&inverse) {
            let id = mul(w, inv);
            for i in 0..3 {
                for j in 0..3 {
                    rest_error = rest_error.max((id.0[i][j] - f32::from(u8::from(i == j))).abs());
                }
                rest_error = rest_error.max(id.1[i].abs());
            }
        }
        let bind = edge_stretch(&dm, &inverse, None, false);
        println!("{name}: rest times inverse {rest_error:.1e}; bind stretch {bind:?}");
        assert!(rest_error < 1e-5, "{name}: {rest_error}");
        assert!((bind.2 - 1.0).abs() < 1e-3, "{name}: {bind:?}");
        let g = tags::anim::AnimationGraph::read(&mut map, jid).unwrap();
        let mut shown = 0;
        for (ai, a) in g.animations.iter().enumerate() {
            if a.anim_type != tags::anim::AnimationType::Base || shown >= 8 || ai % 7 != 0 {
                continue;
            }
            let Ok(clip) = g.load_clip(ai, &mut disc) else { continue };
            if clip.is_aim_grid() {
                continue;
            }
            let f = (a.frame_count as usize) / 2;
            let pose = clip.sample_frame(f).unwrap();
            let an = a.name.resolve(&strings).ok().flatten().unwrap_or("?");
            let (ok, flipped) = (
                edge_stretch(&dm, &inverse, Some(&pose), false),
                edge_stretch(&dm, &inverse, Some(&pose), true),
            );
            println!("  {an} frame {f}: stretch {ok:?}; with conjugated rotations {flipped:?}");
            assert!((ok.0 - 1.0).abs() < 0.01 && ok.1 < 3.0 && ok.1 < flipped.1, "{name} {an}: {ok:?} {flipped:?}");
            shown += 1;
        }
        checked += 1;
    }
    assert!(checked > 0);
}
