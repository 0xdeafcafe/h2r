//! Our campaigns' level pictures in the level select
//! (docs/halo1/campaign-picker.md, "Pictures").
//!
//! The level select draws each level's picture from its record in the
//! globals' level list (`campaign_list::UI_LEVEL_PICTURE`, a `bitm`
//! reference). Retail's are mainmenu's `ui\code_global_bitmaps\campaign0`
//! to `campaign19`, one per list slot (15 used, the last 5 the free slots'
//! own). A converted level's card is Halo 1's own picture (written at import
//! as `<data>/campaigns/<key>/cards/<map>.png`, `h1convert::ui`).
//!
//! The cards go into mainmenu as new tags, in the VFS only
//! ([`CardRewrite`], a `vfs::Rewrite` like the button icons' font rewrite):
//! the disc's map decomposed (`TagSet::from_map`, every tag where it was),
//! the index given room for more (and the tag-memory bound, which retail
//! mainmenu already meets, lifted for a few kilobytes), one `bitm` per
//! card shaped like
//! `campaign0` ([`TEMPLATE`]; `tags::rgba_bitmap`), scaled to cover its
//! 220x207, put in the template's frame ([`framed`]: retail's frame is in
//! each picture's pixels, not drawn by the screen), and the map built
//! again. The free slots' bitmaps are left alone.
//! Each card's datum goes into a table the level lists read
//! (`campaigns::extend`). The rebuilt map's checksum is the builder's; the
//! cache copy's compare at `0x214616` (the header checksum's one reader,
//! `0x214618`) passes.

use std::collections::BTreeMap;
use std::io::{BufReader, Cursor};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use map::build::TagSet;
use map::{FourCC, Map, TagId};
use tags::rgba_bitmap::{self, Template};

/// The map the level select lives in, as the VFS names it.
pub const MAINMENU: &str = "maps\\mainmenu.map";
/// The retail level picture the cards are shaped like.
pub const TEMPLATE: &str = "ui\\code_global_bitmaps\\campaign0";
/// Where a campaign's cards are, under its folder.
pub const CARDS_DIR: &str = "cards";

/// Card datums in the rewritten mainmenu, by level map name (lower case).
pub type Table = Arc<Mutex<BTreeMap<String, u32>>>;

/// One level's card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// The level's map name, lower case.
    pub map: String,
    /// The tag's name in mainmenu.
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The tag name of `key`'s card for `map`.
pub fn tag_name(key: &str, map: &str) -> String {
    format!("ui\\code_global_bitmaps\\{key}_{map}").to_ascii_lowercase()
}

/// A card file: an 8-bit RGBA or RGB PNG.
pub fn read_png(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let d = png::Decoder::new(BufReader::new(std::fs::File::open(path)?));
    let mut r = d.read_info()?;
    let mut buf = vec![0; r.output_buffer_size().context("png size")?];
    let info = r.next_frame(&mut buf)?;
    let rgba = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buf[..info.buffer_size()].to_vec(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => buf[..info.buffer_size()]
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        (t, d) => bail!("{t:?} {d:?} png"),
    };
    Ok((info.width, info.height, rgba))
}

/// The cards of `levels` (key, map) found under `data_root`; a level
/// without one keeps the picture `campaigns::extend` gives it. Says what
/// it skipped.
pub fn read_cards(data_root: &Path, levels: &[(String, String)]) -> Vec<Card> {
    let mut out = Vec::new();
    for (key, map) in levels {
        let map = map.to_ascii_lowercase();
        let path = data_root
            .join("campaigns")
            .join(key)
            .join(CARDS_DIR)
            .join(format!("{map}.png"));
        if !path.is_file() {
            continue;
        }
        match read_png(&path) {
            Ok((width, height, rgba)) if width > 0 && height > 0 => out.push(Card {
                name: tag_name(key, &map),
                map,
                width,
                height,
                rgba,
            }),
            Ok(_) => eprintln!("campaigns: {}: an empty card; skipped", path.display()),
            Err(e) => eprintln!("campaigns: {}: {e:#}; skipped", path.display()),
        }
    }
    out
}

/// `rgba` (`sw` x `sh`) scaled to cover `dw` x `dh`: the smaller scale
/// that fills both, centred, the overflow cropped; bilinear.
pub fn cover(sw: u32, sh: u32, rgba: &[u8], dw: u32, dh: u32) -> Vec<u8> {
    let scale = (dw as f32 / sw as f32).max(dh as f32 / sh as f32);
    let (ox, oy) = (
        (sw as f32 - dw as f32 / scale) / 2.0,
        (sh as f32 - dh as f32 / scale) / 2.0,
    );
    let px = |x: i64, y: i64, c: usize| {
        let x = x.clamp(0, i64::from(sw) - 1) as usize;
        let y = y.clamp(0, i64::from(sh) - 1) as usize;
        f32::from(rgba[(y * sw as usize + x) * 4 + c])
    };
    let mut out = Vec::with_capacity((dw * dh * 4) as usize);
    for y in 0..dh {
        let fy = oy + (y as f32 + 0.5) / scale - 0.5;
        let (y0, ty) = (fy.floor() as i64, fy - fy.floor());
        for x in 0..dw {
            let fx = ox + (x as f32 + 0.5) / scale - 0.5;
            let (x0, tx) = (fx.floor() as i64, fx - fx.floor());
            for c in 0..4 {
                let top = px(x0, y0, c) * (1.0 - tx) + px(x0 + 1, y0, c) * tx;
                let bottom = px(x0, y0 + 1, c) * (1.0 - tx) + px(x0 + 1, y0 + 1, c) * tx;
                out.push((top * (1.0 - ty) + bottom * ty).round() as u8);
            }
        }
    }
    out
}

