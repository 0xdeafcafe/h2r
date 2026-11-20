//! Synthetic-only tests for render-model geometry. No game data.
use super::*;
use crate::geometry::{
    PART_FLAG_TRIANGLE_LIST, VertexKind, list_triangles, strip_triangles, unpack_normpacked3,
};
use crate::wire::BinaryRecord;
use map::FourCC;

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// Payload file offset and virtual base of the single synthetic tag.
const TAG: usize = 0x1200;
const VA: u32 = 0x8100_0000;
const SECTION: usize = TAG + 0x160;
const RESOURCES: usize = TAG + 0x240;
const PART: usize = 8 + 0x70 + 4; // raw offset of the part record

/// A minimal cache containing one `mode` tag, built like the crate's other
/// synthetic maps (header, index, one group, one tag at 0x1200).
fn map_bytes() -> Vec<u8> {
    let mut b = vec![0; 0x2000];
    b[..4].copy_from_slice(b"daeh");
    b[0x7fc..0x800].copy_from_slice(b"toof");
    b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
    for (at, n) in [
        (4, 8),
        (8, 0x2000),
        (0x10, 0x1000),
        (0x14, 0x200),
        (0x18, 0x400),
    ] {
        put(&mut b, at, n);
    }
    for (at, n) in [
        (0, 0x80000020),
        (4, 1),
        (8, 0x8000002c),
        (12, u32::MAX),
        (16, u32::MAX),
        (24, 1),
        (28, FourCC(*b"tags").as_u32()),
    ] {
        put(&mut b, 0x1000 + at, n);
    }
    put(&mut b, 0x1020, FourCC(*b"mode").as_u32());
    put(&mut b, 0x1024, u32::MAX);
    put(&mut b, 0x1028, u32::MAX);
    for (at, n) in [
        (0, FourCC(*b"mode").as_u32()),
        (4, 0xa0000000),
        (8, VA),
        (12, 0x400),
    ] {
        put(&mut b, 0x102c + at, n);
    }
    let t = TAG;
    put(&mut b, t, u32::MAX); // name
    // compression, regions, sections, nodes, materials
    for (at, count, off) in [
        (0x14, 1, 0x100),
        (0x1c, 1, 0x140),
        (0x24, 1, 0x160),
        (0x48, 1, 0x1c0),
        (0x60, 1, 0x220),
    ] {
        put(&mut b, t + at, count);
        put(&mut b, t + at + 4, VA + off);
    }
    // Bounds: x [-1, 1], y [-2, 2], z [0, 4], u [0, 1], v [0, 1].
    for (i, f) in [-1f32, 1., -2., 2., 0., 4., 0., 1., 0., 1.]
        .iter()
        .enumerate()
    {
        put(&mut b, t + 0x100 + i * 4, f.to_bits());
    }
    // Region -> one permutation whose highest LOD is section 0.
    put(&mut b, t + 0x140, u32::MAX);
    put16(&mut b, t + 0x144, 0xffff);
    put(&mut b, t + 0x148, 1);
    put(&mut b, t + 0x14c, VA + 0x150);
    put(&mut b, t + 0x150, u32::MAX);
    for i in 0..5 {
        put16(&mut b, t + 0x154 + i * 2, 0xffff);
    }
    put16(&mut b, t + 0x15e, 0);
    // Node: no links.
    put(&mut b, t + 0x1c0, u32::MAX);
    for at in [4, 6, 8] {
        put16(&mut b, t + 0x1c0 + at, 0xffff);
    }
    // Material: null references.
    for at in [0, 4, 8, 12] {
        put(&mut b, t + 0x220 + at, u32::MAX);
    }
    b
}

