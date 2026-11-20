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
        incident: UP,
    }
}

/// Two meshes: a unit square (two triangles) with shader "a" and one
/// triangle above it with shader "b".
fn meshes() -> Vec<Mesh> {
    vec![
        Mesh {
            shader: "a".into(),
            page: None,
            cluster: 0,
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
            cluster: 1,
            vertices: [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]]
                .map(vertex)
                .to_vec(),
            triangles: vec![[0, 1, 2]],
        },
    ]
}

fn templates(set: &mut TagSet) -> Templates {
    let mut collision_material = [0u8; COLLISION_MATERIAL];
    collision_material[8..10].copy_from_slice(&3u16.to_le_bytes());
    collision_material[0xa..0xc].fill(0xff);
    collision_material[0xc..0x14].fill(0xff);
    let group = FourCC(*b"shad");
    set.add_group(group, None, None);
    let material_heads = ["a", "b"].into_iter().map(|name| {
        let shader = set.add_tag(group, name, TagPayload::Built(Block::zeroed(0x80))).unwrap();
        let mut head = [0u8; 16];
        head[8..12].copy_from_slice(&group.as_u32().to_le_bytes());
        head[12..16].copy_from_slice(&shader.0.to_le_bytes());
        head
    }).collect();
    Templates {
        collision_materials: vec![collision_material; 2],
        material_heads,
        part_kinds: vec![PART_KIND, PART_KIND_TRANSPARENT],
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
    build_with(sections, lit, false)
}

/// [`build_lit`]; with `sounds`, two looping sounds and a sound
/// environment, cluster `c` playing background `c % 2` (environment 0 on
/// even clusters, none on odd), and leaf `k` in cluster `k % clusters`.
fn build_with(sections: &[Section], lit: bool, sounds: bool) -> Built {
    build_full(
        sections,
        lit,
        sounds,
        &crate::clusters::Visibility::default(),
        &[],
    )
}

/// [`build_with`], with `visibility`'s portals.
fn build_full(
    sections: &[Section],
    lit: bool,
    sounds: bool,
    visibility: &crate::clusters::Visibility,
    atmosphere: &[u8],
) -> Built {
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
        let mut lightmap = Block::zeroed(0x4c);
        lightmap.block(0x44, 1, Block::zeroed(0x74));
        set.add_tag(
            FourCC(*b"bitm"),
            "invented\\lightmap",
            TagPayload::Built(lightmap),
        )
        .unwrap()
    } else {
        TagId::NULL
    };
    let collision = crate::collision::convert(&room()).unwrap();
    let t = templates(&mut set);
    let mopp = stand_in_mopp();
    let mut sound = Sound::default();
    if sounds {
        for g in [b"lsnd", b"snde"] {
            set.add_group(FourCC(*g), None, None);
        }
        for k in 0..2 {
            let id = set
                .add_tag(
                    FourCC(*b"lsnd"),
                    &format!("invented\\loop{k}"),
                    TagPayload::Built(Block::zeroed(0x40)),
                )
                .unwrap();
            sound.backgrounds.push((format!("loop{k}"), id));
        }
        let id = set
            .add_tag(
                FourCC(*b"snde"),
                "invented\\room",
                TagPayload::Built(Block::zeroed(0x48)),
            )
            .unwrap();
        sound.environments.push(("room".into(), id));
        let n = sections.len().max(1);
        sound.clusters = (0..n)
            .map(|c| [(c % 2) as i16, if c % 2 == 0 { 0 } else { -1 }])
            .collect();
        sound.leaves = (0..collision.leaves.len())
            .map(|k| (k % n) as u16)
            .collect();
    }
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
        sound: &sound,
        visibility,
        atmosphere,
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

    // Cluster fields the engine reads (docs/halo1/multiplayer.md).
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
    // Each material points to its source shader, including after relocation.
    let mats = off(word(&p, 0xa8));
    for (k, name) in ["a", "b"].into_iter().enumerate() {
        let at = mats + k * MATERIAL + 8;
        assert_eq!(word(&p, at), FourCC(*b"shad").as_u32());
        assert_eq!(map.tag(TagId(word(&p, at + 4))).unwrap().name.as_deref(), Some(name));
    }
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
        cluster: 0,
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
        check_sound_clusters(&p, at).unwrap_or_else(|e| panic!("{n} clusters: {e}"));
        // One ambience and one reverb sound cluster: no palette entry, no
        // portals, every cluster inside; and one zeroed element at +0x94.
        for field in [0x148, 0x150] {
            assert_eq!(word(&p, field), 1, "{n} clusters: +{field:#x}");
            let e = (word(&p, field + 4) - at) as usize;
            assert_eq!(word(&p, e), u32::MAX, "no palette entry");
            assert_eq!(word(&p, e + 4), 0, "no portals");
            assert_eq!(word(&p, e + 0xc) as usize, n);
            let list = (word(&p, e + 0x10) - at) as usize;
            let interior: Vec<u16> = p[list..list + 2 * n]
                .chunks(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            assert_eq!(interior, (0..n as u16).collect::<Vec<_>>());
        }
        assert_eq!(word(&p, 0x94), 1);
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
    // Without the elements the checks fail, as the engine would: 16
    // clusters naming sound cluster 0 of an empty block.
    let mut p = vec![0u8; 0x240 + 16 * CLUSTER];
    p[0x9c..0xa0].copy_from_slice(&16u32.to_le_bytes());
    p[0xa0..0xa4].copy_from_slice(&0x240u32.to_le_bytes());
    assert!(check_audibility(&p, 0).is_err());
    assert!(check_sound_clusters(&p, 0).is_err());
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

/// Retail's part kinds by template (`part_kind`'s census).
#[test]
fn part_kinds_follow_retail_by_template() {
    let t = |leaf: &str| format!("shaders\\shader_templates\\{leaf}");
    for (template, kind) in [
        ("opaque\\tex_bump", PART_KIND),
        ("opaque\\tex_bump_alpha_test_single_pass", PART_KIND),
        ("opaque\\tex_bump_env_illum", PART_KIND),
        ("opaque\\decal_simple", PART_KIND_DECAL),
        ("opaque\\overlay", PART_KIND_DECAL),
        ("transparent\\one_alpha_env", PART_KIND_TRANSPARENT),
        ("transparent\\one_alpha_env_illum", PART_KIND_TRANSPARENT),
        ("water\\water_pool", PART_KIND_TRANSPARENT),
        ("water\\water_pool_static", PART_KIND_TRANSPARENT),
        ("water\\water_edge_blend", PART_KIND),
    ] {
        assert_eq!(part_kind(&t(template)), kind, "{template}");
    }
    assert_eq!(part_kind(""), PART_KIND);
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(word(b, at))
}

#[test]
fn render_fields_are_filled_the_way_retail_fills_them() {
    let mut m = meshes();
    m.iter_mut().for_each(|m| m.page = Some(0));
    let sections = pack(&m, &[0, 1]);
    let b = build_lit(&sections, true);
    let mut map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    let render = StructureRender::read(&mut map, b.sbsp).unwrap();
    let c = &render.clusters[0];
    let w = |at: usize| u16::from_le_bytes([c.raw[at], c.raw[at + 1]]);

    // Section info: 7 vertices, 3 triangles, 2 parts. Material 1's part
    // (one triangle, three vertices) is transparent, so 2 triangles and
    // 1 part cast shadows and 4 vertices and 1 part are opaque; rigid;
    // lightmap coordinates compressed; one subpart per part; lit, with
    // incident radiance.
    let head: Vec<u16> = (0..0x28).step_by(2).map(w).collect();
    assert_eq!(
        head,
        [7, 3, 2, 2, 1, 0, 4, 1, 0, 2, 0, 4, 0, 0, 0, 0, 0, 0, 2, 3]
    );
    assert_eq!(&c.raw[0x6c..0x72], [0, 0xff, 0, 0xff, 0xff, 0xff]);
    assert_eq!(word(&c.raw, 0x94), 0x1234_5678, "the structure checksum");

    // Predicted resources: the cluster's own geometry, then its lightmap
    // page (the synthetic shaders declare no bitmap parameters).
    let tag = map.tag(b.sbsp).unwrap().clone();
    let at = tag.location.unwrap().address;
    let p = map.read_tag_bytes(b.sbsp).unwrap();
    let clusters = (word(&p, 0xa0) - at) as usize;
    assert_eq!(word(&p, clusters + 0x84), 2);
    let list = (word(&p, clusters + 0x88) - at) as usize;
    let bitmap = map
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"bitm"))
        .unwrap()
        .id;
    assert_eq!(&p[list..list + 4], [3, 0, 0, 0]);
    assert_eq!(word(&p, list + 4), b.sbsp.0);
    assert_eq!(&p[list + 8..list + 12], [0, 0, 0, 0]);
    assert_eq!(word(&p, list + 12), bitmap.0);

    // Parts: their material's kind (material 1's base head is
    // transparent in `templates`), flags 0xb, bounding sphere and box of
    // their own vertices.
    let mut raw = Mem(b.bytes.clone());
    let s = c.geometry.load(&mut raw, Some(2)).unwrap().unwrap();
    assert!(s.parts.iter().all(|p| p.flags == 0xb));
    for p in &s.parts {
        let want = [PART_KIND, PART_KIND_TRANSPARENT][usize::from(p.material)];
        assert_eq!(p.kind, want, "material {}", p.material);
    }
    let floor = &s.parts[0].raw;
    let centre: Vec<f32> = (0..3).map(|k| f32_at(floor, 0x10 + 4 * k)).collect();
    assert_eq!(centre, [0.5, 0.5, 0.0]);
    assert!((f32_at(floor, 0x2c) - 0.5f32.sqrt()).abs() < 1e-6);
    let bounds: Vec<f32> = (0..6).map(|k| f32_at(floor, 0x30 + 4 * k)).collect();
    assert_eq!(bounds, [0.0, 1.0, 0.0, 1.0, 0.0, 0.0]);
    assert_eq!(f32_at(&s.parts[1].raw, 0x18), 1.0, "the roof's centre z");

    // Stream 0x30: the incident direction, one packed word per vertex.
    let incident = s
        .stream(tags::geometry::VertexKind::Unknown(0x30))
        .expect("stream 0x30");
    let d = s.data(incident);
    assert_eq!(d.len(), 4 * 7);
    assert!(
        d.as_chunks::<4>()
            .0
            .iter()
            .all(|v| u32::from_le_bytes(*v) == UP)
    );

    // Unlit: no lighting flags, no lightmap page predicted.
    let b = build(&pack(&meshes(), &[0, 1]));
    let mut map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    let render = StructureRender::read(&mut map, b.sbsp).unwrap();
    let c = &render.clusters[0];
    assert_eq!(&c.raw[0x26..0x28], [0, 0]);
    assert_eq!(word(&c.raw, 0x84), 1);
}

