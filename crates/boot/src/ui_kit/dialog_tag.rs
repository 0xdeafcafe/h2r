//! The modal dialog in game, as far as it can be made without a run
//! (docs/game/ui-builder.md "The modal dialog in game"): the tag pieces a
//! dialog's [`View`] needs, the tag copy that holds them, and the screen
//! request that opens it the way the game opens its own dialogs.
//!
//! **Pieces.** [`pieces`] turns a dialog view into what a tag must carry:
//! header layout 2 in the root flags (the title and the legend are the
//! header's), the panel's and cells' retail bitmap blocks
//! ([`list_class::PictureSpec`]s, found at run time through the screen
//! list), and a text block for the body, the status line and each cell's
//! digit. The alpha each one shows with is kept beside it: a block has no
//! alpha, so the dimmed cells are set on their widgets after the pane is
//! built (`+0x64`, as `list_class` tints do).
//!
//! **The tag copy.** [`write_tag`] writes a `wgit` in guest memory from the
//! Controls tag our classes are built with: its root with the header
//! layout bits set to layout 2, one pane holding only our bitmaps, our text
//! blocks and the one list, and no other block (the Controls pane's own
//! texts and pictures are not a dialog's).
//!
//! **Opening.** The game's dialogs (`0x19b527`, `0x19b5af`) fill their
//! request with window kind 1, not the 5 `0x149f1e` uses, and call the
//! loader themselves. The push (`0x147f6d`) picks the screen stack by that
//! kind: 5 is the main menu's stack, 1 the one dialogs go on over it.
//! [`request`] writes that request; our class loader passes `+4` and `+8`
//! to the Controls constructor and the request to the push, as the
//! retail loaders do.

use engine::memory::{GuestMemory, MemFault};

use crate::list_class::{
    self, BITMAP_BLOCK_SIZE, LIST_BLOCK_SIZE, ListLayout, PictureSpec, SCREEN_ROOT_SIZE,
    TEXT_BLOCK_SIZE, TextBlock,
};
use crate::ui_kit::view::{Art, Justify, Kind, View};

/// A `wgit` root's flags (`+0`): bits 4, 3 and 0 pick header layouts 1, 2
/// and 3 (`0x22fda6`, `tags::ui::Screen::header_layout`).
pub const LAYOUT_BITS: u32 = 0x19;
/// Header layout 2: the large dialog's title and legend rectangles.
pub const LAYOUT_2: u32 = 0x08;

/// A screen request's size, and the window kinds the push (`0x147f6d`)
/// switches on at `+4`.
pub const REQUEST_SIZE: u32 = 0x20;
/// What `0x149f1e` (`OPEN_SCREEN`) asks for: the main menu's screens.
pub const MAIN_WINDOW: u32 = 5;
/// What the game's dialogs ask for (`0x19b527`, `0x19b5af`).
pub const DIALOG_WINDOW: u32 = 1;
/// The request's `+8`: 4 from `0x149f1e` and from the dialogs alike.
pub const REQUEST_B: u32 = 4;

/// One bitmap of the dialog's pane: the retail block, where it goes, and
/// the alpha its widget shows with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtPiece {
    pub id: &'static str,
    pub index: usize,
    pub spec: PictureSpec,
    pub alpha: u8,
}

/// One text block of ours: the view widget's id, the block, its
/// justification, and the alpha its widget shows with.
#[derive(Clone, Debug, PartialEq)]
pub struct TextPiece {
    pub id: String,
    pub block: TextBlock,
    pub justify: Justify,
    pub alpha: u8,
}

/// What a dialog's tag has to carry.
#[derive(Clone, Debug, PartialEq)]
pub struct Pieces {
    /// The root flags' header layout bits.
    pub layout: u32,
    /// The panel and the cells, in draw order.
    pub art: Vec<ArtPiece>,
    /// The body, the cells' digits and the status, in the view's order.
    pub texts: Vec<TextPiece>,
}

impl Pieces {
    /// The tag copy's size: root, pane, list, bitmaps and texts.
    pub fn tag_size(&self) -> u32 {
        SCREEN_ROOT_SIZE
            + list_class::PANE_SIZE
            + LIST_BLOCK_SIZE
            + self.art.len() as u32 * BITMAP_BLOCK_SIZE
            + self.texts.len() as u32 * TEXT_BLOCK_SIZE
    }
}

/// The header draws the title, and the footer widget the legend; every
/// other text of the view is a block of ours.
const HEADER_TEXTS: [&str; 2] = ["title", "legend"];

