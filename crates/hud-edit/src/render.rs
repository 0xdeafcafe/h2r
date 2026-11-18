//! The preview: a HUD drawn as the engine places it on a 640 by 480
//! canvas, from the map's own bitmaps and the disc's fonts, with where
//! each widget landed and which pixels it covers (for the checks).

use std::collections::HashMap;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use assets::ResourceFiles;
use map::{Map, TagId};

use crate::layout::{self, Mode, SPECIAL_SHIELD};
use crate::model::{Hud, Widget, WidgetKey, WidgetKind};
use crate::place::{self, Frame, Rect, SCREEN};
use crate::state::{self, HudState};

pub const W: u32 = SCREEN[0] as u32;
pub const H: u32 = SCREEN[1] as u32;

/// RGBA, 640 by 480, top-left origin.
#[derive(Clone)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Canvas {
    /// A dim, game-like backdrop so light HUD art reads.
    pub fn backdrop() -> Canvas {
        let mut rgba = Vec::with_capacity((W * H * 4) as usize);
        for y in 0..H {
            for x in 0..W {
                let t = y as f32 / H as f32;
                let g = ((x / 32 + y / 32) % 2) as f32 * 4.0;
                rgba.extend_from_slice(&[
                    (38.0 + 20.0 * t + g) as u8,
                    (46.0 + 18.0 * t + g) as u8,
                    (40.0 + 8.0 * t + g) as u8,
                    255,
                ]);
            }
        }
        Canvas {
            width: W,
            height: H,
            rgba,
        }
    }
    fn blend(&mut self, x: i32, y: i32, c: [u8; 4]) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 || c[3] == 0 {
            return false;
        }
        let i = ((y as u32 * self.width + x as u32) * 4) as usize;
        let a = u32::from(c[3]);
        for (d, s) in self.rgba[i..i + 3].iter_mut().zip(c) {
            *d = ((u32::from(s) * a + u32::from(*d) * (255 - a)) / 255) as u8;
        }
        true
    }
    pub fn write_png(&self, path: &Path) -> anyhow::Result<()> {
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = png::Encoder::new(file, self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&self.rgba)?;
        Ok(())
    }
}

/// What a widget draws of its bitmap.
#[derive(Clone, Debug)]
pub struct Sprite {
    /// In HUD pixels.
    pub size: [f32; 2],
    /// The sprite's texels (`size` rounded), when its image decoded.
    pub rgba: Option<Vec<u8>>,
    pub px: [u32; 2],
    /// Why the texels are missing, or what was assumed.
    pub note: Option<String>,
}

/// The map's bitmaps, decoded once each.
pub struct Art {
    files: ResourceFiles,
    bitmaps: HashMap<TagId, Option<tags::Bitmap>>,
    images: HashMap<(TagId, usize), Result<(u32, u32, Vec<u8>), String>>,
}

impl Art {
    pub fn new(files: ResourceFiles) -> Self {
        Art {
            files,
            bitmaps: HashMap::new(),
            images: HashMap::new(),
        }
    }

    fn bitmap<R: Read + Seek>(&mut self, map: &mut Map<R>, id: TagId) -> Option<&tags::Bitmap> {
        self.bitmaps
            .entry(id)
            .or_insert_with(|| match tags::DecodedTag::read(map, id).ok()?.data {
                tags::TagData::Bitmap(b) => Some(*b),
                _ => None,
            })
            .as_ref()
    }

