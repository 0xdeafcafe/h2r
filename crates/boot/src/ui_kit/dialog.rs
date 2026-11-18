//! Modal dialogs (docs/game/ui-builder.md "Screen kinds"): a panel over
//! whatever screen is beneath, as the game's own dialogs sit.
//!
//! The large dialog is `error_dialog_large`'s (screen 240, header layout
//! 2): its `ul_07`/`br_07` corners and `dialog1`/`dialog2` panel, the
//! title in the layout's header rectangle and the legend in its second
//! one. Code entry adds a row of the Live passcode dialog's cells
//! (`dialog_live_password`, screen 21, its `password` bitmap), a digit
//! over each, the cursor's cell lit and the rest dimmed. No retail dialog
//! tag draws anything behind its panel (no dim layer, no widget outside
//! the panel and corners), so a dialog's view is [`Backing::Over`] the
//! screen beneath.
//!
//! The body, cell row and status line are ours, laid out inside the
//! panel between the header and the footer; [`DialogGeometry::fallback`]
//! is the disc's tags as `h2 ui` reads them, held to the disc by
//! `ui_kit::screens::tests::preview_and_check_against_the_disc`.

use serde::Serialize;
use tags::ui::Rect;

use crate::ui_kit::spec::{ScreenSpec, wrap};
use crate::ui_kit::view::{
    Art, Backing, Beneath, FRAME_DIM, Justify, Kind, Layer, Metrics, PaneArt, ROW_TEXT,
    UNFOCUSED_ALPHA, View, Widget,
};

/// `error_dialog_large`'s screen id: the large dialog's panel and header
/// layout.
pub const LARGE_SCREEN_ID: i16 = 240;
/// `dialog_live_password`'s screen id, and its first `password` cell's
/// bitmap index in pane 0.
pub const PASSWORD_SCREEN_ID: i16 = 21;
pub const PASSWORD_CELL: u16 = 4;

/// What a dialog shows besides its title and legend (the spec's `title`
/// and `prompts`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DialogSpec {
    pub body: String,
    pub cells: Option<Cells>,
    /// A line under the cells (or the body): a check running, an error.
    pub status: Option<String>,
    /// What the preview draws beneath it.
    pub beneath: Beneath,
}

/// A row of entry cells: each one's text, the cursor's cell, and a wider
/// gap after cell `gap_after` (a code in two halves).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cells {
    pub text: Vec<String>,
    pub cursor: Option<usize>,
    pub gap_after: Option<usize>,
}

/// Where a large dialog's parts go: the disc's ([`crate::ui_kit::preview::Disc::dialog_geometry`])
/// or [`DialogGeometry::fallback`].
#[derive(Clone, Debug, PartialEq)]
pub struct DialogGeometry {
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
/// the footer (top -83): the body under the title, the cells at the
/// passcode dialog's own height (its cells' top is 6), the status line
/// above the legend.
pub const BODY_TOP: i16 = 66;
pub const CELL_TOP: i16 = 6;
pub const STATUS: Rect = rect(-51, -243, -80, 255);
/// The text column's edges: the header layout's title rectangle's.
pub const LEFT: i16 = -243;
pub const RIGHT: i16 = 255;
/// Cells this far apart (the passcode dialog's are 58 to 59 apart, 55
/// wide), and this much more after `gap_after`.
pub const CELL_GAP: i32 = 4;
pub const HALF_GAP: i32 = 12;

impl DialogGeometry {
    /// The disc's large dialog, as `h2 ui` reads `error_dialog_large` and
    /// `dialog_live_password`.
    pub fn fallback() -> DialogGeometry {
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
        DialogGeometry {
            header: rect(93, -243, 73, 255),
            header_font: 2,
            header_colour: [0.678431, 0.764706, 0.847059],
            footer: rect(-83, -243, -103, 240),
            panel: vec![
                panel(0, [-273, 109], 128),
                panel(1, [203, -33], 128),
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

/// Dialog `d` of spec `s` as a [`View`] in state `state`: the panel, the
/// title, the body, the cells, the status and the legend, over the screen
/// beneath.
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
                art: a.art,
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
        CELL_TOP + 8
    } else {
        STATUS.top + 8
    };
    let body = rect(BODY_TOP, LEFT, body_bottom, RIGHT);
    let lines = wrap(&d.body, i32::from(RIGHT - LEFT), &|x| {
        m.width(g.body_font, x)
    });
    w.push(text(
        "body",
        body,
        g.body_font,
        Justify::Left,
        lines.join("\n"),
        rgb(g.body_colour),
    ));
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
                    art: g.cell.art,
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
        let lines = wrap(st, STATUS.width(), &|x| m.width(g.body_font, x));
        w.push(text(
            "status",
            STATUS,
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
        backing: Backing::Over(d.beneath),
        widgets: w,
        focus: d
            .cells
            .as_ref()
            .and_then(|c| c.cursor)
            .map(|i| format!("cell.{i}")),
        list: None,
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
            beneath: Beneath::MainMenu,
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
        assert_eq!(v.backing, Backing::Over(Beneath::MainMenu));
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
}
