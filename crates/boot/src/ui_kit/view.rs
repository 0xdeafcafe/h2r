//! A screen as data (docs/game/ui-builder.md "The model"): every widget
//! with a stable id, its rectangle in UI units (640 by 480, origin at the
//! centre, Y up), its text or the retail art it names. The preview draws a
//! [`View`], the checks read it, and the rects go out as JSON beside each
//! PNG.
//!
//! Nothing here reads the disc. Art is named, never held: a bitmap block
//! of a retail screen or list skin, with the size the screen expects it to
//! have (the disc-backed test compares that size with the image's).

use serde::Serialize;
use tags::ui::Rect;

use crate::ui_kit::spec::{ListPlace, ScreenSpec, ValueStyle, wrap};

/// Text justification, as a text block's flags give it
/// (`tags::ui::justification`: 0 left, 1 right, 2 centred).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    Left,
    Right,
    Centre,
}

impl Justify {
    pub fn from_flag(j: u8) -> Self {
        match j {
            1 => Justify::Right,
            2 => Justify::Centre,
            _ => Justify::Left,
        }
    }
}

/// A retail bitmap block, by where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Art {
    /// Bitmap `index` of pane `pane` of the screen whose `wgit` has
    /// `screen_id` (as `list_class::PictureSpec` names it), showing
    /// `image`.
    Screen {
        screen_id: i16,
        pane: u16,
        index: u16,
        image: i16,
    },
    /// Bitmap `index` of list skin `skin` (an index into `wigl +0x138`).
    Skin { skin: i16, index: u16, image: i16 },
}

/// What is drawn under the screen's own widgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backing {
    /// Nothing: the screen beneath shows.
    None,
    /// The game shell's background screen (screen 5), as under every
    /// settings screen.
    GameShell,
}

/// The order widgets are drawn in, and which may overlap: art under the
/// content it frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// Frame art of the screen.
    Frame,
    /// A row's or a cell's own art (its bar, its box).
    Under,
    /// Texts.
    Content,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    Text {
        text: String,
        font: i16,
        justify: Justify,
        colour: [u8; 3],
    },
    Art {
        art: Art,
        /// The image's size the rectangle was made from.
        size: [u16; 2],
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Widget {
    pub id: String,
    /// `top, left, bottom, right`.
    pub rect: [i16; 4],
    pub layer: Layer,
    /// 0 hides it; art draws with this alpha.
    pub alpha: u8,
    #[serde(flatten)]
    pub kind: Kind,
}

impl Widget {
    pub fn rect(&self) -> Rect {
        let [top, left, bottom, right] = self.rect;
        Rect {
            top,
            left,
            bottom,
            right,
        }
    }
    /// The rectangle in pixels of a 640 by 480 frame: `x, y, width, height`.
    pub fn pixels(&self) -> [i32; 4] {
        let [top, left, bottom, right] = self.rect.map(i32::from);
        [320 + left, 240 - top, right - left, top - bottom]
    }
    pub fn text(&self) -> Option<&str> {
        match &self.kind {
            Kind::Text { text, .. } => Some(text),
            Kind::Art { .. } => None,
        }
    }
}

/// A screen in one state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct View {
    pub screen: String,
    pub state: String,
    pub backing: Backing,
    /// In draw order.
    pub widgets: Vec<Widget>,
    /// The focused element's id prefix (`row.3`, `cell.2`).
    pub focus: Option<String>,
    /// The list's anchor and skin, when the screen has one.
    pub list: Option<ListAt>,
}

/// Where a list is and which skin draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ListAt {
    pub anchor: [i16; 2],
    pub skin: i16,
}

impl View {
    pub fn widget(&self, id: &str) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.id == id)
    }
    /// The view as the JSON written beside its PNG: every widget with its
    /// rect in UI units and in pixels.
    pub fn json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(ws) = v.get_mut("widgets").and_then(|w| w.as_array_mut()) {
            for (j, w) in ws.iter_mut().zip(&self.widgets) {
                j["pixels"] = serde_json::json!(w.pixels());
            }
        }
        v
    }
}

/// Text widths and line heights: the disc's fonts, or [`Synthetic`].
pub trait Metrics {
    fn width(&self, font: i16, text: &str) -> i32;
    fn line_height(&self, font: i16) -> i32;
}

