//! Synthetic tests for the level geometry and views. Every byte is invented.
use super::*;
use crate::live_render::{VIEWS, basis, fit_box, render, shadow_map};
use serde_json::Value;

fn tri(p: [[f32; 3]; 3]) -> Tri {
    let n = normalize(cross(sub(p[1], p[0]), sub(p[2], p[0])));
    Tri {
        p,
        n: [n; 3],
        surface: Surface::Unknown,
    }
}

/// An axis-aligned box as 12 outward, counter-clockwise triangles.
fn cuboid(lo: [f32; 3], hi: [f32; 3]) -> Vec<Tri> {
    let c = |i: usize| {
        [
            if i & 1 == 0 { lo[0] } else { hi[0] },
            if i & 2 == 0 { lo[1] } else { hi[1] },
            if i & 4 == 0 { lo[2] } else { hi[2] },
        ]
    };
    // Faces as corner quads, counter-clockwise seen from outside.
    let quads = [
        [0, 2, 3, 1], // bottom (-z)
        [4, 5, 7, 6], // top (+z)
        [0, 1, 5, 4], // south (-y)
        [2, 6, 7, 3], // north (+y)
        [0, 4, 6, 2], // west (-x)
        [1, 3, 7, 5], // east (+x)
    ];
    quads
        .iter()
        .flat_map(|q| {
            [
                tri([c(q[0]), c(q[1]), c(q[2])]),
                tri([c(q[0]), c(q[2]), c(q[3])]),
            ]
        })
        .collect()
}

#[test]
fn shader_names_give_classes_by_generic_words() {
    assert_eq!(surface_from_name(r"levels\x\shaders\sky_far"), Surface::Sky);
    assert_eq!(surface_from_name(r"a\b\level_clouds"), Surface::Sky);
    assert_eq!(surface_from_name(r"a\b\panel_window"), Surface::Unknown);
    assert_eq!(
        surface_from_name(r"a\b\glass_panel"),
        Surface::Class(Class::Glass)
    );
    assert_eq!(
        surface_from_name(r"a\b\big_waterfall"),
        Surface::Class(Class::Water)
    );
    assert_eq!(surface_from_name(r"a\b\seam"), Surface::Unknown, "not sea");
    assert_eq!(
        surface_from_name(r"a\b\waterside_cliff"),
        Surface::Unknown,
        "a level named after water"
    );
    assert_eq!(
        surface_from_name(r"a\b\wall_rusted_b"),
        Surface::Class(Class::Metal)
    );
    assert_eq!(
        surface_from_name(r"a\b\tall_grass"),
        Surface::Class(Class::Foliage)
    );
    assert_eq!(
        surface_from_name(r"a\b\metal_plate"),
        Surface::Class(Class::Metal)
    );
    assert_eq!(surface_from_name(r"a\b\concrete"), Surface::Unknown);
    assert_eq!(
        surface_from_name(r"levels\waterside\metal\rock"),
        Surface::Unknown,
        "folders do not count"
    );
}

#[test]
fn facing_decides_floor_wall_and_ceiling() {
    let up = [0.0, 0.0, 1.0];
    let side = [1.0, 0.0, 0.0];
    let down = [0.0, 0.0, -1.0];
    assert_eq!(classify(Surface::Unknown, up), Class::Floor);
    assert_eq!(classify(Surface::Unknown, side), Class::Wall);
    assert_eq!(classify(Surface::Unknown, down), Class::Default);
    assert_eq!(classify(Surface::Class(Class::Metal), up), Class::Floor);
    assert_eq!(classify(Surface::Class(Class::Metal), side), Class::Metal);
    assert_eq!(classify(Surface::Class(Class::Water), up), Class::Water);
}

