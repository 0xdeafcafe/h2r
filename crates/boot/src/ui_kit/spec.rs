//! A screen of ours as data: what it shows and what each row does, in a
//! form the static checks ([`check`]), the offline preview
//! (`ui_kit::preview`), the open-screen log line and the agent API's UI
//! stack can all read (docs/game/ui-screens.md "Describe").

use serde::Serialize;

use crate::list_class::{ListLayout, TextBlock};
use crate::ui_kit::ids;
use crate::ui_kit::input::{Input, Prompt, footer_legend, legend, legend_named};

/// What a row is.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RowKind {
    /// A runs it.
    Action,
    /// Left and right step through `values` (`< value >`); `current` is
    /// shown. A toggle is a value with two.
    Value { values: Vec<String>, current: usize },
    /// Shown, not pressed: `value` beside the label.
    Info { value: String },
    /// A opens the game's keyboard; `value` is the text so far.
    Text { value: String, max_chars: u16 },
}

/// Where a row's value goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueStyle {
    /// In the label: `Mythic: < On >` (a one-text skin, the level select's).
    Inline,
    /// In the skin's value text (the Controls skin's second column).
    Column,
}

/// What a press on the row does, as far as a static reader can tell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Press {
    /// Logs or steps (`Chosen::Stepped`).
    Step,
    /// Opens the named screen of ours.
    Open(String),
    /// The screen's action.
    Act,
    /// Opens the keyboard.
    Keyboard,
    /// Nothing (an Info row).
    Nothing,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RowSpec {
    pub label: String,
    #[serde(flatten)]
    pub kind: RowKind,
    pub press: Press,
    /// The focused row's description, when the screen has one.
    pub description: Option<String>,
    /// The string ids the row shows (empty for texts served by slot).
    pub ids: Vec<u32>,
}

impl RowSpec {
    pub fn action(label: &str) -> Self {
        RowSpec {
            label: label.into(),
            kind: RowKind::Action,
            press: Press::Act,
            description: None,
            ids: Vec::new(),
        }
    }
    pub fn value(label: &str, values: &[&str], current: usize) -> Self {
        RowSpec {
            label: label.into(),
            kind: RowKind::Value {
                values: values.iter().map(|v| v.to_string()).collect(),
                current,
            },
            press: Press::Step,
            description: None,
            ids: Vec::new(),
        }
    }
    pub fn info(label: &str, value: &str) -> Self {
        RowSpec {
            label: label.into(),
            kind: RowKind::Info {
                value: value.into(),
            },
            press: Press::Nothing,
            description: None,
            ids: Vec::new(),
        }
    }
    pub fn text(label: &str, value: &str, max_chars: u16) -> Self {
        RowSpec {
            label: label.into(),
            kind: RowKind::Text {
                value: value.into(),
                max_chars,
            },
            press: Press::Keyboard,
            description: None,
            ids: Vec::new(),
        }
    }
    pub fn press(mut self, press: Press) -> Self {
        self.press = press;
        self
    }
    pub fn describe(mut self, text: &str) -> Self {
        self.description = Some(text.into());
        self
    }
    pub fn ids(mut self, ids: &[u32]) -> Self {
        self.ids = ids.to_vec();
        self
    }
    /// The value as shown: `< On >` for a value row, the text for the
    /// others, `None` for an action.
    pub fn shown_value(&self) -> Option<String> {
        match &self.kind {
            RowKind::Action => None,
            RowKind::Value { values, current } => Some(format!("< {} >", values.get(*current)?)),
            RowKind::Info { value } | RowKind::Text { value, .. } => {
                (!value.is_empty()).then(|| value.clone())
            }
        }
    }
    /// The texts the row's item draws: the label text, and the value text
    /// for a [`ValueStyle::Column`] skin.
    pub fn cells(&self, style: ValueStyle) -> (String, Option<String>) {
        match (style, self.shown_value()) {
            (ValueStyle::Inline, Some(v)) => (format!("{}: {v}", self.label), None),
            (ValueStyle::Inline, None) => (self.label.clone(), None),
            (ValueStyle::Column, v) => (self.label.clone(), v),
        }
    }
    /// Every value the row can show, laid out as `cells` would (for the
    /// width check: the longest must fit, not just the current).
    pub fn every_cells(&self, style: ValueStyle) -> Vec<(String, Option<String>)> {
        match &self.kind {
            RowKind::Value { values, .. } => (0..values.len())
                .map(|current| {
                    RowSpec {
                        kind: RowKind::Value {
                            values: values.clone(),
                            current,
                        },
                        ..self.clone()
                    }
                    .cells(style)
                })
                .collect(),
            _ => vec![self.cells(style)],
        }
    }
}

