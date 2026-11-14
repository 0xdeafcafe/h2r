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
