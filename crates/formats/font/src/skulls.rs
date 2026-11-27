//! The Firefight HUD's skull pictures (docs/game/firefight.md, "The HUD"):
//! seven slots, two pictures each, off and on, at spare private-use code
//! points, so a gauge of the HUD kit (`hud_kit_core::model::Glyphs`) draws
//! one in the HUD's own text colour as it draws a letter. Each is the
//! skull's glyph, the strokes of [`crate::strokes`] (`game::skull_glyphs`),
//! rasterised to [`PICTURE`] pixels square: nothing is copied from a disc
//! bitmap or any game's skull art. Off is the faint track
//! ([`crate::ring::TRACK`]), on is lit.
//!
//! The same HUD's progress pips are here too ([`Pip`]): a round's circle
//! and a wave's slanted bar, off and on, after the skulls' units. They
//! are shapes of ours in the HUD's text colour, the count and order
//! Halo 3: ODST's Firefight shows (three circles, five bars, filling from
//! the left), with none of its art.
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

/// A progress pip of the Firefight HUD: one round of the set, or one wave
/// of the round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pip {
    /// A round: a filled circle.
    Round = 0,
    /// A wave: a bar slanted to the right.
    Wave = 1,
}

/// The pips' height, and the square their strokes are rasterised in.
pub const PIP: u16 = 10;
/// U+E60E: the round pip when off. `FIRST_PIP + 2 pip + on`, to U+E611,
/// after the skulls' pictures.
pub const FIRST_PIP: u16 = FIRST + COUNT;
/// How many pip pictures: 4.
pub const PIPS: u16 = 4;

const ROUND_PIP: &[strokes::Stroke] = &[strokes::Stroke::Dot {
    centre: [0.0, 0.0],
    radius: 0.6,
}];
const fn pip_dot(x: f32, y: f32) -> strokes::Stroke {
    strokes::Stroke::Dot {
        centre: [x, y],
        radius: 0.25,
    }
}
/// Five discs in a row are one solid bar two and a half pixels thick with
/// round ends, leaning 0.4 of the box to the right. (A line's distance
/// never falls far enough below zero to light a texel fully at this size.)
const WAVE_PIP: &[strokes::Stroke] = &[
    pip_dot(-0.2, -0.5),
    pip_dot(-0.1, -0.25),
    pip_dot(0.0, 0.0),
    pip_dot(0.1, 0.25),
    pip_dot(0.2, 0.5),
];

impl Pip {
    pub const ALL: [Pip; 2] = [Pip::Round, Pip::Wave];

    /// The pip's strokes, in the glyph box ([`crate::strokes`]).
    pub const fn strokes(self) -> &'static [strokes::Stroke] {
        match self {
            Pip::Round => ROUND_PIP,
            Pip::Wave => WAVE_PIP,
        }
    }

    /// The picture's width: the middle columns of the [`PIP`] pixel
    /// square, which hold all of the pip's ink.
    pub const fn width(self) -> u16 {
        match self {
            Pip::Round => 8,
            Pip::Wave => 6,
        }
    }

    /// The pip's picture's code unit, off or on.
    pub const fn unit(self, on: bool) -> u16 {
        FIRST_PIP + self as u16 * 2 + on as u16
    }

    /// The pip and step a code unit draws, if it is a pip's.
    pub fn parts(unit: u16) -> Option<(Pip, bool)> {
        let n = unit.checked_sub(FIRST_PIP).filter(|n| *n < PIPS)?;
        Some((Pip::ALL[usize::from(n / 2)], n % 2 == 1))
    }

    /// The pip's picture in white, [`Pip::width`] by [`PIP`] pixels,
    /// ARGB4444: lit, or the faint track ([`ring::TRACK`]).
    pub fn draw(self, on: bool) -> Vec<u16> {
        let (n, w) = (usize::from(PIP), usize::from(self.width()));
        let skip = (n - w) / 2;
        let level = if on { 1.0 } else { ring::TRACK };
        let mask = strokes::rasterise(self.strokes(), u32::from(PIP));
        mask.chunks(n)
            .flat_map(|row| row[skip..skip + w].iter())
            .map(|coverage| {
                let alpha = (f32::from(*coverage) / 255.0 * 15.0 * level).round() as u16;
                (alpha.min(15) << 12) | 0x0fff
            })
            .collect()
    }
}

