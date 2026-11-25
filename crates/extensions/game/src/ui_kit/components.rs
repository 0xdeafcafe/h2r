//! The design system (docs/game/ui-builder.md "The design system"): the
//! pieces our screens are built from. Their tokens and presets (colours,
//! fonts by role, spacing, each component's font, colour and
//! justification) are `content/ui/design.yaml` ([`crate::ui_kit::data`]);
//! this module puts them together.
//!
//! - [`Screen`]: a screen of any kind (docs/game/ui-builder.md "Screen
//!   kinds"), its inputs following from its rows and its legend.
//! - [`row`] and [`legend`]: the row kinds and the legend's prompts.
//! - [`Specimen`]: the page that steps a specimen or a layout.
//! - [`specimens`]: the `design.<component>` and `layout.<name>` pages.
//! - [`catalogue_json`]: the catalogue the agent API gives.

use serde::Serialize;

use crate::list_class::{ListLayout, TextBlock};
use crate::ui_kit::dialog::{Cells, DialogBase, DialogSpec};
use crate::ui_kit::input::{Glyph, Input, Prompt};
use crate::ui_kit::spec::{
    DescriptionBlock, ListPlace, PaneText, Press, RowKind, RowSpec, ScreenSpec, ValueStyle,
};
use crate::ui_kit::view::{Opener, PaneArt};

mod specimen;
pub mod specimens;

pub use specimen::{Effect, Specimen};

/// A colour as the views carry it (as `crate::ui_kit::view` converts).
pub fn rgb(c: [f32; 3]) -> [u8; 3] {
    c.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8)
}

/// The rows a list or a dialog of rows is made of. The skin decides
/// where a value goes ([`ValueStyle`]).
pub mod row {
    use crate::ui_kit::spec::{Press, RowSpec};

    /// A runs the screen's action.
    pub fn action(label: &str) -> RowSpec {
        RowSpec::action(label).press(Press::Act)
    }
    /// A opens screen `name` of ours.
    pub fn open(label: &str, name: &str) -> RowSpec {
        RowSpec::action(label).press(Press::Open(name.into()))
    }
    /// Left and right step through `values`.
    pub fn value(label: &str, values: &[&str], current: usize) -> RowSpec {
        RowSpec::value(label, values, current).press(Press::Step)
    }
    /// A opens the value picker on `values`.
    pub fn pick(label: &str, values: &[&str], current: usize) -> RowSpec {
        RowSpec::value(label, values, current)
            .press(Press::Open(crate::ui_kit::picker::NAME.into()))
    }
    /// Shown, not pressed: `value` beside the label.
    pub fn info(label: &str, value: &str) -> RowSpec {
        RowSpec::info(label, value)
    }
    /// A opens the keyboard; `value` is the text so far.
    pub fn text(label: &str, value: &str, max_chars: u16) -> RowSpec {
        RowSpec::text(label, value, max_chars)
    }
    /// A row the player can't use now (the skulls pages' locked skull):
    /// focusable, never pressed, its label alone with nothing in the
    /// value's place, so it never reads as a setting's value. Its help
    /// line says why, and the legend offers only B (`legend::disabled_row`).
    pub fn disabled(label: &str) -> RowSpec {
        RowSpec::info(label, "")
    }
}

/// The legend's prompts, by what they do: the same button reads the same
/// everywhere. The words are drawn in capitals ([`crate::ui_kit::input::legend`]).
pub mod legend {
    use crate::ui_kit::input::{Glyph, Prompt};

    pub fn select() -> Prompt {
        Prompt::new(&[Glyph::A], "Select")
    }
    pub fn back() -> Prompt {
        Prompt::new(&[Glyph::B], "Back")
    }
    pub fn cancel() -> Prompt {
        Prompt::new(&[Glyph::B], "Cancel")
    }
    pub fn done() -> Prompt {
        Prompt::new(&[Glyph::A], "Done")
    }
    /// A runs the screen's start (the skulls page's Start Level).
    pub fn start() -> Prompt {
        Prompt::new(&[Glyph::A], "Start")
    }
    /// A opens the value picker on the focused row (`row::pick`).
    pub fn pick() -> Prompt {
        Prompt::new(&[Glyph::A], "Change")
    }
    /// A row whose A opens the value picker: `A CHANGE  B BACK`.
    pub fn setting_row() -> Vec<Prompt> {
        vec![pick(), back()]
    }
    /// A row the player can't use now (`row::disabled`): `B BACK` alone.
    pub fn disabled_row() -> Vec<Prompt> {
        vec![back()]
    }
    /// Left and right step the focused value.
    pub fn change() -> Prompt {
        Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change")
    }
    /// Left and right move a cursor.
    pub fn move_cursor() -> Prompt {
        Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Move")
    }
    /// Up and down change the digit under the cursor.
    pub fn digit() -> Prompt {
        Prompt::new(&[Glyph::DpadUp, Glyph::DpadDown], "Digit")
    }

