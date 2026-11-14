use super::*;
use std::io::Cursor;

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn index(rows: &[(u32, u32)]) -> Vec<u8> {
    rows.iter()
        .flat_map(|(id, offset)| [id.to_le_bytes(), offset.to_le_bytes()].concat())
        .collect()
}
fn localization() -> Localization {
    let descriptors = std::array::from_fn(|_| LocaleDescriptor::decode(&[0; 28]).unwrap());
    let mut tables = std::array::from_fn(|_| LocaleTable::decode(&[], vec![]).unwrap());
    tables[0] = LocaleTable::decode(
        &index(&[(0, 0), (0x03000001, 1), (0x03000001, 7)]),
        b"\0first\0second\0".to_vec(),
    )
    .unwrap();
    tables[1] =
        LocaleTable::decode(&index(&[(0x03000001, 0)]), "雪\0".as_bytes().to_vec()).unwrap();
    Localization {
        descriptors,
        tables,
    }
}
#[test]
fn xbox_id_has_no_set_bits() {
    let id = StringId(0x7f123456);
    assert_eq!(
        id.decode(),
        StringIdParts {
            index: 0x123456,
            length: 127,
        }
    );
    assert_eq!(StringId::EMPTY.decode().index, 0);
    assert_eq!(StringId::NULL.decode().index, 0xffffff);
    assert_eq!(id.index(), id.decode().index as usize);
    assert_eq!(id.length(), id.decode().length as usize);
    assert_eq!(StringId::NULL.resolve(&[]).unwrap(), None);
    assert_eq!(
        StringId::EMPTY.resolve(&[Some(String::new())]).unwrap(),
        Some("")
    );
}
#[test]
fn scoped_lookup_keeps_duplicate_keys_languages_misses_and_empty_distinct() {
    let l = localization();
    let mut unic = Unic::decode(&[0; 52]).unwrap();
    unic.ranges[0] = LanguageRange { start: 2, count: 1 };
    unic.ranges[1] = LanguageRange { start: 0, count: 1 };
    let id = StringId(0x03000001);
    assert_eq!(
        l.first_text(id, Language::English).as_deref(),
        Some("first")
    );
    assert_eq!(
        l.first_utf16(id, Language::English),
        Some("first".encode_utf16().collect())
    );
    assert_eq!(
        l.text_in(&unic, id, Language::English).unwrap().as_deref(),
        Some("second")
    );
    assert_eq!(
        l.text_in(&unic, id, Language::Japanese).unwrap().as_deref(),
        Some("雪")
    );
    assert_eq!(
        l.utf16_in(&unic, id, Language::Japanese).unwrap(),
        Some(vec![0x96ea])
    );
    assert_eq!(l.first_text(id, Language::Portuguese), None);
    assert_eq!(l.first_text(StringId(1), Language::English), None); // high byte matters
    assert_eq!(l.first_text(StringId::NULL, Language::English), None);
    assert_eq!(
        l.first_text(StringId::EMPTY, Language::English).as_deref(),
        Some("")
    );
    assert_eq!(
        l.text_in(&unic, StringId::EMPTY, Language::English)
            .unwrap(),
        None
    );
    l.validate_unic(&unic).unwrap();
}
#[test]
fn unic_requires_retail_size_zero_build_fields_and_bounded_signed_ranges() {
    assert!(Unic::decode(&[0; 51]).is_err());
    assert!(Unic::decode(&[0; 53]).is_err());
    let mut b = [0; 52];
    b[0] = 1;
    assert!(Unic::decode(&b).is_err());
    b[0] = 0;
    b[16..20].copy_from_slice(&[0xff, 0xff, 1, 0]);
    assert!(Unic::decode(&b).is_err());
    b[16..20].copy_from_slice(&[0, 0, 0xff, 0xff]);
    assert!(Unic::decode(&b).is_err());
    b[16..20].copy_from_slice(&[0xff, 0xff, 0, 0]);
    let mut unic = Unic::decode(&b).unwrap();
    localization().validate_unic(&unic).unwrap();
    unic.ranges[8] = LanguageRange { start: 0, count: 1 };
    assert!(localization().validate_unic(&unic).is_err());
    unic.ranges[8] = LanguageRange::default();
    unic.ranges[0] = LanguageRange { start: 2, count: 2 };
    assert!(localization().validate_unic(&unic).is_err());
}
#[test]
fn blob_bounds_terminators_stride_and_nonsequential_offsets() {
    assert!(LocaleTable::decode(&[0; 7], vec![0]).is_err());
    assert!(LocaleTable::decode(&index(&[(1, 1)]), vec![0]).is_err());
    assert!(LocaleTable::decode(&index(&[(1, u32::MAX)]), vec![0]).is_err());
    assert!(LocaleTable::decode(&index(&[(1, 0)]), b"unterminated".to_vec()).is_err());
    let t = LocaleTable::decode(&index(&[(2, 2), (1, 0), (3, 2)]), b"a\0b\0".to_vec()).unwrap();
    assert_eq!(t.text_at(0).as_deref(), Some("b"));
    assert_eq!(t.text_at(1).as_deref(), Some("a"));
    assert_eq!(t.text_at(2), t.text_at(0));
    assert_eq!(t.text_at(3), None);
}
#[test]
fn converter_matches_engine_for_bmp_invalid_truncation_and_nonstandard_input() {
    assert_eq!(
        engine_wide("aé雪\u{e000}".as_bytes()),
        vec![97, 0xe9, 0x96ea, 0xe000]
    );
    assert_eq!(engine_wide(b"a\xe9\x9b"), vec![97]);
    assert_eq!(engine_wide(b"a\xffb\x80c"), vec![97, 98, 99]);
    assert_eq!(engine_wide(b"a\0ignored"), vec![97]);
    assert_eq!(engine_wide(b"\xc1\x81"), vec![65]); // overlong accepted by Xbox
    assert_eq!(engine_wide(b"\xed\xa0\x80"), vec![0xd800]); // surrogate preserved
    assert_eq!(engine_wide("😀".as_bytes()), vec![0xf600]); // Xbox narrows, no surrogate pair
    let t = LocaleTable::decode(&index(&[(1, 0)]), b"a\xe9\x9b\0".to_vec()).unwrap();
    assert_eq!(t.non_utf8_entries(), 1);
    assert_eq!(t.text_at(0).as_deref(), Some("a"));
}
#[test]
fn language_codes_and_slot_order() {
    for (i, code) in ["en", "ja", "de", "fr", "es", "it", "ko", "zh", "pt"]
        .iter()
        .enumerate()
    {
        assert_eq!(code.parse::<Language>().unwrap() as usize, i);
        assert_eq!(Language::ALL[i].code(), *code);
    }
    assert_eq!("JA".parse::<Language>().unwrap(), Language::Japanese);
    assert!("bad".parse::<Language>().is_err());
}