#[test]
fn meshes_weld_classify_and_drop_degenerates() {
    let mut tris = cuboid([0.0, 0.0, 0.0], [10.0, 20.0, 4.0]);
    // Degenerate, and a duplicate of the first top triangle.
    tris.push(tri([[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]));
    tris.push(tris[2]);
    let mesh = build_mesh(&tris);
    assert_eq!(mesh.triangles(), 12);
    // Top is floor, bottom is a ceiling, four walls: each face keeps its
    // own normal, so 4 vertices per face.
    let classes: Vec<(Class, usize, usize)> = mesh
        .primitives
        .iter()
        .map(|(c, p)| (*c, p.indices.len() / 3, p.positions.len()))
        .collect();
    assert_eq!(
        classes,
        [
            (Class::Floor, 2, 4),
            (Class::Wall, 8, 16),
            (Class::Default, 2, 4)
        ]
    );
    assert_eq!(mesh.bounds.min, [0.0, 0.0, 0.0]);
    assert_eq!(mesh.bounds.max, [10.0, 20.0, 4.0]);
}

#[test]
fn bad_vertex_normals_fall_back_to_the_face() {
    let mut t = tri([[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    t.n = [[0.0; 3], [0.0, 0.0, -1.0], [0.0, 0.6, 0.8]];
    let mesh = build_mesh(&[t]);
    let n = &mesh.primitives[0].1.normals;
    assert_eq!(n[0], [0.0, 0.0, 1.0], "zero");
    assert_eq!(n[1], [0.0, 0.0, 1.0], "against the winding");
    assert_eq!(n[2], [0.0, 0.6, 0.8], "kept");
}

/// Splits a GLB into its JSON and BIN chunks, checking the framing.
fn parse_glb(glb: &[u8]) -> (Value, Vec<u8>) {
    let u32_at = |o: usize| u32::from_le_bytes(glb[o..o + 4].try_into().unwrap());
    assert_eq!(&glb[..4], b"glTF");
    assert_eq!(u32_at(4), 2);
    assert_eq!(u32_at(8) as usize, glb.len());
    let json_len = u32_at(12) as usize;
    assert_eq!(&glb[16..20], b"JSON");
    assert_eq!(json_len % 4, 0);
    let json: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
    let rest = 20 + json_len;
    if rest == glb.len() {
        return (json, Vec::new());
    }
    let bin_len = u32_at(rest) as usize;
    assert_eq!(&glb[rest + 4..rest + 8], b"BIN\0");
    assert_eq!(rest + 8 + bin_len, glb.len());
    (json, glb[rest + 8..].to_vec())
}

/// The bytes an accessor covers.
fn accessor<'a>(doc: &Value, bin: &'a [u8], i: u64) -> (&'a [u8], usize) {
    let a = &doc["accessors"][i as usize];
    let v = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let (off, len) = (
        v["byteOffset"].as_u64().unwrap() as usize,
        v["byteLength"].as_u64().unwrap() as usize,
    );
    assert!(off + len <= bin.len());
    assert_eq!(off % 4, 0);
    (&bin[off..off + len], a["count"].as_u64().unwrap() as usize)
}

#[test]
fn glb_round_trips_with_converted_axes() {
    let mesh = build_mesh(&cuboid([1.0, 2.0, 3.0], [11.0, 22.0, 7.0]));
    let glb = write_glb(&mesh);
    // An independent glTF reader accepts it.
    let parsed = gltf::Gltf::from_slice(&glb).expect("valid glTF");
    assert_eq!(parsed.meshes().count(), 1);
    assert_eq!(parsed.nodes().next().unwrap().name(), Some("structure"));
    assert!(
        parsed
            .materials()
            .all(|m| Class::ALL.iter().any(|c| Some(c.name()) == m.name()))
    );

    let (doc, bin) = parse_glb(&glb);
    assert_eq!(doc["asset"]["version"], "2.0");
    let extras = &doc["asset"]["extras"];
    assert_eq!(extras["generator"], GENERATOR);
    assert_eq!(extras["geometry_version"], GEOMETRY_VERSION);
    assert_eq!(extras["bounds"]["min"], serde_json::json!([1.0, 2.0, 3.0]));
    assert_eq!(
        extras["bounds"]["max"],
        serde_json::json!([11.0, 22.0, 7.0])
    );
    assert_eq!(doc["nodes"][0]["name"], "structure");
    assert!(
        doc["nodes"][0].get("matrix").is_none(),
        "no node transforms"
    );
    let prims = doc["meshes"][0]["primitives"].as_array().unwrap();
    assert_eq!(prims.len(), mesh.primitives.len());
    let mut triangles = 0;
    for (prim, (class, src)) in prims.iter().zip(&mesh.primitives) {
        let m = &doc["materials"][prim["material"].as_u64().unwrap() as usize];
        assert_eq!(m["name"], class.name());
        let (pos, n) = accessor(&doc, &bin, prim["attributes"]["POSITION"].as_u64().unwrap());
        assert_eq!(n, src.positions.len());
        let floats: Vec<f32> = pos
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        for (k, p) in src.positions.iter().enumerate() {
            // Game (x, y, z) is glTF (x, z, -y).
            assert_eq!(&floats[3 * k..3 * k + 3], &[p[0], p[2], -p[1]]);
        }
        let (nrm, _) = accessor(&doc, &bin, prim["attributes"]["NORMAL"].as_u64().unwrap());
        let nf: Vec<f32> = nrm
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        if *class == Class::Floor {
            // Game up (+z) is glTF up (+y).
            assert_eq!(&nf[..3], &[0.0, 1.0, 0.0]);
        }
        let ia = prim["indices"].as_u64().unwrap();
        assert_eq!(doc["accessors"][ia as usize]["componentType"], 5123, "u16");
        let (idx, ni) = accessor(&doc, &bin, ia);
        assert_eq!(ni % 3, 0);
        assert_eq!(idx.len(), 2 * ni);
        triangles += ni / 3;
        assert!(
            idx.as_chunks::<2>()
                .0
                .iter()
                .all(|b| (u16::from_le_bytes(*b) as usize) < n)
        );
    }
    assert_eq!(triangles, 12);
    // Accessor min/max in glTF axes.
    let first = &doc["accessors"][0];
    assert!(first["min"].is_array() && first["max"].is_array());
}

#[test]
fn big_primitives_split_so_indices_fit_in_u16() {
    // A strip of triangles over more vertices than u16 can index.
    let n = MAX_PRIMITIVE_VERTICES + 10;
    let p = Primitive {
        positions: (0..n).map(|i| [i as f32, (i % 2) as f32, 0.0]).collect(),
        normals: vec![[0.0, 0.0, 1.0]; n],
        indices: (0..n as u32 - 2).flat_map(|i| [i, i + 1, i + 2]).collect(),
    };
    let parts = split_u16(&p);
    assert_eq!(parts.len(), 2);
    let mut tris = Vec::new();
    for q in &parts {
        assert!(q.positions.len() <= MAX_PRIMITIVE_VERTICES);
        for t in q.indices.as_chunks::<3>().0 {
            tris.push(t.map(|i| q.positions[i as usize]));
        }
    }
    let want: Vec<[[f32; 3]; 3]> = p
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| p.positions[i as usize]))
        .collect();
    assert_eq!(tris, want, "every triangle once, in order");
    // A small one is untouched.
    let small = build_mesh(&cuboid([0.0; 3], [1.0; 3]));
    assert_eq!(
        split_u16(&small.primitives[0].1)[0].indices,
        small.primitives[0].1.indices
    );
}

#[test]
fn transparent_classes_blend_and_an_empty_level_is_still_valid() {
    let mut t = cuboid([0.0; 3], [1.0; 3]);
    for x in &mut t {
        x.surface = Surface::Class(Class::Glass);
    }
    let (doc, _) = parse_glb(&write_glb(&build_mesh(&t)));
    assert_eq!(doc["materials"][0]["name"], "glass");
    assert_eq!(doc["materials"][0]["alphaMode"], "BLEND");
    let empty = write_glb(&build_mesh(&[]));
    gltf::Gltf::from_slice(&empty).expect("valid empty glTF");
}

// ---- A synthetic structure BSP through the real decoder -----------------

const BASE: u32 = 0x8000_0000;
const OWNER: TagId = TagId(0xe001_0002);

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
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

/// A `blkh` block for one section: one triangle-list part (`kind`,
/// material 1) over a unit right triangle with +z normals.
fn section_block(kind: u16) -> (Vec<u8>, Vec<[u8; 16]>) {
    let mut part = vec![0u8; 0x48];
    put16(&mut part, 0, kind);
    put16(&mut part, 2, 0x8);
    put16(&mut part, 4, 1);
    put16(&mut part, 8, 3);
    put16(&mut part, 0xc, 1);
    let mut sub = vec![0u8; 8];
    put16(&mut sub, 2, 3);
    put16(&mut sub, 4, 0xffff);
    let indices: Vec<u8> = [0u16, 1, 2].iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut desc = vec![0u8; 0x40];
    desc[0] = 0x01;
    desc[1] = 12;
    desc[0x20] = 0x1b;
    desc[0x21] = 12;
    let positions: Vec<u8> = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let frame: Vec<u8> = (0..3)
        .flat_map(|_| [511u32 << 22, 0, 0])
        .flat_map(|v: u32| v.to_le_bytes())
        .collect();
    let res: Vec<(u8, u16, u16, Vec<u8>)> = vec![
        (0, 0, 0x48, part),
        (0, 8, 8, sub),
        (0, 0x20, 2, indices),
        (0, 0x38, 0x20, desc),
        (2, 0x38, 0, positions),
        (2, 0x38, 1, frame),
    ];
    let mut section = vec![0u8; 0x44];
    let mut data = Vec::new();
    let mut table = Vec::new();
    for (k, field, secondary, bytes) in &res {
        if *k == 0 {
            put32(
                &mut section,
                usize::from(*field),
                (bytes.len() / usize::from(*secondary)) as u32,
            );
        }
        data.extend(b"crsr");
        let mut e = [0u8; 16];
        e[0] = *k;
        e[1] = 2;
        put16(&mut e, 2, *field);
        put16(&mut e, 4, *field);
        put16(&mut e, 6, *secondary);
        put32(&mut e, 8, bytes.len() as u32);
        put32(&mut e, 0xc, data.len() as u32);
        data.extend(bytes);
        table.push(e);
    }
    let mut block = b"hklb".to_vec();
    block.extend((data.len() as u32).to_le_bytes());
    block.extend(&section);
    block.extend(&data);
    block.extend(b"fklb");
    (block, table)
}

fn put_geometry(p: &mut [u8], at: usize, table_at: usize, block: &[u8], table: &[[u8; 16]]) {
    put16(p, at, 3);
    put16(p, at + 2, 1);
    put16(p, at + 4, 1);
    put16(p, at + 0xc, 3);
    let b = at + 0x28;
    put32(p, b, 0x100);
    put32(p, b + 4, block.len() as u32);
    put32(p, b + 8, 0x44);
    put32(p, b + 0xc, (block.len() - 0x44 - 12) as u32);
    put32(p, b + 0x10, table.len() as u32);
    put32(p, b + 0x14, BASE + table_at as u32);
    put32(p, b + 0x18, OWNER.0);
    put16(p, b + 0x1c, 0x24);
    p[b + 0x1f] = 1;
    put32(p, b + 0x20, u32::MAX);
    for (i, e) in table.iter().enumerate() {
        p[table_at + 16 * i..table_at + 16 * (i + 1)].copy_from_slice(e);
    }
}

/// A BSP with one cluster, one definition and one placement (scale 2,
/// forward +y, left −x, up +z, at (10, 20, 30)), two materials.
fn synthetic_bsp(kind: u16) -> (StructureRender, Mem) {
    let (block, table) = section_block(kind);
    let mut raw = vec![0u8; 0x100];
    raw.extend(&block);
    let mut p = vec![0u8; 0x1000];
    put32(&mut p, 0xa4, 2);
    put32(&mut p, 0x9c, 1);
    put32(&mut p, 0xa0, BASE + 0x200);
    put_geometry(&mut p, 0x200, 0x800, &block, &table);
    for (i, v) in [-1.0f32, 2.0, -1.0, 2.0, -1.0, 1.0].iter().enumerate() {
        put32(&mut p, 0x200 + 0x54 + 4 * i, v.to_bits());
    }
    put32(&mut p, 0x138, 1);
    put32(&mut p, 0x13c, BASE + 0x400);
    put_geometry(&mut p, 0x400, 0x900, &block, &table);
    put32(&mut p, 0x140, 1);
    put32(&mut p, 0x144, BASE + 0x600);
    let m = [
        2.0f32, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 20.0, 30.0,
    ];
    for (i, v) in m.iter().enumerate() {
        put32(&mut p, 0x600 + 4 * i, v.to_bits());
    }
    let bsp = StructureRender::decode(OWNER, &TagView::new(&p, BASE)).unwrap();
    (bsp, Mem(raw))
}

#[test]
fn a_synthetic_bsp_becomes_a_placed_welded_mesh() {
    let (bsp, mut raw) = synthetic_bsp(2);
    let (tris, failed) = bsp_triangles(&bsp, &mut raw, &[Surface::Unknown, Surface::Unknown]);
    assert_eq!((tris.len(), failed), (2, 0));
    assert_eq!(tris[0].p, [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    // Instance: position + 2·(x·forward + y·left + z·up).
    assert_eq!(
        tris[1].p,
        [[10.0, 20.0, 30.0], [10.0, 22.0, 30.0], [8.0, 20.0, 30.0]]
    );
    for t in &tris {
        assert!((t.n[0][2] - 1.0).abs() < 1e-3);
    }
    let mesh = build_mesh(&tris);
    assert_eq!(mesh.triangles(), 2);
    assert_eq!(mesh.primitives[0].0, Class::Floor);
    assert_eq!(mesh.bounds.min, [0.0, 0.0, 0.0]);
    assert_eq!(mesh.bounds.max, [10.0, 22.0, 30.0]);
    let (doc, _) = parse_glb(&write_glb(&mesh));
    assert_eq!(
        doc["accessors"][0]["max"],
        serde_json::json!([10.0, 30.0, 0.0]),
        "glTF max of (x, z, -y)"
    );

    // A sky material and undrawn part kinds are not level geometry.
    let (sky, _) = bsp_triangles(&bsp, &mut raw, &[Surface::Unknown, Surface::Sky]);
    assert!(sky.is_empty());
    for kind in [0, 5] {
        let (bsp, mut raw) = synthetic_bsp(kind);
        assert!(
            bsp_triangles(&bsp, &mut raw, &[]).0.is_empty(),
            "kind {kind}"
        );
    }
}

#[test]
fn a_broken_section_is_counted_not_fatal() {
    let (bsp, _) = synthetic_bsp(2);
    let mut empty = Mem(Vec::new());
    let (tris, failed) = bsp_triangles(&bsp, &mut empty, &[]);
    assert!(tris.is_empty());
    assert_eq!(failed, 2, "the cluster and the definition");
}

// ---- Levels with several BSPs (campaign) ----------------------------------

fn node_names(doc: &Value) -> Vec<&str> {
    doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect()
}

#[test]
fn a_multi_bsp_level_has_a_node_per_bsp_and_skips_a_broken_one() {
    let (bsp, mut raw) = synthetic_bsp(2);
    let (placed, failed) = bsp_triangles(&bsp, &mut raw, &[]);
    assert_eq!(failed, 0);
    let bsps = vec![
        (0, Ok(build_mesh(&cuboid([0.0; 3], [10.0, 10.0, 3.0])))),
        (1, Err(anyhow!("reading BSP 1: broken"))),
        (2, Ok(build_mesh(&placed))),
        (3, Ok(build_mesh(&[]))),
    ];
    let level = Level::build("lvl", MapKind::Campaign, bsps).unwrap();
    assert_eq!(level.bsp_count, 4);
    assert_eq!(
        level.bsps.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(
        level.skipped.keys().collect::<Vec<_>>(),
        ["map/lvl/bsp_1", "map/lvl/bsp_3"]
    );
    assert!(level.skipped["map/lvl/bsp_1"].contains("broken"));
    assert_eq!(level.mesh.triangles(), 12 + 2, "the views draw every BSP");
    assert_eq!(level.mesh.bounds.max, [10.0, 22.0, 30.0]);

    let glb = level.glb();
    let parsed = gltf::Gltf::from_slice(&glb).expect("valid glTF");
    assert_eq!(parsed.meshes().count(), 2);
    let (doc, _) = parse_glb(&glb);
    assert_eq!(node_names(&doc), ["bsp_0", "bsp_2"]);
    assert_eq!(doc["scenes"][0]["nodes"], serde_json::json!([0, 1]));
    assert_eq!(doc["nodes"][1]["extras"]["bsp"], 2);
    assert_eq!(doc["meshes"][1]["name"], "bsp_2");
    assert!(doc["nodes"][0].get("matrix").is_none());
    // One material per class, shared by both meshes.
    let names: Vec<&str> = doc["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    let unique: HashSet<&str> = names.iter().copied().collect();
    assert_eq!(names.len(), unique.len());
    let triangles: u64 = doc["meshes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["primitives"].as_array().unwrap())
        .map(|p| {
            doc["accessors"][p["indices"].as_u64().unwrap() as usize]["count"]
                .as_u64()
                .unwrap()
                / 3
        })
        .sum();
    assert_eq!(triangles, 14);
    assert_eq!(
        doc["asset"]["extras"]["bounds"]["max"],
        serde_json::json!([10.0, 22.0, 30.0])
    );
}

#[test]
fn a_single_bsp_keeps_the_structure_node_and_no_bsp_is_an_error() {
    let one = Level::build(
        "mp",
        MapKind::Multiplayer,
        vec![(0, Ok(build_mesh(&cuboid([0.0; 3], [1.0; 3]))))],
    )
    .unwrap();
    let (doc, _) = parse_glb(&one.glb());
    assert_eq!(node_names(&doc), ["structure"]);
    assert!(doc["nodes"][0].get("extras").is_none());
    assert_eq!(doc["meshes"][0]["name"], "structure");
    // Same bytes as the single-mesh writer the multiplayer contract uses.
    assert_eq!(one.glb(), write_glb(&one.mesh));

    let none = Level::build(
        "lvl",
        MapKind::Campaign,
        vec![(0, Err(anyhow!("a"))), (1, Ok(build_mesh(&[])))],
    );
    assert!(none.is_err());
    let lone = Level::build(
        "mp",
        MapKind::Multiplayer,
        vec![(0, Err(anyhow!("bad tag")))],
    );
    assert!(format!("{:#}", lone.err().unwrap()).contains("bad tag"));
}

#[test]
fn templates_make_layers_drop_sky_and_mark_water_and_glass() {
    let t = surface_from_template;
    let layer = |l: Layer| Some(Surface::Layer(l));
    // Not drawn: sky templates (named for the sky first, not additive).
    for name in [
        r"shaders\shader_templates\opaque\sky_one",
        r"shaders\shader_templates\transparent\sky_two_alpha_clouds",
    ] {
        assert_eq!(t(name), Some(Surface::Sky), "{name}");
    }
    // Decals: overlays multiply, other decals blend or cut out.
    assert_eq!(
        t(r"shaders\shader_templates\opaque\overlay"),
        layer(Layer::decal(Blend::Multiply))
    );
    assert_eq!(
        t(r"shaders\shader_templates\transparent\decal_simple"),
        layer(Layer::decal(Blend::Alpha))
    );
    assert_eq!(
        t(r"shaders\shader_templates\opaque\decal_alpha_test"),
        layer(Layer::decal(Blend::Test))
    );
    // Effects: anything additive (glows, light volumes, an additive sky
    // card is an effect in the level), and alpha cloud cards (waterfalls).
    for name in [
        r"shaders\shader_templates\transparent\one_add_illum_detail",
        r"shaders\shader_templates\transparent\add_glow",
        r"shaders\shader_templates\transparent\sky_two_add_clouds",
    ] {
        assert_eq!(t(name), layer(Layer::effect(Blend::Additive)), "{name}");
    }
    assert_eq!(
        t(r"shaders\shader_templates\transparent\two_alpha_clouds"),
        layer(Layer::effect(Blend::Alpha))
    );
    assert_eq!(
        t(r"shaders\shader_templates\water\riverbed"),
        Some(Surface::Class(Class::Water)),
        "a water folder"
    );
    assert_eq!(
        t(r"shaders\shader_templates\transparent\waves"),
        Some(Surface::Class(Class::Water))
    );
    assert_eq!(
        t(r"shaders\shader_templates\transparent\one_alpha_env"),
        Some(Surface::Class(Class::Glass))
    );
    assert_eq!(t(r"shaders\shader_templates\opaque\tex_bump_env"), None);
    assert_eq!(
        t(r"shaders\shader_templates\opaque\tex_bump_env_detail_overlay"),
        None,
        "a detail overlay blend on a solid surface"
    );
    // Whole words only: "address" is not "add", "decals" folder is not
    // the template's own name.
    assert_eq!(t(r"shaders\shader_templates\opaque\address_bump"), None);
    assert_eq!(t(r"shaders\decal\tex_bump"), None);

    // Sky by either name; then the template decides layers and water; the
    // name decides the rest; a transparent template makes an unnamed
    // surface glass.
    let add = Some(r"shaders\shader_templates\transparent\one_add_illum");
    let alpha = Some(r"shaders\shader_templates\transparent\one_alpha_env");
    let opaque = Some(r"shaders\shader_templates\opaque\tex_bump");
    let overlay = Some(r"shaders\shader_templates\opaque\overlay");
    let clouds = Some(r"shaders\shader_templates\transparent\two_alpha_clouds");
    let glow = Surface::Layer(Layer::effect(Blend::Additive));
    assert_eq!(surface_of(Some(r"a\b\light_long_volume"), add), glow);
    assert_eq!(surface_of(Some(r"a\b\metal_glow"), add), glow);
    assert_eq!(
        surface_of(Some(r"a\b\glow_glass"), add),
        glow,
        "the template wins over glass"
    );
    assert_eq!(
        surface_of(Some(r"a\b\water_stain"), overlay),
        Surface::Layer(Layer::decal(Blend::Multiply)),
        "a stain named for water is still a decal"
    );
    assert_eq!(
        surface_of(Some(r"a\b\waterfall"), clouds),
        Surface::Layer(Layer::effect(Blend::Alpha))
    );
    assert_eq!(
        surface_of(Some(r"a\b\high_clouds"), clouds),
        Surface::Sky,
        "named for clouds"
    );
    assert_eq!(surface_of(Some(r"a\b\fog_wisps"), alpha), Surface::Sky);
    assert_eq!(
        surface_of(Some(r"a\b\metal_grate"), alpha),
        Surface::Class(Class::Metal)
    );
    assert_eq!(
        surface_of(Some(r"a\b\panel"), alpha),
        Surface::Class(Class::Glass)
    );
    assert_eq!(surface_of(None, alpha), Surface::Class(Class::Glass));
    assert_eq!(surface_of(Some(r"a\b\panel"), opaque), Surface::Unknown);
    assert_eq!(surface_of(Some(r"a\b\sky_far"), opaque), Surface::Sky);
    assert_eq!(surface_of(Some(r"a\b\panel"), None), Surface::Unknown);
    assert_eq!(surface_of(None, None), Surface::Unknown);
    assert!(!glow.is_level() && !Surface::Sky.is_level() && Surface::Unknown.is_level());
    // Machines: as the level, except unknown is metal.
    assert_eq!(machine_surface(Some(r"a\b\beam"), add), glow);
    assert_eq!(
        machine_surface(Some(r"a\b\panel"), opaque),
        Surface::Class(Class::Metal)
    );
}

#[test]
fn a_triangle_is_backdrop_by_area() {
    // Area 0.5 * side²: 4000 at side sqrt(8000).
    let side = (2.0 * BACKDROP_AREA).sqrt();
    let right = |s: f32| tri([[0.0; 3], [s, 0.0, 0.0], [0.0, s, 0.0]]);
    assert!(is_backdrop(&right(side * 1.001)));
    assert!(!is_backdrop(&right(side * 0.999)));
    assert!(!is_backdrop(&right(40.0)), "a big multiplayer floor");
    // Tilted: area, not footprint.
    let wall = tri([[0.0; 3], [200.0, 0.0, 0.0], [0.0, 0.0, 200.0]]);
    assert!(is_backdrop(&wall));
    let sea = Tri {
        surface: Surface::Class(Class::Water),
        ..right(1000.0)
    };
    assert!(!is_backdrop(&sea), "water stays");
}

#[test]
fn a_backdrop_plane_goes_whole_and_the_level_beside_it_stays() {
    // A huge plane at z = 0 and, sharing its corner (200, 0, 0), a fan of
    // small coplanar triangles; a small floor beyond them, slightly raised
    // (out of the plane) but sharing a corner; a wall standing on the
    // plane's edge; a coplanar triangle of another surface; and a detached
    // small triangle in the same plane.
    let huge = tri([[0.0; 3], [200.0, 0.0, 0.0], [0.0, 200.0, 0.0]]);
    let fan = [
        tri([[200.0, 0.0, 0.0], [210.0, 0.0, 0.0], [205.0, 5.0, 0.1]]),
        tri([[210.0, 0.0, 0.0], [220.0, 0.0, 0.0], [205.0, 5.0, 0.1]]),
    ];
    let raised = tri([[220.0, 0.0, 0.0], [230.0, 0.0, 2.0], [225.0, 5.0, 2.0]]);
    let wall = tri([[0.0; 3], [0.0, 0.0, 10.0], [0.0, 10.0, 0.0]]);
    let metal = Tri {
        surface: Surface::Class(Class::Metal),
        ..tri([[0.0, 200.0, 0.0], [5.0, 200.0, 0.0], [0.0, 205.0, 0.0]])
    };
    let apart = tri([
        [500.0, 500.0, 0.0],
        [510.0, 500.0, 0.0],
        [500.0, 510.0, 0.0],
    ]);
    let tris = vec![fan[0], raised, huge, wall, metal, fan[1], apart];
    let (level, backdrop) = split_backdrop(tris);
    let ps = |v: &[Tri]| v.iter().map(|t| t.p).collect::<Vec<_>>();
    assert_eq!(ps(&backdrop), [fan[0].p, huge.p, fan[1].p]);
    assert_eq!(ps(&level), [raised.p, wall.p, metal.p, apart.p]);
    let (level, backdrop) = split_backdrop(Vec::new());
    assert!(level.is_empty() && backdrop.is_empty());
}

#[test]
fn scripts_switch_to_gameplay_bsps_by_number_only() {
    let source = "(script startup main (begin (switch_bsp 2) (sleep 30)))\n\
                  (script dormant cine (switch_bsp_by_name level_cine_bsp))\n\
                  (script static void a (switch_bsp 4 ))\n\
                  (script static void b (switch_bsp (+ 1 2)))\n\
                  (script static void c (switch_bsp 7x))\n\
                  (script static void d (switch_bsp 12))";
    assert_eq!(
        scripted_switches(source).into_iter().collect::<Vec<_>>(),
        [2, 4, 12]
    );
    assert!(scripted_switches("").is_empty());

    // BSP 0 always; both ends of every switch volume; scripted targets.
    let scripted = BTreeSet::from([5]);
    let g = gameplay_from(&[(0, 1), (1, 2), (2, 1), (-1, 3)], &scripted);
    assert_eq!(g.into_iter().collect::<Vec<_>>(), [0, 1, 2, 3, 5]);
    assert_eq!(
        gameplay_from(&[], &BTreeSet::new())
            .into_iter()
            .collect::<Vec<_>>(),
        [0]
    );
}

#[test]
fn cinematic_bsps_are_flagged_and_left_out_of_the_views_and_backdrops_are_children() {
    let play = build_mesh(&cuboid([0.0; 3], [10.0, 10.0, 3.0]));
    let cine = build_mesh(&cuboid([500.0, 500.0, 0.0], [510.0, 510.0, 3.0]));
    let far = build_mesh(&[tri([
        [-1000.0, -1000.0, -5.0],
        [1000.0, -1000.0, -5.0],
        [0.0, 1000.0, -5.0],
    ])]);
    let mut level = Level::build(
        "lvl",
        MapKind::Campaign,
        vec![(0, Ok(play)), (1, Ok(build_mesh(&[]))), (2, Ok(cine))],
    )
    .unwrap();
    assert_eq!(level.mesh.bounds.max[0], 510.0, "every BSP before");
    level.backdrops.insert(0, far);
    let level = level.with_cinematic(BTreeSet::from([1, 2]));
    // The views and frame are the gameplay BSPs only.
    assert_eq!(level.mesh.triangles(), 12);
    assert_eq!(level.mesh.bounds.max, [10.0, 10.0, 3.0]);
    assert!(level.frame.max[0] <= 10.0 * 1.1);

    let glb = level.glb();
    gltf::Gltf::from_slice(&glb).expect("valid glTF");
    let (doc, _) = parse_glb(&glb);
    // The node index is still the scenario's BSP index.
    assert_eq!(node_names(&doc), ["bsp_0", "backdrop_0", "bsp_2"]);
    assert_eq!(doc["scenes"][0]["nodes"], serde_json::json!([0, 2]));
    assert_eq!(doc["nodes"][0]["extras"], serde_json::json!({ "bsp": 0 }));
    assert_eq!(doc["nodes"][0]["children"], serde_json::json!([1]));
    assert_eq!(
        doc["nodes"][1]["extras"],
        serde_json::json!({ "backdrop": true })
    );
    assert_eq!(
        doc["nodes"][2]["extras"],
        serde_json::json!({ "bsp": 2, "cinematic": true })
    );
    assert_eq!(
        doc["asset"]["extras"]["bounds"]["max"],
        serde_json::json!([10.0, 10.0, 3.0])
    );

    // Features go in `asset.extras` when there are any.
    let mut level = level;
    level.features = crate::live_features::Features {
        teleporters: Vec::new(),
        links: Vec::new(),
        lifts: vec![crate::live_features::Lift {
            id: "l0".into(),
            pos: [1.0, 2.0, 3.0],
            dir: [0.0, 0.0, 1.0],
            length: 4.0,
            radius: 1.0,
            source: "test".into(),
        }],
    };
    let (doc, _) = parse_glb(&level.glb());
    assert_eq!(
        doc["asset"]["extras"]["features"]["lifts"][0]["pos"],
        serde_json::json!([1.0, 2.0, 3.0])
    );
    assert_eq!(
        doc["asset"]["extras"]["features"]["teleporters"],
        serde_json::json!([])
    );
    assert_eq!(doc["asset"]["extras"]["geometry_version"], GEOMETRY_VERSION);

    // A single BSP: the backdrop hangs off `structure`; marking every BSP
    // cinematic leaves the views alone.
    let mut one = Level::build(
        "mp",
        MapKind::Campaign,
        vec![(0, Ok(build_mesh(&cuboid([0.0; 3], [1.0; 3]))))],
    )
    .unwrap();
    one.backdrops.insert(
        0,
        build_mesh(&[tri([[0.0; 3], [200.0, 0.0, 0.0], [0.0, 200.0, 0.0]])]),
    );
    let one = one.with_cinematic(BTreeSet::from([0]));
    assert_eq!(one.mesh.triangles(), 12);
    let (doc, _) = parse_glb(&one.glb());
    assert_eq!(node_names(&doc), ["structure", "backdrop"]);
    assert!(doc["nodes"][0].get("extras").is_none());
}

// ---- Views ---------------------------------------------------------------

fn level(tris: &[Tri]) -> (LevelMesh, Bounds) {
    let mesh = build_mesh(tris);
    let frame = mesh.bounds;
    (mesh, frame)
}

fn alpha(rgba: &[u8], w: usize, x: usize, y: usize) -> u8 {
    rgba[(y * w + x) * 4 + 3]
}

fn apply(m: &[[f64; 4]; 2], p: [f32; 3]) -> [f64; 2] {
    let v = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2]), 1.0];
    [0, 1].map(|r| (0..4).map(|k| m[r][k] * v[k]).sum())
}

#[test]
fn the_top_view_covers_exactly_the_projected_box() {
    let (mesh, frame) = level(&cuboid([0.0; 3], [10.0, 10.0, 4.0]));
    let shadow = shadow_map(&mesh, &frame);
    // 64 px, 4 px margin: 5.6 px per unit; (0, 0) lands on pixel corner
    // (4, 60) and (10, 10) on (60, 4).
    let cam = fit_box(basis(None), &frame, 64.0, 64.0, 4.0);
    let m = cam.world_to_pixel();
    let close = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3;
    assert!(close(apply(&m, [0.0, 0.0, 4.0]), [4.0, 60.0]));
    assert!(
        close(apply(&m, [10.0, 10.0, 0.0]), [60.0, 4.0]),
        "z does not move a top view"
    );
    let rgba = render(&mesh, &frame, &shadow, &cam, 64, 64, false);
    let mut full = 0;
    let mut partial = 0;
    for y in 0..64 {
        for x in 0..64 {
            match alpha(&rgba, 64, x, y) {
                255 => {
                    full += 1;
                    assert!((4..60).contains(&x) && (4..60).contains(&y), "{x},{y}");
                }
                0 => assert!(!((4..60).contains(&x) && (4..60).contains(&y)), "{x},{y}"),
                _ => partial += 1,
            }
        }
    }
    assert_eq!((full, partial), (56 * 56, 0));
    // The middle of the top is lit floor colour, brighter than its edge line.
    let px = |x: usize, y: usize| &rgba[(y * 64 + x) * 4..(y * 64 + x) * 4 + 3];
    assert!(px(32, 32).iter().all(|&c| c > 90), "{:?}", px(32, 32));
    assert!(px(4, 32)[0] < px(32, 32)[0], "silhouette line");
}

#[test]
fn iso_views_draw_each_world_point_where_the_matrix_puts_it() {
    // A floor slab with a block on it, seen from the south-west.
    let mut tris = cuboid([0.0, 0.0, -1.0], [40.0, 40.0, 0.0]);
    tris.extend(cuboid([15.0, 15.0, 0.0], [25.0, 25.0, 8.0]));
    let (mesh, frame) = level(&tris);
    let (w, h) = (640usize, 480usize);
    let cam = fit_box(basis(Some(225.0)), &frame, w as f32, h as f32, 8.0);
    let m = cam.world_to_pixel();
    for p in [[1.0, 2.0, 3.0], [-5.0, 7.0, 0.5], [30.0, 10.0, 8.0]] {
        let a = apply(&m, p);
        let b = cam.project(p);
        assert!((a[0] - f64::from(b[0])).abs() < 1e-3 && (a[1] - f64::from(b[1])).abs() < 1e-3);
    }
    // At the pixel the matrix gives, the nearest surface is the point
    // itself when it is visible, and something nearer when it is hidden.
    let clip = crate::live_render::clip_box(&frame);
    let (depth, normal) = crate::live_render::gbuffer(&mesh, &cam, &clip, None, 1, w, h);
    let probe = |p: [f32; 3]| {
        let q = apply(&m, p);
        let i = q[1] as usize * w + q[0] as usize;
        (depth[i] - dot(p, cam.forward), normal[i])
    };
    let tolerance = 2.0 / cam.scale;
    for (p, class) in [
        ([20.0, 20.0, 8.0], Class::Floor), // roof
        ([20.0, 15.0, 4.0], Class::Wall),  // south wall, facing the camera
        ([15.0, 20.0, 4.0], Class::Wall),  // west wall
        ([5.0, 30.0, 0.0], Class::Floor),  // open floor
    ] {
        let (dz, n) = probe(p);
        assert!(dz.abs() < tolerance, "{p:?}: {dz}");
        assert_eq!(Class::ALL[n[3] as usize], class, "{p:?}");
    }
    let (dz, _) = probe([25.5, 25.5, 0.0]);
    assert!(dz < -1.0, "the floor behind the block is hidden: {dz}");
    // The north and east walls face away and are culled.
    let (dz, _) = probe([20.0, 25.0, 4.0]);
    assert!(dz < -1.0, "{dz}");

    // Drawn: covered where the level is, transparent around it.
    let shadow = shadow_map(&mesh, &frame);
    let rgba = render(&mesh, &frame, &shadow, &cam, w, h, false);
    let q = apply(&m, [20.0, 20.0, 8.0]);
    assert_eq!(alpha(&rgba, w, q[0] as usize, q[1] as usize), 255);
    assert_eq!(alpha(&rgba, w, 2, 2), 0);
}

#[test]
fn the_frame_skips_sparse_backdrop_and_the_views_cut_it_away() {
    // A detailed structure high up, over a huge plain with a lonely rock.
    let mut tris = Vec::new();
    for i in 0..10 {
        for j in 0..10 {
            let (x, y) = (i as f32 * 2.0, j as f32 * 2.0);
            tris.extend(cuboid([x, y, 100.0], [x + 1.0, y + 1.0, 101.0]));
        }
    }
    tris.extend(cuboid([-500.0, -500.0, -10.0], [500.0, 500.0, 0.0]));
    tris.extend(cuboid([300.0, 300.0, 0.0], [310.0, 310.0, 10.0]));
    let mesh = build_mesh(&tris);
    let f = frame(&mesh);
    for k in 0..2 {
        assert!(f.min[k] > -40.0 && f.max[k] < 60.0, "{f:?}");
        assert!(f.min[k] <= 0.0 && f.max[k] >= 19.0, "{f:?}");
    }
    assert!(f.min[2] > 50.0 && f.max[2] >= 101.0, "{f:?}");
    // The plain is cut away: nothing but the structure is drawn.
    let cam = fit_box(basis(None), &f, 64.0, 64.0, 0.0);
    let clip = crate::live_render::clip_box(&f);
    let (depth, _) = crate::live_render::gbuffer(&mesh, &cam, &clip, None, 1, 64, 64);
    let drawn: Vec<f32> = depth.iter().copied().filter(|d| d.is_finite()).collect();
    assert!(!drawn.is_empty());
    assert!(
        drawn.iter().all(|&d| d <= -100.0),
        "only tops at z 101 face up"
    );
}

#[test]
fn sparse_height_tails_behind_a_gap_are_left_out() {
    let mut z: Vec<f32> = (0..100).map(|i| 50.0 + i as f32 * 0.1).collect();
    assert_eq!(z_range(&z), (50.0, 59.9));
    z.insert(0, -40.0);
    z.insert(0, -41.0);
    z.push(200.0);
    assert_eq!(z_range(&z), (50.0, 59.9));
    // A big share is not a tail.
    let mut z: Vec<f32> = (0..20).map(|_| 0.0).collect();
    z.extend((0..80).map(|i| 50.0 + i as f32 * 0.1));
    assert_eq!(z_range(&z).0, 0.0);
}

/// A quad from four corners, counter-clockwise seen from where it faces.
fn quad(c: [[f32; 3]; 4]) -> [Tri; 2] {
    [tri([c[0], c[1], c[2]]), tri([c[0], c[2], c[3]])]
}
/// A horizontal quad at height `z` facing up or down.
fn flat(x0: f32, y0: f32, x1: f32, y1: f32, z: f32, up: bool) -> [Tri; 2] {
    if up {
        quad([[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]])
    } else {
        quad([[x0, y0, z], [x0, y1, z], [x1, y1, z], [x1, y0, z]])
    }
}

#[test]
fn the_top_view_cuts_roofs_away_but_keeps_bridges_and_walls() {
    let mut tris = Vec::new();
    // Ground, z 0, 60 x 40.
    tris.extend(flat(0.0, 0.0, 60.0, 40.0, 0.0, true));
    // A 20 x 20 room: floor at z 0 (the ground), ceiling at 3 facing down,
    // a roof on top at 3.5, and 1-unit walls whose tops are at 3.5.
    tris.extend(flat(5.0, 5.0, 25.0, 25.0, 3.0, false));
    tris.extend(flat(4.0, 4.0, 26.0, 26.0, 3.5, true));
    // A 1-unit-wide bridge at z 5 over the ground, underside at 4.8.
    tris.extend(flat(30.0, 18.0, 58.0, 19.0, 5.0, true));
    tris.extend(flat(30.0, 18.0, 58.0, 19.0, 4.8, false));
    // A rock standing on the ground: top only (its bottom is buried).
    tris.extend(flat(40.0, 30.0, 50.0, 38.0, 2.0, true));
    tris.extend(flat(40.0, 30.0, 50.0, 38.0, -1.0, false));
    let mesh = build_mesh(&tris);
    let frame = mesh.bounds;
    let clip = crate::live_render::clip_box(&frame);
    let (w, h) = (300usize, 200usize);
    let cam = fit_box(basis(None), &frame, w as f32, h as f32, 0.0);
    let m = cam.world_to_pixel();
    let cut = crate::live_render::Cutaway::new(&mesh, &cam, &clip, w, h);
    let (depth, _) = crate::live_render::gbuffer(&mesh, &cam, &clip, Some(&cut), 1, w, h);
    let (plain, _) = crate::live_render::gbuffer(&mesh, &cam, &clip, None, 1, w, h);
    let z_at = |d: &[f32], p: [f32; 2]| {
        let q = apply(&m, [p[0], p[1], 0.0]);
        -d[q[1] as usize * w + q[0] as usize]
    };
    // Without the cutaway the room is its roof; with it, its floor.
    assert_eq!(z_at(&plain, [15.0, 15.0]), 3.5);
    assert_eq!(z_at(&depth, [15.0, 15.0]), 0.0, "room floor");
    assert_eq!(z_at(&depth, [6.0, 15.0]), 0.0, "near the wall, inside");
    assert_eq!(z_at(&depth, [4.5, 15.0]), 3.5, "the wall keeps its top");
    // The bridge is too narrow to be a room; the rock has no ceiling.
    assert_eq!(z_at(&depth, [44.0, 18.5]), 5.0, "bridge");
    assert_eq!(z_at(&depth, [45.0, 34.0]), 2.0, "rock");
    assert_eq!(z_at(&depth, [28.0, 35.0]), 0.0, "open ground");
    // world_to_pixel is the same camera either way, and a full render with
    // the cutaway still covers the level.
    let shadow = shadow_map(&mesh, &frame);
    let rgba = render(&mesh, &frame, &shadow, &cam, w, h, true);
    let q = apply(&m, [15.0, 15.0, 0.0]);
    assert_eq!(alpha(&rgba, w, q[0] as usize, q[1] as usize), 255);
}

#[test]
fn views_fit_what_is_drawn_not_the_frame_box() {
    // A thin diagonal walkway in a big, mostly empty frame.
    let tris = cuboid([0.0, 0.0, 0.0], [100.0, 4.0, 1.0]);
    let mesh = build_mesh(&tris);
    let frame = Bounds {
        min: [-50.0, -50.0, -20.0],
        max: [150.0, 50.0, 20.0],
    };
    let spec = &VIEWS[0];
    let cam = crate::live_render::fit(spec, &mesh, &frame);
    let m = cam.world_to_pixel();
    let (w, h) = (f64::from(spec.width), f64::from(spec.height));
    let pts: Vec<[f64; 2]> = mesh
        .bounds
        .corners()
        .iter()
        .map(|&c| apply(&m, c))
        .collect();
    let (x0, x1) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[0]), a.1.max(p[0])));
    let margin = 0.03 * w.min(h);
    assert!(
        (x0 - margin).abs() < 1.0 && (x1 - (w - margin)).abs() < 1.0,
        "{x0} {x1}"
    );
    assert!(pts.iter().all(|p| p[1] >= 0.0 && p[1] <= h));
    // Geometry crossing the clip box is fitted by its cut, not its ends.
    let long = build_mesh(&cuboid([-1000.0, 0.0, 0.0], [1000.0, 4.0, 1.0]));
    let cam = crate::live_render::fit(spec, &long, &frame);
    let a = apply(&cam.world_to_pixel(), [-50.0, 2.0, 0.0]);
    assert!(a[0] > 0.0 && a[0] < margin + 25.0, "{a:?}");
}