#[test]
fn a_shaders_bitmaps_are_read_from_its_parameters() {
    let mut set = TagSet::new(CacheType::Multiplayer, "t", "invented\\t");
    for g in [b"bitm", b"shad"] {
        set.add_group(FourCC(*g), None, None);
    }
    let bitmap = |set: &mut TagSet, name: &str| {
        let mut b = Block::zeroed(0x4c);
        b.block(0x44, 2, Block::zeroed(2 * 0x74));
        set.add_tag(FourCC(*b"bitm"), name, TagPayload::Built(b))
            .unwrap()
    };
    let (a, b) = (
        bitmap(&mut set, "invented\\a"),
        bitmap(&mut set, "invented\\b"),
    );
    // Three parameters: a, b, a again; then a datum that is no bitmap.
    let mut params = Block::zeroed(0xc * 4);
    for (i, id) in [a, b, a, TagId(0x1234_0005)].iter().enumerate() {
        params.bytes[0xc * i..0xc * i + 4].copy_from_slice(&id.0.to_le_bytes());
    }
    let mut pp = Block::zeroed(0x7c);
    pp.block(0x04, 4, params);
    let mut root = Block::zeroed(0x54);
    root.block(0x20, 1, pp);
    let shader = set
        .add_tag(FourCC(*b"shad"), "invented\\s", TagPayload::Built(root))
        .unwrap();
    assert_eq!(shader_bitmaps(&set, shader), [a, b]);
    assert_eq!(bitmap_images(&set, a), 2);
    assert_eq!(
        bitmap_images(&set, shader),
        0,
        "not a built bitmap with images"
    );
    assert!(shader_bitmaps(&set, TagId::NULL).is_empty());
}

