use super::*;
use std::io::Cursor;

const INDEX: usize = 0x1000;
const TAGS: usize = INDEX + 32 + 5 * 12;
const META: u32 = 0x9000_0000;
const BSP: u32 = 0x8001_0000;
fn put(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn id(slot: u32) -> TagId {
    TagId(0xa000_0000 | slot)
}

// Entirely invented layout and strings; no fixture comes from a game file.
fn fixture() -> Vec<u8> {
    let mut b = vec![0; 0x2000];
    b[..4].copy_from_slice(b"daeh");
    b[0x7fc..0x800].copy_from_slice(b"toof");
    for (o, v) in [
        (4, 8),
        (8, 0x2000),
        (16, 0x1000),
        (20, 0x200),
        (24, 0x600),
        (28, 0x800),
    ] {
        put(&mut b, o, v);
    }
    b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
    b[0x198..0x19c].copy_from_slice(b"test");
    // Two indexed strings plus a missing entry. Index order differs from blob order.
    put(&mut b, 0x164, 3);
    put(&mut b, 0x168, 12);
    put(&mut b, 0x16c, 0x820);
    put(&mut b, 0x170, 0x800);
    b[0x800..0x80c].copy_from_slice(b"alpha\0bravo\0");
    put(&mut b, 0x820, 6);
    put(&mut b, 0x824, 0);
    put(&mut b, 0x828, u32::MAX);
    // Deliberately share a name between classes.
    let names = b"same\0scene\0terrain\0light\0";
    b[0x900..0x900 + names.len()].copy_from_slice(names);
    put(&mut b, 0x2c0, 6);
    put(&mut b, 0x2c4, 0x900);
    put(&mut b, 0x2c8, names.len() as u32);
    put(&mut b, 0x2cc, 0x880);
    for (i, v) in [0, 5, 11, 19, 0, u32::MAX].into_iter().enumerate() {
        put(&mut b, 0x880 + i * 4, v);
    }
    let classes = [*b"matg", *b"scnr", *b"sbsp", *b"ltmp", *b"bitm"];
    for (i, c) in classes.into_iter().enumerate() {
        let at = INDEX + 32 + i * 12;
        put(&mut b, at, FourCC(c).as_u32());
        put(&mut b, at + 4, u32::MAX);
        put(&mut b, at + 8, u32::MAX);
        let t = TAGS + i * 16;
        put(&mut b, t, FourCC(c).as_u32());
        put(&mut b, t + 4, id(i as u32).0);
    }
    for (o, v) in [
        (0, 0x8000_0020),
        (4, 5),
        (8, 0x8000_005c),
        (12, id(1).0),
        (16, id(0).0),
        (24, 6),
        (28, FourCC(*b"tags").as_u32()),
    ] {
        put(&mut b, INDEX + o, v);
    }
    put(&mut b, TAGS + 8, META);
    put(&mut b, TAGS + 12, 16);
    put(&mut b, TAGS + 16 + 8, META + 0x20);
    put(&mut b, TAGS + 16 + 12, 0x300);
    put(&mut b, TAGS + 64 + 8, META + 0x500);
    put(&mut b, TAGS + 64 + 12, 16);
    b[TAGS + 80..TAGS + 96].fill(0xff);
    b[0x1200..0x1210].fill(0x42);
    put(&mut b, 0x1220 + 0x210, 1);
    put(&mut b, 0x1220 + 0x214, META + 0x400);
    for (o, v) in [
        (0, 0xa00),
        (4, 0x80),
        (8, BSP),
        (0x14, id(2).0),
        (0x1c, id(3).0),
    ] {
        put(&mut b, 0x1600 + o, v);
    }
    for (o, v) in [
        (0, 0x40),
        (4, BSP + 16),
        (8, BSP + 32),
        (12, FourCC(*b"sbsp").as_u32()),
    ] {
        put(&mut b, 0xa00 + o, v);
    }
    b[0xa10..0xa20].fill(0x55);
    b[0xa20..0xa40].fill(0x66);
    b
}
fn parse(b: Vec<u8>) -> Result<Map<Cursor<Vec<u8>>>> {
    Map::from_reader(Cursor::new(b))
}

#[test]
fn header_tables_identity_and_payloads() {
    let mut m = parse(fixture()).unwrap();
    assert_eq!(m.header().name, "test");
    assert_eq!(m.index_slots(), 6);
    assert_eq!(m.tags().len(), 5);
    assert_eq!(
        m.strings(),
        &[Some("bravo".into()), Some("alpha".into()), None]
    );
    assert_eq!(m.scenario().unwrap().id, id(1));
    assert_eq!(m.globals().unwrap().id, id(0));
    assert_eq!(m.tag("scene").unwrap().group, FourCC(*b"scnr"));
    assert!(matches!(m.tag("same"), Err(Error::AmbiguousName)));
    assert_eq!(
        m.tag(TagQuery::NameInGroup("same", FourCC(*b"bitm")))
            .unwrap()
            .id,
        id(4)
    );
    assert!(matches!(m.tag(0xb000_0000), Err(Error::TagNotFound))); // same slot, stale salt
    assert!(matches!(m.tag(TagId::NULL), Err(Error::TagNotFound)));
    assert_eq!(m.read_tag_bytes(id(0)).unwrap(), vec![0x42; 16]);
    assert_eq!(m.index_space().translate(0x8000_0020, 12).unwrap(), 0x1020);
    assert_eq!(m.file_offset(META + 0x20, 0x300).unwrap(), 0x1220);
}
#[test]
fn bsp_and_lightmap_use_their_own_address_space() {
    let mut m = parse(fixture()).unwrap();
    assert_eq!(m.tag(id(2)).unwrap().index_address, 0);
    assert_eq!(m.read_tag_bytes(id(2)).unwrap(), vec![0x55; 16]);
    assert_eq!(m.read_tag_bytes(id(3)).unwrap(), vec![0x66; 32]);
    assert!(m.file_offset(BSP + 16, 1).is_err());
    assert!(
        m.tag(id(2))
            .unwrap()
            .location
            .unwrap()
            .space
            .translate(BSP + 0x7f, 2)
            .is_err()
    );
}
#[test]
fn shared_cache_empty_bsp_is_not_an_empty_payload() {
    let mut b = fixture();
    put(&mut b, 0x1600, 0);
    let mut m = parse(b).unwrap();
    assert!(matches!(m.read_tag_bytes(id(2)), Err(Error::NoTagData)));
}
#[test]
fn regions_cannot_read_the_next_container_entry() {
    let mut b = vec![0xcc; 73];
    b.extend(fixture());
    b.extend(vec![0; 0x3000]);
    let m = Map::from_region(Cursor::new(b.clone()), 73, 0x2000).unwrap();
    assert_eq!(m.file_offset(META, 16).unwrap(), 0x1200);
    put(&mut b, 73 + 8, 0x3000);
    assert!(Map::from_region(Cursor::new(b), 73, 0x2000).is_err());
    assert!(Map::from_region(Cursor::new(fixture()), u64::MAX, 1).is_err());
}
#[test]
fn rejects_truncation_and_wrong_platform() {
    for len in [0, 4, 0x7ff, 0x1000, 0x1fff] {
        assert!(parse(fixture()[..len].to_vec()).is_err());
    }
    for at in [0, 0x7fc, INDEX + 28] {
        let mut b = fixture();
        b[at] ^= 1;
        assert!(parse(b).is_err());
    }
    let mut b = fixture();
    put(&mut b, 4, 7);
    assert!(matches!(parse(b), Err(Error::Unsupported(_))));
    let mut b = fixture();
    b[0x120] = b'1';
    assert!(matches!(parse(b), Err(Error::Unsupported(_))));
}
#[test]
fn rejects_corrupt_counts_pointers_names_and_salts() {
    for (at, v) in [
        (0x164, u32::MAX),
        (INDEX + 24, 65537),
        (INDEX + 8, 0x7fff_ffff),
        (0x820, 12),
        (0x820, u32::MAX - 1),
        (TAGS + 4, 0xa000_0007),
        (TAGS + 64 + 8, META + 0x600),
        (TAGS + 64 + 12, u32::MAX),
        (0x2c0, 5),
        (0x1600 + 0x14, 0xb000_0002),
        (0xa00 + 8, BSP - 1),
        (0xa00, 0x81),
    ] {
        let mut b = fixture();
        put(&mut b, at, v);
        assert!(parse(b).is_err(), "accepted mutation at {at:#x}");
    }
    let mut b = fixture();
    b[0x80b] = b'x';
    assert!(parse(b).is_err());
    let mut b = fixture();
    b[0x800] = 0xff;
    assert!(matches!(parse(b), Err(Error::Encoding(_))));
}
#[test]
fn supports_missing_names_and_empty_index() {
    let mut b = fixture();
    put(&mut b, 0x2c0, 0);
    let m = parse(b).unwrap();
    assert!(m.tags().iter().all(|t| t.name.is_none()));
    let mut b = fixture();
    put(&mut b, INDEX + 24, 0);
    put(&mut b, 0x2c0, 0);
    put(&mut b, INDEX + 12, u32::MAX);
    put(&mut b, INDEX + 16, u32::MAX);
    let m = parse(b).unwrap();
    assert!(m.tags().is_empty());
    assert!(m.metadata_space().is_none());
}
#[test]
fn raw_selectors_fourcc_and_nulls() {
    for (i, file) in [
        ResourceFile::Local,
        ResourceFile::MainMenu,
        ResourceFile::Shared,
        ResourceFile::SinglePlayerShared,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            RawPointer::decode((i as u32) << 30 | 0x1234),
            Some(RawPointer {
                file,
                offset: 0x1234
            })
        );
    }
    assert_eq!(RawPointer::decode(u32::MAX), None);
    assert_eq!(
        FourCC::from_u32(u32::from_le_bytes(*b"!dns")).to_string(),
        "snd!"
    );
    assert_eq!("snd!".parse::<FourCC>().unwrap(), FourCC(*b"snd!"));
    assert!("abcdx".parse::<FourCC>().is_err());
}

