//! A Halo 1 map's own card in the map list: its name, description and
//! picture, as Halo 1's menu shows them, read from the disc's `ui.map`.
//!
//! D (`ui.map` on the Xbox disc): the multiplayer map select draws
//! `ui\shell\bitmaps\mp_map_grafix` (14 images, 256x128), names from
//! `ui\shell\main_menu\mp_map_list` (`ustr`, 15 strings) and descriptions
//! from `ui\shell\main_menu\multiplayer_type_select\mp_map_select\map_data`
//! (13 strings), all three in the list order [`MP_ORDER`] (image 9 is
//! Blood Gulch, image 0 Battle Creek). The campaign's are
//! `ui\shell\bitmaps\sp_levels` (11 images), `ui\shell\main_menu\map_list`
//! and `ui\shell\main_menu\solo_level_select\map_data`, in play order
//! [`CAMPAIGN_ORDER`]. Every picture fills only the top-left
//! [`PICTURE`] of its image (the rest is black on all 25).

use h1map::{Group, H1Map};

use crate::{Error, Result};

/// The multiplayer list's order: map file names.
pub const MP_ORDER: [&str; 13] = [
    "beavercreek",
    "sidewinder",
    "damnation",
    "ratrace",
    "prisoner",
    "hangemhigh",
    "chillout",
    "carousel",
    "boardingaction",
    "bloodgulch",
    "wizard",
    "putput",
    "longest",
];

/// The campaign's play order, kept in `h1-contract` (the game picker
/// reads it).
pub use h1_contract::campaign::CAMPAIGN_ORDER;

/// The picture's part of each list image: width, height.
pub const PICTURE: (u32, u32) = (139, 113);

const MP_PICTURES: &str = "ui\\shell\\bitmaps\\mp_map_grafix";
const MP_NAMES: &str = "ui\\shell\\main_menu\\mp_map_list";
const MP_TEXT: &str = "ui\\shell\\main_menu\\multiplayer_type_select\\mp_map_select\\map_data";
const SP_PICTURES: &str = "ui\\shell\\bitmaps\\sp_levels";
const SP_NAMES: &str = "ui\\shell\\main_menu\\map_list";
const SP_TEXT: &str = "ui\\shell\\main_menu\\solo_level_select\\map_data";

/// One map's card.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub name: String,
    pub description: String,
    /// [`PICTURE`]-sized RGBA.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The multiplayer map `map`'s card, or `None` for a map the list hasn't.
pub fn mp_card(ui: &H1Map, map: &str) -> Result<Option<Card>> {
    match MP_ORDER.iter().position(|m| *m == map) {
        Some(i) => card(ui, i, MP_PICTURES, MP_NAMES, MP_TEXT).map(Some),
        None => Ok(None),
    }
}

/// The campaign level `level`'s card, or `None` for one the list hasn't.
pub fn campaign_card(ui: &H1Map, level: &str) -> Result<Option<Card>> {
    match CAMPAIGN_ORDER.iter().position(|m| *m == level) {
        Some(i) => card(ui, i, SP_PICTURES, SP_NAMES, SP_TEXT).map(Some),
        None => Ok(None),
    }
}

fn card(ui: &H1Map, i: usize, pictures: &str, names: &str, text: &str) -> Result<Card> {
    let find = |name: &str, group: &[u8; 4]| {
        ui.tags()
            .iter()
            .find(|t| t.name == name && t.group == Group::new(group))
            .ok_or_else(|| Error::Ui(format!("ui.map has no {name}")))
    };
    let strings = |name: &str| -> Result<Vec<String>> {
        let t = find(name, b"ustr")?;
        Ok(h1_assets::text::h1::Ustr::read(ui, t)
            .map_err(|e| Error::Ui(format!("{name}: {e}")))?
            .strings)
    };
    let name = strings(names)?
        .get(i)
        .map(|s| tidy(s))
        .ok_or_else(|| Error::Ui(format!("{names} has no string {i}")))?;
    let description = strings(text)?.get(i).map(|s| tidy(s)).unwrap_or_default();
    let t = find(pictures, b"bitm")?;
    let bm =
        h1map::bitmap::Bitmap::read(ui, t).map_err(|e| Error::Ui(format!("{pictures}: {e}")))?;
    let image = bm
        .images
        .get(i)
        .ok_or_else(|| Error::Ui(format!("{pictures} has no image {i}")))?;
    let px = bm
        .pixels(ui, i)
        .map_err(|e| Error::Ui(format!("{pictures} [{i}]: {e}")))?;
    let levels = h1_assets::bitmaps::decode::decode_image(image, px)
        .map_err(|e| Error::Ui(format!("{pictures} [{i}]: {e}")))?;
    let top = levels.first().ok_or(Error::Ui("no level".into()))?;
    let (w, h) = (PICTURE.0.min(top.width), PICTURE.1.min(top.height));
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let at = (y * top.width * 4) as usize;
        rgba.extend_from_slice(&top.rgba[at..at + (w * 4) as usize]);
    }
    Ok(Card {
        name,
        description,
        rgba,
        width: w,
        height: h,
    })
}

/// One line of a menu string: its lines joined (a word broken with a
/// hyphen rejoined), a blank line a sentence break.
pub fn tidy(s: &str) -> String {
    let mut out = String::new();
    for para in s.split("\r\n\r\n").map(str::trim).filter(|p| !p.is_empty()) {
        let mut text = String::new();
        for line in para.split("\r\n").map(str::trim) {
            if line.is_empty() {
                continue;
            }
            if text.ends_with('-') {
                // A broken word ("Cove-nant") or a hyphenated one
                // ("Hydro-Processing").
                if line.starts_with(|c: char| c.is_lowercase()) {
                    text.pop();
                }
            } else if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(line);
        }
        if !out.is_empty() {
            if !out.ends_with(['.', '!', '?']) {
                out.push('.');
            }
            out.push(' ');
        }
        out.push_str(&text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_strings_read_as_one_line() {
        assert_eq!(
            tidy("The Quick \r\nand the Dead\r\n\r\n4-16 players\r\n\r\nSupports vehicles"),
            "The Quick and the Dead. 4-16 players. Supports vehicles"
        );
        assert_eq!(
            tidy("Reactivate the wea-\r\npon at the heart of \r\nHalo...and learn the \r\ntruth."),
            "Reactivate the weapon at the heart of Halo...and learn the truth."
        );
        assert_eq!(tidy("The Pillar of \r\nAutumn"), "The Pillar of Autumn");
        assert_eq!(
            tidy("Covenant Hydro-\r\nProcessing Center\r\n\r\n4-8 players"),
            "Covenant Hydro-Processing Center. 4-8 players"
        );
        assert_eq!(tidy("Blood Gulch"), "Blood Gulch");
    }

    #[test]
    fn every_listed_map_converts() {
        for m in MP_ORDER {
            assert!(crate::convert::known(m).is_some(), "{m}");
        }
    }
}
