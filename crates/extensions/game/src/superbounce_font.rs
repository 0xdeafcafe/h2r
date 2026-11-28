//! The disc's HUD font and colour for the superbounce world labels
//! (docs/game/superbounce.md, "Charge, crouch spots and landings").
//!
//! The font is the one the HUD draws messages and prompts in: engine font
//! 6, line 6 of `font_table.txt` counted from 0 (docs/game/hud-kit.md,
//! "Fonts"). [`Capture`] is a VFS rewrite that changes nothing: it keeps a
//! copy of the table and of that font as the game reads them, after the
//! icon and bot-mark rewrites, so the copy is the font the HUD has. The
//! first [`look`] after that packs the font's printable ASCII and button
//! glyphs (U+E100 to U+E111, docs/input/button-icons.md) into an atlas and
//! publishes it ([`gfx::world_lines::set_hud_font`]) for the window to
//! draw [`LabelFace::Hud`] labels with.
//!
//! The colour is the user's HUD text colour, the first of the two RGB
//! triples `0x0013927e` returns (docs/game/hud-kit.md, "Colours"), read
//! from the guest.
//!
//! No quiet fallback: without the font the window draws the labels in its
//! 8x8 debug font, the label text writes the button's letter instead of
//! its glyph, and the log says why, once. Without the colour the labels
//! are drawn white, and the log says why, once.

use std::sync::{Arc, Mutex, MutexGuard};

use guest_layout::GuestMemory;
use gfx::world_lines::{GlyphPixels, LabelFace, LabelFont};

/// Where the font table is.
pub const TABLE: &str = "maps\\fonts\\font_table.txt";
/// The directory of the font files.
pub const FONTS: &str = "maps\\fonts\\";
/// The table line (from 0) of the HUD's message and prompt font.
pub const HUD_FONT_LINE: usize = 6;
/// The user's HUD text colour: three floats, red first (the human triple
/// `0x0013927e` returns; the Elite's is `0x00468c94`).
pub const HUD_COLOUR_ADDR: u32 = 0x0046_8c84;
/// The labels' colour when the HUD colour cannot be read.
pub const UNREAD_COLOUR: [f32; 3] = [1.0, 1.0, 1.0];
/// The X button's glyph (docs/input/button-icons.md).
pub const X_GLYPH: char = '\u{e102}';
/// The A button's glyph (docs/input/button-icons.md): jump.
pub const A_GLYPH: char = '\u{e100}';
/// The button glyphs packed with the letters: A to the right stick click.
pub const BUTTONS: std::ops::RangeInclusive<u16> = 0xe100..=0xe111;

/// How the world labels look this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The HUD text colour, 0..1.
    pub colour: [f32; 3],
    /// Always [`LabelFace::Hud`]; the window draws the 8x8 font while no
    /// HUD font is loaded.
    pub face: LabelFace,
    /// The HUD font is loaded and has the X button's glyph.
    pub glyphs: bool,
}

impl Look {
    /// The X button as label text: its glyph, or the letter without one.
    pub fn x_button(&self) -> String {
        if self.glyphs {
            X_GLYPH.to_string()
        } else {
            "X".into()
        }
    }

    /// The A button as label text: its glyph, or the letter without one.
    pub fn a_button(&self) -> String {
        if self.glyphs {
            A_GLYPH.to_string()
        } else {
            "A".into()
        }
    }
}

/// The table's line `line` (from 0): the font file's name. CRLF lines.
pub fn table_line(table: &[u8], line: usize) -> Option<String> {
    let text = std::str::from_utf8(table).ok()?;
    let name = text.lines().nth(line)?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// What has been seen of the fonts so far.
#[derive(Default)]
struct Captured {
    /// The HUD font's file name, once the table is read.
    name: Option<String>,
    /// Font files read before the table, by name (only then).
    early: Vec<(String, Arc<[u8]>)>,
    /// The HUD font's bytes.
    font: Option<(String, Arc<[u8]>)>,
    /// The atlas built from them, or why there is none.
    built: Option<Result<Arc<LabelFont>, String>>,
    /// The fallback has been noted.
    noted_font: bool,
    noted_colour: bool,
}

static CAPTURED: Mutex<Option<Captured>> = Mutex::new(None);

fn captured() -> MutexGuard<'static, Option<Captured>> {
    CAPTURED.lock().unwrap_or_else(|e| e.into_inner())
}

fn file_name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}

/// Keeps the font table and the HUD font as the game reads them. Serves
/// every file unchanged; chain it after the rewrites that patch fonts, or
/// give the package rewrite's `observing_fonts` [`Capture::keep`].
#[derive(Default)]
pub struct Capture;

