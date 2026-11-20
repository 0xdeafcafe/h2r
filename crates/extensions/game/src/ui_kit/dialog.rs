//! Modal dialogs (docs/game/ui-builder.md "Screen kinds"): a panel over
//! the screen that opened it, as the game's own dialogs sit.
//!
//! The large dialog is `error_dialog_large`'s (screen 240, header layout
//! 2): its `ul_07`/`br_07` corners and `dialog1`/`dialog2` panel, the
//! title in the layout's header rectangle and the legend in its second
//! one. Code entry adds a row of the Live passcode dialog's cells
//! (`dialog_live_password`, screen 21, its `password` bitmap), a digit
//! over each, the cursor's cell lit and the rest dimmed. No retail dialog
//! tag draws anything behind its panel (no dim layer, no widget outside
//! the panel and corners), so a dialog's view is [`Backing::Over`] the
//! screen that opened it ([`Opener`]).
//!
//! The body, cell row and status line are ours, laid out inside the
//! panel between the header and the footer; [`DialogGeometry::fallback`]
//! is the disc's tags as `h2 ui` reads them, held to the disc by
//! `ui_kit::screens::tests::preview_and_check_against_the_disc`.

use serde::Serialize;
use tags::ui::Rect;

use crate::ui_kit::spec::{ListPlace, RowSpec, ScreenSpec, wrap};
use crate::ui_kit::view::{
    Art, Backing, FRAME_DIM, Geometry, Justify, Kind, Layer, ListAt, Metrics, Opener, PaneArt,
    ROW_TEXT, UNFOCUSED_ALPHA, View, Widget, view_of,
};

/// `error_dialog_large`'s screen id: the large dialog's panel and header
/// layout.
pub const LARGE_SCREEN_ID: i16 = 240;
/// The upper-left and lower-right corner bitmap blocks in pane 0.
pub(crate) const FRAME_CORNERS: [u16; 2] = [0, 1];
pub(crate) const FRAME_CORNER_SIZE: u16 = 128;
/// `dialog_live_password`'s screen id, and its first `password` cell's
/// bitmap index in pane 0.
pub const PASSWORD_SCREEN_ID: i16 = 21;
pub const PASSWORD_CELL: u16 = 4;

/// The retail dialog a dialog of ours is built from: its tag's header
/// layout, panel art and list, copied at run time
/// ([`crate::ui_kit::dialog_tag::write_base_tag`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogBase {
    /// `error_dialog_large` (screen 240): header layout 2, the
    /// `dialog1`/`dialog2` panel and its corners, a list at (-235, 30).
    /// In `mainmenu.map` and in the game maps' interfaces alike, so it is
    /// the one a screen opened in a game (the pause menus) can use.
    #[default]
    Large,
    /// `quick_options` (screen 222, the lobby's Game Setup > Quick
    /// Options): header layout 1, the full-screen `fs_dialog` panel, an
    /// eleven-row settings list (skin 2, a label and a value per row) at
    /// (-254, 87) and a help line above it; the game's own settings
    /// dialog. Only in `mainmenu.map`.
    QuickOptions,
    /// `low_level_settings` (screen 223, the variant editor's and Quick
    /// Options' value picker): the large dialog's panel and header layout
    /// 2 (title "SCORE TO WIN ROUND", legend in its footer), a six-row
    /// list of one-text items on `default6` (skin 22, its scroll arrows
    /// at the list's left) at (-231, 30), and the setting's description
    /// in a text block on the right ([`PICKER_HELP_TEXT`]). Only in
    /// `mainmenu.map`; in a game the picker is the large dialog laid out
    /// the same way (`crate::ui_kit::picker`).
    ValuePicker,
}

impl DialogBase {
    pub fn screen_id(self) -> i16 {
        match self {
            DialogBase::Large => LARGE_SCREEN_ID,
            DialogBase::QuickOptions => QUICK_OPTIONS_SCREEN_ID,
            DialogBase::ValuePicker => VALUE_PICKER_SCREEN_ID,
        }
    }
    /// Where a row's value goes: alone in the value column of Quick
    /// Options' settings skin (`setting_lists`: a label and a value text
    /// per row, `Score to Win  50`, no arrows), after the label on the
    /// large dialog's one-text skin.
    pub fn style(self) -> crate::ui_kit::spec::ValueStyle {
        match self {
            DialogBase::Large | DialogBase::ValuePicker => crate::ui_kit::spec::ValueStyle::Inline,
            DialogBase::QuickOptions => crate::ui_kit::spec::ValueStyle::Setting,
        }
    }
    /// Where a dialog of rows on this base says what the focused row does:
    /// Quick Options' own help line ([`QUICK_HELP_TEXT`]); the large dialog
    /// has none.
    pub fn help(self) -> Option<crate::list_class::TextBlock> {
        match self {
            DialogBase::Large => None,
            DialogBase::QuickOptions => Some(QUICK_HELP_TEXT),
            DialogBase::ValuePicker => Some(PICKER_HELP_TEXT),
        }
    }
    /// The base pane's bitmaps (by block index) that are the retail
    /// dialog's content rather than its backing, left out of ours:
    /// Quick Options' game type picture beside the variant's name.
    pub fn content_bitmaps(self) -> &'static [u16] {
        match self {
            DialogBase::Large | DialogBase::ValuePicker => &[],
            DialogBase::QuickOptions => &[1],
        }
    }
    /// Where a page on this base shows its icon: Quick Options' game
    /// type picture ([`QUICK_PICTURE`]); the other bases have none.
    pub fn picture(self) -> Option<crate::list_class::PictureSpec> {
        match self {
            DialogBase::QuickOptions => Some(QUICK_PICTURE),
            DialogBase::Large | DialogBase::ValuePicker => None,
        }
    }
    /// Whether its tag is in the game maps' interfaces (`shared.map`,
    /// `single_player_shared.map`) as well as the main menu's.
    pub fn in_game(self) -> bool {
        self == DialogBase::Large
    }
}