// Entire map invented here: one matg, one unic, one name and one locale row.
fn map_bytes() -> Vec<u8> {
    let mut b = vec![0; 0x2000];
    b[..4].copy_from_slice(b"daeh");
    b[0x7fc..0x800].copy_from_slice(b"toof");
    b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
    for (o, v) in [
        (4, 8),
        (8, 0x2000),
        (16, 0x1000),
        (20, 0x100),
        (24, 0x400),
        (28, 0x400),
        (0x164, 1),
        (0x168, 4),
        (0x16c, 0x800),
        (0x170, 0x804),
    ] {
        put(&mut b, o, v);
    }
    b[0x804..0x808].copy_from_slice(b"key\0");
    for (o, v) in [
        (0, 0x80000020),
        (4, 2),
        (8, 0x80000038),
        (12, u32::MAX),
        (16, 0xa0000000),
        (24, 2),
        (28, FourCC(*b"tags").as_u32()),
    ] {
        put(&mut b, 0x1000 + o, v);
    }
    for (i, group) in [*b"matg", *b"unic"].iter().enumerate() {
        put(&mut b, 0x1020 + i * 12, FourCC(*group).as_u32());
        put(&mut b, 0x1024 + i * 12, u32::MAX);
        put(&mut b, 0x1028 + i * 12, u32::MAX);
        put(&mut b, 0x1038 + i * 16, FourCC(*group).as_u32());
        put(&mut b, 0x103c + i * 16, 0xa0000000 + i as u32);
    }
    put(&mut b, 0x1040, 0x90000000);
    put(&mut b, 0x1044, 0x284);
    put(&mut b, 0x1050, 0x90000300);
    put(&mut b, 0x1054, 52);
    for (o, v) in [(8, 1), (12, 7), (16, 0x900), (20, 0x908), (24, 0xdeadbe00)] {
        put(&mut b, 0x1288 + o, v);
    }
    put(&mut b, 0x900, 0x03000000);
    put(&mut b, 0x904, 0);
    b[0x908..0x90f].copy_from_slice("invent\0".as_bytes());
    put(&mut b, 0x1410, 1 << 16);
    b
}
#[test]
fn reads_locale_file_offsets_inside_bounded_disc_region() {
    let raw = map_bytes();
    let mut disc = vec![0xcc; 123];
    disc.extend(&raw);
    disc.extend([0x55; 256]);
    let mut m = Map::from_region(Cursor::new(disc), 123, raw.len() as u64).unwrap();
    let l = Localization::read(&mut m).unwrap();
    let unic = Unic::read(&mut m, TagId(0xa0000001)).unwrap();
    l.validate_unic(&unic).unwrap();
    assert_eq!(
        l.text_in(&unic, StringId(0x03000000), Language::English)
            .unwrap()
            .as_deref(),
        Some("invent")
    );
    assert!(Unic::read(&mut m, TagId(0xa0000000)).is_err());
    // Serialized runtime words are not resident host pointers; high loaded bytes are opaque.
    assert_eq!(l.descriptors[0].loaded, 0xdeadbe00);
    for (at, value) in [
        (0x1290, u32::MAX),
        (0x1298, 0x1ffc),
        (0x129c, 0x1fff),
        (0x904, 7),
    ] {
        let mut bad = raw.clone();
        put(&mut bad, at, value);
        let mut m = Map::from_reader(Cursor::new(bad)).unwrap();
        assert!(Localization::read(&mut m).is_err(), "{at:#x}: {value:#x}");
    }
}