    /// Whether `text` is a legend as the design system writes it
    /// (`content/ui/design.yaml`, `legend`): each prompt's glyphs
    /// `glyph_gap` spaces apart, one space, its words in capitals, prompts
    /// `prompt_gap` spaces apart. `Err` says what is off.
    pub fn well_formed(text: &str, glyph_gap: usize, prompt_gap: usize) -> Result<(), String> {
        let glyph = |c: char| ('\u{e100}'..='\u{e1ff}').contains(&c);
        if !text.starts_with(glyph) {
            return Err("a legend starts with its first glyph".into());
        }
        let ggap = " ".repeat(glyph_gap);
        let pgap = " ".repeat(prompt_gap);
        // Walk prompt by prompt: glyphs, one space, words up to the gap
        // before the next glyph.
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            // Glyphs, `glyph_gap` apart.
            let mut glyphs = 0;
            loop {
                if i < chars.len() && glyph(chars[i]) {
                    glyphs += 1;
                    i += 1;
                } else {
                    return Err(format!("prompt {}: a glyph is expected", glyphs + 1));
                }
                let rest: String = chars[i..].iter().collect();
                if rest.starts_with(&ggap) && rest[ggap.len()..].starts_with(glyph) {
                    i += glyph_gap;
                    continue;
                }
                break;
            }
            if chars.get(i) != Some(&' ') || chars.get(i + 1).is_none_or(|c| *c == ' ' || glyph(*c)) {
                return Err("one space between a prompt's glyphs and its words".into());
            }
            i += 1;
            // Words, to the gap before the next glyph or the end.
            let start = i;
            while i < chars.len() {
                let rest: String = chars[i..].iter().collect();
                if rest.starts_with(&pgap) && rest[pgap.len()..].starts_with(glyph) {
                    break;
                }
                i += 1;
            }
            let words: String = chars[start..i].iter().collect();
            if words.chars().any(char::is_lowercase) {
                return Err(format!("{words:?}: a legend's words are in capitals"));
            }
            if words.ends_with(' ') || words.contains("  ") || words.chars().any(glyph) {
                return Err(format!("{words:?}: prompts are {prompt_gap} spaces apart"));
            }
            if i < chars.len() {
                i += prompt_gap;
            }
        }
        Ok(())
    }
}

/// A screen's kind, as the design system names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A list over the game shell (header layout 0), or over the live
    /// picture (transparent).
    List,
    /// A dialog on a retail dialog's panel: Quick Options' (header layout
    /// 1), the large dialog's or the picker's (2).
    Dialog(DialogBase),
    /// The large dialog with entry cells.
    CodeEntry,
}

impl Kind {
    pub fn of(s: &ScreenSpec) -> Kind {
        if let Some(d) = &s.dialog {
            if d.cells.is_some() {
                Kind::CodeEntry
            } else if s.content.picker {
                Kind::Dialog(DialogBase::ValuePicker)
            } else {
                Kind::Dialog(d.base)
            }
        } else {
            Kind::List
        }
    }
    /// Its retail header layout.
    pub fn header_layout(self) -> u8 {
        match self {
            Kind::List => 0,
            Kind::Dialog(DialogBase::QuickOptions) => 1,
            Kind::Dialog(_) | Kind::CodeEntry => 2,
        }
    }
    /// Its name in findings, in the docs' `kind` and `layout`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::List => "list",
            Kind::Dialog(DialogBase::QuickOptions) => "dialog.settings",
            Kind::Dialog(DialogBase::Large) => "dialog.large",
            Kind::Dialog(DialogBase::ValuePicker) => "picker",
            Kind::CodeEntry => "code_entry",
        }
    }
}

