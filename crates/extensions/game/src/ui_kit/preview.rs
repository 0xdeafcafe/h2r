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

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use map::TagId;
use tags::ui::{self as tui, MapSource, Rect, UiTag};

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
    level_pictures: OnceCell<BTreeMap<String, u32>>,
    /// Every screen of `mainmenu.map` by screen id, decoded on first use.
    screens: RefCell<Option<HashMap<i16, tui::Screen>>>,
    /// The screens modals open over, made and drawn once.
    openers: RefCell<HashMap<Opener, Option<Drawn>>>,
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
                // As the game opens it: with the bot mark and the Mods
                // list's marks added (`bot_mark::GearRewrite`).
                let raw = x.read(&find(&format!("/maps/fonts/{name}"))?)?;
                let font = crate::bot_mark::GearRewrite::patch(&raw).unwrap_or(raw);
                files.insert(name.clone(), font);
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
        let (screen, strings) = {
            let mut m = self.map.borrow_mut();
            let id = m.tag(map::TagQuery::NameInGroup(o.tag, tui::SCREEN))?.id;
            let mut src = MapSource::new(&mut m);
            let UiTag::Screen(screen) = src.decode_tag(id)?.tag else {
                bail!("{} is not a screen", o.tag);
            };
            drop(src);
            let pane = screen
                .panes
                .get(o.pane)
                .with_context(|| format!("{} has no pane {}", o.tag, o.pane))?;
            let ids: Vec<u32> = pane
                .text_blocks
                .iter()
                .map(|t| t.string.0)
                .chain([screen.header.0])
                .collect();
            let mut strings = HashMap::new();
            if let Some(u) = screen.strings.id {
                let loc = tags::localization::Localization::read(&mut m)?;
                let unic = loc.unic(&mut m, u)?;
                for id in ids {
                    if let Ok(Some(t)) = loc.text_in(
                        &unic,
                        tags::localization::StringId(id),
                        tags::localization::Language::English,
                    ) {
                        strings.insert(id, t);
                    }
                }
            }
            (screen, strings)
        };
        let pane = &screen.panes[o.pane];
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
            && let Some(h) = strings.get(&screen.header.0)
        {
            w.push(text(
                "opener.title".into(),
                self.shared.header_layouts[layout].header,
                self.shared.header_fonts[layout],
                0,
                h.clone(),
                rgb([
                    self.shared.header_color[1],
                    self.shared.header_color[2],
                    self.shared.header_color[3],
                ]),
            ));
        }
        for (i, t) in pane.text_blocks.iter().enumerate() {
            if let Some(s) = strings.get(&t.string.0).filter(|s| !s.is_empty())
                && on_screen(t.rect.top, t.rect.left)
            {
                w.push(text(
                    format!("opener.text.{i}"),
                    t.rect,
                    t.font,
                    tui::justification(t.flags),
                    s.clone(),
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

    /// [`Disc::opener_view`] and its picture, made once per opener.
    fn opener_drawn(&self, o: &Opener) -> Option<Drawn> {
        if let Some(d) = self.openers.borrow().get(o) {
            return d.clone();
        }
        let d = self.opener_view(o).ok().map(|v| {
            let c = render_view_where(&v, self, &|_| true, false);
            std::sync::Arc::new((v, c))
        });
        self.openers.borrow_mut().insert(*o, d.clone());
        d
    }

    /// A retail screen at rest, as its tag lays it out: pane `r.pane`'s
    /// bitmaps, text blocks and buttons, and its tables' cells, over the
    /// game shell background, with the header and footer in the `wigl`
    /// header layout. Texts come from the screen's own string table
    /// (`unic`, English) unless `r.text` answers the block's string id; the
    /// game's code fills the empty blocks and the format strings at run
    /// time, which `r.text` stands in for (`Some("")` leaves one blank).
    /// Each table cell's first text block shows `r.cells[i]`; a cell with
    /// no entry is not drawn (the game hides empty player slots).
    pub fn render_retail(&self, r: &Retail<'_>) -> Result<Canvas> {
        let (screen, unic, skins) = {
            let mut m = self.map.borrow_mut();
            let id = m.tag(map::TagQuery::NameInGroup(r.tag, tui::SCREEN))?.id;
            let mut src = MapSource::new(&mut m);
            let UiTag::Screen(screen) = src.decode_tag(id)?.tag else {
                bail!("{} is not a screen", r.tag);
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
            (screen, unic, skins)
        };
        let pane = screen
            .panes
            .get(r.pane)
            .with_context(|| format!("{} has no pane {}", r.tag, r.pane))?;
        let lookup = |id: u32| -> Option<String> {
            if let Some(t) = (r.text)(id) {
                return Some(t);
            }
            let (unic, loc) = unic.as_ref()?;
            loc.text_in(
                unic,
                tags::localization::StringId(id),
                tags::localization::Language::English,
            )
            .ok()
            .flatten()
        };
        let g = self.geometry(ListPlace::Controls)?;
        let rgb = |v: [f32; 3]| v.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8);
        let mut c = Canvas::new(640, 480, [0, 0, 0, 255]);
        for p in &self.background.panes {
            self.draw_blocks(&mut c, p.bitmaps.iter());
        }
        // Blocks parked off screen (9999) are the game's spares.
        let on_screen = |rect: &Rect| rect.top.abs() < 1000 && rect.left.abs() < 1000;
        self.draw_blocks(
            &mut c,
            pane.bitmaps
                .iter()
                .filter(|b| b.top_left.x.abs() < 1000 && b.top_left.y.abs() < 1000),
        );
        if screen.flags & tui::Screen::NO_HEADER == 0
            && let Some(h) = lookup(screen.header.0)
        {
            c.text(
                &self.fonts,
                g.header_font,
                &h,
                g.header,
                0,
                rgb(g.header_colour),
            );
        }
        if let Some(f) = &r.footer {
            c.text(
                &self.fonts,
                g.header_font,
                f,
                g.footer,
                1,
                rgb(g.header_colour),
            );
        }
        for b in &pane.buttons {
            if let Some(img) = self.image(b.bitmap, 0) {
                c.image(
                    &img,
                    i32::from(b.rect.left) + i32::from(b.bitmap_offset.x),
                    i32::from(b.rect.top) + i32::from(b.bitmap_offset.y),
                    255,
                );
            }
            if let Some(t) = lookup(b.string.0) {
                c.text(
                    &self.fonts,
                    b.font,
                    &t,
                    b.rect,
                    tui::justification(b.flags),
                    rgb(b.text_color),
                );
            }
        }
        for t in pane.text_blocks.iter().filter(|t| on_screen(&t.rect)) {
            if let Some(text) = lookup(t.string.0).filter(|s| !s.is_empty()) {
                c.text(
                    &self.fonts,
                    t.font,
                    &text,
                    t.rect,
                    tui::justification(t.flags),
                    rgb(t.text_color),
                );
            }
        }
        for table in &pane.tables {
            let Some(skin) = table.skin.id.and_then(|id| skins.get(&id)) else {
                continue;
            };
            for (i, cell) in r.cells.iter().enumerate() {
                let Some(o) = table.cell_origin(i as i32) else {
                    continue;
                };
                let (ox, oy) = (i32::from(o.x), i32::from(o.y));
                let mut bitmaps: Vec<&tui::BitmapBlock> = skin.bitmaps.iter().collect();
                bitmaps.sort_by_key(|b| b.render_depth_bias);
                if let Some(b) = bitmaps.first()
                    && let Some(img) = self.image(b.bitmap, b.image)
                {
                    c.image(
                        &img,
                        ox + i32::from(b.top_left.x),
                        oy + i32::from(b.top_left.y) + img.height as i32,
                        255,
                    );
                }
                if let Some(t) = skin.text_blocks.first() {
                    let at = Rect {
                        top: (i32::from(t.rect.top) + oy) as i16,
                        bottom: (i32::from(t.rect.bottom) + oy) as i16,
                        left: (i32::from(t.rect.left) + ox) as i16,
                        right: (i32::from(t.rect.right) + ox) as i16,
                    };
                    c.text(
                        &self.fonts,
                        t.font,
                        cell,
                        at,
                        tui::justification(t.flags),
                        rgb(t.text_color),
                    );
                }
            }
        }
        Ok(c)
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
    /// The text for a string id, standing in for what the game's code
    /// writes (formats, empty blocks); None takes the tag's own.
    pub text: &'a dyn Fn(u32) -> Option<String>,
    /// The table cells' texts (player names), one per filled slot.
    pub cells: &'a [String],
    pub footer: Option<String>,
}

impl Metrics for Fonts {
    fn width(&self, font: i16, text: &str) -> i32 {
        Fonts::width(self, font, text)
    }
    fn line_height(&self, font: i16) -> i32 {
        Fonts::line_height(self, font)
    }
}

impl Metrics for Disc {
    fn width(&self, font: i16, text: &str) -> i32 {
        self.fonts.width(font, text)
    }
    fn line_height(&self, font: i16) -> i32 {
        self.fonts.line_height(font)
    }
}

/// What a [`View`] is drawn from: the disc ([`Disc`]) or nothing
/// ([`SyntheticSource`]).
pub trait Source: Metrics {
    /// The image art names; `None` draws a placeholder box.
    fn art(&self, a: &Art) -> Option<Image>;
    fn backing(&self, c: &mut Canvas, b: Backing);
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
    fn list_of(&self, screen_id: i16, pane: u16) -> Option<Option<ListAt>> {
        let s = self.screen(screen_id)?;
        let p = s.panes.get(usize::from(pane))?;
        Some(p.lists.first().map(|l| ListAt {
            anchor: [l.anchor.x, l.anchor.y],
            skin: l.skin,
        }))
    }
    fn opener(&self, o: &Opener) -> Option<View> {
        self.opener_drawn(o).map(|d| d.0.clone())
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
                if let Some(d) = self.opener_drawn(&o) {
                    c.blit(&d.1, 0, 0);
                }
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
        // when an audit samples the pixels underneath the dialog's text.
        render_composite(parent, src, &|_| true, false, clear)
    } else {
        let mut c = Canvas::new(640, 480, clear);
        src.backing(&mut c, v.backing);
        c
    };
    let mut order: Vec<&crate::ui_kit::view::Widget> = v.widgets.iter().collect();
    order.sort_by_key(|w| w.layer);
    for w in order.into_iter().filter(|w| w.alpha > 0 && keep(w)) {
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
                src.text(&mut c, *font, text, r, *justify, *colour, w.alpha);
                if marks && overflows(w, src) {
                    c.outline(r, [230, 40, 40, 255]);
                }
            }
        }
    }
    c
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
        let shadow = (160 * u32::from(alpha) / 255) as u8;
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
                    self.run(&font, &units, pen + 1, y + 1, [0, 0, 0, shadow], false);
                }
                self.run(&font, &units, pen, y, [rgb[0], rgb[1], rgb[2], alpha], icon);
                pen += fonts.width(index, &s);
            }
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
        let mut view = if let Some(d) = &s.dialog {
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
            dialog_view(s, d, &geometry, self.source(), state)
        } else {
            view_of(s, &self.geometry(s.list)?, self.source(), state)
        };
        if let Some(parent) = &s.underlay {
            view.backing = Backing::None;
            view.underlay = Some(Box::new(self.view(parent, state)?));
        }
        Ok(view)
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
        c.write_png(&dir.join(format!("{stem}.png")))?;
        std::fs::write(
            dir.join(format!("{stem}.json")),
            serde_json::to_string_pretty(&v.json())?,
        )?;
        out.push((format!("{screen}/{state}"), c));
    }
    Ok(out)
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