#[test]
fn polygons_are_cut_to_the_box() {
    let b = Bounds {
        min: [0.0; 3],
        max: [1.0; 3],
    };
    let t = [[-1.0, 0.5, 0.5], [2.0, 0.5, 0.5], [0.5, 0.9, 0.5]];
    let out = crate::live_render::clip_polygon(&t, &b);
    assert!(out.len() >= 4);
    assert!(
        out.iter()
            .all(|p| (0..3).all(|k| (0.0..=1.0).contains(&p[k])))
    );
    let far = [[5.0, 5.0, 5.0], [6.0, 5.0, 5.0], [5.0, 6.0, 5.0]];
    assert!(crate::live_render::clip_polygon(&far, &b).is_empty());
}

#[test]
fn views_json_and_manifest_entries_follow_the_contract() {
    let (mesh, frame) = level(&cuboid([0.0; 3], [100.0, 60.0, 10.0]));
    let views: Vec<ViewEntry> = cameras(&mesh, &frame).into_iter().map(|(e, _)| e).collect();
    let ids: Vec<&str> = views.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, ["top", "iso-ne", "iso-nw", "iso-sw", "iso-se"]);
    let file = ViewsFile {
        geometry_version: GEOMETRY_VERSION,
        map: "stem".into(),
        bounds: mesh.bounds,
        frame,
        views,
    };
    let v = serde_json::to_value(&file).unwrap();
    assert_eq!(v["geometry_version"], 1);
    assert_eq!(v["bounds"]["max"], serde_json::json!([100.0, 60.0, 10.0]));
    let top = &v["views"][0];
    assert_eq!(top["kind"], "ortho");
    assert_eq!(
        (top["width"].as_u64(), top["height"].as_u64()),
        (Some(2048), Some(2048))
    );
    assert_eq!(top["up"], "north");
    assert!(top.get("depth_sorted").is_none());
    assert_eq!(top["cutaway"], true);
    assert!(v["views"][1].get("cutaway").is_none());
    assert_eq!(top["file"], "views/top.png");
    let iso = &v["views"][1];
    assert_eq!(
        (iso["width"].as_u64(), iso["height"].as_u64()),
        (Some(2048), Some(1536))
    );
    assert_eq!(iso["depth_sorted"], true);
    assert_eq!(iso["world_to_pixel"].as_array().unwrap().len(), 2);
    assert_eq!(iso["world_to_pixel"][0].as_array().unwrap().len(), 4);
    // North is up in the top view: +y goes to smaller pixel y.
    let m = &file.views[0].world_to_pixel;
    assert!(apply(m, [50.0, 50.0, 0.0])[1] < apply(m, [50.0, 10.0, 0.0])[1]);
    // Every view keeps the frame on its canvas.
    for view in &file.views {
        for c in frame.corners() {
            let [x, y] = apply(&view.world_to_pixel, c);
            assert!(
                (0.0..=f64::from(view.width)).contains(&x),
                "{}: {x}",
                view.id
            );
            assert!(
                (0.0..=f64::from(view.height)).contains(&y),
                "{}: {y}",
                view.id
            );
        }
    }
    assert_eq!(serde_json::from_value::<ViewsFile>(v).unwrap(), file);

    // The manifest entry.
    let mut manifest =
        crate::live_art::build_manifest("k".into(), BTreeMap::new(), Vec::new(), BTreeMap::new());
    let none = serde_json::to_value(&manifest).unwrap();
    assert!(
        none.get("maps").is_none(),
        "absent when nothing was extracted"
    );
    manifest.maps.insert(
        "stem".into(),
        MapEntry {
            geometry: Some("map/stem/map.glb".into()),
            views: Some("map/stem/views.json".into()),
            kind: Some(MapKind::Campaign),
            bsps: Some(3),
        },
    );
    let j = serde_json::to_value(&manifest).unwrap();
    assert_eq!(
        j["maps"]["stem"],
        serde_json::json!({"geometry": "map/stem/map.glb", "views": "map/stem/views.json",
                           "kind": "campaign", "bsps": 3})
    );
    let back: crate::live_art::Manifest = serde_json::from_value(j).unwrap();
    assert_eq!(back, manifest);
    // An older manifest without maps still reads, and so does an entry
    // without a kind.
    let old: crate::live_art::Manifest = serde_json::from_value(none).unwrap();
    assert!(old.maps.is_empty());
    let older: MapEntry =
        serde_json::from_value(serde_json::json!({"geometry": "map/s/map.glb"})).unwrap();
    assert_eq!((older.kind, older.bsps), (None, None));
    let mp: MapEntry = serde_json::from_value(serde_json::json!({"kind": "multiplayer"})).unwrap();
    assert_eq!(mp.kind, Some(MapKind::Multiplayer));
}