#[test]
fn bsp_without_a_lightmap_uses_the_remaining_region() {
    let mut b = fixture();
    put(&mut b, 0x1600 + 0x1c, u32::MAX);
    put(&mut b, 0xa00 + 8, 0);
    let mut m = parse(b).unwrap();
    let bytes = m.read_tag_bytes(id(2)).unwrap();
    assert_eq!(bytes.len(), 0x30);
    assert_eq!(&bytes[..16], &[0x55; 16]);
    assert!(matches!(m.read_tag_bytes(id(3)), Err(Error::NoTagData)));
}

#[test]
fn bsps_can_reuse_virtual_addresses_without_aliasing_file_data() {
    let mut b = fixture();
    put(&mut b, INDEX + 24, 8);
    put(&mut b, 0x2c0, 0);
    put(&mut b, 0x1220 + 0x210, 2);
    let descriptor = b[0x1600..0x1644].to_vec();
    b[0x1644..0x1688].copy_from_slice(&descriptor);
    put(&mut b, 0x1644, 0xb00);
    put(&mut b, 0x1644 + 0x14, id(6).0);
    put(&mut b, 0x1644 + 0x1c, id(7).0);
    let header = b[0xa00..0xa10].to_vec();
    b[0xb00..0xb10].copy_from_slice(&header);
    b[0xb10..0xb20].fill(0x77);
    for (slot, class) in [(6, *b"sbsp"), (7, *b"ltmp")] {
        put(&mut b, TAGS + slot * 16, FourCC(class).as_u32());
        put(&mut b, TAGS + slot * 16 + 4, id(slot as u32).0);
    }
    let mut m = parse(b).unwrap();
    assert_eq!(m.read_tag_bytes(id(2)).unwrap(), vec![0x55; 16]);
    assert_eq!(m.read_tag_bytes(id(6)).unwrap(), vec![0x77; 16]);
    assert_eq!(
        m.tag(id(6))
            .unwrap()
            .location
            .unwrap()
            .file_offset()
            .unwrap(),
        0xb10
    );
}