/// Raw block for a 4-vertex quad strip, plus the section record and its
/// resource table written into `map`.
fn raw_block(map: &mut [u8]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut resources = Vec::new();
    let mut add = |body: &mut Vec<u8>, kind: u8, loc: u16, element: u16, data: &[u8]| {
        body.extend_from_slice(b"crsr");
        resources.push((kind, loc, element, data.len() as u32, body.len() as u32));
        body.extend_from_slice(data);
    };
    let mut part = vec![0u8; 0x48];
    put16(&mut part, 0, 2); // drawn
    put16(&mut part, 4, 0); // material 0
    put16(&mut part, 8, 4); // strip length
    add(&mut body, 0, 0, 0x48, &part);
    let strip: Vec<u8> = [0u16, 1, 2, 3]
        .iter()
        .flat_map(|i| i.to_le_bytes())
        .collect();
    add(&mut body, 0, 0x20, 2, &strip);
    let mut vbs = vec![0u8; 0x40];
    vbs[0] = 0x02;
    vbs[1] = 6;
    vbs[0x20] = 0x19;
    vbs[0x21] = 4;
    add(&mut body, 0, 0x38, 0x20, &vbs);
    let pos: Vec<u8> = [
        [-32767i16, -32767, -32767],
        [32767, -32767, 0],
        [-32767, 32767, 0],
        [32767, 32767, 32767],
    ]
    .iter()
    .flatten()
    .flat_map(|v| v.to_le_bytes())
    .collect();
    add(&mut body, 2, 0x38, 0, &pos);
    let uv: Vec<u8> = [[-32767i16, 0], [32767, 0], [0, 32767], [0, -32767]]
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    add(&mut body, 2, 0x38, 1, &uv);

    let mut header = vec![0u8; 0x70];
    put(&mut header, 0, 1); // parts
    put(&mut header, 0x20, 4); // strip indices
    put(&mut header, 0x38, 2); // vertex buffers
    let mut raw = b"hklb".to_vec();
    raw.extend_from_slice(&(body.len() as u32).to_le_bytes());
    raw.extend_from_slice(&header);
    raw.extend_from_slice(&body);
    raw.extend_from_slice(b"fklb");

    let s = SECTION;
    put16(map, s + 4, 4); // vertices
    put16(map, s + 6, 2); // triangles
    put16(map, s + 8, 1); // parts
    put16(map, s + 0x18, 1); // no vertex nodes
    put16(map, s + 0x1a, 3); // compressed position + texcoord
    put(map, s + 0x38, 0); // local file, offset 0
    put(map, s + 0x3c, raw.len() as u32);
    put(map, s + 0x40, 0x70);
    put(map, s + 0x44, body.len() as u32);
    put(map, s + 0x48, resources.len() as u32);
    put(map, s + 0x4c, VA + 0x240);
    put(map, s + 0x50, 0xa0000000); // owner: this tag
    put16(map, s + 0x54, 0xfff8); // runtime block at -8 (section +0x30)
    map[s + 0x57] = 1;
    put(map, s + 0x58, u32::MAX); // no cache handle
    for (i, (kind, loc, element, size, offset)) in resources.into_iter().enumerate() {
        let r = RESOURCES + i * 16;
        map[r] = kind;
        map[r + 1] = 2;
        put16(map, r + 2, loc);
        put16(map, r + 4, loc);
        put16(map, r + 6, element);
        put(map, r + 8, size);
        put(map, r + 12, offset);
    }
    raw
}

struct Mem(Vec<u8>);
impl RawSource for Mem {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> Result<Vec<u8>> {
        if p.file != map::ResourceFile::Local {
            return Err(Error::MissingResource(p.file));
        }
        self.0
            .get(p.offset as usize..p.offset as usize + size as usize)
            .map(<[u8]>::to_vec)
            .ok_or(Error::Invalid("synthetic raw range"))
    }
}

fn fixture() -> (Vec<u8>, Vec<u8>) {
    let mut m = map_bytes();
    let raw = raw_block(&mut m);
    (m, raw)
}
fn read(m: Vec<u8>, raw: Vec<u8>) -> Result<RenderModelGeometry> {
    let mut map = Map::from_reader(std::io::Cursor::new(m))?;
    RenderModelGeometry::read(&mut map, TagId(0xa0000000), &mut Mem(raw))
}
/// The message of the section error `r` must fail with.
fn section_error(r: Result<RenderModelGeometry>) -> &'static str {
    match r {
        Err(Error::Section { source, .. }) => match *source {
            Error::Invalid(m) => m,
            other => panic!("expected Invalid, got {other:?}"),
        },
        other => panic!("expected section error, got {other:?}"),
    }
}
/// Offsets inside the raw block: the vertex-buffer descriptors and the
/// index block follow the part record.
const INDICES: usize = PART + 0x48 + 4;
const DESCRIPTORS: usize = INDICES + 8 + 4;