#[test]
fn ninth_slot_and_nonempty_blob_with_zero_rows_are_validated() {
    let mut raw = map_bytes();
    let first = raw[0x1288..0x12a4].to_vec();
    raw[0x1368..0x1384].copy_from_slice(&first);
    put(&mut raw, 0x1430, 1 << 16);
    let mut map = Map::from_reader(Cursor::new(raw.clone())).unwrap();
    let l = Localization::read(&mut map).unwrap();
    let unic = Unic::read(&mut map, TagId(0xa0000001)).unwrap();
    l.validate_unic(&unic).unwrap();
    assert_eq!(
        l.text_in(&unic, StringId(0x03000000), Language::Portuguese)
            .unwrap()
            .as_deref(),
        Some("invent")
    );
    put(&mut raw, 0x1370, 0); // no rows, but still a nonempty blob
    put(&mut raw, 0x137c, 0x1fff);
    let mut map = Map::from_reader(Cursor::new(raw)).unwrap();
    assert!(Localization::read(&mut map).is_err());
}

#[test]
fn scoped_lookup_distinguishes_misses_from_corrupt_ranges() {
    let l = localization();
    let mut unic = Unic::decode(&[0; 52]).unwrap();
    unic.ranges[0] = LanguageRange { start: 0, count: 1 };
    assert!(
        l.text_in(&unic, StringId(123), Language::English)
            .unwrap()
            .is_none()
    );
    assert!(
        l.utf16_in(&unic, StringId(123), Language::English)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        l.text_in(&unic, StringId::EMPTY, Language::English)
            .unwrap()
            .as_deref(),
        Some("")
    );
    unic.ranges[0].count = 4;
    assert!(l.text_in(&unic, StringId(123), Language::English).is_err());
    assert!(l.utf16_in(&unic, StringId(123), Language::English).is_err());
}

