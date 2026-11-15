use super::*;
use crate::decode::Payload;
use crate::records::*;
use crate::wire::{BinaryRecord, DataReference, StringId, TagBlock, TagReference, Wire};
use map::{AddressSpace, FourCC, RawPointer, ResourceFile, TagId, TagLocation};

fn payload(b: &[u8]) -> Payload<'_> {
    Payload {
        bytes: b,
        mode: DecodeMode::Strict,
        warnings: Default::default(),
        location: TagLocation {
            address: 0x8100_0000,
            size: b.len() as u32,
            space: AddressSpace {
                virtual_base: 0x8100_0000,
                file_offset: 0x800,
                size: 0x10000,
            },
        },
    }
}
fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

#[test]
fn every_generated_codec_roundtrips_unknowns_float_bits_and_arrays() {
    fn check<T: BinaryRecord>() {
        // Deterministic synthetic bytes, including arbitrary floats and enums.
        let bytes: Vec<_> = (0..T::SIZE).map(|i| (i * 73 + 151) as u8).collect();
        let value = T::decode(&bytes).unwrap();
        let mut out = vec![0; T::SIZE];
        value.encode_into(&mut out).unwrap();
        assert_eq!(out, bytes, "{}", T::META.name);
        assert!(T::decode(&bytes[..T::SIZE - 1]).is_err());
        assert!(value.encode_into(&mut out[..T::SIZE - 1]).is_err());
        let mut long = bytes.clone();
        long.push(0);
        assert!(T::decode(&long).is_err());
        for field in T::META.fields {
            assert!(field.offset + field.size <= T::SIZE);
        }
    }
    check::<BitmapHeader>();
    check::<BitmapImage>();
    check::<BitmapSequence>();
    check::<BitmapSprite>();
    check::<ScenarioHeader>();
    check::<GlobalsHeader>();
    check::<LocaleDescriptor>();
    check::<BspHeader>();
    check::<RenderModelHeader>();
}
#[test]
fn editing_one_field_preserves_every_other_byte() {
    let before = vec![0xaa; BitmapImage::SIZE];
    let mut image = BitmapImage::decode(&before).unwrap();
    image.width = 1024;
    let mut after = vec![0; BitmapImage::SIZE];
    image.encode_into(&mut after).unwrap();
    assert_eq!(&after[..4], &before[..4]);
    assert_eq!(&after[6..], &before[6..]);
    assert_eq!(&after[4..6], &1024u16.to_le_bytes());
}
#[test]
fn blocks_use_the_owning_tag_not_any_address_in_the_map() {
    let bytes = vec![0; 256];
    let p = payload(&bytes);
    assert_eq!(
        p.block(
            TagBlock {
                count: 4,
                address: 0x8100_0080
            },
            32
        )
        .unwrap()
        .len(),
        128
    );
    assert!(
        p.block(
            TagBlock {
                count: 5,
                address: 0x8100_0080
            },
            32
        )
        .is_err()
    );
    assert!(
        p.block(
            TagBlock {
                count: 1,
                address: 0x80ff_ffff
            },
            1
        )
        .is_err()
    );
    assert!(
        p.block(
            TagBlock {
                count: u32::MAX,
                address: 0x8100_0000
            },
            u32::MAX
        )
        .is_err()
    );
    assert!(
        p.block(
            TagBlock {
                count: 0,
                address: u32::MAX
            },
            0x74
        )
        .unwrap()
        .is_empty()
    );
}
#[test]
fn full_root_validation_rejects_unknown_nonempty_blocks_and_bad_data_refs() {
    let mut bytes = vec![0; ScenarioHeader::SIZE];
    let mut stats = Validation::default();
    payload(&bytes)
        .record::<ScenarioHeader>(0, &mut stats)
        .unwrap();
    assert_eq!(stats.blocks, 9);
    put(&mut bytes, 0x28, 1);
    assert!(matches!(
        payload(&bytes).record::<ScenarioHeader>(0, &mut stats),
        Err(Error::Field { .. })
    ));
    put(&mut bytes, 0x28, 0);
    put(&mut bytes, 0x30, 100);
    put(&mut bytes, 0x34, 0x8100_03d0);
    assert!(
        payload(&bytes)
            .record::<ScenarioHeader>(0, &mut stats)
            .is_err()
    );
}
#[test]
fn pointer_primitives_are_eight_bytes_and_preserve_nulls() {
    assert_eq!(<TagBlock as Wire>::SIZE, 8);
    assert_eq!(<DataReference as Wire>::SIZE, 8);
    assert_eq!(<TagReference as Wire>::SIZE, 8);
    let reference = TagReference {
        group: FourCC(*b"bitm"),
        id: TagId::NULL,
    };
    let mut b = [0; 8];
    reference.write(&mut b).unwrap();
    assert_eq!(&b[..4], b"mtib");
    assert_eq!(TagReference::read(&b).unwrap(), reference);
    assert!(TagBlock::read(&b[..7]).is_err());
}
#[test]
fn string_ids_validate_length_and_keep_null_distinct_from_empty() {
    let strings = vec![Some(String::new()), Some("sample".into()), None];
    assert_eq!(StringId(0).resolve(&strings).unwrap(), Some(""));
    assert_eq!(
        StringId((6 << 24) | 1).resolve(&strings).unwrap(),
        Some("sample")
    );
    assert_eq!(StringId(u32::MAX).resolve(&strings).unwrap(), None);
    assert!(StringId(1).resolve(&strings).is_err());
    assert!(StringId(2).resolve(&strings).is_err());
}
#[test]
fn raw_resources_require_the_selected_file_and_complete_range() {
    let r = ResourceRange {
        pointer: RawPointer {
            file: ResourceFile::Shared,
            offset: 0x100,
        },
        size: 16,
    };
    let mut sizes = ResourceSizes::default();
    sizes.set(ResourceFile::Local, 0x10000);
    assert!(matches!(sizes.validate(r), Err(Error::MissingResource(_))));
    sizes.set(ResourceFile::Shared, 0x110);
    sizes.validate(r).unwrap();
    sizes.set(ResourceFile::Shared, 0x10f);
    assert!(matches!(
        sizes.validate(r),
        Err(Error::ResourceBounds { .. })
    ));
}
#[test]
fn bitmap_depth_is_one_byte_separate_from_flags() {
    let mut b = vec![0; BitmapImage::SIZE];
    b[8] = 3;
    b[9] = 0xff;
    let image = BitmapImage::decode(&b).unwrap();
    assert_eq!(image.depth, 3);
    assert_eq!(image.runtime_flags, 255);
    assert_eq!(PixelFormat::from(65535), PixelFormat::Unknown(65535));
}

