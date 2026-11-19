//! Where a mod's page puts things (docs/game/maps.md "A mod's page"): the
//! pregame lobby's layout (`pregame_lobby`, screen 14), the retail screen
//! that shows a chosen map's details. In the lobby only Start Game and
//! Game Setup at the top are selectable; everything under them (the
//! status text on its panel, the game's settings, the map's picture, the
//! two-column player table on the right) is information. A mod's page is
//! the same: its one action where the lobby's buttons are, then the mod's
//! words on the lobby's panel, the base map's picture under them, and a
//! table of facts and the checklist of what it changes in the lobby's
//! table column.
//!
//! Every rectangle, font, colour, animation and depth bias below is the
//! lobby tag's own (pane 0, read off `mainmenu.map`), unless its comment
//! says how it moved. The disc test holds them to the tag.

use crate::list_class::{ListLayout, Panel, PanelText, PictureSpec, TextBlock};

/// The pregame lobby's screen id.
pub const LOBBY_SCREEN_ID: u32 = 14;

/// The lobby's text colours: white for its labels and names, the pale
/// blue of its status and values (and of every retail description), and
/// the lighter blue of the Live menu's hints.
pub const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
pub const PALE_BLUE: [f32; 3] = [0.388235, 0.580392, 0.756863];
pub const HINT_BLUE: [f32; 3] = [0.678431, 0.764706, 0.847059];

/// The lobby's texts' animation (11) and depth biases (5; its table's
/// left column 6).
pub const TEXT_ANIMATION: u16 = 11;
pub const TEXT_BIAS: i16 = 5;
pub const LABEL_BIAS: i16 = 6;

/// The one action (Download or Remove) where the lobby's buttons are
/// (`[173 -274 153 -118]`, on `name_field`): a skin-0 item, whose bar is at
/// +(10, 6) and 21 high, sits on 153..174 from an anchor at y 147; its
/// left is the lobby status text's (-258), less the skin's 20 (its frame
/// cap then stays inside the title-safe area). One row, no frame art.
pub const LIST: ListLayout = ListLayout {
    skin: 0,
    anchor: [-262, 147],
    bitmaps_left_of: i16::MIN,
    visible: 1,
};

/// A lobby bitmap (pane 0) as a picture spec.
const fn lobby(bitmap: u32, image: i16, at: [i16; 2], animation: u16) -> PictureSpec {
    PictureSpec {
        screen_id: LOBBY_SCREEN_ID,
        pane: 0,
        bitmap,
        image,
        at,
        animation,
    }
}

/// The lobby's backings, at their own places: `name_field` (338 by 35)
/// under the buttons, and `lobby_display` (512 by 128) under the status.
pub const NAME_FIELD: PictureSpec = lobby(0, 0, [-279, 174], 11);
pub const NAME_FIELD_SIZE: [u16; 2] = [338, 35];
pub const LOBBY_DISPLAY: PictureSpec = lobby(1, 0, [-283, 149], 3);
pub const LOBBY_DISPLAY_SIZE: [u16; 2] = [512, 128];

/// The map's picture: the lobby's own picture widget (`unknown_map`, 220
/// by 207, animation 14), whose bitmap the page points at the base map's
/// slot picture as the list's does. Moved from the lobby's (-152, 32) to
/// the left edge of the column (-258, 30): in the lobby the settings lines
/// run over the picture's left; here nothing does.
pub const PICTURE: PictureSpec = lobby(2, 0, [-258, 30], 14);
pub const PICTURE_SIZE: [u16; 2] = [220, 207];

/// The panel art, backings first ([`crate::list_class::ListClass::panel`]).
pub const ART: [PictureSpec; 2] = [NAME_FIELD, LOBBY_DISPLAY];
pub const ART_SIZES: [[u16; 2]; 2] = [NAME_FIELD_SIZE, LOBBY_DISPLAY_SIZE];

const fn text(rect: [i16; 4], colour: [f32; 3]) -> TextBlock {
    TextBlock {
        rect,
        font: 1,
        colour,
        animation: TEXT_ANIMATION,
    }
}

/// The mod's own words: the lobby's status text (`[135 -258 61 45]`,
/// pale blue), with room for three lines above the action hint.
pub const BODY: TextBlock = text([135, -258, 78, 45], PALE_BLUE);
/// What the action does now (the focused row's description, set by the
/// class): one line under the body, on the lobby panel where its "open"
/// line is, in the Live menu's hint blue.
pub const STATUS: TextBlock = text([75, -258, 56, 45], HINT_BLUE);