/// Which list the screen's rows sit in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListPlace {
    /// The Controls tag's own list (two columns, full width).
    Controls,
    /// Moved and reskinned ([`ListLayout`]).
    Moved {
        skin: i16,
        anchor: [i16; 2],
        /// The Controls tag's frame art kept: bitmaps left of this x.
        bitmaps_left_of: i16,
    },
}

impl From<Option<ListLayout>> for ListPlace {
    fn from(l: Option<ListLayout>) -> Self {
        l.map_or(ListPlace::Controls, |l| ListPlace::Moved {
            skin: l.skin,
            anchor: l.anchor,
            bitmaps_left_of: l.bitmaps_left_of,
        })
    }
}

/// The description's text block.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DescriptionBlock {
    /// `top, left, bottom, right`, Y up, screen centre origin.
    pub rect: [i16; 4],
    pub font: i16,
    pub colour: [f32; 3],
}

impl From<TextBlock> for DescriptionBlock {
    fn from(b: TextBlock) -> Self {
        DescriptionBlock {
            rect: b.rect,
            font: b.font,
            colour: b.colour,
        }
    }
}

/// A screen of ours, described.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScreenSpec {
    /// The id `Open` rows and `list_class::loader_of` know it by.
    pub name: String,
    pub title: String,
    pub list: ListPlace,
    pub style: ValueStyle,
    pub rows: Vec<RowSpec>,
    pub focus: usize,
    pub description: Option<DescriptionBlock>,
    /// The legend under the description (or alone when there is none).
    pub prompts: Vec<Prompt>,
    /// The inputs the screen takes itself; B and Back close unless taken.
    pub inputs: Vec<Input>,
    /// More rows than the list shows is meant (the list scrolls).
    pub scrolls: bool,
    /// The legend goes in the footer band (the screen's button legend,
    /// `list_class::FOOTER`), as on the game's own screens, rather than
    /// under the description.
    pub footer: bool,
    /// The screen's own ids (title and the like).
    pub ids: Vec<u32>,
    /// Retail art the screen adds of its own (a frame, a picture), drawn
    /// over the frame art the layout keeps.
    pub art: Vec<crate::ui_kit::view::PaneArt>,
}

impl ScreenSpec {
    pub fn new(name: &str, title: &str) -> Self {
        ScreenSpec {
            name: name.into(),
            title: title.into(),
            list: ListPlace::Controls,
            style: ValueStyle::Column,
            rows: Vec::new(),
            focus: 0,
            description: None,
            prompts: Vec::new(),
            inputs: Vec::new(),
            scrolls: false,
            footer: false,
            ids: Vec::new(),
            art: Vec::new(),
        }
    }
    /// The footer legend as drawn, when the prompts go there.
    pub fn footer_text(&self) -> Option<String> {
        (self.footer && !self.prompts.is_empty()).then(|| footer_legend(&self.prompts))
    }
    /// The focused row's description with the legend under it, as drawn.
    pub fn description_text(&self) -> Option<String> {
        let row = self.rows.get(self.focus)?.description.clone();
        let legend = if self.footer {
            String::new()
        } else {
            legend(&self.prompts)
        };
        match (row, legend.is_empty()) {
            (Some(d), true) => Some(d),
            (Some(d), false) => Some(format!("{d}\n\n{legend}")),
            (None, false) => Some(legend),
            (None, true) => None,
        }
    }
    /// One line for the log when the screen opens: `name: "Title"; rows:
    /// [0] Label = value ...; focus n; prompts [A] ...`.
    pub fn log_line(&self) -> String {
        let mut s = format!("{}: {:?}; rows:", self.name, self.title);
        for (i, r) in self.rows.iter().enumerate() {
            let (label, value) = r.cells(ValueStyle::Column);
            match value {
                Some(v) => s.push_str(&format!(" [{i}] {label:?} = {v:?}")),
                None => s.push_str(&format!(" [{i}] {label:?}")),
            }
        }
        s.push_str(&format!("; focus {}", self.focus));
        if !self.prompts.is_empty() {
            s.push_str(&format!("; prompts {}", legend_named(&self.prompts)));
        }
        s
    }
}