// This cache is invented in memory. No fixture or string comes from a game file.
pub(crate) fn bitmap_map() -> Vec<u8> {
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
    put(&mut b, 0x1020, FourCC(*b"bitm").as_u32());
    put(&mut b, 0x1024, u32::MAX);
    put(&mut b, 0x1028, u32::MAX);
    for (at, n) in [
        (0, FourCC(*b"bitm").as_u32()),
        (4, 0xa0000000),
        (8, 0x81000000),
        (12, 0x400),
    ] {
        put(&mut b, 0x102c + at, n);
    }
    put(&mut b, 0x1200 + 0x3c, 1);
    put(&mut b, 0x1200 + 0x40, 0x81000180);
    put(&mut b, 0x1200 + 0x44, 2);
    put(&mut b, 0x1200 + 0x48, 0x81000080);
    for at in [0x1280, 0x12f4] {
        put(&mut b, at, FourCC(*b"bitm").as_u32());
        b[at + 4..at + 6].copy_from_slice(&16u16.to_le_bytes());
        b[at + 6..at + 8].copy_from_slice(&8u16.to_le_bytes());
        b[at + 8] = 1;
        b[at + 0xc] = 14;
        b[at + 0xe] = 2;
        b[at + 0x14] = 3;
        put(&mut b, at + 0x1c, 0x80000100);
        put(&mut b, at + 0x34, 0x80);
    }
    put(&mut b, 0x1380 + 0x34, 1);
    put(&mut b, 0x1380 + 0x38, 0x810001c0);
    b
}
#[test]
fn reads_a_bitmap_end_to_end_and_rejects_corrupt_nested_pointers() {
    use std::io::Cursor;
    let mut m = map::Map::from_reader(Cursor::new(bitmap_map())).unwrap();
    let d = DecodedTag::read(&mut m, TagId(0xa0000000)).unwrap();
    let TagData::Bitmap(ref b) = d.data else {
        panic!("wrong group")
    };
    assert_eq!(b.images.len(), 2);
    assert_eq!(b.sequences.len(), 1);
    assert_eq!(b.sequences[0].sprites.len(), 1);
    assert_eq!(b.images[0].dimensions, [16, 8, 1]);
    assert_eq!(b.images[0].format, PixelFormat::Dxt1);
    assert_eq!(d.validation.records, 5);
    assert_eq!(d.validation.nonempty_blocks, 3);
    let mut sizes = ResourceSizes::default();
    sizes.set(ResourceFile::Shared, 0x180);
    assert_eq!(d.validate_resources(&sizes).unwrap(), 2);
    for (at, value) in [
        (0x1280, 0),
        (0x1380 + 0x38, 0x810003ff),
        (0x1244, u32::MAX),
        (0x1280 + 0x1c, u32::MAX),
        (0x1280 + 0x64, 4),
    ] {
        let mut bytes = bitmap_map();
        put(&mut bytes, at, value);
        let mut m = map::Map::from_reader(Cursor::new(bytes)).unwrap();
        assert!(
            DecodedTag::read(&mut m, TagId(0xa0000000)).is_err(),
            "accepted {at:x}"
        );
    }
}