/// What dialog view `v` needs in its tag (see the module docs). Art that
/// is not a retail screen's (a list skin's) has no block to copy and is
/// left out, as no dialog view has any.
pub fn pieces(v: &View) -> Pieces {
    let mut art = Vec::new();
    let mut texts = Vec::new();
    for w in &v.widgets {
        let [top, left, bottom, right] = w.rect;
        match &w.kind {
            Kind::Art {
                art:
                    Art::Screen {
                        screen_id,
                        pane,
                        index,
                        image,
                    },
                ..
            } => art.push(ArtPiece {
                id: if w.id.starts_with("cell.") {
                    "cell"
                } else {
                    "panel"
                },
                index: art.len(),
                spec: PictureSpec {
                    screen_id: *screen_id as u32,
                    pane: u32::from(*pane),
                    bitmap: u32::from(*index),
                    image: *image,
                    at: [left, top],
                    animation: 0,
                },
                alpha: w.alpha,
            }),
            Kind::Art { .. } => {}
            Kind::Text {
                font,
                justify,
                colour,
                ..
            } if !HEADER_TEXTS.contains(&w.id.as_str()) => texts.push(TextPiece {
                id: w.id.clone(),
                block: TextBlock {
                    rect: [top, left, bottom, right],
                    font: *font,
                    colour: colour.map(|c| f32::from(c) / 255.0),
                    animation: 0,
                },
                justify: *justify,
                alpha: w.alpha,
            }),
            Kind::Text { .. } => {}
        }
    }
    Pieces {
        layout: LAYOUT_2,
        art,
        texts,
    }
}

/// Text block flag bits for a justification (`tags::ui::justification`:
/// bit 0 left, bit 1 right, neither centred).
pub fn justify_flags(j: Justify) -> u32 {
    match j {
        Justify::Left => 1,
        Justify::Right => 2,
        Justify::Centre => 0,
    }
}

/// A text block's string id (`+0x24`, looked up through the screen's
/// strings, so a locale source of ours serves it).
const TEXT_STRING: usize = 0x24;

/// Text piece `t` as a block showing string id `string`.
pub fn text_block(t: &TextPiece, string: u32) -> [u8; TEXT_BLOCK_SIZE as usize] {
    let mut b = t.block.bytes();
    b[0..4].copy_from_slice(&justify_flags(t.justify).to_le_bytes());
    b[TEXT_STRING..TEXT_STRING + 4].copy_from_slice(&string.to_le_bytes());
    b
}

/// A pane's blocks (count, address), each 8 bytes: buttons, lists, a
/// reserved one, texts, bitmaps, model scenes, a reserved one, shader
/// bitmaps, tables (`tags::ui::Pane`).
const PANE_BLOCKS: [u32; 9] = [0x04, 0x0c, 0x14, 0x1c, 0x24, 0x2c, 0x34, 0x3c, 0x44];
const PANE_LISTS: u32 = 0x0c;
const PANE_TEXTS: u32 = 0x1c;
const PANE_BITMAPS: u32 = 0x24;
const LIST_SKIN: u32 = 0x04;
const LIST_VISIBLE: u32 = 0x06;
const LIST_ANCHOR: u32 = 0x08;