/// A screen that can say what it shows now.
pub trait Describe {
    fn spec(&self) -> ScreenSpec;
}

// ---- static checks ----

/// What a list and its texts have room for, from the disc's tags and fonts
/// (`ui_kit::preview::Geometry`) or [`Room::FALLBACK`].
pub struct Room<'a> {
    /// Rows the list shows before it scrolls.
    pub visible: usize,
    /// Units the label and value texts are wide.
    pub label: i32,
    pub value: Option<i32>,
    /// A text's width in units, in the font the row uses.
    pub row_width: &'a dyn Fn(&str) -> i32,
    /// A text's width in the description's font, and its line height.
    pub description_width: &'a dyn Fn(&str) -> i32,
    pub description_line: i32,
    /// Whether the fonts draw a code point (the icon range included).
    pub has_glyph: &'a dyn Fn(char) -> bool,
    /// The footer legend's width, and the text's width in its font.
    pub footer: i32,
    pub footer_width: &'a dyn Fn(&str) -> i32,
}

/// Without the disc: the Controls list's five rows, the skin's 190-unit
/// text, and conduit-12's average of 7.5 units a character ("Custom Game
/// Browser", 19 characters, renders 136 wide; docs/live/live-custom-games.md).
pub mod fallback {
    pub const VISIBLE: usize = 5;
    pub const LABEL: i32 = 190;
    pub fn width(s: &str) -> i32 {
        (s.chars().count() as i32 * 15 + 1) / 2
    }
    pub const LINE: i32 = 15;
    /// The footer legend's room: header layout 0's second rectangle.
    pub const FOOTER: i32 = 300;
}

/// One thing wrong with a screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// More rows than the list shows, and the screen did not say it scrolls.
    Rows { rows: usize, visible: usize },
    /// A label or value wider than its text.
    Wide {
        row: usize,
        text: String,
        width: i32,
        room: i32,
    },
    /// The description (with the legend) runs past its rectangle.
    Description {
        row: usize,
        lines: usize,
        room: usize,
    },
    /// A string id no registered family owns.
    Id { row: Option<usize>, id: u32 },
    /// A row the screen cannot act on.
    Press { row: usize, why: String },
    /// A character the fonts do not draw.
    Glyph { row: Option<usize>, ch: char },
    /// The focus is past the rows.
    Focus { focus: usize, rows: usize },
    /// The footer legend is wider than the footer's text.
    Footer { text: String, width: i32, room: i32 },
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Finding::Rows { rows, visible } => write!(
                f,
                "{rows} rows but the list shows {visible}: say it scrolls (ScreenSpec::scrolls) or cut rows"
            ),
            Finding::Wide {
                row,
                text,
                width,
                room,
            } => write!(f, "row {row}: {text:?} is {width} wide in {room}"),
            Finding::Description { row, lines, room } => write!(
                f,
                "row {row}: the description takes {lines} lines in room for {room}"
            ),
            Finding::Id { row, id } => write!(f, "row {row:?}: id {id:#x} is in no family"),
            Finding::Press { row, why } => write!(f, "row {row}: {why}"),
            Finding::Glyph { row, ch } => {
                write!(f, "row {row:?}: U+{:04X} is not in the font", *ch as u32)
            }
            Finding::Focus { focus, rows } => write!(f, "focus {focus} past {rows} rows"),
            Finding::Footer { text, width, room } => {
                write!(f, "footer {text:?} is {width} wide in {room}")
            }
        }
    }
}

/// Greedy word wrap of `text` to `width` units by `measure`, keeping its
/// own line breaks (`\n`, `\r\n`).
pub fn wrap(text: &str, width: i32, measure: &dyn Fn(&str) -> i32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n').map(|l| l.trim_end_matches('\r')) {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if measure(&candidate) <= width || line.is_empty() {
                line = candidate;
            } else {
                out.push(std::mem::replace(&mut line, word.to_string()));
            }
        }
        out.push(line);
    }
    out
}