/// `quick_options`'s screen id.
pub const QUICK_OPTIONS_SCREEN_ID: i16 = 222;
/// `low_level_settings`' screen id: the value picker.
pub const VALUE_PICKER_SCREEN_ID: i16 = 223;

/// What a dialog shows besides its title and legend (the spec's `title`
/// and `prompts`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DialogSpec {
    /// The retail dialog it is built from.
    pub base: DialogBase,
    /// Empty for a dialog of rows alone.
    pub body: String,
    pub cells: Option<Cells>,
    /// A line under the cells (or the body): a check running, an error.
    pub status: Option<String>,
    /// The screen it opened over, as it was: the preview draws it
    /// underneath, and the dialog's texts are checked against its texts.
    pub opener: Opener,
    /// Rows in the base's list: a settings dialog. The focused one is the
    /// spec's (`ScreenSpec::focus`).
    pub rows: Vec<RowSpec>,
}

impl DialogSpec {
    /// A dialog of `body` on the large dialog over `opener`, no rows.
    pub fn new(body: &str, opener: Opener) -> DialogSpec {
        DialogSpec {
            base: DialogBase::Large,
            body: body.into(),
            cells: None,
            status: None,
            opener,
            rows: Vec::new(),
        }
    }
}

/// A row of entry cells: each one's text, the cursor's cell, and a wider
/// gap after cell `gap_after` (a code in two halves).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cells {
    pub text: Vec<String>,
    pub cursor: Option<usize>,
    pub gap_after: Option<usize>,
}

/// Where a dialog's parts go: the disc's ([`crate::ui_kit::preview::Disc::dialog_geometry_for`])
/// or [`DialogGeometry::fallback_for`].
#[derive(Clone, Debug, PartialEq)]
pub struct DialogGeometry {
    /// The base dialog's screen id.
    pub screen_id: i16,
    /// Header layout 2's rectangles: the title, and the legend.
    pub header: Rect,
    pub header_font: i16,
    pub header_colour: [f32; 3],
    pub footer: Rect,
    /// The corners and the panel, in the tag's draw order.
    pub panel: Vec<PaneArt>,
    /// The dialog's text block's font and colour.
    pub body_font: i16,
    pub body_colour: [f32; 3],
    /// One entry cell's art and size.
    pub cell: PaneArt,
    /// The base's list (anchor and skin) and the rows it lays out.
    pub list: ListAt,
    pub list_items: usize,
    /// Its rows' geometry on that skin ([`crate::ui_kit::preview::Disc::geometry`]
    /// of the list where the base has it).
    pub rows: Geometry,
}

const fn rect(top: i16, left: i16, bottom: i16, right: i16) -> Rect {
    Rect {
        top,
        left,
        bottom,
        right,
    }
}

/// Our parts' places, inside the panel between the header (bottom 73) and
/// the footer (top -83). The main menu stays drawn in full under a dialog
/// (docs/game/ui-builder.md "The modal dialog"), and its logo is at
/// `[45 -255 -19 257]`: the band from the body's top down to [`STRIP_BOTTOM`]
/// is the panel's cleanest. The body starts where retail's dialog texts
/// do (72 and 74); a code page's body stays in the band, and a one-line
/// body has the status line under it there when two lines fit (they don't
/// at the disc's font 1, 19 units a line in a 27-unit band). The cells are at the passcode
/// dialog's own height (its cells' top is 6). Otherwise the status line
/// sits above the legend ([`STATUS`]).
pub const BODY_TOP: i16 = 72;
pub const STRIP_BOTTOM: i16 = 45;
pub const CELL_TOP: i16 = 6;
pub const STATUS: Rect = rect(-51, -243, -80, 255);
/// Header layout 1's rectangles (`quick_options`' header), as the disc's
/// `wigl` has them.
pub const LAYOUT1_HEADER: Rect = rect(190, -253, 170, 30);
pub const LAYOUT1_FOOTER: Rect = rect(-178, -200, -198, 259);
/// `quick_options`' help line, above its list: where a settings dialog's
/// description goes.
pub const QUICK_HELP: Rect = rect(143, -224, 123, 270);
/// The help line's text block as `quick_options` has it (pane 0's text
/// block 0, string `quick_options_help`: one line, "Temporarily change
/// settings for this game variant."): left-justified, font 1, the pale
/// blue, the pane's animation (31), so it comes and goes with the panel.
/// A settings dialog of ours shows the focused row's description in it
/// ([`DialogBase::help`]); `ui_kit::screens::tests::preview_and_check_against_the_disc`
/// holds it to the disc's block.
pub const QUICK_HELP_TEXT: crate::list_class::TextBlock = crate::list_class::TextBlock {
    rect: [
        QUICK_HELP.top,
        QUICK_HELP.left,
        QUICK_HELP.bottom,
        QUICK_HELP.right,
    ],
    font: 1,
    colour: [0.388235, 0.580392, 0.756863],
    animation: QUICK_ANIMATION,
};
/// `quick_options`' pane, list and text animation slots (1-based, as its
/// tag stores them): the pane and its texts 31, its list 32.
pub const QUICK_ANIMATION: u16 = 31;
/// Quick Options' list: `setting_lists` (skin 2), eleven rows from
/// (-254, 87), each item 26 high; a label (left, the pale blue) and a
/// value (from x 256, a paler blue) in font 4.
pub const QUICK_LIST: ListAt = ListAt {
    anchor: [-254, 87],
    skin: 2,
};
pub const QUICK_LIST_ROWS: usize = 11;

