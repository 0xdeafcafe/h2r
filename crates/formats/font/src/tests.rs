use crate::testutil::*;
use crate::*;

fn font_bytes() -> Vec<u8> {
    build(
        1,
        1,
        0,
        &[
            G {
                unit: 0x20,
                advance: 3,
                x_offset: 0,
                y_offset: 1,
                width: 1,
                height: 1,
                data: vec![0x01],
            },
            solid(b'A' as u16, 3, 0),
            solid(b'V' as u16, 3, 1),
            solid(0x3042, 5, 0),
        ],
        &[(b'A', b'V', -1)],
    )
}

#[test]
fn parses_synthetic_font() {
    let bytes = font_bytes();
    let f = Font::parse(&bytes).unwrap();
    assert_eq!(f.header.glyph_count, 4);
    assert_eq!(f.line_height(), 2);
    assert_eq!(f.glyph_index(0x3042), 3);
    assert_eq!(f.glyph_index(0x9999), 0);
    assert_eq!(f.kerning(b'A' as u16, b'V' as u16), -1);
    assert_eq!(f.kerning(b'V' as u16, b'A' as u16), 0);
    assert_eq!(f.kerning(0x3042, b'A' as u16), 0);
    assert_eq!(f.validate().unwrap(), 4);
    let g = f.glyph(b'A' as u16).unwrap();
    assert_eq!(g.pixels, vec![0xffff; 4]);
}

#[test]
fn rejects_bad_headers() {
    let good = font_bytes();
    assert!(matches!(
        Font::parse(&good[..100]),
        Err(Error::Truncated { .. })
    ));
    let mut b = good.clone();
    b[HEADER_OFFSET] ^= 1;
    assert!(matches!(Font::parse(&b), Err(Error::BadMagic(_))));
    let mut b = good.clone();
    b[HEADER_OFFSET + 0x20] = 97;
    assert!(matches!(Font::parse(&b), Err(Error::Invalid(_))));
    // glyph count larger than the file
    let mut b = good.clone();
    b[HEADER_OFFSET + 0xc..HEADER_OFFSET + 0x10].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Font::parse(&b).is_err());
    // every truncation point must be an error or a clean validation, never a panic
    for n in 0..good.len().min(0x41000) {
        if let Ok(f) = Font::parse(&good[..n]) {
            let _ = f.validate();
        }
    }
}