/// Every finding for `spec` in `room`.
pub fn check(spec: &ScreenSpec, room: &Room<'_>) -> Vec<Finding> {
    let mut out = Vec::new();
    if spec.rows.len() > room.visible && !spec.scrolls {
        out.push(Finding::Rows {
            rows: spec.rows.len(),
            visible: room.visible,
        });
    }
    if spec.focus >= spec.rows.len().max(1) {
        out.push(Finding::Focus {
            focus: spec.focus,
            rows: spec.rows.len(),
        });
    }
    let owned = |id: u32| ids::FAMILIES.iter().any(|f| f.owns(id));
    for id in &spec.ids {
        if !owned(*id) {
            out.push(Finding::Id { row: None, id: *id });
        }
    }
    let glyphs = |row: Option<usize>, s: &str, out: &mut Vec<Finding>| {
        for ch in s.chars().filter(|c| !c.is_control()) {
            if !(room.has_glyph)(ch) && !out.contains(&Finding::Glyph { row, ch }) {
                out.push(Finding::Glyph { row, ch });
            }
        }
    };
    glyphs(None, &spec.title, &mut out);
    let sideways = spec.inputs.contains(&Input::Left) && spec.inputs.contains(&Input::Right);
    for (i, r) in spec.rows.iter().enumerate() {
        for id in &r.ids {
            if !owned(*id) {
                out.push(Finding::Id {
                    row: Some(i),
                    id: *id,
                });
            }
        }
        for (label, value) in r.every_cells(spec.style) {
            glyphs(Some(i), &label, &mut out);
            let w = (room.row_width)(&label);
            if w > room.label {
                out.push(Finding::Wide {
                    row: i,
                    text: label,
                    width: w,
                    room: room.label,
                });
            }
            if let Some(v) = value {
                glyphs(Some(i), &v, &mut out);
                let (w, r) = ((room.row_width)(&v), room.value.unwrap_or(0));
                if w > r {
                    out.push(Finding::Wide {
                        row: i,
                        text: v,
                        width: w,
                        room: r,
                    });
                }
            }
        }
        match (&r.kind, &r.press) {
            (RowKind::Action, Press::Nothing) => out.push(Finding::Press {
                row: i,
                why: "an action row whose press does nothing".into(),
            }),
            (RowKind::Value { values, .. }, _) if values.len() < 2 => out.push(Finding::Press {
                row: i,
                why: "a value row with fewer than two values".into(),
            }),
            (RowKind::Value { .. }, _) if !sideways => out.push(Finding::Press {
                row: i,
                why: "a value row on a screen that does not take left and right".into(),
            }),
            (RowKind::Text { .. }, p) if *p != Press::Keyboard => out.push(Finding::Press {
                row: i,
                why: "a text row whose press does not open the keyboard".into(),
            }),
            _ => {}
        }
    }
    if let Some(text) = spec.footer_text() {
        glyphs(None, &text, &mut out);
        let w = (room.footer_width)(&text);
        if w > room.footer {
            out.push(Finding::Footer {
                text,
                width: w,
                room: room.footer,
            });
        }
    }
    if let Some(d) = spec.description {
        let height = i32::from(d.rect[0]) - i32::from(d.rect[2]);
        let width = i32::from(d.rect[3]) - i32::from(d.rect[1]);
        let fits = usize::try_from(height / room.description_line.max(1)).unwrap_or(0);
        for i in 0..spec.rows.len() {
            let s = ScreenSpec {
                focus: i,
                ..spec.clone()
            };
            let Some(text) = s.description_text() else {
                continue;
            };
            glyphs(Some(i), &text, &mut out);
            let lines = wrap(&text, width, room.description_width).len();
            if lines > fits {
                out.push(Finding::Description {
                    row: i,
                    lines,
                    room: fits,
                });
            }
        }
    }
    out
}