/// Metrics without a disc: every character 6 units wide (about the
/// disc fonts' mean; a button glyph 16), lines 16 high, whatever the font.
#[derive(Clone, Copy, Debug, Default)]
pub struct Synthetic;

impl Synthetic {
    pub const ADVANCE: i32 = 6;
    pub const GLYPH: i32 = 16;
    pub const LINE: i32 = 16;
}

impl Metrics for Synthetic {
    fn width(&self, _font: i16, text: &str) -> i32 {
        text.split('\n')
            .map(|l| {
                l.chars()
                    .map(|c| {
                        if ('\u{e000}'..='\u{e3ff}').contains(&c) {
                            Self::GLYPH
                        } else {
                            Self::ADVANCE
                        }
                    })
                    .sum()
            })
            .max()
            .unwrap_or(0)
    }
    fn line_height(&self, _font: i16) -> i32 {
        Self::LINE
    }
}

/// A list skin's bitmap, relative to an item's origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemArt {
    pub index: u16,
    pub image: i16,
    /// The block's point. Inside an item a bitmap covers `y` up to `y`
    /// plus its height (docs/game/ui-screens.md "Item spacing").
    pub at: [i16; 2],
    pub size: [u16; 2],
    /// Flag bit 0: the focus frame, left out of the item's height, lit
    /// on the focused row.
    pub frame: bool,
    pub depth: i16,
}

/// A screen's frame art: a retail screen's bitmap at its own point (its
/// top-left).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PaneArt {
    pub art: Art,
    pub at: [i16; 2],
    pub size: [u16; 2],
    pub depth: i16,
}

/// Where a list's items, texts and art go: from the disc's tags
/// (`preview::Disc::geometry`) or [`Geometry::fallback`].
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub anchor: [i16; 2],
    pub visible: usize,
    /// Each item is this much lower than the one before (the item's
    /// bounding box over its texts and its bitmaps without flag bit 0).
    pub item_height: i32,
    /// The skin's text blocks, relative to an item's origin: label, then
    /// value when the skin has two; font and justification.
    pub label: (Rect, i16, u8),
    pub value: Option<(Rect, i16, u8)>,
    pub item_colour: [f32; 3],
    pub header: Rect,
    pub header_font: i16,
    pub header_colour: [f32; 3],
    /// The footer legend's rectangle (the header layout's second one).
    pub footer: Rect,
    pub skin: i16,
    pub item_art: Vec<ItemArt>,
    /// The Controls screen's frame art kept for this list.
    pub frame_art: Vec<PaneArt>,
}

/// The Controls screen's id (its `wgit +4`).
pub const CONTROLS_SCREEN_ID: i16 = 39;

const fn rect(top: i16, left: i16, bottom: i16, right: i16) -> Rect {
    Rect {
        top,
        left,
        bottom,
        right,
    }
}

impl Geometry {
    /// The geometry without a disc, as `h2 ui` reads the retail tags
    /// (layout numbers only; the disc-backed test checks they still
    /// match). Lists on skin 2 (the Controls skin) or 0 (the default).
    pub fn fallback(place: ListPlace) -> Geometry {
        let controls_frame = [
            PaneArt {
                art: Art::Screen {
                    screen_id: CONTROLS_SCREEN_ID,
                    pane: 0,
                    index: 0,
                    image: 0,
                },
                at: [-280, 170],
                size: [256, 128],
                depth: 4,
            },
            PaneArt {
                art: Art::Screen {
                    screen_id: CONTROLS_SCREEN_ID,
                    pane: 0,
                    index: 1,
                    image: 0,
                },
                at: [150, -30],
                size: [256, 256],
                depth: 4,
            },
        ];
        let base = Geometry {
            anchor: [-254, 130],
            visible: crate::ui_kit::preview::SHOWN,
            item_height: 26,
            label: (rect(23, 10, 0, 255), 4, 0),
            value: Some((rect(23, 256, 0, 500), 4, 2)),
            item_colour: [0.388235, 0.580392, 0.756863],
            header: rect(204, -270, 180, 70),
            header_font: 2,
            header_colour: [0.678431, 0.764706, 0.847059],
            footer: rect(-187, -272, -211, 272),
            skin: 2,
            item_art: vec![
                ItemArt {
                    index: 0,
                    image: 0,
                    at: [0, 0],
                    size: [508, 26],
                    frame: false,
                    depth: 10,
                },
                ItemArt {
                    index: 1,
                    image: 0,
                    at: [-12, -4],
                    size: [533, 34],
                    frame: true,
                    depth: 11,
                },
            ],
            frame_art: controls_frame.to_vec(),
        };
        match place {
            ListPlace::Controls => base,
            ListPlace::Moved {
                anchor,
                bitmaps_left_of,
                ..
            } => Geometry {
                anchor,
                item_height: 22,
                item_colour: [0.560784, 0.694118, 0.811765],
                label: (rect(28, 20, 8, 210), 1, 0),
                value: None,
                skin: 0,
                item_art: vec![
                    ItemArt {
                        index: 0,
                        image: 0,
                        at: [10, 6],
                        size: [200, 21],
                        frame: false,
                        depth: 10,
                    },
                    ItemArt {
                        index: 1,
                        image: 0,
                        at: [0, -1],
                        size: [28, 34],
                        frame: true,
                        depth: 11,
                    },
                ],
                frame_art: controls_frame
                    .into_iter()
                    .filter(|a| a.at[0] < bitmaps_left_of)
                    .collect(),
                ..base
            },
        }
    }
}