/// Quick Options' game type picture: pane 0's bitmap block 1,
/// `mp_games2.bitm` (eleven 40 x 40 icons, the variant's game type's
/// filled by code) at (-267, 166), depth 3, the pane's animation (31),
/// left of the variant's name and the help line. A page of ours on Quick
/// Options shows its own icon of the set there
/// ([`crate::ui_kit::screen::KitScreen::header`]): the block copied
/// ([`crate::list_class::ListClass::picture`]) and its image set as the
/// screen opens ([`crate::list_class::Rows::picture_image`]). The block's
/// own image field is 0, as `PictureSpec::image` checks.
/// `ui_kit::screens::tests::preview_and_check_against_the_disc` holds it
/// to the disc's block.
pub const QUICK_PICTURE: crate::list_class::PictureSpec = crate::list_class::PictureSpec {
    screen_id: QUICK_OPTIONS_SCREEN_ID as u32,
    pane: 0,
    bitmap: 1,
    image: 0,
    at: [-267, 166],
    animation: QUICK_ANIMATION,
};
/// The picture's size: every `mp_games2` icon is 40 x 40 (image 9, 41).
pub const QUICK_PICTURE_SIZE: [u16; 2] = [40, 40];
/// The picture block's render depth bias.
pub const QUICK_PICTURE_DEPTH: i16 = 3;

/// [`QUICK_PICTURE`] showing icon `image`, as the preview draws it.
pub fn quick_icon(image: u16) -> PaneArt {
    PaneArt {
        art: Art::Screen {
            screen_id: QUICK_OPTIONS_SCREEN_ID,
            pane: QUICK_PICTURE.pane as u16,
            index: QUICK_PICTURE.bitmap as u16,
            image: image as i16,
        },
        at: QUICK_PICTURE.at,
        size: QUICK_PICTURE_SIZE,
        depth: QUICK_PICTURE_DEPTH,
    }
}
/// `mp_games2.bitm`'s icons (read off `mainmenu.map`, 2026-11-19, and
/// looked at with `h2 bitmap`), by what each shows. The pages of ours give
/// each row one (`crate::h1_options::OptionRow::icon`,
/// `crate::ui_kit::choice::ChoiceRow::icon`), shown in Quick Options'
/// game type picture while the row has the focus.
pub mod icon {
    /// A Spartan standing: Slayer's.
    pub const SPARTAN: u16 = 0;
    /// A crown on a hill: King of the Hill's.
    pub const CROWN: u16 = 1;
    /// A lightning bolt.
    pub const LIGHTNING: u16 = 2;
    /// A skull in a circle: Oddball's.
    pub const SKULL: u16 = 3;
    /// A helmet in a sunburst.
    pub const HELMET: u16 = 4;
    /// A hanging sack.
    pub const SACK: u16 = 5;
    /// A flag: Capture the Flag's.
    pub const FLAG: u16 = 6;
    /// A bomb with a lit fuse: Assault's.
    pub const BOMB: u16 = 7;
    /// Four squares in a diamond: Territories'.
    pub const SQUARES: u16 = 8;
    /// A banner of three stars (41 wide, the others 40).
    pub const BANNER: u16 = 9;
    /// A question mark.
    pub const QUESTION: u16 = 10;
    /// How many there are.
    pub const COUNT: u16 = 11;
}