// ---- The owner's disc ----------------------------------------------------

/// Where the disc check writes: `$H2_GEOMETRY_CHECK`, else this default
/// (outside the repository either way).
const CHECK_DIR: &str = "/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc/map-quality";

/// The extraction to compare against: the campaign geometry check's art.
const BEFORE_DIR: &str =
    "/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc/campaign-geometry/art";

/// Every level the extractor reads, with its kind.
fn levels(disc: &Disc) -> Vec<(&MapFile, MapKind)> {
    disc.maps_of(CacheType::Multiplayer)
        .map(|m| (m, MapKind::Multiplayer))
        .chain(
            disc.maps_of(CacheType::Campaign)
                .map(|m| (m, MapKind::Campaign)),
        )
        .collect()
}

/// Box-downscale an RGBA image by an integer factor, over a dark backdrop.
fn thumb(rgba: &[u8], w: usize, h: usize, f: usize, bg: [u8; 3]) -> (usize, usize, Vec<u8>) {
    let (tw, th) = (w / f, h / f);
    let mut out = vec![0u8; tw * th * 4];
    for ty in 0..th {
        for tx in 0..tw {
            let mut acc = [0f32; 3];
            for y in ty * f..ty * f + f {
                for x in tx * f..tx * f + f {
                    let i = (y * w + x) * 4;
                    let a = f32::from(rgba[i + 3]) / 255.0;
                    for k in 0..3 {
                        acc[k] += f32::from(rgba[i + k]) * a + f32::from(bg[k]) * (1.0 - a);
                    }
                }
            }
            let o = (ty * tw + tx) * 4;
            for k in 0..3 {
                out[o + k] = (acc[k] / (f * f) as f32) as u8;
            }
            out[o + 3] = 255;
        }
    }
    (tw, th, out)
}

