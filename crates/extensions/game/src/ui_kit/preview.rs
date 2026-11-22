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
//! A retail screen ([`Disc::render_retail`]) is drawn from its own tag in
//! a state its sample gives it ([`retail_sample`]), its texts broken and
//! cut to their rectangles as the game does ([`text_layout`]).
//!
//! What it leaves out: the scroll of the track
//! lines, the skulls page's picture, the dimming of unfocused rows (taken
//! as 55%, not traced) and model scenes. Nothing is kept: the PNG goes
//! where the caller says, never into the repository.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use map::TagId;
use tags::ui::{self as tui, MapSource, Rect, UiTag};

pub mod backdrop;
mod retail_edit;
pub mod retail_sample;
pub mod text_layout;
#[cfg(test)]
mod retail_tests;
pub use retail_edit::{
    PARKED, RetailBlocks, RetailEdits, parked, retail_block, retail_copy_id, retail_string_field,
};

use crate::ui_kit::dialog::{self, DialogGeometry, dialog_view};
use crate::ui_kit::spec::{self, Finding, ListPlace, Room, RowSpec, ScreenSpec};
use crate::ui_kit::view::{
    Art, Backing, ItemArt, Justify, Kind, Layer, ListAt, Metrics, Opener, PaneArt, ROW_TEXT,
    Synthetic, UNFOCUSED_ALPHA, View, Widget, view_of,
};

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