/// Writes at `at` the dialog's tag (see the module docs) from the retail
/// Controls tag at `retail`: its root with `pieces.layout` for the header
/// layout bits, one pane with the retail pane's flags and animation, the
/// first retail list placed by `list`, `art` (the blocks of
/// `pieces.art`, found through the screen list and placed), and the text
/// blocks of `pieces.texts` showing `strings`, one each. `None` when the
/// retail tag has no pane or no list, or `art` and `strings` don't match
/// the pieces.
pub fn write_tag(
    m: &mut dyn GuestMemory,
    retail: u32,
    at: u32,
    pieces: &Pieces,
    art: &[[u8; BITMAP_BLOCK_SIZE as usize]],
    strings: &[u32],
    list: &ListLayout,
) -> Result<Option<u32>, MemFault> {
    if art.len() != pieces.art.len() || strings.len() != pieces.texts.len() {
        return Ok(None);
    }
    let (panes, pane_from) = (
        m.u32(retail + list_class::SCREEN_PANES)?,
        m.u32(retail + list_class::SCREEN_PANES + 4)?,
    );
    if panes == 0 {
        return Ok(None);
    }
    let (lists, list_from) = (
        m.u32(pane_from + PANE_LISTS)?,
        m.u32(pane_from + PANE_LISTS + 4)?,
    );
    if lists == 0 {
        return Ok(None);
    }
    let copy = |m: &mut dyn GuestMemory, from: u32, to: u32, len: u32| -> Result<(), MemFault> {
        let mut b = vec![0u8; len as usize];
        m.read(from, &mut b)?;
        m.write(to, &b)
    };
    let pane = at + SCREEN_ROOT_SIZE;
    let list_at = pane + list_class::PANE_SIZE;
    let bitmaps = list_at + LIST_BLOCK_SIZE;
    let texts = bitmaps + art.len() as u32 * BITMAP_BLOCK_SIZE;

    copy(m, retail, at, SCREEN_ROOT_SIZE)?;
    let flags = m.u32(at)?;
    m.set_u32(at, (flags & !LAYOUT_BITS) | pieces.layout)?;
    m.set_u32(at + list_class::SCREEN_PANES, 1)?;
    m.set_u32(at + list_class::SCREEN_PANES + 4, pane)?;

    m.write(pane, &[0u8; list_class::PANE_SIZE as usize])?;
    copy(m, pane_from, pane, 4)?;
    debug_assert!(PANE_BLOCKS.iter().all(|o| o + 8 <= list_class::PANE_SIZE));

    copy(m, list_from, list_at, LIST_BLOCK_SIZE)?;
    m.set_u16(list_at + LIST_SKIN, list.skin as u16)?;
    if list.visible > 0 {
        m.set_u16(list_at + LIST_VISIBLE, list.visible as u16)?;
    }
    m.set_u16(list_at + LIST_ANCHOR, list.anchor[0] as u16)?;
    m.set_u16(list_at + LIST_ANCHOR + 2, list.anchor[1] as u16)?;
    m.set_u32(pane + PANE_LISTS, 1)?;
    m.set_u32(pane + PANE_LISTS + 4, list_at)?;

    for (i, b) in art.iter().enumerate() {
        m.write(bitmaps + i as u32 * BITMAP_BLOCK_SIZE, b)?;
    }
    m.set_u32(pane + PANE_BITMAPS, art.len() as u32)?;
    m.set_u32(pane + PANE_BITMAPS + 4, bitmaps)?;

    for (i, (t, &s)) in pieces.texts.iter().zip(strings).enumerate() {
        m.write(texts + i as u32 * TEXT_BLOCK_SIZE, &text_block(t, s))?;
    }
    m.set_u32(pane + PANE_TEXTS, pieces.texts.len() as u32)?;
    m.set_u32(pane + PANE_TEXTS + 4, texts)?;
    Ok(Some(at))
}