#[test]
fn public_file_ranges_obey_declared_map_bounds_and_read_budget() {
    let raw = fixture();
    let mut container = vec![0xaa; 37];
    container.extend(&raw);
    container.extend([0xbb; 32]);
    let mut map = Map::from_region(Cursor::new(container), 37, raw.len() as u64 + 32).unwrap();
    assert_eq!(map.read_file_range(0x800, 5).unwrap(), b"alpha");
    assert!(map.read_file_range(0x1fff, 2).is_err());
    assert!(map.read_file_range(u64::MAX, 1).is_err());
    assert!(matches!(
        map.read_file_range(0, 64 * 1024 * 1024 + 1),
        Err(Error::Limit)
    ));
    assert!(map.read_file_range(0x2000, 0).unwrap().is_empty());
}

#[test]
fn lossless_writer_preserves_unknown_bytes_padding_and_container_bounds() {
    let mut bytes = fixture();
    bytes[0x300..0x700].fill(0x95); // uninterpreted header, not disposable padding
    bytes[0x1900..].fill(0x7b); // opaque raw/padding
    bytes.extend([0x56; 29]); // physical storage tail, beyond declared size
    let mut container = vec![0xcc; 37];
    container.extend(&bytes);
    container.extend([0xee; 100]);
    let mut map = Map::from_region(Cursor::new(container), 37, bytes.len() as u64).unwrap();
    let mut output = Vec::new();
    assert_eq!(map.write_to(&mut output).unwrap(), bytes.len() as u64);
    assert_eq!(output, bytes);
    let mut editor_output = Cursor::new(Vec::new());
    map.edit().write_to(&mut editor_output).unwrap();
    assert_eq!(editor_output.into_inner(), bytes);
}

