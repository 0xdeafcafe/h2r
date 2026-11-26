//! Each skull's glyph (docs/game/skulls.md, "The model picture"): our own
//! line art, one per skull in [`skulls::ABOUT`](crate::skulls::ABOUT)'s
//! order. The strokes and their sampling live in `font::strokes`, a crate
//! with no game in it, so the HUD kit's offline tools draw the Firefight
//! HUD's skull gauges from the same strokes (`font::skulls`); this module
//! is the game's name for them and holds what needs the skull list.

pub use font::strokes::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skulls::ABOUT;

    /// Iron's name in [`ABOUT`], the skull of the HUD's seventh slot.
    const ABOUT_NAME_OF_IRON: &str = "Iron";

    /// The masks' size in these tests.
    const SIZE: u32 = 64;

    #[test]
    fn the_distance_field_is_negative_inside_a_stroke_and_measures_out_from_it() {
        let line = [l(-0.5, 0.0, 0.5, 0.0)];
        assert!((signed_distance(&line, [0.0, 0.0]) + HALF_WIDTH).abs() < 1e-6);
        assert!((signed_distance(&line, [0.0, 0.3]) - (0.3 - HALF_WIDTH)).abs() < 1e-6);
        // Past the end the cap is round.
        assert!((signed_distance(&line, [0.8, 0.0]) - (0.3 - HALF_WIDTH)).abs() < 1e-6);
        // The nearest stroke wins.
        let two = [l(-0.5, 0.0, 0.5, 0.0), dot(0.0, 0.6, 0.1)];
        assert!((signed_distance(&two, [0.0, 0.45]) - 0.05).abs() < 1e-6);
        // The field holds the same distances at its texels' centres, row
        // 0 at the top.
        let field = distance_field(&two, 8);
        assert_eq!(field.len(), 64);
        assert_eq!(field[8 + 4], signed_distance(&two, [0.125, 0.625]));
        assert!(field[8 + 4] < field[6 * 8 + 4], "the dot is at the top");
        for (i, strokes) in GLYPHS.iter().enumerate() {
            let field = distance_field(strokes, FIELD);
            assert_eq!(field, distance_field(strokes, FIELD), "deterministic");
            let inside = field.iter().filter(|d| **d < 0.0).count();
            assert!(inside > 600, "{} has almost no ink", ABOUT[i].name);
            // The outer texels are clear of every stroke by the box's
            // margin, less half a texel.
            let n = FIELD as usize;
            let margin = 1.0 - BOX - 1.0 / FIELD as f32;
            for k in 0..n {
                for t in [k, (n - 1) * n + k, k * n, k * n + n - 1] {
                    assert!(field[t] >= margin - 1e-4, "{}: {}", ABOUT[i].name, field[t]);
                }
            }
        }
    }

    #[test]
    fn every_skull_has_a_glyph_and_no_two_are_alike() {
        assert_eq!(GLYPHS.len(), ABOUT.len());
        let masks: Vec<Vec<u8>> = (0..ABOUT.len())
            .map(|i| rasterise(glyph(i).expect("a glyph"), SIZE))
            .collect();
        for (i, m) in masks.iter().enumerate() {
            let inked = m.iter().filter(|c| **c > 127).count();
            assert!(inked > 60, "{} has almost no ink ({inked})", ABOUT[i].name);
            assert!(
                inked < m.len() / 2,
                "{} is mostly ink ({inked})",
                ABOUT[i].name
            );
            for (j, other) in masks.iter().enumerate().skip(i + 1) {
                let differ = m
                    .iter()
                    .zip(other)
                    .filter(|(a, b)| a.abs_diff(**b) > 127)
                    .count();
                assert!(
                    differ > 40,
                    "{} and {} look alike ({differ})",
                    ABOUT[i].name,
                    ABOUT[j].name
                );
            }
        }
        assert!(glyph(ABOUT.len()).is_none());
    }

    #[test]
    fn rasterising_is_deterministic() {
        for strokes in GLYPHS {
            assert_eq!(rasterise(strokes, SIZE), rasterise(strokes, SIZE));
            assert_eq!(rasterise(strokes, 32), rasterise(strokes, 32));
        }
    }

    #[test]
    fn every_glyph_stays_inside_its_box() {
        for (i, strokes) in GLYPHS.iter().enumerate() {
            for s in *strokes {
                assert!(
                    s.reach() <= BOX + 1e-4,
                    "{}: {s:?} reaches {}",
                    ABOUT[i].name,
                    s.reach()
                );
            }
            for size in [32, SIZE, 128] {
                let m = rasterise(strokes, size);
                let border = (0..size).flat_map(|k| [(0, k), (size - 1, k), (k, 0), (k, size - 1)]);
                for (row, col) in border {
                    assert_eq!(
                        m[(row * size + col) as usize],
                        0,
                        "{} inks its edge at {size}",
                        ABOUT[i].name
                    );
                }
            }
        }
    }

    #[test]
    fn strokes_draw_where_they_are() {
        let m = rasterise(&[l(-0.5, 0.0, 0.5, 0.0)], 20);
        // Rows 9 and 10 straddle y = 0 (their centres 0.05 off it); the
        // line is 0.14 wide (1.4 texels), so both are mostly inked.
        assert!(m[9 * 20 + 10] > 127 && m[10 * 20 + 10] > 127);
        assert_eq!(m[2 * 20 + 10], 0);
        assert_eq!(m[9 * 20 + 1], 0, "past the line's end");
        // A half arc on the left: ink at its left, none at its right.
        let a = rasterise(&[arc(0.0, 0.0, 0.6, 90.0, 270.0)], 20);
        assert!(a[9 * 20 + 4] > 127 && a[9 * 20 + 15] == 0);
        // A dot is filled.
        let d = rasterise(&[dot(0.0, 0.0, 0.3)], 20);
        assert_eq!(d[9 * 20 + 9], 255);
    }

    /// The strokes this module names are the font crate's, and the list
    /// they are indexed by is the skull list.
    #[test]
    fn the_strokes_index_the_skull_list() {
        assert_eq!(font::strokes::TOTAL, crate::skulls::TOTAL);
        assert_eq!(GLYPHS.len(), crate::skulls::TOTAL);
    }

    /// The Firefight HUD's skull pictures are the skulls' own strokes
    /// rasterised (`font::skulls`): each slot shows the skull the layout's
    /// slot names (Iron, which has no session skull of ours yet, by its
    /// name in the skull list), off faint and on lit, at the picture size
    /// the kit's gauge claims, and the served font gets them.
    #[test]
    fn skull_pictures_come_from_our_strokes() {
        use crate::firefight::hud::layout::SKULL_SLOTS;
        use hud_kit_core::views::firefight::{SKULL_PICTURE, skull_glyphs};
        assert_eq!(font::skulls::SLOTS, SKULL_SLOTS.len());
        for (slot, skull) in SKULL_SLOTS.iter().enumerate() {
            let index = font::skulls::SLOT_SKULLS[slot];
            match skull {
                Some(s) => assert_eq!(*s as usize, index, "slot {slot}"),
                None => assert_eq!(ABOUT_NAME_OF_IRON, ABOUT[index].name, "slot {slot} is Iron"),
            }
            let strokes = glyph(index).expect("a glyph");
            let mask = rasterise(strokes, SKULL_PICTURE as u32);
            let g = skull_glyphs(slot);
            let on = g.picture(1.0).expect("the lit picture");
            let off = g.picture(0.0).expect("the off picture");
            assert_eq!((on.width, on.height), (SKULL_PICTURE as u16, SKULL_PICTURE as u16));
            assert_eq!((g.size.0, g.size.1), (SKULL_PICTURE, SKULL_PICTURE));
            for (i, c) in mask.iter().enumerate() {
                let lit = (f32::from(*c) / 255.0 * 15.0).round() as u16;
                assert_eq!(on.pixels[i] >> 12, lit, "slot {slot} texel {i}");
                assert!(off.pixels[i] >> 12 <= lit.min(4), "slot {slot} texel {i}");
            }
        }
        // A font without them is served them, each once, in every file.
        let blank = {
            let mut data = vec![0; font::RECORDS_OFFSET + font::RECORD_LEN];
            let put = |data: &mut Vec<u8>, at: usize, n: u32| {
                data[at..at + 4].copy_from_slice(&n.to_le_bytes())
            };
            put(&mut data, font::HEADER_OFFSET, font::MAGIC);
            put(&mut data, font::HEADER_OFFSET + 4, 18);
            put(&mut data, font::HEADER_OFFSET + 0xc, 1);
            data
        };
        let served = crate::bot_mark::GearRewrite::patch("maps\\fonts\\blank", &blank).unwrap();
        let f = font::Font::parse(&served).unwrap();
        for slot in 0..font::skulls::SLOTS {
            for on in [false, true] {
                let unit = font::skulls::unit(slot, on);
                assert!(font::ring::has_glyph(&f, unit), "U+{unit:04X}");
                let g = f.glyph(unit).unwrap();
                assert_eq!((g.record.width, g.record.height), (14, 14), "U+{unit:04X}");
            }
        }
    }
}
