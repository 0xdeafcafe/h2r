//! A screen drawn offline as the game draws it (docs/game/ui-screens.md
//! "Preview"), from the disc's `mainmenu.map` and fonts:
//!
//! - the game shell's background screen (`game_shell_background`: the
//!   header and footer bands of `framing_center`, the scrolling track
//!   lines), then the Controls screen's own frame art, each bitmap block
//!   drawn at its image's size, lowest render depth first;
//! - the header text in the `wigl` header layout's rectangle, font and
//!   colour; the footer legend in the layout's second rectangle;
//! - the list from its anchor, one item per row down to the rows shown,
//!   each item the skin's bitmaps (the row bar, the frame) and texts
//!   (label, value) with their justification, focused bright;
//! - the description block; text with a drop shadow; button glyphs in
//!   their own colours from the font.
//!
//! What it leaves out: animations (all at rest), the scroll of the track
//! lines, the skulls page's picture, the dimming of unfocused rows (taken
//! as 55%, not traced) and model scenes. Nothing is kept: the PNG goes
//! where the caller says, never into the repository.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use map::TagId;
use tags::ui::{self as tui, MapSource, Rect, UiTag};

use crate::ui_kit::spec::{self, Finding, ListPlace, Room, ScreenSpec, ValueStyle};

/// The Controls screen whose tag our screens are built from.
pub const CONTROLS_TAG: &str =
    r"ui\screens\game_shell\settings_screen\player_profile\controller_settings";
/// The screen drawn under every game shell screen.
pub const BACKGROUND_TAG: &str = r"ui\screens\game_shell\main_menu_screen\game_shell_background";

/// Rows a screen of ours shows before it scrolls. The layout
/// (`0x24bc12`) places up to the list block's visible count (the Controls
/// tag's 6) of the handler's item children, one item height apart; the
/// co-op screen in game (2026-11-16 screenshot: six rows, row 0 at the
/// top, focus on row 4, row 5 not drawn) shows five. The check takes the
/// smaller.
pub const SHOWN: usize = 5;

/// Where a list's items and texts go, from the tags.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub anchor: [i16; 2],
    pub visible: usize,
    /// Each item is this much lower than the one before (the item's
    /// bounding box over its texts and its bitmaps without flag bit 0).
    pub item_height: i32,
    /// The skin's text blocks, relative to an item's origin: label, then
    /// value when the skin has two; font and justification.
    pub label: (Rect, i16, u8),
    pub value: Option<(Rect, i16, u8)>,
    pub item_colour: [f32; 3],
    pub header: Rect,
    pub header_font: i16,
    pub header_colour: [f32; 3],
    /// The footer legend's rectangle (the header layout's second one).
    pub footer: Rect,
}

/// The disc's fonts by `font_table.txt` line: a text block's font index is
/// the engine font id (`0x2537d7` keeps the block's `+0xa`), and every
/// measure and draw maps an id through the 11-entry table at `0x4e28f4`
/// (`0x1407d8`, `0x122540`), which the font init (`0x121fc0`) fills line by
/// line from `font_table.txt`.
pub struct Fonts {
    pub table: Vec<String>,
    files: HashMap<String, Vec<u8>>,
}

impl Fonts {
    /// The file for engine font `index`.
    pub fn file(&self, index: i16) -> Option<(&str, &[u8])> {
        let name = self.table.get(usize::try_from(index).ok()?)?;
        self.files.get(name).map(|b| (name.as_str(), b.as_slice()))
    }
    fn font(&self, index: i16) -> Option<font::Font<'_>> {
        self.file(index)
            .and_then(|(_, b)| font::Font::parse(b).ok())
    }
    /// `text`'s width in font `index` (the pen's travel: kerning, x offset
    /// and advance per unit, `font::raster`'s layout), widest line.
    pub fn width(&self, index: i16, text: &str) -> i32 {
        let Some(font) = self.font(index) else {
            return spec::fallback::width(text);
        };
        text.split('\n')
            .map(|line| {
                let mut prev = None;
                let mut w = 0i32;
                for u in line.trim_end_matches('\r').encode_utf16() {
                    if u < 0x20 {
                        continue;
                    }
                    let k = prev.map_or(0, |p| i32::from(font.kerning(p, u)));
                    if let Ok(r) = font.record(font.glyph_index(u)) {
                        w += k + i32::from(r.x_offset) + i32::from(r.advance);
                    }
                    prev = Some(u);
                }
                w
            })
            .max()
            .unwrap_or(0)
    }
    pub fn line_height(&self, index: i16) -> i32 {
        self.font(index)
            .map_or(spec::fallback::LINE, |f| f.line_height())
    }
    /// Whether font `index` has its own picture for `c` (not record 0,
    /// nor the filler every unused icon code point draws).
    pub fn has(&self, index: i16, c: char) -> bool {
        let Some(font) = self.font(index) else {
            return true;
        };
        let filler = font.glyph_index(0xe3ff);
        let mut buf = [0u16; 2];
        c.encode_utf16(&mut buf).iter().all(|&u| {
            let i = font.glyph_index(u);
            u == u16::from(b' ') || (i != 0 && !((0xe000..=0xe3ff).contains(&u) && i == filler))
        })
    }
}