#[test]
fn decodes_compressed_quad_strip() {
    let (m, raw) = fixture();
    let g = read(m, raw).unwrap();
    assert_eq!(g.sections.len(), 1);
    assert_eq!(g.default_sections(), vec![0]);
    assert_eq!(g.regions[0].permutations[0].lod_sections[5], Some(0));
    assert_eq!(g.regions[0].permutations[0].lod_sections[0], None);
    let s = &g.sections[0];
    let v = &s.vertices;
    assert_eq!(v.positions[0], [-1.0, -2.0, 0.0]);
    assert_eq!(v.positions[1], [1.0, -2.0, 2.0]);
    assert_eq!(v.positions[3], [1.0, 2.0, 4.0]);
    assert_eq!(v.texcoords[0], [0.0, 0.5]);
    assert_eq!(v.texcoords[2], [0.5, 1.0]);
    assert!(v.node_weights.is_empty());
    let d = &s.geometry;
    // Strip 0,1,2,3: second triangle flips winding.
    assert_eq!(d.part_triangles(&d.parts[0]), vec![[0, 1, 2], [1, 3, 2]]);
    assert_eq!(d.vertex_buffers[0].kind, VertexKind::CompressedPosition);
    assert_eq!(s.block.owner, TagId(0xa0000000));
}

#[test]
fn triangle_list_flag_switches_topology() {
    let (mut m, mut raw) = fixture();
    put16(&mut raw, PART + 2, PART_FLAG_TRIANGLE_LIST);
    assert_eq!(
        section_error(read(m.clone(), raw.clone())),
        "triangle-list part is not whole triangles"
    );
    // Shrinking the part alone leaves an index the parts do not cover.
    put16(&mut raw, PART + 8, 3);
    assert_eq!(
        section_error(read(m.clone(), raw.clone())),
        "parts do not tile the index block"
    );
    // Shrink the index block to match: 3 indices, 6 bytes.
    put(&mut raw, 8 + 0x20, 3);
    put(&mut m, RESOURCES + 16 + 8, 6);
    put16(&mut m, SECTION + 6, 1);
    let g = read(m, raw).unwrap();
    let d = &g.sections[0].geometry;
    assert_eq!(d.part_triangles(&d.parts[0]), vec![[0, 1, 2]]);
}

#[test]
fn strips_and_lists_drop_degenerates() {
    assert_eq!(strip_triangles(&[0, 1, 2, 2, 3, 3, 4, 5, 6]).len(), 3);
    assert_eq!(
        strip_triangles(&[0, 1, 2, 3, 4]),
        vec![[0, 1, 2], [1, 3, 2], [2, 3, 4]]
    );
    assert!(strip_triangles(&[0, 1]).is_empty());
    assert_eq!(list_triangles(&[0, 1, 2, 3, 3, 4, 5]), vec![[0, 1, 2]]);
}

#[test]
fn normpacked3_extremes_and_sign() {
    assert_eq!(unpack_normpacked3(0x3ff), [1.0, 0.0, 0.0]);
    assert_eq!(unpack_normpacked3(0x401 << 11), [0.0, -1.0, 0.0]);
    assert_eq!(unpack_normpacked3(0x1ff << 22), [0.0, 0.0, 1.0]);
    assert_eq!(unpack_normpacked3(0x200 << 22), [0.0, 0.0, -1.0]);
}

#[test]
fn raw_block_framing_is_checked() {
    let (m, raw) = fixture();
    let mut bad = raw.clone();
    bad[0] = b'x';
    assert_eq!(
        section_error(read(m.clone(), bad)),
        "geometry block lacks blkh header"
    );
    let mut bad = raw.clone();
    let n = bad.len();
    bad[n - 1] = b'x';
    assert_eq!(
        section_error(read(m.clone(), bad)),
        "geometry block lacks blkf footer"
    );
    let mut bad = raw;
    bad[8 + 0x70] = b'x'; // first resource marker
    assert_eq!(
        section_error(read(m, bad)),
        "block resource lacks rsrc marker"
    );
}

