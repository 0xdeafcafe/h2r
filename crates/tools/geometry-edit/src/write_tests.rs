//! One section's in-place rewrite, on invented sections.
use super::*;
use crate::import::section_source;
use map::{ResourceFile, TagId};
use mesh_edit::{Mesh, MeshOp, SelectHow};
use tags::geometry::{LoadedBlock, decode_section};
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

fn part(count: u16, flags: u16, bounds: [f32; 6]) -> Vec<u8> {
    let mut p = vec![0u8; 0x48];
    p[0..2].copy_from_slice(&2u16.to_le_bytes());
    p[2..4].copy_from_slice(&flags.to_le_bytes());
    p[8..10].copy_from_slice(&count.to_le_bytes());
    p[0xc..0xe].copy_from_slice(&1u16.to_le_bytes());
    p[0x30..0x48].copy_from_slice(&floats(&bounds));
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

/// Encode `resources` for `layout`, then read the block back as the map
/// reader does.
fn section(layout: SectionLayout, resources: Vec<ResourceData>) -> (Section, GeometryBlockInfo) {
    let data = SectionData::new(vec![0; layout.section_size as usize], resources);
    let e = encode_section(&layout, &data).unwrap();
    let info = GeometryBlockInfo {
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
    };
    let block = LoadedBlock::parse(&info, e.block).unwrap();
    (decode_section(block, &layout, None).unwrap(), info)
}

/// A floor quad, 4 units square, `+z` frame, as a structure section.
fn floor() -> (Section, GeometryBlockInfo) {
    let corners = [[0.0f32, 0.0], [4.0, 0.0], [0.0, 4.0], [4.0, 4.0]];
    let positions = b32(corners
        .iter()
        .flat_map(|c| [c[0], c[1], 0.0])
        .map(f32::to_bits));
    let uvs = b32([0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0].map(f32::to_bits));
    let frame = b32((0..4).flat_map(|_| [511u32 << 22, 0x3ff, 0x3ff << 11]));
    let vb = ResourceKind::VertexBuffer;
    section(
        STRUCTURE_SECTION,
        vec![
            res(
                ResourceKind::Block,
                0,
                0x48,
                part(6, 0xb, [0.0, 4.0, 0.0, 4.0, 0.0, 0.0]),
            ),
            res(ResourceKind::Block, 8, 8, b16(&[0, 6, 0xffff, 0])),
            res(ResourceKind::Block, 0x20, 2, b16(&[0, 1, 2, 1, 3, 2])),
            res(
                ResourceKind::Block,
                0x38,
                0x20,
                descriptors(&[(0x01, 12), (0x18, 8), (0x1b, 12)]),
            ),
            res(vb, 0x38, 0, positions),
            res(vb, 0x38, 1, uvs),
            res(vb, 0x38, 2, frame),
        ],
    )
}

/// The same quad as a model section: s16 positions with one node each in
/// the box x, y in [-1, 1], z in [0, 1].
const BOX: [f32; 6] = [-1.0, 1.0, -1.0, 1.0, 0.0, 1.0];
fn model_quad() -> (Section, GeometryBlockInfo) {
    let mut positions = Vec::new();
    for (x, y) in [
        (-32767i16, -32767i16),
        (32767, -32767),
        (-32767, 32767),
        (32767, 32767),
    ] {
        for c in [x, y, -32767] {
            positions.extend(c.to_le_bytes());
        }
        positions.extend([0u8, 0]);
    }
    let vb = ResourceKind::VertexBuffer;
    section(
        MODEL_SECTION,
        vec![
            res(ResourceKind::Block, 0, 0x48, part(6, 0xb, [0.0; 6])),
            res(ResourceKind::Block, 8, 8, b16(&[0, 6, 0xffff, 0])),
            res(ResourceKind::Block, 0x20, 2, b16(&[0, 1, 2, 1, 3, 2])),
            res(ResourceKind::Block, 0x38, 0x20, descriptors(&[(0x04, 8)])),
            res(vb, 0x38, 0, positions),
        ],
    )
}

fn compression() -> Compression {
    Compression {
        position: BOX,
        texcoord: [0.0, 1.0, 0.0, 1.0],
    }
}

/// The mesh of a section, and its source.
fn opened(
    s: &Section,
    info: &GeometryBlockInfo,
    layout: SectionLayout,
    c: Option<&Compression>,
) -> (Mesh, SectionSource) {
    let source = section_source(s, c).unwrap();
    let src = SectionSource::new(
        s,
        info,
        layout,
        source.positions.clone(),
        layout.section_size == 0x44,
    )
    .unwrap();
    (Mesh::from_triangles(source).unwrap().0, src)
}

fn lift(mesh: &mut Mesh, at: [f32; 3], dz: f32) {
    let v = mesh.vert_ids().find(|&v| mesh.position(v) == at).unwrap();
    mesh.apply(&MeshOp::Select {
        ids: vec![v.0],
        how: SelectHow::Set,
    })
    .unwrap();
    mesh.apply(&MeshOp::Transform {
        matrix: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, dz],
        ],
        pivot: None,
    })
    .unwrap();
}