/// A decoded bitmap image.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The disc: `mainmenu.map`'s UI tags, its bitmaps and the fonts.
pub struct Disc {
    pub controls: tui::Screen,
    pub background: tui::Screen,
    pub shared: tui::SharedGlobals,
    skins: HashMap<i16, tui::Skin>,
    pub fonts: Fonts,
    map: RefCell<map::Map<BufReader<File>>>,
    files: RefCell<assets::ResourceFiles>,
    images: RefCell<HashMap<(TagId, i16), Option<Image>>>,
}

impl Disc {
    pub fn open(iso: &Path) -> Result<Disc> {
        let mut x = xiso::Xiso::open(BufReader::new(
            File::open(iso).with_context(|| iso.display().to_string())?,
        ))?;
        let part = x.partition_offset();
        let entries = x.entries()?;
        let find = |p: &str| {
            entries
                .iter()
                .find(|e| e.path.eq_ignore_ascii_case(p))
                .cloned()
                .ok_or_else(|| anyhow!("{p} is not on the disc"))
        };
        let table =
            String::from_utf8_lossy(&x.read(&find("/maps/fonts/font_table.txt")?)?).into_owned();
        let table: Vec<String> = table
            .split(['\r', '\n'])
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
            .collect();
        let mut files = HashMap::new();
        for name in &table {
            if !files.contains_key(name) {
                files.insert(
                    name.clone(),
                    x.read(&find(&format!("/maps/fonts/{name}"))?)?,
                );
            }
        }
        let region = |name: &str| -> Result<assets::Region> {
            let e = find(&format!("/maps/{name}.map"))?;
            Ok(assets::Region {
                path: iso.to_path_buf(),
                base: part + u64::from(e.sector) * xiso::SECTOR,
                length: e.size.into(),
            })
        };
        let menu = region("mainmenu")?;
        let mut res = assets::ResourceFiles::new();
        res.set(map::ResourceFile::Local, menu.clone())?;
        res.set(map::ResourceFile::MainMenu, menu.clone())?;
        if let Ok(r) = region("shared") {
            res.set(map::ResourceFile::Shared, r)?;
        }
        let mut m =
            map::Map::from_region(BufReader::new(File::open(iso)?), menu.base, menu.length)?;
        let id_of = |m: &map::Map<BufReader<File>>, name: &str| -> Result<TagId> {
            Ok(m.tag(map::TagQuery::NameInGroup(name, tui::SCREEN))?.id)
        };
        let (controls_id, background_id) = (id_of(&m, CONTROLS_TAG)?, id_of(&m, BACKGROUND_TAG)?);
        let wigl: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == tui::SHARED_GLOBALS)
            .map(|t| t.id)
            .collect();
        let [wigl] = wigl.as_slice() else {
            bail!("mainmenu.map has {} wigl tags", wigl.len());
        };
        let mut src = MapSource::new(&mut m);
        let screen = |src: &mut MapSource<'_, _>, id| -> Result<tui::Screen> {
            match src.decode_tag(id)?.tag {
                UiTag::Screen(s) => Ok(s),
                _ => bail!("{id} is not a screen"),
            }
        };
        let controls = screen(&mut src, controls_id)?;
        let background = screen(&mut src, background_id)?;
        let UiTag::SharedGlobals(shared) = src.decode_tag(*wigl)?.tag else {
            bail!("the wigl tag is not shared globals");
        };
        let mut skins = HashMap::new();
        for i in 0..shared.skins.len() as i16 {
            if let Some(id) = shared.skin(i).and_then(|r| r.id)
                && let Ok((tree, _)) = tui::resolve_skin(&mut src, id)
            {
                skins.insert(i, tree.skin);
            }
        }
        Ok(Disc {
            controls,
            background,
            shared: *shared,
            skins,
            fonts: Fonts { table, files },
            map: RefCell::new(m),
            files: RefCell::new(res),
            images: RefCell::new(HashMap::new()),
        })
    }

    /// Image `index` of the bitmap tag `r`, decoded once.
    pub fn image(&self, r: tui::TagRef, index: i16) -> Option<Image> {
        let id = r.id?;
        if let Some(i) = self.images.borrow().get(&(id, index)) {
            return i.clone();
        }
        let img = (|| -> Option<Image> {
            let mut m = self.map.borrow_mut();
            let tags::TagData::Bitmap(b) = tags::DecodedTag::read(&mut m, id).ok()?.data else {
                return None;
            };
            let image = b.images.get(usize::try_from(index).ok()?)?;
            let d = assets::decode_image(image, &mut self.files.borrow_mut()).ok()?;
            let l = d.levels.into_iter().find(|l| l.level == 0 && l.face == 0)?;
            let n = (l.width * l.height * 4) as usize;
            Some(Image {
                width: l.width,
                height: l.height,
                rgba: l.rgba.get(..n)?.to_vec(),
            })
        })();
        self.images.borrow_mut().insert((id, index), img.clone());
        img
    }

    fn skin(&self, place: ListPlace) -> Result<(&tui::List, &tui::Skin, [i16; 2])> {
        let list = self
            .controls
            .panes
            .first()
            .and_then(|p| p.lists.first())
            .context("the Controls tag has no list")?;
        let (index, anchor) = match place {
            ListPlace::Controls => (list.skin, [list.anchor.x, list.anchor.y]),
            ListPlace::Moved { skin, anchor, .. } => (skin, anchor),
        };
        let skin = self
            .skins
            .get(&index)
            .with_context(|| format!("no list skin {index}"))?;
        Ok((list, skin, anchor))
    }

    /// An item bitmap's extent above the item's origin: `y` to `y` plus
    /// its height (Y up). The engine's item height for the Controls skin
    /// is then its row bar's 26 (`0x2bafa4`), the spacing the game draws.
    fn item_bitmap_span(&self, b: &tui::BitmapBlock) -> Option<(i32, i32, Image)> {
        let img = self.image(b.bitmap, b.image)?;
        let y = i32::from(b.top_left.y);
        Some((y + img.height as i32, y, img))
    }

    /// The list of `place` (the Controls one, or moved and reskinned).
    pub fn geometry(&self, place: ListPlace) -> Result<Geometry> {
        let (list, skin, anchor) = self.skin(place)?;
        let texts: Vec<(Rect, i16, u8, [f32; 3])> = skin
            .text_blocks
            .iter()
            .map(|t| (t.rect, t.font, tui::justification(t.flags), t.text_color))
            .collect();
        let &(label_rect, label_font, label_just, colour) =
            texts.first().context("the skin has no text")?;
        let mut top = i32::MIN;
        let mut bottom = i32::MAX;
        for (r, ..) in &texts {
            top = top.max(i32::from(r.top));
            bottom = bottom.min(i32::from(r.bottom));
        }
        for b in skin.bitmaps.iter().filter(|b| b.flags & 1 == 0) {
            if let Some((t, bt, _)) = self.item_bitmap_span(b) {
                top = top.max(t);
                bottom = bottom.min(bt);
            }
        }
        let layout = self.controls.header_layout();
        let hl = self.shared.header_layouts[layout];
        Ok(Geometry {
            anchor,
            visible: usize::try_from(list.visible_items).unwrap_or(0).min(SHOWN),
            item_height: top - bottom,
            label: (label_rect, label_font, label_just),
            value: texts.get(1).map(|t| (t.0, t.1, t.2)),
            item_colour: colour,
            header: hl.header,
            header_font: self.shared.header_fonts[layout],
            header_colour: [
                self.shared.header_color[1],
                self.shared.header_color[2],
                self.shared.header_color[3],
            ],
            footer: hl.secondary,
        })
    }

    /// [`spec::check`] against this disc's geometry and fonts.
    pub fn check(&self, s: &ScreenSpec) -> Result<Vec<Finding>> {
        let g = self.geometry(s.list)?;
        let row_font = g.label.1;
        let desc_font = s.description.map_or(1, |d| d.font);
        let row_width = |t: &str| self.fonts.width(row_font, t);
        let description_width = |t: &str| self.fonts.width(desc_font, t);
        let footer_width = |t: &str| self.fonts.width(g.header_font, t);
        let has_glyph = |c: char| self.fonts.has(row_font, c);
        let room = Room {
            visible: g.visible,
            label: g.label.0.width(),
            value: g.value.map(|v| v.0.width()),
            row_width: &row_width,
            description_width: &description_width,
            description_line: self.fonts.line_height(desc_font),
            has_glyph: &has_glyph,
            footer: g.footer.width(),
            footer_width: &footer_width,
        };
        Ok(spec::check(s, &room))
    }

    /// A screen's bitmap blocks, lowest render depth first.
    fn draw_blocks<'a>(&self, c: &mut Canvas, blocks: impl Iterator<Item = &'a tui::BitmapBlock>) {
        let mut v: Vec<&tui::BitmapBlock> = blocks.collect();
        v.sort_by_key(|b| b.render_depth_bias);
        for b in v {
            if let Some(img) = self.image(b.bitmap, b.image) {
                c.image(&img, i32::from(b.top_left.x), i32::from(b.top_left.y), 255);
            }
        }
    }

    /// `s` drawn at 640 by 480; findings outlined in red.
    pub fn render(&self, s: &ScreenSpec) -> Result<Canvas> {
        let g = self.geometry(s.list)?;
        let (_, skin, _) = self.skin(s.list)?;
        let mut c = Canvas::new(640, 480, [0, 0, 0, 255]);
        let rgb = |v: [f32; 3]| v.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8);
        // The background screen, then the Controls frame art (all of it
        // for the Controls place, left of the cut for a moved list).
        for p in &self.background.panes {
            self.draw_blocks(&mut c, p.bitmaps.iter());
        }
        let cut = match s.list {
            ListPlace::Controls => i16::MAX,
            ListPlace::Moved {
                bitmaps_left_of, ..
            } => bitmaps_left_of,
        };
        if let Some(p) = self.controls.panes.first() {
            self.draw_blocks(&mut c, p.bitmaps.iter().filter(|b| b.top_left.x < cut));
        }
        // Header and footer.
        c.text(
            &self.fonts,
            g.header_font,
            &s.title,
            g.header,
            0,
            rgb(g.header_colour),
        );
        if let Some(f) = s.footer_text() {
            c.text(
                &self.fonts,
                g.header_font,
                &f,
                g.footer,
                1,
                rgb(g.header_colour),
            );
            if self.fonts.width(g.header_font, &f) > g.footer.width() {
                c.outline(g.footer, [230, 40, 40, 255]);
            }
        }
        // Rows: the window of `visible` around the focus.
        let first = s.focus.saturating_sub(g.visible.saturating_sub(1));
        let style = match g.value {
            Some(_) => s.style,
            None => ValueStyle::Inline,
        };
        for (slot, (i, row)) in s
            .rows
            .iter()
            .enumerate()
            .skip(first)
            .take(g.visible)
            .enumerate()
        {
            let ox = i32::from(g.anchor[0]);
            let oy = i32::from(g.anchor[1]) - slot as i32 * g.item_height;
            let focused = i == s.focus;
            let mut bitmaps: Vec<&tui::BitmapBlock> = skin.bitmaps.iter().collect();
            bitmaps.sort_by_key(|b| b.render_depth_bias);
            for b in bitmaps {
                if let Some((top, _, img)) = self.item_bitmap_span(b) {
                    // The frame (flag bit 0, left out of the height) is
                    // every row's, lit on the focused one.
                    let alpha = if b.flags & 1 != 0 && !focused {
                        110
                    } else {
                        255
                    };
                    c.image(&img, ox + i32::from(b.top_left.x), oy + top, alpha);
                }
            }
            let at = |r: Rect| Rect {
                top: (i32::from(r.top) + oy) as i16,
                bottom: (i32::from(r.bottom) + oy) as i16,
                left: (i32::from(r.left) + ox) as i16,
                right: (i32::from(r.right) + ox) as i16,
            };
            let colour = rgb(if focused {
                g.item_colour
            } else {
                g.item_colour.map(|v| v * 0.55)
            });
            let (label, value) = row.cells(style);
            let (lr, lf, lj) = g.label;
            c.text(&self.fonts, lf, &label, at(lr), lj, colour);
            if self.fonts.width(lf, &label) > lr.width() {
                c.outline(at(lr), [230, 40, 40, 255]);
            }
            if let (Some(v), Some((vr, vf, vj))) = (value, g.value) {
                c.text(&self.fonts, vf, &v, at(vr), vj, colour);
                if self.fonts.width(vf, &v) > vr.width() {
                    c.outline(at(vr), [230, 40, 40, 255]);
                }
            }
        }
        if s.rows.len() > g.visible && !s.scrolls {
            let y = i32::from(g.anchor[1]) - g.visible as i32 * g.item_height;
            c.fill(
                Rect {
                    top: y as i16,
                    bottom: (y - 3) as i16,
                    left: g.anchor[0],
                    right: g.anchor[0] + 40,
                },
                [230, 40, 40, 255],
            );
        }
        // Description.
        if let (Some(d), Some(text)) = (s.description, s.description_text()) {
            let rect = Rect {
                top: d.rect[0],
                left: d.rect[1],
                bottom: d.rect[2],
                right: d.rect[3],
            };
            let lines = spec::wrap(&text, rect.width(), &|t| self.fonts.width(d.font, t));
            let fits = (rect.height() / self.fonts.line_height(d.font).max(1)) as usize;
            c.text(
                &self.fonts,
                d.font,
                &lines.join("\n"),
                rect,
                0,
                rgb(d.colour),
            );
            if lines.len() > fits {
                c.outline(rect, [230, 40, 40, 255]);
            }
        }
        Ok(c)
    }
}