/// The table column: the lobby's player rows, 22 apart from the top one
/// `[177 118 157 276]`, its names white and left (bias 6) and its values
/// pale blue and right (bias 5). Widened left into the gutter so each
/// cell has its own box. Right edge 272, the title-safe area's
/// (the lobby's runs to 276 and 279).
pub const TABLE_LEFT: i16 = 83;
pub const VALUE_LEFT: i16 = 163;
pub const TABLE_RIGHT: i16 = 272;
pub const TABLE_TOP: i16 = 177;
pub const ROW_STEP: i16 = 22;
pub const ROW_HEIGHT: i16 = 20;
/// Rows of facts (Map, By, Size, ...), then the checklist's heading, then
/// its rows of two.
pub const FACT_ROWS: usize = 10;
pub const CHECK_ROWS: usize = 4;
/// The checklist's rows run wider than the table, from right of the map's
/// picture (which ends at -38): two columns of conduit-12 `[x] Geometry`
/// need about 90 each, and the table's 154 holds two of 75.
pub const CHECK_LEFT: i16 = 40;
/// Where the checklist's second column starts.
pub const CHECK_SPLIT: i16 = 157;

/// Row `k` of the table column's rectangle.
pub const fn row_rect(k: usize) -> [i16; 4] {
    let top = TABLE_TOP - ROW_STEP * k as i16;
    [top, TABLE_LEFT, top - ROW_HEIGHT, TABLE_RIGHT]
}

/// The panel texts' slots: the body, a label and a value per fact row,
/// the checklist's heading, two columns per checklist row.
pub const BODY_TEXT: usize = 0;
pub const fn fact_label(k: usize) -> usize {
    1 + 2 * k
}
pub const fn fact_value(k: usize) -> usize {
    2 + 2 * k
}
pub const CHECK_HEADING: usize = 1 + 2 * FACT_ROWS;
pub const fn check_cell(row: usize, column: usize) -> usize {
    CHECK_HEADING + 1 + 2 * row + column
}
pub const TEXTS: usize = CHECK_HEADING + 1 + 2 * CHECK_ROWS;

/// Panel text `i`'s block.
pub fn panel_text(i: usize) -> PanelText {
    let left = |rect, colour, bias| PanelText {
        block: text(rect, colour),
        right: false,
        bias,
    };
    if i == BODY_TEXT {
        return left(BODY.rect, BODY.colour, TEXT_BIAS);
    }
    if i < CHECK_HEADING {
        let k = (i - 1) / 2;
        let [top, l, bottom, r] = row_rect(k);
        return if i == fact_label(k) {
            left([top, l, bottom, VALUE_LEFT - 5], WHITE, LABEL_BIAS)
        } else {
            PanelText {
                block: text([top, VALUE_LEFT, bottom, r], PALE_BLUE),
                right: true,
                bias: TEXT_BIAS,
            }
        };
    }
    if i == CHECK_HEADING {
        let [top, _, bottom, right] = row_rect(FACT_ROWS);
        return left([top, CHECK_LEFT, bottom, right], WHITE, LABEL_BIAS);
    }
    let cell = i - CHECK_HEADING - 1;
    let (row, column) = (cell / 2, cell % 2);
    let [top, _, bottom, r] = row_rect(FACT_ROWS + 1 + row);
    let rect = if column == 0 {
        [top, CHECK_LEFT, bottom, CHECK_SPLIT - 5]
    } else {
        [top, CHECK_SPLIT, bottom, r]
    };
    left(rect, PALE_BLUE, TEXT_BIAS)
}

/// The page's panel: the lobby's backings and every text block.
pub fn panel() -> Panel {
    Panel {
        art: ART.to_vec(),
        texts: (0..TEXTS).map(panel_text).collect(),
    }
}

/// The ids the view and the checks give panel text `i`.
pub fn text_id(i: usize) -> String {
    if i == BODY_TEXT {
        "page.body".into()
    } else if i < CHECK_HEADING {
        let k = (i - 1) / 2;
        if i == fact_label(k) {
            format!("page.fact.{k}.label")
        } else {
            format!("page.fact.{k}.value")
        }
    } else if i == CHECK_HEADING {
        "page.check.heading".into()
    } else {
        let cell = i - CHECK_HEADING - 1;
        format!("page.check.{}.{}", cell / 2, cell % 2)
    }
}