/// Extracts every multiplayer map's geometry from the owner's disc into
/// the check directory (outside the repository) with timings and a contact
/// sheet. `H2_DISC`, else `disc/halo2.iso` in the main checkout.
#[test]
#[ignore = "needs the disc; writes outside the repository"]
fn geometry_from_the_disc() {
    let disc_path = std::env::var("H2_DISC").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../../disc/halo2.iso").into()
    });
    let keep = std::env::var("H2_GEOMETRY_CHECK").unwrap_or_else(|_| CHECK_DIR.into());
    let check = Path::new(&keep);
    let out = check.join("art");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let started = Instant::now();
    let disc = Disc::scan(Path::new(&disc_path)).unwrap();
    let scanned = started.elapsed();
    let x = extract(&disc, &out);
    let total = started.elapsed();
    let mut report = format!(
        "scan {scanned:?}, geometry {:?} total ({} maps)\n",
        total - scanned,
        x.maps.len()
    );
    for s in &x.stats {
        report += &format!(
            "{:<20} {:<11} bsps {}/{} machines {:>3} ({:>6} tris) src {:>7} backdrop {:>4} tris {:>7} verts {:>7} glb {:>5} KiB failed {} shaders {}/{} load {:>6.0?} render {:>6.0?} {:?}\n",
            s.stem,
            format!("{:?}", s.kind.unwrap()),
            s.bsps,
            s.scenario_bsps,
            s.machines,
            s.machine_triangles,
            s.source_triangles,
            s.backdrop_triangles,
            s.triangles,
            s.vertices,
            s.glb_bytes / 1024,
            s.failed_sections,
            s.shaders_named,
            s.materials,
            s.load,
            s.render,
            s.classes
        );
    }
    // Layers and skies: what each level got.
    for s in &x.stats {
        report += &format!(
            "{:<20} layers {:>6} tris {:>4} materials dropped {:>5} sky.glb {:>5} KiB\n",
            s.stem,
            s.layer_triangles,
            s.layer_materials,
            s.layers_dropped,
            s.sky_bytes / 1024
        );
    }
    for stem in x.maps.keys() {
        let glb = fs::read(out.join("map").join(stem).join("map.glb")).unwrap();
        let (doc, _) = parse_glb(&glb);
        report += &format!("{stem:<20} sky {}\n", doc["asset"]["extras"]["sky"]);
        let mut blends: BTreeMap<String, usize> = BTreeMap::new();
        for m in doc["materials"].as_array().into_iter().flatten() {
            if let Some(l) = m["extras"]["layer"].as_str() {
                let tex = if m["pbrMetallicRoughness"].get("baseColorTexture").is_some() {
                    ""
                } else {
                    " untextured"
                };
                *blends
                    .entry(format!(
                        "{l}/{}{tex}",
                        m["extras"]["blend"].as_str().unwrap_or("")
                    ))
                    .or_default() += 1;
            }
        }
        report += &format!("{stem:<20} layer materials {blends:?}\n");
    }
    for (stem, e) in &x.maps {
        let v: ViewsFile =
            serde_json::from_slice(&fs::read(out.join(e.views.as_ref().unwrap())).unwrap())
                .unwrap();
        report += &format!("{stem:<16} bounds {:?} frame {:?}\n", v.bounds, v.frame);
    }
    for (k, why) in &x.skipped {
        report += &format!("skipped {k}: {why}\n");
    }
    print!("{report}");
    fs::write(check.join("report.txt"), &report).unwrap();

    // The shader names behind the classes, for review only (outside the
    // repository).
    let mut names = String::new();
    for (m, _) in levels(&disc) {
        let (mut map, _) = disc.open(m).unwrap();
        for bsp in scenario_bsps(&mut map).unwrap() {
            if bsp == TagId::NULL {
                continue;
            }
            for (i, n) in material_shaders(&mut map, bsp)
                .unwrap()
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                let n = n.as_deref().unwrap_or("-");
                names += &format!("{} {i:>3} {:?} {n}\n", m.name(), surface_from_name(n));
            }
        }
    }
    fs::write(check.join("shaders.txt"), names).unwrap();

    // Every glb parses; every views.json reads back.
    for (stem, e) in &x.maps {
        let glb = fs::read(out.join(e.geometry.as_ref().unwrap())).unwrap();
        gltf::Gltf::from_slice(&glb).unwrap_or_else(|err| panic!("{stem}: {err}"));
        let views: ViewsFile =
            serde_json::from_slice(&fs::read(out.join(e.views.as_ref().unwrap())).unwrap())
                .unwrap();
        assert_eq!(views.views.len(), 5, "{stem}");
    }

    // The machine levels' top and one iso view, for review.
    let shots = check.join("machines");
    let _ = fs::remove_dir_all(&shots);
    fs::create_dir_all(&shots).unwrap();
    for stem in ["zanzibar", "ascension"] {
        for view in ["top", "iso-ne"] {
            let from = out.join("map").join(stem).join(format!("views/{view}.png"));
            if from.exists() {
                fs::copy(&from, shots.join(format!("{stem}-{view}.png"))).unwrap();
            }
        }
    }

    // The top view without the cutaway, side by side with it, at half
    // size, for review.
    let pairs = check.join("cutaway");
    let _ = fs::remove_dir_all(&pairs);
    fs::create_dir_all(&pairs).unwrap();
    let mut split = String::new();
    for (m, kind) in levels(&disc) {
        let mut stats = MapStats::default();
        let Ok(level) = load_level(&disc, m, kind, &mut stats) else {
            continue;
        };
        let (mut map, _) = disc.open(m).unwrap();
        let names: Vec<String> = scenario_bsps(&mut map)
            .unwrap()
            .into_iter()
            .map(|id| {
                let name = map.tag(id).ok().and_then(|t| t.name.clone());
                let name = name.unwrap_or_default();
                name.rsplit(['\\', '/'])
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        split += &format!(
            "{:<20} bsps {} cinematic {:?} backdrop {:?} {names:?}\n",
            m.name(),
            level.bsp_count,
            level.cinematic,
            level
                .backdrops
                .iter()
                .map(|(i, b)| (*i, b.triangles()))
                .collect::<Vec<_>>()
        );
        let (entry, cam) = cameras(&level.mesh, &level.frame).remove(0);
        let (w, h) = (entry.width as usize, entry.height as usize);
        let bg = [0x15, 0x19, 0x1e];
        let mut halves = Vec::new();
        for cut in [false, true] {
            let rgba =
                live_render::render(&level.mesh, &level.frame, &level.shadow, &cam, w, h, cut);
            halves.push(thumb(&rgba, w, h, 2, bg));
        }
        let (tw, th) = (halves[0].0, halves[0].1);
        let mut pair = vec![0u8; tw * 2 * th * 4];
        for y in 0..th {
            for (k, (_, _, t)) in halves.iter().enumerate() {
                let dst = (y * tw * 2 + k * tw) * 4;
                pair[dst..dst + tw * 4].copy_from_slice(&t[y * tw * 4..(y + 1) * tw * 4]);
            }
        }
        let mut png = Vec::new();
        assets::png::write_rgba(&mut png, (tw * 2) as u32, th as u32, &pair).unwrap();
        fs::write(pairs.join(format!("{}.png", m.name())), png).unwrap();
    }

    fs::write(check.join("bsps.txt"), split).unwrap();

    // Before and after: a reference extraction's top and iso-ne views
    // beside these, a row per map (`H2_GEOMETRY_BEFORE`, an earlier art
    // directory; skipped when absent).
    let before = std::env::var("H2_GEOMETRY_BEFORE").unwrap_or_else(|_| BEFORE_DIR.into());
    let before = Path::new(&before);
    if before.exists() {
        let compare = check.join("compare");
        let _ = fs::remove_dir_all(&compare);
        fs::create_dir_all(&compare).unwrap();
        let bg = [0x15, 0x19, 0x1e];
        let f = 4;
        let cell = 2048 / f;
        let mut sheet_rows = Vec::new();
        for stem in x.maps.keys() {
            let mut row = vec![0u8; cell * 4 * cell * 4];
            for px in row.as_chunks_mut::<4>().0 {
                *px = [bg[0], bg[1], bg[2], 255];
            }
            for (col, (dir, view)) in [
                (before, "top"),
                (out.as_path(), "top"),
                (before, "iso-ne"),
                (out.as_path(), "iso-ne"),
            ]
            .into_iter()
            .enumerate()
            {
                let spec = VIEWS.iter().find(|v| v.id == view).unwrap();
                let path = dir.join("map").join(stem).join(format!("views/{view}.png"));
                let Ok(img) = image_rgba(&path) else { continue };
                let (tw, th, t) = thumb(&img, spec.width as usize, spec.height as usize, f, bg);
                for y in 0..th.min(cell) {
                    let dst = (y * cell * 4 + col * cell) * 4;
                    let n = tw.min(cell) * 4;
                    row[dst..dst + n].copy_from_slice(&t[y * tw * 4..y * tw * 4 + n]);
                }
            }
            let mut png = Vec::new();
            assets::png::write_rgba(&mut png, (cell * 4) as u32, cell as u32, &row).unwrap();
            fs::write(compare.join(format!("{stem}.png")), png).unwrap();
            sheet_rows.push(thumb(&row, cell * 4, cell, 2, bg));
        }
        let (rw, rh) = (sheet_rows[0].0, sheet_rows[0].1);
        let mut sheet = Vec::with_capacity(rw * rh * 4 * sheet_rows.len());
        for (_, _, r) in &sheet_rows {
            sheet.extend_from_slice(r);
        }
        let mut png = Vec::new();
        assets::png::write_rgba(&mut png, rw as u32, (rh * sheet_rows.len()) as u32, &sheet)
            .unwrap();
        fs::write(check.join("compare.png"), png).unwrap();
    }

    // Contact sheet: a row per map, its five views.
    let bg = [0x15, 0x19, 0x1e];
    let f = 6;
    let (cell_w, cell_h) = (2048 / f, 2048 / f);
    let rows = x.maps.len();
    let (sw, sh) = (cell_w * 5, cell_h * rows.max(1));
    let mut sheet = vec![0u8; sw * sh * 4];
    for px in sheet.as_chunks_mut::<4>().0 {
        *px = [bg[0], bg[1], bg[2], 255];
    }
    for (row, stem) in x.maps.keys().enumerate() {
        for (col, spec) in VIEWS.iter().enumerate() {
            let path = out
                .join("map")
                .join(stem)
                .join(format!("views/{}.png", spec.id));
            let Ok(img) = image_rgba(&path) else { continue };
            let (tw, th, t) = thumb(&img, spec.width as usize, spec.height as usize, f, bg);
            for y in 0..th {
                let dst = ((row * cell_h + y) * sw + col * cell_w) * 4;
                sheet[dst..dst + tw * 4].copy_from_slice(&t[y * tw * 4..(y + 1) * tw * 4]);
            }
        }
    }
    let mut png = Vec::new();
    assets::png::write_rgba(&mut png, sw as u32, sh as u32, &sheet).unwrap();
    fs::write(check.join("contact.png"), png).unwrap();
    assert!(!x.maps.is_empty());
}

fn image_rgba(path: &Path) -> Result<Vec<u8>> {
    let decoder = png::Decoder::new(std::io::BufReader::new(File::open(path)?));
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("png size")?];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    Ok(buf)
}