/// An RGBA canvas in UI units: 640 by 480, origin at the centre, Y up.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32, bg: [u8; 4]) -> Self {
        Canvas {
            width,
            height,
            rgba: bg.repeat((width * height) as usize),
        }
    }
    fn px(&self, x: i32, y: i32) -> (i32, i32) {
        (self.width as i32 / 2 + x, self.height as i32 / 2 - y)
    }
    fn blend(&mut self, x: i32, y: i32, c: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = ((y as u32 * self.width + x as u32) * 4) as usize;
        let a = u32::from(c[3]);
        for (d, s) in self.rgba[i..i + 3].iter_mut().zip(c) {
            *d = ((u32::from(s) * a + u32::from(*d) * (255 - a)) / 255) as u8;
        }
        self.rgba[i + 3] = 255;
    }
    /// `img` with its top-left at UI point `(x, y)`, its alpha scaled by
    /// `alpha`.
    pub fn image(&mut self, img: &Image, x: i32, y: i32, alpha: u8) {
        let (x0, y0) = self.px(x, y);
        for row in 0..img.height {
            for col in 0..img.width {
                let i = ((row * img.width + col) * 4) as usize;
                let p = &img.rgba[i..i + 4];
                let a = (u32::from(p[3]) * u32::from(alpha) / 255) as u8;
                if a != 0 {
                    self.blend(x0 + col as i32, y0 + row as i32, [p[0], p[1], p[2], a]);
                }
            }
        }
    }
    pub fn fill(&mut self, r: Rect, c: [u8; 4]) {
        let (x0, y0) = self.px(i32::from(r.left), i32::from(r.top));
        let (x1, y1) = self.px(i32::from(r.right), i32::from(r.bottom));
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, c);
            }
        }
    }
    pub fn outline(&mut self, r: Rect, c: [u8; 4]) {
        let (x0, y0) = self.px(i32::from(r.left), i32::from(r.top));
        let (x1, y1) = self.px(i32::from(r.right), i32::from(r.bottom));
        for x in x0..=x1 {
            self.blend(x, y0, c);
            self.blend(x, y1, c);
        }
        for y in y0..=y1 {
            self.blend(x0, y, c);
            self.blend(x1, y, c);
        }
    }
    /// One run of text at pixel `(x, y)` (its top-left).
    fn run(
        &mut self,
        font: &font::Font<'_>,
        units: &[u16],
        x: i32,
        y: i32,
        colour: [u8; 4],
        own: bool,
    ) {
        let Ok(b) = font::render(
            font,
            units,
            font::RenderOptions {
                color: colour,
                glyph_colors: own,
            },
        ) else {
            return;
        };
        for row in 0..b.height {
            for col in 0..b.width {
                let i = ((row * b.width + col) * 4) as usize;
                let p = [b.rgba[i], b.rgba[i + 1], b.rgba[i + 2], b.rgba[i + 3]];
                if p[3] != 0 {
                    self.blend(x + col as i32, y + row as i32, p);
                }
            }
        }
    }
    /// `text` in font `index` in `r`, justified (0 left, 1 right, 2
    /// centre), with a drop shadow; button glyphs (U+E000 to U+E3FF) in
    /// their own colours. Not clipped: what runs out of the rectangle
    /// shows.
    pub fn text(&mut self, fonts: &Fonts, index: i16, text: &str, r: Rect, just: u8, rgb: [u8; 3]) {
        let Some(font) = fonts.font(index) else {
            return;
        };
        let line_h = font.line_height();
        let (left, top) = self.px(i32::from(r.left), i32::from(r.top));
        for (n, line) in text.split('\n').enumerate() {
            let w = fonts.width(index, line);
            let x = match just {
                1 => left + r.width() - w,
                2 => left + (r.width() - w) / 2,
                _ => left,
            };
            let y = top + n as i32 * line_h;
            // Runs of plain text and of icons, each drawn on its own.
            let mut runs: Vec<(bool, String)> = Vec::new();
            for ch in line.chars() {
                let icon = ('\u{e000}'..='\u{e3ff}').contains(&ch);
                match runs.last_mut() {
                    Some((i, s)) if *i == icon => s.push(ch),
                    _ => runs.push((icon, ch.to_string())),
                }
            }
            let mut pen = x;
            for (icon, s) in runs {
                let units: Vec<u16> = s.encode_utf16().collect();
                if !icon {
                    self.run(&font, &units, pen + 1, y + 1, [0, 0, 0, 160], false);
                }
                self.run(&font, &units, pen, y, [rgb[0], rgb[1], rgb[2], 255], icon);
                pen += fonts.width(index, &s);
            }
        }
    }
    /// `self` and `other` side by side, `other` scaled to this height.
    pub fn beside(&self, other: &Canvas) -> Canvas {
        let mut out = Canvas::new(
            self.width + other.width + 8,
            self.height.max(other.height),
            [255, 255, 255, 255],
        );
        for (dx, c) in [(0, self), (self.width + 8, other)] {
            for y in 0..c.height {
                for x in 0..c.width {
                    let i = ((y * c.width + x) * 4) as usize;
                    let o = ((y * out.width + x + dx) * 4) as usize;
                    out.rgba[o..o + 4].copy_from_slice(&c.rgba[i..i + 4]);
                }
            }
        }
        out
    }
    /// A PNG (8-bit RGB or RGBA) as a canvas.
    pub fn read_png(path: &Path) -> Result<Canvas> {
        let d = png::Decoder::new(BufReader::new(File::open(path)?));
        let mut r = d.read_info()?;
        let mut buf = vec![0; r.output_buffer_size().context("png size")?];
        let info = r.next_frame(&mut buf)?;
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
            png::ColorType::Rgb => buf[..info.buffer_size()]
                .chunks(3)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            t => bail!("{t:?} png"),
        };
        Ok(Canvas {
            width: info.width,
            height: info.height,
            rgba,
        })
    }
    pub fn write_png(&self, path: &Path) -> Result<()> {
        let f = std::io::BufWriter::new(
            File::create(path).with_context(|| path.display().to_string())?,
        );
        let mut e = png::Encoder::new(f, self.width, self.height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?.write_image_data(&self.rgba)?;
        Ok(())
    }
}