    /// Bitmap `id`'s sequence `seq` as a widget draws it (E `0x22ae21`):
    /// the sequence's first sprite (its bounds in its image), or with no
    /// sprites its first image whole; a bitmap without sequences (or a
    /// sequence out of range) draws image 0.
    pub fn sprite<R: Read + Seek>(
        &mut self,
        map: &mut Map<R>,
        id: TagId,
        seq: i8,
    ) -> Result<Sprite, String> {
        let b = self
            .bitmap(map, id)
            .ok_or_else(|| format!("{id} is not a readable bitmap"))?;
        let mut note = None;
        let pick = usize::try_from(seq)
            .ok()
            .and_then(|s| b.sequences.get(s));
        if pick.is_none() && !b.sequences.is_empty() {
            note = Some(format!("sequence {seq} is not in the bitmap: image 0 drawn"));
        }
        let (image, bounds) = match pick {
            Some(s) if !s.sprites.is_empty() => {
                let p = &s.sprites[0];
                (usize::try_from(p.image_index).unwrap_or(0), p.bounds)
            }
            Some(s) => (
                usize::try_from(s.wire.first_image).unwrap_or(0),
                [0.0, 1.0, 0.0, 1.0],
            ),
            None => (0, [0.0, 1.0, 0.0, 1.0]),
        };
        let img = b
            .images
            .get(image)
            .ok_or_else(|| format!("image {image} is not in the bitmap"))?;
        let [iw, ih, _] = img.dimensions.map(f32::from);
        let size = [(bounds[1] - bounds[0]) * iw, (bounds[3] - bounds[2]) * ih];
        let px = size.map(|v| v.round().max(0.0) as u32);
        let img = img.clone();
        let decoded = self
            .images
            .entry((id, image))
            .or_insert_with(|| {
                let d = assets::decode_image(&img, &mut self.files).map_err(|e| e.to_string())?;
                let l = d
                    .levels
                    .into_iter()
                    .find(|l| l.face == 0 && l.level == 0)
                    .ok_or("no top level")?;
                let n = (l.width * l.height * 4) as usize;
                Ok((l.width, l.height, l.rgba.get(..n).ok_or("short level")?.to_vec()))
            })
            .clone();
        let rgba = match decoded {
            Ok((w, h, rgba)) => {
                let x0 = (bounds[0] * w as f32).round() as u32;
                let y0 = (bounds[2] * h as f32).round() as u32;
                let mut out = Vec::with_capacity((px[0] * px[1] * 4) as usize);
                for y in 0..px[1] {
                    for x in 0..px[0] {
                        let (sx, sy) = ((x0 + x).min(w - 1), (y0 + y).min(h - 1));
                        let i = ((sy * w + sx) * 4) as usize;
                        out.extend_from_slice(&rgba[i..i + 4]);
                    }
                }
                Some(out)
            }
            Err(e) => {
                note = Some(format!("pixels not decoded ({e})"));
                None
            }
        };
        Ok(Sprite {
            size,
            rgba,
            px,
            note,
        })
    }
}

/// The disc's fonts by `font_table.txt` line (the engine's font index:
/// docs/game/ui-builder.md, `boot::ui_kit::preview::Fonts`).
pub struct Fonts {
    pub table: Vec<String>,
    files: HashMap<String, Vec<u8>>,
}

impl Fonts {
    /// From a disc image (`/maps/fonts`) or a folder holding a `fonts`
    /// folder beside the map.
    pub fn find(map_path: &Path, in_image: bool) -> Option<Fonts> {
        if in_image {
            let mut x = xiso::Xiso::open(BufReader::new(std::fs::File::open(map_path).ok()?)).ok()?;
            let entries = x.entries().ok()?;
            let mut read = |p: &str| {
                let e = entries.iter().find(|e| e.path.eq_ignore_ascii_case(p))?.clone();
                x.read(&e).ok()
            };
            let table = Self::table(&read("/maps/fonts/font_table.txt")?);
            let mut files = HashMap::new();
            for n in &table {
                if let Some(b) = read(&format!("/maps/fonts/{n}")) {
                    files.insert(n.clone(), b);
                }
            }
            return Some(Fonts { table, files });
        }
        let dir = map_path.parent()?.join("fonts");
        let table = Self::table(&std::fs::read(dir.join("font_table.txt")).ok()?);
        let files = table
            .iter()
            .filter_map(|n| Some((n.clone(), std::fs::read(dir.join(n)).ok()?)))
            .collect();
        Some(Fonts { table, files })
    }
    fn table(b: &[u8]) -> Vec<String> {
        String::from_utf8_lossy(b)
            .split(['\r', '\n'])
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    }
    fn font(&self, index: i16) -> Option<font::Font<'_>> {
        let name = self.table.get(usize::try_from(index).ok()?)?;
        font::Font::parse(self.files.get(name)?).ok()
    }
    /// `text` in engine font `index`: its picture and the line height.
    pub fn render(&self, index: i16, text: &str, colour: [u8; 4]) -> Option<(font::Bitmap, i32)> {
        let f = self.font(index)?;
        let units: Vec<u16> = text.encode_utf16().collect();
        let b = font::render(
            &f,
            &units,
            font::RenderOptions {
                color: colour,
                glyph_colors: false,
            },
        )
        .ok()?;
        Some((b, f.line_height()))
    }
}

/// Fallback text metrics without the disc's fonts: per character and line.
pub const FALLBACK_CHAR: [f32; 2] = [7.0, 12.0];

/// Where one widget landed.
#[derive(Clone, Debug)]
pub struct Placed {
    /// 0 the HUD edited; 1.. the HUDs drawn with it (weapon, vehicle).
    pub hud: usize,
    pub key: WidgetKey,
    pub name: String,
    /// The anchor point plus the offset.
    pub point: Option<[f32; 2]>,
    pub rect: Option<Rect>,
    /// The gate let it through (always, with the gate off).
    pub shown: bool,
    /// Drawn (shown and something to draw).
    pub drawn: bool,
    /// What it draws, for text widgets.
    pub text: Option<String>,
    /// Which screen pixels it covers with alpha above a quarter.
    pub cover: Option<Cover>,
    pub notes: Vec<String>,
}