#[test]
fn resource_sizes_and_indices_are_bounded() {
    let (mut m, raw) = fixture();
    // Index resource claims 2-byte elements but the wrong total.
    put(&mut m, RESOURCES + 16 + 8, 6);
    assert_eq!(
        section_error(read(m, raw.clone())),
        "section count disagrees with its block resource"
    );
    let (m, mut raw) = fixture();
    // Index 9 >= 4 vertices.
    put16(&mut raw, INDICES, 9);
    assert_eq!(section_error(read(m, raw)), "index outside vertex streams");
    let (mut m, raw) = fixture();
    put(&mut m, RESOURCES + 4 * 16 + 12, 0xffff); // vertex resource offset
    assert_eq!(
        section_error(read(m, raw)),
        "block resource outside resource area"
    );
    // The section record's vertex count must match the streams.
    let (mut m, raw) = fixture();
    put16(&mut m, SECTION + 4, 5);
    assert_eq!(
        section_error(read(m, raw)),
        "vertex streams disagree with the owner's vertex count"
    );
}

#[test]
fn compression_flags_must_match_stream_kinds() {
    let (mut m, raw) = fixture();
    put16(&mut m, SECTION + 0x1a, 2); // claims float positions
    assert_eq!(
        section_error(read(m, raw.clone())),
        "position stream disagrees with compression flags"
    );
    let (mut m, raw) = fixture();
    put16(&mut m, SECTION + 0x1a, 1); // claims float texcoords
    assert_eq!(
        section_error(read(m, raw)),
        "texcoord stream disagrees with compression flags"
    );
    // Compressed streams need the model's bounds.
    let (mut m, raw) = fixture();
    put(&mut m, TAG + 0x14, 0);
    assert_eq!(
        section_error(read(m, raw)),
        "compressed stream without bounds"
    );
}

#[test]
fn unknown_vertex_kinds_are_preserved_with_their_stride() {
    let (m, mut raw) = fixture();
    let vb1 = DESCRIPTORS + 0x20;
    raw[vb1] = 0x30; // unknown type, stride 4 kept
    let g = read(m, raw).unwrap();
    let s = &g.sections[0];
    let vb = &s.geometry.vertex_buffers[1];
    assert_eq!(vb.kind, VertexKind::Unknown(0x30));
    assert_eq!(s.geometry.data(vb).len(), 16);
    assert!(s.vertices.texcoords.is_empty());
    // A known type with the wrong stride is rejected.
    let (m, mut raw) = fixture();
    raw[vb1 + 1] = 8;
    assert_eq!(
        section_error(read(m, raw)),
        "vertex stream stride disagrees with its type"
    );
}

#[test]
fn permutation_and_material_indices_are_checked() {
    let (mut m, raw) = fixture();
    put16(&mut m, TAG + 0x15e, 1); // only one section exists
    assert!(matches!(
        read(m, raw),
        Err(Error::Invalid("permutation section index"))
    ));
    let (m, mut raw) = fixture();
    put16(&mut raw, PART + 4, 3); // only one material exists
    assert_eq!(section_error(read(m, raw)), "part material index");
    // 0xffff means no material.
    let (m, mut raw) = fixture();
    put16(&mut raw, PART + 4, 0xffff);
    assert!(read(m, raw).is_ok());
}