pub use crate::ui_kit::view::Geometry;

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
    /// Loads the installed fonts for diagnostics without decoding menu art.
    pub(crate) fn open(iso: &Path) -> Result<Self> {
        let mut disc = xiso::Xiso::open(BufReader::new(
            File::open(iso).with_context(|| iso.display().to_string())?,
        ))?;
        let entries = disc.entries()?;
        Self::read(|name| {
            let entry = entries.iter().find(|entry| entry.path.eq_ignore_ascii_case(name))
                .ok_or_else(|| anyhow!("{name} is not on the disc"))?;
            Ok(disc.read(entry)?)
        })
    }

    fn read(mut read: impl FnMut(&str) -> Result<Vec<u8>>) -> Result<Self> {
        let table =
            String::from_utf8_lossy(&read("/maps/fonts/font_table.txt")?).into_owned();
        let table: Vec<String> = table
            .split(['\r', '\n'])
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
            .collect();
        let mut files = HashMap::new();
        for name in &table {
            if !files.contains_key(name) {
                // As the game opens it: with the bot mark, the Mods
                // list's marks and, in the message fonts, the superbounce
                // medals added (`bot_mark::GearRewrite`).
                let raw = read(&format!("/maps/fonts/{name}"))?;
                let font = crate::bot_mark::GearRewrite::patch(name, &raw).unwrap_or(raw);
                files.insert(name.clone(), font);
            }
        }
        Ok(Self { table, files })
    }

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
    /// `text`'s lines in a rectangle `room` wide in font `index`, as the
    /// game breaks a text widget's ([`text_layout::lines`]).
    pub fn wrap(&self, index: i16, text: &str, room: i32) -> Vec<text_layout::Line> {
        let font = self.font(index);
        text_layout::lines(text, room, &|prev, c| match &font {
            Some(f) => glyph_advance(f, prev, c),
            None => spec::fallback::width(c.encode_utf8(&mut [0; 4])),
        })
    }
    /// How many lines a rectangle `height` high draws in font `index`
    /// ([`text_layout::lines_drawn`]).
    pub fn lines_drawn(&self, index: i16, height: i32) -> usize {
        match self.font(index) {
            Some(f) => {
                text_layout::lines_drawn(height, i32::from(f.header.ascent), f.line_height())
            }
            None => text_layout::lines_drawn(height, spec::fallback::LINE, spec::fallback::LINE),
        }
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

/// The pen's travel for `c` after `prev` in `font`: kerning, x offset
/// and advance ([`Fonts::width`]'s sum, one glyph of it).
fn glyph_advance(font: &font::Font<'_>, prev: Option<char>, c: char) -> i32 {
    let unit = |c: char| u16::try_from(u32::from(c)).ok();
    let Some(u) = unit(c) else {
        return 0;
    };
    let k = prev
        .and_then(unit)
        .map_or(0, |p| i32::from(font.kerning(p, u)));
    font.record(font.glyph_index(u))
        .map_or(0, |r| k + i32::from(r.x_offset) + i32::from(r.advance))
}

/// The emblem bitmaps a lobby's cells draw through `ui_small_emblem`:
/// 32 by 32 images with the emblem in their top-left 19 by 19.
const EMBLEM_FOREGROUND: &str = r"ui\global_bitmaps\emblems\foreground_small";
const EMBLEM_BACKGROUND: &str = r"ui\global_bitmaps\emblems\background_small";

/// An emblem from its two layers in `colours` (the foreground's two,
/// then the background's two). Each layer is two masks, one in red and
/// one in blue (`emblem_rules` in `crates/apps/cli/src/live_art.rs`,
/// docs/live/live-api.md): a
/// layer's colour is its first colour by red plus its second by blue,
/// and the foreground covers the background by the larger of its masks.
/// A pixel no mask covers is clear: the art fills only a corner of its
/// image. `None` when the layers differ in size.
fn emblem(bg: &Image, fg: &Image, colours: [[u8; 3]; 4]) -> Option<Image> {
    if (bg.width, bg.height) != (fg.width, fg.height) {
        return None;
    }
    let [fa, fb, ba, bb] = colours;
    let mut rgba = Vec::with_capacity(bg.rgba.len());
    for (b, f) in bg.rgba.chunks_exact(4).zip(fg.rgba.chunks_exact(4)) {
        let (br, bm) = (u32::from(b[0]), u32::from(b[2]));
        let (fr, fm) = (u32::from(f[0]), u32::from(f[2]));
        let keep = 255 - fr.max(fm);
        for c in 0..3 {
            let under = ((u32::from(ba[c]) * br + u32::from(bb[c]) * bm) / 255).min(255);
            let over = u32::from(fa[c]) * fr + u32::from(fb[c]) * fm;
            rgba.push(((over + under * keep) / 255).min(255) as u8);
        }
        rgba.push(br.max(bm).max(fr).max(fm) as u8);
    }
    Some(Image {
        width: bg.width,
        height: bg.height,
        rgba,
    })
}

/// `img` in colour `tint`: each pixel's colour scaled by it, as the game
/// draws a white bar in a player's colour.
fn tinted(img: &Image, tint: [u8; 3]) -> Image {
    let mut out = img.clone();
    for p in out.rgba.chunks_exact_mut(4) {
        for (c, t) in p.iter_mut().zip(tint) {
            *c = (u16::from(*c) * u16::from(t) / 255) as u8;
        }
    }
    out
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
    level_pictures: OnceCell<BTreeMap<String, u32>>,
    /// Every screen of `mainmenu.map` by screen id, decoded on first use.
    screens: RefCell<Option<HashMap<i16, tui::Screen>>>,
    /// The screens modals open over, made and drawn once, or why one
    /// could not be.
    openers: RefCell<HashMap<Opener, std::result::Result<Drawn, String>>>,
}

/// The screen ids of `shared.map`'s screen tags, sorted: what a game's UI
/// can build a screen on (a screen of ours on a retail base needs its base
/// here, `crate::forge_screens`).
pub fn shared_screen_ids(iso: &Path) -> Result<Vec<i16>> {
    let mut x = xiso::Xiso::open(BufReader::new(
        File::open(iso).with_context(|| iso.display().to_string())?,
    ))?;
    let part = x.partition_offset();
    let e = x
        .entries()?
        .into_iter()
        .find(|e| e.path.eq_ignore_ascii_case("/maps/shared.map"))
        .ok_or_else(|| anyhow!("/maps/shared.map is not on the disc"))?;
    let base = part + u64::from(e.sector) * xiso::SECTOR;
    let mut m = map::Map::from_region(BufReader::new(File::open(iso)?), base, e.size.into())?;
    let ids: Vec<TagId> = m
        .tags()
        .iter()
        .filter(|t| t.group == tui::SCREEN)
        .map(|t| t.id)
        .collect();
    let mut src = MapSource::new(&mut m);
    let mut v = Vec::new();
    for t in ids {
        if let UiTag::Screen(sc) = src.decode_tag(t)?.tag {
            v.push(sc.screen_id);
        }
    }
    v.sort_unstable();
    Ok(v)
}

/// A view and its picture, shared.
type Drawn = std::sync::Arc<(View, Canvas)>;

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
        let fonts = Fonts::read(|path| Ok(x.read(&find(path)?)?))?;
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
            fonts,
            map: RefCell::new(m),
            files: RefCell::new(res),
            images: RefCell::new(HashMap::new()),
            level_pictures: OnceCell::new(),
            screens: RefCell::new(None),
            openers: RefCell::new(HashMap::new()),
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

    fn skin(&self, place: ListPlace) -> Result<(&tui::List, &tui::Skin, [i16; 2], i16)> {
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
        Ok((list, skin, anchor, index))
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
        let (list, skin, anchor, skin_index) = self.skin(place)?;
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
        let cut = match place {
            ListPlace::Controls => i16::MAX,
            ListPlace::Moved {
                bitmaps_left_of, ..
            } => bitmaps_left_of,
        };
        Ok(Geometry {
            anchor,
            visible: match place {
                ListPlace::Moved { visible, .. } if visible > 0 => place.shown(),
                _ => usize::try_from(list.visible_items).unwrap_or(0).min(SHOWN),
            },
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
            skin: skin_index,
            item_art: skin
                .bitmaps
                .iter()
                .enumerate()
                .filter_map(|(k, b)| {
                    let img = self.image(b.bitmap, b.image)?;
                    Some(ItemArt {
                        index: k as u16,
                        image: b.image,
                        at: [b.top_left.x, b.top_left.y],
                        size: [img.width as u16, img.height as u16],
                        frame: b.flags & 1 != 0,
                        depth: b.render_depth_bias,
                    })
                })
                .collect(),
            frame_art: self.pane_art(&self.controls, 0, |b| b.top_left.x < cut),
        })
    }

    /// Pane `pane`'s bitmaps of `screen` that `keep` keeps, as art by
    /// reference, at their image sizes.
    fn pane_art(
        &self,
        screen: &tui::Screen,
        pane: usize,
        keep: impl Fn(&tui::BitmapBlock) -> bool,
    ) -> Vec<PaneArt> {
        let Some(p) = screen.panes.get(pane) else {
            return Vec::new();
        };
        p.bitmaps
            .iter()
            .enumerate()
            .filter(|(_, b)| keep(b))
            .filter_map(|(k, b)| {
                let img = self.image(b.bitmap, b.image)?;
                Some(PaneArt {
                    art: Art::Screen {
                        screen_id: screen.screen_id,
                        pane: pane as u16,
                        index: k as u16,
                        image: b.image,
                    },
                    at: [b.top_left.x, b.top_left.y],
                    size: [img.width as u16, img.height as u16],
                    depth: b.render_depth_bias,
                })
            })
            .collect()
    }

    /// The large dialog as the disc lays it out: `error_dialog_large`'s
    /// header layout, panel and text block, and `dialog_live_password`'s
    /// entry cell ([`DialogGeometry::fallback`] is this, read by `h2 ui`).
    pub fn dialog_geometry(&self) -> Result<DialogGeometry> {
        self.dialog_geometry_for(dialog::DialogBase::Large)
    }

    /// Dialog `base` as the disc lays it out: its tag's header layout,
    /// panel art (the null placeholders left out), first text block's
    /// font and colour, and list, and `dialog_live_password`'s entry cell
    /// ([`DialogGeometry::fallback_for`] is this).
    pub fn dialog_geometry_for(&self, base: dialog::DialogBase) -> Result<DialogGeometry> {
        let large = self
            .screen(base.screen_id())
            .with_context(|| format!("no dialog screen {}", base.screen_id()))?;
        let password = self
            .screen(dialog::PASSWORD_SCREEN_ID)
            .context("no dialog_live_password (screen 21)")?;
        let layout = large.header_layout();
        let hl = self.shared.header_layouts[layout];
        let body = large
            .panes
            .first()
            .and_then(|p| p.text_blocks.first())
            .context("the dialog has no text block")?;
        let l = large
            .panes
            .first()
            .and_then(|p| p.lists.first())
            .context("the dialog has no list")?;
        let list = ListAt {
            anchor: [l.anchor.x, l.anchor.y],
            skin: l.skin,
        };
        let items = usize::try_from(l.visible_items).unwrap_or(0);
        let cell = self
            .pane_art(&password, 0, |_| true)
            .into_iter()
            .find(|a| matches!(a.art, Art::Screen { index, .. } if index == dialog::PASSWORD_CELL))
            .context("dialog_live_password has no password cell")?;
        Ok(DialogGeometry {
            screen_id: large.screen_id,
            list,
            list_items: items,
            rows: self.geometry(DialogGeometry::list_place(list, items))?,
            header: hl.header,
            header_font: self.shared.header_fonts[layout],
            header_colour: [
                self.shared.header_color[1],
                self.shared.header_color[2],
                self.shared.header_color[3],
            ],
            footer: hl.secondary,
            panel: self
                .pane_art(&large, 0, |b| {
                    b.top_left.x.abs() < 1000 && b.top_left.y.abs() < 1000
                })
                .into_iter()
                .filter(|a| a.size[0] > 2 && a.size[1] > 2)
                .filter(|a| {
                    !matches!(a.art, Art::Screen { index, .. }
                    if base.content_bitmaps().contains(&index))
                })
                .collect(),
            body_font: body.font,
            body_colour: body.text_color,
            cell,
        })
    }

    /// The screen of `mainmenu.map` with screen id `id`.
    /// Every screen tag's name in `mainmenu.map`, sorted: what
    /// [`Disc::render_retail`] draws.
    pub fn retail_screens(&self) -> Vec<String> {
        let m = self.map.borrow();
        let mut v: Vec<String> = m
            .tags()
            .iter()
            .filter(|t| t.group == tui::SCREEN)
            .filter_map(|t| t.name.clone())
            .collect();
        v.sort();
        v
    }

    pub fn screen(&self, id: i16) -> Option<tui::Screen> {
        if self.screens.borrow().is_none() {
            let mut all = HashMap::new();
            let mut m = self.map.borrow_mut();
            let ids: Vec<TagId> = m
                .tags()
                .iter()
                .filter(|t| t.group == tui::SCREEN)
                .map(|t| t.id)
                .collect();
            let mut src = MapSource::new(&mut m);
            for t in ids {
                if let Ok(d) = src.decode_tag(t)
                    && let UiTag::Screen(sc) = d.tag
                {
                    all.insert(sc.screen_id, sc);
                }
            }
            *self.screens.borrow_mut() = Some(all);
        }
        self.screens.borrow().as_ref()?.get(&id).cloned()
    }

    /// The image `a` names.
    pub fn art(&self, a: &Art) -> Option<Image> {
        match *a {
            Art::Solid { .. } => None,
            Art::Level { ref stem } => {
                let pictures = self.level_pictures.get_or_init(|| {
                    let template = crate::mods_screen::page_layout::PICTURE;
                    let unknown_map = self
                        .screen(template.screen_id as i16)
                        .and_then(|s| {
                            s.panes
                                .get(template.pane as usize)?
                                .bitmaps
                                .get(template.bitmap as usize)?
                                .bitmap
                                .id
                        })
                        .map(|id| id.0);
                    let mut map = self.map.borrow_mut();
                    super::level_art::pictures(
                        tags::ui_levels::read_mp(&mut *map).unwrap_or_default(),
                        tags::ui_levels::read_campaign(&mut *map).unwrap_or_default(),
                        unknown_map,
                    )
                });
                self.image(
                    tui::TagRef {
                        group: tui::BITMAP,
                        id: Some(TagId(*pictures.get(stem)?)),
                    },
                    0,
                )
            }
            Art::Skin { skin, index, image } => {
                let b = self.skins.get(&skin)?.bitmaps.get(usize::from(index))?;
                self.image(b.bitmap, image)
            }
            Art::Screen {
                screen_id,
                pane,
                index,
                image,
            } => {
                let sc = self.screen(screen_id)?;
                let b = sc
                    .panes
                    .get(usize::from(pane))?
                    .bitmaps
                    .get(usize::from(index))?;
                self.image(b.bitmap, image)
            }
        }
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

    /// What the game draws between a dialog and its opener, at rest: the
    /// whole screen filled with the shared globals' overlay colour
    /// (`Disc::dim`), then blurred as `0x235756` blurs it
    /// ([`backdrop::blur`]).
    pub fn backdrop(&self, c: &mut Canvas) {
        self.dim(c);
        backdrop::blur(c, backdrop::AT_REST);
    }

    /// The backdrop's fill: the whole screen in the shared globals'
    /// overlay colour ([`tui::SharedGlobals::overlay_color`], `wigl
    /// +0x6c`; on the disc 85% of (0, 20, 44)). The window stack's render
    /// (`0x234f8c`) draws it under a screen whose lower window holds one
    /// (`0x23029a`, fill `0x147cdb`), at its alpha times the screen's
    /// transition fraction: at rest, its own alpha.
    fn dim(&self, c: &mut Canvas) {
        let [a, r, g, b] = self.shared.overlay_color();
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        c.fill(WHOLE_SCREEN, [byte(r), byte(g), byte(b), byte(a)]);
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

    /// Screen `o` as it was when a modal opened over it, as a [`View`]:
    /// the game shell under it, its pane's bitmaps (lowest render depth
    /// first) and texts in its own strings, its header, and its list's
    /// rows ([`Opener::rows`]) on its skin with [`Opener::focus`] lit, as
    /// [`view_of`] draws a list. Blocks parked off screen are left out.
    pub fn opener_view(&self, o: &Opener) -> Result<View> {
        let parts = self.retail_parts(o.tag)?;
        let screen = &parts.screen;
        let pane = screen
            .panes
            .get(o.pane)
            .with_context(|| format!("{} has no pane {}", o.tag, o.pane))?;
        // The texts as the opener's sample state has them, as
        // `retail_view_edited` reads them: units substituted, hidden
        // blocks left out.
        let (table, name, named) = parts.strings();
        let state = retail_sample::State {
            sample: retail_sample::for_screen(o.tag).map_err(|e| anyhow!(e))?,
            pane: o.pane,
            strings: retail_sample::Strings {
                text: &table,
                name: &name,
                named: &named,
            },
        };
        let on_screen = |top: i16, left: i16| top.abs() < 1000 && left.abs() < 1000;
        let rgb = |v: [f32; 3]| v.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8);
        let mut w = Vec::new();
        let mut blocks: Vec<(usize, &tui::BitmapBlock)> = pane.bitmaps.iter().enumerate().collect();
        blocks.sort_by_key(|(_, b)| b.render_depth_bias);
        for (i, b) in blocks {
            let (x, y) = (b.top_left.x, b.top_left.y);
            let Some(img) = self.image(b.bitmap, b.image).filter(|_| on_screen(y, x)) else {
                continue;
            };
            let size = [img.width as u16, img.height as u16];
            w.push(Widget {
                id: format!("opener.art.{i}"),
                rect: [y, x, y - size[1] as i16, x + size[0] as i16],
                layer: Layer::Frame,
                alpha: 255,
                kind: Kind::Art {
                    art: Art::Screen {
                        screen_id: screen.screen_id,
                        pane: o.pane as u16,
                        index: i as u16,
                        image: b.image,
                    },
                    size,
                },
            });
        }
        let text = |id: String, r: Rect, font: i16, j: u8, text: String, colour| Widget {
            id,
            rect: [r.top, r.left, r.bottom, r.right],
            layer: Layer::Content,
            alpha: 255,
            kind: Kind::Text {
                text,
                font,
                justify: Justify::from_flag(j),
                colour,
            },
        };
        let layout = screen.header_layout();
        if screen.flags & tui::Screen::NO_HEADER == 0
            && let Some(h) = state.text("title", screen.header.0).filter(|s| !s.is_empty())
        {
            w.push(text(
                "opener.title".into(),
                self.shared.header_layouts[layout].header,
                self.shared.header_fonts[layout],
                0,
                h,
                rgb([
                    self.shared.header_color[1],
                    self.shared.header_color[2],
                    self.shared.header_color[3],
                ]),
            ));
        }
        for (i, t) in pane.text_blocks.iter().enumerate() {
            if let Some(s) = state
                .text(&format!("text.{i}"), t.string.0)
                .filter(|s| !s.is_empty())
                && on_screen(t.rect.top, t.rect.left)
            {
                w.push(text(
                    format!("opener.text.{i}"),
                    t.rect,
                    t.font,
                    tui::justification(t.flags),
                    s,
                    rgb(t.text_color),
                ));
            }
        }
        if let Some(l) = pane.lists.first() {
            let place = ListPlace::Moved {
                skin: l.skin,
                anchor: [l.anchor.x, l.anchor.y],
                bitmaps_left_of: i16::MIN,
                visible: o.rows.len() as i16,
            };
            let mut s = ScreenSpec::new(o.name, "");
            s.list = place;
            s.rows = o.rows.iter().map(|r| RowSpec::action(r)).collect();
            s.focus = o.focus;
            let rows = view_of(&s, &self.geometry(place)?, &self.fonts, "");
            w.extend(
                rows.widgets
                    .into_iter()
                    .filter(|r| r.id.starts_with("row."))
                    .map(|r| Widget {
                        id: format!("opener.{}", r.id),
                        ..r
                    }),
            );
        }
        Ok(View {
            screen: o.name.into(),
            state: o.state(),
            backing: Backing::GameShell,
            underlay: None,
            widgets: w,
            focus: None,
            list: None,
        })
    }

    /// [`Disc::opener_view`] and its picture, made once per opener. The
    /// picture is the opener drawn as a retail screen
    /// ([`Disc::render_retail_layer`], in its sample state: units
    /// substituted, texts wrapped) with [`Opener::rows`] on its first
    /// list; the view is kept for the over-opener notes.
    fn opener_drawn(&self, o: &Opener) -> Result<Drawn> {
        if let Some(d) = self.openers.borrow().get(o) {
            return d.clone().map_err(|e| anyhow!(e));
        }
        let d = self.opener_view(o).and_then(|v| {
            let r = Retail {
                tag: o.tag,
                pane: o.pane,
                text: &|_| None,
                cells: &[],
                footer: None,
                rows: Some((o.rows, o.focus)),
                sample: None,
            };
            let c = self.render_retail_layer(&r, &RetailEdits::default(), false)?;
            Ok(std::sync::Arc::new((v, c)))
        });
        let d = d.map_err(|e| format!("the opener {}: {e:#}", o.name));
        self.openers.borrow_mut().insert(*o, d.clone());
        d.map_err(|e| anyhow!(e))
    }

    /// Screen tag `tag` of `mainmenu.map`, decoded, with its string table
    /// and its tables' skins.
    fn retail_parts(&self, tag: &str) -> Result<RetailParts> {
        let mut m = self.map.borrow_mut();
        let id = m.tag(map::TagQuery::NameInGroup(tag, tui::SCREEN))?.id;
        let mut src = MapSource::new(&mut m);
        let UiTag::Screen(screen) = src.decode_tag(id)?.tag else {
            bail!("{tag} is not a screen");
        };
        let mut skins = HashMap::new();
        for t in screen.panes.iter().flat_map(|p| p.tables.iter()) {
            if let Some(id) = t.skin.id
                && let Ok((tree, _)) = tui::resolve_skin(&mut src, id)
            {
                skins.insert(id, tree.skin);
            }
        }
        drop(src);
        let loc = tags::localization::Localization::read(&mut m)?;
        let unic = match screen.strings.id {
            Some(u) => Some((loc.unic(&mut m, u)?, loc)),
            None => None,
        };
        let mut names = HashMap::new();
        let mut ids = HashMap::new();
        if let Some((u, loc)) = &unic {
            let english = tags::localization::Language::English;
            for e in loc.entries_for(u, english)? {
                if let Ok(Some(n)) = e.id.resolve(m.strings()) {
                    names.insert(e.id.0, n.to_owned());
                    ids.entry(n.to_owned()).or_insert(e.id.0);
                }
            }
        }
        // The blocks' own strings have names whether or not the table
        // has text for them (a status line's default).
        let block_strings = screen
            .panes
            .iter()
            .flat_map(|p| {
                p.text_blocks
                    .iter()
                    .map(|t| t.string)
                    .chain(p.buttons.iter().map(|b| b.string))
            })
            .chain([screen.header]);
        for id in block_strings {
            if let Ok(Some(n)) = id.resolve(m.strings()) {
                names.entry(id.0).or_insert_with(|| n.to_owned());
            }
        }
        let bitmaps = screen
            .panes
            .iter()
            .flat_map(|p| p.bitmaps.iter())
            .chain(skins.values().flat_map(|s| s.bitmaps.iter()))
            .filter_map(|b| b.bitmap.id)
            .filter_map(|id| Some((id, m.tag(map::TagQuery::Id(id)).ok()?.name.clone()?)))
            .collect();
        Ok(RetailParts {
            screen,
            unic,
            skins,
            names,
            ids,
            bitmaps,
        })
    }

    /// A retail screen in a state the game shows it in: pane `r.pane`'s
    /// bitmaps, buttons, lists, text blocks and tables' cells, over the
    /// game shell background, with the header and footer in the `wigl`
    /// header layout.
    ///
    /// The tag lays out every block the game might show, and the game's
    /// code picks among them, writes the empty ones and replaces the
    /// substitution units in the strings. The screen's sample
    /// ([`retail_sample`], `assets/ui/retail-samples/`) stands in for
    /// that code: which blocks show, what they read, the rows of a list
    /// and the cells of a table. Without one, a string holding a
    /// substitution unit is not drawn. `r.text` answers a string id ahead
    /// of the sample (`Some("")` leaves a block blank), `r.cells` and
    /// `r.footer` ahead of the sample's cells and footer.
    ///
    /// Texts are in the screen's own string table (`unic`, English),
    /// broken into lines and cut to their rectangles as the game does
    /// ([`Canvas::text_boxed`]). A cell with no entry is not drawn (the
    /// game hides empty player slots).
    pub fn render_retail(&self, r: &Retail<'_>) -> Result<Canvas> {
        self.render_retail_with(r, &RetailPlaces::new())
    }

    /// [`Disc::render_retail`] with the widgets in `places` moved there
    /// (the editor's edits of the tag, read back from its document).
    pub fn render_retail_with(&self, r: &Retail<'_>, places: &RetailPlaces) -> Result<Canvas> {
        self.render_retail_edited(r, &RetailEdits::placed(places))
    }

    /// [`Disc::render_retail`] as the editor's edits of the tag leave it
    /// (`retail_edit`): the edited pane's blocks in place of the disc's,
    /// the moved widgets where they are, and the stand-in text for a
    /// string ID the screen's table has none for.
    pub fn render_retail_edited(&self, r: &Retail<'_>, edits: &RetailEdits) -> Result<Canvas> {
        self.render_retail_layer(r, edits, true)
    }

    /// [`Disc::render_retail_edited`]; with `over`, a screen whose sample
    /// names the screen it opens over is drawn on that screen's picture
    /// in place of the game shell background.
    fn render_retail_layer(
        &self,
        r: &Retail<'_>,
        edits: &RetailEdits,
        over: bool,
    ) -> Result<Canvas> {
        let parts = self.retail_parts(r.tag)?;
        let mut pane = parts
            .screen
            .panes
            .get(r.pane)
            .with_context(|| format!("{} has no pane {}", r.tag, r.pane))?
            .clone();
        let header = edits.apply(&mut pane, parts.screen.header.0);
        let screen = &parts.screen;
        let skins = &parts.skins;
        let (table, name, named) = parts.strings();
        let state = retail_sample::State {
            sample: sample_of(r)?,
            pane: r.pane,
            strings: retail_sample::Strings {
                text: &table,
                name: &name,
                named: &named,
            },
        };
        let missing = state.missing();
        if !missing.is_empty() {
            bail!(
                "the sample for {} names strings its table lacks: {}",
                r.tag,
                missing.join(", ")
            );
        }
        let lookup = |widget: &str, id: u32| {
            edits
                .text(id)
                .or_else(|| (r.text)(id))
                .or_else(|| state.text(widget, id))
        };
        // The header and footer are in the screen's own header layout.
        let layout = screen.header_layout();
        let (hl, header_font) = (
            self.shared.header_layouts[layout],
            self.shared.header_fonts[layout],
        );
        let header_colour = [
            self.shared.header_color[1],
            self.shared.header_color[2],
            self.shared.header_color[3],
        ];
        let rgb = |v: [f32; 3]| v.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8);
        let under = state
            .sample
            .and_then(|s| s.over.as_deref())
            .filter(|_| over);
        let mut c = match under {
            // A dialog: the screen it opened over, in that screen's state,
            // under the window stack's backdrop ([`Disc::backdrop`]).
            Some(tag) => {
                let mut c = self.render_retail_layer(
                    &Retail {
                        tag,
                        pane: 0,
                        text: &|_| None,
                        cells: &[],
                        footer: None,
                        rows: None,
                        sample: None,
                    },
                    &RetailEdits::default(),
                    false,
                )?;
                self.backdrop(&mut c);
                c
            }
            None => {
                let mut c = Canvas::new(640, 480, [0, 0, 0, 255]);
                for p in &self.background.panes {
                    self.draw_blocks(&mut c, p.bitmaps.iter());
                }
                c
            }
        };
        // The pane's bitmaps the state shows, lowest render depth first.
        let mut blocks: Vec<(&tui::BitmapBlock, retail_sample::Shown<'_>)> = pane
            .bitmaps
            .iter()
            .enumerate()
            .filter(|(_, b)| on_screen_point(b.top_left))
            .filter_map(|(i, b)| {
                let tag = b.bitmap.id.and_then(|id| parts.bitmaps.get(&id));
                Some((b, state.bitmap(&format!("bitmap.{i}"), tag.map(String::as_str))?))
            })
            .collect();
        blocks.sort_by_key(|(b, _)| b.render_depth_bias);
        for (b, shown) in blocks {
            let img = self.shown_image(b, shown);
            if let (Some(stem), None) = (shown.level, &img) {
                bail!(
                    "the sample for {} names a map with no picture in the disc's level list: {stem}",
                    r.tag
                );
            }
            if let Some(img) = img {
                c.image(&img, i32::from(b.top_left.x), i32::from(b.top_left.y), 255);
            }
        }
        if screen.flags & tui::Screen::NO_HEADER == 0
            && let Some(h) = lookup("title", header)
        {
            c.text_boxed(
                &self.fonts,
                header_font,
                &h,
                hl.header,
                0,
                rgb(header_colour),
                255,
            );
        }
        if let Some(f) = r.footer.clone().or_else(|| state.footer()) {
            c.text(
                &self.fonts,
                header_font,
                &f,
                hl.secondary,
                1,
                rgb(header_colour),
            );
        }
        for (i, b) in pane.buttons.iter().enumerate() {
            let id = format!("button.{i}");
            if state.hidden(&id, b.string.0) {
                continue;
            }
            // The button draw (`0x253ea8`) adds the offset's x to the
            // rect's left and subtracts its y from the rect's top
            // (`0x253fb0`, `subss`): the lobby's (-12, -5) puts the art's
            // top 5 units above the label's.
            if let Some(img) = self.image(b.bitmap, 0) {
                c.image(
                    &img,
                    i32::from(b.rect.left) + i32::from(b.bitmap_offset.x),
                    i32::from(b.rect.top) - i32::from(b.bitmap_offset.y),
                    255,
                );
            }
            if let Some(t) = lookup(&id, b.string.0) {
                c.text_boxed(
                    &self.fonts,
                    b.font,
                    &t,
                    b.rect,
                    tui::justification(b.flags),
                    rgb(b.text_color),
                    255,
                );
            }
        }
        // A list's rows are the game's code's: the caller's for the
        // first list ([`Retail::rows`]), else the sample's, on the list's
        // skin from its anchor, as a screen of ours lays its rows.
        for (i, l) in pane.lists.iter().enumerate() {
            let given = r
                .rows
                .filter(|(rows, _)| i == 0 && !rows.is_empty())
                .map(|(rows, focus)| {
                    (rows.iter().map(|t| (*t).to_owned()).collect::<Vec<String>>(), focus)
                });
            let Some((rows, focus)) = given.or_else(|| state.rows(i)) else {
                continue;
            };
            let place = ListPlace::Moved {
                skin: l.skin,
                anchor: [l.anchor.x, l.anchor.y],
                bitmaps_left_of: i16::MIN,
                visible: l.visible_items.max(1),
            };
            let mut s = ScreenSpec::new(r.tag, "");
            s.list = place;
            s.rows = rows.iter().map(|t| RowSpec::action(t)).collect();
            s.focus = focus;
            let v = view_of(&s, &self.geometry(place)?, &self.fonts, "");
            paint(
                &mut c,
                v.widgets.iter().filter(|w| w.id.starts_with("row.")),
                self,
                false,
            );
        }
        for (i, t) in pane.text_blocks.iter().enumerate() {
            if !on_screen_rect(&t.rect) {
                continue;
            }
            if let Some(text) = lookup(&format!("text.{i}"), t.string.0).filter(|s| !s.is_empty()) {
                c.text_boxed(
                    &self.fonts,
                    t.font,
                    &text,
                    t.rect,
                    tui::justification(t.flags),
                    rgb(t.text_color),
                    255,
                );
            }
        }
        // The caller's cells are names alone, on the cell's lowest
        // bitmap; the sample's say what each of the skin's bitmaps shows.
        let named_cells: Vec<retail_sample::Cell> = r
            .cells
            .iter()
            .map(|text| retail_sample::Cell {
                text: text.clone(),
                ..Default::default()
            })
            .collect();
        let (cells, plain) = if named_cells.is_empty() {
            (state.cells(), false)
        } else {
            (named_cells.as_slice(), true)
        };
        for table in &pane.tables {
            let Some(skin) = table.skin.id.and_then(|id| skins.get(&id)) else {
                continue;
            };
            for (i, cell) in cells.iter().enumerate() {
                let Some(o) = table.cell_origin(i as i32) else {
                    continue;
                };
                let (ox, oy) = (i32::from(o.x), i32::from(o.y));
                let mut bitmaps: Vec<(usize, &tui::BitmapBlock)> =
                    skin.bitmaps.iter().enumerate().collect();
                bitmaps.sort_by_key(|(_, b)| b.render_depth_bias);
                if plain {
                    bitmaps.truncate(1);
                }
                for (k, b) in bitmaps {
                    let tag = b.bitmap.id.and_then(|id| parts.bitmaps.get(&id));
                    let shown = retail_sample::cell_bitmap(
                        cell,
                        &format!("bitmap.{k}"),
                        tag.map(String::as_str),
                    );
                    // An item's bitmap covers its point's y up to y plus
                    // its height (docs/formats/ui.md).
                    if let Some(img) = shown.and_then(|s| self.shown_image(b, s)) {
                        c.image(
                            &img,
                            ox + i32::from(b.top_left.x),
                            oy + i32::from(b.top_left.y) + img.height as i32,
                            255,
                        );
                    }
                }
                // The player's emblem, in each of the skin's shader
                // bitmaps. The game places one as it does an item's
                // bitmap: its rectangle moved up by its own height.
                if let Some(e) = cell.emblem.as_ref().filter(|_| !plain) {
                    let img = self
                        .emblem_image(e)
                        .with_context(|| format!("the sample for {}", r.tag))?;
                    for b in &skin.shader_bitmaps {
                        let h = (i32::from(b.rect.top) - i32::from(b.rect.bottom)).abs();
                        c.image(
                            &img,
                            ox + i32::from(b.rect.left),
                            oy + i32::from(b.rect.top) + h,
                            255,
                        );
                    }
                }
                if let Some(t) = skin.text_blocks.first() {
                    let at = Rect {
                        top: (i32::from(t.rect.top) + oy) as i16,
                        bottom: (i32::from(t.rect.bottom) + oy) as i16,
                        left: (i32::from(t.rect.left) + ox) as i16,
                        right: (i32::from(t.rect.right) + ox) as i16,
                    };
                    c.text_boxed(
                        &self.fonts,
                        t.font,
                        &cell.text,
                        at,
                        tui::justification(t.flags),
                        rgb(t.text_color),
                        255,
                    );
                }
            }
        }
        Ok(c)
    }

    /// Emblem `e` in the disc's small emblem art, coloured ([`emblem`]).
    fn emblem_image(&self, e: &retail_sample::Emblem) -> Result<Image> {
        let layer = |tag: &str, index: i16| -> Result<Image> {
            let id = self
                .map
                .borrow_mut()
                .tag(map::TagQuery::NameInGroup(tag, tui::BITMAP))?
                .id;
            let r = tui::TagRef {
                group: tui::BITMAP,
                id: Some(id),
            };
            self.image(r, index)
                .with_context(|| format!("{tag} has no image {index}"))
        };
        let bg = layer(EMBLEM_BACKGROUND, e.background)?;
        let fg = layer(EMBLEM_FOREGROUND, e.foreground)?;
        emblem(&bg, &fg, e.colours).context("the emblem's two layers differ in size")
    }

    /// Bitmap block `b`'s picture as a state shows it: the map picture
    /// or the image the state picks (else the block's own), in the
    /// state's colour.
    fn shown_image(&self, b: &tui::BitmapBlock, shown: retail_sample::Shown<'_>) -> Option<Image> {
        let img = match shown.level {
            Some(stem) => self.art(&Art::Level {
                stem: stem.to_owned(),
            })?,
            None => self.image(b.bitmap, shown.image.unwrap_or(b.image))?,
        };
        Some(match shown.tint {
            Some(t) => tinted(&img, t),
            None => img,
        })
    }

    /// A retail screen's widgets as a [`View`], for the editor to select
    /// and move (docs/game/ui-builder.md "Editing in the editor"), with
    /// `places` applied: `title` (the header), then pane `r.pane`'s
    /// `bitmap.N`, `button.N`, `text.N`, `list.N` and `table.N` by their
    /// index in the tag's block, on screen only. A list's box is its
    /// anchor and its visible rows at the Controls skin's item height, a
    /// table's its cells at their spacing: a handle to move, not a
    /// measured extent.
    pub fn retail_view(&self, r: &Retail<'_>, places: &RetailPlaces) -> Result<View> {
        self.retail_view_edited(r, &RetailEdits::placed(places))
    }

    /// [`Disc::retail_view`] as the editor's edits of the tag leave it
    /// ([`Disc::render_retail_edited`]): a block the tag gained is a
    /// widget by its index like any other, and a parked one is absent.
    pub fn retail_view_edited(&self, r: &Retail<'_>, edits: &RetailEdits) -> Result<View> {
        let parts = self.retail_parts(r.tag)?;
        let mut pane = parts
            .screen
            .panes
            .get(r.pane)
            .with_context(|| format!("{} has no pane {}", r.tag, r.pane))?
            .clone();
        let header = edits.apply(&mut pane, parts.screen.header.0);
        let (table, name, named) = parts.strings();
        let state = retail_sample::State {
            sample: sample_of(r)?,
            pane: r.pane,
            strings: retail_sample::Strings {
                text: &table,
                name: &name,
                named: &named,
            },
        };
        let lookup = |widget: &str, id: u32| {
            edits
                .text(id)
                .or_else(|| (r.text)(id))
                .or_else(|| state.text(widget, id))
        };
        let screen = &parts.screen;
        let g = self.geometry(ListPlace::Controls)?;
        let rgb = |v: [f32; 3]| v.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8);
        let edges = |r: Rect| [r.top, r.left, r.bottom, r.right];
        let flag = |flags: u32| Justify::from_flag(tui::justification(flags));
        let text = |id: String, rect: [i16; 4], font: i16, justify, s: String, colour| Widget {
            id,
            rect,
            layer: Layer::Content,
            alpha: 255,
            kind: Kind::Text {
                text: s,
                font,
                justify,
                colour,
            },
        };
        let mut w = Vec::new();
        for (i, b) in pane.bitmaps.iter().enumerate() {
            if !on_screen_point(b.top_left) {
                continue;
            }
            let size = self
                .image(b.bitmap, b.image)
                .map_or([16, 16], |img| [img.width as u16, img.height as u16]);
            let (x, y) = (b.top_left.x, b.top_left.y);
            w.push(Widget {
                id: format!("bitmap.{i}"),
                rect: [y, x, y - size[1] as i16, x + size[0] as i16],
                layer: Layer::Frame,
                alpha: 255,
                kind: Kind::Art {
                    art: Art::Screen {
                        screen_id: screen.screen_id,
                        pane: r.pane as u16,
                        index: i as u16,
                        image: b.image,
                    },
                    size,
                },
            });
        }
        if screen.flags & tui::Screen::NO_HEADER == 0
            && let Some(h) = lookup("title", header)
        {
            let layout = screen.header_layout();
            w.push(text(
                "title".into(),
                edges(self.shared.header_layouts[layout].header),
                self.shared.header_fonts[layout],
                Justify::Left,
                h,
                rgb(g.header_colour),
            ));
        }
        for (i, b) in pane.buttons.iter().enumerate() {
            let s = lookup(&format!("button.{i}"), b.string.0).unwrap_or_default();
            w.push(text(
                format!("button.{i}"),
                edges(b.rect),
                b.font,
                flag(b.flags),
                s,
                rgb(b.text_color),
            ));
        }
        for (i, t) in pane.text_blocks.iter().enumerate() {
            if !on_screen_rect(&t.rect) {
                continue;
            }
            let s = lookup(&format!("text.{i}"), t.string.0).unwrap_or_default();
            w.push(text(
                format!("text.{i}"),
                edges(t.rect),
                t.font,
                flag(t.flags),
                s,
                rgb(t.text_color),
            ));
        }
        let handle = |id: String, at: tui::Point, size: [i32; 2]| Widget {
            id,
            rect: [
                at.y,
                at.x,
                (i32::from(at.y) - size[1].max(8)) as i16,
                (i32::from(at.x) + size[0].max(8)) as i16,
            ],
            layer: Layer::Under,
            alpha: 255,
            kind: Kind::Art {
                art: Art::Solid { rgba: [0, 0, 0, 0] },
                size: [size[0].max(8) as u16, size[1].max(8) as u16],
            },
        };
        for (i, l) in pane.lists.iter().enumerate() {
            let rows = i32::from(l.visible_items.max(1));
            w.push(handle(
                format!("list.{i}"),
                l.anchor,
                [i32::from(g.label.0.right - g.label.0.left), rows * g.item_height],
            ));
        }
        for (i, t) in pane.tables.iter().enumerate() {
            let size = [
                i32::from(t.columns.max(1)) * i32::from(t.column_spacing).abs(),
                i32::from(t.rows.max(1)) * i32::from(t.row_spacing).abs(),
            ];
            w.push(handle(format!("table.{i}"), t.anchor, size));
        }
        Ok(View {
            screen: r.tag.to_owned(),
            state: format!("pane-{}", r.pane),
            backing: Backing::GameShell,
            underlay: None,
            widgets: w,
            focus: None,
            list: None,
        })
    }

    /// `s` as the builder sees it ([`view_of`]) on this disc's geometry
    /// and fonts.
    pub fn view(&self, s: &ScreenSpec, state: &str) -> Result<View> {
        From::Disc(self).view(s, state)
    }

    /// `s` drawn at 640 by 480; texts that overflow outlined in red.
    pub fn render(&self, s: &ScreenSpec) -> Result<Canvas> {
        Ok(render_view(&self.view(s, "sample")?, self))
    }
}