#[test]
fn edits_rebuild_name_offsets_preserve_payloads_and_checksum_physical_tail() {
    let mut bytes = fixture();
    bytes.extend([0x89; 24]);
    let mut map = parse(bytes.clone()).unwrap();
    let mut edit = map.edit();
    edit.rename_tag(id(4), "new\\longer\\name").unwrap();
    edit.patch_tag(id(0), 3, &[7, 8, 9, 10]).unwrap();
    edit.patch_tag(id(2), 0, &[0xab]).unwrap(); // BSP-specific address space
    let mut output = Cursor::new(vec![0xcc; 37]);
    output.set_position(37);
    let size = edit.write_to(&mut output).unwrap();
    assert_eq!(output.position(), 37 + size);
    let result = &output.get_ref()[37..];
    let mut reread = Map::from_reader(Cursor::new(result)).unwrap();
    reread.validate_checksum().unwrap();
    assert_eq!(
        reread.tag(id(4)).unwrap().name.as_deref(),
        Some("new\\longer\\name")
    );
    assert_eq!(reread.strings(), map.strings());
    assert_eq!(
        reread.tag(id(0)).unwrap().name,
        map.tag(id(0)).unwrap().name
    );
    assert_eq!(reread.tag(id(0)).unwrap().index_address, META);
    assert_eq!(&reread.read_tag_bytes(id(0)).unwrap()[3..7], &[7, 8, 9, 10]);
    assert_eq!(reread.read_tag_bytes(id(2)).unwrap()[0], 0xab);
    assert_eq!(&result[INDEX..INDEX + 0x200], &bytes[INDEX..INDEX + 0x200]);
    assert_eq!(&result[0x2000..0x2018], &bytes[0x2000..]);
    assert!(reread.header().tag_names.data_offset as usize >= bytes.len());
    assert_eq!(reread.header().tag_names.index_offset % 512, 0);
    assert_eq!(size % 512, 0);
    assert_eq!(reread.header().tag_names.data_offset % 512, 0);
    // Only explicit patches and the documented header fields change in the old file.
    let allowed = [8..12, 0x2c0..0x2d4, 0x1203..0x1207, 0xa10..0xa11];
    for i in 0..bytes.len() {
        if !allowed.iter().any(|r| r.contains(&i)) {
            assert_eq!(result[i], bytes[i], "offset {i:#x}");
        }
    }
}