/// A screen request as `0x149f49` fills it: `+0` a zero word, `+2` the
/// player mask, `+4` the window kind, `+8` [`REQUEST_B`], `+0xc` zero,
/// `+0x10` three words of -1 (no source, EDX 0), `+0x1c` the loader.
pub fn request(window: u32, mask: u16, loader: u32) -> [u8; REQUEST_SIZE as usize] {
    let mut r = [0u8; REQUEST_SIZE as usize];
    r[2..4].copy_from_slice(&mask.to_le_bytes());
    r[4..8].copy_from_slice(&window.to_le_bytes());
    r[8..12].copy_from_slice(&REQUEST_B.to_le_bytes());
    r[0x10..0x1c].fill(0xff);
    r[0x1c..0x20].copy_from_slice(&loader.to_le_bytes());
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::dialog::{
        BODY_TOP, CELL_TOP, Cells, DialogGeometry, DialogSpec, LARGE_SCREEN_ID, PASSWORD_CELL,
        PASSWORD_SCREEN_ID, dialog_view,
    };
    use crate::ui_kit::input::{Glyph, Prompt};
    use crate::ui_kit::view::{FRAME_DIM, Opener, Synthetic};
    use engine::testing::FakeGuest;

    /// The sign-in code page with an error showing.
    fn view() -> View {
        let mut s = crate::ui_kit::spec::ScreenSpec::new("signin-code", "Sign in as Chief");
        s.prompts = vec![
            Prompt::new(&[Glyph::A], "Sign in"),
            Prompt::new(&[Glyph::B], "Cancel"),
        ];
        let d = DialogSpec {
            body: "Enter the 6-digit code.".into(),
            cells: Some(Cells {
                text: "123456".chars().map(String::from).collect(),
                cursor: Some(2),
                gap_after: Some(2),
            }),
            status: Some("That code didn't work.".into()),
            opener: Opener::MAIN_MENU,
        };
        dialog_view(&s, &d, &DialogGeometry::fallback(), &Synthetic, "code")
    }

    #[test]
    fn the_code_page_needs_layout_2_ten_bitmaps_and_eight_texts() {
        let p = pieces(&view());
        assert_eq!(p.layout, LAYOUT_2);
        let panel: Vec<_> = p.art.iter().filter(|a| a.id == "panel").collect();
        let cells: Vec<_> = p.art.iter().filter(|a| a.id == "cell").collect();
        assert_eq!(panel.len(), 4);
        assert!(
            panel
                .iter()
                .all(|a| a.spec.screen_id == LARGE_SCREEN_ID as u32)
        );
        assert_eq!(
            panel.iter().map(|a| a.spec.bitmap).collect::<Vec<_>>(),
            [0, 1, 2, 3],
            "the tag's draw order"
        );
        assert_eq!(cells.len(), 6);
        for c in &cells {
            assert_eq!(c.spec.screen_id, PASSWORD_SCREEN_ID as u32);
            assert_eq!(c.spec.bitmap, u32::from(PASSWORD_CELL));
            assert_eq!(c.spec.at[1], CELL_TOP);
        }
        assert_eq!(
            cells.iter().filter(|c| c.alpha == 255).count(),
            1,
            "the cursor's cell is lit"
        );
        assert!(
            cells
                .iter()
                .filter(|c| c.alpha != 255)
                .all(|c| c.alpha == FRAME_DIM)
        );
        let ids: Vec<_> = p.texts.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "body",
                "cell.0.text",
                "cell.1.text",
                "cell.2.text",
                "cell.3.text",
                "cell.4.text",
                "cell.5.text",
                "status"
            ],
            "the title and the legend are the header's"
        );
        assert_eq!(p.texts[0].block.rect[0], BODY_TOP);
        assert_eq!(p.texts[0].justify, Justify::Left);
        assert_eq!(p.texts[1].justify, Justify::Centre);
        assert_eq!(p.tag_size(), 0x68 + 0x4c + 0x18 + 10 * 0x38 + 8 * 0x2c);
    }

    #[test]
    fn text_blocks_carry_justification_and_string() {
        let p = pieces(&view());
        let b = text_block(&p.texts[1], 0xdead_0001);
        let flags = u32::from_le_bytes(b[0..4].try_into().unwrap());
        assert_eq!(tags::ui::justification(flags), 2, "centred");
        assert_eq!(tags::ui::justification(justify_flags(Justify::Right)), 1);
        assert_eq!(tags::ui::justification(justify_flags(Justify::Left)), 0);
        assert_eq!(
            u32::from_le_bytes(b[0x24..0x28].try_into().unwrap()),
            0xdead_0001
        );
        assert_eq!(&b[0x1c..0x1e], &p.texts[1].block.rect[0].to_le_bytes());
    }

    const RETAIL: u32 = 0x0010_0000;
    const RETAIL_PANE: u32 = 0x0010_0100;
    const RETAIL_LIST: u32 = 0x0010_0200;
    const AT: u32 = 0x0020_0000;

    /// A Controls-like tag: header layout 0 plus bit 5, one pane with a
    /// list, two texts, a bitmap and a model scene.
    fn retail() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(RETAIL, 0x1000).map(AT, 0x1000);
        g.put_u32(RETAIL, 0x20 | 0x10)
            .put_u16(RETAIL + 4, 39)
            .put_u32(RETAIL + 0x20, 1)
            .put_u32(RETAIL + 0x24, RETAIL_PANE)
            .put_u32(RETAIL + 0x2c, 0x1234_5678);
        g.put_u16(RETAIL_PANE, 3)
            .put_u16(RETAIL_PANE + 2, 7)
            .put_u32(RETAIL_PANE + 0x0c, 1)
            .put_u32(RETAIL_PANE + 0x10, RETAIL_LIST)
            .put_u32(RETAIL_PANE + 0x1c, 2)
            .put_u32(RETAIL_PANE + 0x20, 0x0010_0800)
            .put_u32(RETAIL_PANE + 0x24, 1)
            .put_u32(RETAIL_PANE + 0x28, 0x0010_0900)
            .put_u32(RETAIL_PANE + 0x2c, 1)
            .put_u32(RETAIL_PANE + 0x30, 0x0010_0a00);
        g.put_u32(RETAIL_LIST, 0xaa)
            .put_u16(RETAIL_LIST + 4, 1)
            .put_u16(RETAIL_LIST + 6, 6)
            .put_u32(RETAIL_LIST + 0x10, 0xbb);
        g
    }

    const LIST: ListLayout = ListLayout {
        skin: 0,
        anchor: [-100, -120],
        bitmaps_left_of: i16::MAX,
        visible: 1,
    };

    #[test]
    fn the_tag_copy_holds_only_the_dialogs_blocks() {
        let p = pieces(&view());
        let art: Vec<_> = (0..p.art.len())
            .map(|i| [i as u8 + 1; BITMAP_BLOCK_SIZE as usize])
            .collect();
        let strings: Vec<_> = (0..p.texts.len() as u32).map(|i| 0x5000 + i).collect();
        let mut g = retail();
        assert_eq!(
            write_tag(&mut g, RETAIL, AT, &p, &art, &strings, &LIST),
            Ok(Some(AT))
        );
        let m: &dyn GuestMemory = &g;
        let u32_at = |a| m.u32(a).unwrap();
        let u16_at = |a| m.u16(a).unwrap();
        assert_eq!(u32_at(AT), 0x20 | LAYOUT_2, "layout 2, the other bits kept");
        assert_eq!(u16_at(AT + 4), 39);
        assert_eq!(u32_at(AT + 0x2c), 0x1234_5678, "the header string stays");
        assert_eq!(u32_at(AT + 0x20), 1);
        let pane = u32_at(AT + 0x24);
        assert_eq!(pane, AT + 0x68);
        assert_eq!(
            (u16_at(pane), u16_at(pane + 2)),
            (3, 7),
            "flags and animation"
        );
        for o in [0x04, 0x14, 0x2c, 0x34, 0x3c, 0x44] {
            assert_eq!(u32_at(pane + o), 0, "block at {o:#x} empty");
        }
        assert_eq!(u32_at(pane + 0x0c), 1);
        let list = u32_at(pane + 0x10);
        assert_eq!(u32_at(list), 0xaa);
        assert_eq!(u32_at(list + 0x10), 0xbb);
        assert_eq!(u16_at(list + 6), 1);
        assert_eq!(
            [u16_at(list + 8) as i16, u16_at(list + 10) as i16],
            LIST.anchor
        );
        assert_eq!(u32_at(pane + 0x24), 10);
        let bitmaps = u32_at(pane + 0x28);
        for i in 0..10u32 {
            assert_eq!(
                m.u8(bitmaps + i * BITMAP_BLOCK_SIZE + 0x37).unwrap(),
                i as u8 + 1
            );
        }
        assert_eq!(u32_at(pane + 0x1c), 8);
        let texts = u32_at(pane + 0x20);
        for (i, t) in p.texts.iter().enumerate() {
            let mut b = [0u8; TEXT_BLOCK_SIZE as usize];
            m.read(texts + i as u32 * TEXT_BLOCK_SIZE, &mut b).unwrap();
            assert_eq!(b, text_block(t, 0x5000 + i as u32), "{}", t.id);
        }
        assert!(texts + 8 * TEXT_BLOCK_SIZE == AT + p.tag_size());
    }

    #[test]
    fn a_tag_copy_needs_a_block_per_piece_and_a_retail_list() {
        let p = pieces(&view());
        let art = vec![[0u8; BITMAP_BLOCK_SIZE as usize]; p.art.len()];
        let strings = vec![0; p.texts.len()];
        let mut g = retail();
        assert_eq!(
            write_tag(&mut g, RETAIL, AT, &p, &art[1..], &strings, &LIST),
            Ok(None)
        );
        assert_eq!(
            write_tag(&mut g, RETAIL, AT, &p, &art, &strings[1..], &LIST),
            Ok(None)
        );
        g.put_u32(RETAIL_PANE + 0x0c, 0);
        assert_eq!(
            write_tag(&mut g, RETAIL, AT, &p, &art, &strings, &LIST),
            Ok(None)
        );
        g.put_u32(RETAIL + 0x20, 0);
        assert_eq!(
            write_tag(&mut g, RETAIL, AT, &p, &art, &strings, &LIST),
            Ok(None)
        );
    }

    /// `0x149f1e` and the dialogs differ only in the window kind.
    #[test]
    fn a_dialog_request_differs_from_open_screens_in_the_window() {
        let menu = request(MAIN_WINDOW, 0b10, 0x002b_7714);
        let dialog = request(DIALOG_WINDOW, 0b10, 0x002b_7714);
        let word = |r: &[u8; 32], o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
        assert_eq!(&menu[0..4], &[0, 0, 2, 0], "zero word, then the mask");
        assert_eq!(word(&menu, 4), 5);
        assert_eq!(word(&dialog, 4), 1);
        for r in [&menu, &dialog] {
            assert_eq!(word(r, 8), 4);
            assert_eq!(word(r, 0xc), 0);
            assert_eq!([word(r, 0x10), word(r, 0x14), word(r, 0x18)], [u32::MAX; 3]);
            assert_eq!(word(r, 0x1c), 0x002b_7714);
        }
        let differ: Vec<_> = (0..32).filter(|&i| menu[i] != dialog[i]).collect();
        assert_eq!(differ, [4]);
    }
}