impl Capture {
    /// Keep `original` if it is the table or a font file, as `rewrite`
    /// does; the package rewrite calls it with each font it serves.
    pub fn keep(path: &str, original: &[u8]) {
        let mut guard = captured();
        let c = guard.get_or_insert_with(Captured::default);
        let lower = path.to_ascii_lowercase();
        if lower == TABLE {
            c.name = table_line(original, HUD_FONT_LINE);
            let Some(name) = c.name.clone() else {
                crate::log::note(&format!(
                    "superbounce: {TABLE} has no line {HUD_FONT_LINE}; world labels use the 8x8 debug font"
                ));
                c.early.clear();
                return;
            };
            if let Some(i) = c
                .early
                .iter()
                .position(|(n, _)| n.eq_ignore_ascii_case(&name))
            {
                c.font = Some(c.early.swap_remove(i));
            }
            c.early.clear();
            return;
        }
        let name = file_name(path);
        match &c.name {
            Some(want) if want.eq_ignore_ascii_case(name) => {
                c.font = Some((name.to_string(), original.into()));
                c.built = None;
            }
            Some(_) => {}
            // Before the table: keep the Latin fonts until it says which.
            None if !["cht_", "jpn_", "kor_"]
                .iter()
                .any(|p| lower.contains(&format!("\\{p}"))) =>
            {
                c.early.push((name.to_string(), original.into()));
            }
            None => {}
        }
    }
}

impl vfs::Rewrite for Capture {
    fn wants(&self, path: &str) -> bool {
        let p = path.to_ascii_lowercase();
        p == TABLE || (p.starts_with(FONTS) && !p.ends_with(".txt"))
    }
    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        Self::keep(path, original);
        None
    }
}

/// Every glyph the world labels draw: printable ASCII and the buttons.
fn units() -> impl Iterator<Item = u16> {
    (0x20..=0x7e).chain(BUTTONS)
}

/// One decoded glyph as straight-alpha RGBA. Plain glyphs are white,
/// tinted by the label; a glyph with any lit pixel not white keeps its
/// colours (the face buttons), as the HUD draws them.
fn pixels(unit: u16, g: &font::Glyph) -> GlyphPixels {
    let r = &g.record;
    let mut rgba = Vec::with_capacity(g.pixels.len() * 4);
    let mut coloured = false;
    for &px in &g.pixels {
        let a = ((px >> 12) * 17) as u8;
        coloured |= a > 0 && px & 0x0fff != 0x0fff;
        let c = |shift: u16| (((px >> shift) & 15) * 17) as u8;
        rgba.extend([c(8), c(4), c(0), a]);
    }
    if !coloured {
        for px in rgba.chunks_exact_mut(4) {
            px[..3].fill(255);
        }
    }
    GlyphPixels {
        unit,
        advance: r.advance,
        x_offset: r.x_offset,
        y_offset: r.y_offset,
        width: r.width,
        height: r.height,
        rgba,
        coloured,
    }
}

/// The label atlas of a font file: its printable ASCII and button glyphs
/// that it has ([`font::Font::has_glyph`]), and their kerning.
pub fn atlas(name: &str, bytes: &[u8]) -> Result<LabelFont, String> {
    let f = font::Font::parse(bytes).map_err(|e| format!("{name}: {e}"))?;
    let mut glyphs = Vec::new();
    for unit in units().filter(|&u| f.has_glyph(u)) {
        let g = f.glyph(unit).map_err(|e| format!("{name}: {e}"))?;
        glyphs.push(pixels(unit, &g));
    }
    if glyphs.is_empty() {
        return Err(format!("{name} has none of the label glyphs"));
    }
    let kerning = f
        .header
        .kerning
        .iter()
        .map(|k| (u16::from(k.first), u16::from(k.second)))
        .map(|(a, b)| (a, b, f.kerning(a, b)))
        .filter(|k| k.2 != 0)
        .collect();
    Ok(LabelFont::build(
        name,
        f.header.ascent,
        f.header.descent,
        glyphs,
        kerning,
    ))
}

/// The HUD font, built on first use and published to the window; or why
/// there is none.
fn hud_font() -> Result<Arc<LabelFont>, String> {
    let mut guard = captured();
    let c = guard.get_or_insert_with(Captured::default);
    if let Some(built) = &c.built {
        return built.clone();
    }
    let Some((name, bytes)) = c.font.clone() else {
        return Err(match &c.name {
            None => format!("{TABLE} was not read"),
            Some(name) => format!("the HUD font {name} was not read"),
        });
    };
    let built = atlas(&name, &bytes).map(Arc::new);
    gfx::world_lines::set_hud_font(built.as_ref().ok().cloned());
    c.built = Some(built.clone());
    built
}

