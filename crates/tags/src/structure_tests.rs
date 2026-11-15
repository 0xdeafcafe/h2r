//! Synthetic tests for `geometry` and `structure`. Every byte here is invented.
use crate::Error;
use crate::geometry::{
    GeometryBlockInfo, LoadedBlock, Part, RawSource, ResourceKind, TagView, VertexKind,
    decode_section, load_block, unpack_normpacked3,
};
use crate::structure::{Lightmap, STRUCTURE_SECTION, StructureRender};
use map::{RawPointer, ResourceFile, TagId};

const BASE: u32 = 0x8000_0000;
const OWNER: TagId = TagId(0xe001_0002);

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

#[derive(Clone)]
struct Raw(Vec<u8>);
impl RawSource for Raw {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> crate::Result<Vec<u8>> {
        assert_eq!(p.file, ResourceFile::Local);
        let start = p.offset as usize;
        self.0
            .get(start..start + size as usize)
            .map(<[u8]>::to_vec)
            .ok_or(Error::Invalid("test raw range"))
    }
}

/// One resource: (kind, field offset, secondary, bytes).
type Res = (u8, u16, u16, Vec<u8>);

/// The default section: one triangle (3 vertices) in one list part with
/// one subpart, positions and tangent frames.
fn default_resources() -> Vec<Res> {
    let mut part = vec![0u8; 0x48];
    put16(&mut part, 0, 2); // kind
    put16(&mut part, 2, 0xb); // flags: triangle list
    put16(&mut part, 4, 7); // material
    put16(&mut part, 8, 3); // index count
    put16(&mut part, 0xc, 1); // subpart count
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
    // +z normal packed 11:11:10 (z = 511 in the top ten bits), twice zero.
    let frame: Vec<u8> = (0..3)
        .flat_map(|_| [511u32 << 22, 0, 0])
        .flat_map(|v: u32| v.to_le_bytes())
        .collect();
    vec![
        (0, 0, 0x48, part),
        (0, 8, 8, sub),
        (0, 0x20, 2, indices),
        (0, 0x38, 0x20, desc),
        (2, 0x38, 0, positions),
        (2, 0x38, 1, frame),
    ]
}