#[test]
fn weighted_vertices_carry_nodes_and_normalised_weights() {
    let (mut m, mut raw) = fixture();
    // Re-type buffer 0 as three weighted nodes (stride 12) and grow its data.
    raw[DESCRIPTORS] = 0x08;
    raw[DESCRIPTORS + 1] = 12;
    // Rebuild: simpler to splice a new position resource at the body end.
    let body_end = raw.len() - 4;
    let mut skin = Vec::new();
    for _ in 0..4 {
        skin.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        skin.extend_from_slice(&[0, 0, 0]);
        skin.extend_from_slice(&[255, 0, 0]);
    }
    let offset = (body_end + 4 - (8 + 0x70)) as u32;
    let mut new_raw = raw[..body_end].to_vec();
    new_raw.extend_from_slice(b"crsr");
    new_raw.extend_from_slice(&skin);
    new_raw.extend_from_slice(b"fklb");
    let body_len = (new_raw.len() - 4 - 8 - 0x70) as u32;
    put(&mut new_raw, 4, body_len);
    put(&mut m, SECTION + 0x3c, new_raw.len() as u32);
    put(&mut m, SECTION + 0x44, body_len);
    put(&mut m, RESOURCES + 3 * 16 + 8, 48);
    put(&mut m, RESOURCES + 3 * 16 + 12, offset);
    put16(&mut m, SECTION + 0x18, 3);
    let g = read(m, new_raw).unwrap();
    let v = &g.sections[0].vertices;
    assert_eq!(v.node_weights.len(), 4);
    assert_eq!(v.node_weights[0], [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(v.positions[0], [0.0, 0.0, 2.0]);
}

#[test]
fn geometry_codecs_roundtrip() {
    fn check<T: BinaryRecord>() {
        let bytes: Vec<_> = (0..T::SIZE).map(|i| (i * 37 + 11) as u8).collect();
        let v = T::decode(&bytes).unwrap();
        let mut out = vec![0; T::SIZE];
        v.encode_into(&mut out).unwrap();
        assert_eq!(out, bytes, "{}", T::META.name);
    }
    check::<ModeGeometryHeader>();
    check::<ModeCompression>();
    check::<ModeRegion>();
    check::<ModePermutation>();
    check::<ModeSection>();
    check::<ModeNode>();
    check::<ModeMaterial>();
}

// ---- block info and hostile inputs -------------------------------------------

#[test]
fn block_info_owner_and_runtime_state_are_checked() {
    let (mut m, raw) = fixture();
    put(&mut m, SECTION + 0x50, 0xa0010001);
    assert_eq!(
        section_error(read(m, raw)),
        "geometry block owner is not this tag"
    );
    let (mut m, raw) = fixture();
    put16(&mut m, SECTION + 0x54, 0x24);
    assert_eq!(
        section_error(read(m, raw)),
        "model section runtime block on disc"
    );
    let (mut m, raw) = fixture();
    put(&mut m, SECTION + 0x34, 0x8000_0000);
    assert_eq!(
        section_error(read(m, raw)),
        "model section runtime block on disc"
    );
    let (mut m, raw) = fixture();
    m[SECTION + 0x56] = 1; // loaded flag
    assert_eq!(
        section_error(read(m, raw)),
        "geometry block info has runtime state"
    );
    let (mut m, raw) = fixture();
    put(&mut m, SECTION + 0x38, u32::MAX);
    put(&mut m, SECTION + 0x3c, 0);
    put(&mut m, SECTION + 0x50, u32::MAX); // an absent block names no owner
    assert_eq!(
        section_error(read(m, raw)),
        "model section has no raw block"
    );
}

#[test]
fn huge_vertex_buffer_count_is_rejected_before_allocation() {
    // A descriptor count of u32::MAX must not size any table: it is checked
    // against the bytes that back it first.
    let (m, mut raw) = fixture();
    put(&mut raw, 8 + 0x38, u32::MAX);
    assert_eq!(
        section_error(read(m, raw)),
        "section count disagrees with its block resource"
    );
    // Same count with the backing resources removed (table truncated).
    let (mut m, mut raw) = fixture();
    put(&mut raw, 8 + 0x38, u32::MAX);
    put(&mut m, SECTION + 0x48, 2); // keep only parts and indices
    assert_eq!(
        section_error(read(m, raw)),
        "populated section block has no resource"
    );
}

#[test]
fn aliased_resources_are_rejected() {
    // A sixth resource re-reads vertex buffer 0's bytes as buffer 2.
    let (mut m, raw) = fixture();
    let src = RESOURCES + 3 * 16;
    let dst = RESOURCES + 5 * 16;
    let copy = m[src..src + 16].to_vec();
    m[dst..dst + 16].copy_from_slice(&copy);
    put16(&mut m, dst + 6, 2);
    put(&mut m, SECTION + 0x48, 6);
    assert_eq!(section_error(read(m, raw)), "block resources overlap");
    // The same buffer index twice.
    let (mut m, raw) = fixture();
    let copy = m[src..src + 16].to_vec();
    m[dst..dst + 16].copy_from_slice(&copy);
    put(&mut m, SECTION + 0x48, 6);
    assert_eq!(section_error(read(m, raw)), "duplicate block resource");
    // Partial overlap: buffer 1 starts inside buffer 0, right after a fake
    // marker planted in buffer 0's bytes.
    let (mut m, mut raw) = fixture();
    let r3 = RESOURCES + 3 * 16 + 12;
    let off0 = u32::from_le_bytes(m[r3..r3 + 4].try_into().unwrap()) as usize;
    let body = 8 + 0x70;
    raw[body + off0 + 4..body + off0 + 8].copy_from_slice(b"crsr");
    put(&mut m, RESOURCES + 4 * 16 + 12, off0 as u32 + 8);
    assert_eq!(section_error(read(m, raw)), "block resources overlap");
}

#[test]
fn repeated_streams_are_rejected() {
    // Buffer 0 re-typed as a second s16 texcoord stream (24 bytes divide
    // by its stride 4, so only the repeat is wrong).
    let (m, mut raw) = fixture();
    raw[DESCRIPTORS] = 0x19;
    raw[DESCRIPTORS + 1] = 4;
    assert_eq!(
        section_error(read(m, raw)),
        "section repeats a vertex stream kind"
    );
}

#[test]
fn non_finite_bounds_are_rejected() {
    let (mut m, raw) = fixture();
    put(&mut m, TAG + 0x100, f32::NAN.to_bits());
    assert_eq!(section_error(read(m, raw)), "non-finite compression bounds");
}

#[test]
fn node_map_entries_are_checked() {
    // Add a one-byte node map resource at the end of the body.
    let (mut m, raw) = fixture();
    let body_end = raw.len() - 4;
    let offset = (body_end + 4 - (8 + 0x70)) as u32;
    let mut new_raw = raw[..body_end].to_vec();
    new_raw.extend_from_slice(b"crsr");
    new_raw.push(1); // node 1: the model has one node
    new_raw.extend_from_slice(b"fklb");
    let body_len = (new_raw.len() - 4 - 8 - 0x70) as u32;
    put(&mut new_raw, 4, body_len);
    put(&mut new_raw, 8 + 0x64, 1);
    put(&mut m, SECTION + 0x3c, new_raw.len() as u32);
    put(&mut m, SECTION + 0x44, body_len);
    put(&mut m, SECTION + 0x48, 6);
    let r = RESOURCES + 5 * 16;
    put16(&mut m, r + 2, 0x64);
    put16(&mut m, r + 4, 0x64);
    put16(&mut m, r + 6, 1);
    put(&mut m, r + 8, 1);
    put(&mut m, r + 12, offset);
    m[r + 1] = 2;
    assert_eq!(
        section_error(read(m.clone(), new_raw.clone())),
        "node map entry out of range"
    );
    let at = new_raw.len() - 5;
    new_raw[at] = 0;
    let g = read(m, new_raw).unwrap();
    assert_eq!(g.sections[0].node_map, [0]);
}

#[test]
fn blocks_outside_the_tags_payload_are_read_from_the_whole_metadata() {
    // The same model, its payload declared to end before its sections,
    // nodes and materials: as a converted map that places identical blocks
    // once leaves them in another tag's payload.
    let (mut m, raw) = fixture();
    let whole = read(m.clone(), raw.clone()).unwrap();
    put(&mut m, 0x102c + 12, 0x140);
    let mut map = Map::from_reader(std::io::Cursor::new(m)).unwrap();
    let id = TagId(0xa0000000);
    let bytes = map.read_tag_bytes(id).unwrap();
    assert_eq!(bytes.len(), 0x140);
    let g = RenderModelGeometry::read(&mut map, id, &mut Mem(raw.clone())).unwrap();
    assert_eq!(g.sections.len(), whole.sections.len());
    assert_eq!(g.regions.len(), 1);
    assert_eq!(g.default_sections(), whole.default_sections());
    assert_eq!(g.sections[0].vertices.positions, whole.sections[0].vertices.positions);
    // The same with the metadata already read.
    let meta = crate::object::Metadata::read(&mut map).unwrap();
    let h = RenderModelGeometry::read_in(&mut map, id, &mut Mem(raw), Some(&meta)).unwrap();
    assert_eq!(h.materials.len(), whole.materials.len());
}