/// The pass at E `0x00044e90` draws a cluster for a set of volumes through
/// its section's visibility MOPP (`+0x28`) and takes the object branch,
/// reading the render entry's null object, when the reorder table (`+0x30`)
/// is empty: every converted level faulted there at version 7. Retail keys
/// are subpart indices; a table of at most twice the subparts keeps the
/// keys direct (`0x000459a8`).
#[test]
fn every_section_has_a_visibility_mopp_with_one_leaf_per_subpart() {
    let sections = pack(&meshes(), &[0, 1]);
    let b = build(&sections);
    let mut map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    let render = StructureRender::read(&mut map, b.sbsp).unwrap();
    let c = &render.clusters[0];
    let mut raw = Mem(b.bytes.clone());
    let s = c.geometry.load(&mut raw, Some(2)).unwrap().unwrap();
    let block = tags::geometry::load_block(&c.geometry.block, &mut raw)
        .unwrap()
        .unwrap();
    let section = block.section();
    let subparts = word(section, 0x08) as usize;
    assert_eq!(subparts, s.subparts.len());
    // The mask the engine allocates has a bit per subpart (cluster +0x24).
    assert_eq!(
        u16::from_le_bytes([c.raw[0x24], c.raw[0x25]]) as usize,
        subparts
    );
    // Leaf order and its inverse: exactly 2n, so the normal path.
    assert_eq!(word(section, 0x30) as usize, 2 * subparts);
    let data = block
        .resources()
        .iter()
        .find(|r| r.field_offset == 0x28)
        .expect("a visibility MOPP");
    let table = block
        .resources()
        .iter()
        .find(|r| r.field_offset == 0x30)
        .expect("a reorder table");
    assert_eq!(data.secondary, 0xffff);
    assert_eq!(word(section, 0x28), data.size);
    assert_eq!(
        (8 + SECTION + data.offset.unwrap() as usize) % 16,
        0,
        "the code object is 16-aligned in the loaded block"
    );
    let code_bytes = block.resource(data).unwrap();
    check_visibility(subparts, code_bytes, block.resource(table).unwrap()).unwrap();
    let code = mopp::Code::parse(code_bytes).unwrap();
    // Each subpart's own box finds it; nothing but subpart keys is emitted.
    let keys = mopp::bounds::key_bounds(&code).unwrap();
    assert_eq!(
        keys.keys().copied().collect::<Vec<_>>(),
        (0..subparts as u32).collect::<Vec<_>>()
    );
    let v = s.vertices(None).unwrap();
    for (k, p) in s.parts.iter().enumerate() {
        let idx = s.part_indices(p);
        let mut b = mopp::Aabb {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        };
        for &i in idx {
            for a in 0..3 {
                b.min[a] = b.min[a].min(v.positions[i as usize][a]);
                b.max[a] = b.max[a].max(v.positions[i as usize][a]);
            }
        }
        assert!(mopp::query_aabb(&code, &b).unwrap().contains(&(k as u32)));
    }
}