/// The user's HUD text colour from the guest.
pub fn hud_colour(m: &dyn GuestMemory) -> Result<[f32; 3], String> {
    let c = m
        .vec3(HUD_COLOUR_ADDR)
        .map_err(|e| format!("reading {HUD_COLOUR_ADDR:#010x}: {e:?}"))?;
    if c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
        Ok(c)
    } else {
        Err(format!("{HUD_COLOUR_ADDR:#010x} holds {c:?}, not a colour"))
    }
}

/// How this frame's labels look, noting once each thing that falls back.
pub fn look(m: &dyn GuestMemory) -> Look {
    let font = hud_font();
    let colour = hud_colour(m);
    let mut guard = captured();
    let c = guard.get_or_insert_with(Captured::default);
    if let Err(why) = &font
        && !c.noted_font
    {
        c.noted_font = true;
        crate::log::note(&format!(
            "superbounce: world labels use the 8x8 debug font and button letters: {why}"
        ));
    }
    if let Err(why) = &colour
        && !c.noted_colour
    {
        c.noted_colour = true;
        crate::log::note(&format!(
            "superbounce: world lines are white, not the HUD colour: {why}"
        ));
    }
    Look {
        colour: colour.unwrap_or(UNREAD_COLOUR),
        face: LabelFace::Hud,
        glyphs: font.is_ok_and(|f| f.covers(&X_GLYPH.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_names_a_font_by_its_line_from_zero() {
        let table = b"fixedsys-9\r\nconduit-12\r\nhandel_gothic-14\r\nconduit-12\r\nconduit-13\r\nconduit-12\r\nconduit-13\r\nhandel_gothic-11\r\nMSLCD-14\r\nconduit-16\r\nhandel_gothic-24";
        assert_eq!(table_line(table, 6).as_deref(), Some("conduit-13"));
        assert_eq!(table_line(table, 8).as_deref(), Some("MSLCD-14"));
        assert_eq!(table_line(table, 10).as_deref(), Some("handel_gothic-24"));
        assert_eq!(table_line(table, 11), None);
        assert_eq!(table_line(b"a\r\n\r\n", 1), None);
    }

    #[test]
    fn the_capture_wants_the_table_and_the_fonts_and_changes_nothing() {
        use vfs::Rewrite;
        let mut c = Capture;
        assert!(c.wants("maps\\fonts\\font_table.txt"));
        assert!(c.wants("MAPS\\FONTS\\conduit-13"));
        assert!(!c.wants("maps\\fonts\\font_table_jpn.txt"));
        assert!(!c.wants("maps\\shared.map"));
        assert_eq!(c.rewrite("maps\\fonts\\nothing-1", b"x"), None);
    }

    #[test]
    fn plain_glyphs_are_white_and_button_glyphs_keep_their_colours() {
        let record = font::GlyphRecord {
            advance: 2,
            data_len: 0,
            width: 2,
            height: 1,
            x_offset: 0,
            y_offset: 10,
            data_offset: 0,
        };
        // A plain glyph's colour is the stream's white; half a pixel lit.
        let plain = font::Glyph {
            record,
            pixels: vec![0xffff, 0x8fff],
        };
        let p = pixels(0x41, &plain);
        assert!(!p.coloured);
        assert_eq!(p.rgba, [255, 255, 255, 255, 255, 255, 255, 136]);
        // A blue ball: its own colour, its transparent pixels not counted.
        let ball = font::Glyph {
            record,
            pixels: vec![0xf04f, 0x0123],
        };
        let b = pixels(0xe102, &ball);
        assert!(b.coloured);
        assert_eq!(&b.rgba[..4], &[0, 68, 255, 255]);
        assert_eq!(b.rgba[7], 0);
    }

    #[test]
    fn the_x_button_is_its_glyph_only_with_the_hud_font() {
        let mut look = Look {
            colour: [1.0; 3],
            face: LabelFace::Hud,
            glyphs: true,
        };
        assert_eq!(look.x_button(), "\u{e102}");
        look.glyphs = false;
        assert_eq!(look.x_button(), "X");
    }

    #[test]
    fn a_file_that_is_not_a_font_has_no_atlas() {
        assert!(atlas("junk", b"not a font").is_err());
    }
}