/// A retail screen to draw ([`Disc::render_retail`]).
pub struct Retail<'a> {
    /// The screen tag's name in `mainmenu.map`.
    pub tag: &'a str,
    pub pane: usize,
    /// The text for a string id, ahead of the screen's sample
    /// ([`retail_sample`]); None takes the sample's, else the tag's own.
    pub text: &'a dyn Fn(u32) -> Option<String>,
    /// The table cells' texts (player names), one per filled slot, each
    /// on the cell skin's lowest bitmap; empty takes the sample's cells.
    pub cells: &'a [String],
    /// The footer's legend; None takes the sample's.
    pub footer: Option<String>,
    /// The first list's rows and its focused row, ahead of the sample's
    /// (an [`Opener`]'s); None, or no rows, takes the sample's.
    pub rows: Option<(&'a [&'a str], usize)>,
    /// The sample's variant by name (`variant:` in its file,
    /// [`retail_sample::for_screen_variant`]); None takes the screen's
    /// plain sample. A name the screen has no sample for fails the draw.
    pub sample: Option<&'a str>,
}

/// The sample `r` is drawn in: its variant, or the screen's plain one.
fn sample_of(r: &Retail<'_>) -> Result<Option<&'static retail_sample::RetailSample>> {
    let Some(name) = r.sample else {
        return retail_sample::for_screen(r.tag).map_err(|e| anyhow!(e));
    };
    match retail_sample::for_screen_variant(r.tag, name).map_err(|e| anyhow!(e))? {
        Some(s) => Ok(Some(s)),
        None => {
            let names = retail_sample::variants(r.tag).map_err(|e| anyhow!(e))?;
            bail!(
                "{} has no sample named {name:?} (its samples: {})",
                r.tag,
                if names.is_empty() { "none".to_owned() } else { names.join(", ") }
            )
        }
    }
}

/// A decoded retail screen: its tag, its string table and its tables'
/// skins ([`Disc::render_retail`], [`Disc::retail_view`]).
struct RetailParts {
    screen: tui::Screen,
    unic: Option<(tags::localization::Unic, tags::localization::Localization)>,
    skins: HashMap<TagId, tui::Skin>,
    /// The string table's names: id to name, and name to id.
    names: HashMap<u32, String>,
    ids: HashMap<String, u32>,
    /// The tag paths of the bitmaps the panes and the skins draw.
    bitmaps: HashMap<TagId, String>,
}

impl RetailParts {
    /// The screen's string table as a state reads it
    /// ([`retail_sample::Strings`]): a string id's text, its name, and
    /// the text of the string named so.
    #[allow(clippy::type_complexity)]
    fn strings(
        &self,
    ) -> (
        impl Fn(u32) -> Option<String> + '_,
        impl Fn(u32) -> Option<String> + '_,
        impl Fn(&str) -> Option<String> + '_,
    ) {
        (
            move |id: u32| self.text(id),
            move |id: u32| self.names.get(&id).cloned(),
            move |name: &str| self.ids.get(name).and_then(|id| self.text(*id)),
        )
    }

    /// String id `id`'s text in the screen's own table (English).
    fn text(&self, id: u32) -> Option<String> {
        let (unic, loc) = self.unic.as_ref()?;
        loc.text_in(
            unic,
            tags::localization::StringId(id),
            tags::localization::Language::English,
        )
        .ok()
        .flatten()
    }
}

/// The 640 by 480 screen in UI units.
const WHOLE_SCREEN: Rect = Rect {
    top: 240,
    left: -320,
    bottom: -240,
    right: 320,
};

/// The backdrop's fill under a dialog without a disc: the disc's overlay
/// colour (`Disc::dim`), 85% of (0, 20, 44).
pub const SYNTHETIC_DIM: [u8; 4] = [0, 20, 44, 217];

/// Blocks parked off screen (9999) are the game's spares.
fn on_screen_rect(r: &Rect) -> bool {
    r.top.abs() < 1000 && r.left.abs() < 1000
}

fn on_screen_point(p: tui::Point) -> bool {
    p.x.abs() < 1000 && p.y.abs() < 1000
}

/// Retail widgets moved by the editor: widget id ([`Disc::retail_view`]'s)
/// to its rect, `top, left, bottom, right` in UI units. A bitmap, list
/// or table takes the rect's top-left as its point.
pub type RetailPlaces = BTreeMap<String, [i16; 4]>;

/// Where a retail widget's place is in its screen tag: a field path of
/// the `wgit` definition (`crates/formats/tags/src/defs/wgit.txt`) and
/// whether it is a rectangle or a point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetailField {
    /// `i16[4]`: top, left, bottom, right.
    Rect(String),
    /// `i16[2]`: x, y (the top-left).
    Point(String),
}