/// Build a `blkh` block and its resource table for a 0x44-byte section.
/// Section counts are derived from the block resources.
fn build_block(res: &[Res]) -> (Vec<u8>, Vec<[u8; 16]>) {
    let mut section = vec![0u8; 0x44];
    let mut data = Vec::new();
    let mut table = Vec::new();
    for (kind, field, secondary, bytes) in res {
        if *kind == 0 {
            put32(
                &mut section,
                usize::from(*field),
                (bytes.len() / usize::from(*secondary)) as u32,
            );
        }
        data.extend(b"crsr"); // `rsrc` as a little-endian FourCC
        let mut e = [0u8; 16];
        e[0] = *kind;
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

/// Write one 0x54-byte geometry header at `at` with its resource table at
/// `table_at`, naming a raw block at file offset 0x100.
fn put_geometry(p: &mut [u8], at: usize, table_at: usize, block: &[u8], table: &[[u8; 16]]) {
    put16(p, at, 3); // total vertices
    put16(p, at + 2, 1); // total triangles
    put16(p, at + 4, 1); // total parts
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

/// A synthetic BSP payload with one cluster, one definition and one
/// instance, all sharing the raw block built from `res`.
fn bsp_with(res: &[Res]) -> (Vec<u8>, Raw) {
    let (block, table) = build_block(res);
    let mut raw = vec![0u8; 0x100];
    raw.extend(&block);
    let mut p = vec![0u8; 0x1000];
    put32(&mut p, 8, 0xc0ffee);
    put32(&mut p, 0xa4, 8); // eight materials
    // Cluster block at 0x200, definitions at 0x400, instances at 0x600.
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
    // Scale 2, forward = +y, left = -x, up = +z, at (10, 20, 30).
    let m = [
        2.0f32, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 20.0, 30.0,
    ];
    for (i, v) in m.iter().enumerate() {
        put32(&mut p, 0x600 + 4 * i, v.to_bits());
    }
    (p, Raw(raw))
}
fn bsp_payload() -> (Vec<u8>, Raw) {
    bsp_with(&default_resources())
}
fn decode(p: &[u8]) -> crate::Result<StructureRender> {
    StructureRender::decode(OWNER, &TagView::new(p, BASE))
}
/// Load the cluster of a BSP built from `res`.
fn load_cluster(res: &[Res]) -> crate::Result<Option<crate::geometry::Section>> {
    let (p, mut raw) = bsp_with(res);
    decode(&p)?.clusters[0].geometry.load(&mut raw, Some(8))
}
/// As [`load_cluster`], with the cluster header's triangle and part totals
/// set to match a nonstandard fixture.
fn load_cluster_counted(
    res: &[Res],
    triangles: u16,
    parts: u16,
) -> crate::Result<Option<crate::geometry::Section>> {
    let (mut p, mut raw) = bsp_with(res);
    put16(&mut p, 0x200 + 2, triangles);
    put16(&mut p, 0x200 + 4, parts);
    decode(&p)?.clusters[0].geometry.load(&mut raw, Some(8))
}
fn invalid(r: crate::Result<impl std::fmt::Debug>) -> &'static str {
    match r {
        Err(Error::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn decodes_and_loads_structure_geometry() {
    let (p, mut raw) = bsp_payload();
    let bsp = decode(&p).unwrap();
    assert_eq!(bsp.checksum, 0xc0ffee);
    assert_eq!(
        (
            bsp.clusters.len(),
            bsp.instance_definitions.len(),
            bsp.instances.len()
        ),
        (1, 1, 1)
    );
    let c = &bsp.clusters[0];
    assert_eq!(c.bounds, [-1.0, 2.0, -1.0, 2.0, -1.0, 1.0]);
    let s = c.geometry.load(&mut raw, Some(8)).unwrap().unwrap();
    assert_eq!(s.indices, [0, 1, 2]);
    assert_eq!(s.parts[0].material, 7);
    assert!(s.parts[0].is_drawn() && s.parts[0].is_triangle_list());
    assert_eq!(s.part_triangles(&s.parts[0]), [[0, 1, 2]]);
    assert_eq!(s.subparts[0].index_count, 3);
    assert_eq!(s.vertex_count(), 3);
    let v = s.vertices(None).unwrap();
    assert_eq!(v.positions[1], [1.0, 0.0, 0.0]);
    assert_eq!(v.normals[2], [0.0, 0.0, 1.0]);
    assert!(s.stream(VertexKind::TangentFrame).is_some());
    let i = &bsp.instances[0];
    assert_eq!(i.definition, 0);
    assert!(i.is_rigid());
    assert_eq!(i.transform([1.0, 0.0, 0.0]), [10.0, 22.0, 30.0]);
    assert_eq!(i.transform([0.0, 1.0, 0.0]), [8.0, 20.0, 30.0]);
    assert_eq!(i.rotate([0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]);
}

#[test]
fn rejects_bad_framing_and_references() {
    let (p, raw) = bsp_payload();
    let bsp = decode(&p).unwrap();
    let info = bsp.clusters[0].geometry.block.clone();
    assert_eq!(info.resources[4].kind, ResourceKind::VertexBuffer);

    // Footer, header and per-resource markers are all required.
    for corrupt in [0x100usize, raw.0.len() - 1, 0x100 + 8 + 0x44] {
        let mut r = raw.clone();
        r.0[corrupt] ^= 0xff;
        assert!(
            load_block(&info, &mut r).is_err(),
            "corrupting {corrupt:#x}"
        );
    }
    // A section count that disagrees with its resource is rejected.
    let mut r = raw.clone();
    put32(&mut r.0, 0x100 + 8 + 0x20, 4);
    assert_eq!(
        invalid(load_block(&info, &mut r)),
        "section count disagrees with its block resource"
    );
    // An element size the layout does not expect is rejected.
    let mut res = default_resources();
    res[1].2 = 4; // subparts as 4-byte elements
    assert_eq!(
        invalid(load_cluster(&res)),
        "geometry block resource has unexpected element size"
    );
    // A vertex stride that contradicts the stream type is rejected.
    let mut res = default_resources();
    res[3].3[1] = 4;
    assert_eq!(
        invalid(load_cluster(&res)),
        "vertex stream stride disagrees with its type"
    );
    // Out-of-range indices.
    let mut res = default_resources();
    res[2].3[0] = 9;
    assert_eq!(invalid(load_cluster(&res)), "index outside vertex streams");
    // Part materials must be below the BSP's material count.
    let (p, mut raw) = bsp_payload();
    let s = decode(&p).unwrap().clusters[0].geometry.clone();
    assert_eq!(
        invalid(s.load(&mut raw, Some(7))),
        "part material outside the material block"
    );

    // Instances must name an existing definition.
    let mut q = p.clone();
    put16(&mut q, 0x600 + 0x34, 1);
    assert_eq!(
        invalid(decode(&q)),
        "instance refers to a missing definition"
    );
    // Geometry must be owned by the BSP being read.
    assert!(StructureRender::decode(TagId(1), &TagView::new(&p, BASE)).is_err());
    // Block pointers outside the payload are rejected.
    let mut q = p.clone();
    put32(&mut q, 0xa0, BASE + 0x2000);
    assert!(matches!(decode(&q), Err(Error::Bounds { .. })));
    // A populated runtime section block with a raw block is not a disc state.
    let mut q = p.clone();
    put32(&mut q, 0x200 + 0x4c, 1);
    assert_eq!(
        invalid(decode(&q)),
        "runtime section block is populated on disc"
    );
}

#[test]
fn absent_raw_block_loads_nothing_and_must_be_empty() {
    let (mut p, mut raw) = bsp_payload();
    put32(&mut p, 0x200 + 0x28, u32::MAX);
    // A pointer-less record must also have size 0 ...
    assert_eq!(
        invalid(decode(&p)),
        "geometry block pointer and size disagree"
    );
    put32(&mut p, 0x200 + 0x2c, 0);
    // ... and no owner ...
    assert_eq!(invalid(decode(&p)), "geometry block owner is not this tag");
    put32(&mut p, 0x200 + 0x40, u32::MAX);
    // ... and no resources unless it is an in-tag section.
    assert_eq!(invalid(decode(&p)), "absent geometry block has resources");
    put32(&mut p, 0x200 + 0x38, 0);
    let bsp = decode(&p).unwrap();
    assert!(bsp.clusters[0].geometry.block.block.is_none());
    assert!(
        bsp.clusters[0]
            .geometry
            .load(&mut raw, None)
            .unwrap()
            .is_none()
    );
    // A present pointer with block size 0 is rejected.
    let (mut p, _) = bsp_payload();
    put32(&mut p, 0x200 + 0x2c, 0);
    assert_eq!(
        invalid(decode(&p)),
        "geometry block pointer and size disagree"
    );
}

#[test]
fn raw_blocks_must_be_unloaded_and_resources_consistent() {
    let (p, raw) = bsp_payload();
    let info = decode(&p).unwrap().clusters[0].geometry.block.clone();
    let mut loaded = info.clone();
    loaded.runtime_loaded = 1;
    assert_eq!(
        invalid(load_block(&loaded, &mut raw.clone())),
        "geometry block info has runtime state"
    );
    let mut odd = info.clone();
    odd.resources[0].unknown_2 = 0x10;
    assert_eq!(
        invalid(load_block(&odd, &mut raw.clone())),
        "block resource field offsets differ"
    );
    let mut far = info.clone();
    far.resources[0].field_offset = 0x40;
    far.resources[0].unknown_2 = 0x40;
    assert_eq!(
        invalid(load_block(&far, &mut raw.clone())),
        "block resource field outside section"
    );
    // Oversized blocks are refused before reading.
    let mut huge = info.clone();
    huge.block_size = u32::MAX;
    assert_eq!(
        invalid(load_block(&huge, &mut raw.clone())),
        "geometry block exceeds size budget"
    );
    // Unknown resource kinds are rejected while decoding the table.
    let mut q = p.clone();
    q[0x800] = 3;
    assert_eq!(invalid(decode(&q)), "unknown block resource kind");
}

// ---- B1: aliasing and amplification ---------------------------------------

#[test]
fn duplicate_and_overlapping_resources_are_rejected() {
    // A second resource for vertex buffer 0 (same bytes appended again).
    let mut res = default_resources();
    res.push(res[4].clone());
    assert_eq!(invalid(load_cluster(&res)), "duplicate block resource");
    // A second block resource for the index field.
    let mut res = default_resources();
    res.push(res[2].clone());
    assert_eq!(invalid(load_cluster(&res)), "duplicate block resource");
    // Buffer 1's entry re-reads buffer 0's bytes.
    let (p, raw) = bsp_payload();
    let info = decode(&p).unwrap().clusters[0].geometry.block.clone();
    let mut alias = info.clone();
    alias.resources[5].offset = alias.resources[4].offset;
    alias.resources[5].size = alias.resources[4].size;
    assert_eq!(
        invalid(load_block(&alias, &mut raw.clone())),
        "block resources overlap"
    );
    // Partial overlap behind a planted marker.
    let mut r = raw.clone();
    let off = info.resources[4].offset.unwrap() as usize;
    let at = 0x100 + 8 + 0x44 + off + 4;
    r.0[at..at + 4].copy_from_slice(b"crsr");
    let mut part = info.clone();
    part.resources[5].offset = Some(off as u32 + 8);
    part.resources[5].size = 36;
    assert_eq!(
        invalid(load_block(&part, &mut r)),
        "block resources overlap"
    );
    // A vertex resource with no descriptor slot.
    let mut res = default_resources();
    res[5].2 = 7;
    assert_eq!(
        invalid(load_cluster(&res)),
        "vertex resource has no descriptor"
    );
}

#[test]
fn aliased_tag_blocks_exhaust_the_budget() {
    // 64 records whose blocks all name the same 0x400-byte table: each read
    // is in bounds, but together they exceed the payload.
    let mut p = vec![0u8; 0x1000];
    for i in 0..64 {
        put32(&mut p, 8 * i, 0x100);
        put32(&mut p, 8 * i + 4, BASE + 0x800);
    }
    let view = TagView::new(&p, BASE);
    let mut result = Ok(());
    for i in 0..64 {
        if let Err(e) = view.block(&p, 8 * i, 4) {
            result = Err(e);
            break;
        }
    }
    assert_eq!(invalid(result), "tag blocks exceed the payload budget");
    // Through the lightmap reader: bucket references that all alias one
    // large offsets table.
    let mut l = lightmap_payload();
    let refs = 0x6c0;
    put32(&mut l, 0x104 + 0x60, 20);
    put32(&mut l, 0x104 + 0x64, BASE + refs as u32);
    for i in 0..20 {
        put16(&mut l, refs + 0xc * i + 2, 0);
        put32(&mut l, refs + 0xc * i + 4, 0x300);
        put32(&mut l, refs + 0xc * i + 8, BASE);
    }
    assert_eq!(
        invalid(Lightmap::decode(TagId(9), &TagView::new(&l, BASE))),
        "tag blocks exceed the payload budget"
    );
}

#[test]
fn loaded_block_resources_are_private_and_optional() {
    let (p, mut raw) = bsp_payload();
    let info = decode(&p).unwrap().clusters[0].geometry.block.clone();
    let block = load_block(&info, &mut raw).unwrap().unwrap();
    assert_eq!(
        block.resource(&info.resources[2]).unwrap(),
        [0, 0, 1, 0, 2, 0]
    );
    // An entry that is not this block's (another field) has no bytes.
    let mut other = info.resources[2].clone();
    other.field_offset = 0x10;
    assert!(block.resource(&other).is_none());
    // A block parsed for another info cannot be built from these bytes.
    let mut wrong = info.clone();
    wrong.section_size = 0x40;
    assert!(LoadedBlock::parse(&wrong, raw.0[0x100..].to_vec()).is_err());
    // The layout's struct size must match the block's.
    let mut bucket = crate::structure::BUCKET_SECTION;
    bucket.vertex_buffers = 0x38;
    assert_eq!(
        invalid(decode_section(block, &bucket, None)),
        "section struct size disagrees with its layout"
    );
}

// ---- B2: geometry invariants for every layout -----------------------------

#[test]
fn vertex_counts_must_agree() {
    // The owner's count must match the streams.
    let (mut p, mut raw) = bsp_payload();
    put16(&mut p, 0x200, 4);
    assert_eq!(
        invalid(
            decode(&p).unwrap().clusters[0]
                .geometry
                .load(&mut raw, None)
        ),
        "vertex streams disagree with the owner's vertex count"
    );
    // Streams must agree with each other.
    let mut res = default_resources();
    res[5].3.truncate(24);
    assert_eq!(
        invalid(load_cluster(&res)),
        "vertex streams disagree on vertex count"
    );
    // Two position streams.
    let mut res = default_resources();
    res[3].3[0x20] = 0x01;
    res[5].3 = vec![0; 36];
    assert_eq!(
        invalid(load_cluster(&res)),
        "section repeats a vertex stream kind"
    );
}

#[test]
fn an_empty_stream_set_is_not_a_vertex_count() {
    // No descriptors but three vertices claimed.
    let res: Vec<Res> = default_resources().into_iter().take(3).collect();
    assert_eq!(invalid(load_cluster(&res)), "section has no vertex streams");
    // The genuinely empty section (no parts, indices or streams, count 0).
    let (mut p, mut raw) = bsp_with(&[]);
    for o in [0, 2, 4] {
        put16(&mut p, 0x200 + o, 0);
    }
    let s = decode(&p).unwrap().clusters[0]
        .geometry
        .load(&mut raw, None)
        .unwrap()
        .unwrap();
    assert_eq!((s.vertex_count(), s.parts.len()), (0, 0));
}

// ---- B3: tiling -----------------------------------------------------------

#[test]
fn parts_and_subparts_must_tile() {
    let set_part = |at: usize, v: u16| {
        let mut res = default_resources();
        put16(&mut res[0].3, at, v);
        res
    };
    // Part starts past index 0.
    assert_eq!(
        invalid(load_cluster(&set_part(6, 1))),
        "parts do not tile the index block"
    );
    // Part ends short of the index block (two indices: not whole triangles).
    assert_eq!(
        invalid(load_cluster(&set_part(8, 2))),
        "triangle-list part is not whole triangles"
    );
    // A strip part shorter than the index block leaves indices uncovered.
    let mut res = set_part(2, 0);
    put16(&mut res[0].3, 8, 2);
    put16(&mut res[1].3, 2, 2);
    assert_eq!(
        invalid(load_cluster(&res)),
        "parts do not tile the index block"
    );
    // Subpart run outside the subpart block.
    assert_eq!(
        invalid(load_cluster(&set_part(0xc, 2))),
        "parts do not tile the subpart block"
    );
    // Subpart naming another part.
    let mut res = default_resources();
    put16(&mut res[1].3, 6, 1);
    assert_eq!(invalid(load_cluster(&res)), "subpart names another part");
    // Subpart not covering its part.
    let mut res = default_resources();
    put16(&mut res[1].3, 2, 2);
    assert_eq!(
        invalid(load_cluster(&res)),
        "subparts do not tile their part"
    );
    let mut res = default_resources();
    put16(&mut res[1].3, 0, 1);
    assert_eq!(invalid(load_cluster(&res)), "subpart outside its part");
    // Multiple parts: gaps and overlaps between them.
    // The well-formed control: two adjacent parts over six indices.
    let s = load_cluster_counted(&two_parts([(0, 3), (3, 3)], 6), 2, 2)
        .unwrap()
        .unwrap();
    assert_eq!(s.parts.len(), 2);
    // A gap between the parts (indices 3..6 uncovered), ending exactly at the
    // index block so only the part-start continuity check sees it.
    assert_eq!(
        invalid(load_cluster_counted(&two_parts([(0, 3), (6, 3)], 9), 3, 2)),
        "parts do not tile the index block"
    );
    // Overlapping parts (3..6 drawn twice), also ending at the block end.
    assert_eq!(
        invalid(load_cluster_counted(&two_parts([(0, 6), (3, 3)], 6), 2, 2)),
        "parts do not tile the index block"
    );
}

/// Two triangle-list parts, each with one subpart covering it exactly, over an
/// index block of `index_count` indices. `parts` is (first_index, count).
fn two_parts(parts: [(u16, u16); 2], index_count: u16) -> Vec<Res> {
    let mut res = default_resources();
    let template = res[0].3.clone();
    let mut part_block = Vec::new();
    let mut sub_block = Vec::new();
    for (i, (first, count)) in parts.iter().enumerate() {
        let mut part = template.clone();
        put16(&mut part, 6, *first);
        put16(&mut part, 8, *count);
        put16(&mut part, 0xa, i as u16); // first subpart
        put16(&mut part, 0xc, 1);
        part_block.extend(part);
        let mut sub = vec![0u8; 8];
        put16(&mut sub, 0, *first);
        put16(&mut sub, 2, *count);
        put16(&mut sub, 4, 0xffff);
        put16(&mut sub, 6, i as u16);
        sub_block.extend(sub);
    }
    res[0].3 = part_block;
    res[1].3 = sub_block;
    res[2].3 = (0..index_count)
        .flat_map(|i| (i % 3).to_le_bytes())
        .collect();
    res
}

#[test]
fn nonempty_part_with_no_subparts_is_rejected() {
    // Subpart count 0 and no subpart resource: the part's three indices are
    // covered by no subpart, which structure geometry never does.
    let mut res = default_resources();
    put16(&mut res[0].3, 0xc, 0);
    res.remove(1);
    assert_eq!(
        invalid(load_cluster(&res)),
        "subparts do not tile their part"
    );
    // An empty part with no subparts is still fine.
    let mut res = default_resources();
    put16(&mut res[0].3, 8, 0);
    put16(&mut res[0].3, 0xc, 0);
    res.remove(1);
    res[1].3.clear(); // no indices either
    let s = load_cluster_counted(&res, 0, 1).unwrap().unwrap();
    assert_eq!((s.parts.len(), s.indices.len()), (1, 0));
}

#[test]
fn part_ranges_do_not_overflow_u16() {
    let p = Part {
        kind: 2,
        flags: 0,
        material: 0,
        first_index: u16::MAX,
        index_count: 2,
        first_subpart: 0,
        subpart_count: 0,
        raw: vec![],
    };
    assert_eq!(p.index_range(), 65535..65537);
}

// ---- non-finite values ----------------------------------------------------

#[test]
fn non_finite_placements_bounds_and_positions_are_rejected() {
    for at in [0x600, 0x604, 0x628] {
        let (mut p, _) = bsp_payload();
        put32(&mut p, at, f32::NAN.to_bits());
        assert_eq!(invalid(decode(&p)), "non-finite instance placement");
    }
    let (mut p, _) = bsp_payload();
    put32(&mut p, 0x200 + 0x58, f32::INFINITY.to_bits());
    assert_eq!(invalid(decode(&p)), "non-finite cluster bounds");
    let mut res = default_resources();
    res[4].3[4..8].copy_from_slice(&f32::NAN.to_le_bytes());
    let s = load_cluster(&res).unwrap().unwrap();
    assert_eq!(invalid(s.vertices(None)), "non-finite vertex attribute");
}

#[test]
fn packed_normals_are_signed_11_11_10() {
    assert_eq!(unpack_normpacked3(0x3ff), [1.0, 0.0, 0.0]);
    assert_eq!(unpack_normpacked3(0x401), [-1.0, 0.0, 0.0]);
    assert_eq!(unpack_normpacked3(0x3ff << 11), [0.0, 1.0, 0.0]);
    assert_eq!(unpack_normpacked3(0x201 << 22), [0.0, 0.0, -1.0]);
}

// ---- lightmaps --------------------------------------------------------------

fn lightmap_payload() -> Vec<u8> {
    let mut p = vec![0u8; 0x800];
    put32(&mut p, 0x80, 1);
    put32(&mut p, 0x84, BASE + 0x104);
    let g = 0x104;
    put32(&mut p, g + 4, 0xc0ffee);
    put32(&mut p, g + 8, 1); // one palette at 0x200
    put32(&mut p, g + 0xc, BASE + 0x200);
    p[0x200..0x204].copy_from_slice(&0xff11_2233u32.to_le_bytes());
    p[g + 0x18..g + 0x1c].copy_from_slice(b"mtib");
    put32(&mut p, g + 0x1c, 0xe002_0003);
    put32(&mut p, g + 0x28, 2); // cluster render info at 0x600
    put32(&mut p, g + 0x2c, BASE + 0x600);
    p[0x600..0x608].copy_from_slice(&[3, 0, 0, 0, 0xff, 0xff, 0xff, 0]);
    put32(&mut p, g + 0x40, 1); // one bucket at 0x640, no raw block
    put32(&mut p, g + 0x44, BASE + 0x640);
    put16(&mut p, 0x640, 1);
    put32(&mut p, 0x640 + 0xc, u32::MAX);
    put32(&mut p, 0x640 + 0xc + 0x18, u32::MAX); // no owner
    put32(&mut p, g + 0x50, 1); // one instance bucket ref at 0x680
    put32(&mut p, g + 0x54, BASE + 0x680);
    put32(&mut p, 0x680 + 4, 2);
    put32(&mut p, 0x680 + 8, BASE + 0x6a0);
    put16(&mut p, 0x6a0, 12);
    put16(&mut p, 0x6a2, 0xffff);
    p
}
fn decode_lightmap(p: &[u8]) -> crate::Result<Lightmap> {
    Lightmap::decode(TagId(9), &TagView::new(p, BASE))
}

#[test]
fn decodes_lightmap_group() {
    let p = lightmap_payload();
    let l = decode_lightmap(&p).unwrap();
    let g0 = &l.groups[0];
    assert_eq!(g0.structure_checksum, 0xc0ffee);
    assert_eq!(g0.palettes[0][0], 0xff11_2233);
    assert_eq!(g0.bitmap, Some(TagId(0xe002_0003)));
    assert_eq!(g0.cluster_render_info[0].bitmap_index, 3);
    assert_eq!(
        (
            g0.cluster_render_info[1].bitmap_index,
            g0.cluster_render_info[1].palette_index
        ),
        (-1, -1)
    );
    assert_eq!(g0.buckets[0].flags, 1);
    assert!(g0.buckets[0].is_patched());
    assert!(g0.buckets[0].block.block.is_none());
    assert_eq!(g0.instance_bucket_refs[0].section_offsets, [Some(12), None]);

    // A bucket reference beyond the bucket block, or a non-bitmap reference, fails.
    let mut q = p.clone();
    put16(&mut q, 0x680 + 2, 1);
    assert_eq!(
        invalid(decode_lightmap(&q)),
        "bucket reference outside bucket block"
    );
    let mut q = p.clone();
    q[0x104 + 0x18..0x104 + 0x1c].copy_from_slice(b"rdhs");
    assert_eq!(
        invalid(decode_lightmap(&q)),
        "lightmap bitmap reference is not a bitmap"
    );
}

#[test]
fn lightmap_groups_use_the_engine_stride() {
    // E 0x00015c6b: `add esi, 68h` per group. A second group sits 0x68
    // bytes after the first, not 0x98.
    let mut p = lightmap_payload();
    put32(&mut p, 0x80, 2);
    let g1 = 0x104 + 0x68;
    put32(&mut p, g1 + 4, 0xbeef);
    put32(&mut p, g1 + 0x1c, u32::MAX);
    let l = decode_lightmap(&p).unwrap();
    assert_eq!(l.groups.len(), 2);
    assert_eq!(l.groups[0].raw.len(), 0x68);
    assert_eq!(l.groups[1].structure_checksum, 0xbeef);
    assert_eq!(l.groups[1].bitmap, None);
    assert!(l.groups[1].palettes.is_empty());
}

#[test]
fn malformed_block_descriptors_are_errors_not_panics() {
    let empty = TagView::new(&[], BASE);
    assert_eq!(
        invalid(empty.block(&[], 0, 4)),
        "tag block field outside its record"
    );
    assert_eq!(
        invalid(empty.block(&[0; 8], usize::MAX - 2, 4)),
        "tag block field outside its record"
    );
    assert_eq!(
        invalid(empty.block(&[0; 7], 0, 4)),
        "tag block field outside its record"
    );
    assert_eq!(
        invalid(empty.block(&[0; 8], 0, 0)),
        "tag block stride is zero"
    );
    assert_eq!(
        invalid(empty.elements(1, BASE, 0)),
        "tag block stride is zero"
    );
    // A count whose size overflows is rejected before any read.
    let p = vec![0u8; 0x10];
    let view = TagView::new(&p, BASE);
    assert!(view.elements(u32::MAX, BASE, usize::MAX).is_err());
    assert!(empty.block(&[0; 8], 0, 4).unwrap().is_empty());
}

#[test]
fn lightmap_buckets_and_references_are_checked() {
    // A reference with no offsets: the engine would read past it.
    let mut q = lightmap_payload();
    put32(&mut q, 0x680 + 4, 0);
    assert_eq!(
        invalid(decode_lightmap(&q)),
        "bucket reference has no offsets"
    );
    // An instance reference to bucket 1 of a one-bucket block.
    let mut q = lightmap_payload();
    put16(&mut q, 0x680 + 2, 1);
    assert_eq!(
        invalid(decode_lightmap(&q)),
        "bucket reference outside bucket block"
    );
    // The same through the cluster references (one at 0x6c0, bucket 0xffff).
    let mut q = lightmap_payload();
    put32(&mut q, 0x104 + 0x60, 1);
    put32(&mut q, 0x104 + 0x64, BASE + 0x6c0);
    put32(&mut q, 0x6c0 + 4, 1);
    put32(&mut q, 0x6c0 + 8, BASE + 0x6a0);
    assert!(decode_lightmap(&q).is_ok());
    put16(&mut q, 0x6c0 + 2, 0xffff);
    assert_eq!(
        invalid(decode_lightmap(&q)),
        "bucket reference outside bucket block"
    );
    // The bucket's runtime block must be empty on disc.
    let mut q = lightmap_payload();
    put32(&mut q, 0x640 + 0x30, 1);
    assert_eq!(
        invalid(decode_lightmap(&q)),
        "lightmap bucket runtime block is populated on disc"
    );
    // A bucket without a raw block names no owner.
    let mut q = lightmap_payload();
    put32(&mut q, 0x640 + 0xc + 0x18, 0xe003_0004);
    assert_eq!(
        invalid(decode_lightmap(&q)),
        "geometry block owner is not this tag"
    );
}

#[test]
fn lightmap_geometry_owner_is_checked() {
    // One cluster-geometry header without a raw block at 0x700.
    let mut p = lightmap_payload();
    let g = 0x104;
    put32(&mut p, g + 0x20, 1);
    put32(&mut p, g + 0x24, BASE + 0x700);
    put32(&mut p, 0x700 + 0x28, u32::MAX);
    put32(&mut p, 0x700 + 0x28 + 0x18, u32::MAX);
    assert!(decode_lightmap(&p).is_ok());
    put32(&mut p, 0x700 + 0x28 + 0x18, 0xe001_0002);
    assert_eq!(
        invalid(decode_lightmap(&p)),
        "geometry block owner is not this tag"
    );
}

#[test]
fn block_info_sizes() {
    assert_eq!(GeometryBlockInfo::SIZE, 0x24);
    assert_eq!(crate::structure::GeometryHeader::SIZE, 0x54);
    assert_eq!(STRUCTURE_SECTION.section_size, 0x44);
}