/// Converter version 9 keyed every triangle by its part and wrote an
/// identity table of `n` entries: a wholly seen subtree's first and last
/// key were read at `table[n + key]`, past the table, and the bits set
/// from there overran the view's mask area (a30, d20, a10, b30, d40).
#[test]
fn a_table_of_n_or_a_key_on_many_leaves_is_refused() {
    let sections = pack(&meshes(), &[0, 1]);
    let s = &sections[0];
    let n = s.parts.len();
    let (code, table) = visibility_mopp(s);
    check_visibility(n, &code, &table).unwrap();
    // The version 9 table: identity, n long.
    let identity: Vec<u8> = (0..n as u16).flat_map(u16::to_le_bytes).collect();
    assert!(check_visibility(n, &code, &identity).is_err());
    // The version 9 code: a leaf per triangle, keyed by part.
    let mut per_triangle = Vec::new();
    for (k, p) in s.parts.iter().enumerate() {
        let first = usize::from(p.first);
        for t in s.indices[first..first + usize::from(p.count)]
            .as_chunks::<3>()
            .0
        {
            per_triangle.push(mopp::Surface {
                key: k as u32,
                vertices: t
                    .iter()
                    .map(|&i| s.vertices[usize::from(i)].position)
                    .collect(),
            });
        }
    }
    let v9 = mopp::build(&per_triangle).to_block();
    if per_triangle.len() > n {
        assert!(check_visibility(n, &v9, &table).is_err());
    }
}

