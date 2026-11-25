//! A retail screen as the editor's edits of its tag leave it
//! (docs/game/ui-builder.md "Editing in the editor", "Retail tag edits").
//!
//! The picture of a retail screen is drawn from the disc's map. The
//! editor's edits live in the tag document of the user's map, so the
//! editor reads the edited pane's blocks back from that document
//! ([`RetailBlocks::decode`]) and hands them over as [`RetailEdits`]:
//! the pane is drawn with those blocks in place of the disc's. That
//! covers every block edit at once: a moved rect, a block parked off
//! screen ([`parked`]), a block the tag gained (a copy), a string ID or
//! a bitmap reference set.
//!
//! A string ID the screen's string table has no text for (one of ours,
//! which the game's code answers at run time) draws the stand-in text the
//! editor holds for it ([`RetailEdits::texts`]).

use std::collections::BTreeMap;

use super::{RetailPlaces, place_retail, tui};

/// Where the game parks a spare block: off screen, where neither the
/// game nor [`super::Disc::render_retail`] draws it.
pub const PARKED: i16 = 9999;

/// `rect` parked off screen as the disc's spare blocks are: a rect keeps
/// its left and right and takes [`PARKED`] as top and bottom, a point
/// (`point`) goes to `PARKED, PARKED` and keeps its size.
pub fn parked(rect: [i16; 4], point: bool) -> [i16; 4] {
    let [t, l, b, r] = rect;
    if point {
        let (w, h) = (r.saturating_sub(l), t.saturating_sub(b));
        [PARKED, PARKED, PARKED.saturating_sub(h), PARKED.saturating_add(w)]
    } else {
        [PARKED, l, PARKED, r]
    }
}

/// The blocks of one pane that the editor edits as whole elements, and
/// the screen's header string ID, as the edited tag has them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RetailBlocks {
    /// The screen's `header` string ID.
    pub header: u32,
    pub text_blocks: Vec<tui::TextBlock>,
    pub buttons: Vec<tui::Button>,
    pub bitmaps: Vec<tui::BitmapBlock>,
}

impl RetailBlocks {
    /// The blocks from their elements' bytes, each slice a whole number
    /// of elements (a short tail is dropped). References are taken as
    /// stored: the drawing looks them up in the disc's map and skips one
    /// it lacks.
    pub fn decode(header: u32, text_blocks: &[u8], buttons: &[u8], bitmaps: &[u8]) -> Self {
        RetailBlocks {
            header,
            text_blocks: text_blocks
                .chunks_exact(tui::TextBlock::SIZE)
                .filter_map(tui::TextBlock::from_element)
                .collect(),
            buttons: buttons
                .chunks_exact(tui::Button::SIZE)
                .filter_map(tui::Button::from_element)
                .collect(),
            bitmaps: bitmaps
                .chunks_exact(tui::BitmapBlock::SIZE)
                .filter_map(tui::BitmapBlock::from_element)
                .collect(),
        }
    }
}

/// The editor's edits of one pane of a retail screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RetailEdits {
    /// Widgets moved, by id (lists and tables move this way only).
    pub places: RetailPlaces,
    /// The pane's edited blocks; none draws the disc's.
    pub blocks: Option<RetailBlocks>,
    /// Stand-in texts by string ID, for IDs the game's code answers.
    pub texts: BTreeMap<u32, String>,
}

impl RetailEdits {
    /// Moves alone.
    pub fn placed(places: &RetailPlaces) -> Self {
        RetailEdits {
            places: places.clone(),
            ..Self::default()
        }
    }

    /// Whether nothing is edited.
    pub fn is_empty(&self) -> bool {
        self.places.is_empty() && self.blocks.is_none() && self.texts.is_empty()
    }