/// Whether `template`'s pixels (RGBA) carry the level pictures' frame:
/// every retail picture (`campaign0` to `campaign14`, V) is 220x207 with a
/// one-pixel opaque border (alpha 255, a pale blue), a bottom-right corner
/// cut off on the diagonal (alpha 0 past it, 136 pixels), and the picture
/// inside at alpha 191, so the screen shows it a quarter see-through. The
/// screen draws no frame of its own: the bitmap is the frame. A template
/// is taken as a frame when it has opaque pixels and more part
/// see-through ones than those (retail: 834 and 44,570), so a plain
/// picture is never stamped over a card.
pub fn has_frame(template: &[u8]) -> bool {
    let (mut inside, mut border) = (0usize, 0usize);
    for p in template.chunks_exact(4) {
        match p[3] {
            255 => border += 1,
            0 => {}
            _ => inside += 1,
        }
    }
    border > 0 && inside > border
}

/// `card` (RGBA, the template's size) in the frame of `template`, pixel by
/// pixel from the template's alpha: opaque, the template's own pixel (the
/// border); clear, clear (the cut corner); anything else, the card's
/// colour at the template's alpha (the see-through inside). Only the
/// user's disc's pixels go in: nothing of the frame is kept here.
pub fn framed(card: &[u8], template: &[u8]) -> Vec<u8> {
    card.chunks_exact(4)
        .zip(template.chunks_exact(4))
        .flat_map(|(c, t)| match t[3] {
            255 => [t[0], t[1], t[2], 255],
            0 => [0; 4],
            a => [c[0], c[1], c[2], a],
        })
        .collect()
}

/// What [`add_cards`] made.
#[derive(Debug)]
pub struct Added {
    pub map: Vec<u8>,
    /// Each card's datum by map name.
    pub table: BTreeMap<String, u32>,
    /// Whether the cards took [`TEMPLATE`]'s frame ([`framed`]); when its
    /// pixels can't be read or hold none, the cards go in plain.
    pub framed: bool,
}

/// `mainmenu` with one new `bitm` per card, shaped like [`TEMPLATE`] and
/// in its frame.
pub fn add_cards(mainmenu: &[u8], cards: &[Card]) -> Result<Added> {
    let mut map = Map::from_reader(Cursor::new(mainmenu.to_vec()))?;
    let template_id = map
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"bitm") && t.name.as_deref() == Some(TEMPLATE))
        .map(|t| t.id)
        .with_context(|| format!("no {TEMPLATE}"))?;
    let template = Template::read(&mut map, template_id)?;
    let (w, h) = template.size();
    let frame = template
        .rgba(&mut map)
        .ok()
        .filter(|px| px.len() == usize::from(w) * usize::from(h) * 4 && has_frame(px));
    let mut set = TagSet::from_map(&mut map)?;
    // The index takes more tags (h1sounds::convert::make_index_room).
    let pinned = set
        .pinned
        .as_mut()
        .context("a decomposed map has its image")?;
    pinned.clear_index_tail();
    pinned.index_entries_end = pinned.index.len() as u32;
    // Retail mainmenu is the largest of its type, which is all the
    // builder's tag-memory bound knows (map-format.md: `+0x1c`'s bound is
    // empirical; no loader constant limits it, the arena does, and it
    // holds a campaign map's 0xc24000). The cards add a few kilobytes
    // (the disc test bounds it).
    set.check_tag_memory = false;
    let mut table = BTreeMap::new();
    for c in cards {
        let mut rgba = cover(c.width, c.height, &c.rgba, u32::from(w), u32::from(h));
        if let Some(f) = &frame {
            rgba = framed(&rgba, f);
        }
        let id: TagId = rgba_bitmap::install(&mut set, &template, &c.name, w, h, &rgba)
            .with_context(|| c.name.clone())?;
        table.insert(c.map.clone(), id.0);
    }
    let (map, _) = set.build()?;
    Ok(Added {
        map,
        table,
        framed: frame.is_some(),
    })
}

/// Serves mainmenu with the cards in it (see the module docs), and fills
/// the table the level lists read.
pub struct CardRewrite {
    cards: Vec<Card>,
    table: Table,
}

impl CardRewrite {
    /// `None` without cards: mainmenu is served as it is.
    pub fn new(cards: Vec<Card>, table: Table) -> Option<Self> {
        (!cards.is_empty()).then_some(Self { cards, table })
    }
}

impl vfs::Rewrite for CardRewrite {
    fn wants(&self, path: &str) -> bool {
        path.eq_ignore_ascii_case(MAINMENU)
    }
    fn rewrite(&mut self, _path: &str, original: &[u8]) -> Option<Vec<u8>> {
        let started = std::time::Instant::now();
        match add_cards(original, &self.cards) {
            Ok(Added { map, table, framed }) => {
                eprintln!(
                    "campaigns: {} level pictures added to mainmenu{} ({} ms)",
                    table.len(),
                    if framed { ", framed as retail's" } else { ", unframed (the template has no frame)" },
                    started.elapsed().as_millis()
                );
                *self.table.lock().unwrap_or_else(|e| e.into_inner()) = table;
                Some(map)
            }
            Err(e) => {
                eprintln!("campaigns: mainmenu left as it is, no level pictures: {e:#}");
                None
            }
        }
    }
    fn skipped(&mut self, path: &str) {
        eprintln!("campaigns: {path} is too large to rewrite; no level pictures");
    }
}

#[cfg(test)]
#[path = "campaign_cards_tests.rs"]
mod tests;
