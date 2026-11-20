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
//! U+2713 installed, U+2193 downloading), drawn in code the same way.

/// Adds the gear to every font file.
pub struct GearRewrite;

impl GearRewrite {
    /// The font with the gear and the Mods list's marks
    /// (`font::mod_marks`: new, installed, downloading) added, or `None` if
    /// it is not a font or draws them all already.
    pub fn patch(original: &[u8]) -> Option<Vec<u8>> {
        let font = font::Font::parse(original).ok()?;
        let mut add: Vec<font::patch::Replacement> =
            font::gear::replacement(&font).into_iter().collect();
        add.extend(font::mod_marks::replacements(&font));
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
        let out = Self::patch(original);
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
        // A font that has it is left alone.
        assert_eq!(r.rewrite("maps\\fonts\\again", &out), None);
        assert_eq!(r.rewrite("maps\\fonts\\junk", b"not a font"), None);
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
