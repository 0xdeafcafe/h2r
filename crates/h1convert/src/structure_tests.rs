//! The converted structure BSP and lightmap, written into an invented map
//! and read back through Halo 2's own decoders (`tags::structure`,
//! `tags::collision`, `map::Map`). Invented geometry only.

use std::io::Cursor;

use map::build::{Block, BspField, FixupKind, TagPayload, TagSet};
use map::{CacheType, FourCC, Map, RawPointer, TagId};
use tags::geometry::RawSource;
use tags::structure::{Lightmap, StructureRender};

use crate::convert::{Sandbox, known, map_name, scenario_path, structure_mopp};
use crate::structure::*;
use crate::tests::room;

/// Test only: a MOPP block that loads but emits nothing (0x30-byte header,
/// then `00`, return). Never written into a map for play; it lets the rest
/// of the structure be checked while the MOPP writer is elsewhere.
fn stand_in_mopp() -> Vec<u8> {
    let mut m = vec![0u8; 0x31];
    m[0x20] = 1;
    m
}

/// A packed 11:11:10 +z normal (frame: normal, binormal, tangent).
const UP: u32 = 511 << 22;

fn vertex(p: [f32; 3]) -> Vertex {
    Vertex {
        position: p,
        frame: [UP, 0, 0],
        texcoord: [p[0], p[1]],
        lightmap: [0.25, 0.75],
    }
}

/// Two meshes: a unit square (two triangles) with shader "a" and one
/// triangle above it with shader "b".
fn meshes() -> Vec<Mesh> {
    vec![
        Mesh {
            shader: "a".into(),
            page: None,
            vertices: [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ]
            .map(vertex)
            .to_vec(),
            triangles: vec![[0, 1, 2], [0, 2, 3]],
        },
        Mesh {
            shader: "b".into(),
            page: None,
            vertices: [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]]
                .map(vertex)
                .to_vec(),
            triangles: vec![[0, 1, 2]],
        },
    ]
}

fn templates() -> Templates {
    let mut collision_material = [0u8; COLLISION_MATERIAL];
    collision_material[8..10].copy_from_slice(&3u16.to_le_bytes());
    collision_material[0xc..0x14].fill(0xff);
    let mut material_head = [0u8; 16];
    material_head[8..16].fill(0xff);
    let mut second = [0u8; 16];
    second[8..16].fill(0xee);
    Templates {
        collision_material,
        material_heads: vec![material_head, second],
    }
}

struct Mem(Vec<u8>);
impl RawSource for Mem {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        let s = p.offset as usize;
        self.0
            .get(s..s + size as usize)
            .map(<[u8]>::to_vec)
            .ok_or(tags::Error::Invalid("test raw range"))
    }
}

struct Built {
    bytes: Vec<u8>,
    sbsp: TagId,
    ltmp: TagId,
    collision: tags::collision::Bsp,
}

fn build(sections: &[Section]) -> Built {
    build_lit(sections, false)
}