#[test]
fn unic_helper_reads_and_checks_every_language_before_returning() {
    let mut m = Map::from_reader(Cursor::new(map_bytes())).unwrap();
    let l = Localization::read(&mut m).unwrap();
    assert!(l.unic(&mut m, TagId(0xa0000001)).is_ok());
    assert!(l.unic(&mut m, TagId(0xa0000000)).is_err());
    let mut raw = map_bytes();
    put(&mut raw, 0x1430, 1 << 16); // Portuguese is empty in the locale table.
    let mut m = Map::from_reader(Cursor::new(raw)).unwrap();
    assert!(Unic::read(&mut m, TagId(0xa0000001)).is_ok());
    assert!(l.unic(&mut m, TagId(0xa0000001)).is_err());
}

#[test]
fn unresolvable_name_ids_do_not_prevent_full_key_lookup() {
    // Name length mismatch, missing name index, and the name-table null sentinel.
    for key in [0x02000000, 0x03000001, u32::MAX] {
        let mut raw = map_bytes();
        put(&mut raw, 0x900, key);
        let mut m = Map::from_reader(Cursor::new(raw)).unwrap();
        let l = Localization::read(&mut m).unwrap();
        let unic = l.unic(&mut m, TagId(0xa0000001)).unwrap();
        assert_eq!(
            l.text_in(&unic, StringId(key), Language::English)
                .unwrap()
                .as_deref(),
            Some("invent")
        );
        assert_eq!(
            l.first_text(StringId(key), Language::English).as_deref(),
            Some("invent")
        );
    }
}

#[test]
fn short_matg_fails_before_reading_descriptors() {
    let mut raw = map_bytes();
    put(&mut raw, 0x1044, 0x283);
    let mut m = Map::from_reader(Cursor::new(raw)).unwrap();
    assert!(matches!(
        Localization::read(&mut m),
        Err(Error::Invalid("short matg locales"))
    ));
}

#[test]
fn encoded_byte_limit_includes_index_and_accepts_exact_boundary() {
    let index = index(&[(1, 0)]);
    let table = LocaleTable::decode(&index, vec![0; MAX_BYTES as usize - index.len()]).unwrap();
    assert_eq!(table.entries().len(), 1);
    drop(table);
    assert!(matches!(
        LocaleTable::decode(&index, vec![0; MAX_BYTES as usize - index.len() + 1]),
        Err(Error::Map(map::Error::Limit))
    ));
}

#[test]
fn repeated_offsets_cannot_amplify_scanning_past_budget() {
    // One MiB string, aliased 65 times: small encoded input, >64 MiB scanning.
    let mut blob = vec![b'x'; 1024 * 1024];
    *blob.last_mut().unwrap() = 0;
    let rows = vec![(1, 0); 65];
    let table = LocaleTable::decode(&index(&rows[..64]), blob.clone()).unwrap();
    assert_eq!(table.entries().len(), 64); // exact scan budget includes NUL
    drop(table);
    assert!(matches!(
        LocaleTable::decode(&index(&rows), blob),
        Err(Error::Invalid("locale string scan limit exceeded"))
    ));
    assert!(matches!(
        LocaleTable::decode(&index(&[(1, 0)]), b"no terminator".to_vec()),
        Err(Error::Invalid("unterminated locale string"))
    ));
}