#[test]
fn bad_records_are_errors() {
    let mut b = font_bytes();
    let rec = |i: usize| RECORDS_OFFSET + i * RECORD_LEN;
    // data offset beyond the file
    b[rec(1) + 12..rec(1) + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    let f = Font::parse(&b).unwrap();
    assert!(f.glyph(b'A' as u16).is_err());
    assert!(f.validate().is_err());
    // offset pointing back into the tables
    let mut b = font_bytes();
    b[rec(1) + 12..rec(1) + 16].copy_from_slice(&0u32.to_le_bytes());
    assert!(Font::parse(&b).unwrap().glyph(b'A' as u16).is_err());
    // oversize bitmap
    let mut b = font_bytes();
    b[rec(1) + 4..rec(1) + 6].copy_from_slice(&129u16.to_le_bytes());
    assert!(Font::parse(&b).unwrap().glyph(b'A' as u16).is_err());
    // zero-length data on a non-empty bitmap
    let mut b = font_bytes();
    b[rec(1) + 2..rec(1) + 4].copy_from_slice(&0u16.to_le_bytes());
    assert!(Font::parse(&b).unwrap().glyph(b'A' as u16).is_err());
    // ... but a zero-width placeholder needs no data
    b[rec(1) + 4..rec(1) + 6].copy_from_slice(&0u16.to_le_bytes());
    b[rec(1) + 12..rec(1) + 16].copy_from_slice(&0u32.to_le_bytes());
    assert!(
        Font::parse(&b)
            .unwrap()
            .glyph(b'A' as u16)
            .unwrap()
            .pixels
            .is_empty()
    );
    // compressed size above the engine limit
    let mut b = font_bytes();
    b[rec(1) + 2..rec(1) + 4].copy_from_slice(&0x1001u16.to_le_bytes());
    assert!(Font::parse(&b).unwrap().glyph(b'A' as u16).is_err());
    // character map entry out of range
    let mut b = font_bytes();
    b[CHARMAP_OFFSET + 0x41 * 4..CHARMAP_OFFSET + 0x41 * 4 + 4]
        .copy_from_slice(&99u32.to_le_bytes());
    let f = Font::parse(&b).unwrap();
    assert!(f.glyph(0x41).is_err());
    assert!(f.validate().is_err());
}

#[test]
fn decoder_run_kinds() {
    // literal sets colour 0x123 with alpha 0xf, then a run of 2 transparent
    // pixels, 1 opaque, and a short-run pixel with a 3-bit alpha 5 + 3 more opaque.
    let s = [0x00, 0xf1, 0x23, 0x02, 0x41, 0b10_101_010];
    let p = decode_pixels(&s, 1 + 2 + 1 + 1 + 3).unwrap();
    assert_eq!(
        p,
        vec![
            0xf123, 0x0123, 0x0123, 0xf123, 0xb123, 0xf123, 0xf123, 0xf123
        ]
    );
    // 11 aaa bbb: two pixels, alpha 7 -> 15 and alpha 1 -> 3
    assert_eq!(
        decode_pixels(&[0b11_111_001], 2).unwrap(),
        vec![0xffff, 0x3fff]
    );
    // 10 aaa rrr with r=7: pixel + 2 transparent; r=4: +5
    assert_eq!(decode_pixels(&[0b10_000_111], 3).unwrap(), vec![0x0fff; 3]);
    assert_eq!(decode_pixels(&[0b10_000_100], 6).unwrap(), vec![0x0fff; 6]);
    // r=1 -> +4 opaque
    assert_eq!(
        decode_pixels(&[0b10_000_001], 5).unwrap(),
        vec![0x0fff, 0xffff, 0xffff, 0xffff, 0xffff]
    );
    // n = 0 opaque run emits nothing
    assert_eq!(decode_pixels(&[0x40, 0x41], 1).unwrap(), vec![0xffff]);
}

#[test]
fn decoder_rejects_bad_streams() {
    assert!(
        decode_pixels(&[0x00, 0xff], 1).is_err(),
        "truncated literal"
    );
    assert!(decode_pixels(&[0x00], 1).is_err());
    assert!(decode_pixels(&[0x45], 4).is_err(), "overrun");
    assert!(decode_pixels(&[0x43], 4).is_err(), "underrun");
    assert!(decode_pixels(&[], 1).is_err());
    assert!(decode_pixels(&[0x7f; 64], 100).is_err());
}

#[test]
fn renders_text_with_kerning_and_lines() {
    let bytes = font_bytes();
    let f = Font::parse(&bytes).unwrap();
    let u = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    // A at x=0, advance 3; V kerned -1 and x_offset 1: drawn at 3 - 1 + 1 = 3.
    let b = render(&f, &u("AV"), RenderOptions::default()).unwrap();
    assert_eq!((b.width, b.height), (6, 2));
    let a = |x: usize, y: usize| b.rgba[(y * b.width as usize + x) * 4 + 3];
    assert_eq!(
        [a(0, 0), a(1, 0), a(2, 0), a(3, 0), a(4, 0), a(5, 0)],
        [255, 255, 0, 255, 255, 0]
    );
    // second line starts back at x=0
    let b2 = render(&f, &u("A\r\nA"), RenderOptions::default()).unwrap();
    assert_eq!(b2.height, 4);
    assert_eq!(b2.rgba[(2 * b2.width as usize) * 4 + 3], 255);
    // unmapped unit renders record 0 (blank space); empty string is 1x? and valid
    let b3 = render(&f, &u("\u{4e00}"), RenderOptions::default()).unwrap();
    assert!(b3.rgba.chunks(4).all(|p| p[3] == 0));
    assert!(render(&f, &[], RenderOptions::default()).is_ok());
    // wide kana in the synthetic font
    assert!(render(&f, &u("\u{3042}A"), RenderOptions::default()).is_ok());
}

#[test]
fn glyph_colors_are_optional() {
    // literal word 0xf0f0 (colour 0x0f0, green), then a 1-pixel opaque run in that colour
    let g = G {
        unit: 0x41,
        advance: 2,
        x_offset: 0,
        y_offset: 1,
        width: 2,
        height: 1,
        data: vec![0x00, 0xf0, 0xf0, 0x41],
    };
    let bytes = build(1, 0, 0, &[g], &[]);
    let f = Font::parse(&bytes).unwrap();
    let t = render(
        &f,
        &[0x41],
        RenderOptions {
            color: [10, 20, 30, 255],
            glyph_colors: true,
        },
    )
    .unwrap();
    assert_eq!(&t.rgba[0..4], &[0, 255, 0, 255]);
    let t = render(
        &f,
        &[0x41],
        RenderOptions {
            color: [10, 20, 30, 255],
            glyph_colors: false,
        },
    )
    .unwrap();
    assert_eq!(&t.rgba[0..4], &[10, 20, 30, 255]);
}

#[test]
fn transparent_colour_does_not_panic() {
    let bytes = font_bytes();
    let f = Font::parse(&bytes).unwrap();
    for ca in [0u8, 1] {
        for gc in [false, true] {
            let o = RenderOptions {
                color: [255, 255, 255, ca],
                glyph_colors: gc,
            };
            let b = render(&f, &[b'A' as u16, b'V' as u16], o).unwrap();
            if ca == 0 {
                assert!(b.rgba.chunks(4).all(|p| p[3] == 0));
            }
        }
    }
}

#[test]
fn glyph_cache_has_a_budget() {
    // 65,504 distinct records of 128x128 sharing one stream (`01 111111` x 260 = 16380
    // pixels plus a 4-pixel tail), mapped from consecutive code units.
    let mut stream = vec![0x7f; 260];
    stream.push(0x44);
    let glyphs: Vec<G> = (0u32..65_504)
        .map(|i| G {
            unit: (i + 0x20) as u16,
            advance: 1,
            x_offset: 0,
            y_offset: 1,
            width: 128,
            height: 128,
            data: Vec::new(),
        })
        .collect();
    let mut bytes = build(1, 1, 0, &glyphs, &[]);
    // point every record at one shared stream appended to the file
    let at = bytes.len();
    bytes.extend_from_slice(&stream);
    let h = HEADER_OFFSET + 0x18;
    bytes[h..h + 4].copy_from_slice(&(stream.len() as u32).to_le_bytes());
    for i in 0..glyphs.len() {
        let o = RECORDS_OFFSET + i * RECORD_LEN;
        bytes[o + 2..o + 4].copy_from_slice(&(stream.len() as u16).to_le_bytes());
        bytes[o + 12..o + 16].copy_from_slice(&(at as u32).to_le_bytes());
    }
    let f = Font::parse(&bytes).unwrap();
    assert!(f.glyph_by_index(0).is_ok());
    let text: Vec<u16> = (0x20u32..0x20 + 4000).map(|u| u as u16).collect();
    assert_eq!(
        render(&f, &text, RenderOptions::default()),
        Err(Error::TooLarge)
    );
}

#[test]
fn decode_pixels_validates_want_first() {
    assert!(decode_pixels(&[], usize::MAX).is_err());
    assert!(decode_pixels(&[0x45], 129 * 129).is_err());
    assert!(decode_pixels(&[], 0).unwrap().is_empty());
}

#[test]
fn pixel_data_must_lie_in_declared_region() {
    let rec = |i: usize| RECORDS_OFFSET + i * RECORD_LEN;
    // offset inside the record table (the old check only rejected the char map)
    let mut b = font_bytes();
    b[rec(1) + 12..rec(1) + 16].copy_from_slice(&((rec(2)) as u32).to_le_bytes());
    assert!(Font::parse(&b).unwrap().glyph(b'A' as u16).is_err());
    // data past the declared data_size, though inside the file
    let mut b = font_bytes();
    let h = HEADER_OFFSET + 0x18;
    b[h..h + 4].copy_from_slice(&1u32.to_le_bytes());
    let f = Font::parse(&b).unwrap();
    assert!(f.validate().is_err());
    // declared data_size beyond the file is rejected at parse time
    let mut b = font_bytes();
    b[h..h + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(Font::parse(&b), Err(Error::Truncated { .. })));
}

#[test]
fn render_is_bounded() {
    let bytes = font_bytes();
    let f = Font::parse(&bytes).unwrap();
    let long = vec![0x3042u16; 20_000];
    assert_eq!(
        render(&f, &long, RenderOptions::default()),
        Err(Error::TooLarge)
    );
    let lines = vec![0x0au16; 20_000];
    assert_eq!(
        render(&f, &lines, RenderOptions::default()),
        Err(Error::TooLarge)
    );
}

/// Parses and fully validates every file under `/maps/fonts` on the disc.
/// Needs the disc: `H2_DISC=... cargo test -p font -- --ignored`.
#[test]
#[ignore]
fn every_disc_font_validates() {
    let path = std::env::var_os("H2_DISC").expect("set H2_DISC to the disc image");
    let mut iso =
        xiso::Xiso::open(std::io::BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    let mut n = 0;
    for e in iso.entries().unwrap() {
        let name = e.path.to_ascii_lowercase();
        if e.is_dir || !name.starts_with("/maps/fonts/") || name.ends_with(".txt") {
            continue;
        }
        let bytes = iso.read(&e).unwrap();
        let f = Font::parse(&bytes).unwrap_or_else(|err| panic!("{}: {err}", e.path));
        f.validate()
            .unwrap_or_else(|err| panic!("{}: {err}", e.path));
        n += 1;
    }
    assert!(n >= 30, "only {n} fonts found");
}