/// A screen of any kind, put together from the components: the list,
/// skin, description block, header and footer of its kind, and the inputs
/// its rows and legend need.
#[derive(Clone, Debug)]
pub struct Screen {
    spec: ScreenSpec,
}

impl Screen {
    /// The Controls list over the game shell (header layout 0), its legend
    /// in the footer.
    pub fn list(name: &str, title: &str) -> Screen {
        let mut spec = ScreenSpec::new(name, title);
        spec.footer = true;
        Screen { spec }
    }
    /// A list moved and reskinned, values as `style` puts them.
    pub fn moved(name: &str, title: &str, layout: ListLayout, style: ValueStyle) -> Screen {
        let mut s = Screen::list(name, title);
        s.spec.list = ListPlace::from(Some(layout));
        s.spec.style = style;
        s
    }
    /// The level select's list (the skulls pages'): moved
    /// left, values inline.
    pub fn level_select(name: &str, title: &str) -> Screen {
        Screen::moved(name, title, crate::skulls_menu::LIST_LAYOUT, ValueStyle::Inline)
    }
    /// A dialog on `base` over `opener`: its value style and its help
    /// block, where the base has one. A settings dialog is its doc's
    /// (`crate::ui_kit::settings`), not built here.
    pub fn dialog(name: &str, title: &str, base: DialogBase, opener: Opener) -> Screen {
        let mut spec = ScreenSpec::new(name, title);
        spec.footer = true;
        spec.style = base.style();
        spec.description = base.help().map(DescriptionBlock::from);
        spec.dialog = Some(DialogSpec {
            base,
            ..DialogSpec::new("", opener)
        });
        Screen { spec }
    }
    /// The large dialog (header layout 2).
    pub fn large(name: &str, title: &str, opener: Opener) -> Screen {
        Screen::dialog(name, title, DialogBase::Large, opener)
    }
    /// A message on the large dialog: `body`, no rows.
    pub fn message(name: &str, title: &str, body: &str, opener: Opener) -> Screen {
        let mut s = Screen::large(name, title, opener);
        if let Some(d) = &mut s.spec.dialog {
            d.body = body.into();
        }
        s
    }
    /// Code entry: a message with a row of cells.
    pub fn code_entry(name: &str, title: &str, body: &str, cells: Cells, opener: Opener) -> Screen {
        let mut s = Screen::message(name, title, body, opener);
        if let Some(d) = &mut s.spec.dialog {
            d.cells = Some(cells);
        }
        s
    }
    /// The value picker over `opener` ([`crate::ui_kit::picker`]'s own
    /// layout and legend).
    pub fn picker(
        name: &str,
        title: &str,
        values: &[&str],
        focus: usize,
        help: &str,
        opener: Opener,
    ) -> Screen {
        let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
        Screen {
            spec: crate::ui_kit::picker::picker_spec(
                name,
                DialogBase::ValuePicker,
                title,
                &values,
                focus,
                help,
                opener,
            ),
        }
    }

    pub fn row(mut self, row: RowSpec) -> Screen {
        self.spec.rows.push(row);
        self
    }
    pub fn legend(mut self, prompts: Vec<Prompt>) -> Screen {
        self.spec.prompts = prompts;
        self
    }
    /// The legend under the description, on the panel's edge, rather than
    /// in the footer.
    pub fn legend_on_panel(mut self) -> Screen {
        self.spec.footer = false;
        self
    }
    /// The focused row's description in `block`; each row has one.
    pub fn description(mut self, block: TextBlock) -> Screen {
        self.spec.description = Some(block.into());
        self.spec.content.row_help = true;
        self
    }
    /// The screen's own line in the description block.
    pub fn help(mut self, text: &str) -> Screen {
        self.spec.help = Some(text.into());
        self.spec.content.row_help = false;
        self
    }
    pub fn text(mut self, t: PaneText) -> Screen {
        self.spec.texts.push(t);
        self
    }
    pub fn art(mut self, a: PaneArt) -> Screen {
        self.spec.art.push(a);
        self
    }
    pub fn scrolls(mut self) -> Screen {
        self.spec.scrolls = true;
        self
    }
    /// A dialog's status line.
    pub fn status(mut self, text: &str) -> Screen {
        if let Some(d) = &mut self.spec.dialog {
            d.status = Some(text.into());
        }
        self
    }
    /// Drawn over a kit screen of ours rather than its opener.
    pub fn over(mut self, under: ScreenSpec) -> Screen {
        self.spec.underlay = Some(Box::new(under));
        self
    }