#[test]
fn locale_blob_bounds_are_checked_even_with_zero_strings() {
    let mut b = bitmap_map();
    b[0x1200..0x1600].fill(0);
    // The two root references are null, not stale slot-zero references.
    put(&mut b, 0x1200 + 0x16c, u32::MAX);
    put(&mut b, 0x1200 + 0x184, u32::MAX);
    put(&mut b, 0x1020, FourCC(*b"matg").as_u32());
    put(&mut b, 0x102c, FourCC(*b"matg").as_u32());
    put(&mut b, 0x1200 + 0x194, 8);
    put(&mut b, 0x1200 + 0x19c, 0x1ffc);
    let mut m = map::Map::from_reader(std::io::Cursor::new(b)).unwrap();
    assert!(matches!(
        DecodedTag::read(&mut m, TagId(0xa0000000)),
        Err(Error::Invalid("locale file range"))
    ));
}

fn root_map<T: BinaryRecord>(group: [u8; 4]) -> Vec<u8> {
    let mut b = bitmap_map();
    b[0x1200..0x1600].fill(0);
    for at in [0x1020, 0x102c] {
        put(&mut b, at, FourCC(group).as_u32());
    }
    for field in T::META.fields {
        if field.kind == crate::wire::FieldKind::TagRef {
            put(&mut b, 0x1200 + field.offset + 4, u32::MAX);
        }
    }
    b
}
fn decode(b: Vec<u8>) -> Result<DecodedTag> {
    let mut m = map::Map::from_reader(std::io::Cursor::new(b))?;
    DecodedTag::read(&mut m, TagId(0xa0000000))
}

#[test]
fn aliased_sprite_blocks_have_a_total_allocation_budget() {
    let mut b = bitmap_map();
    // Each of three spans fits individually. Their sum must also fit the
    // payload-derived record budget (0x400 / 0x20 == 32 sprites).
    put(&mut b, 0x123c, 3);
    for at in [0x1380, 0x13bc, 0x13f8] {
        put(&mut b, at + 0x34, 11);
        put(&mut b, at + 0x38, 0x81000280);
    }
    assert!(matches!(
        decode(b.clone()),
        Err(Error::Invalid("total sprite count exceeds payload budget"))
    ));
    put(&mut b, 0x13f8 + 0x34, 10); // Exactly 32: limited aliasing remains supported.
    let d = decode(b).unwrap();
    let TagData::Bitmap(v) = d.data else {
        panic!("bitmap")
    };
    assert_eq!(
        v.sequences.iter().map(|s| s.sprites.len()).sum::<usize>(),
        32
    );
}