#[test]
fn clusters_name_their_sounds_and_leaves_their_cluster() {
    let sections = crate::clusters::pack_clusters(&meshes(), &[0, 1], 2).unwrap();
    assert_eq!(sections.len(), 2);
    let b = build_with(&sections, false, true);
    let map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    let t = map.tag(b.sbsp).unwrap().clone();
    let at = t.location.unwrap().address;
    let mut m = map;
    let sbsp = m.read_tag_bytes(b.sbsp).unwrap();
    let block = |field: usize, len: usize| -> Vec<Vec<u8>> {
        let (n, p) = (word(&sbsp, field) as usize, word(&sbsp, field + 4));
        let p = p.wrapping_sub(at) as usize;
        (0..n)
            .map(|k| sbsp[p + k * len..p + (k + 1) * len].to_vec())
            .collect()
    };
    // The structure's palettes: what the clusters index (E 0x0011bf46).
    let bg = block(0xd4, 0x64);
    let env = block(0xdc, 0x48);
    assert_eq!((bg.len(), env.len()), (2, 1));
    assert_eq!(&bg[1][..6], b"loop1\0");
    assert_eq!(word(&bg[0], 0x20), FourCC(*b"lsnd").as_u32());
    let lsnd = m.tag(TagId(word(&bg[1], 0x24))).unwrap();
    assert_eq!(lsnd.group, FourCC(*b"lsnd"));
    assert_eq!(word(&env[0], 0x20), FourCC(*b"snde").as_u32());
    let clusters = block(0x9c, CLUSTER);
    let half = |b: &[u8], o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
    assert_eq!(
        clusters
            .iter()
            .map(|c| [half(c, 0x72), half(c, 0x74)])
            .collect::<Vec<_>>(),
        vec![[0, 0], [1, -1]]
    );
    let leaves = block(0x2c, 8);
    assert_eq!(leaves.len(), b.collision.leaves.len());
    for (k, l) in leaves.iter().enumerate() {
        assert_eq!(half(l, 0) as usize, k % 2, "leaf {k}");
    }
}

#[test]
fn portals_and_each_cluster_s_list_are_written_where_the_flood_reads_them() {
    use crate::clusters::{PORTAL, Portal, Visibility};
    let sections = crate::clusters::pack_clusters(&meshes(), &[0, 1], 2).unwrap();
    let visibility = Visibility {
        portals: vec![Portal {
            clusters: [0, 1],
            plane: 2,
            centroid: [0.5, 0.5, 0.5],
            radius: 0.75,
            vertices: vec![[0.0, 0.0, 0.5], [1.0, 0.0, 0.5], [1.0, 1.0, 0.5]],
        }],
        clusters: vec![vec![0], vec![0]],
        skies: vec![1, 0],
        pvs: [1u32.to_le_bytes(), 2u32.to_le_bytes()].concat(),
    };
    let b = build_full(&sections, false, false, &visibility, &[]);
    let map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    let at = map.tag(b.sbsp).unwrap().location.unwrap().address;
    let mut m = map;
    let sbsp = m.read_tag_bytes(b.sbsp).unwrap();
    let field = |b: &[u8], o: usize| -> (usize, usize) {
        (
            word(b, o) as usize,
            word(b, o + 4).wrapping_sub(at) as usize,
        )
    };
    let (len, v) = field(&sbsp, 0x54);
    assert_eq!(len, 8);
    assert_eq!([word(&sbsp, v), word(&sbsp, v + 4)], [1, 2]);
    let f32_at = |o: usize| f32::from_le_bytes(sbsp[o..o + 4].try_into().unwrap());
    // sbsp +0x5c: 0x24 each, the clusters, plane, centre, radius, open.
    let (n, p) = field(&sbsp, 0x5c);
    assert_eq!(n, 1);
    let e = &sbsp[p..p + PORTAL];
    assert_eq!(&e[..4], &[0, 0, 1, 0]);
    assert_eq!(word(e, 4), 2);
    assert_eq!([f32_at(p + 8), f32_at(p + 0x14)], [0.5, 0.75]);
    assert_eq!(word(e, 0x18), 0, "open");
    let (vn, vp) = field(e, 0x1c);
    assert_eq!(vn, 3);
    assert_eq!([f32_at(vp + 12), f32_at(vp + 32)], [1.0, 0.5]);
    // Each cluster's +0x8c list names the portal.
    let (_, clusters) = field(&sbsp, 0x9c);
    for c in 0..2 {
        let (ln, lp) = field(&sbsp[clusters + CLUSTER * c..], 0x8c);
        assert_eq!(ln, 1, "cluster {c}");
        assert_eq!(&sbsp[lp..lp + 2], &[0, 0]);
        // +0x6e: the cluster's sky, the one the engine draws from it.
        assert_eq!(sbsp[clusters + CLUSTER * c + 0x6e], [1, 0][c], "cluster {c} sky");
    }
}