    /// The spec: a dialog's rows in its base's list, and the inputs the
    /// rows and the legend need (up, down, A and B on any screen but a
    /// HUD; left and right for a stepping row or cells; each legend
    /// glyph's input).
    pub fn build(self) -> ScreenSpec {
        let mut s = self.spec;
        if let Some(d) = &mut s.dialog {
            d.rows = s.rows.clone();
            if !s.rows.is_empty() && !s.content.picker {
                s.list = ListPlace::from(Some(crate::ui_kit::dialog_tag::base_layout(
                    d.base,
                    s.rows.len(),
                )));
            }
        }
        let mut inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
        let steps = s
            .rows
            .iter()
            .any(|r| matches!(r.kind, RowKind::Value { .. }) && r.press == Press::Step);
        let cells = s.dialog.as_ref().is_some_and(|d| d.cells.is_some());
        if steps || cells {
            inputs.extend([Input::Left, Input::Right]);
        }
        let glyphs: Vec<Glyph> = s
            .prompts
            .iter()
            .chain(s.rows.iter().flat_map(|r| r.prompts.iter().flatten()))
            .flat_map(|p| p.glyphs.iter().copied())
            .collect();
        for g in glyphs {
            if let Some(i) = input_of(g)
                && !inputs.contains(&i)
            {
                inputs.push(i);
            }
        }
        s.inputs = inputs;
        s
    }
}

/// A row of `digits` entry cells, the cursor's lit, a wider gap after
/// `gap_after` (a code in two halves).
pub fn dialog_cells(digits: &str, cursor: Option<usize>, gap_after: Option<usize>) -> Cells {
    Cells {
        text: digits.chars().map(String::from).collect(),
        cursor,
        gap_after,
    }
}

/// The input that raises glyph `g`, when a screen of ours takes one.
pub fn input_of(g: Glyph) -> Option<Input> {
    [
        Input::Up,
        Input::Down,
        Input::Left,
        Input::Right,
        Input::A,
        Input::B,
        Input::X,
        Input::Y,
        Input::Start,
        Input::Back,
    ]
    .into_iter()
    .find(|&i| Glyph::of(i) == g)
}

/// The catalogue as the agent API gives it (`ui.components`): each
/// component of `design.yaml` with its role, font, colour, justification,
/// variants and specimen page; each layout page with its kind and states;
/// the palette and the fonts by role.
pub fn catalogue_json() -> serde_json::Value {
    let d = crate::ui_kit::data::design();
    let components: Vec<serde_json::Value> = d
        .components
        .iter()
        .map(|(name, c)| {
            serde_json::json!({
                "name": name,
                "role": c.role,
                "font": c.font,
                "colour": c.colour,
                "justify": c.justify,
                "variants": c.variants,
                "specimen": c.specimen,
            })
        })
        .collect();
    let layouts: Vec<serde_json::Value> = crate::ui_kit::screens::NAMES
        .iter()
        .filter(|n| n.starts_with("layout."))
        .map(|n| {
            let live = crate::ui_kit::screens::live(n);
            let kind = live.as_ref().map(|l| Kind::of(&l.spec()).name());
            serde_json::json!({
                "name": n,
                "kind": kind,
                "states": live.map(|l| l.state_names()).unwrap_or_default(),
            })
        })
        .collect();
    serde_json::json!({
        "components": components,
        "layouts": layouts,
        "palette": d.palette_rgb().into_iter().map(|(n, c)| serde_json::json!({"name": n, "rgb": c})).collect::<Vec<_>>(),
        "fonts": d.fonts,
    })
}

/// Page `name` (`design.<component>` or `layout.<name>`), live.
pub fn live(name: &str) -> Option<Box<dyn crate::ui_kit::page::Live>> {
    Some(Specimen::new(specimens::states(name)?).live())
}

#[cfg(test)]
mod tests;
