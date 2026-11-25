//! The bot mark's glyph (docs/game/bots.md, "Clanker appearance and Live player options"): a gear at U+2699 in
//! every font file the game opens, so a bot's name (`⚙ ` and its tag, as
//! `bots::phantom::session` writes it) draws the mark and not a box.
//!
//! Every file under `maps\fonts` gets it, as the icon sets do
//! (`crate::icons`): so no font slot, whichever UI element it serves, is
//! left without it, and the font-slot trace (keyboard-charset.md, "Which
//! font draws which field") is not needed for it. The glyph is drawn in
//! code (`font::gear`); nothing comes from the disc. The same rewrite
//! adds the Mods list's state marks (`font::mod_marks`: U+2605 new,
//! U+2713 installed, U+2193 downloading), drawn in code the same way, and
//! to the two HUD message fonts the superbounce medals (`font::medals`,
//! U+E2A0..U+E2A8; docs/game/hud-kit.md, "The medals"), and to every
//! font the HUD's eight direction arrows (`font::arrows`,
//! U+E2B0..U+E2B7; docs/game/hud-kit.md, "Direction arrows") and the
//! charge ring's arcs (`font::ring`, U+E2C0..U+E2FE; "The charge
//! segments").

/// Adds the gear to every font file.
pub struct GearRewrite;

impl GearRewrite {
    /// The font file `name` (a bare name or its path) with the gear and
    /// the Mods list's marks (`font::mod_marks`: new, installed,
    /// downloading), the HUD's direction arrows (`font::arrows`) and the
    /// charge ring's arcs (`font::ring`) added,
    /// and the superbounce medals for a message font
    /// (`font::medals::wants_font`), or `None` if it is not a font or draws
    /// them all already.
    pub fn patch(name: &str, original: &[u8]) -> Option<Vec<u8>> {
        let font = font::Font::parse(original).ok()?;
        let mut add: Vec<font::patch::Replacement> =
            font::gear::replacement(&font).into_iter().collect();
        add.extend(font::mod_marks::replacements(&font));
        add.extend(font::arrows::replacements(&font));
        add.extend(font::ring::replacements(&font));
        if font::medals::wants_font(name) {
            add.extend(font::medals::replacements(&font));
        }
        if add.is_empty() {
            return None;
        }
        let out = font::patch::apply(original, &add).ok()?;
        font::Font::parse(&out).ok()?.validate().ok()?;
        Some(out)
    }
}

impl vfs::Rewrite for GearRewrite {
    fn wants(&self, path: &str) -> bool {
        let p = path.to_ascii_lowercase();
        p.starts_with("maps\\fonts\\") && !p.ends_with(".txt")
    }
    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        let out = Self::patch(path, original);
        if out.is_none() {
            crate::log::note(&format!(
                "bot mark: {path} left as it is (a bot's mark draws as a box there)"
            ));
        }
        out
    }
    fn skipped(&mut self, path: &str) {
        crate::log::note(&format!(
            "bot mark: {path} exceeds the rewrite limit; a bot's mark draws as a box there"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs::Rewrite;

    /// A font of one blank record every unit maps to.
    fn blank_font() -> Vec<u8> {
        let mut data = vec![0; font::RECORDS_OFFSET + font::RECORD_LEN];
        let put = |data: &mut Vec<u8>, at: usize, n: u32| {
            data[at..at + 4].copy_from_slice(&n.to_le_bytes())
        };
        put(&mut data, font::HEADER_OFFSET, font::MAGIC);
        put(&mut data, font::HEADER_OFFSET + 4, 18);
        put(&mut data, font::HEADER_OFFSET + 0xc, 1);
        data
    }

    #[test]
    fn every_font_gets_the_gear_once() {
        let mut r = GearRewrite;
        assert!(r.wants("maps\\fonts\\conduit-16"));
        assert!(r.wants("MAPS\\FONTS\\Arial-13"));
        assert!(!r.wants("maps\\fonts\\font_table.txt"));
        assert!(!r.wants("maps\\mainmenu.map"));
        let out = r.rewrite("maps\\fonts\\blank", &blank_font()).unwrap();
        let f = font::Font::parse(&out).unwrap();
        let g = f.glyph(font::gear::GEAR).unwrap();
        // 18 ascent and no 'H': seven tenths, 12 pixels.
        assert_eq!((g.record.width, g.record.height), (12, 12));
        assert!(g.pixels.iter().any(|p| p >> 12 == 0xf));
        for unit in font::mod_marks::MARKS {
            let m = f.glyph(unit).unwrap();
            assert_eq!((m.record.width, m.record.height), (12, 12), "U+{unit:04X}");
            assert!(font::mod_marks::has_glyph(&f, unit));
        }
        for step in 0..font::arrows::COUNT {
            let unit = font::arrows::unit(step);
            let a = f.glyph(unit).unwrap();
            assert_eq!((a.record.width, a.record.height), (12, 12), "U+{unit:04X}");
            assert!(font::arrows::has_glyph(&f, unit));
        }
        for segment in 0..font::ring::SEGMENTS {
            for step in 0..=font::ring::STEPS {
                let unit = font::ring::unit(segment, step);
                let a = f.glyph(unit).unwrap();
                assert_eq!(
                    (a.record.width, a.record.height),
                    font::ring::size(segment),
                    "U+{unit:04X}"
                );
                assert!(font::ring::has_glyph(&f, unit));
            }
        }
        // A font that has it is left alone.
        assert_eq!(r.rewrite("maps\\fonts\\again", &out), None);
        assert_eq!(r.rewrite("maps\\fonts\\junk", b"not a font"), None);
    }

    /// The message fonts get the superbounce medals, placed as their own
    /// Double Kill medal; other fonts do not.
    #[test]
    fn the_message_fonts_get_the_medals() {
        let retail = font::patch::Replacement {
            unit: font::medals::RETAIL_MEDAL,
            width: 24,
            height: 24,
            advance: 25,
            x_offset: 0,
            y_offset: Some(18),
            pixels: vec![0xfec7; 24 * 24],
        };
        let with_medal = font::patch::apply(&blank_font(), &[retail]).unwrap();
        let mut r = GearRewrite;
        let out = r.rewrite("maps\\fonts\\conduit-13", &with_medal).unwrap();
        let f = font::Font::parse(&out).unwrap();
        for (unit, _) in font::medals::MEDALS {
            assert!(font::medals::has_glyph(&f, unit), "U+{unit:04X}");
            let g = f.glyph(unit).unwrap();
            assert_eq!((g.record.width, g.record.advance), (24, 25));
        }
        let other = r.rewrite("maps\\fonts\\conduit-16", &with_medal).unwrap();
        let f = font::Font::parse(&other).unwrap();
        for (unit, _) in font::medals::MEDALS {
            assert!(!font::medals::has_glyph(&f, unit), "U+{unit:04X}");
        }
    }

    #[test]
    fn the_mark_is_the_gear() {
        assert_eq!(
            bots::phantom::session::MARK as u32,
            u32::from(font::gear::GEAR)
        );
        assert_eq!(bots::phantom::session::MARK, live_proto::bots::BOT_MARK);
    }
}