#[test]
fn unused_child_layouts_are_opaque_and_preserved() {
    use crate::wire::FieldKind;
    for meta in [
        ScenarioHeader::META,
        GlobalsHeader::META,
        BspHeader::META,
        RenderModelHeader::META,
    ] {
        assert!(
            !meta
                .fields
                .iter()
                .any(|f| matches!(f.kind, FieldKind::Block(_)))
        );
        assert!(
            meta.fields
                .iter()
                .any(|f| matches!(f.kind, FieldKind::Bytes(_)))
        );
        for f in meta
            .fields
            .iter()
            .filter(|f| f.kind == FieldKind::EmptyBlock)
        {
            assert!(f.name.starts_with("unsupported_"));
        }
    }
    let mut b = root_map::<ScenarioHeader>(*b"scnr");
    // Previously interpreted as a named child block with a source-derived stride.
    // It is now bytes, with no invented pointer validation or child semantics.
    b[0x1240..0x1248].fill(0xa5);
    let d = decode(b.clone()).unwrap();
    let TagData::Scenario(v) = d.data else {
        panic!("scenario")
    };
    let mut out = vec![0; ScenarioHeader::SIZE];
    v.header.encode_into(&mut out).unwrap();
    assert_eq!(out, b[0x1200..0x1200 + ScenarioHeader::SIZE]);
}

#[test]
fn strict_rejects_and_lenient_warns_without_dereferencing_unknown_blocks() {
    let mut b = root_map::<RenderModelHeader>(*b"mode");
    put(&mut b, 0x1200, u32::MAX); // Null name.
    put(&mut b, 0x120c, 7);
    put(&mut b, 0x1210, u32::MAX); // Unknown layout: do not pretend to bound-check.
    let Err(Error::Field { source, .. }) = decode(b.clone()) else {
        panic!("strict guard")
    };
    assert!(matches!(
        *source,
        Error::UnsupportedBlock("unsupported_00c")
    ));
    let mut m = map::Map::from_reader(std::io::Cursor::new(b)).unwrap();
    let d = DecodedTag::read_with_mode(&mut m, TagId(0xa0000000), DecodeMode::Lenient).unwrap();
    assert_eq!(
        d.warnings,
        vec![DecodeWarning {
            record: "RenderModelHeader",
            field: "unsupported_00c",
            block: TagBlock {
                count: 7,
                address: u32::MAX
            },
        }]
    );
    assert_eq!(d.validation.nonempty_blocks, 0);
    let TagData::RenderModel(v) = d.data else {
        panic!("model")
    };
    assert_eq!(v.header.unsupported_00c.count, 7);
}

#[test]
fn reviewed_block_alignment_is_checked_in_both_modes() {
    let mut b = bitmap_map();
    put(&mut b, 0x1248, 0x81000081); // Still in bounds, but not word-aligned.
    for mode in [DecodeMode::Strict, DecodeMode::Lenient] {
        let mut m = map::Map::from_reader(std::io::Cursor::new(b.clone())).unwrap();
        let Err(Error::Field { source, .. }) =
            DecodedTag::read_with_mode(&mut m, TagId(0xa0000000), mode)
        else {
            panic!("alignment")
        };
        assert!(matches!(
            *source,
            Error::Invalid("unaligned reviewed block pointer")
        ));
    }
}

#[test]
fn references_accept_concrete_parent_grandparent_and_reject_class_mismatch() {
    for (group, accepted) in [
        (*b"scnr", true),
        (*b"obje", true),
        (*b"base", true),
        (*b"bitm", false),
    ] {
        let mut b = root_map::<ScenarioHeader>(*b"scnr");
        put(&mut b, 0x1024, FourCC(*b"obje").as_u32());
        put(&mut b, 0x1028, FourCC(*b"base").as_u32());
        put(&mut b, 0x1200, FourCC(group).as_u32());
        put(&mut b, 0x1204, 0xa0000000); // Valid full salted ID, self-reference.
        if accepted {
            assert_eq!(decode(b).unwrap().validation.tag_references, 9);
        } else {
            assert!(matches!(
                decode(b),
                Err(Error::Invalid("tag reference class mismatch"))
            ));
        }
    }
}