fn rgb(v: [f32; 3]) -> [u8; 3] {
    v.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Row text is white; an unfocused row's is drawn at this alpha. Seen in
/// game, not traced: in the 2026-11-16 screenshot the focused row's text
/// peaks at about (229, 230, 231) and the others' at (103, 109, 118), white
/// at about 40% over their bar (27, 37, 56). The skin's own text colour is
/// not what the game draws for rows.
pub const UNFOCUSED_ALPHA: u8 = 100;
/// Row text's colour, focused or not.
pub const ROW_TEXT: [u8; 3] = [255, 255, 255];
/// An unfocused row's frame alpha.
pub const FRAME_DIM: u8 = 110;

/// A kit screen (a [`ScreenSpec`]) as a [`View`] in state `state`: the
/// game shell backing, the Controls frame art, the title, the rows in the
/// visible window around the focus, the description and the footer legend.
pub fn view_of(s: &ScreenSpec, g: &Geometry, m: &dyn Metrics, state: &str) -> View {
    let mut w = Vec::new();
    let mut frame: Vec<&PaneArt> = g.frame_art.iter().collect();
    frame.sort_by_key(|a| a.depth);
    for (k, a) in frame.into_iter().enumerate() {
        w.push(Widget {
            id: format!("frame.{k}"),
            rect: [
                a.at[1],
                a.at[0],
                a.at[1] - a.size[1] as i16,
                a.at[0] + a.size[0] as i16,
            ],
            layer: Layer::Frame,
            alpha: 255,
            kind: Kind::Art {
                art: a.art,
                size: a.size,
            },
        });
    }
    let text = |id: String, r: Rect, font: i16, j: u8, text: String, colour: [u8; 3]| Widget {
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
    w.push(text(
        "title".into(),
        g.header,
        g.header_font,
        0,
        s.title.clone(),
        rgb(g.header_colour),
    ));
    let first = s.focus.saturating_sub(g.visible.saturating_sub(1));
    let style = if g.value.is_some() {
        s.style
    } else {
        ValueStyle::Inline
    };
    let mut art: Vec<&ItemArt> = g.item_art.iter().collect();
    art.sort_by_key(|a| a.depth);
    for (slot, (i, row)) in s
        .rows
        .iter()
        .enumerate()
        .skip(first)
        .take(g.visible)
        .enumerate()
    {
        let ox = g.anchor[0];
        let oy = (i32::from(g.anchor[1]) - slot as i32 * g.item_height) as i16;
        let focused = i == s.focus;
        for (k, a) in art.iter().enumerate() {
            let top = oy + a.at[1] + a.size[1] as i16;
            let left = ox + a.at[0];
            w.push(Widget {
                id: format!("row.{i}.art.{k}"),
                rect: [top, left, top - a.size[1] as i16, left + a.size[0] as i16],
                layer: Layer::Under,
                alpha: if a.frame && !focused { FRAME_DIM } else { 255 },
                kind: Kind::Art {
                    art: Art::Skin {
                        skin: g.skin,
                        index: a.index,
                        image: a.image,
                    },
                    size: a.size,
                },
            });
        }
        let at = |r: Rect| Rect {
            top: r.top + oy,
            bottom: r.bottom + oy,
            left: r.left + ox,
            right: r.right + ox,
        };
        let alpha = if focused { 255 } else { UNFOCUSED_ALPHA };
        let (label, value) = row.cells(style);
        let (lr, lf, lj) = g.label;
        let mut l = text(format!("row.{i}.label"), at(lr), lf, lj, label, ROW_TEXT);
        l.alpha = alpha;
        w.push(l);
        if let (Some(v), Some((vr, vf, vj))) = (value, g.value) {
            let mut t = text(format!("row.{i}.value"), at(vr), vf, vj, v, ROW_TEXT);
            t.alpha = alpha;
            w.push(t);
        }
    }
    for (k, a) in s.art.iter().enumerate() {
        w.push(Widget {
            id: format!("art.{k}"),
            rect: [
                a.at[1],
                a.at[0],
                a.at[1] - a.size[1] as i16,
                a.at[0] + a.size[0] as i16,
            ],
            layer: Layer::Frame,
            alpha: 255,
            kind: Kind::Art {
                art: a.art,
                size: a.size,
            },
        });
    }
    if let (Some(d), Some(t)) = (s.description, s.description_text()) {
        let [top, left, bottom, right] = d.rect;
        let lines = wrap(&t, i32::from(right - left), &|x| m.width(d.font, x));
        w.push(text(
            "description".into(),
            rect(top, left, bottom, right),
            d.font,
            0,
            lines.join("\n"),
            rgb(d.colour),
        ));
    }
    if let Some(f) = s.footer_text() {
        w.push(text(
            "legend".into(),
            g.footer,
            g.header_font,
            1,
            f,
            rgb(g.header_colour),
        ));
    }
    View {
        screen: s.name.clone(),
        state: state.into(),
        backing: Backing::GameShell,
        widgets: w,
        focus: (!s.rows.is_empty()).then(|| format!("row.{}", s.focus)),
        list: Some(ListAt {
            anchor: g.anchor,
            skin: g.skin,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::spec::RowSpec;

    fn spec(rows: usize, focus: usize) -> ScreenSpec {
        let mut s = ScreenSpec::new("t", "Title");
        for i in 0..rows {
            s.rows
                .push(RowSpec::value(&format!("Row {i}"), &["On", "Off"], 0));
        }
        s.focus = focus;
        s
    }

    #[test]
    fn rows_stack_one_item_height_apart_from_the_anchor() {
        let g = Geometry::fallback(ListPlace::Controls);
        let v = view_of(&spec(3, 0), &g, &Synthetic, "s");
        let bar = |i: usize| v.widget(&format!("row.{i}.art.0")).unwrap().rect;
        // The row bar covers the item's origin up to its height.
        assert_eq!(bar(0), [130 + 26, -254, 130, -254 + 508]);
        assert_eq!(bar(1)[0] - bar(0)[0], -26);
        assert_eq!(v.widget("row.2.value").unwrap().text(), Some("< On >"));
        assert_eq!(v.focus.as_deref(), Some("row.0"));
    }

    #[test]
    fn the_window_follows_the_focus_and_dims_the_rest() {
        let g = Geometry::fallback(ListPlace::Controls);
        let v = view_of(&spec(8, 7), &g, &Synthetic, "s");
        // Five shown, the last five.
        assert!(v.widget("row.2.label").is_none());
        assert!(v.widget("row.3.label").is_some());
        assert_eq!(v.widget("row.7.label").unwrap().alpha, 255);
        assert_eq!(v.widget("row.6.label").unwrap().alpha, UNFOCUSED_ALPHA);
        assert_eq!(v.widget("row.6.art.1").unwrap().alpha, FRAME_DIM);
        assert_eq!(v.widget("row.7.art.1").unwrap().alpha, 255);
    }

    #[test]
    fn json_has_every_widget_with_pixels() {
        let g = Geometry::fallback(ListPlace::Controls);
        let v = view_of(&spec(1, 0), &g, &Synthetic, "focus-0");
        let j = v.json();
        assert_eq!(j["state"], "focus-0");
        let title = j["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == "title")
            .unwrap();
        assert_eq!(title["kind"], "text");
        assert_eq!(title["pixels"], serde_json::json!([50, 36, 340, 24]));
    }

    #[test]
    fn synthetic_widths_count_glyphs_double() {
        assert_eq!(Synthetic.width(1, "ab\u{e100}"), 28);
        assert_eq!(Synthetic.width(1, "abc\nd"), 18);
    }
}