// ---- Machines --------------------------------------------------------------

/// An invented placement record: palette index, position, yaw/pitch/roll,
/// scale.
fn placement_record(palette: i16, pos: [f32; 3], rot: [f32; 3], scale: f32) -> Vec<u8> {
    let mut b = vec![0xaau8; 0x48];
    b[0..2].copy_from_slice(&palette.to_le_bytes());
    for (k, v) in pos.iter().chain(&rot).chain([&scale]).enumerate() {
        put32(&mut b, 8 + 4 * k, v.to_bits());
    }
    b
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

#[test]
fn placements_read_yaw_then_pitch_then_roll_and_scale() {
    use std::f32::consts::FRAC_PI_2 as Q;
    let at =
        Placement::parse(&placement_record(3, [10.0, 20.0, 30.0], [Q, 0.0, 0.0], 2.0)).unwrap();
    assert_eq!(at.palette, 3);
    assert_eq!(at.position, [10.0, 20.0, 30.0]);
    // Yaw turns forward (+x) to +y, about +z.
    assert!(close(at.axes()[0], [0.0, 1.0, 0.0]));
    assert!(
        close(at.transform([1.0, 0.0, 0.0]), [10.0, 22.0, 30.0]),
        "scaled by 2"
    );
    assert!(
        close(at.rotate([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]),
        "directions unscaled"
    );
    // Pitch tips forward up.
    let pitch = Placement {
        rotation: [0.0, Q, 0.0],
        ..at
    };
    assert!(close(pitch.axes()[0], [0.0, 0.0, 1.0]));
    // Roll turns left (+y) up, about forward.
    let roll = Placement {
        rotation: [0.0, 0.0, Q],
        ..at
    };
    assert!(close(roll.axes()[1], [0.0, 0.0, 1.0]));
    assert!(close(roll.axes()[0], [1.0, 0.0, 0.0]));
    // Order: roll, then pitch, then yaw (outermost). With yaw and pitch
    // both 90°, up ends at −y; pitching after the yaw would leave it at −x.
    let both = Placement {
        rotation: [Q, Q, 0.0],
        ..at
    };
    let [f, l, u] = both.axes();
    assert!(close(f, [0.0, 0.0, 1.0]));
    assert!(close(l, [-1.0, 0.0, 0.0]));
    assert!(close(u, [0.0, -1.0, 0.0]));
    // Scale zero (and below) means one.
    let unit = Placement { scale: 0.0, ..at };
    assert_eq!(unit.scale(), 1.0);
    assert!(close(unit.transform([1.0, 0.0, 0.0]), [10.0, 21.0, 30.0]));
    assert!(Placement::parse(&[0u8; 0x20]).is_none(), "short record");
}

#[test]
fn the_glb_matrix_is_the_placement_in_gltf_axes() {
    let at = Placement {
        palette: 0,
        position: [3.0, -4.0, 5.0],
        rotation: [0.7, -0.3, 1.1],
        scale: 1.5,
    };
    let m = at.gltf_matrix();
    for p in [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [2.0, -3.0, 0.5],
    ] {
        let g = to_gltf(p);
        let out: [f32; 3] =
            std::array::from_fn(|r| m[r] * g[0] + m[4 + r] * g[1] + m[8 + r] * g[2] + m[12 + r]);
        assert!(close(out, to_gltf(at.transform(p))), "{p:?}");
    }
    assert_eq!(&m[3..4], &[0.0]);
    assert_eq!(m[15], 1.0);
}

#[test]
fn machines_are_metal_shared_meshes_under_a_machines_node() {
    // A model: a box (unnamed shader -> metal, even its top) and a glass
    // pane.
    let mut tris: Vec<Tri> = cuboid([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0])
        .into_iter()
        .map(|t| Tri {
            surface: machine_surface(None, None),
            ..t
        })
        .collect();
    tris.extend(flat(-1.0, -1.0, 1.0, 1.0, 3.0, true).map(|t| Tri {
        surface: machine_surface(Some("objects\\thing\\shaders\\glass_pane"), None),
        ..t
    }));
    let model = MachineModel {
        stem: "wheel".into(),
        mesh: object_mesh(&tris),
    };
    let classes: Vec<Class> = model.mesh.primitives.iter().map(|(c, _)| *c).collect();
    assert_eq!(classes, [Class::Metal, Class::Glass]);
    assert_eq!(model.mesh.triangles(), 14);

    let ground = build_mesh(&flat(-20.0, -20.0, 20.0, 20.0, 0.0, true));
    let level = Level::build("lvl", MapKind::Multiplayer, vec![(0, Ok(ground))]).unwrap();
    let at = |i: usize, x: f32| PlacedMachine {
        name: format!("wheel_{i}"),
        model: 0,
        at: Placement {
            palette: 0,
            position: [x, 0.0, 0.0],
            rotation: [0.0; 3],
            scale: 1.0,
        },
    };
    let level = level.with_machines(vec![model], vec![at(0, -5.0), at(2, 5.0)]);
    // The views draw both copies, placed; the level's counts and bounds are
    // the BSP's.
    let placed: usize = level
        .mesh
        .objects
        .iter()
        .map(|(_, p)| p.indices.len() / 3)
        .sum();
    assert_eq!(placed, 28);
    assert_eq!(level.mesh.triangles(), 2);
    assert_eq!(level.mesh.bounds.max[2], 0.0);
    assert!(
        level.mesh.objects[0]
            .1
            .positions
            .iter()
            .any(|p| close(*p, [6.0, 1.0, 2.0]))
    );

    let glb = level.glb();
    gltf::Gltf::from_slice(&glb).expect("valid glTF");
    let (doc, _) = parse_glb(&glb);
    assert_eq!(
        node_names(&doc),
        ["structure", "machines", "wheel_0", "wheel_2"]
    );
    assert_eq!(doc["scenes"][0]["nodes"], serde_json::json!([0, 1]));
    let machines = &doc["nodes"][1];
    assert!(machines.get("mesh").is_none());
    assert_eq!(machines["children"], serde_json::json!([2, 3]));
    assert!(
        doc["nodes"][0].get("matrix").is_none(),
        "the level is world space"
    );
    // One mesh for both placements, each with its own matrix.
    assert_eq!(doc["meshes"].as_array().unwrap().len(), 2);
    assert_eq!(doc["nodes"][2]["mesh"], doc["nodes"][3]["mesh"]);
    let tx = |n: usize| doc["nodes"][n]["matrix"][12].as_f64().unwrap();
    assert_eq!((tx(2), tx(3)), (-5.0, 5.0));
    // The placement in game coordinates too, as frames give a machine.
    assert_eq!(
        doc["nodes"][3]["extras"],
        serde_json::json!({
            "pos": [5.0, 0.0, 0.0], "forward": [1.0, 0.0, 0.0],
            "up": [0.0, 0.0, 1.0], "scale": 1.0
        })
    );
    assert!(doc["asset"]["extras"].get("features").is_none());
    let names: Vec<&str> = doc["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["floor", "metal", "glass"]);
}

#[test]
fn the_cutaway_never_erases_a_machine() {
    // A box hovering over the ground: its underside is a ceiling with a top
    // above it and floor below, so as level geometry the top view cuts it.
    let ground = flat(0.0, 0.0, 40.0, 40.0, 0.0, true).to_vec();
    let hover = cuboid([10.0, 10.0, 2.0], [30.0, 30.0, 6.0]);
    let (w, h) = (200usize, 200usize);
    let top_at_centre = |mesh: &LevelMesh| {
        let frame = mesh.bounds;
        let clip = crate::live_render::clip_box(&frame);
        let cam = fit_box(basis(None), &frame, w as f32, h as f32, 0.0);
        let cut = crate::live_render::Cutaway::new(mesh, &cam, &clip, w, h);
        let (d, _) = crate::live_render::gbuffer(mesh, &cam, &clip, Some(&cut), 1, w, h);
        let q = apply(&cam.world_to_pixel(), [20.0, 20.0, 0.0]);
        -d[q[1] as usize * w + q[0] as usize]
    };
    let as_level = build_mesh(&[ground.clone(), hover.clone()].concat());
    assert_eq!(top_at_centre(&as_level), 0.0, "level geometry is cut");
    let mut with_object = build_mesh(&[ground, hover.clone()].concat());
    with_object.primitives = build_mesh(&flat(0.0, 0.0, 40.0, 40.0, 0.0, true)).primitives;
    with_object.objects = object_mesh(&hover).primitives;
    assert_eq!(
        top_at_centre(&with_object),
        6.0,
        "the machine keeps its top"
    );
}

/// Diagnostic: per map, BSP and material, the shader template, shader,
/// drawn triangle count and surface by name alone and with the template;
/// the materials whose surface the template changed on their own (outside
/// the repository).
#[test]
#[ignore = "needs the disc; writes outside the repository"]
fn shader_templates_on_the_disc() {
    let disc_path = std::env::var("H2_DISC").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../../disc/halo2.iso").into()
    });
    let disc = Disc::scan(Path::new(&disc_path)).unwrap();
    let mut out = String::new();
    let mut changed = String::new();
    for (m, _) in levels(&disc) {
        let (mut map, mut files) = disc.open(m).unwrap();
        for (bi, bsp_id) in scenario_bsps(&mut map).unwrap().into_iter().enumerate() {
            if bsp_id == TagId::NULL {
                continue;
            }
            let ids = material_shader_ids(&mut map, bsp_id).unwrap();
            let bsp = StructureRender::read(&mut map, bsp_id).unwrap();
            let mut count =
                vec![(0usize, 0f32, BTreeMap::<u16, usize>::new(), 0f32); ids.len() + 1];
            let materials = Some(bsp.material_count as usize);
            let mut raw = Raw(&mut files);
            let mut add = |s: &Section| {
                let Ok(v) = s.vertices(None) else { return };
                for part in &s.parts {
                    let k = usize::from(part.material).min(ids.len());
                    *count[k].2.entry(part.kind).or_default() += 1;
                    if !part.is_drawn() {
                        continue;
                    }
                    for t in s.part_triangles(part) {
                        let [a, b, c] = t.map(|i| v.positions[usize::from(i)]);
                        let f = cross(sub(b, a), sub(c, a));
                        let area = dot(f, f).sqrt() / 2.0;
                        count[k].0 += 1;
                        count[k].1 += area;
                        count[k].3 = count[k].3.max(area);
                    }
                }
            };
            for c in &bsp.clusters {
                if let Ok(Some(s)) = c.geometry.load(&mut raw, materials) {
                    add(&s);
                }
            }
            for d in &bsp.instance_definitions {
                if let Ok(Some(s)) = d.geometry.load(&mut raw, materials) {
                    add(&s);
                }
            }
            for (i, id) in ids.iter().enumerate() {
                let shader = map
                    .tag(*id)
                    .ok()
                    .and_then(|t| t.name.clone())
                    .unwrap_or_default();
                let template = shader_template(&mut map, *id).unwrap_or_default();
                let (n, area, kinds, max) = &count[i];
                let before = surface_from_name(&shader);
                let after = surface_of(Some(&shader), Some(&template));
                let line = format!(
                    "{} bsp{bi} m{i:<3} tris {n:>6} area {area:>9.0} max {max:>8.0} kinds {kinds:?} {before:?} -> {after:?} {template} | {shader}\n",
                    m.name()
                );
                if before != after && *n > 0 {
                    changed += &line;
                }
                out += &line;
            }
        }
    }
    let keep = std::env::var("H2_GEOMETRY_CHECK").unwrap_or_else(|_| CHECK_DIR.into());
    fs::create_dir_all(&keep).unwrap();
    fs::write(Path::new(&keep).join("templates.txt"), out).unwrap();
    fs::write(Path::new(&keep).join("surfaces-changed.txt"), changed).unwrap();
}

// ---- Layers (decals and effects) and the sky --------------------------------

#[test]
fn a_layer_material_comes_out_as_layer_triangles_not_level() {
    let (bsp, mut raw) = synthetic_bsp(2);
    let decal = Surface::Layer(Layer::decal(Blend::Multiply));
    // The synthetic sections draw with material 1.
    let (tris, layers, failed) = bsp_geometry(&bsp, &mut raw, &[decal, Surface::Unknown]);
    assert_eq!(failed, 0);
    assert_eq!((tris.len(), layers.len()), (2, 0));
    let (tris, layers, _) = bsp_geometry(&bsp, &mut raw, &[Surface::Unknown, decal]);
    assert_eq!(
        (tris.len(), layers.len()),
        (0, 2),
        "every triangle is a layer now"
    );
    assert_eq!(layers[0].material, 1);
    assert_eq!(layers[0].p, [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    // Placed like the level: the instance's transform.
    assert_eq!(layers[1].p[0], [10.0, 20.0, 30.0]);
}

fn layer_tri(p: [[f32; 3]; 3], uv: [[f32; 2]; 3], material: usize) -> crate::live_layers::LayerTri {
    crate::live_layers::LayerTri {
        p,
        n: [[0.0, 0.0, 1.0]; 3],
        uv,
        material,
    }
}

#[test]
fn layer_triangles_weld_per_material_by_position_normal_and_uv() {
    use crate::live_layers::build_layer_prims;
    let a = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let b = [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
    let uv = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    let prims = build_layer_prims(&[
        layer_tri(a, uv, 3),
        layer_tri(b, [[1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], 3),
        // The same corner with another texture coordinate is another vertex.
        layer_tri(b, [[5.0, 0.0], [1.0, 1.0], [0.0, 1.0]], 1),
        // Degenerate and non-finite triangles are dropped.
        layer_tri([[0.0; 3]; 3], uv, 1),
        layer_tri(
            [[f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uv,
            1,
        ),
    ]);
    assert_eq!(prims.iter().map(|p| p.material).collect::<Vec<_>>(), [1, 3]);
    assert_eq!(prims[1].positions.len(), 4, "two triangles sharing an edge");
    assert_eq!(prims[1].triangles(), 2);
    assert_eq!(prims[0].triangles(), 1);
    assert_eq!(prims[1].uvs[1], [1.0, 0.0]);
}

#[test]
fn a_glow_without_a_picture_is_kept_faint_but_a_decal_without_one_is_dropped() {
    use crate::live_layers::pick_bitmap;
    let glow = Layer::effect(Blend::Additive);
    let names = |v: &[&str]| {
        v.iter()
            .enumerate()
            .map(|(i, n)| (i as u32, n.to_string()))
            .collect::<Vec<_>>()
    };
    // The diffuse map first, for any layer.
    let both = names(&[r"x\bitmaps\panel_bump", r"x\bitmaps\letters"]);
    assert_eq!(pick_bitmap(&both, Layer::decal(Blend::Alpha)), Some(1));
    // A glow's picture is often named for self-illumination.
    let illum = names(&[
        r"shaders\default_bitmaps\gray_50",
        r"x\bitmaps\light_volume_illum",
    ]);
    assert_eq!(pick_bitmap(&illum, glow), Some(1));
    assert_eq!(pick_bitmap(&illum, Layer::decal(Blend::Alpha)), None);
    assert!(glow.drawn_without_texture());
    assert!(!Layer::decal(Blend::Multiply).drawn_without_texture());
    assert!(!Layer::effect(Blend::Alpha).drawn_without_texture());
}

#[test]
fn a_decal_blends_as_its_picture_says() {
    let img = |px: &[[u8; 4]]| crate::live_models::Image {
        width: px.len() as u32,
        height: 1,
        rgba: px.iter().flatten().copied().collect(),
    };
    // Opaque and centred on grey: doubles what is behind.
    let grey = img(&[
        [128, 128, 128, 255],
        [20, 20, 20, 255],
        [130, 120, 110, 255],
        [200, 200, 200, 255],
    ]);
    assert_eq!(
        Layer::decal(Blend::Multiply).with_picture(&grey),
        Layer::decal(Blend::Double)
    );
    assert_eq!(
        Layer::decal(Blend::Alpha).with_picture(&grey),
        Layer::decal(Blend::Double)
    );
    // Opaque and mostly light: multiplies.
    let light = img(&[
        [250, 250, 250, 255],
        [240, 200, 200, 255],
        [40, 40, 40, 255],
    ]);
    assert_eq!(
        Layer::decal(Blend::Alpha).with_picture(&light),
        Layer::decal(Blend::Multiply)
    );
    // With alpha: blends by it, unless cut out.
    let sign = img(&[[200, 10, 10, 255], [0, 0, 0, 0]]);
    assert_eq!(
        Layer::decal(Blend::Multiply).with_picture(&sign),
        Layer::decal(Blend::Alpha)
    );
    assert_eq!(
        Layer::decal(Blend::Test).with_picture(&sign),
        Layer::decal(Blend::Test)
    );
    // Effects are as named.
    assert_eq!(
        Layer::effect(Blend::Additive).with_picture(&grey),
        Layer::effect(Blend::Additive)
    );
    assert_eq!(Layer::decal(Blend::Double).blend_name(), "double");
}

fn texture(bitmap: u32) -> std::sync::Arc<crate::live_layers::Texture> {
    std::sync::Arc::new(crate::live_layers::Texture {
        bitmap,
        image: crate::live_models::Image {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        },
    })
}

#[test]
fn layers_are_children_of_their_bsp_with_textured_blended_materials() {
    let bsps = vec![
        (0, Ok(build_mesh(&cuboid([0.0; 3], [10.0, 10.0, 3.0])))),
        (
            1,
            Ok(build_mesh(&cuboid([20.0, 0.0, 0.0], [30.0, 10.0, 3.0]))),
        ),
    ];
    let mut level = Level::build("lvl", MapKind::Campaign, bsps).unwrap();
    level.bsp_names.insert(1, "earth_2".into());
    let uv = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    let wall = [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 0.0, 2.0]];
    let air = [[5.0, 5.0, 1.0], [6.0, 5.0, 1.0], [5.0, 5.0, 4.0]];
    // BSP materials: 0 a stain (multiply), 1 a light volume (additive), 2
    // a painted sign (alpha, but its picture failed), 3 level.
    let surfaces = [
        Surface::Layer(Layer::decal(Blend::Multiply)),
        Surface::Layer(Layer::effect(Blend::Additive)),
        Surface::Layer(Layer::decal(Blend::Alpha)),
        Surface::Unknown,
    ];
    let shaders: Vec<(TagId, String)> = (0..4)
        .map(|i| {
            (
                TagId(0xe000_0010 + i),
                format!(r"levels\shaders\shader_{i}"),
            )
        })
        .collect();
    let tris = [
        layer_tri(wall, uv, 0),
        layer_tri(air, uv, 1),
        layer_tri(wall, uv, 2),
        layer_tri(air, uv, 3),
    ];
    let mut asked = Vec::new();
    let dropped = level.add_layers(0, &tris, &shaders, &surfaces, |id, layer| {
        asked.push(id.0);
        // The stain and the glow share one bitmap; the sign has none.
        (layer.blend != Blend::Alpha).then(|| texture(7))
    });
    assert_eq!(dropped, 2, "the picture-less sign and the level triangle");
    assert_eq!(asked, [0xe000_0010, 0xe000_0011, 0xe000_0012]);
    // The same shader in another BSP shares the material and asks no more.
    level.add_layers(1, &[layer_tri(wall, uv, 0)], &shaders, &surfaces, |_, _| {
        panic!("cached")
    });
    assert_eq!(level.layer_materials.len(), 2);
    assert_eq!(level.layer_materials[0].name, "shader_0");

    let glb = level.glb();
    gltf::Gltf::from_slice(&glb).expect("valid glTF");
    let (doc, bin) = parse_glb(&glb);
    assert_eq!(
        node_names(&doc),
        ["bsp_0", "decals_0", "effects_0", "bsp_1", "decals_1"]
    );
    assert_eq!(doc["nodes"][0]["children"], serde_json::json!([1, 2]));
    assert_eq!(doc["nodes"][1]["extras"]["decals"], true);
    assert_eq!(doc["nodes"][2]["extras"]["effects"], true);
    assert_eq!(doc["nodes"][3]["extras"]["name"], "earth_2");
    assert!(doc["nodes"][0]["extras"].get("name").is_none());
    let material = |node: usize| {
        let mesh = doc["nodes"][node]["mesh"].as_u64().unwrap() as usize;
        let prim = &doc["meshes"][mesh]["primitives"][0];
        assert!(prim["attributes"].get("TEXCOORD_0").is_some());
        assert_eq!(
            doc["accessors"][prim["indices"].as_u64().unwrap() as usize]["componentType"],
            5125
        );
        doc["materials"][prim["material"].as_u64().unwrap() as usize].clone()
    };
    let stain = material(1);
    assert_eq!(stain["name"], "shader_0");
    assert_eq!(
        stain["extras"],
        serde_json::json!({"layer": "decal", "blend": "multiply"})
    );
    assert_eq!(stain["alphaMode"], "BLEND");
    assert!(stain["extensions"].get("KHR_materials_unlit").is_some());
    assert!(stain.get("doubleSided").is_none());
    let glow = material(2);
    assert_eq!(glow["extras"]["blend"], "additive");
    assert_eq!(glow["doubleSided"], true);
    assert_eq!(material(4), stain, "shared across BSPs");
    // One image for the shared bitmap, a decodable PNG.
    assert_eq!(doc["images"].as_array().unwrap().len(), 1);
    assert_eq!(
        stain["pbrMetallicRoughness"]["baseColorTexture"],
        glow["pbrMetallicRoughness"]["baseColorTexture"]
    );
    let view = &doc["bufferViews"][doc["images"][0]["bufferView"].as_u64().unwrap() as usize];
    let at = view["byteOffset"].as_u64().unwrap() as usize;
    assert_eq!(&bin[at..at + 4], b"\x89PNG");
    assert_eq!(
        doc["extensionsUsed"],
        serde_json::json!(["KHR_materials_unlit"])
    );
    // Class materials are untouched and the level mesh has no layers.
    assert!(
        doc["materials"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "wall")
    );
}

fn sky_header() -> Vec<u8> {
    use crate::live_sky::*;
    let mut b = vec![0u8; 0x200];
    put32(&mut b, SKY_MODEL + 4, 0xe17a_0005);
    put32(&mut b, SKY_SCALE, 150f32.to_bits());
    for (k, v) in [0.2f32, 0.3, 0.4].iter().enumerate() {
        put32(&mut b, SKY_CLEAR + 4 * k, v.to_bits());
    }
    // Atmospheric fog: one element at 0x100.
    put32(&mut b, SKY_ATMOSPHERIC_FOG, 1);
    put32(&mut b, SKY_ATMOSPHERIC_FOG + 4, BASE + 0x100);
    for (k, v) in [0.5f32, 0.6, 0.7, 0.55, 70.0, 180.0].iter().enumerate() {
        put32(&mut b, 0x100 + 4 * k, v.to_bits());
    }
    b
}

#[test]
fn a_sky_tag_gives_its_model_clear_colour_and_fog() {
    use crate::live_sky::*;
    let b = sky_header();
    let view = TagView::new(&b, BASE);
    let f = sky_fields(&view, &b[..SKY_HEADER]);
    assert_eq!(f.model, Some(TagId(0xe17a_0005)));
    assert_eq!(f.scale, 150.0);
    assert_eq!(f.clear, Some([0.2, 0.3, 0.4]));
    assert_eq!(
        f.fog,
        Some(SkyFog {
            colour: [0.5, 0.6, 0.7],
            density: 0.55,
            start: 70.0,
            end: 180.0
        })
    );
    // No model, black clear colour, a fog with nothing to it: none of them.
    let mut b = sky_header();
    put32(&mut b, SKY_MODEL + 4, u32::MAX);
    for k in 0..3 {
        put32(&mut b, SKY_CLEAR + 4 * k, 0);
    }
    put32(&mut b, 0x100 + 0xc, 0);
    let view = TagView::new(&b, BASE);
    let f = sky_fields(&view, &b[..SKY_HEADER]);
    assert_eq!((f.model, f.clear, f.fog), (None, None, None));
    // The secondary fog stands in when the atmospheric one says nothing.
    put32(&mut b, SKY_SECONDARY_FOG, 1);
    put32(&mut b, SKY_SECONDARY_FOG + 4, BASE + 0x140);
    for (k, v) in [0.1f32, 0.1, 0.1, 0.3, 20.0, 100.0].iter().enumerate() {
        put32(&mut b, 0x140 + 4 * k, v.to_bits());
    }
    let view = TagView::new(&b, BASE);
    assert_eq!(sky_fields(&view, &b[..SKY_HEADER]).fog.unwrap().end, 100.0);
    // An end before the start is no fog.
    let mut e = [0u8; 0x18];
    for (k, v) in [0.5f32, 0.5, 0.5, 0.5, 50.0, 10.0].iter().enumerate() {
        e[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
    }
    assert_eq!(fog_element(&e), None);
}

#[test]
fn sky_materials_blend_by_template_words_and_the_sky_is_in_the_extras() {
    use crate::live_sky::*;
    assert_eq!(
        sky_blend(r"shaders\shader_templates\transparent\sky_two_add_clouds"),
        "additive"
    );
    assert_eq!(
        sky_blend(r"shaders\shader_templates\transparent\sky_two_alpha_clouds"),
        "alpha"
    );
    assert_eq!(
        sky_blend(r"shaders\shader_templates\opaque\tex_bump"),
        "opaque"
    );
    let mut level = Level::build(
        "mp",
        MapKind::Multiplayer,
        vec![(0, Ok(build_mesh(&cuboid([0.0; 3], [1.0; 3]))))],
    )
    .unwrap();
    level.sky = Some(SkyInfo {
        name: "beach".into(),
        glb: Some("sky.glb".into()),
        clear: None,
        fog: Some(SkyFog {
            colour: [0.5, 0.6, 0.7],
            density: 0.5,
            start: 1.0,
            end: 40.0,
        }),
        scale: 1.0,
    });
    let (doc, _) = parse_glb(&level.glb());
    let sky = &doc["asset"]["extras"]["sky"];
    assert_eq!(sky["glb"], "sky.glb");
    assert_eq!(sky["fog"]["end"], 40.0);
    assert!(sky.get("clear").is_none());
    let back: SkyInfo = serde_json::from_value(sky.clone()).unwrap();
    assert_eq!(back, level.sky.clone().unwrap());
}