#[test]
fn mode_name_resolves_and_rejects_encoded_length_mismatch() {
    let mut b = root_map::<RenderModelHeader>(*b"mode");
    for (at, value) in [(0x164, 1), (0x168, 7), (0x16c, 0x800), (0x170, 0x804)] {
        put(&mut b, at, value);
    }
    b[0x804..0x80b].copy_from_slice(b"sample\0");
    put(&mut b, 0x1200, 6 << 24);
    let d = decode(b.clone()).unwrap();
    let TagData::RenderModel(v) = d.data else {
        panic!("model")
    };
    assert_eq!(v.name.as_deref(), Some("sample"));
    put(&mut b, 0x1200, 5 << 24);
    assert!(matches!(decode(b), Err(Error::Invalid("string ID length"))));
}

#[test]
fn low_detail_ranges_reject_overflow_and_validate_selected_resource_end() {
    let mut b = bitmap_map();
    put(&mut b, 0x158, u32::MAX - 3);
    put(&mut b, 0x15c, 16);
    put(&mut b, 0x1280 + 0x60, 8);
    put(&mut b, 0x1280 + 0x64, 8);
    assert!(matches!(
        decode(b.clone()),
        Err(Error::Invalid("low-detail bitmap offset overflow"))
    ));
    put(&mut b, 0x15c, u32::MAX);
    put(&mut b, 0x1280 + 0x60, u32::MAX);
    assert!(matches!(
        decode(b.clone()),
        Err(Error::Invalid("low-detail bitmap range"))
    ));
    put(&mut b, 0x158, 0x1800);
    put(&mut b, 0x15c, 16);
    put(&mut b, 0x1280 + 0x60, 8);
    let d = decode(b).unwrap();
    let mut sizes = ResourceSizes::default();
    sizes.set(ResourceFile::Shared, 0x180);
    assert!(matches!(
        d.validate_resources(&sizes),
        Err(Error::MissingResource(ResourceFile::Local))
    ));
    sizes.set(ResourceFile::Local, 0x1810);
    assert_eq!(d.validate_resources(&sizes).unwrap(), 3); // Two LODs plus low-detail.
    sizes.set(ResourceFile::Local, 0x180f);
    assert!(matches!(
        d.validate_resources(&sizes),
        Err(Error::ResourceBounds {
            file: ResourceFile::Local,
            offset: 0x1808,
            size: 8,
            length: 0x180f
        })
    ));
    sizes.set(ResourceFile::Local, 0x1810);
    sizes.set(ResourceFile::Shared, 0x17f);
    assert!(matches!(
        d.validate_resources(&sizes),
        Err(Error::ResourceBounds {
            file: ResourceFile::Shared,
            ..
        })
    ));
}

