//! Glyph pixel compressor: the inverse of [`crate::decode_pixels`].
//!
//! Used to put generated glyphs (button icons, `docs/input/button-icons.md`) into a
//! font the game reads. The stream uses only the byte kinds the engine's
//! decompressor (`0x122610`) understands; see `docs/formats/fonts.md` for the table.

/// The 3-bit code for a 4-bit alpha, if a run byte can carry it. The engine
/// expands `a` to `(a << 1) | (a & 1)`, so only 0, 3, 4, 7, 8, 11, 12 and 15
/// are reachable without a literal word.
pub fn alpha3_code(alpha4: u16) -> Option<u8> {
    let a = (alpha4 >> 1) as u8 & 7;
    (u16::from((a << 1) | (a & 1)) == alpha4).then_some(a)
}

/// Snap a 4-bit alpha to the nearest value a run byte can carry (ties go up).
pub fn quantize_alpha(alpha4: u16) -> u16 {
    const LEVELS: [u16; 8] = [0, 3, 4, 7, 8, 11, 12, 15];
    // Reversed so that, of two equally near levels, the higher wins.
    *LEVELS
        .iter()
        .rev()
        .min_by_key(|l| l.abs_diff(alpha4))
        .unwrap()
}

/// Compress ARGB4444 pixels (row-major) into a glyph stream.
///
/// Decoding the result gives the same alpha for every pixel and the same
/// colour for every pixel that is not fully transparent. A fully transparent
/// pixel comes back in whatever colour is current at that point, which is how
/// the retail glyphs store them too.
pub fn encode_pixels(pixels: &[u16]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut color: u16 = 0xfff;
    let mut i = 0;
    while i < pixels.len() {
        let p = pixels[i];
        let (a, c) = (p >> 12, p & 0xfff);
        if a == 0 {
            let n = run(pixels, i, |q| q >> 12 == 0);
            out.push(n as u8);
            i += n;
            continue;
        }
        let Some(code) = alpha3_code(a).filter(|_| c == color) else {
            out.push(0);
            out.extend_from_slice(&p.to_be_bytes());
            color = c;
            i += 1;
            continue;
        };
        if a == 15 {
            let n = run(pixels, i, |q| q == p);
            out.push(0x40 | n as u8);
            i += n;
            continue;
        }
        // One partly covered pixel, paired with the next when that one needs
        // no literal either.
        let next = pixels.get(i + 1).and_then(|&q| {
            let (qa, qc) = (q >> 12, q & 0xfff);
            alpha3_code(qa).filter(|_| qa == 0 || qc == color)
        });
        match next {
            Some(b) => {
                out.push(0xc0 | (code << 3) | b);
                i += 2;
            }
            None => {
                out.push(0x80 | (code << 3));
                i += 1;
            }
        }
    }
    out
}

/// Length of the run starting at `i` whose pixels satisfy `same`, at most 63.
fn run(pixels: &[u16], i: usize, same: impl Fn(u16) -> bool) -> usize {
    pixels[i..]
        .iter()
        .take(63)
        .take_while(|&&q| same(q))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_pixels;

    /// Decoded pixels must match in alpha everywhere and in colour wherever
    /// the pixel is visible.
    fn assert_round_trip(px: &[u16]) -> Vec<u8> {
        let stream = encode_pixels(px);
        let back = decode_pixels(&stream, px.len()).expect("stream decodes to the same count");
        for (i, (a, b)) in px.iter().zip(&back).enumerate() {
            assert_eq!(a >> 12, b >> 12, "alpha of pixel {i}");
            if a >> 12 != 0 {
                assert_eq!(a, b, "pixel {i}");
            }
        }
        stream
    }

    #[test]
    fn alpha_codes_are_the_eight_levels_the_engine_expands_to() {
        let reachable: Vec<u16> = (0..16).filter(|a| alpha3_code(*a).is_some()).collect();
        assert_eq!(reachable, [0, 3, 4, 7, 8, 11, 12, 15]);
        for a in reachable {
            let c = alpha3_code(a).unwrap();
            assert_eq!(u16::from((c << 1) | (c & 1)), a);
        }
    }

    #[test]
    fn quantize_snaps_to_the_nearest_level() {
        let got: Vec<u16> = (0..16).map(quantize_alpha).collect();
        assert_eq!(got, [0, 0, 3, 3, 4, 4, 7, 7, 8, 8, 11, 11, 12, 12, 15, 15]);
    }

    #[test]
    fn empty_input_is_an_empty_stream() {
        assert!(encode_pixels(&[]).is_empty());
    }

    #[test]
    fn runs_use_one_byte_and_split_at_63() {
        assert_eq!(assert_round_trip(&[0x0fff; 10]), [10]);
        assert_eq!(assert_round_trip(&[0xffff; 10]), [0x40 | 10]);
        assert_eq!(assert_round_trip(&[0x0fff; 64]), [63, 1]);
        assert_eq!(assert_round_trip(&[0xffff; 130]), [0x7f, 0x7f, 0x44]);
    }

    #[test]
    fn partial_white_pixels_pair_up_without_literals() {
        // 3/15 then 8/15 pair; 12/15 then transparent pair; a lone 7/15.
        assert_eq!(
            assert_round_trip(&[0x3fff, 0x8fff, 0xcfff, 0x0fff, 0x7fff]),
            [0xc0 | 1 << 3 | 4, 0xc0 | 6 << 3, 0x80 | 3 << 3]
        );
    }

    #[test]
    fn colour_changes_and_odd_alphas_become_literals() {
        // A red pixel sets the colour; the following red run needs no literal.
        assert_eq!(
            assert_round_trip(&[0xff00, 0xff00, 0xff00]),
            [0, 0xff, 0x00, 0x42]
        );
        // Alpha 5 is not a run level, so it is always a literal.
        assert_eq!(assert_round_trip(&[0x5fff]), [0, 0x5f, 0xff]);
        // A transparent pixel never costs a literal, whatever its colour.
        assert_eq!(assert_round_trip(&[0x0123, 0xffff]), [1, 0x41]);
    }

    #[test]
    fn every_pixel_value_pattern_round_trips() {
        // A deterministic mix of colours and all 16 alphas, with runs.
        let mut px = Vec::new();
        let mut s: u32 = 0x1234_5678;
        for i in 0..4000 {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let alpha = (s >> 28) as u16;
            let colour = if i % 7 < 4 {
                0xfff
            } else {
                (s >> 8) as u16 & 0xfff
            };
            let n = 1 + (s >> 4) as usize % 5;
            px.extend(std::iter::repeat_n(alpha << 12 | colour, n));
        }
        assert_round_trip(&px);
    }
}