/// A set of screen pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Cover(Vec<u64>);

impl Cover {
    fn new() -> Self {
        Cover(vec![0; (W * H).div_ceil(64) as usize])
    }
    fn set(&mut self, x: i32, y: i32) {
        if x >= 0 && y >= 0 && (x as u32) < W && (y as u32) < H {
            let i = y as usize * W as usize + x as usize;
            self.0[i / 64] |= 1 << (i % 64);
        }
    }
    pub fn count(&self) -> u32 {
        self.0.iter().map(|w| w.count_ones()).sum()
    }
    pub fn overlap(&self, o: &Cover) -> u32 {
        self.0.iter().zip(&o.0).map(|(a, b)| (a & b).count_ones()).sum()
    }
}

/// The drawing and where everything landed.
pub struct Picture {
    pub canvas: Canvas,
    pub placed: Vec<Placed>,
    pub frame: Frame,
    pub fonts_found: bool,
}

/// The text a text widget draws (E `0x22a871`): with flag 1 the number
/// its string names (`hud_input1`..`4`: the inputs), padded to two
/// digits with flag 2 and three with 4; flag 8 the player's name;
/// otherwise the string ID's name stands for its localized text.
pub fn widget_text(w: &Widget, string: Option<&str>, st: &HudState) -> String {
    let flags = w.flags();
    if flags & 8 != 0 {
        return "Chief".to_owned();
    }
    let name = string.unwrap_or("");
    if flags & 1 != 0 {
        let n = name
            .strip_prefix("hud_input")
            .and_then(|d| d.parse::<usize>().ok())
            .filter(|d| (1..=4).contains(d));
        let v = match n {
            Some(d) => st.input(w.inputs()[d - 1]).0,
            None => 0.0,
        };
        let v = v.max(0.0).round() as u32;
        let pad = if flags & 4 != 0 {
            3
        } else if flags & 2 != 0 {
            2
        } else {
            1
        };
        return format!("{v:0pad$}");
    }
    format!("<{name}>")
}

/// The texel at `(x, y)` of a meter, as `widget_legacy_meter` leaves it:
/// full where the texture's blue is below the fill, faint elsewhere so the
/// empty part still shows in the editor.
fn meter(p: [u8; 4], fill: f32) -> [u8; 4] {
    if f32::from(p[2]) / 255.0 < fill {
        p
    } else {
        [p[0], p[1], p[2], p[3] / 5]
    }
}

/// Draws `huds` (the edited one first; the others drawn with it, as the
/// engine draws a unit's and its weapon's `nhdt`s together, E `0x1394b9`).
pub fn draw<R: Read + Seek>(
    map: &mut Map<R>,
    art: &mut Art,
    fonts: Option<&Fonts>,
    huds: &[&Hud],
    st: &HudState,
) -> Picture {
    let mode: Mode = st.mode;
    let frame = Frame::for_mode(mode);
    let mut canvas = Canvas::backdrop();
    let mut placed = Vec::new();
    let masks = st.masks();
    for (h, hud) in huds.iter().enumerate() {
        for w in &hud.widgets {
            let shown = !st.gate || state::shown(w.state(), masks);
            let mut notes = Vec::new();
            let point = place::widget_point(&frame, w.anchor(), w.offset(mode));
            let point = point.or_else(|| {
                notes.push(format!(
                    "anchor {} is not a corner: drawn at the frame's centre",
                    w.anchor()
                ));
                place::widget_point(&frame, 4, w.offset(mode))
            });
            let mut p = Placed {
                hud: h,
                key: w.key,
                name: w.name.clone(),
                point,
                rect: None,
                shown,
                drawn: false,
                text: None,
                cover: None,
                notes,
            };
            match w.key.kind {
                WidgetKind::Bitmap => draw_bitmap(map, art, &mut canvas, w, st, &mut p),
                WidgetKind::Text => draw_text(map, fonts, &mut canvas, w, st, &mut p),
            }
            placed.push(p);
        }
    }
    Picture {
        canvas,
        placed,
        frame,
        fonts_found: fonts.is_some(),
    }
}

