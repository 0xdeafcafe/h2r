//! The section encoder against the decoder, on invented sections of both
//! layouts. Every byte here is made up.
use super::*;
use crate::geometry::{
    GeometryBlockInfo, LoadedBlock, Section, decode_section, unpack_normpacked3,
};
use crate::model::MODEL_SECTION;
use crate::structure::STRUCTURE_SECTION;
use map::{RawPointer, ResourceFile, TagId};

/// (kind, field, secondary, bytes); `None` bytes is a skipped entry.
type Res = (u8, u16, u16, Option<Vec<u8>>);

fn b16(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn b32(v: impl IntoIterator<Item = u32>) -> Vec<u8> {
    v.into_iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn kind(k: u8) -> ResourceKind {
    match k {
        0 => ResourceKind::Block,
        1 => ResourceKind::Data,
        _ => ResourceKind::VertexBuffer,
    }
}

/// A block framed by hand, not by the encoder: no alignment, `gap` junk
/// bytes after every resource, and the section counts set from the block
/// resources. Returns the block info it parses with and its bytes.
fn hand_block(section_size: usize, res: &[Res], gap: usize) -> (GeometryBlockInfo, Vec<u8>) {
    let mut section = vec![0u8; section_size];
    let mut area = Vec::new();
    let mut table = Vec::new();
    for (k, field, secondary, bytes) in res {
        let Some(bytes) = bytes else {
            table.push(BlockResource {
                kind: kind(*k),
                unknown_1: 2,
                unknown_2: *field,
                field_offset: *field,
                secondary: *secondary,
                size: 0x30,
                offset: None,
            });
            continue;
        };
        if *k == 0 {
            let at = usize::from(*field);
            let count = (bytes.len() / usize::from(*secondary)) as u32;
            section[at..at + 4].copy_from_slice(&count.to_le_bytes());
        }
        area.extend(b"crsr");
        table.push(BlockResource {
            kind: kind(*k),
            unknown_1: 2,
            unknown_2: *field,
            field_offset: *field,
            secondary: *secondary,
            size: bytes.len() as u32,
            offset: Some(area.len() as u32),
        });
        area.extend(bytes);
        area.extend(std::iter::repeat_n(0xee, gap));
    }
    let mut block = b"hklb".to_vec();
    block.extend((area.len() as u32).to_le_bytes());
    block.extend(&section);
    block.extend(&area);
    block.extend(b"fklb");
    let info = info_for(&block, section_size as u32, area.len() as u32, table);
    (info, block)
}

fn info_for(
    block: &[u8],
    section: u32,
    area: u32,
    resources: Vec<BlockResource>,
) -> GeometryBlockInfo {
    GeometryBlockInfo {
        block: Some(RawPointer {
            file: ResourceFile::Local,
            offset: 0x4000,
        }),
        block_size: block.len() as u32,
        section_size: section,
        resource_size: area,
        resources,
        owner: TagId(0xe100_0004),
        runtime_block_offset: 0x24,
        runtime_loaded: 0,
        unknown_1f: 1,
        runtime_cache_handle: u32::MAX,
    }
}

fn part(first: u16, count: u16, subpart: u16, flags: u16) -> Vec<u8> {
    let mut p = vec![0u8; 0x48];
    p[0..2].copy_from_slice(&2u16.to_le_bytes());
    p[2..4].copy_from_slice(&flags.to_le_bytes());
    p[6..8].copy_from_slice(&first.to_le_bytes());
    p[8..10].copy_from_slice(&count.to_le_bytes());
    p[0xa..0xc].copy_from_slice(&subpart.to_le_bytes());
    p[0xc..0xe].copy_from_slice(&1u16.to_le_bytes());
    // An invented sphere and box, so bounds bytes are not all zero.
    for (i, v) in [0.5f32, 0.5, 0.0].iter().enumerate() {
        p[0x10 + 4 * i..0x14 + 4 * i].copy_from_slice(&v.to_bits().to_le_bytes());
    }
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

/// A structure section: a quad of two list triangles, f32 positions and
/// coordinates, a frame and lightmap coordinates, and one skipped entry.
fn structure_resources() -> Vec<Res> {
    let positions = b32([
        0.0f32, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 2.0, 2.0, 0.0,
    ]
    .map(f32::to_bits));
    let uvs = b32([0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0].map(f32::to_bits));
    let frame = b32((0..4).flat_map(|_| [511u32 << 22, 0x3ff, 0x3ff << 11]));
    vec![
        (0, 0, 0x48, Some(part(0, 6, 0, 0xb))),
        (0, 8, 8, Some(b16(&[0, 6, 0xffff, 0]))),
        (0, 0x20, 2, Some(b16(&[0, 1, 2, 1, 3, 2]))),
        (
            0,
            0x38,
            0x20,
            Some(descriptors(&[(0x01, 12), (0x18, 8), (0x1b, 12), (0x1f, 4)])),
        ),
        (2, 0x38, 0, Some(positions)),
        (2, 0x38, 1, Some(uvs)),
        (2, 0x38, 2, Some(frame)),
        (2, 0x38, 3, Some(b16(&[0x4000; 8]))),
        (1, 0x2c, 0xffff, None),
    ]
}

/// A model section: a strip part, s16 positions with one node each, s16
/// coordinates, a frame, a node map and an uninterpreted block at 0x10.
fn model_resources() -> Vec<Res> {
    let one_node = |x: i16, y: i16, z: i16, node: u8| {
        let mut v = Vec::new();
        for c in [x, y, z] {
            v.extend(c.to_le_bytes());
        }
        v.extend([node, 0]);
        v
    };
    let positions: Vec<u8> = [
        one_node(-32767, -32767, 0, 0),
        one_node(32767, -32767, 0, 1),
        one_node(-32767, 32767, 0, 0),
        one_node(32767, 32767, 0, 1),
    ]
    .concat();
    let uvs = b16(&[
        0x8001, 0x8001, 0x7fff, 0x8001, 0x8001, 0x7fff, 0x7fff, 0x7fff,
    ]);
    let frame = b32((0..4).flat_map(|_| [511u32 << 22, 0, 0]));
    vec![
        (0, 0, 0x48, Some(part(0, 4, 0, 0))),
        (0, 8, 8, Some(b16(&[0, 0, 0x1234, 0]))),
        (0, 0x10, 0x14, Some((0..0x28u8).collect())),
        (0, 0x20, 2, Some(b16(&[0, 1, 2, 3]))),
        (
            0,
            0x38,
            0x20,
            Some(descriptors(&[(0x04, 8), (0x19, 4), (0x1b, 12)])),
        ),
        (2, 0x38, 0, Some(positions)),
        (2, 0x38, 1, Some(uvs)),
        (2, 0x38, 2, Some(frame)),
        (0, 0x64, 1, Some(vec![3, 5])),
    ]
}

fn layouts() -> [(&'static str, SectionLayout, Vec<Res>); 2] {
    [
        ("structure", STRUCTURE_SECTION, structure_resources()),
        ("model", MODEL_SECTION, model_resources()),
    ]
}

fn decoded(info: &GeometryBlockInfo, bytes: Vec<u8>, layout: &SectionLayout) -> Section {
    let block = LoadedBlock::parse(info, bytes).expect("block parses");
    decode_section(block, layout, None).expect("section decodes")
}

fn parse_encoded(e: &EncodedSection, layout: &SectionLayout) -> Section {
    let info = info_for(
        &e.block,
        e.section_size,
        e.resource_size,
        e.resources.clone(),
    );
    decoded(&info, e.block.clone(), layout)
}

#[test]
fn a_hand_framed_block_encodes_back_to_its_bytes() {
    for (name, layout, res) in layouts() {
        for gap in [0, 3] {
            let (info, bytes) = hand_block(layout.section_size as usize, &res, gap);
            let section = decoded(&info, bytes.clone(), &layout);
            let data = SectionData::from_block(section.block());
            assert!(data.keeps_sizes(), "{name}");
            let e = encode_section(&layout, &data).unwrap();
            assert_eq!(e.block, bytes, "{name} gap {gap}");
            let records: Vec<_> = e.resources.iter().map(BlockResource::encode).collect();
            let source: Vec<_> = info.resources.iter().map(BlockResource::encode).collect();
            assert_eq!(records, source, "{name}");
            assert_eq!(
                (e.section_size, e.resource_size),
                (info.section_size, info.resource_size)
            );
        }
    }
}

#[test]
fn an_encoded_section_decodes_to_what_was_given() {
    for (name, layout, res) in layouts() {
        let given: Vec<ResourceData> = res
            .iter()
            .map(|(k, field, secondary, bytes)| ResourceData {
                kind: kind(*k),
                unknown_1: 2,
                field: *field,
                secondary: *secondary,
                declared_size: bytes.as_ref().map_or(0x30, |b| b.len() as u32),
                bytes: bytes.clone(),
            })
            .collect();
        let data = SectionData::new(vec![0; layout.section_size as usize], given.clone());
        assert!(!data.keeps_sizes());
        let e = encode_section(&layout, &data).unwrap();
        let section = parse_encoded(&e, &layout);
        let back = SectionData::from_block(section.block());
        assert_eq!(back.resources, given, "{name}");
        // The decoder read what the resources say.
        let (info, bytes) = hand_block(layout.section_size as usize, &res, 0);
        let hand = decoded(&info, bytes, &layout);
        assert_eq!(section.indices, hand.indices, "{name}");
        assert_eq!(
            section
                .parts
                .iter()
                .map(|p| p.raw.clone())
                .collect::<Vec<_>>(),
            hand.parts.iter().map(|p| p.raw.clone()).collect::<Vec<_>>(),
        );
        assert_eq!(section.section_struct(), hand.section_struct(), "{name}");
        // Encoding the decoded section again is the same block.
        assert_eq!(
            encode_section(&layout, &back).unwrap().block,
            e.block,
            "{name}"
        );
    }
}

#[test]
fn an_edit_that_keeps_sizes_changes_only_its_bytes() {
    let (_, layout, res) = layouts().into_iter().next().unwrap();
    let (info, bytes) = hand_block(layout.section_size as usize, &res, 1);
    let section = decoded(&info, bytes.clone(), &layout);
    let mut data = SectionData::from_block(section.block());
    let stream = data
        .resource_mut(ResourceKind::VertexBuffer, 0x38, 0)
        .unwrap();
    // Vertex 3's z.
    stream[3 * 12 + 8..3 * 12 + 12].copy_from_slice(&1.5f32.to_bits().to_le_bytes());
    let e = encode_section(&layout, &data).unwrap();
    assert_eq!(e.block.len(), bytes.len());
    let runs = changed_runs(&bytes, &e.block);
    assert_eq!(runs.len(), 1);
    // 0.0 to 1.5 changes the top two bytes of the float only.
    assert!(1.5f32.to_le_bytes().ends_with(&runs[0].1), "{runs:?}");
    let positions = parse_encoded(&e, &layout).vertices(None).unwrap().positions;
    assert_eq!(positions[3], [2.0, 2.0, 1.5]);
}

#[test]
fn a_grown_block_is_laid_out_again_and_keeps_the_rest() {
    let (_, layout, res) = layouts().into_iter().nth(1).unwrap();
    let (info, bytes) = hand_block(layout.section_size as usize, &res, 2);
    let section = decoded(&info, bytes, &layout);
    let mut data = SectionData::from_block(section.block());
    // The strip gains a vertex reference (one more triangle), its part
    // and subpart follow.
    *data.resource_mut(ResourceKind::Block, 0x20, 0).unwrap() = b16(&[0, 1, 2, 3, 2]);
    *data.resource_mut(ResourceKind::Block, 0, 0).unwrap() = part(0, 5, 0, 0);
    assert!(!data.keeps_sizes());
    let e = encode_section(&layout, &data).unwrap();
    let grown = parse_encoded(&e, &layout);
    assert_eq!(grown.indices, [0, 1, 2, 3, 2]);
    assert_eq!(u32_at(grown.section_struct(), 0x20), 5);
    assert_eq!(
        grown.extra_block(0x10, 0x14).unwrap().concat(),
        (0..0x28u8).collect::<Vec<_>>()
    );
    assert_eq!(grown.extra_block(0x64, 1).unwrap().concat(), [3, 5]);
}

#[test]
fn a_struct_of_another_size_or_a_ragged_block_is_refused() {
    let (_, layout, res) = layouts().into_iter().next().unwrap();
    let (info, bytes) = hand_block(layout.section_size as usize, &res, 0);
    let section = decoded(&info, bytes, &layout);
    let mut data = SectionData::from_block(section.block());
    data.section.push(0);
    assert!(encode_section(&layout, &data).is_err());
    data.section.pop();
    data.resource_mut(ResourceKind::Block, 0x20, 0)
        .unwrap()
        .push(7);
    assert!(encode_section(&layout, &data).is_err());
}

#[test]
fn packers_invert_the_decoders_expansions() {
    for v in [
        [0.0f32, 0.0, 1.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.6, -0.48, 0.64],
    ] {
        let back = unpack_normpacked3(pack_normpacked3(v));
        for k in 0..3 {
            assert!((back[k] - v[k]).abs() < 2.0 / 511.0, "{v:?} -> {back:?}");
        }
    }
    assert_eq!(pack_normpacked3([0.0, 0.0, 1.0]), 511 << 22);
    let expand = |s: i16, min: f32, max: f32| {
        let u = (f32::from(s) / 32767.0).max(-1.0);
        (u + 1.0) * 0.5 * (max - min) + min
    };
    for v in [-3.0f32, -1.25, 0.0, 0.7, 5.0] {
        let s = quantize(v, -3.0, 5.0).unwrap();
        assert!((expand(s, -3.0, 5.0) - v).abs() <= 8.0 / 32767.0, "{v}");
    }
    assert_eq!(quantize(5.1, -3.0, 5.0), None);
    assert_eq!(quantize(f32::NAN, -3.0, 5.0), None);
    assert_eq!(snorm16(1.0), 32767);
    assert_eq!(snorm16(-2.0), -32767);
}