/// [`build`], with a lightmap bitmap tag the lightmap names when `lit`.
fn build_lit(sections: &[Section], lit: bool) -> Built {
    let mut set = TagSet::new(CacheType::Multiplayer, "t", "invented\\t");
    for g in [b"matg", b"scnr", b"sbsp", b"ltmp"] {
        set.add_group(FourCC(*g), None, None);
    }
    set.strings = vec![Some(String::new())];
    let globals = set
        .add_tag(
            FourCC(*b"matg"),
            "globals\\globals",
            TagPayload::Built(Block::zeroed(0x40)),
        )
        .unwrap();
    let scenario = set
        .add_tag(FourCC(*b"scnr"), "invented\\t", TagPayload::Empty)
        .unwrap();
    let sbsp = set
        .add_tag(FourCC(*b"sbsp"), "invented\\t", TagPayload::Empty)
        .unwrap();
    let ltmp = set
        .add_tag(FourCC(*b"ltmp"), "invented\\t", TagPayload::Empty)
        .unwrap();
    let lightmap_bitmap = if lit {
        set.add_group(FourCC(*b"bitm"), None, None);
        set.add_tag(
            FourCC(*b"bitm"),
            "invented\\lightmap",
            TagPayload::Built(Block::zeroed(0x4c)),
        )
        .unwrap()
    } else {
        TagId::NULL
    };
    let collision = crate::collision::convert(&room()).unwrap();
    let t = templates();
    let mopp = stand_in_mopp();
    let s = Structure {
        collision: &collision,
        collision_materials: 2,
        sections,
        materials: &[0, 1],
        bounds: [-1.0, 2.0, -1.0, 2.0, -1.0, 2.0],
        mopp: &mopp,
        checksum: 0x1234_5678,
        templates: &t,
        lightmap_bitmap,
    };
    let section = s.write(&mut set, sbsp, ltmp).unwrap();
    let mut descriptor = Block::zeroed(0x44);
    for (at, field) in [
        (0, BspField::FileOffset),
        (4, BspField::Size),
        (8, BspField::Address),
    ] {
        descriptor.fixup(at, FixupKind::Bsp { section, field });
    }
    descriptor.tag_id(0x14, sbsp).tag_id(0x1c, ltmp);
    let mut scen = Block::zeroed(0x3e0);
    scen.block(0x210, 1, descriptor);
    set.tag_mut(scenario).unwrap().payload = TagPayload::Built(scen);
    set.scenario = scenario;
    set.globals = globals;
    let (bytes, _) = set.build().unwrap();
    Built {
        bytes,
        sbsp,
        ltmp,
        collision,
    }
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

#[test]
fn a_converted_structure_reads_back_through_halo_2s_decoders() {
    let m = meshes();
    let sections = pack(&m, &[0, 1]);
    assert_eq!(sections.len(), 1);
    let b = build(&sections);
    let mut map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    map.validate_checksum().unwrap();

    // Render geometry: one cluster, two parts, every vertex in place.
    let render = StructureRender::read(&mut map, b.sbsp).unwrap();
    assert_eq!(render.checksum, 0x1234_5678);
    assert_eq!(render.material_count, 2);
    assert_eq!(render.clusters.len(), 1);
    let c = &render.clusters[0];
    assert_eq!(c.bounds, [0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
    let mut raw = Mem(b.bytes.clone());
    let s = c.geometry.load(&mut raw, Some(2)).unwrap().unwrap();
    assert_eq!(s.parts.len(), 2);
    assert!(s.parts.iter().all(|p| p.is_drawn() && p.is_triangle_list()));
    assert_eq!(
        s.parts
            .iter()
            .map(|p| (p.material, p.index_count))
            .collect::<Vec<_>>(),
        [(0, 6), (1, 3)]
    );
    let v = s.vertices(None).unwrap();
    let want: Vec<[f32; 3]> = m
        .iter()
        .flat_map(|m| m.vertices.iter().map(|v| v.position))
        .collect();
    assert_eq!(v.positions, want);
    assert_eq!(v.texcoords[2], [1.0, 1.0]);
    assert!((v.normals[0][2] - 1.0).abs() < 1e-3);
    assert!((v.lightmap_texcoords[0][1] - 0.75).abs() < 1e-3);
    let tris: Vec<[u16; 3]> = s.parts.iter().flat_map(|p| s.part_triangles(p)).collect();
    assert_eq!(tris, [[0, 1, 2], [0, 2, 3], [4, 5, 6]]);

    // Cluster fields the engine reads (docs/game/h1-multiplayer.md).
    assert_eq!(c.raw[0x6e], 0, "the scenario's first sky");
    assert_eq!(&c.raw[0x6f..0x72], [0xff; 3]);
    assert_eq!(&c.raw[0x72..0x78], [0xff; 6]);

    // Lightmap: one group, the same checksum, no bitmap, a render info
    // entry per cluster.
    let light = Lightmap::read(&mut map, b.ltmp).unwrap();
    assert_eq!(light.groups.len(), 1);
    let g = &light.groups[0];
    assert_eq!(g.structure_checksum, 0x1234_5678);
    assert_eq!(g.bitmap, None);
    assert_eq!(g.cluster_render_info.len(), 1);
    assert!(g.buckets.is_empty());

    // The root: collision, leaves, visibility, world box and the MOPP block.
    let tag = map.tag(b.sbsp).unwrap().clone();
    let at = tag.location.unwrap().address;
    let p = map.read_tag_bytes(b.sbsp).unwrap();
    let off = |a: u32| (a - at) as usize;
    assert_eq!(word(&p, 0x14), 1);
    let col = tags::collision::Bsp::decode_blocks(&p, at, off(word(&p, 0x18))).unwrap();
    assert_eq!(col, b.collision);
    assert_eq!(word(&p, 0x0c), 2, "collision materials");
    let cm = off(word(&p, 0x10));
    assert_eq!(&p[cm + 8..cm + 12], [3, 0, 0xff, 0xff]);
    assert_eq!(word(&p, 0x2c) as usize, b.collision.leaves.len());
    let leaves = off(word(&p, 0x30));
    assert!(
        (0..b.collision.leaves.len()).all(|i| p[leaves + 8 * i..leaves + 8 * i + 8] == [0; 8]),
        "every leaf in cluster 0"
    );
    assert_eq!(word(&p, off(word(&p, 0x58))), 1, "cluster 0 sees itself");
    assert_eq!(f32::from_bits(word(&p, 0x38)), 2.0, "x max");
    assert_eq!(f32::from_bits(word(&p, 0x1f8)), 2.0, "Havok box x max");
    let mopp = word(&p, 0x1e4);
    assert_eq!(mopp % 16, 0);
    assert_eq!(word(&p, 0x1e0), 0x31);
    assert_eq!(p[off(mopp)..off(mopp) + 0x31], stand_in_mopp()[..]);
    for (count, address) in [(0x21c, 0x220), (0x234, 0x238)] {
        assert_eq!(word(&p, count), 1);
        assert_ne!(word(&p, address), 0);
    }
    // Each material: the template's head, an empty block, 0xff at +0x1c.
    let mats = off(word(&p, 0xa8));
    assert_eq!(&p[mats + 8..mats + 16], [0xff; 8]);
    assert_eq!(
        &p[mats + 0x28..mats + 0x30],
        [0xee; 8],
        "material 1 from base material 1"
    );
    assert_eq!(word(&p, mats + 0x10), 0);
    assert_eq!(p[mats + 0x1c], 0xff);
}

#[test]
fn large_geometry_splits_into_clusters_that_see_each_other() {
    // 30,000 separate triangles: 90,000 vertices, more than one section.
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for i in 0..30_000u32 {
        let x = i as f32;
        for p in [[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]] {
            vertices.push(vertex(p));
        }
        triangles.push([3 * i, 3 * i + 1, 3 * i + 2]);
    }
    let mesh = Mesh {
        shader: "a".into(),
        page: None,
        vertices,
        triangles,
    };
    let sections = pack(std::slice::from_ref(&mesh), &[0]);
    assert_eq!(sections.len(), 2);
    for s in &sections {
        assert!(s.vertices.len() <= SECTION_MAX && s.indices.len() <= SECTION_MAX);
        let tiled: usize = s.parts.iter().map(|p| usize::from(p.count)).sum();
        assert_eq!(tiled, s.indices.len());
    }
    // Every triangle once, in order.
    let back: Vec<[f32; 3]> = sections
        .iter()
        .flat_map(|s| {
            s.indices
                .iter()
                .map(|&i| s.vertices[usize::from(i)].position)
        })
        .collect();
    let want: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.position).collect();
    assert_eq!(back, want);

    let b = build(&sections);
    let mut map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    let render = StructureRender::read(&mut map, b.sbsp).unwrap();
    assert_eq!(render.clusters.len(), 2);
    let mut raw = Mem(b.bytes.clone());
    let loaded: usize = render
        .clusters
        .iter()
        .map(|c| {
            c.geometry
                .load(&mut raw, Some(2))
                .unwrap()
                .unwrap()
                .indices
                .len()
        })
        .sum();
    assert_eq!(loaded, 90_000);
    let light = Lightmap::read(&mut map, b.ltmp).unwrap();
    assert_eq!(light.groups[0].cluster_render_info.len(), 2);
    // Visibility: two rows of one word, both clusters set in each.
    let at = map.tag(b.sbsp).unwrap().location.unwrap().address;
    let p = map.read_tag_bytes(b.sbsp).unwrap();
    let vis = (word(&p, 0x58) - at) as usize;
    assert_eq!(word(&p, 0x54), 8);
    assert_eq!([word(&p, vis), word(&p, vis + 4)], [3, 3]);
    // Two clusters: one pair, near (distance byte 0) and not deafened.
    check_audibility(&p, at).unwrap();
    let e = (word(&p, 0x228) - at) as usize;
    assert_eq!(word(&p, e + 0x24), 1);
    assert_eq!(p[(word(&p, e + 0x28) - at) as usize], 0);
}

/// The crash of the first converted Blood Gulch (16 clusters): the engine
/// compares clusters every tick through `sbsp+0x224`, and without it reads
/// the null sound PAS data at `+0xe8` (E `0x00249f03`). Every retail map
/// has one element, no doors, a distance byte and a deafening bit per pair.
#[test]
fn every_cluster_pair_has_an_audibility_distance() {
    for n in [1, 2, 16, 47] {
        let sections = vec![Section::default(); n];
        let b = build(&sections);
        let mut map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
        let at = map.tag(b.sbsp).unwrap().location.unwrap().address;
        let p = map.read_tag_bytes(b.sbsp).unwrap();
        assert_eq!(word(&p, 0x9c) as usize, n);
        check_audibility(&p, at).unwrap_or_else(|e| panic!("{n} clusters: {e}"));
        assert_eq!(word(&p, 0x224), 1, "{n} clusters");
        let e = (word(&p, 0x228) - at) as usize;
        assert_eq!(word(&p, e), 0, "no doors");
        let pairs = n * (n - 1) / 2;
        assert_eq!(word(&p, e + 0x24) as usize, pairs);
        assert_eq!(word(&p, e + 0x1c) as usize, pairs.div_ceil(32));
        if pairs > 0 {
            let d = (word(&p, e + 0x28) - at) as usize;
            assert!(p[d..d + pairs].iter().all(|&x| x == 0), "every pair near");
            let w = (word(&p, e + 0x20) - at) as usize;
            assert!(p[w..w + 4 * pairs.div_ceil(32)].iter().all(|&x| x == 0));
        }
        // The engine's own index for the last pair lands in the table.
        if n > 1 {
            let (i, j) = (n - 2, n - 1);
            assert!((n - 1) * i - i * (i + 1) / 2 + j - 1 < pairs);
        }
    }
    // Without the element the check fails, as the engine would.
    let mut p = vec![0u8; 0x240];
    p[0x9c..0xa0].copy_from_slice(&16u32.to_le_bytes());
    assert!(check_audibility(&p, 0).is_err());
}

#[test]
fn winding_is_turned_to_agree_with_the_normals() {
    let mut m = meshes();
    assert!(!fix_winding(&mut m), "counter-clockwise about +z already");
    for mesh in &mut m {
        for t in &mut mesh.triangles {
            t.swap(1, 2);
        }
    }
    assert!(fix_winding(&mut m));
    assert_eq!(m[0].triangles[0], [0, 1, 2]);
}

#[test]
fn the_structure_gets_valid_mopp_code_keying_every_surface() {
    let collision = crate::collision::convert(&room()).unwrap();
    let code = structure_mopp(&collision).unwrap();
    let code = mopp::code::Code::parse(&code).unwrap();
    assert!(mopp::check::validate(&code.bytecode).is_ok());
    let mut keys = mopp::bounds::key_bounds(&code)
        .unwrap()
        .into_keys()
        .collect::<Vec<_>>();
    keys.sort();
    let want: Vec<u32> = (0..collision.surfaces.len() as u32)
        .map(|i| 0x2000_0000 | i)
        .collect();
    assert_eq!(keys, want);
}

#[test]
fn names_and_known_maps() {
    assert_eq!(map_name("BeaverCreek"), "h1_beavercreek");
    assert_eq!(
        scenario_path("beavercreek"),
        "scenarios\\multi\\h1_beavercreek\\h1_beavercreek"
    );
    assert_eq!(known("beavercreek"), Some(("Battle Creek", "beavercreek")));
    assert_eq!(known("icefields").map(|k| k.1), Some("coagulation"));
    assert_eq!(known("bloodgulch").map(|k| k.1), Some("coagulation"));
    assert_eq!(known("nowhere"), None);
    assert_eq!(Sandbox::Halo2.name(), "h2");
}

#[test]
fn trigger_volumes_are_parked_out_of_the_level() {
    use crate::convert::{PARKED, TRIGGER_VOLUME, park_trigger_volumes};
    let address = 0x8000_0000u32;
    let mut scnr = vec![0x11u8; 0x400 + 2 * TRIGGER_VOLUME];
    scnr[0x108..0x10c].copy_from_slice(&2u32.to_le_bytes());
    scnr[0x10c..0x110].copy_from_slice(&(address + 0x400).to_le_bytes());
    assert_eq!(park_trigger_volumes(&mut scnr, address).unwrap(), 2);
    for i in 0..2 {
        let e = &scnr[0x400 + i * TRIGGER_VOLUME..][..TRIGGER_VOLUME];
        let origin: Vec<f32> = (0..3)
            .map(|k| f32::from_le_bytes(e[0x24 + 4 * k..0x28 + 4 * k].try_into().unwrap()))
            .collect();
        assert_eq!(origin, PARKED);
        assert!(
            e[..0x24].iter().chain(&e[0x30..]).all(|&b| b == 0x11),
            "nothing else moves"
        );
    }
    // Outside the tag: refused, not written.
    scnr[0x108..0x10c].copy_from_slice(&3u32.to_le_bytes());
    assert!(park_trigger_volumes(&mut scnr, address).is_err());
    scnr[0x108..0x10c].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(park_trigger_volumes(&mut scnr, address).unwrap(), 0);
}

#[test]
fn each_lightmap_page_gets_its_own_cluster_and_render_info() {
    // Mesh "a" on page 1, "b" on page 0, and an unlit copy of "b".
    let mut m = meshes();
    m[0].page = Some(1);
    m[1].page = Some(0);
    let mut unlit = m[1].clone();
    unlit.page = None;
    m.push(unlit);
    let sections = pack(&m, &[0, 1, 1]);
    let pages: Vec<_> = sections.iter().map(|s| s.page).collect();
    assert_eq!(pages, [Some(0), Some(1), None], "page order, unlit last");
    assert_eq!(sections[1].indices.len(), 6, "mesh a");

    let b = build_lit(&sections, true);
    let mut map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    let light = Lightmap::read(&mut map, b.ltmp).unwrap();
    let g = &light.groups[0];
    let bitmap = map
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"bitm"))
        .unwrap()
        .id;
    assert_eq!(g.bitmap, Some(bitmap));
    let info: Vec<_> = g
        .cluster_render_info
        .iter()
        .map(|r| (r.bitmap_index, r.palette_index))
        .collect();
    assert_eq!(info, [(0, -1), (1, -1), (-1, -1)]);

    // Without a bitmap every cluster is unlit, whatever its page.
    let b = build(&sections);
    let mut map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    let g = &Lightmap::read(&mut map, b.ltmp).unwrap().groups[0];
    assert_eq!(g.bitmap, None);
    assert!(g.cluster_render_info.iter().all(|r| r.bitmap_index == -1));
}
