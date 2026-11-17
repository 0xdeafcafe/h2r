//! A screen drawn offline (docs/game/ui-screens.md "Preview"): the
//! Controls screen's list and header from the disc's `mainmenu.map`
//! (`tags::ui`, as `h2 ui` prints them), the list skin's text rectangles,
//! the description block, and the disc's own fonts. Nothing here is kept:
//! the PNG goes where the caller says, never into the repository.
//!
//! What it does not draw: the frame art, bitmaps, the list skin's
//! highlight art and animations. Text is left-justified. The font a text
//! block's index names is taken as that line of `font_table.txt` (the
//! engine's slot order); which slot serves which widget is not traced
//! (docs/formats/fonts.md "Open"), so widths are measured in the named
//! file and the preview says which.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use tags::ui::{self as tui, MapSource, Rect, UiTag};

use crate::ui_kit::spec::{self, Finding, ListPlace, Room, ScreenSpec, ValueStyle};

/// The Controls screen whose tag our screens are built from.
pub const CONTROLS_TAG: &str = r"ui\screens\game_shell\settings_screen\player_profile\controller_settings";

/// Where a list's items and texts go, from the tags.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub anchor: [i16; 2],
    pub visible: usize,
    /// Each item is this much lower than the one before (the skin's
    /// bounding box).
    pub item_height: i32,
    /// The skin's text blocks, relative to an item's origin: label, then
    /// value when the skin has two.
    pub label: (Rect, i16),
    pub value: Option<(Rect, i16)>,
    pub item_colour: [f32; 3],
    pub header: Rect,
    pub header_font: i16,
    pub header_colour: [f32; 3],
}

/// The disc's fonts by `font_table.txt` line.
pub struct Fonts {
    pub table: Vec<String>,
    files: HashMap<String, Vec<u8>>,
}