#[test]
fn a_cluster_without_a_sky_shows_its_nearest_neighbour_s_or_none() {
    use crate::clusters::{Portal, spread_skies};
    let portal = |a: u16, b: u16| Portal {
        clusters: [a, b],
        plane: 0,
        centroid: [0.0; 3],
        radius: 1.0,
        vertices: Vec::new(),
    };
    // 0 (sky 1) - 1 (none) - 2 (none) - 3 (sky 0); 4 sealed off, so no
    // sky reaches it and it shows none, as in Halo 1.
    let portals = [portal(0, 1), portal(1, 2), portal(2, 3)];
    assert_eq!(
        spread_skies(vec![1, -1, -1, 0, -1], &portals, 5),
        vec![1, 1, 0, 0, -1]
    );
    // a10's cryo bay (a10a): clusters joined by portals, none with a sky.
    // Halo 1 draws no sky there; sky 0 drew as a grey slab over the bay.
    assert_eq!(
        spread_skies(vec![-1, -1, -1], &[portal(0, 1), portal(1, 2)], 3),
        vec![-1, -1, -1]
    );
    // A structure with no Halo 1 clusters shows sky 0.
    assert_eq!(spread_skies(Vec::new(), &[], 2), vec![0, 0]);
}

#[test]
fn a_cluster_too_big_for_one_section_is_refused() {
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for i in 0..(SECTION_MAX as u32 / 3 + 1) {
        for p in [
            [i as f32, 0.0, 0.0],
            [i as f32, 1.0, 0.0],
            [i as f32, 0.0, 1.0],
        ] {
            vertices.push(vertex(p));
        }
        triangles.push([3 * i, 3 * i + 1, 3 * i + 2]);
    }
    let mesh = Mesh {
        shader: "a".into(),
        page: None,
        cluster: 1,
        vertices,
        triangles,
    };
    assert!(crate::clusters::pack_clusters(std::slice::from_ref(&mesh), &[0], 2).is_err());
    let small = &meshes()[..1];
    let sections = crate::clusters::pack_clusters(small, &[0], 3).unwrap();
    assert_eq!(
        sections.len(),
        3,
        "a cluster with nothing gets an empty section"
    );
    assert!(sections[1].indices.is_empty() && sections[2].indices.is_empty());
}