impl RetailField {
    pub fn path(&self) -> &str {
        match self {
            RetailField::Rect(p) | RetailField::Point(p) => p,
        }
    }
    /// The field's value for rect `r`, as the tag editor's JSON takes it.
    pub fn value(&self, r: [i16; 4]) -> serde_json::Value {
        match self {
            RetailField::Rect(_) => serde_json::json!(r),
            RetailField::Point(_) => serde_json::json!([r[1], r[0]]),
        }
    }
}

/// Widget `id` of pane `pane`'s field, or why it has none: the header
/// is the shared `wigl` layout's, and anything else is not a block.
pub fn retail_field(pane: usize, id: &str) -> Result<RetailField, String> {
    let (kind, index) = id
        .split_once('.')
        .and_then(|(k, i)| Some((k, i.parse::<usize>().ok()?)))
        .ok_or_else(|| match id {
            "title" => "refused: a retail screen's title sits in the header layout of the \
                        shared wigl tag, which every screen on that layout uses"
                .to_owned(),
            _ => format!("refused: {id} is not a block of the screen tag"),
        })?;
    let at = |block: &str, field: &str| format!("panes[{pane}].{block}[{index}].{field}");
    match kind {
        "text" => Ok(RetailField::Rect(at("text_blocks", "rect"))),
        "button" => Ok(RetailField::Rect(at("buttons", "rect"))),
        "bitmap" => Ok(RetailField::Point(at("bitmaps", "top_left"))),
        "list" => Ok(RetailField::Point(at("lists", "anchor"))),
        "table" => Ok(RetailField::Point(at("tables", "anchor"))),
        _ => Err(format!("refused: {id} is not a block of the screen tag")),
    }
}