/// The picture code unit `unit` draws ([`draw`], [`Pip::draw`]); `None`
/// for a unit that is not one of these.
pub fn picture(unit: u16) -> Option<Picture> {
    if let Some((pip, on)) = Pip::parts(unit) {
        return Some(Picture {
            width: pip.width(),
            height: PIP,
            pixels: pip.draw(on),
        });
    }
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
    let skulls = (0..SLOTS)
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
        });
    let pips = Pip::ALL
        .into_iter()
        .flat_map(|p| [false, true].map(move |on| (p, on)))
        .filter(|(p, on)| !ring::has_glyph(font, p.unit(*on)))
        .map(|(p, on)| Replacement {
            unit: p.unit(on),
            width: p.width(),
            height: PIP,
            advance: p.width() as i16,
            x_offset: 0,
            y_offset: Some(font.header.ascent),
            pixels: p.draw(on),
        });
    skulls.chain(pips).collect()
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

    /// The pips follow the skulls' units and read back; a picture is its
    /// width by [`PIP`], lit when on and faint when off, with ink in the
    /// middle and none in its top and bottom rows; the two shapes differ.
    #[test]
    fn a_pip_is_a_circle_or_a_slanted_bar_off_or_on() {
        assert_eq!(Pip::Round.unit(false), 0xe60e);
        assert_eq!(Pip::Wave.unit(true), 0xe611);
        assert_eq!(FIRST_PIP + PIPS - 1, Pip::Wave.unit(true));
        assert_eq!(Pip::parts(FIRST_PIP - 1), None);
        assert_eq!(Pip::parts(FIRST_PIP + PIPS), None);
        assert_eq!(parts(FIRST_PIP), None, "a pip is no skull");
        for pip in Pip::ALL {
            let (w, h) = (usize::from(pip.width()), usize::from(PIP));
            let (off, on) = (pip.draw(false), pip.draw(true));
            assert_eq!((off.len(), on.len()), (w * h, w * h));
            for (a, b) in off.iter().zip(&on) {
                assert_eq!((a & 0x0fff, b & 0x0fff), (0x0fff, 0x0fff));
                assert!(a >> 12 <= b >> 12, "off is never brighter");
                if b >> 12 == 15 {
                    assert_eq!(a >> 12, 4, "faint where on is lit");
                }
            }
            // Lit at the centre, clear on the top and bottom rows' ends.
            assert_eq!(on[(h / 2) * w + w / 2] >> 12, 15, "{pip:?}");
            assert!(on.iter().filter(|p| *p >> 12 == 15).count() >= 8, "{pip:?}");
            assert_eq!(on[0] >> 12, 0, "{pip:?}");
            assert_eq!(on[w * h - 1] >> 12, 0, "{pip:?}");
            for on_now in [false, true] {
                assert_eq!(Pip::parts(pip.unit(on_now)), Some((pip, on_now)));
                let p = picture(pip.unit(on_now)).unwrap();
                assert_eq!((p.width, p.height), (pip.width(), PIP));
                assert_eq!(p.pixels, pip.draw(on_now));
            }
        }
        // The wave's bar leans right: its top row's ink is right of its
        // bottom row's.
        let bar = Pip::Wave.draw(true);
        let (w, h) = (usize::from(Pip::Wave.width()), usize::from(PIP));
        let middle = |row: usize| {
            let lit: Vec<usize> = (0..w).filter(|c| bar[row * w + c] >> 12 >= 8).collect();
            assert!(!lit.is_empty(), "row {row}");
            lit.iter().sum::<usize>() as f32 / lit.len() as f32
        };
        assert!(middle(2) > middle(h - 3), "{} {}", middle(2), middle(h - 3));
    }
}