/// [`check`] without the disc: [`fallback`]'s room, every character drawn.
pub fn check_fallback(spec: &ScreenSpec) -> Vec<Finding> {
    let w = fallback::width;
    let room = Room {
        visible: fallback::VISIBLE,
        label: fallback::LABEL,
        value: Some(fallback::LABEL),
        row_width: &w,
        description_width: &w,
        description_line: fallback::LINE,
        has_glyph: &|_| true,
        footer: fallback::FOOTER,
        footer_width: &w,
    };
    check(spec, &room)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::input::Glyph;

    fn room<'a>(w: &'a dyn Fn(&str) -> i32) -> Room<'a> {
        Room {
            visible: 5,
            label: 190,
            value: Some(100),
            row_width: w,
            description_width: w,
            description_line: 15,
            has_glyph: &|c| c != '\u{e1ff}',
            footer: 300,
            footer_width: w,
        }
    }

    #[test]
    fn a_sixth_row_needs_scroll_intent() {
        let mut s = ScreenSpec::new("t", "T");
        s.rows = (0..6)
            .map(|i| RowSpec::action(&format!("Row {i}")))
            .collect();
        let w = |t: &str| t.len() as i32 * 7;
        assert_eq!(
            check(&s, &room(&w)),
            vec![Finding::Rows {
                rows: 6,
                visible: 5
            }]
        );
        s.scrolls = true;
        assert_eq!(check(&s, &room(&w)), vec![]);
    }

    /// The longest of every value is measured, not only the current one:
    /// "Grunt Birthday Party: < Off >" against an inline skin.
    #[test]
    fn every_value_of_a_row_must_fit() {
        let mut s = ScreenSpec::new("t", "T");
        s.style = ValueStyle::Inline;
        s.inputs = vec![Input::Left, Input::Right];
        s.rows = vec![RowSpec::value("Grunt Birthday Party", &["On", "Off"], 0)];
        // On is 196 wide, Off 203: only Off overflows a 200-unit text.
        let w = |t: &str| t.len() as i32 * 7;
        let f = check(
            &s,
            &Room {
                label: 200,
                ..room(&w)
            },
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(
            matches!(&f[0], Finding::Wide { text, width: 203, room: 200, .. } if text == "Grunt Birthday Party: < Off >"),
            "{f:?}"
        );
    }

    #[test]
    fn presses_inputs_ids_and_glyphs_are_checked() {
        let mut s = ScreenSpec::new("t", "T");
        s.ids = vec![0x1234_5678];
        s.rows = vec![
            RowSpec::action("Go").press(Press::Nothing),
            RowSpec::value("Pick", &["A", "B"], 0),
            RowSpec::text("Name", "", 15).press(Press::Act),
            RowSpec::info("Odd \u{e1ff}", "x").ids(&[ids::family("signin").id(0x10)]),
        ];
        let w = |t: &str| t.len() as i32;
        let f = check(&s, &room(&w));
        let why: Vec<String> = f.iter().map(|f| f.to_string()).collect();
        assert_eq!(f.len(), 5, "{why:#?}");
        assert!(f.contains(&Finding::Id {
            row: None,
            id: 0x1234_5678
        }));
        assert!(f.contains(&Finding::Glyph {
            row: Some(3),
            ch: '\u{e1ff}'
        }));
        assert!(why[1].contains("does nothing") && why[2].contains("left and right"));
        assert!(why[3].contains("keyboard"));
    }

    #[test]
    fn the_description_with_its_legend_must_fit_its_rectangle() {
        let mut s = ScreenSpec::new("t", "T");
        s.description = Some(DescriptionBlock {
            rect: [0, 0, -45, 100],
            font: 1,
            colour: [1.0; 3],
        });
        s.prompts = vec![Prompt::new(&[Glyph::A], "Go")];
        s.rows = vec![
            RowSpec::action("a").describe("short"),
            RowSpec::action("b").describe("one two three four five six seven eight nine"),
        ];
        let w = |t: &str| t.chars().count() as i32 * 10;
        let f = check(&s, &room(&w));
        // Row 0: "short", blank, legend = 3 lines in room for 3.
        assert_eq!(
            f,
            vec![Finding::Description {
                row: 1,
                lines: 7,
                room: 3
            }]
        );
        assert_eq!(
            s.description_text().unwrap(),
            "short\n\n\u{e100} Go",
            "focus 0"
        );
    }

    #[test]
    fn the_log_line_names_rows_values_and_prompts() {
        let mut s = ScreenSpec::new("kit.example", "Example");
        s.rows = vec![
            RowSpec::value("Grunts", &["On", "Off"], 1),
            RowSpec::action("Start"),
        ];
        s.prompts = vec![Prompt::new(&[Glyph::A], "Start")];
        assert_eq!(
            s.log_line(),
            "kit.example: \"Example\"; rows: [0] \"Grunts\" = \"< Off >\" [1] \"Start\"; focus 0; prompts [A] Start"
        );
    }
}