#[test]
fn an_unedited_section_writes_back_byte_for_byte() {
    let (s, info) = floor();
    let (mut mesh, src) = opened(&s, &info, STRUCTURE_SECTION, None);
    // A selection is not an edit.
    mesh.apply(&MeshOp::SelectAll).unwrap();
    let e = mesh.export();
    assert!(e.corners.iter().all(|c| c.dirty == 0));
    let data = rewrite(&src, &e.positions, Some(&e.corners), None, false).unwrap();
    assert_eq!(block_runs(&src, &data).unwrap(), Vec::new());
}

#[test]
fn a_moved_vertex_changes_only_its_streams_and_its_part_bounds() {
    let (s, info) = floor();
    let (mut mesh, src) = opened(&s, &info, STRUCTURE_SECTION, None);
    lift(&mut mesh, [4.0, 4.0, 0.0], 1.0);
    let e = mesh.export();
    let data = rewrite(&src, &e.positions, Some(&e.corners), None, false).unwrap();
    let runs = block_runs(&src, &data).unwrap();
    assert!(!runs.is_empty());
    // Every changed byte is a position, a frame word or the part's
    // sphere and box.
    let block = s.block();
    let range = |kind, field, index| {
        let r = block.resource_by_key(kind, field, index).unwrap();
        let start = r.as_ptr() as usize - block.bytes().as_ptr() as usize;
        start..start + r.len()
    };
    let positions = range(ResourceKind::VertexBuffer, 0x38, 0);
    let frames = range(ResourceKind::VertexBuffer, 0x38, 2);
    let parts = range(ResourceKind::Block, 0, 0);
    let bounds = parts.start + 0x10..parts.start + 0x48;
    for (at, bytes) in &runs {
        for i in *at..*at + bytes.len() {
            assert!(
                positions.contains(&i) || frames.contains(&i) || bounds.contains(&i),
                "byte {i:#x} changed outside the vertex and bound fields"
            );
        }
    }
    // Only vertex 3's position moved.
    for (at, bytes) in &runs {
        if positions.contains(at) {
            assert!(*at >= positions.start + 36 && at + bytes.len() <= positions.start + 48);
        }
    }
    let mut written = src.bytes.clone();
    for (at, bytes) in runs {
        written[at..at + bytes.len()].copy_from_slice(&bytes);
    }
    let back = decode_section(
        LoadedBlock::parse(&info, written).unwrap(),
        &STRUCTURE_SECTION,
        None,
    )
    .unwrap();
    let v = back.vertices(None).unwrap();
    assert_eq!(v.positions[3], [4.0, 4.0, 1.0]);
    assert_eq!(v.positions[0], [0.0, 0.0, 0.0]);
    assert_eq!(back.indices, s.indices);
    let p = &back.parts[0].raw;
    let f = |o: usize| f32::from_bits(u32::from_le_bytes(p[o..o + 4].try_into().unwrap()));
    assert_eq!(
        [f(0x30), f(0x34), f(0x38), f(0x3c), f(0x40), f(0x44)],
        [0.0, 4.0, 0.0, 4.0, 0.0, 1.0]
    );
    assert_eq!([f(0x10), f(0x14), f(0x18)], [2.0, 2.0, 0.5]);
    assert!(f(0x2c) > 0.0);
}

#[test]
fn a_vertex_outside_the_box_needs_the_box_grown_then_every_vertex_is_requantised() {
    let c = compression();
    let (s, info) = model_quad();
    let (mut mesh, src) = opened(&s, &info, MODEL_SECTION, Some(&c));
    lift(&mut mesh, [1.0, 1.0, 0.0], 2.0);
    let e = mesh.export();
    let err = rewrite(&src, &e.positions, Some(&e.corners), Some(&c), false).unwrap_err();
    assert!(err.contains("outside the compression box"), "{err}");
    let grown = Compression {
        position: [-1.0, 1.0, -1.0, 1.0, 0.0, 2.0],
        ..c
    };
    let data = rewrite(&src, &e.positions, Some(&e.corners), Some(&grown), true).unwrap();
    let runs = block_runs(&src, &data).unwrap();
    let mut written = src.bytes.clone();
    for (at, bytes) in runs {
        written[at..at + bytes.len()].copy_from_slice(&bytes);
    }
    let back = decode_section(
        LoadedBlock::parse(&info, written).unwrap(),
        &MODEL_SECTION,
        None,
    )
    .unwrap();
    let v = back.vertices(Some(&grown)).unwrap();
    let want = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [-1.0, 1.0, 0.0],
        [1.0, 1.0, 2.0],
    ];
    for (got, want) in v.positions.iter().zip(want) {
        for k in 0..3 {
            assert!((got[k] - want[k]).abs() < 1e-4, "{got:?} vs {want:?}");
        }
    }
    // The node bytes are kept.
    assert_eq!(v.node_indices, s.vertices(Some(&c)).unwrap().node_indices);
    // Model parts carry no bounds this write keeps.
    assert_eq!(back.parts[0].raw, s.parts[0].raw);
}

#[test]
fn a_count_change_is_refused() {
    let (s, info) = floor();
    let (_, src) = opened(&s, &info, STRUCTURE_SECTION, None);
    let mut positions = src.positions.clone();
    positions.push([0.0; 3]);
    assert!(rewrite(&src, &positions, None, None, false).is_err());
}