    /// `pane` as edited, and the header's string ID (`header` is the
    /// disc's).
    pub(super) fn apply(&self, pane: &mut tui::Pane, header: u32) -> u32 {
        let header = match &self.blocks {
            Some(b) => {
                pane.text_blocks = b.text_blocks.clone();
                pane.buttons = b.buttons.clone();
                pane.bitmaps = b.bitmaps.clone();
                b.header
            }
            None => header,
        };
        place_retail(pane, &self.places);
        header
    }

    /// The stand-in text of string ID `id`, if it has one.
    pub fn text(&self, id: u32) -> Option<String> {
        self.texts.get(&id).cloned()
    }
}

/// Splits a block widget's id (`text.3`) into its kind and index.
fn kind_index(id: &str) -> Option<(&str, usize)> {
    let (k, i) = id.split_once('.')?;
    Some((k, i.parse().ok()?))
}

/// The block of pane `pane` that widget `id` is an element of, and its
/// index: `("panes[0].text_blocks", 3)` for `text.3`. Texts, buttons and
/// bitmaps only: the blocks the editor copies and decodes.
pub fn retail_block(pane: usize, id: &str) -> Result<(String, usize), String> {
    let (name, index) = match kind_index(id) {
        Some(("text", i)) => ("text_blocks", i),
        Some(("button", i)) => ("buttons", i),
        Some(("bitmap", i)) => ("bitmaps", i),
        _ => {
            return Err(format!(
                "refused: {id} is not a text, button or bitmap block of the screen tag"
            ));
        }
    };
    Ok((format!("panes[{pane}].{name}"), index))
}

/// The string ID field of widget `id`: the screen's `header` for `title`,
/// a text block's `text`, a button's `label`.
pub fn retail_string_field(pane: usize, id: &str) -> Result<String, String> {
    if id == "title" {
        return Ok("header".to_owned());
    }
    match kind_index(id) {
        Some(("text", i)) => Ok(format!("panes[{pane}].text_blocks[{i}].text")),
        Some(("button", i)) => Ok(format!("panes[{pane}].buttons[{i}].label")),
        _ => Err(format!(
            "refused: {id} has no string ID (the title, a text block and a button have one)"
        )),
    }
}

