//! One section's grown rewrite, on invented sections.
use super::*;
use crate::import::section_source;
use map::{RawPointer, ResourceFile, TagId};
use mesh_edit::{Mesh, MeshOp, SelectMode};
use tags::geometry::{GeometryBlockInfo, LoadedBlock, Section, SectionLayout, decode_section};
use tags::geometry_write::ResourceData;
use tags::model::MODEL_SECTION;
use tags::structure::STRUCTURE_SECTION;

fn b16(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn b32(v: impl IntoIterator<Item = u32>) -> Vec<u8> {
    v.into_iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn res(kind: ResourceKind, field: u16, secondary: u16, bytes: Vec<u8>) -> ResourceData {
    ResourceData {
        kind,
        unknown_1: 2,
        field,
        secondary,
        declared_size: bytes.len() as u32,
        bytes: Some(bytes),
    }
}

fn part(count: u16, flags: u16, material: u16) -> Vec<u8> {
    let mut p = vec![0u8; 0x48];
    p[0..2].copy_from_slice(&2u16.to_le_bytes());
    p[2..4].copy_from_slice(&flags.to_le_bytes());
    p[4..6].copy_from_slice(&material.to_le_bytes());
    p[8..10].copy_from_slice(&count.to_le_bytes());
    p[0xc..0xe].copy_from_slice(&1u16.to_le_bytes());
    p
}

fn descriptors(kinds: &[(u8, u8)]) -> Vec<u8> {
    let mut d = vec![0u8; 0x20 * kinds.len()];
    for (i, (k, s)) in kinds.iter().enumerate() {
        d[i * 0x20] = *k;
        d[i * 0x20 + 1] = *s;
    }
    d
}

fn info(e: &EncodedSection) -> GeometryBlockInfo {
    GeometryBlockInfo {
        block: Some(RawPointer {
            file: ResourceFile::Local,
            offset: 0x8000,
        }),
        block_size: e.block.len() as u32,
        section_size: e.section_size,
        resource_size: e.resource_size,
        resources: e.resources.clone(),
        owner: TagId(0xe100_0004),
        runtime_block_offset: 0x24,
        runtime_loaded: 0,
        unknown_1f: 1,
        runtime_cache_handle: u32::MAX,
    }
}

fn decode(layout: SectionLayout, e: &EncodedSection) -> Section {
    let block = LoadedBlock::parse(&info(e), e.block.clone()).unwrap();
    decode_section(block, &layout, None).unwrap()
}

fn section(layout: SectionLayout, resources: Vec<ResourceData>) -> (Section, GeometryBlockInfo) {
    let data = SectionData::new(vec![0; layout.section_size as usize], resources);
    let e = encode_section(&layout, &data).unwrap();
    (decode(layout, &e), info(&e))
}

/// A floor quad, 4 units square, with an incident-light stream (`0x30`,
/// not interpreted), and the visibility code and table of its one part.
fn floor() -> (Section, GeometryBlockInfo) {
    let corners = [[0.0f32, 0.0], [4.0, 0.0], [0.0, 4.0], [4.0, 4.0]];
    let positions = b32(corners
        .iter()
        .flat_map(|c| [c[0], c[1], 0.0])
        .map(f32::to_bits));
    let uvs = b32([0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0].map(f32::to_bits));
    let frame = b32((0..4).flat_map(|_| [511u32 << 22, 0x3ff, 0x3ff << 11]));
    let light = b32([0x1111_1111u32, 0x2222_2222, 0x3333_3333, 0x4444_4444]);
    let (code, table) = visibility::build(&[[0.0, 4.0, 0.0, 4.0, 0.0, 0.0]]);
    let vb = ResourceKind::VertexBuffer;
    section(
        STRUCTURE_SECTION,
        vec![
            res(ResourceKind::Block, 0, 0x48, part(6, 0xb, 0)),
            res(ResourceKind::Block, 8, 8, b16(&[0, 6, 0xffff, 0])),
            res(ResourceKind::Block, 0x20, 2, b16(&[0, 1, 2, 1, 3, 2])),
            res(ResourceKind::Data, visibility::CODE_FIELD, 0xffff, code),
            res(ResourceKind::Block, visibility::TABLE_FIELD, 2, table),
            res(
                ResourceKind::Block,
                0x38,
                0x20,
                descriptors(&[(0x01, 12), (0x18, 8), (0x1b, 12), (0x30, 4)]),
            ),
            res(vb, 0x38, 0, positions),
            res(vb, 0x38, 1, uvs),
            res(vb, 0x38, 2, frame),
            res(vb, 0x38, 3, light),
        ],
    )
}

fn structure_options() -> Options<'static> {
    Options {
        compression: None,
        requantise: false,
        sky: false,
        visibility: true,
    }
}

/// The section's source and its export after `ops`.
fn edited(
    s: &Section,
    info: &GeometryBlockInfo,
    layout: SectionLayout,
    compression: Option<&Compression>,
    part_bounds: bool,
    ops: &[MeshOp],
) -> (SectionSource, Export) {
    let source = section_source(s, compression).unwrap();
    let src =
        SectionSource::new(s, info, layout, source.positions.clone(), part_bounds).unwrap();
    let (mut mesh, _) = Mesh::from_triangles(source).unwrap();
    mesh.apply_all(ops).unwrap();
    (src, mesh.export())
}

fn extrude_all() -> Vec<MeshOp> {
    vec![
        MeshOp::Mode {
            mode: SelectMode::Face,
        },
        MeshOp::SelectAll,
        MeshOp::ExtrudeFaces {
            offset: [0.0, 0.0, 1.0],
        },
    ]
}

#[test]
fn an_extruded_floor_decodes_to_its_export_with_a_checked_visibility_code() {
    let (s, info) = floor();
    let (src, e) = edited(&s, &info, STRUCTURE_SECTION, None, true, &extrude_all());
    assert!(e.positions.len() > 4);
    let g = grow(&src, &e, &structure_options()).unwrap();
    assert!(!g.fits(&src));
    let back = decode(STRUCTURE_SECTION, &g.encoded);
    assert_eq!(back.vertex_count(), e.positions.len());
    assert_eq!(back.vertices(None).unwrap().positions, e.positions);
    let triangles: Vec<[u32; 3]> = back
        .parts
        .iter()
        .flat_map(|p| back.part_triangles(p))
        .map(|t| t.map(u32::from))
        .collect();
    assert_eq!(triangles, e.triangles);
    assert_eq!((back.parts.len(), back.subparts.len()), (1, 1));
    assert!(back.parts[0].is_triangle_list());
    // The part's box holds the top.
    let p = &back.parts[0].raw;
    assert_eq!(f32::from_le_bytes(p[0x44..0x48].try_into().unwrap()), 1.0);
    // The visibility code and table, as the converter's check reads them.
    let block = back.block();
    let code = block
        .resource_by_key(ResourceKind::Data, visibility::CODE_FIELD, 0)
        .unwrap();
    let table = block
        .resource_by_key(ResourceKind::Block, visibility::TABLE_FIELD, 0)
        .unwrap();
    h1convert::structure::check_visibility(back.subparts.len(), code, table).unwrap();
    // A source vertex keeps its uninterpreted bytes; a new one copies the
    // vertex it was extruded from.
    let light = back.vertex_buffers.iter().find(|b| b.type_id == 0x30).unwrap();
    let rows: Vec<&[u8]> = back.data(light).chunks_exact(4).collect();
    assert_eq!(rows[0], 0x1111_1111u32.to_le_bytes());
    for i in 4..e.positions.len() {
        let from = e.vert_ids[i].and_then(|v| (0..4).find(|&j| e.vert_ids[j] == Some(v)));
        if let Some(j) = from {
            assert_eq!(rows[i], rows[j], "vertex {i}");
        }
    }
    // The section info counts follow.
    let info = g.structure_info();
    assert_eq!(info[0], (0, e.positions.len() as u16));
    assert_eq!(info[1], (2, e.triangles.len() as u16));
    assert_eq!(info[2], (4, 1));
}

#[test]
fn an_unedited_section_grows_into_its_own_block_byte_for_byte() {
    // The code and table are rebuilt from the same box, and every vertex
    // keeps its bytes, so the encoding is the source block and is written
    // in place.
    let (s, info) = floor();
    let (src, e) = edited(&s, &info, STRUCTURE_SECTION, None, true, &[]);
    let g = grow(&src, &e, &structure_options()).unwrap();
    assert!(g.fits(&src));
    assert_eq!(g.encoded.block, src.bytes);
}

#[test]
fn a_face_in_a_new_material_or_a_sky_strip_is_refused() {
    let (s, info) = floor();
    let (src, mut e) = edited(&s, &info, STRUCTURE_SECTION, None, true, &extrude_all());
    e.materials[0] = 7;
    let why = grow(&src, &e, &structure_options()).err().expect("grow refuses");
    assert!(why.contains("material"), "{why}");

    // A model's strip part, in a sky's model.
    let mut positions = Vec::new();
    for (x, y) in [(-32767i16, -32767i16), (32767, -32767), (-32767, 32767), (32767, 32767)] {
        for c in [x, y, -32767] {
            positions.extend(c.to_le_bytes());
        }
        positions.extend([0u8, 0]);
    }
    let (m, minfo) = section(
        MODEL_SECTION,
        vec![
            res(ResourceKind::Block, 0, 0x48, part(4, 0, 0)),
            res(ResourceKind::Block, 8, 8, b16(&[0, 0, 0x1234, 0])),
            res(ResourceKind::Block, 0x20, 2, b16(&[0, 1, 2, 3])),
            res(ResourceKind::Block, 0x38, 0x20, descriptors(&[(0x04, 8)])),
            res(ResourceKind::VertexBuffer, 0x38, 0, positions),
        ],
    );
    let boxed = Compression {
        position: [-1.0, 1.0, -1.0, 1.0, 0.0, 2.0],
        texcoord: [0.0, 1.0, 0.0, 1.0],
    };
    let (src, e) = edited(&m, &minfo, MODEL_SECTION, Some(&boxed), false, &extrude_all());
    let sky = Options {
        compression: Some(&boxed),
        requantise: false,
        sky: true,
        visibility: false,
    };
    let why = grow(&src, &e, &sky).err().unwrap();
    assert!(why.contains("strip"), "{why}");
    // Not a sky's: the strip becomes a list, and new vertices take their
    // template's node.
    let object = Options { sky: false, ..sky };
    let g = grow(&src, &e, &object).unwrap();
    let back = decode(MODEL_SECTION, &g.encoded);
    assert!(back.parts.iter().all(|p| p.is_triangle_list()));
    let v = back.vertices(Some(&boxed)).unwrap();
    assert_eq!(v.positions.len(), e.positions.len());
    for (a, b) in v.positions.iter().zip(&e.positions) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-3, "{a:?} {b:?}");
        }
    }
}

#[test]
fn a_skin_keeps_its_strongest_influences_summing_to_255() {
    let skin = Skin {
        nodes: [1, 2, 3, 4],
        weights: [10, 100, 45, 100],
    };
    let (nodes, weights) = fit(skin, 2);
    assert_eq!(&nodes[..2], &[2, 4]);
    assert_eq!(weights[..2].iter().map(|&w| u32::from(w)).sum::<u32>(), 255);
    assert_eq!(weights[2..], [0, 0]);
    let (nodes, weights) = fit(skin, 4);
    assert_eq!(nodes, [2, 4, 3, 1]);
    assert_eq!(weights.iter().map(|&w| u32::from(w)).sum::<u32>(), 255);
}
