//! The Firefight HUD's skull pictures (docs/game/firefight.md, "The HUD"):
//! seven slots, two pictures each, off and on, at spare private-use code
//! points, so a gauge of the HUD kit (`hud_kit_core::model::Glyphs`) draws
//! one in the HUD's own text colour as it draws a letter. Each is the
//! skull's glyph, the strokes of [`crate::strokes`] (`game::skull_glyphs`),
//! rasterised to [`PICTURE`] pixels square: nothing is copied from a disc
//! bitmap or any game's skull art. Off is the faint track
//! ([`crate::ring::TRACK`]), on is lit.
//!
//! [`replacements`] gives the pictures a font lacks to
//! [`crate::patch::apply`], each with its top-left corner at the text's
//! point, as the charge ring's are.
use crate::Font;
use crate::patch::Replacement;
use crate::ring::{self, Picture};
use crate::strokes;

/// The slots, in the HUD's order from the right: Ghost, Catch, Black Eye,
/// Thunderstorm, Famine, Mythic, Iron.
pub const SLOTS: usize = 7;
/// The skull (an index into [`strokes::GLYPHS`], the game's session skull
/// index) each slot shows.
pub const SLOT_SKULLS: [usize; SLOTS] = [7, 9, 8, 3, 4, 12, 11];
/// The pictures' size, wide and tall: inside the full screen's 16 pixel
/// slots and the half screen's 14.
pub const PICTURE: u16 = 14;
/// U+E600: slot 0's picture when off. Slot `s`'s step `k` (0 off, 1 on) is
/// at `FIRST + 2 s + k`, to U+E60D. After the charge ring's U+E2C0..U+E2FE
/// ([`crate::ring`]); clear of the button icons (U+E100..) and the lobby's
/// marks (U+E4xx).
pub const FIRST: u16 = 0xe600;
/// How many pictures: 14.
pub const COUNT: u16 = SLOTS as u16 * 2;

/// The code unit of slot `slot`'s picture, off or on.
pub const fn unit(slot: usize, on: bool) -> u16 {
    FIRST + (slot % SLOTS) as u16 * 2 + on as u16
}

/// The slot and step a code unit draws, if it is one of these.
pub fn parts(unit: u16) -> Option<(usize, bool)> {
    let n = unit.checked_sub(FIRST).filter(|n| *n < COUNT)?;
    Some((usize::from(n / 2), n % 2 == 1))
}

/// Slot `slot`'s picture in white, [`PICTURE`] pixels square, ARGB4444:
/// the slot's skull's glyph, lit or the faint track.
pub fn draw(slot: usize, on: bool) -> Vec<u16> {
    let glyph = strokes::glyph(SLOT_SKULLS[slot % SLOTS]).unwrap_or(&[]);
    let level = if on { 1.0 } else { ring::TRACK };
    strokes::rasterise(glyph, u32::from(PICTURE))
        .into_iter()
        .map(|coverage| {
            let alpha = (f32::from(coverage) / 255.0 * 15.0 * level).round() as u16;
            (alpha.min(15) << 12) | 0x0fff
        })
        .collect()
}

/// The picture code unit `unit` draws ([`draw`]); `None` for a unit that
/// is not one of these.
pub fn picture(unit: u16) -> Option<Picture> {
    let (slot, on) = parts(unit)?;
    Some(Picture {
        width: PICTURE,
        height: PICTURE,
        pixels: draw(slot, on),
    })
}

/// The pictures `font` lacks, each with its top-left corner at the text's
/// point: no offset across, and as far above the baseline as the font's
/// ascent.
pub fn replacements(font: &Font<'_>) -> Vec<Replacement> {
    (0..SLOTS)
        .flat_map(|s| [false, true].map(move |on| (s, on)))
        .filter(|(s, on)| !ring::has_glyph(font, unit(*s, *on)))
        .map(|(s, on)| Replacement {
            unit: unit(s, on),
            width: PICTURE,
            height: PICTURE,
            advance: PICTURE as i16,
            x_offset: 0,
            y_offset: Some(font.header.ascent),
            pixels: draw(s, on),
        })
        .collect()
}

/// The font file `data` with the pictures it lacks added; `None` when it
/// is not a font, has them all, or the patch fails. For tools that read
/// the disc's fonts as the game's font rewrite serves them.
pub fn patch(data: &[u8]) -> Option<Vec<u8>> {
    let add = replacements(&Font::parse(data).ok()?);
    if add.is_empty() {
        return None;
    }
    crate::patch::apply(data, &add).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slot `s`'s pictures are at two units from [`FIRST`], and read back
    /// from their unit.
    #[test]
    fn the_units_are_two_a_slot() {
        assert_eq!(unit(0, false), 0xe600);
        assert_eq!(unit(6, true), 0xe60d);
        assert_eq!(FIRST + COUNT - 1, unit(SLOTS - 1, true));
        for s in 0..SLOTS {
            for on in [false, true] {
                assert_eq!(parts(unit(s, on)), Some((s, on)));
            }
        }
        assert_eq!(parts(FIRST - 1), None);
        assert_eq!(parts(FIRST + COUNT), None);
        // Clear of the ring's units.
        assert!(FIRST > ring::FIRST + ring::COUNT);
    }

    /// Off is faint, on is lit, both the slot's skull's strokes; the
    /// pictures are the size they claim and do not touch their edge.
    #[test]
    fn a_picture_is_its_skulls_strokes_off_or_on() {
        let n = usize::from(PICTURE);
        for (slot, skull) in SLOT_SKULLS.into_iter().enumerate() {
            let mask = strokes::rasterise(strokes::glyph(skull).unwrap(), u32::from(PICTURE));
            let (off, on) = (draw(slot, false), draw(slot, true));
            assert_eq!((off.len(), on.len()), (n * n, n * n));
            for (i, coverage) in mask.iter().enumerate() {
                assert_eq!(off[i] & 0x0fff, 0x0fff);
                if *coverage == 255 {
                    assert_eq!(on[i] >> 12, 15, "slot {slot} texel {i} lit");
                    assert_eq!(off[i] >> 12, 4, "slot {slot} texel {i} faint");
                }
                if *coverage == 0 {
                    assert_eq!((on[i] >> 12, off[i] >> 12), (0, 0), "slot {slot} texel {i}");
                }
            }
            assert!(on.iter().any(|p| p >> 12 == 15), "slot {slot} has ink");
            let edge = (0..n).flat_map(|k| [k, (n - 1) * n + k, k * n, k * n + n - 1]);
            for t in edge {
                assert_eq!(on[t] >> 12, 0, "slot {slot} inks its edge");
            }
            let p = picture(unit(slot, true)).unwrap();
            assert_eq!((p.width, p.height, p.pixels), (PICTURE, PICTURE, on));
        }
        // No two slots look alike.
        for a in 0..SLOTS {
            for b in a + 1..SLOTS {
                assert_ne!(draw(a, true), draw(b, true), "slots {a} and {b}");
            }
        }
        assert!(picture(unit(0, true) + 20).is_none());
    }
}