#[test]
fn edits_reject_stale_handles_bounds_overlaps_and_nuls() {
    let mut map = parse(fixture()).unwrap();
    let mut edit = map.edit();
    assert!(edit.patch_tag(TagId(0xb0000000), 0, &[1]).is_err());
    assert!(edit.patch_tag(id(0), u32::MAX, &[1]).is_err());
    assert!(edit.patch_tag(id(0), 15, &[1, 2]).is_err());
    assert!(edit.rename_tag(id(0), "bad\0name").is_err());
    edit.patch_tag(id(0), 3, &[1, 2, 3]).unwrap();
    assert!(edit.patch_tag(id(0), 4, &[4]).is_err());
    assert!(edit.patch_tag(id(0), 2, &[4, 5]).is_err());
    edit.patch_tag(id(0), 6, &[4]).unwrap();
}

#[test]
fn renamed_sparse_and_stripped_tables_keep_missing_distinct_from_empty() {
    for stripped in [false, true] {
        let mut bytes = fixture();
        if stripped {
            put(&mut bytes, 0x2c0, 0);
        }
        let mut map = parse(bytes).unwrap();
        let mut edit = map.edit();
        edit.rename_tag(id(0), "").unwrap();
        let mut output = Cursor::new(Vec::new());
        edit.write_to(&mut output).unwrap();
        let mut result = Map::from_reader(output).unwrap();
        assert_eq!(result.tag(id(0)).unwrap().name.as_deref(), Some(""));
        assert_eq!(result.index_slots(), 6);
        if stripped {
            assert!(result.tag(id(1)).unwrap().name.is_none());
        }
        let table = result.header().tag_names;
        let last = result
            .read_file_range(u64::from(table.index_offset) + 20, 4)
            .unwrap();
        assert_eq!(last, u32::MAX.to_le_bytes());
        result.validate_checksum().unwrap();
    }
}

#[test]
fn checksum_detects_raw_mutation_and_includes_storage_padding() {
    let mut bytes = fixture();
    bytes.extend([0x12; 8]);
    let expected = checksum::engine_xor(&bytes[0x800..]);
    put(&mut bytes, 0x2d0, expected);
    parse(bytes.clone()).unwrap().validate_checksum().unwrap();
    bytes[0x1900] ^= 1;
    assert!(parse(bytes.clone()).unwrap().validate_checksum().is_err());
    bytes[0x1900] ^= 1;
    bytes[0x2000] ^= 1;
    assert!(parse(bytes).unwrap().validate_checksum().is_err());
}

#[test]
fn patch_crossing_stream_chunks_and_writes_report_io_failure() {
    let mut bytes = fixture();
    bytes.resize(0x110000, 0);
    put(&mut bytes, 8, 0x110000);
    put(&mut bytes, 0x18, 0x110000 - 0x1200);
    // Make the last ordinary tag straddle the writer's first body chunk.
    put(&mut bytes, TAGS + 64 + 8, META + 0x1007fc - 0x1200);
    let mut map = parse(bytes.clone()).unwrap();
    let mut edit = map.edit();
    edit.patch_tag(id(4), 0, &[0x57; 16]).unwrap();
    let mut output = Cursor::new(Vec::new());
    edit.write_to(&mut output).unwrap();
    assert_eq!(&output.get_ref()[0x1007fc..0x10080c], &[0x57; 16]);
    let mut reread = Map::from_reader(output).unwrap();
    reread.validate_checksum().unwrap();
    let mut short = [0; 8];
    assert!(map.write_to(&mut short.as_mut_slice()).is_err());
}

#[test]
fn name_rebuild_preserves_names_even_for_null_index_slots() {
    let mut bytes = fixture();
    put(&mut bytes, 0x880 + 5 * 4, 0); // null slot with an existing name
    let mut map = parse(bytes).unwrap();
    let mut edit = map.edit();
    edit.rename_tag(id(1), "different").unwrap();
    let mut out = Cursor::new(Vec::new());
    edit.write_to(&mut out).unwrap();
    let mut map = Map::from_reader(out).unwrap();
    let table = map.header().tag_names;
    let index = map.read_file_range(table.index_offset.into(), 24).unwrap();
    let offset = u32::from_le_bytes(index[20..24].try_into().unwrap());
    let value = map
        .read_file_range(u64::from(table.data_offset) + u64::from(offset), 5)
        .unwrap();
    assert_eq!(value, b"same\0");
}