#[test]
fn each_cluster_gets_one_lightmap_page_sampling_what_its_meshes_did() {
    // Cluster 0 samples pages 0 and 1, and has an unlit mesh; cluster 1
    // has only the unlit one; cluster 2 samples page 1.
    let mut ms = meshes();
    ms[0].page = Some(0);
    ms[0].cluster = 0;
    ms[1].page = None;
    ms[1].cluster = 0;
    let mut second = meshes()[0].clone();
    second.page = Some(1);
    second.cluster = 0;
    for (k, v) in second.vertices.iter_mut().enumerate() {
        v.lightmap = [0.1 * k as f32, 0.5];
    }
    let mut unlit = meshes()[1].clone();
    unlit.cluster = 1;
    let mut third = second.clone();
    third.cluster = 2;
    ms.extend([second, unlit, third]);
    let before = ms.clone();
    let pages = crate::clusters::lightmap_pages(&mut ms, &[[16, 16], [32, 8]], 3).unwrap();
    assert_eq!(pages.len(), 2, "cluster 1 has nothing lit");
    let image = |m: &Mesh| m.page;
    assert_eq!(
        ms.iter().map(image).collect::<Vec<_>>(),
        [Some(0), Some(0), Some(0), None, Some(1)]
    );
    // Cluster 0's page: a piece of page 0, one of page 1, one of white.
    let kinds: Vec<Option<u16>> = pages[0].pieces.iter().map(|p| p.page).collect();
    assert_eq!(kinds, [Some(0), Some(1), None]);
    // Every lit vertex lands on the same texel of its source, moved.
    for (b, a) in before.iter().zip(&ms) {
        let (Some(src), Some(dst)) = (b.page, a.page) else {
            continue;
        };
        let page = &pages[usize::from(dst)];
        let piece = page.pieces.iter().find(|p| p.page == Some(src)).unwrap();
        for (vb, va) in b.vertices.iter().zip(&a.vertices) {
            let texel = |uv: f32, n: u32| uv * n as f32;
            for k in 0..2 {
                let from = texel(vb.lightmap[k], piece.page_size[k]) - piece.from[k] as f32;
                let to = texel(va.lightmap[k], page.size[k])
                    - (piece.at[k] + h1bitmaps::atlas::BORDER) as f32;
                assert!((from - to).abs() < 1e-3, "{from} vs {to}");
            }
        }
    }
}

#[test]
fn a_cluster_with_no_geometry_gets_retail_s_empty_raw_block() {
    let sections = crate::clusters::pack_clusters(&meshes()[..1], &[0], 2).unwrap();
    let b = build(&sections);
    let map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    let at = map.tag(b.sbsp).unwrap().location.unwrap().address;
    let mut m = map;
    let sbsp = m.read_tag_bytes(b.sbsp).unwrap();
    let clusters = word(&sbsp, 0xa0).wrapping_sub(at) as usize;
    let c = &sbsp[clusters + CLUSTER..clusters + 2 * CLUSTER];
    assert_ne!(word(c, 0x28), u32::MAX, "a raw block");
    assert_eq!(
        [word(c, 0x2c), word(c, 0x30), word(c, 0x34)],
        [0x50, SECTION as u32, 0]
    );
    assert_eq!(word(c, 0x40), b.sbsp.0);
    assert_eq!(word(c, 0x38), 0, "no resources");
    assert_eq!(word(c, 0x84), 1, "its own geometry predicted");
    assert_eq!(
        f32::from_le_bytes(c[0x54..0x58].try_into().unwrap()),
        f32::MAX
    );
}

#[test]
fn a_surface_listed_by_two_clusters_is_drawn_by_both() {
    let v = vec![vertex([0.0; 3]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])];
    let tris = [[0, 1, 2], [0, 2, 1], [1, 2, 0]];
    // Triangle 0 in cluster 0, 1 in clusters 0 and 2, 2 in none.
    let meshes = crate::clusters::split_by_cluster("s", None, v, &tris, &[vec![0], vec![0, 2], vec![]]);
    let got: Vec<(u16, Vec<[u32; 3]>)> = meshes.into_iter().map(|m| (m.cluster, m.triangles)).collect();
    assert_eq!(got, vec![(0, vec![[0, 1, 2], [0, 2, 1]]), (2, vec![[0, 2, 1]])]);
}

#[test]
fn a_cluster_names_its_atmosphere_where_halo_2_reads_it() {
    let sections = crate::clusters::pack_clusters(&meshes(), &[0, 1], 2).unwrap();
    // Cluster 0 has a sky; cluster 1 takes atmosphere 0 (E 0x0012efb0).
    let b = build_full(&sections, false, false, &Default::default(), &[0xff, 0]);
    let mut map = Map::from_reader(Cursor::new(b.bytes.clone())).unwrap();
    let at = map.tag(b.sbsp).unwrap().location.unwrap().address;
    let sbsp = map.read_tag_bytes(b.sbsp).unwrap();
    let clusters = word(&sbsp, 0xa0).wrapping_sub(at) as usize;
    let atmosphere: Vec<u8> = (0..2).map(|c| sbsp[clusters + CLUSTER * c + 0x6f]).collect();
    assert_eq!(atmosphere, [0xff, 0]);
    // +0x6c stays the scenario's first sky.
    for c in 0..2 {
        assert_eq!(sbsp[clusters + CLUSTER * c + 0x6c], 0);
    }
}