/// `low_level_settings`' list: `default6` (skin 22: `default`'s one text
/// and row bar, its scroll arrows at (-27, 24) and (-27, -88)), six rows
/// from (-231, 30), each 22 high; more values than that scroll.
pub const PICKER_LIST: ListAt = ListAt {
    anchor: [-231, 30],
    skin: 22,
};
pub const PICKER_LIST_ROWS: usize = 6;
/// `low_level_settings`' description block (pane 0's text block 0,
/// string `description`, filled by code): right of the list, from the
/// header's bottom to the footer's top, left-justified, font 1, the pale
/// blue, the pane's animation (23). Where the picker says what the
/// setting does ("Points are earned in Slayer for...").
pub const PICKER_HELP: Rect = rect(74, 15, -80, 250);
pub const PICKER_HELP_TEXT: crate::list_class::TextBlock = crate::list_class::TextBlock {
    rect: [
        PICKER_HELP.top,
        PICKER_HELP.left,
        PICKER_HELP.bottom,
        PICKER_HELP.right,
    ],
    font: 1,
    colour: [0.388235, 0.580392, 0.756863],
    animation: PICKER_ANIMATION,
};
/// `low_level_settings`' pane and text animation slot (1-based), as
/// `error_dialog_large`'s.
pub const PICKER_ANIMATION: u16 = 23;

/// The text column's edges: the header layout's title rectangle's.
pub const LEFT: i16 = -243;
pub const RIGHT: i16 = 255;
/// Cells this far apart (the passcode dialog's are 58 to 59 apart, 55
/// wide), and this much more after `gap_after`.
pub const CELL_GAP: i32 = 4;
pub const HALF_GAP: i32 = 12;

impl DialogGeometry {
    /// The large dialog's ([`DialogGeometry::fallback_for`]).
    pub fn fallback() -> DialogGeometry {
        DialogGeometry::fallback_for(DialogBase::Large)
    }

    /// Where `list` puts its rows: the base's list where it is, showing
    /// `items` rows.
    pub fn list_place(list: ListAt, items: usize) -> ListPlace {
        ListPlace::Moved {
            skin: list.skin,
            anchor: list.anchor,
            bitmaps_left_of: i16::MIN,
            visible: items as i16,
        }
    }

    /// The disc's dialog `base`, as `h2 ui` reads its tag (and
    /// `dialog_live_password` for the cells).
    pub fn fallback_for(base: DialogBase) -> DialogGeometry {
        let large = DialogGeometry::large();
        match base {
            DialogBase::Large => large,
            DialogBase::ValuePicker => {
                // The large dialog's four panel blocks, as
                // `low_level_settings` has them (same bitmaps, places and
                // depth; its own animations).
                let panel = large
                    .panel
                    .iter()
                    .map(|p| PaneArt {
                        art: match p.art.clone() {
                            Art::Screen {
                                pane, index, image, ..
                            } => Art::Screen {
                                screen_id: VALUE_PICKER_SCREEN_ID,
                                pane,
                                index,
                                image,
                            },
                            other => other,
                        },
                        ..*p
                    })
                    .collect();
                let list = PICKER_LIST;
                DialogGeometry {
                    screen_id: VALUE_PICKER_SCREEN_ID,
                    panel,
                    list,
                    list_items: PICKER_LIST_ROWS,
                    rows: Geometry::fallback(Self::list_place(list, PICKER_LIST_ROWS)),
                    ..large
                }
            }
            DialogBase::QuickOptions => {
                let art = |index: u16, at: [i16; 2], size: [u16; 2], depth: i16| PaneArt {
                    art: Art::Screen {
                        screen_id: QUICK_OPTIONS_SCREEN_ID,
                        pane: 0,
                        index,
                        image: 0,
                    },
                    at,
                    size,
                    depth,
                };
                let list = QUICK_LIST;
                DialogGeometry {
                    screen_id: QUICK_OPTIONS_SCREEN_ID,
                    header: LAYOUT1_HEADER,
                    header_font: 2,
                    footer: LAYOUT1_FOOTER,
                    // `fs_dialog` (bitmap 2); bitmap 0 is the 2-pixel
                    // `null` placeholder and 1 the game type's picture
                    // ([`DialogBase::content_bitmaps`]).
                    panel: vec![art(2, [-270, 200], [1024, 512], 1)],
                    list,
                    list_items: QUICK_LIST_ROWS,
                    rows: Geometry::fallback(Self::list_place(list, QUICK_LIST_ROWS)),
                    ..large
                }
            }
        }
    }

    fn large() -> DialogGeometry {
        let panel = |index: u16, at: [i16; 2], size: u16| PaneArt {
            art: Art::Screen {
                screen_id: LARGE_SCREEN_ID,
                pane: 0,
                index,
                image: 0,
            },
            at,
            size: [size, size],
            depth: 0,
        };
        let list = ListAt {
            anchor: [-235, 30],
            skin: 0,
        };
        DialogGeometry {
            screen_id: LARGE_SCREEN_ID,
            list,
            list_items: 2,
            rows: Geometry::fallback(Self::list_place(list, 2)),
            header: rect(93, -243, 73, 255),
            header_font: 2,
            header_colour: [0.678431, 0.764706, 0.847059],
            footer: rect(-83, -243, -103, 240),
            panel: vec![
                panel(FRAME_CORNERS[0], [-273, 109], FRAME_CORNER_SIZE),
                panel(FRAME_CORNERS[1], [203, -33], FRAME_CORNER_SIZE),
                panel(2, [-268, 104], 256),
                panel(3, [12, 153], 256),
            ],
            body_font: 1,
            body_colour: [0.388235, 0.580392, 0.756863],
            cell: PaneArt {
                art: Art::Screen {
                    screen_id: PASSWORD_SCREEN_ID,
                    pane: 0,
                    index: PASSWORD_CELL,
                    image: 0,
                },
                at: [-114, 6],
                size: [55, 49],
                depth: 0,
            },
        }
    }