#[test]
fn hostile_tables_are_rejected_before_writing_an_unreadable_map() {
    // (string index offset, string data offset): tables crossing the header
    // (rewritten on write) must be rejected, not silently corrupted.
    for (index, data) in [(0x2d2, 0x800), (0x820, 0x2cb)] {
        let mut bytes = fixture();
        put(&mut bytes, 0x16c, index);
        put(&mut bytes, 0x170, data);
        let mut map = parse(bytes).unwrap();
        let mut edit = map.edit();
        edit.rename_tag(id(0), "x").unwrap();
        assert!(edit.write_to(&mut Cursor::new(Vec::new())).is_err());
        let mut edit = map.edit();
        edit.patch_tag(id(0), 0, &[1]).unwrap();
        assert!(edit.write_to(&mut Cursor::new(Vec::new())).is_err());
    }
}

#[test]
fn patches_over_the_index_or_tables_are_rejected() {
    // String index at 0x1215..0x1221 overlaps the first byte of tag 1's payload.
    let mut bytes = fixture();
    put(&mut bytes, 0x16c, 0x1215);
    bytes[0x1215..0x1221].fill(0xff);
    let mut map = parse(bytes).unwrap();
    let mut edit = map.edit();
    assert!(edit.patch_tag(id(1), 0, &[1]).is_err());
    edit.patch_tag(id(0), 0, &[1]).unwrap();
}

#[test]
fn low_offset_patch_is_rejected_by_the_header_guard() {
    let mut bytes = fixture();
    put(&mut bytes, 0x1600, 0x400); // BSP payload now starts inside the header
    let moved = bytes[0xa00..0xa40].to_vec();
    bytes[0x400..0x440].copy_from_slice(&moved);
    let mut map = parse(bytes).unwrap();
    let mut edit = map.edit();
    assert!(edit.patch_tag(id(2), 0, &[1]).is_err());
}

fn sized_multiplayer(len: usize) -> Map<Cursor<Vec<u8>>> {
    let mut bytes = fixture();
    bytes.resize(len, 0);
    put(&mut bytes, 8, len as u32);
    put(&mut bytes, 0x140, 1);
    parse(bytes).unwrap()
}

#[test]
fn engine_size_limits_apply_to_source_and_edited_output() {
    const LIMIT: usize = 0x0500_0000;
    // Source over the limit.
    let mut map = sized_multiplayer(LIMIT + 512);
    let mut edit = map.edit();
    edit.patch_tag(id(0), 0, &[1]).unwrap();
    assert!(edit.write_to(&mut Cursor::new(Vec::new())).is_err());
    // Exactly at the limit: a patch fits, a rename (appends) does not.
    let mut map = sized_multiplayer(LIMIT);
    let mut edit = map.edit();
    edit.patch_tag(id(0), 0, &[1]).unwrap();
    assert_eq!(
        edit.write_to(&mut Cursor::new(Vec::new())).unwrap(),
        LIMIT as u64
    );
    let mut edit = map.edit();
    edit.rename_tag(id(0), "x").unwrap();
    assert!(edit.write_to(&mut Cursor::new(Vec::new())).is_err());
}

/// The fixture with its ordinary metadata ending the file, as in retail maps.
fn fixture_at_end() -> Vec<u8> {
    let mut b = fixture();
    b.truncate(0x1800);
    put(&mut b, 8, 0x1800);
    b
}