/// The new widget's id when widget `id`'s block has `count` elements and
/// gains one at its end.
pub fn retail_copy_id(id: &str, count: usize) -> Option<String> {
    kind_index(id).map(|(k, _)| format!("{k}.{count}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element<const N: usize>(rect_at: usize, rect: [i16; 4], string_at: usize, string: u32) -> [u8; N] {
        let mut e = [0u8; N];
        for (i, v) in rect.into_iter().enumerate() {
            e[rect_at + 2 * i..rect_at + 2 * i + 2].copy_from_slice(&v.to_le_bytes());
        }
        e[string_at..string_at + 4].copy_from_slice(&string.to_le_bytes());
        e
    }

    fn pane() -> tui::Pane {
        tui::Pane {
            flags: 0,
            animation: tui::AnimationRef(0),
            buttons: Vec::new(),
            lists: Vec::new(),
            text_blocks: Vec::new(),
            bitmaps: Vec::new(),
            model_scenes: Vec::new(),
            shader_bitmaps: Vec::new(),
            tables: Vec::new(),
        }
    }

    #[test]
    fn a_parked_block_is_off_screen_and_keeps_its_size() {
        assert_eq!(parked([75, -260, 55, 70], false), [PARKED, -260, PARKED, 70]);
        // A point keeps the image's size, so the move is a move only.
        let p = parked([174, -279, 139, 59], true);
        assert_eq!([p[0], p[1]], [PARKED, PARKED]);
        assert_eq!((p[3] - p[1], p[0] - p[2]), (59 + 279, 174 - 139));
        assert!(!super::super::on_screen_rect(&tui::Rect {
            top: PARKED,
            left: -260,
            bottom: PARKED,
            right: 70
        }));
    }

    #[test]
    fn widgets_name_their_block_and_string_fields() {
        assert_eq!(
            retail_block(0, "text.45"),
            Ok(("panes[0].text_blocks".to_owned(), 45))
        );
        assert_eq!(retail_block(2, "button.1").unwrap().0, "panes[2].buttons");
        assert_eq!(retail_block(0, "bitmap.4").unwrap(), ("panes[0].bitmaps".to_owned(), 4));
        for id in ["title", "list.0", "table.0", "text.x", "row.0.label"] {
            let e = retail_block(0, id).unwrap_err();
            assert!(e.starts_with("refused"), "{id}: {e}");
        }
        assert_eq!(retail_string_field(0, "title").unwrap(), "header");
        assert_eq!(
            retail_string_field(0, "text.10").unwrap(),
            "panes[0].text_blocks[10].text"
        );
        assert_eq!(
            retail_string_field(1, "button.0").unwrap(),
            "panes[1].buttons[0].label"
        );
        assert!(retail_string_field(0, "bitmap.0").unwrap_err().starts_with("refused"));
        assert_eq!(retail_copy_id("text.10", 48).as_deref(), Some("text.48"));
        assert_eq!(retail_copy_id("button.1", 2).as_deref(), Some("button.2"));
        assert_eq!(retail_copy_id("title", 2), None);
    }

    #[test]
    fn edited_blocks_stand_in_for_the_discs() {
        // Two text blocks, the second a copy the disc's pane has not got,
        // and one button with its label set.
        let a: [u8; tui::TextBlock::SIZE] = element(0x1c, [75, -260, 55, 70], 0x24, 0x0400_0201);
        let b: [u8; tui::TextBlock::SIZE] = element(0x1c, [95, -260, 75, 70], 0x24, 0x0a00_2000);
        let k: [u8; tui::Button::SIZE] = element(0x1c, [173, -44, 153, 61], 0x30, 0x0a00_2001);
        let texts: Vec<u8> = [a, b].concat();
        let blocks = RetailBlocks::decode(0x0b00_2002, &texts, &k, &[]);
        assert_eq!(blocks.text_blocks.len(), 2);
        assert_eq!(blocks.buttons.len(), 1);
        // A short tail is not an element.
        assert_eq!(RetailBlocks::decode(0, &texts[..texts.len() - 1], &[], &[]).text_blocks.len(), 1);

        let mut places = RetailPlaces::new();
        places.insert("text.0".into(), [70, -250, 50, 80]);
        let edits = RetailEdits {
            places,
            blocks: Some(blocks),
            texts: BTreeMap::from([(0x0a00_2000, "X  Change Mode".to_owned())]),
        };
        assert!(!edits.is_empty());
        let mut p = pane();
        p.text_blocks = Vec::new();
        let header = edits.apply(&mut p, 0x0b00_01dd);
        assert_eq!(header, 0x0b00_2002);
        assert_eq!(p.text_blocks.len(), 2);
        // The places are applied over the blocks.
        assert_eq!(p.text_blocks[0].rect.left, -250);
        assert_eq!(p.text_blocks[1].rect.top, 95);
        assert_eq!(p.text_blocks[1].string.0, 0x0a00_2000);
        assert_eq!(p.buttons[0].string.0, 0x0a00_2001);
        assert_eq!(edits.text(0x0a00_2000).as_deref(), Some("X  Change Mode"));
        assert_eq!(edits.text(0x0400_0201), None);
    }

    #[test]
    fn moves_alone_keep_the_discs_blocks_and_header() {
        let mut p = pane();
        let a: [u8; tui::TextBlock::SIZE] = element(0x1c, [1, 2, 0, 9], 0x24, 7);
        p.text_blocks = RetailBlocks::decode(0, &a, &[], &[]).text_blocks;
        let mut places = RetailPlaces::new();
        places.insert("text.0".into(), [5, 6, 4, 13]);
        let edits = RetailEdits::placed(&places);
        assert_eq!(edits.apply(&mut p, 42), 42);
        assert_eq!(p.text_blocks[0].rect.top, 5);
        assert!(RetailEdits::default().is_empty());
    }
}
