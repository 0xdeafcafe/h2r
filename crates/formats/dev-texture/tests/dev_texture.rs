//! Slice BO2 of docs/game/forge-blockout.md. Every input is invented.

use dev_texture::family::{self, ALL, MAX_TONE_SPREAD, MIN_FAMILY_CONTRAST};
use dev_texture::{font, label, line_colours, tile, Family, Image, TILE_SIZE};
use sha2::{Digest, Sha256};

fn hash(image: &Image) -> String {
    let mut h = Sha256::new();
    h.update(image.width.to_le_bytes());
    h.update(image.height.to_le_bytes());
    h.update(&image.rgba);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A table of faces: shape, width, height, the text its plate reads.
const FACES: [(&str, u32, u32, [&str; 3]); 6] = [
    ("BLOCK", 256, 128, ["BLOCK", "256 \u{d7} 128 U", "7.80 \u{d7} 3.90 M"]),
    ("WALL", 512, 256, ["WALL", "512 \u{d7} 256 U", "15.61 \u{d7} 7.80 M"]),
    ("FLOOR", 1024, 1024, ["FLOOR", "1024 \u{d7} 1024 U", "31.21 \u{d7} 31.21 M"]),
    ("RAMP", 64, 40, ["RAMP", "64 \u{d7} 40 U", "1.95 \u{d7} 1.22 M"]),
    ("CYLINDER", 128, 64, ["CYLINDER", "128 \u{d7} 64 U", "3.90 \u{d7} 1.95 M"]),
    ("STAIRS", 48, 16, ["STAIRS", "48 \u{d7} 16 U", "1.46 \u{d7} 0.49 M"]),
];

#[test]
fn read_label_reads_every_row_of_a_table_of_sizes() {
    let atlas = font::font();
    for (shape, w, h, want) in FACES {
        let Some(plate) = label::layout(shape, w, h) else {
            panic!("{shape} {w}x{h} has no plate");
        };
        // From the raster.
        assert_eq!(label::read_label(&atlas, &plate), want, "{shape} {w}x{h}");
        // From the layout's cells.
        assert_eq!(plate.text(), want, "{shape} {w}x{h}");
    }
}

#[test]
fn every_glyph_reads_back_from_the_atlas() {
    let atlas = font::font();
    for index in 0..39u8 {
        let c = font::char_of(index).unwrap();
        assert_eq!(font::index_of(c), index);
        assert_eq!(font::read_cell(&atlas, index), Some(c), "cell {index}");
    }
    // A cell past the glyphs is the plain background and matches nothing
    // but the blank.
    assert_eq!(font::char_of(39), None);
    assert_eq!(font::index_of('~'), 0);
    assert_eq!(font::index_of('q'), font::index_of('Q'));
}

#[test]
fn glyphs_are_all_different() {
    for a in 0..font::GLYPHS.len() {
        for b in a + 1..font::GLYPHS.len() {
            assert_ne!(font::GLYPHS[a], font::GLYPHS[b], "{a} and {b}");
        }
    }
}

#[test]
fn a_plate_is_48_cells_whatever_the_face() {
    for (shape, w, h, _) in FACES {
        let plate = label::layout(shape, w, h).unwrap();
        assert_eq!(plate.glyphs.len() * plate.glyphs[0].len(), 48);
    }
}

#[test]
fn text_is_centred_in_its_row() {
    let plate = label::layout("RAMP", 256, 128).unwrap();
    // 4 letters in 16 cells: 6 blank, 4 glyphs, 6 blank.
    let row = plate.glyphs[0];
    assert!(row[..6].iter().all(|&g| g == 0));
    assert!(row[6..10].iter().all(|&g| g != 0));
    assert!(row[10..].iter().all(|&g| g == 0));
}

#[test]
fn the_largest_cell_that_fits_is_used() {
    // 16 cells of h plus a margin of h/2 each side: 17h across, 4h tall.
    let h_of = |w, h| label::layout("BLOCK", w, h).map(|p| p.h);
    assert_eq!(h_of(272, 64), Some(16));
    assert_eq!(h_of(271, 64), Some(8));
    assert_eq!(h_of(272, 63), Some(8));
    assert_eq!(h_of(136, 32), Some(8));
    assert_eq!(h_of(68, 16), Some(4));
    assert_eq!(h_of(34, 8), Some(2));
    assert_eq!(h_of(33, 8), None);
    assert_eq!(h_of(34, 7), None);
    assert_eq!(h_of(0, 0), None);
}

#[test]
fn metres_are_rounded_to_two_places() {
    // 3.048 centimetres a unit.
    let rows = label::rows("X", 1, 100);
    assert_eq!(rows[2], "0.03 \u{d7} 3.05 M");
    // Text longer than the plate's 16 cells leaves no plate.
    assert!(label::layout("BLOCK", 100_000, 100_000).is_none());
    // A long name is cut to the plate.
    let plate = label::layout("ABCDEFGHIJKLMNOPQRST", 512, 256).unwrap();
    assert_eq!(plate.text()[0], "ABCDEFGHIJKLMNOP");
}

#[test]
fn a_tiles_opposite_edges_are_equal() {
    let last = TILE_SIZE - 1;
    for family in ALL {
        let t = tile(family);
        assert_eq!((t.width, t.height), (512, 512));
        assert_eq!(t.rgba.len(), 512 * 512 * 4);
        for i in 0..TILE_SIZE {
            assert_eq!(t.pixel(0, i), t.pixel(last, i), "{family:?} column at {i}");
            assert_eq!(t.pixel(i, 0), t.pixel(i, last), "{family:?} row at {i}");
        }
    }
}

#[test]
fn lines_fall_on_16_64_and_256() {
    for family in ALL {
        let t = tile(family);
        let (light, dark) = family.tones();
        let [faint, strong, heavy] = line_colours(family);
        let rgb = |p: [u8; 4]| [p[0], p[1], p[2]];
        // Texels a unit: 2. Sample on rows through a square's middle,
        // 8 units (16 texels) up, where no horizontal line crosses.
        let y = 16;
        for unit in (0..256).step_by(16) {
            let x = unit * 2;
            let want = if unit % 256 == 0 {
                heavy
            } else if unit % 64 == 0 {
                strong
            } else {
                faint
            };
            assert_eq!(rgb(t.pixel(x, y)), want, "{family:?} line at unit {unit}");
        }
        // Between lines is the checker: 8 units either side of line 16.
        assert_eq!(rgb(t.pixel(32 + 16, y)), light);
        // The next square along is the other tone, 32 units on.
        assert_eq!(rgb(t.pixel(64 + 16, y)), dark);
        assert_eq!(rgb(t.pixel(16, y + 64)), dark);
        assert_eq!(rgb(t.pixel(80, y + 64)), light);
        // Lines are one unit wide, so the texel after one is not on it.
        assert_ne!(rgb(t.pixel(33, y)), faint);
        // The three line colours differ from each other and the tones.
        for (i, a) in [faint, strong, heavy].iter().enumerate() {
            assert_ne!(*a, light);
            assert_ne!(*a, dark);
            for b in [faint, strong, heavy].iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
        // The heavier the line, the darker.
        assert!(heavy[1] < strong[1] && strong[1] < faint[1] && faint[1] < dark[1]);
        // Every texel is opaque.
        assert!(t.rgba.chunks_exact(4).all(|p| p[3] == 255));
    }
}

#[test]
fn the_family_contrast_table() {
    for (i, a) in ALL.iter().enumerate() {
        let (light, dark) = a.tones();
        assert!(
            family::distance(light, dark) < MAX_TONE_SPREAD,
            "{a:?} tones too far apart"
        );
        assert_ne!(light, dark);
        for b in ALL.iter().skip(i + 1) {
            let d = family::distance(a.mid(), b.mid());
            assert!(d > MIN_FAMILY_CONTRAST, "{a:?} and {b:?} are {d} apart");
        }
    }
    assert_eq!(ALL.len(), 6);
    assert_eq!(Family::Wall.colour_name(), "orange");
}

fn golden(image: &Image, want: &str, what: &str) {
    assert_eq!(hash(image), want, "{what}: raster changed; bump VERSION");
}

#[test]
fn golden_sha256_of_each_image() {
    golden(&tile(Family::Floor), "f518e0335bb47339e7ac954b46aa0aab3426a7cdfa0d18699032c362a6216f63", "floor");
    golden(&tile(Family::Wall), "06c4ef9966f037e7f04922f0992dc3dfc06aa5dac4cccad6a9011b1f8bab4790", "wall");
    golden(&tile(Family::Ceiling), "573c5652de0ee9e8f2f8a72549cfeae137ec7888acf55b43eed46159ba13ad39", "ceiling");
    golden(&tile(Family::Trim), "6a3a0fbef9016682b4484fc089bd2ce2e43698559e10eb7d25af6adf96730f98", "trim");
    golden(&tile(Family::Nodraw), "54021f44acac776a9421b1c4140ffeef3eb67d5153dc898dccc55448f0dece29", "nodraw");
    golden(&tile(Family::Clip), "77b716dfcea0bce8698650169cae3d9e83eb4e1c5a727bd2cac59883d8869807", "clip");
    golden(&font::font(), "7be0f73fd3996f558d7ec9cd326de0a629ceaaa631d60bc3b7e0103e76da46dd", "font");
    assert_eq!(dev_texture::VERSION, 1);
}

#[test]
fn a_sweep_of_every_size_shows_a_plate_inside_its_face_or_none() {
    for w in (0..=600).step_by(3) {
        for h in (0..=300).step_by(3) {
            let Some(p) = label::layout("BLOCK", w, h) else {
                // None only when even the smallest cell does not fit.
                assert!(w < 34 || h < 8, "{w}x{h} has no plate");
                continue;
            };
            let margin = (p.h / 2) as f32;
            let (pw, ph) = (16.0 * p.h as f32, 3.0 * p.h as f32);
            assert!(p.left >= margin && p.left + pw <= w as f32 - margin, "{w}x{h} x");
            assert!(p.bottom >= margin && p.bottom + ph <= h as f32 - margin, "{w}x{h} y");
            // Centred.
            assert_eq!(p.left, w as f32 - p.left - pw);
            assert_eq!(p.bottom, h as f32 - p.bottom - ph);
            // The first cell's rectangle is the plate's upper-left corner,
            // the last's its lower-right.
            let first = p.cell_rect(0, 0);
            assert_eq!([first[0], first[3]], [p.left, p.bottom + ph]);
            let last = p.cell_rect(2, 15);
            assert_eq!([last[2], last[1]], [p.left + pw, p.bottom]);
            // No smaller than the largest that fits.
            assert!(label::SIZES.contains(&p.h));
        }
    }
}

#[test]
fn cell_uvs_are_the_cells_of_the_atlas() {
    assert_eq!(font::cell_uv(0), [0.0, 0.0, 0.125, 0.125]);
    assert_eq!(font::cell_uv(9), [0.125, 0.125, 0.25, 0.25]);
    assert_eq!(font::cell_uv(38), [0.75, 0.5, 0.875, 0.625]);
}