/// `pane` with the widgets of `places` moved there.
fn place_retail(pane: &mut tui::Pane, places: &RetailPlaces) {
    let point = |r: &[i16; 4]| tui::Point { x: r[1], y: r[0] };
    let rect = |r: &[i16; 4]| Rect {
        top: r[0],
        left: r[1],
        bottom: r[2],
        right: r[3],
    };
    for (id, r) in places {
        let Some((kind, index)) = id
            .split_once('.')
            .and_then(|(k, i)| Some((k, i.parse::<usize>().ok()?)))
        else {
            continue;
        };
        match kind {
            "text" => {
                if let Some(t) = pane.text_blocks.get_mut(index) {
                    t.rect = rect(r);
                }
            }
            "button" => {
                if let Some(b) = pane.buttons.get_mut(index) {
                    b.rect = rect(r);
                }
            }
            "bitmap" => {
                if let Some(b) = pane.bitmaps.get_mut(index) {
                    b.top_left = point(r);
                }
            }
            "list" => {
                if let Some(l) = pane.lists.get_mut(index) {
                    l.anchor = point(r);
                }
            }
            "table" => {
                if let Some(t) = pane.tables.get_mut(index) {
                    t.anchor = point(r);
                }
            }
            _ => {}
        }
    }
}

/// A filmstrip: each frame at half size side by side, its label (a time)
/// above it in [`Canvas::label`]'s font. Returns the strip and each
/// frame's left edge and width in its pixels.
pub fn filmstrip(frames: &[(String, Canvas)]) -> (Canvas, Vec<[u32; 2]>) {
    const GAP: u32 = 8;
    const LABEL: u32 = 22;
    let (tw, th) = (320, 240);
    let n = frames.len() as u32;
    let mut out = Canvas::new(
        n * (tw + GAP) + GAP,
        th + LABEL + 2 * GAP,
        [40, 40, 40, 255],
    );
    let mut at = Vec::new();
    for (i, (label, c)) in frames.iter().enumerate() {
        let x = GAP + i as u32 * (tw + GAP);
        out.label(label, x + 2, GAP + 6, [230, 230, 230]);
        out.blit(&c.half(), x, GAP + LABEL);
        at.push([x, tw]);
    }
    (out, at)
}