impl Fonts {
    /// The file for engine font `index`.
    pub fn file(&self, index: i16) -> Option<(&str, &[u8])> {
        let name = self.table.get(usize::try_from(index).ok()?)?;
        self.files
            .get(name)
            .map(|b| (name.as_str(), b.as_slice()))
    }
    /// `text`'s width in font `index` (the pen's travel: kerning, x offset
    /// and advance per unit, `font::raster`'s layout), widest line.
    pub fn width(&self, index: i16, text: &str) -> i32 {
        let Some(font) = self.file(index).and_then(|(_, b)| font::Font::parse(b).ok()) else {
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
        self.file(index)
            .and_then(|(_, b)| font::Font::parse(b).ok())
            .map_or(spec::fallback::LINE, |f| f.line_height())
    }
    /// Whether font `index` has its own picture for `c` (not record 0,
    /// the filler every unmapped unit draws).
    pub fn has(&self, index: i16, c: char) -> bool {
        let Some(font) = self.file(index).and_then(|(_, b)| font::Font::parse(b).ok()) else {
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

/// The disc: `mainmenu.map`'s UI tags and the fonts.
pub struct Disc {
    pub controls: tui::Screen,
    pub shared: tui::SharedGlobals,
    skins: HashMap<i16, tui::Skin>,
    pub fonts: Fonts,
}

impl Disc {
    pub fn open(iso: &Path) -> Result<Disc> {
        let mut x = xiso::Xiso::open(BufReader::new(File::open(iso).with_context(|| iso.display().to_string())?))?;
        let part = x.partition_offset();
        let entries = x.entries()?;
        let find = |p: &str| {
            entries
                .iter()
                .find(|e| e.path.eq_ignore_ascii_case(p))
                .cloned()
                .ok_or_else(|| anyhow!("{p} is not on the disc"))
        };
        let table = String::from_utf8_lossy(&x.read(&find("/maps/fonts/font_table.txt")?)?).into_owned();
        let table: Vec<String> = table
            .split(['\r', '\n'])
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
            .collect();
        let mut files = HashMap::new();
        for name in &table {
            if !files.contains_key(name) {
                files.insert(name.clone(), x.read(&find(&format!("/maps/fonts/{name}"))?)?);
            }
        }
        let e = find("/maps/mainmenu.map")?;
        let mut m = map::Map::from_region(
            BufReader::new(File::open(iso)?),
            part + u64::from(e.sector) * xiso::SECTOR,
            u64::from(e.size),
        )?;
        let controls_id = m.tag(map::TagQuery::NameInGroup(CONTROLS_TAG, tui::SCREEN))?.id;
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
        let UiTag::Screen(controls) = src.decode_tag(controls_id)?.tag else {
            bail!("the Controls tag is not a screen");
        };
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
            shared: *shared,
            skins,
            fonts: Fonts { table, files },
        })
    }

    /// The list of `place` (the Controls one, or moved and reskinned).
    pub fn geometry(&self, place: ListPlace) -> Result<Geometry> {
        let list = self
            .controls
            .panes
            .first()
            .and_then(|p| p.lists.first())
            .context("the Controls tag has no list")?;
        let (skin_index, anchor) = match place {
            ListPlace::Controls => (list.skin, [list.anchor.x, list.anchor.y]),
            ListPlace::Moved { skin, anchor } => (skin, anchor),
        };
        let skin = self
            .skins
            .get(&skin_index)
            .with_context(|| format!("no list skin {skin_index}"))?;
        let texts: Vec<(Rect, i16, [f32; 3])> = skin
            .text_blocks
            .iter()
            .map(|t| (t.rect, t.font, t.text_color))
            .collect();
        let (label_rect, label_font, colour) = *texts.first().context("the skin has no text")?;
        // The bounding box over the item's children (text blocks, and
        // bitmaps without flag bit 0), top minus bottom (`0x2bafa4`).
        let mut top = i32::MIN;
        let mut bottom = i32::MAX;
        for r in texts.iter().map(|t| t.0).chain(
            skin.bitmaps
                .iter()
                .filter(|b| b.flags & 1 == 0)
                .map(|b| b.rect(None)),
        ) {
            top = top.max(i32::from(r.top));
            bottom = bottom.min(i32::from(r.bottom));
        }
        let layout = self.controls.header_layout();
        Ok(Geometry {
            anchor,
            visible: usize::try_from(list.visible_items).unwrap_or(0),
            item_height: top - bottom,
            label: (label_rect, label_font),
            value: texts.get(1).map(|t| (t.0, t.1)),
            item_colour: colour,
            header: self.shared.header_layouts[layout].header,
            header_font: self.shared.header_fonts[layout],
            header_colour: [
                self.shared.header_color[1],
                self.shared.header_color[2],
                self.shared.header_color[3],
            ],
        })
    }

    /// [`spec::check`] against this disc's geometry and fonts.
    pub fn check(&self, s: &ScreenSpec) -> Result<Vec<Finding>> {
        let g = self.geometry(s.list)?;
        let row_font = g.label.1;
        let desc_font = s.description.map_or(1, |d| d.font);
        let row_width = |t: &str| self.fonts.width(row_font, t);
        let description_width = |t: &str| self.fonts.width(desc_font, t);
        let has_glyph = |c: char| self.fonts.has(row_font, c);
        let room = Room {
            visible: g.visible,
            label: g.label.0.width(),
            value: g.value.map(|v| v.0.width()),
            row_width: &row_width,
            description_width: &description_width,
            description_line: self.fonts.line_height(desc_font),
            has_glyph: &has_glyph,
        };
        Ok(spec::check(s, &room))
    }

    /// `s` drawn at 640 by 480, its findings outlined in red.
    pub fn render(&self, s: &ScreenSpec) -> Result<Canvas> {
        let g = self.geometry(s.list)?;
        let mut c = Canvas::new(640, 480, [12, 22, 40, 255]);
        let rgb = |v: [f32; 3]| v.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8);
        // Header.
        c.text(&self.fonts, g.header_font, &s.title, g.header, rgb(g.header_colour));
        // Rows: the window of `visible` around the focus.
        let first = s.focus.saturating_sub(g.visible.saturating_sub(1));
        for (slot, (i, row)) in s.rows.iter().enumerate().skip(first).take(g.visible).enumerate() {
            let dy = -(slot as i32) * g.item_height;
            let at = |r: Rect| Rect {
                top: (i32::from(r.top) + i32::from(g.anchor[1]) + dy) as i16,
                bottom: (i32::from(r.bottom) + i32::from(g.anchor[1]) + dy) as i16,
                left: r.left + g.anchor[0],
                right: r.right + g.anchor[0],
            };
            let focused = i == s.focus;
            let colour = if focused { [255, 255, 255] } else { rgb(g.item_colour) };
            let (label, value) = row.cells(match g.value {
                Some(_) => s.style,
                None => ValueStyle::Inline,
            });
            let lr = at(g.label.0);
            if focused {
                c.fill(lr, [40, 70, 110, 255]);
            }
            let w = self.fonts.width(g.label.1, &label);
            c.text(&self.fonts, g.label.1, &label, lr, colour);
            if w > g.label.0.width() {
                c.outline(lr, [230, 40, 40, 255]);
            }
            if let (Some(v), Some((vr, vf))) = (value, g.value) {
                let vr = at(vr);
                c.text(&self.fonts, vf, &v, vr, colour);
                if self.fonts.width(vf, &v) > vr.width() {
                    c.outline(vr, [230, 40, 40, 255]);
                }
            }
        }
        if s.rows.len() > g.visible {
            let y = i32::from(g.anchor[1]) - g.visible as i32 * g.item_height;
            let r = Rect {
                top: y as i16,
                bottom: (y - 3) as i16,
                left: g.anchor[0],
                right: g.anchor[0] + 40,
            };
            c.fill(r, if s.scrolls { [120, 160, 200, 255] } else { [230, 40, 40, 255] });
        }
        // Description and legend.
        if let (Some(d), Some(text)) = (s.description, s.description_text()) {
            let rect = Rect {
                top: d.rect[0],
                left: d.rect[1],
                bottom: d.rect[2],
                right: d.rect[3],
            };
            let lines = spec::wrap(&text, rect.width(), &|t| self.fonts.width(d.font, t));
            let lh = self.fonts.line_height(d.font);
            let fits = (rect.height() / lh.max(1)) as usize;
            c.text(&self.fonts, d.font, &lines.join("\n"), rect, rgb(d.colour));
            c.outline(rect, if lines.len() > fits { [230, 40, 40, 255] } else { [50, 70, 100, 255] });
        } else if !s.prompts.is_empty() {
            let legend = crate::ui_kit::input::legend(&s.prompts);
            let r = Rect {
                top: -180,
                bottom: -200,
                left: -280,
                right: 280,
            };
            c.text(&self.fonts, 1, &legend, r, [200, 200, 200]);
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
    fn px(&self, r: Rect) -> (i32, i32, i32, i32) {
        let (cx, cy) = (self.width as i32 / 2, self.height as i32 / 2);
        (
            cx + i32::from(r.left),
            cy - i32::from(r.top),
            cx + i32::from(r.right),
            cy - i32::from(r.bottom),
        )
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
    pub fn fill(&mut self, r: Rect, c: [u8; 4]) {
        let (x0, y0, x1, y1) = self.px(r);
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, c);
            }
        }
    }
    pub fn outline(&mut self, r: Rect, c: [u8; 4]) {
        let (x0, y0, x1, y1) = self.px(r);
        for x in x0..=x1 {
            self.blend(x, y0, c);
            self.blend(x, y1, c);
        }
        for y in y0..=y1 {
            self.blend(x0, y, c);
            self.blend(x1, y, c);
        }
    }
    /// `text` in font `index` from `r`'s top-left (not clipped: what runs
    /// out of the rectangle shows).
    pub fn text(&mut self, fonts: &Fonts, index: i16, text: &str, r: Rect, rgb: [u8; 3]) {
        let Some(font) = fonts.file(index).and_then(|(_, b)| font::Font::parse(b).ok()) else {
            return;
        };
        let units: Vec<u16> = text.encode_utf16().collect();
        let Ok(b) = font::render(
            &font,
            &units,
            font::RenderOptions {
                color: [rgb[0], rgb[1], rgb[2], 255],
                glyph_colors: false,
            },
        ) else {
            return;
        };
        let (x0, y0, _, _) = self.px(r);
        for y in 0..b.height {
            for x in 0..b.width {
                let i = ((y * b.width + x) * 4) as usize;
                let p = [b.rgba[i], b.rgba[i + 1], b.rgba[i + 2], b.rgba[i + 3]];
                if p[3] != 0 {
                    self.blend(x0 + x as i32, y0 + y as i32, p);
                }
            }
        }
    }
    pub fn write_png(&self, path: &Path) -> Result<()> {
        let f = std::io::BufWriter::new(File::create(path).with_context(|| path.display().to_string())?);
        let mut e = png::Encoder::new(f, self.width, self.height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?.write_image_data(&self.rgba)?;
        Ok(())
    }
}
