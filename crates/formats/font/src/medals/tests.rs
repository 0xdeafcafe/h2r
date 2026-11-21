use super::*;
use crate::encode::encode_pixels;
use crate::testutil::{G, build, solid};
use crate::{MAX_GLYPH_BYTES, decode_pixels};

/// A font with a space at record 0, the filler at U+FFFF, and a retail
/// medal of `edge` pixels at U+E038 (advance 25, at the ascent, 18).
fn font_with_medal(edge: u16) -> Vec<u8> {
    let n = usize::from(edge) * usize::from(edge);
    build(
        18,
        6,
        0,
        &[
            G {
                unit: 0x20,
                advance: 5,
                x_offset: 0,
                y_offset: 18,
                width: 1,
                height: 1,
                data: vec![0x01],
            },
            solid(0xffff, 3, 0),
            G {
                unit: RETAIL_MEDAL,
                advance: 25,
                x_offset: 0,
                y_offset: 18,
                width: edge,
                height: edge,
                data: encode_pixels(&vec![0xfec7; n]),
            },
        ],
        &[],
    )
}

fn same_picture(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            // A clear pixel's colour is whatever the stream had last.
            x >> 12 == y >> 12 && (x >> 12 == 0 || x == y)
        })
}

#[test]
fn each_picture_is_a_coloured_medal_that_survives_the_compressor() {
    let n = usize::from(art::SIZE) * usize::from(art::SIZE);
    let mut seen: Vec<&[u16]> = Vec::new();
    for (unit, pic) in MEDALS {
        assert_eq!(pic.len(), n, "U+{unit:04X}");
        let at = |x: usize, y: usize| pic[y * usize::from(art::SIZE) + x];
        // A round medal: clear corners, a solid centre.
        assert_eq!(at(0, 0) >> 12, 0, "U+{unit:04X} corner");
        assert_eq!(at(23, 23) >> 12, 0, "U+{unit:04X} corner");
        assert_eq!(at(12, 12) >> 12, 0xf, "U+{unit:04X} centre");
        // In its own colours, not white for the text's colour.
        let colours: std::collections::HashSet<u16> = pic
            .iter()
            .filter(|p| *p >> 12 != 0)
            .map(|p| p & 0xfff)
            .collect();
        assert!(
            colours.len() > 24,
            "U+{unit:04X}: {} colours",
            colours.len()
        );
        let stream = encode_pixels(pic.as_slice());
        assert!(stream.len() <= usize::from(MAX_GLYPH_BYTES), "U+{unit:04X}");
        let back = decode_pixels(&stream, n).unwrap();
        assert!(same_picture(&back, pic.as_slice()), "U+{unit:04X}");
        assert!(!seen.contains(&pic.as_slice()), "U+{unit:04X} repeats");
        seen.push(pic.as_slice());
    }
    // Spare units of the icon range, clear of the input sets'
    // (U+E200..U+E291, docs/input/button-icons.md) and the uniform tail
    // the sets check (U+E300..).
    for (unit, _) in MEDALS {
        assert!((0xe292..0xe300).contains(&unit), "U+{unit:04X}");
    }
}

#[test]
fn the_medals_take_the_retail_medals_place() {
    let raw = font_with_medal(art::SIZE);
    let font = Font::parse(&raw).unwrap();
    let add = replacements(&font);
    assert_eq!(add.len(), 4);
    for r in &add {
        assert_eq!((r.width, r.height), (24, 24));
        assert_eq!((r.advance, r.x_offset, r.y_offset), (25, 0, Some(18)));
    }
    let out = crate::patch::apply(&raw, &add).unwrap();
    let f = Font::parse(&out).unwrap();
    f.validate().unwrap();
    for (unit, pic) in MEDALS {
        assert!(has_glyph(&f, unit), "U+{unit:04X}");
        let g = f.glyph(unit).unwrap();
        assert!(same_picture(&g.pixels, pic.as_slice()), "U+{unit:04X}");
        assert_eq!(g.record.y_offset, 18);
    }
    // The retail medal is as it was.
    let retail = f.glyph(RETAIL_MEDAL).unwrap();
    assert!(retail.pixels.iter().all(|p| *p == 0xfec7));
    // Patched once: a second pass adds nothing.
    assert!(replacements(&f).is_empty());
}

#[test]
fn a_font_without_a_retail_medal_of_that_size_gets_none() {
    let small = font_with_medal(16);
    assert!(replacements(&Font::parse(&small).unwrap()).is_empty());
    let none = build(
        18,
        6,
        0,
        &[solid(0x20, 5, 0), solid(b'A' as u16, 3, 0)],
        &[],
    );
    assert!(replacements(&Font::parse(&none).unwrap()).is_empty());
}

#[test]
fn only_the_message_fonts_want_them() {
    assert!(wants_font("conduit-13"));
    assert!(wants_font("maps\\fonts\\conduit-12"));
    assert!(wants_font("/maps/fonts/CONDUIT-13"));
    assert!(!wants_font("conduit-16"));
    assert!(!wants_font("maps\\fonts\\handel_gothic-14"));
}

/// The disc's two message fonts take the medals beside their own.
/// Needs the disc: `H2_DISC=... cargo test -p font -- --ignored`.
#[test]
#[ignore]
fn the_disc_message_fonts_take_the_medals() {
    let path = std::env::var_os("H2_DISC").expect("set H2_DISC to the disc image");
    let mut iso =
        xiso::Xiso::open(std::io::BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    let mut n = 0;
    for e in iso.entries().unwrap() {
        let name = e.path.to_ascii_lowercase();
        if e.is_dir || !name.starts_with("/maps/fonts/") || !wants_font(&name) {
            continue;
        }
        let raw = iso.read(&e).unwrap();
        let font = Font::parse(&raw).unwrap();
        for (unit, _) in MEDALS {
            assert!(!has_glyph(&font, unit), "{}: U+{unit:04X} taken", e.path);
        }
        let add = replacements(&font);
        assert_eq!(add.len(), 4, "{}", e.path);
        let out = crate::patch::apply(&raw, &add).unwrap();
        let f = Font::parse(&out).unwrap();
        f.validate().unwrap();
        for (unit, pic) in MEDALS {
            let g = f.glyph(unit).unwrap();
            assert!(
                same_picture(&g.pixels, pic.as_slice()),
                "{} U+{unit:04X}",
                e.path
            );
        }
        n += 1;
    }
    assert_eq!(n, 2);
}