#[test]
fn appended_metadata_grows_the_region_and_moves_nothing() {
    let bytes = fixture_at_end();
    let mut map = parse(bytes.clone()).unwrap();
    let mut edit = map.edit();
    assert_eq!(edit.append_metadata(&[1; 5], 4).unwrap(), META + 0x600);
    assert_eq!(edit.append_metadata(&[2; 3], 16).unwrap(), META + 0x610);
    assert_eq!(edit.appended_len(), 0x13);
    // A patch by address, inside a tag and between tags.
    edit.patch_metadata(META + 0x20, &[9]).unwrap();
    edit.patch_metadata(META + 0x10, &[8, 8]).unwrap();
    edit.rename_tag(id(4), "renamed").unwrap();
    let mut out = Cursor::new(Vec::new());
    let n = edit.write_to(&mut out).unwrap();
    let result = out.into_inner();
    assert_eq!(n as usize, result.len());
    let word = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    assert_eq!(word(&result, 0x18), 0x1600);
    assert_eq!(word(&result, 0x1c), 0x1800);
    // The renamed tables follow the appended page.
    assert_eq!(word(&result, 0x2c4), 0x2800);
    assert_eq!(word(&result, 8) as usize, result.len());
    let allowed = [
        8..12,
        0x18..0x20,
        0x2c0..0x2d4,
        0x1210..0x1212,
        0x1220..0x1221,
    ];
    for i in 0..bytes.len() {
        if !allowed.iter().any(|r| r.contains(&i)) {
            assert_eq!(result[i], bytes[i], "offset {i:#x}");
        }
    }
    assert_eq!(&result[0x1800..0x1805], &[1; 5]);
    assert_eq!(&result[0x1805..0x1810], &[0; 11]);
    assert_eq!(&result[0x1810..0x1813], &[2; 3]);
    assert!(result[0x1813..0x2800].iter().all(|&b| b == 0));
    let mut reread = parse(result).unwrap();
    reread.validate_checksum().unwrap();
    let space = reread.metadata_space().unwrap();
    assert_eq!((space.virtual_base, space.size), (META, 0x1600));
    let at = reread.file_offset(META + 0x610, 3).unwrap();
    assert_eq!(reread.read_file_range(at, 3).unwrap(), [2; 3]);
    assert_eq!(reread.read_tag_bytes(id(1)).unwrap()[0], 9);
    assert_eq!(reread.tag(id(4)).unwrap().name.as_deref(), Some("renamed"));
    // Existing tags keep their addresses.
    for (a, b) in reread.tags().iter().zip(map.tags()) {
        assert_eq!(a.index_address, b.index_address);
    }
}

#[test]
fn appending_metadata_is_refused_outside_its_bounds() {
    // The metadata does not end the file.
    let mut map = parse(fixture()).unwrap();
    assert!(map.edit().append_metadata(&[1], 4).is_err());
    // Physical bytes past the declared length.
    let mut b = fixture_at_end();
    b.extend([0; 512]);
    let mut map = parse(b).unwrap();
    assert!(map.edit().append_metadata(&[1], 4).is_err());
    let mut map = parse(fixture_at_end()).unwrap();
    let mut edit = map.edit();
    assert!(edit.append_metadata(&[1], 3).is_err());
    assert!(edit.append_metadata(&[1], 0x2000).is_err());
    // Patches by address stay inside the source region.
    assert!(edit.patch_metadata(META + 0x5ff, &[1, 2]).is_err());
    assert!(edit.patch_metadata(META - 1, &[1]).is_err());
    assert!(edit.patch_metadata(META + 0x5ff, &[1]).is_ok());
    assert!(edit.patch_metadata(META + 0x5ff, &[1]).is_err());
    // A BSP whose own space sits where the region would grow.
    let mut b = fixture_at_end();
    let bsp = META + 0x800;
    put(&mut b, 0x1600 + 8, bsp);
    put(&mut b, 0xa00 + 4, bsp + 16);
    put(&mut b, 0xa00 + 8, bsp + 32);
    let mut map = parse(b).unwrap();
    assert!(map.edit().append_metadata(&[1], 4).is_err());
    // The total metadata size may not pass the engine bound for the type.
    let limit = writer::max_metadata_size(CacheType::Campaign).unwrap();
    let mut b = fixture_at_end();
    put(&mut b, 0x1c, limit - 0x1000);
    let mut map = parse(b).unwrap();
    let mut edit = map.edit();
    assert_eq!(edit.append_metadata(&[1; 0x1000], 4).unwrap(), META + 0x600);
    assert!(edit.append_metadata(&[1], 4).is_err());
    assert_eq!(edit.appended_len(), 0x1000);
    let mut out = Cursor::new(Vec::new());
    edit.write_to(&mut out).unwrap();
    assert_eq!(parse(out.into_inner()).unwrap().header().meta_size, limit);
}
