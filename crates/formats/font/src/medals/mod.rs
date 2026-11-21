//! The superbounce medal glyphs (docs/game/hud-kit.md, "The medals"):
//! four medals of our own, added to the HUD message fonts at spare
//! private-use code points so a notification line can start with one, as
//! a retail multi-kill line starts with its medal.
//!
//! The pictures are [`art`]: 24 by 24 ARGB 4444 rows drawn for this
//! project by `scripts/superbounce-medals.py` in the manner of the retail
//! multi-kill discs. Nothing is copied from a disc font. [`replacements`]
//! gives them to [`crate::patch::apply`] for the fonts in [`FONTS`], placed
//! as the font places its own Double Kill medal (U+E038), so a line of ours
//! sits where a retail one does.

pub mod art;

use crate::Font;
use crate::patch::Replacement;

/// U+E2A0: Superbounce, a bounce.
pub const SUPERBOUNCE: u16 = 0xe2a0;
/// U+E2A1: Double Bounce, the second bounce of a chain.
pub const DOUBLE_BOUNCE: u16 = 0xe2a1;
/// U+E2A2: Triple Bounce, the third.
pub const TRIPLE_BOUNCE: u16 = 0xe2a2;
/// U+E2A3: Chain Bounce, the fifth.
pub const CHAIN_BOUNCE: u16 = 0xe2a3;

/// Every medal and its picture, in tier order.
pub const MEDALS: [(u16, &[u16; 576]); 4] = [
    (SUPERBOUNCE, &art::SUPERBOUNCE),
    (DOUBLE_BOUNCE, &art::DOUBLE_BOUNCE),
    (TRIPLE_BOUNCE, &art::TRIPLE_BOUNCE),
    (CHAIN_BOUNCE, &art::CHAIN_BOUNCE),
];

/// The fonts that get the medals: the two HUD message fonts, the ones the
/// notification list draws in (docs/game/hud-kit.md, "Fonts").
pub const FONTS: [&str; 2] = ["conduit-13", "conduit-12"];

/// The retail medal whose place the new ones take: the Double Kill disc.
pub const RETAIL_MEDAL: u16 = 0xe038;

/// Whether the font file named `name` (a bare name, or a path whose last
/// part is the name, in either slash) gets the medals.
pub fn wants_font(name: &str) -> bool {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    FONTS.iter().any(|f| f.eq_ignore_ascii_case(base))
}

/// The medals `font` lacks, each with the retail medal's offsets and
/// advance. Empty when the font has no retail medal of the pictures' size
/// (the pictures are not scaled).
pub fn replacements(font: &Font<'_>) -> Vec<Replacement> {
    let Some(retail) = retail_record(font) else {
        return Vec::new();
    };
    MEDALS
        .into_iter()
        .filter(|(unit, _)| !has_glyph(font, *unit))
        .map(|(unit, pixels)| Replacement {
            unit,
            width: art::SIZE,
            height: art::SIZE,
            advance: retail.advance,
            x_offset: retail.x_offset,
            y_offset: Some(retail.y_offset),
            pixels: pixels.to_vec(),
        })
        .collect()
}

/// The font's own Double Kill medal's record, when it has one as big as
/// the pictures.
fn retail_record(font: &Font<'_>) -> Option<crate::GlyphRecord> {
    let i = font.glyph_index(RETAIL_MEDAL);
    if i == 0 || i == font.glyph_index(0xffff) {
        return None;
    }
    font.record(i)
        .ok()
        .filter(|r| r.width == art::SIZE && r.height == art::SIZE)
}

/// Whether `font` has a picture of its own at `unit`: a record that is not
/// record 0, nor the filler U+FFFF shares, with a bitmap.
pub fn has_glyph(font: &Font<'_>, unit: u16) -> bool {
    let i = font.glyph_index(unit);
    i != 0
        && i != font.glyph_index(0xffff)
        && font.record(i).is_ok_and(|r| r.width > 0 && r.height > 0)
}

#[cfg(test)]
mod tests;