impl Metrics for Fonts {
    fn width(&self, font: i16, text: &str) -> i32 {
        Fonts::width(self, font, text)
    }
    fn line_height(&self, font: i16) -> i32 {
        Fonts::line_height(self, font)
    }
    fn has_glyph(&self, font: i16, ch: char) -> bool {
        self.has(font, ch)
    }
}

impl Metrics for Disc {
    fn width(&self, font: i16, text: &str) -> i32 {
        self.fonts.width(font, text)
    }
    fn line_height(&self, font: i16) -> i32 {
        self.fonts.line_height(font)
    }
    fn has_glyph(&self, font: i16, ch: char) -> bool {
        self.fonts.has(font, ch)
    }
}

/// What a [`View`] is drawn from: the disc ([`Disc`]) or nothing
/// ([`SyntheticSource`]).
pub trait Source: Metrics {
    /// The image art names; `None` draws a placeholder box.
    fn art(&self, a: &Art) -> Option<Image>;
    /// Whether the source can resolve an image, or None when it has no art.
    fn art_available(&self, _art: &Art) -> Option<bool> {
        None
    }
    fn backing(&self, c: &mut Canvas, b: Backing);
    /// What the game draws over the screen under a dialog, before the
    /// dialog: the overlay colour's fill, then the blur
    /// ([`Disc::backdrop`]). [`Backing::Over`] draws it over the opener,
    /// and a view drawn over a screen of ours (its `underlay`) over that.
    fn backdrop(&self, c: &mut Canvas);
    #[allow(clippy::too_many_arguments)]
    fn text(
        &self,
        c: &mut Canvas,
        font: i16,
        text: &str,
        r: Rect,
        j: Justify,
        rgb: [u8; 3],
        alpha: u8,
    );
    /// The list of retail screen `screen_id`'s pane `pane`: `Some(None)`
    /// when it has none, `None` when the source doesn't know the screen.
    fn list_of(&self, screen_id: i16, pane: u16) -> Option<Option<ListAt>>;
    /// The screen a modal opened over, as drawn under it; `None` when the
    /// source can't make it.
    fn opener(&self, _o: &Opener) -> Option<View> {
        None
    }
}

fn justify_flag(j: Justify) -> u8 {
    match j {
        Justify::Left => 0,
        Justify::Right => 1,
        Justify::Centre => 2,
    }
}

impl Source for Disc {
    fn art(&self, a: &Art) -> Option<Image> {
        Disc::art(self, a)
    }
    fn art_available(&self, art: &Art) -> Option<bool> {
        Some(self.art(art).is_some())
    }
    fn list_of(&self, screen_id: i16, pane: u16) -> Option<Option<ListAt>> {
        let s = self.screen(screen_id)?;
        let p = s.panes.get(usize::from(pane))?;
        Some(p.lists.first().map(|l| ListAt {
            anchor: [l.anchor.x, l.anchor.y],
            skin: l.skin,
        }))
    }
    fn opener(&self, o: &Opener) -> Option<View> {
        self.opener_drawn(o).ok().map(|d| d.0.clone())
    }
    fn backdrop(&self, c: &mut Canvas) {
        Disc::backdrop(self, c);
    }
    fn backing(&self, c: &mut Canvas, b: Backing) {
        match b {
            Backing::None => {}
            Backing::GameShell => {
                for p in &self.background.panes {
                    self.draw_blocks(c, p.bitmaps.iter());
                }
            }
            Backing::Over(o) => {
                if let Ok(d) = self.opener_drawn(&o) {
                    c.blit(&d.1, 0, 0);
                }
                Source::backdrop(self, c);
            }
        }
    }
    fn text(
        &self,
        c: &mut Canvas,
        font: i16,
        text: &str,
        r: Rect,
        j: Justify,
        rgb: [u8; 3],
        alpha: u8,
    ) {
        c.text_alpha(&self.fonts, font, text, r, justify_flag(j), rgb, alpha);
    }
}

/// A source without a disc: art as outlined boxes of its declared size,
/// text as blocks on [`Synthetic`]'s 6 by 16 grid, button glyphs as green
/// squares. Layout only: what CI can draw.
#[derive(Clone, Copy, Debug, Default)]
pub struct SyntheticSource;

impl Metrics for SyntheticSource {
    fn width(&self, font: i16, text: &str) -> i32 {
        Synthetic.width(font, text)
    }
    fn line_height(&self, font: i16) -> i32 {
        Synthetic.line_height(font)
    }
}

impl Source for SyntheticSource {
    fn art(&self, _a: &Art) -> Option<Image> {
        None
    }
    /// The lists of the retail screens whose art our screens borrow, as
    /// `h2 ui` reads them (the disc test holds these to the disc): the
    /// Controls screen (39) and the level select (11).
    fn list_of(&self, screen_id: i16, pane: u16) -> Option<Option<ListAt>> {
        match (screen_id, pane) {
            (crate::ui_kit::view::CONTROLS_SCREEN_ID, 0) => Some(Some(ListAt {
                anchor: [-254, 130],
                skin: 2,
            })),
            (crate::skulls_menu::LEVEL_SELECT_SCREEN_ID, 0) => Some(Some(ListAt {
                anchor: [-215, 130],
                skin: 0,
            })),
            _ => None,
        }
    }
    /// The disc's backdrop with the overlay colour fixed
    /// ([`SYNTHETIC_DIM`]).
    fn backdrop(&self, c: &mut Canvas) {
        c.fill(WHOLE_SCREEN, SYNTHETIC_DIM);
        backdrop::blur(c, backdrop::AT_REST);
    }
    /// The opener's rows as a list on the left, its focused row lit.
    fn opener(&self, o: &Opener) -> Option<View> {
        let widgets = o
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let top = 120 - i as i16 * 30;
                Widget {
                    id: format!("opener.row.{i}.label"),
                    rect: [top - 3, -240, top - 19, -50],
                    layer: Layer::Content,
                    alpha: if i == o.focus { 255 } else { UNFOCUSED_ALPHA },
                    kind: Kind::Text {
                        text: (*row).into(),
                        font: 1,
                        justify: Justify::Left,
                        colour: ROW_TEXT,
                    },
                }
            })
            .collect();
        Some(View {
            screen: o.name.into(),
            state: o.state(),
            backing: Backing::GameShell,
            underlay: None,
            widgets,
            focus: None,
            list: None,
        })
    }
    fn backing(&self, c: &mut Canvas, b: Backing) {
        if let Backing::Over(o) = b {
            // The game shell, and the opener's list as boxes on its left.
            self.backing(c, Backing::GameShell);
            for i in 0..o.rows.len() {
                let top = 120 - i as i32 * 30;
                let lit = if i == o.focus { 255 } else { 140 };
                c.fill(Rect::from_xywh(-250, top, 200, 22), [40, 60, 96, lit]);
            }
            if let Some(v) = self.opener(&o) {
                for w in &v.widgets {
                    if let Kind::Text {
                        text,
                        font,
                        justify,
                        colour,
                    } = &w.kind
                    {
                        self.text(c, *font, text, w.rect(), *justify, *colour, w.alpha);
                    }
                }
            }
            self.backdrop(c);
            return;
        }
        if b == Backing::GameShell {
            let band = [24, 44, 72, 255];
            c.fill(
                Rect {
                    top: 240,
                    left: -320,
                    bottom: 172,
                    right: 320,
                },
                band,
            );
            c.fill(
                Rect {
                    top: -172,
                    left: -320,
                    bottom: -240,
                    right: 320,
                },
                band,
            );
            c.fill(
                Rect {
                    top: 172,
                    left: -320,
                    bottom: -172,
                    right: 320,
                },
                [14, 24, 40, 255],
            );
        }
    }
    fn text(
        &self,
        c: &mut Canvas,
        font: i16,
        text: &str,
        r: Rect,
        j: Justify,
        rgb: [u8; 3],
        alpha: u8,
    ) {
        for (n, line) in text.split('\n').enumerate() {
            let w = Synthetic.width(font, line);
            let mut x = i32::from(r.left)
                + match j {
                    Justify::Left => 0,
                    Justify::Right => r.width() - w,
                    Justify::Centre => (r.width() - w) / 2,
                };
            let top = i32::from(r.top) - n as i32 * Synthetic::LINE;
            for ch in line.chars() {
                if ('\u{e000}'..='\u{e3ff}').contains(&ch) {
                    c.fill(
                        Rect::from_xywh(x + 1, top - 1, 14, 14),
                        [60, 170, 70, alpha],
                    );
                    x += Synthetic::GLYPH;
                } else {
                    if !ch.is_whitespace() {
                        c.fill(
                            Rect::from_xywh(x + 1, top - 4, 6, 10),
                            [rgb[0], rgb[1], rgb[2], alpha],
                        );
                    }
                    x += Synthetic::ADVANCE;
                }
            }
        }
    }
}

trait FromXywh {
    fn from_xywh(x: i32, top: i32, w: i32, h: i32) -> Rect;
}
impl FromXywh for Rect {
    /// `x`, `top` the top-left (Y up), `w` by `h`.
    fn from_xywh(x: i32, top: i32, w: i32, h: i32) -> Rect {
        Rect {
            top: top as i16,
            left: x as i16,
            bottom: (top - h) as i16,
            right: (x + w) as i16,
        }
    }
}

/// Whether text widget `w`'s text overflows its rect in `m`: a line wider
/// than it, or more lines than its height holds.
pub fn overflows(w: &crate::ui_kit::view::Widget, m: &dyn Metrics) -> bool {
    let Kind::Text { text, font, .. } = &w.kind else {
        return false;
    };
    let r = w.rect();
    let lines = text.split('\n').count() as i32;
    !text.is_empty() && (m.width(*font, text) > r.width() || lines * m.line_height(*font) > r.height())
}

/// `v` drawn at 640 by 480 from `src`: the backing, then its widgets by
/// layer (frame art, row and cell art, texts), each with its alpha. Texts
/// that overflow are outlined in red; art the source lacks is an outlined
/// box.
pub fn render_view(v: &View, src: &dyn Source) -> Canvas {
    render_view_where(v, src, &|_| true, true)
}

/// [`render_view`] of the widgets `keep` keeps, with the overflow outlines
/// when `marks` (the contrast check draws what is under the texts).
pub fn render_view_where(
    v: &View,
    src: &dyn Source,
    keep: &dyn Fn(&crate::ui_kit::view::Widget) -> bool,
    marks: bool,
) -> Canvas {
    render_composite(v, src, keep, marks, [0, 0, 0, 255])
}