fn bsp_map() -> Vec<u8> {
    let mut b = root_map::<ScenarioHeader>(*b"scnr");
    // Two groups/tags, first tag anchors ordinary metadata. The BSP lives elsewhere.
    b.copy_within(0x102c..0x103c, 0x1038);
    for (at, v) in [
        (0x1004, 2),
        (0x1008, 0x80000038),
        (0x100c, 0xa0000000),
        (0x1018, 2),
        (0x18, 0x600),
        (0x1044, 0x450),
        (0x102c, FourCC(*b"sbsp").as_u32()),
        (0x1030, u32::MAX),
        (0x1034, u32::MAX),
        (0x1048, FourCC(*b"sbsp").as_u32()),
        (0x104c, 0xa0000001),
        (0x1050, 0),
        (0x1054, 0),
        (0x1410, 1),
        (0x1414, 0x81000400),
        (0x1600, 0xa00),
        (0x1604, 0x300),
        (0x1608, 0x80010000),
        (0x1614, 0xa0000001),
        (0x161c, u32::MAX),
        (0xa00, 0x300),
        (0xa04, 0x80010010),
        (0xa0c, FourCC(*b"sbsp").as_u32()),
        (0xa10 + 0x1dc, u32::MAX),
        (0xa10 + 0x54, 0x80),
        (0xa10 + 0x58, 0x80010280),
    ] {
        put(&mut b, at, v);
    }
    b
}
#[test]
fn bsp_data_references_use_the_bsp_address_space_and_payload_end() {
    let b = bsp_map();
    let read = |b| {
        let mut m = map::Map::from_reader(std::io::Cursor::new(b)).unwrap();
        let location = m.tag(TagId(0xa0000001)).unwrap().location.unwrap();
        assert_eq!(location.file_offset().unwrap(), 0xa10);
        DecodedTag::read(&mut m, TagId(0xa0000001))
    };
    let d = read(b.clone()).unwrap();
    assert!(matches!(d.data, TagData::Bsp(_)));
    for address in [0x81000280, 0x80010284, 0x8001000c] {
        let mut corrupt = b.clone();
        put(&mut corrupt, 0xa10 + 0x58, address);
        assert!(matches!(
            read(corrupt),
            Err(Error::Field {
                field: "cluster_data",
                ..
            })
        ));
    }
}

#[test]
fn readable_debug_uses_choices_compact_arrays_none_and_empty_summary() {
    use crate::wire::{Enum16, Flags16};
    assert_eq!(format!("{:#?}", Enum16(14)), "Enum16(0xe)");
    assert_eq!(format!("{:#?}", Flags16(8)), "Flags16(0x8)");
    assert_eq!(format!("{:#?}", StringId(0)), "StringId(0x0)");
    assert_eq!(
        format!(
            "{:#?}",
            TagReference {
                group: FourCC(*b"bitm"),
                id: TagId::NULL
            }
        ),
        "none"
    );
    let mut record = BitmapImage::decode(&[0; BitmapImage::SIZE]).unwrap();
    record.format = Enum16(14); // Formatter reads edits, not the original bytes.
    record.flags = Flags16(0x8008);
    record.lod_sizes = [1, 2, 3];
    let text = format!("{record:#?}");
    assert!(text.contains("format: DXT1 (14)"));
    assert!(text.contains("flags: swizzled | 0x8008"));
    assert!(text.contains("lod_sizes: [0x1, 0x2, 0x3]"));
    record.format = Enum16(65535);
    assert!(format!("{record:#?}").contains("format: 0xffff"));
    let root = BitmapHeader::decode(&[0; BitmapHeader::SIZE]).unwrap();
    let text = format!("{root:#?}");
    assert!(text.contains("empty_blocks: 2"));
    assert!(!text.contains("sequences:"));
    assert!(!text.contains("images:"));
    let text = format!("{:#?}", decode(bitmap_map()).unwrap().data);
    assert!(text.contains("dimensions: [16, 8, 1]"));
    // The generator emits actual multi-line methods, not dense one-line codecs.
    let generated = include_str!(concat!(env!("OUT_DIR"), "/records.rs"));
    assert!(generated.contains("fn decode(bytes: &[u8]) -> crate::Result<Self> {\n        exact"));
}

#[test]
fn locale_descriptors_start_at_188_and_include_the_ninth_record() {
    let mut b = root_map::<GlobalsHeader>(*b"matg");
    put(&mut b, 0x1200 + 0x188, 0x12345678); // Runtime pointer, not string count.
    let last = 0x1200 + 0x188 + 8 * 0x1c;
    put(&mut b, last + 0xc, 8);
    put(&mut b, last + 0x14, 0x1ff8); // Ends exactly at the declared file length.
    let d = decode(b.clone()).unwrap();
    let TagData::Globals(v) = d.data else {
        panic!("globals")
    };
    assert_eq!(v.locales.len(), 9);
    assert_eq!(v.locales[0].runtime_index, 0x12345678);
    assert_eq!(v.locales[0].count, 0);
    assert_eq!(v.locales[8].data_size, 8);
    put(&mut b, last + 0x14, 0x1ff9);
    assert!(matches!(
        decode(b),
        Err(Error::Invalid("locale file range"))
    ));
}