    /// The panel's bounding box, its corners included: what a modal may
    /// cover, and what its content must stay inside.
    pub fn area(&self) -> Rect {
        let mut r = rect(i16::MIN, i16::MAX, i16::MAX, i16::MIN);
        for a in &self.panel {
            let p = art_rect(a);
            r.top = r.top.max(p.top);
            r.left = r.left.min(p.left);
            r.bottom = r.bottom.min(p.bottom);
            r.right = r.right.max(p.right);
        }
        r
    }
}

fn art_rect(a: &PaneArt) -> Rect {
    rect(
        a.at[1],
        a.at[0],
        a.at[1] - a.size[1] as i16,
        a.at[0] + a.size[0] as i16,
    )
}

fn rgb(v: [f32; 3]) -> [u8; 3] {
    v.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Each cell's left edge, the row centred on the panel.
pub fn cell_lefts(n: usize, width: i32, gap_after: Option<usize>) -> Vec<i32> {
    let extra = |i: usize| match gap_after {
        Some(g) if i > g => HALF_GAP,
        _ => 0,
    };
    let total = n as i32 * width + n.saturating_sub(1) as i32 * CELL_GAP + extra(n);
    (0..n)
        .map(|i| -total / 2 + i as i32 * (width + CELL_GAP) + extra(i))
        .collect()
}

/// The placement the layout pins (the `placement` check, run for every
/// modal by [`crate::ui_kit::layout::check`]): what of `v` breaks it.
/// The body starts at [`BODY_TOP`]; with cells, its ink stays above
/// [`STRIP_BOTTOM`] and every cell is at [`CELL_TOP`]; a one-line status
/// under a one-line body sits in that band when two lines fit there.
pub fn placement(v: &View, m: &dyn Metrics) -> Vec<(String, &'static str)> {
    use crate::ui_kit::layout::ink;
    let mut out = Vec::new();
    let Some(body) = v.widget("body") else {
        if v.widgets.iter().any(|w| w.id.starts_with("row.")) {
            // A dialog of rows: its list is the base's own.
            return out;
        }
        return vec![("body".into(), "a dialog has a body or rows")];
    };
    if body.rect[0] != BODY_TOP {
        out.push((
            "body".into(),
            "the body starts at 72, as retail's dialog texts do",
        ));
    }
    let Kind::Text { font, .. } = &body.kind else {
        return out;
    };
    let lh = m.line_height(*font);
    let body_ink = ink(body, m);
    let cells = v
        .widgets
        .iter()
        .filter(|w| w.id.starts_with("cell.") && w.id.ends_with(".art"));
    let mut has_cells = false;
    for c in cells {
        has_cells = true;
        if c.rect[0] != CELL_TOP {
            out.push((
                c.id.clone(),
                "cells sit at the passcode dialog's height (top 6)",
            ));
        }
    }
    if has_cells && body_ink.is_some_and(|r| r.bottom < STRIP_BOTTOM) {
        out.push((
            "body".into(),
            "a code page's body stays above the logo (down to 45)",
        ));
    }
    let one_line = |w: &Widget| w.text().is_some_and(|t| !t.contains('\n'));
    let fits = i32::from(BODY_TOP) - 2 * lh >= i32::from(STRIP_BOTTOM);
    if let Some(st) = v.widget("status")
        && fits
        && one_line(body)
        && one_line(st)
        && ink(st, m).is_some_and(|r| r.top > BODY_TOP || r.bottom < STRIP_BOTTOM)
    {
        out.push((
            "status".into(),
            "under a one-line body, the status is in the band above the logo",
        ));
    }
    out
}

/// Where the status line goes: under a one-line body in the band above
/// the logo when a line of it fits there, else above the legend.
pub fn status_place(
    d: &DialogSpec,
    body_lines: usize,
    g: &DialogGeometry,
    m: &dyn Metrics,
) -> Rect {
    let lh = m.line_height(g.body_font);
    let top = i32::from(BODY_TOP) - lh;
    let one_line = d
        .status
        .as_ref()
        .is_some_and(|st| m.width(g.body_font, st) <= i32::from(RIGHT - LEFT));
    if body_lines == 1 && one_line && top - lh >= i32::from(STRIP_BOTTOM) {
        rect(top as i16, LEFT, STRIP_BOTTOM, RIGHT)
    } else {
        STATUS
    }
}

/// Dialog `d`'s rows in its base's list, laid out as [`view_of`] lays a
/// list on that skin (the focused row lit, the window around the focus),
/// and the focused row's description in `s`'s description block, if any.
fn row_widgets(s: &ScreenSpec, d: &DialogSpec, g: &DialogGeometry, m: &dyn Metrics) -> Vec<Widget> {
    if d.rows.is_empty() {
        return Vec::new();
    }
    // The tag copy's list shows every row (its visible count is set to
    // them, `dialog_tag::write_base_tag`); the panel check says when they
    // don't fit.
    let shown = d.rows.len();
    let mut list = ScreenSpec::new(&s.name, &s.title);
    list.rows = d.rows.clone();
    list.focus = s.focus;
    list.style = s.style;
    list.description = s.description;
    list.help = s.help.clone();
    list.list = DialogGeometry::list_place(g.list, shown);
    let rows = Geometry {
        visible: shown,
        ..g.rows.clone()
    };
    view_of(&list, &rows, m, "")
        .widgets
        .into_iter()
        .filter(|w| w.id.starts_with("row.") || w.id == "description")
        .collect()
}

/// Dialog `d` of spec `s` as a [`View`] in state `state`: the panel, the
/// title, the body, the rows, the cells, the status and the legend, over
/// the screen that opened it.
pub fn dialog_view(
    s: &ScreenSpec,
    d: &DialogSpec,
    g: &DialogGeometry,
    m: &dyn Metrics,
    state: &str,
) -> View {
    let mut w = Vec::new();
    for (k, a) in g.panel.iter().enumerate() {
        let r = art_rect(a);
        w.push(Widget {
            id: format!("panel.{k}"),
            rect: [r.top, r.left, r.bottom, r.right],
            layer: Layer::Panel,
            alpha: 255,
            kind: Kind::Art {
                art: a.art.clone(),
                size: a.size,
            },
        });
    }
    let text = |id: &str, r: Rect, font: i16, justify: Justify, text: String, colour| Widget {
        id: id.into(),
        rect: [r.top, r.left, r.bottom, r.right],
        layer: Layer::Content,
        alpha: 255,
        kind: Kind::Text {
            text,
            font,
            justify,
            colour,
        },
    };
    w.push(text(
        "title",
        g.header,
        g.header_font,
        Justify::Left,
        s.title.clone(),
        rgb(g.header_colour),
    ));
    let body_bottom = if d.cells.is_some() {
        STRIP_BOTTOM
    } else {
        STATUS.top + 8
    };
    let body = rect(BODY_TOP, LEFT, body_bottom, RIGHT);
    let lines = wrap(&d.body, i32::from(RIGHT - LEFT), &|x| {
        m.width(g.body_font, x)
    });
    let status_at = status_place(d, lines.len(), g, m);
    let body = if status_at == STATUS {
        body
    } else {
        Rect {
            bottom: status_at.top,
            ..body
        }
    };
    if !d.body.is_empty() {
        w.push(text(
            "body",
            body,
            g.body_font,
            Justify::Left,
            lines.join("\n"),
            rgb(g.body_colour),
        ));
    }
    w.extend(row_widgets(s, d, g, m));
    // The screen's own art (a page's icon in Quick Options' picture).
    for (k, a) in s.shown_art().iter().enumerate() {
        let r = art_rect(a);
        w.push(Widget {
            id: format!("art.{k}"),
            rect: [r.top, r.left, r.bottom, r.right],
            layer: Layer::Frame,
            alpha: 255,
            kind: Kind::Art {
                art: a.art.clone(),
                size: a.size,
            },
        });
    }
    if let Some(c) = &d.cells {
        let [cw, ch] = g.cell.size.map(i32::from);
        let lh = m.line_height(g.header_font);
        for (i, left) in cell_lefts(c.text.len(), cw, c.gap_after)
            .into_iter()
            .enumerate()
        {
            let lit = c.cursor == Some(i);
            let r = rect(
                CELL_TOP,
                left as i16,
                (i32::from(CELL_TOP) - ch) as i16,
                (left + cw) as i16,
            );
            w.push(Widget {
                id: format!("cell.{i}.art"),
                rect: [r.top, r.left, r.bottom, r.right],
                layer: Layer::Under,
                alpha: if lit { 255 } else { FRAME_DIM },
                kind: Kind::Art {
                    art: g.cell.art.clone(),
                    size: g.cell.size,
                },
            });
            let top = i32::from(CELL_TOP) - (ch - lh) / 2;
            let mut t = text(
                &format!("cell.{i}.text"),
                rect(top as i16, r.left, (top - lh) as i16, r.right),
                g.header_font,
                Justify::Centre,
                c.text[i].clone(),
                ROW_TEXT,
            );
            t.alpha = if lit { 255 } else { UNFOCUSED_ALPHA };
            w.push(t);
        }
    }
    if let Some(st) = &d.status {
        let lines = wrap(st, status_at.width(), &|x| m.width(g.body_font, x));
        w.push(text(
            "status",
            status_at,
            g.body_font,
            Justify::Left,
            lines.join("\n"),
            rgb(g.header_colour),
        ));
    }
    if !s.prompts.is_empty() {
        w.push(text(
            "legend",
            g.footer,
            g.header_font,
            Justify::Right,
            crate::ui_kit::input::footer_legend(&s.prompts),
            rgb(g.header_colour),
        ));
    }
    View {
        screen: s.name.clone(),
        state: state.into(),
        backing: Backing::Over(d.opener),
        underlay: None,
        widgets: w,
        focus: d
            .cells
            .as_ref()
            .and_then(|c| c.cursor)
            .map(|i| format!("cell.{i}"))
            .or_else(|| (!d.rows.is_empty()).then(|| format!("row.{}", s.focus))),
        list: (!d.rows.is_empty()).then_some(g.list),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::input::{Glyph, Prompt};
    use crate::ui_kit::layout::{Issue, Rules, check};
    use crate::ui_kit::preview::SyntheticSource;
    use crate::ui_kit::view::Synthetic;

    fn code(cursor: usize, status: Option<&str>) -> ScreenSpec {
        let mut s = ScreenSpec::new("t", "Sign in as Chief");
        s.prompts = vec![
            Prompt::new(&[Glyph::DpadUp, Glyph::DpadDown], "Digit"),
            Prompt::new(&[Glyph::A], "Sign in"),
            Prompt::new(&[Glyph::B], "Cancel"),
        ];
        s.dialog = Some(DialogSpec {
            body: "Enter the 6-digit code.".into(),
            cells: Some(Cells {
                text: (0..6).map(|i| i.to_string()).collect(),
                cursor: Some(cursor),
                gap_after: Some(2),
            }),
            status: status.map(String::from),
            opener: Opener::MAIN_MENU,
            ..DialogSpec::new("", Opener::MAIN_MENU)
        });
        s
    }

    fn view(s: &ScreenSpec) -> View {
        dialog_view(
            s,
            s.dialog.as_ref().unwrap(),
            &DialogGeometry::fallback(),
            &Synthetic,
            "t",
        )
    }

    /// Six cells centred on the panel, 59 apart, the halves 12 further.
    #[test]
    fn cells_centre_in_two_halves() {
        let l = cell_lefts(6, 55, Some(2));
        assert_eq!(l, [-181, -122, -63, 8, 67, 126]);
        assert_eq!(l[5] + 55, 181, "the row is centred");
        assert_eq!(cell_lefts(4, 55, None), [-116, -57, 2, 61]);
    }

    /// The cursor's cell is lit, the rest dimmed; the view is over the
    /// main menu with the focus on the cell.
    #[test]
    fn the_cursor_cell_is_lit() {
        let v = view(&code(3, None));
        assert_eq!(v.backing, Backing::Over(Opener::MAIN_MENU));
        assert_eq!(v.focus.as_deref(), Some("cell.3"));
        assert_eq!(v.widget("cell.3.art").unwrap().alpha, 255);
        assert_eq!(v.widget("cell.2.art").unwrap().alpha, FRAME_DIM);
        assert_eq!(v.widget("cell.3.text").unwrap().alpha, 255);
        assert_eq!(v.widget("cell.0.text").unwrap().alpha, UNFOCUSED_ALPHA);
        assert_eq!(v.widget("cell.0.text").unwrap().text(), Some("0"));
        assert!(v.list.is_none());
    }

    /// Everything of ours is inside the panel, and the panel's art is
    /// drawn first.
    #[test]
    fn the_content_stays_inside_the_panel() {
        let g = DialogGeometry::fallback();
        let a = g.area();
        assert_eq!(a, rect(153, -273, -161, 331));
        let v = view(&code(0, Some("Wrong code. 2 tries left.")));
        assert_eq!(v.widgets[0].layer, Layer::Panel);
        for w in v.widgets.iter().filter(|w| w.layer != Layer::Panel) {
            let r = w.rect();
            assert!(
                r.top <= a.top && r.bottom >= a.bottom && r.left >= a.left && r.right <= a.right,
                "{}",
                w.id
            );
        }
    }

    /// The code dialog passes the layout checks without a disc, the
    /// modal's rules included.
    #[test]
    fn a_code_dialog_passes_the_checks() {
        for (cursor, status) in [(0, None), (5, Some("Checking with live.example.net..."))] {
            let s = code(cursor, status);
            let r = check(&view(&s), &SyntheticSource, &Rules::for_spec(&s));
            assert_eq!(r.issues, vec![], "cursor {cursor}");
        }
    }

    /// Content past the panel is caught, and a reserved area under the
    /// panel is not.
    #[test]
    fn modal_rules_hold_content_to_the_panel() {
        let s = code(0, Some("Wrong code."));
        let mut v = view(&s);
        let mut rules = Rules::for_spec(&s);
        rules.reserved = vec![("logo".into(), rect(140, -200, 60, 200))];
        assert_eq!(check(&v, &SyntheticSource, &rules).issues, vec![]);
        let status = v.widgets.iter_mut().find(|w| w.id == "status").unwrap();
        status.rect = [200, -243, 170, 255];
        let issues = check(&v, &SyntheticSource, &rules).issues;
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, Issue::OutsidePanel { id } if id == "status")),
            "{issues:?}"
        );
    }

    /// A dialog of rows: Quick Options' panel, no body, its rows (a label
    /// and a value each) where its list is, the focused one lit, and the
    /// description in the help line; the checks pass it.
    #[test]
    fn a_rows_dialog_draws_its_rows_in_the_bases_list() {
        let mut s = ScreenSpec::new("t", "Bots");
        s.prompts = vec![Prompt::new(&[Glyph::B], "Back")];
        s.style = DialogBase::QuickOptions.style();
        s.focus = 1;
        s.rows = vec![
            RowSpec::value("Count", &["1", "4"], 1),
            RowSpec::value("Difficulty", &["Easy", "Normal"], 1),
        ];
        let d = DialogSpec {
            base: DialogBase::QuickOptions,
            rows: s.rows.clone(),
            ..DialogSpec::new("", Opener::MAIN_MENU)
        };
        let g = DialogGeometry::fallback_for(DialogBase::QuickOptions);
        s.dialog = Some(d.clone());
        let v = dialog_view(&s, &d, &g, &Synthetic, "t");
        assert!(v.widget("body").is_none());
        assert_eq!(v.list, Some(g.list));
        assert_eq!(v.focus.as_deref(), Some("row.1"));
        for id in ["row.0.label", "row.0.value", "row.1.label", "row.1.value"] {
            assert!(v.widget(id).is_some(), "{id}");
        }
        let (a, b) = (
            v.widget("row.0.label").unwrap(),
            v.widget("row.1.label").unwrap(),
        );
        assert!(a.rect[0] > b.rect[0], "row 0 above row 1");
        assert!(a.rect[1] >= g.list.anchor[0], "from the list's anchor");
        assert!(
            v.widget("row.0.value").unwrap().rect[1] > a.rect[1],
            "the value in its column"
        );
        // The value alone, as Quick Options shows its own: no arrows.
        assert_eq!(v.widget("row.1.value").unwrap().text(), Some("Normal"));
        assert_eq!(v.widget("row.1.label").unwrap().text(), Some("Difficulty"));
        assert_eq!(placement(&v, &Synthetic), vec![]);
        let issues = check(&v, &SyntheticSource, &Rules::for_spec(&s)).issues;
        assert!(
            issues.iter().all(|i| matches!(i, Issue::Contrast { .. })),
            "{issues:?}"
        );
        // Neither a body nor rows: nothing to show.
        let empty = DialogSpec {
            rows: Vec::new(),
            ..d
        };
        let v = dialog_view(&s, &empty, &g, &Synthetic, "t");
        assert_eq!(placement(&v, &Synthetic).len(), 1);
    }

    /// Metrics with 12-unit lines, so two fit in the band above the logo.
    struct Short;
    impl Metrics for Short {
        fn width(&self, f: i16, t: &str) -> i32 {
            Synthetic.width(f, t)
        }
        fn line_height(&self, _f: i16) -> i32 {
            12
        }
    }

    /// The body at 72; the status under a one-line body in the band above
    /// the logo when two lines fit there, above the legend when not; and
    /// the placement check pins each.
    #[test]
    fn the_body_and_status_keep_above_the_logo() {
        let s = code(0, Some("Wrong code."));
        let d = s.dialog.as_ref().unwrap();
        let g = DialogGeometry::fallback();
        let v = dialog_view(&s, d, &g, &Short, "t");
        assert_eq!(v.widget("body").unwrap().rect[0], BODY_TOP);
        assert_eq!(
            v.widget("status").unwrap().rect,
            [60, LEFT, STRIP_BOTTOM, RIGHT]
        );
        assert_eq!(placement(&v, &Short), vec![]);
        // Synthetic lines are 16: two don't fit in 27, so the status
        // stays above the legend.
        let v16 = dialog_view(&s, d, &g, &Synthetic, "t");
        let st = v16.widget("status").unwrap().rect;
        assert_eq!(st, [STATUS.top, STATUS.left, STATUS.bottom, STATUS.right]);
        assert_eq!(placement(&v16, &Synthetic), vec![]);

        let broken = |f: &dyn Fn(&mut View)| {
            let mut v = v.clone();
            f(&mut v);
            placement(&v, &Short)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
        };
        let at = |v: &mut View, id: &str, r: [i16; 4]| {
            v.widgets.iter_mut().find(|w| w.id == id).unwrap().rect = r;
        };
        assert_eq!(broken(&|v| at(v, "status", STATUS_RECT)), ["status"]);
        assert_eq!(broken(&|v| at(v, "body", [66, LEFT, 45, RIGHT])), ["body"]);
        assert_eq!(
            broken(&|v| at(v, "cell.2.art", [10, -63, -39, -8])),
            ["cell.2.art"]
        );
        let long = "word ".repeat(200);
        assert_eq!(
            broken(&|v| {
                if let Some(Widget {
                    kind: Kind::Text { text, .. },
                    ..
                }) = v.widgets.iter_mut().find(|w| w.id == "body")
                {
                    *text = long.trim().replace(' ', "\n");
                }
            }),
            ["body"],
            "a code page's body running into the logo"
        );
    }

    const STATUS_RECT: [i16; 4] = [STATUS.top, STATUS.left, STATUS.bottom, STATUS.right];
}