/// Draw a live kit page into a straight-alpha surface for the shell.
pub fn render_overlay(v: &View, src: &dyn Source) -> Canvas {
    render_composite(v, src, &|_| true, false, [0; 4])
}

fn render_composite(
    v: &View,
    src: &dyn Source,
    keep: &dyn Fn(&crate::ui_kit::view::Widget) -> bool,
    marks: bool,
    clear: [u8; 4],
) -> Canvas {
    let mut c = if let Some(parent) = &v.underlay {
        // Widget filters apply to this screen. The parent stays painted
        // when an audit samples the pixels underneath the dialog's text,
        // under the backdrop a dialog draws over what it opened on.
        let mut c = render_composite(parent, src, &|_| true, false, clear);
        src.backdrop(&mut c);
        c
    } else {
        let mut c = Canvas::new(640, 480, clear);
        src.backing(&mut c, v.backing);
        c
    };
    paint(&mut c, v.widgets.iter().filter(|w| keep(w)), src, marks);
    c
}

/// `widgets` drawn on `c` from `src` by layer (frame art, row and cell
/// art, texts), each with its alpha; with `marks`, texts that overflow
/// are outlined in red.
fn paint<'a>(
    c: &mut Canvas,
    widgets: impl Iterator<Item = &'a crate::ui_kit::view::Widget>,
    src: &dyn Source,
    marks: bool,
) {
    let mut order: Vec<&crate::ui_kit::view::Widget> = widgets.collect();
    order.sort_by_key(|w| w.layer);
    for w in order.into_iter().filter(|w| w.alpha > 0) {
        let r = w.rect();
        match &w.kind {
            Kind::Art { art: Art::Solid { rgba }, .. } => {
                let [red, green, blue, alpha] = *rgba;
                c.fill(r, [red, green, blue, (u16::from(alpha) * u16::from(w.alpha) / 255) as u8]);
            }
            Kind::Art { art, .. } => match src.art(art) {
                Some(img) => c.image(&img, i32::from(r.left), i32::from(r.top), w.alpha),
                None => {
                    // Over the synthetic backing this is about the
                    // Controls row bar's mean colour in game (27, 37, 56).
                    c.fill(r, [50, 70, 110, w.alpha / 4]);
                    c.outline(r, [120, 160, 220, w.alpha]);
                }
            },
            Kind::Text {
                text,
                font,
                justify,
                colour,
            } => {
                src.text(c, *font, text, r, *justify, *colour, w.alpha);
                if marks && overflows(w, src) {
                    c.outline(r, [230, 40, 40, 255]);
                }
            }
        }
    }
}

/// A contact sheet: each tile at half size under its label (in
/// [`Canvas::label`]'s pixel font, whatever the source), `columns` across.
pub fn sheet(tiles: &[(String, Canvas)], columns: usize) -> Canvas {
    const GAP: u32 = 8;
    const LABEL: u32 = 22;
    let (tw, th) = (320, 240);
    let columns = columns.max(1);
    let rows = tiles.len().div_ceil(columns) as u32;
    let mut out = Canvas::new(
        columns as u32 * (tw + GAP) + GAP,
        rows * (th + LABEL + GAP) + GAP,
        [40, 40, 40, 255],
    );
    for (i, (label, c)) in tiles.iter().enumerate() {
        let (x, y) = (
            GAP + (i % columns) as u32 * (tw + GAP),
            GAP + (i / columns) as u32 * (th + LABEL + GAP),
        );
        out.label(label, x + 2, y + 6, [230, 230, 230]);
        out.blit(&c.half(), x, y + LABEL);
    }
    out
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
        let retained = u32::from(self.rgba[i + 3]) * (255 - a);
        let alpha = a * 255 + retained;
        if alpha == 0 {
            return;
        }
        for (d, s) in self.rgba[i..i + 3].iter_mut().zip(c) {
            *d = ((u32::from(s) * a * 255 + u32::from(*d) * retained) / alpha) as u8;
        }
        self.rgba[i + 3] = ((alpha + 127) / 255) as u8;
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
        self.text_alpha(fonts, index, text, r, just, rgb, 255);
    }
    /// [`Canvas::text`] at `alpha`.
    #[allow(clippy::too_many_arguments)]
    pub fn text_alpha(
        &mut self,
        fonts: &Fonts,
        index: i16,
        text: &str,
        r: Rect,
        just: u8,
        rgb: [u8; 3],
        alpha: u8,
    ) {
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
            self.line(fonts, &font, index, line, x, top + n as i32 * line_h, rgb, alpha);
        }
    }
    /// `text` as the game draws a text widget in `r` (docs/formats/ui.md
    /// "Text layout"): broken into lines at the rectangle's width
    /// ([`Fonts::wrap`]), each justified in it, and only the lines whose
    /// baseline is above its bottom ([`Fonts::lines_drawn`]).
    #[allow(clippy::too_many_arguments)]
    pub fn text_boxed(
        &mut self,
        fonts: &Fonts,
        index: i16,
        text: &str,
        r: Rect,
        just: u8,
        rgb: [u8; 3],
        alpha: u8,
    ) {
        let Some(font) = fonts.font(index) else {
            return;
        };
        let line_h = font.line_height();
        let (left, top) = self.px(i32::from(r.left), i32::from(r.top));
        let lines = fonts.wrap(index, text, r.width());
        let shown = fonts.lines_drawn(index, r.height());
        for (n, l) in lines.iter().take(shown).enumerate() {
            let x = match just {
                1 => left + r.width() - l.width,
                2 => left + (r.width() - l.width) / 2,
                _ => left,
            };
            self.line(fonts, &font, index, &l.text, x, top + n as i32 * line_h, rgb, alpha);
        }
    }
    /// One line of text with its top-left at pixel `(x, y)`: runs of
    /// plain text, shadowed, and of icons, each drawn on its own.
    #[allow(clippy::too_many_arguments)]
    fn line(
        &mut self,
        fonts: &Fonts,
        font: &font::Font<'_>,
        index: i16,
        line: &str,
        x: i32,
        y: i32,
        rgb: [u8; 3],
        alpha: u8,
    ) {
        let shadow = (160 * u32::from(alpha) / 255) as u8;
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
                self.run(font, &units, pen + 1, y + 1, [0, 0, 0, shadow], false);
            }
            self.run(font, &units, pen, y, [rgb[0], rgb[1], rgb[2], alpha], icon);
            pen += fonts.width(index, &s);
        }
    }
    /// `other` copied in with its top-left at pixel `(x, y)`.
    pub fn blit(&mut self, other: &Canvas, x: u32, y: u32) {
        for row in 0..other.height.min(self.height.saturating_sub(y)) {
            let w = other.width.min(self.width.saturating_sub(x)) as usize * 4;
            let i = (row * other.width * 4) as usize;
            let o = (((y + row) * self.width + x) * 4) as usize;
            self.rgba[o..o + w].copy_from_slice(&other.rgba[i..i + w]);
        }
    }
    /// `text` in a 3 by 5 pixel font at twice its size, its top-left at
    /// pixel `(x, y)`: letters (either case), digits and `-./_`; anything
    /// else is a gap. For sheet labels, readable with no disc.
    pub fn label(&mut self, text: &str, x: u32, y: u32, rgb: [u8; 3]) {
        for (n, ch) in text.chars().enumerate() {
            let rows = tiny_glyph(ch.to_ascii_lowercase());
            for (r, bits) in rows.iter().enumerate() {
                for col in 0..3 {
                    if bits & (0b100 >> col) != 0 {
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            self.blend(
                                (x + n as u32 * 8 + col * 2 + dx) as i32,
                                (y + r as u32 * 2 + dy) as i32,
                                [rgb[0], rgb[1], rgb[2], 255],
                            );
                        }
                    }
                }
            }
        }
    }
    /// Half size, each pixel the mean of four.
    pub fn half(&self) -> Canvas {
        let (w, h) = (self.width / 2, self.height / 2);
        let mut out = Canvas::new(w, h, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 0..w {
                for ch in 0..4 {
                    let at = |dx: u32, dy: u32| {
                        u32::from(
                            self.rgba[(((2 * y + dy) * self.width + 2 * x + dx) * 4 + ch) as usize],
                        )
                    };
                    out.rgba[((y * w + x) * 4 + ch) as usize] =
                        ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) / 4) as u8;
                }
            }
        }
        out
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

/// What a preview is drawn from: the disc, or the synthetic source and
/// [`Geometry::fallback`].
pub enum From<'a> {
    Disc(&'a Disc),
    Synthetic,
}

impl From<'_> {
    pub fn source(&self) -> &dyn Source {
        match self {
            From::Disc(d) => *d,
            From::Synthetic => &SyntheticSource,
        }
    }
    pub fn geometry(&self, place: ListPlace) -> Result<Geometry> {
        match self {
            From::Disc(d) => d.geometry(place),
            From::Synthetic => Ok(Geometry::fallback(place)),
        }
    }
    /// `s` in state `state` as a view: a dialog's
    /// ([`crate::ui_kit::dialog::dialog_view`]) or a list's ([`view_of`]).
    pub fn view(&self, s: &ScreenSpec, state: &str) -> Result<View> {
        self.view_with_metrics(s, state, self.source())
    }

    pub(crate) fn view_with_metrics(
        &self,
        s: &ScreenSpec,
        state: &str,
        metrics: &dyn Metrics,
    ) -> Result<View> {
        let mut view = if let Some(hud) = &s.hud {
            View {
                screen: s.name.clone(),
                state: state.into(),
                backing: Backing::None,
                widgets: hud.widgets.clone(),
                focus: None,
                list: None,
                underlay: None,
            }
        } else if let Some(d) = &s.dialog {
            let mut geometry = match self {
                From::Disc(disc) => disc.dialog_geometry_for(d.base)?,
                From::Synthetic => DialogGeometry::fallback_for(d.base),
            };
            if matches!(s.list, ListPlace::Moved { .. }) {
                geometry.rows = self.geometry(s.list)?;
                geometry.list = ListAt {
                    anchor: geometry.rows.anchor,
                    skin: geometry.rows.skin,
                };
            }
            dialog_view(s, d, &geometry, metrics, state)
        } else {
            view_of(s, &self.geometry(s.list)?, metrics, state)
        };
        if let Some(parent) = &s.underlay {
            view.backing = Backing::None;
            view.underlay = Some(Box::new(self.view_with_metrics(parent, state, metrics)?));
        }
        Ok(view)
    }
    /// The builder's reusable pose sampler for a page in a named state.
    pub fn animated_view(
        &self,
        s: &ScreenSpec,
        state: &str,
    ) -> Result<crate::ui_kit::animation::AnimatedView> {
        use crate::ui_kit::animation::AnimatedView;
        Ok(AnimatedView::new(
            self.view(s, state)?,
            s.animation,
            s.dialog.is_some(),
        ))
    }

    /// `s` in state `state`: its view and its picture.
    pub fn draw(&self, s: &ScreenSpec, state: &str) -> Result<(View, Canvas)> {
        let v = self.view(s, state)?;
        let c = render_view(&v, self.source());
        Ok((v, c))
    }
}

