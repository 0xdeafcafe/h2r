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
        let (idx, ni) = accessor(&doc, &bin, prim["indices"].as_u64().unwrap());
        assert_eq!(ni % 3, 0);
        triangles += ni / 3;
        assert!(
            idx.as_chunks::<4>()
                .0
                .iter()
                .all(|b| (u32::from_le_bytes(*b) as usize) < n)
        );
    }
    assert_eq!(triangles, 12);
    // Accessor min/max in glTF axes.
    let first = &doc["accessors"][0];
    assert!(first["min"].is_array() && first["max"].is_array());
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
        },
    );
    let j = serde_json::to_value(&manifest).unwrap();
    assert_eq!(
        j["maps"]["stem"],
        serde_json::json!({"geometry": "map/stem/map.glb", "views": "map/stem/views.json"})
    );
    let back: crate::live_art::Manifest = serde_json::from_value(j).unwrap();
    assert_eq!(back, manifest);
    // An older manifest without maps still reads.
    let old: crate::live_art::Manifest = serde_json::from_value(none).unwrap();
    assert!(old.maps.is_empty());
}

// ---- The owner's disc ----------------------------------------------------

const CHECK_DIR: &str = "/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc/geometry-check";

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
    let check = Path::new(CHECK_DIR);
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
            "{:<16} src {:>7} tris {:>7} verts {:>7} glb {:>5} KiB failed {} shaders {}/{} load {:>6.0?} render {:>6.0?} {:?}\n",
            s.stem,
            s.source_triangles,
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
    for m in disc.maps_of(CacheType::Multiplayer) {
        let (mut map, _) = disc.open(m).unwrap();
        for bsp in scenario_bsps(&mut map).unwrap() {
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

    // The top view without the cutaway, side by side with it, at half
    // size, for review.
    let pairs = check.join("cutaway");
    let _ = fs::remove_dir_all(&pairs);
    fs::create_dir_all(&pairs).unwrap();
    for m in disc.maps_of(CacheType::Multiplayer) {
        let mut stats = MapStats::default();
        let Ok(level) = load_level(&disc, m, &mut stats) else {
            continue;
        };
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