fn draw_bitmap<R: Read + Seek>(
    map: &mut Map<R>,
    art: &mut Art,
    canvas: &mut Canvas,
    w: &Widget,
    st: &HudState,
    p: &mut Placed,
) {
    let mode = st.mode;
    let Some(point) = p.point else { return };
    let Some(bitmap) = w.bitmap() else {
        p.notes.push("no bitmap".into());
        return;
    };
    let sprite = match art.sprite(map, bitmap, w.sequence(mode)) {
        Ok(s) => s,
        Err(e) => {
            p.notes.push(e);
            return;
        }
    };
    p.notes.extend(sprite.note.clone());
    let rect = place::bitmap_rect(point, sprite.size, w.registration(mode));
    p.rect = Some(rect);
    if !p.shown {
        return;
    }
    // The fill a meter shows: a meter shader reads hud_input1; the shield
    // meter (special type 5) the shield, through its shader's span of the
    // arc's gradient (docs/halo1/health.md).
    let shader = w
        .shader()
        .and_then(|s| map.tag(s).ok().and_then(|t| t.name.clone()))
        .unwrap_or_default();
    let fill = if w.special() == SPECIAL_SHIELD {
        Some(0.2156 + st.shield.clamp(0.0, 1.0) * (0.992 - 0.2156))
    } else if shader.contains("meter") {
        Some(st.input(w.inputs()[0]).0)
    } else {
        None
    };
    let flags = w.flags();
    let [x0, y0] = [rect.x0.round() as i32, rect.y0.round() as i32];
    let [pw, ph] = sprite.px;
    let mut cover = Cover::new();
    for y in 0..ph {
        for x in 0..pw {
            // Flags bits 0 and 1 mirror the texture (0x22b29c).
            let sx = if flags & 1 != 0 { pw - 1 - x } else { x };
            let sy = if flags & 2 != 0 { ph - 1 - y } else { y };
            let c = match &sprite.rgba {
                Some(rgba) => {
                    let i = ((sy * pw + sx) * 4) as usize;
                    let t = [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]];
                    match fill {
                        Some(f) => meter(t, f),
                        None => t,
                    }
                }
                // No texels: a hatched stand-in of the sprite's size.
                None => {
                    if (x + y) % 6 < 2 {
                        [255, 80, 200, 160]
                    } else {
                        [255, 80, 200, 40]
                    }
                }
            };
            let (cx, cy) = (x0 + x as i32, y0 + y as i32);
            if c[3] > 64 {
                cover.set(cx, cy);
            }
            canvas.blend(cx, cy, c);
        }
    }
    p.drawn = true;
    p.cover = Some(cover);
}

fn draw_text<R: Read + Seek>(
    map: &mut Map<R>,
    fonts: Option<&Fonts>,
    canvas: &mut Canvas,
    w: &Widget,
    st: &HudState,
    p: &mut Placed,
) {
    let mode = st.mode;
    let Some(point) = p.point else { return };
    if w.shader().is_none() {
        p.notes.push("no shader: the engine draws nothing (0x22aa2f)".into());
        return;
    }
    let Some(font) = layout::text_font(w.font(mode)) else {
        p.notes
            .push(format!("font byte {} names no font: not drawn", w.font(mode)));
        return;
    };
    let string = crate::model::string(map, w.string());
    let text = widget_text(w, string.as_deref(), st);
    let colour = [140, 200, 255, 255];
    let rendered = fonts.and_then(|f| f.render(font, &text, colour));
    let size = match &rendered {
        Some((b, line)) => [b.width as f32, (*line).max(b.height as i32) as f32],
        None => {
            p.notes
                .push("the disc's fonts were not found: text measured with a stand-in".into());
            [FALLBACK_CHAR[0] * text.chars().count() as f32, FALLBACK_CHAR[1]]
        }
    };
    let rect = place::text_rect(point, size, w.justification());
    p.rect = Some(rect);
    p.text = Some(text);
    if !p.shown {
        return;
    }
    let [x0, y0] = [rect.x0.round() as i32, rect.y0.round() as i32];
    let mut cover = Cover::new();
    match rendered {
        Some((b, _)) => {
            for y in 0..b.height {
                for x in 0..b.width {
                    let i = ((y * b.width + x) * 4) as usize;
                    let c = [b.rgba[i], b.rgba[i + 1], b.rgba[i + 2], b.rgba[i + 3]];
                    let (cx, cy) = (x0 + x as i32, y0 + y as i32);
                    if c[3] > 64 {
                        cover.set(cx, cy);
                    }
                    canvas.blend(cx, cy, c);
                }
            }
        }
        None => {
            let [rw, rh] = size.map(|v| v as i32);
            for y in 2..rh - 2 {
                for x in 0..rw {
                    if x % FALLBACK_CHAR[0] as i32 != 6 {
                        cover.set(x0 + x, y0 + y);
                        canvas.blend(x0 + x, y0 + y, colour);
                    }
                }
            }
        }
    }
    p.drawn = true;
    p.cover = Some(cover);
}