/// Writes `<screen>-<state>.png` and `<screen>-<state>.json` (the view,
/// every widget's rect) into `dir` for each of `states`, and returns the
/// pictures by `screen/state`. `screen` is the preview's name for it
/// ([`crate::ui_kit::screens::NAMES`]), not the spec's: the campaign and
/// Halo 1 Game Options pages are both `skulls` specs, and named by the
/// spec the second overwrote the first's files.
pub fn write_states(
    from: &From<'_>,
    dir: &Path,
    screen: &str,
    states: &[(String, ScreenSpec)],
) -> Result<Vec<(String, Canvas)>> {
    std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    let mut out = Vec::new();
    for (state, s) in states {
        let (v, c) = from.draw(s, state)?;
        let stem = format!("{screen}-{state}");
        write_frame(dir, &stem, &c, &v, None)?;
        out.push((format!("{screen}/{state}"), c));
    }
    Ok(out)
}

/// Writes PNG and JSON samples of named states at the requested points
/// of their transitions. The page geometry is prepared once per state.
pub fn write_animation_states(
    from: &From<'_>,
    dir: &Path,
    screen: &str,
    states: &[(String, ScreenSpec)],
    phases: &[crate::ui_kit::animation::Phase],
) -> Result<Vec<String>> {
    std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    let mut out = Vec::new();
    for (state, spec) in states {
        let mut animated = from.animated_view(spec, state)?;
        for &phase in phases {
            let v = animated.seek(phase);
            let c = render_view(v, from.source());
            let stem = format!("{screen}-{state}-{phase}");
            write_frame(dir, &stem, &c, v, Some(phase))?;
            out.push(format!("{screen}/{state}-{phase}"));
        }
    }
    Ok(out)
}

fn write_frame(
    dir: &Path,
    stem: &str,
    canvas: &Canvas,
    view: &View,
    phase: Option<crate::ui_kit::animation::Phase>,
) -> Result<()> {
    canvas.write_png(&dir.join(format!("{stem}.png")))?;
    let mut json = view.json();
    if let Some(phase) = phase {
        json["animation"] = serde_json::to_value(phase)?;
    }
    std::fs::write(
        dir.join(format!("{stem}.json")),
        serde_json::to_string_pretty(&json)?,
    )?;
    Ok(())
}

/// Every screen of [`crate::ui_kit::screens::NAMES`] in every state of
/// [`crate::ui_kit::screens::states`] into `dir`, and `sheet.png`, all of
/// them on one page. Returns the sheet's path.
pub fn write_sheet(from: &From<'_>, dir: &Path) -> Result<std::path::PathBuf> {
    use crate::ui_kit::screens;
    let mut tiles = Vec::new();
    for name in screens::NAMES {
        let states = screens::states(name).with_context(|| format!("no states for {name}"))?;
        tiles.extend(write_states(from, dir, name, &states)?);
    }
    let path = dir.join("sheet.png");
    sheet(&tiles, 4).write_png(&path)?;
    Ok(path)
}

/// A character of [`Canvas::label`]'s font: five rows of three bits, the
/// left pixel the high bit.
fn tiny_glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 1, 1],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        'a' => [2, 5, 7, 5, 5],
        'b' => [6, 5, 6, 5, 6],
        'c' => [3, 4, 4, 4, 3],
        'd' => [6, 5, 5, 5, 6],
        'e' => [7, 4, 6, 4, 7],
        'f' => [7, 4, 6, 4, 4],
        'g' => [3, 4, 5, 5, 3],
        'h' => [5, 5, 7, 5, 5],
        'i' => [7, 2, 2, 2, 7],
        'j' => [1, 1, 1, 5, 2],
        'k' => [5, 5, 6, 5, 5],
        'l' => [4, 4, 4, 4, 7],
        'm' => [5, 7, 7, 5, 5],
        'n' => [6, 5, 5, 5, 5],
        'o' => [2, 5, 5, 5, 2],
        'p' => [6, 5, 6, 4, 4],
        'q' => [2, 5, 5, 6, 3],
        'r' => [6, 5, 6, 5, 5],
        's' => [3, 4, 2, 1, 6],
        't' => [7, 2, 2, 2, 2],
        'u' => [5, 5, 5, 5, 7],
        'v' => [5, 5, 5, 5, 2],
        'w' => [5, 5, 7, 7, 5],
        'x' => [5, 5, 2, 5, 5],
        'y' => [5, 5, 2, 2, 2],
        'z' => [7, 1, 2, 4, 7],
        '-' => [0, 0, 7, 0, 0],
        '.' => [0, 0, 0, 0, 2],
        '/' => [1, 1, 2, 4, 4],
        '_' => [0, 0, 0, 0, 7],
        _ => [0; 5],
    }
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn retail_widgets_name_their_tag_fields() {
        assert_eq!(
            retail_field(0, "text.3"),
            Ok(RetailField::Rect("panes[0].text_blocks[3].rect".into()))
        );
        assert_eq!(
            retail_field(1, "button.0").unwrap().path(),
            "panes[1].buttons[0].rect"
        );
        assert_eq!(
            retail_field(0, "bitmap.2"),
            Ok(RetailField::Point("panes[0].bitmaps[2].top_left".into()))
        );
        assert_eq!(
            retail_field(0, "list.0").unwrap().path(),
            "panes[0].lists[0].anchor"
        );
        assert_eq!(
            retail_field(0, "table.1").unwrap().path(),
            "panes[0].tables[1].anchor"
        );
        let title = retail_field(0, "title").unwrap_err();
        assert!(title.starts_with("refused") && title.contains("wigl"), "{title}");
        assert!(retail_field(0, "row.0.label").is_err());
        assert!(retail_field(0, "text.x").is_err());
        // A point takes the rect's top-left as x, y.
        let r = [100, -50, 80, 20];
        assert_eq!(RetailField::Point(String::new()).value(r), serde_json::json!([-50, 100]));
        assert_eq!(RetailField::Rect(String::new()).value(r), serde_json::json!(r));
    }

    #[test]
    fn places_move_the_blocks_they_name() {
        let mut pane = tui::Pane {
            flags: 0,
            animation: tui::AnimationRef(0),
            buttons: Vec::new(),
            lists: vec![tui::List {
                flags: 0,
                skin: 2,
                visible_items: 5,
                anchor: tui::Point { x: -254, y: 130 },
                animation: tui::AnimationRef(0),
                item_value: 0,
            }],
            text_blocks: Vec::new(),
            bitmaps: Vec::new(),
            model_scenes: Vec::new(),
            shader_bitmaps: Vec::new(),
            tables: Vec::new(),
        };
        let mut places = RetailPlaces::new();
        places.insert("list.0".into(), [140, -244, 0, 0]);
        // Blocks the pane has not got, and ids that are not blocks, are skipped.
        places.insert("text.4".into(), [1, 2, 3, 4]);
        places.insert("title".into(), [1, 2, 3, 4]);
        place_retail(&mut pane, &places);
        assert_eq!(pane.lists[0].anchor, tui::Point { x: -244, y: 140 });
    }

    #[test]
    fn a_filmstrip_lays_its_frames_side_by_side() {
        let frames: Vec<(String, Canvas)> = (0..3)
            .map(|i| (format!("t{i}.00s"), Canvas::new(640, 480, [i * 80, 0, 0, 255])))
            .collect();
        let (strip, at) = filmstrip(&frames);
        assert_eq!(at, vec![[8, 320], [336, 320], [664, 320]]);
        assert_eq!(strip.width, 3 * (320 + 8) + 8);
        assert_eq!(strip.height, 240 + 22 + 16);
        // The second frame's pixels are its own, at half size.
        let px = |x: u32, y: u32| {
            let i = ((y * strip.width + x) * 4) as usize;
            strip.rgba[i]
        };
        assert_eq!(px(336 + 10, 8 + 22 + 10), 80);
        assert_eq!(px(664 + 10, 8 + 22 + 10), 160);
    }
}

#[cfg(test)]
mod emblem_tests {
    use super::{Image, emblem};

    fn px(pixels: &[[u8; 4]]) -> Image {
        Image {
            width: pixels.len() as u32,
            height: 1,
            rgba: pixels.concat(),
        }
    }

    /// Red picks a layer's first colour and blue its second; the
    /// foreground covers the background by its masks; a pixel no mask
    /// covers is clear.
    #[test]
    fn an_emblem_is_its_two_layers_in_four_colours() {
        let (fa, fb, ba, bb) = ([240, 240, 240], [20, 20, 20], [176, 44, 40], [44, 84, 176]);
        // Background: first colour, second colour, first, none.
        let bg = px(&[[255, 255, 0, 0], [0, 0, 255, 255], [255, 255, 0, 0], [0, 0, 0, 0]]);
        // Foreground: none, none, all of its first colour, none.
        let fg = px(&[[0, 0, 0, 0], [0, 0, 0, 0], [255, 255, 0, 0], [0, 0, 0, 0]]);
        let e = emblem(&bg, &fg, [fa, fb, ba, bb]).unwrap();
        let at = |i: usize| &e.rgba[i * 4..i * 4 + 4];
        assert_eq!(at(0), [176, 44, 40, 255]);
        assert_eq!(at(1), [44, 84, 176, 255]);
        assert_eq!(at(2), [240, 240, 240, 255], "the foreground covers it");
        assert_eq!(at(3)[3], 0, "outside the art nothing is drawn");
        // Half a foreground mask lets half the background through.
        let half = emblem(&px(&[[255, 255, 0, 0]]), &px(&[[0, 0, 128, 128]]), [fa, fb, ba, bb]);
        let half = half.unwrap();
        assert!(half.rgba[0] > 20 && half.rgba[0] < 176, "{:?}", half.rgba);
        assert_eq!(half.rgba[3], 255);
        // Layers of different sizes are not an emblem.
        assert!(emblem(&bg, &px(&[[0, 0, 0, 0]]), [fa, fb, ba, bb]).is_none());
    }
}
