//! Content contracts shared by the page-model and screen checks.

use serde::Serialize;

use super::Finding;
use crate::ui_kit::input::{Glyph, Input, Prompt};
use crate::ui_kit::spec::{Press, RowKind, RowSpec, ScreenSpec};
use crate::ui_kit::view::{Kind, View};

pub const DEAD_CONTROL: &str = "dead_control";
pub const MISSING_CONTENT: &str = "missing_content";

/// Slots whose purpose cannot be inferred from their position or label.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Requirements {
    /// The visible message widget when this state has no items to show.
    /// A catalog can still have filters or a Retry row in this state.
    pub empty_text: Option<String>,
    /// Icon and emblem widgets this state must show, by view id.
    pub required_icons: Vec<String>,
    /// Value slots whose source explicitly has no value in this state.
    pub unavailable_values: Vec<String>,
    /// Each row is meant to fill the description slot.
    pub row_help: bool,
    /// This is a picker, including bases shared with other dialogs.
    pub picker: bool,
}

impl Requirements {
    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

fn rows(s: &ScreenSpec) -> &[RowSpec] {
    s.dialog.as_ref().map_or(&s.rows, |d| &d.rows)
}

fn prompts(s: &ScreenSpec) -> &[Prompt] {
    if s.dialog.is_none() && s.footer {
        if let Some(p) = rows(s).get(s.focus).and_then(|r| r.prompts.as_deref()) {
            return p;
        }
    }
    &s.prompts
}

const INPUTS: [Input; 10] = [
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
];

fn input_of(glyph: Glyph) -> Option<Input> {
    INPUTS.into_iter().find(|&i| Glyph::of(i) == glyph)
}

fn input_target(s: &ScreenSpec, input: Input) -> String {
    if input == Input::A && !rows(s).is_empty() {
        format!("row.{}", s.focus)
    } else {
        "legend".into()
    }
}

#[cfg(test)]
fn active_confirm(s: &ScreenSpec) -> bool {
    rows(s)
        .get(s.focus)
        .is_some_and(|r| r.press != Press::Nothing)
}

/// Whether the model owes a response to an input in this state. Directional
/// list navigation is checked separately so list endpoints may stop.
#[cfg(test)]
pub(super) fn promises_input(s: &ScreenSpec, input: Input) -> bool {
    (input == Input::A && active_confirm(s))
        || prompts(s)
            .iter()
            .any(|p| p.glyphs.contains(&Glyph::of(input)))
}

/// Checks the actions exercised on a fresh model in the same state.
/// A multi-button prompt can work in only one direction at an endpoint.
#[cfg(test)]
pub(super) fn check_prompt_effects(s: &ScreenSpec, results: &[(Input, bool)]) -> Vec<Finding> {
    let mut out = Vec::new();
    let handled = |input| {
        results
            .iter()
            .find(|(candidate, _)| *candidate == input)
            .map(|(_, handled)| *handled)
    };
    for p in prompts(s) {
        let inputs: Vec<_> = p
            .glyphs
            .iter()
            .filter_map(|&glyph| input_of(glyph))
            .collect();
        if !inputs.is_empty() && inputs.iter().all(|&input| handled(input) == Some(false)) {
            let target = if inputs.contains(&Input::A) {
                input_target(s, Input::A)
            } else {
                "legend".into()
            };
            out.push(Finding::new(
                DEAD_CONTROL,
                target,
                format!("{:?} has no action in this state", p.text),
            ));
        }
    }
    if active_confirm(s)
        && !prompts(s).iter().any(|p| p.glyphs.contains(&Glyph::A))
        && handled(Input::A) == Some(false)
    {
        out.push(Finding::new(
            DEAD_CONTROL,
            input_target(s, Input::A),
            "A has no action in this state",
        ));
    }
    out
}

/// A reachability probe supplies one bit for each row that Up or Down can
/// reach from the model's initial focus.
#[cfg(test)]
pub(super) fn check_reachability(s: &ScreenSpec, reached: &[bool]) -> Vec<Finding> {
    rows(s)
        .iter()
        .enumerate()
        .filter(|(i, _)| !reached.get(*i).copied().unwrap_or(false))
        .map(|(i, _)| {
            Finding::new(
                DEAD_CONTROL,
                format!("row.{i}"),
                "focus cannot reach this row",
            )
        })
        .collect()
}

fn text_present(view: &View, id: &str) -> bool {
    view.widget(id)
        .filter(|w| w.alpha != 0)
        .and_then(|w| w.text())
        .is_some_and(|t| !t.trim().is_empty())
}

fn art_present(view: &View, id: &str) -> bool {
    view.widget(id).is_some_and(|w| {
        w.alpha != 0 && matches!(w.kind, Kind::Art { size: [w, h], .. } if w != 0 && h != 0)
    })
}

fn missing(out: &mut Vec<Finding>, id: impl Into<String>, detail: &str) {
    out.push(Finding::new(MISSING_CONTENT, id, detail));
}

fn is_sentinel(text: &str) -> bool {
    matches!(text.trim(), "—" | "-") || text.trim().eq_ignore_ascii_case("Unavailable")
}

fn check_value(out: &mut Vec<Finding>, s: &ScreenSpec, id: &str, text: &str) {
    if is_sentinel(text) && !s.content.unavailable_values.iter().any(|v| v == id) {
        missing(out, id, "value still shows the unavailable placeholder");
    }
}

fn check_rows(out: &mut Vec<Finding>, s: &ScreenSpec, view: &View, rows: &[RowSpec]) {
    let per_row_help = s.content.row_help
        || rows.iter().any(|r| {
            r.description
                .as_deref()
                .is_some_and(|d| !d.trim().is_empty())
        });
    let has_help = s.help.as_deref().is_some_and(|h| !h.trim().is_empty());
    for (i, row) in rows.iter().enumerate() {
        let prefix = format!("row.{i}");
        let label = format!("{prefix}.label");
        let value = format!("{prefix}.value");
        if row.label.trim().is_empty()
            || ((!s.scrolls || i == s.focus) && !text_present(view, &label))
        {
            missing(out, label, "row has no visible label");
        }
        // Info rows intentionally take focus for reading and can still
        // advertise screen-wide buttons. Other row kinds owe an action.
        if (row.press == Press::Nothing && !matches!(row.kind, RowKind::Info { .. }))
            || matches!(&row.press, Press::Open(name) if name.trim().is_empty())
            || (row.press != Press::Nothing && !s.inputs.contains(&Input::A))
        {
            out.push(Finding::new(
                DEAD_CONTROL,
                &prefix,
                "row has no usable confirm action",
            ));
        }
        if let RowKind::Value { values, current } = &row.kind
            && values.get(*current).is_none_or(|v| v.trim().is_empty())
        {
            missing(out, &value, "value row has no selected option");
        }
        if let Some(v) = row.plain_value() {
            check_value(out, s, &value, &v);
        }
        let row_help = row
            .description
            .as_deref()
            .is_some_and(|d| !d.trim().is_empty());
        if per_row_help && !has_help && !row_help {
            missing(
                out,
                format!("{prefix}.description"),
                "row has no help for the description slot",
            );
        }
        if i == s.focus
            && (has_help || row_help)
            && (s.description.is_none() || !text_present(view, "description"))
        {
            missing(
                out,
                "description",
                "declared help has no visible description widget",
            );
        }
    }
}

pub(super) fn check(s: &ScreenSpec, view: &View) -> Vec<Finding> {
    let mut out = Vec::new();
    let rows = rows(s);
    check_rows(&mut out, s, view, rows);
    if rows.len() > 1
        && !s
            .inputs
            .iter()
            .any(|input| matches!(input, Input::Up | Input::Down))
    {
        out.push(Finding::new(
            DEAD_CONTROL,
            "list",
            "focus has no navigation input for this list",
        ));
    }

    let shown_prompts = prompts(s);
    for prompt in shown_prompts
        .iter()
        .filter(|prompt| prompt.glyphs.is_empty())
    {
        out.push(Finding::new(
            DEAD_CONTROL,
            "legend",
            format!("{:?} has no controller button", prompt.text),
        ));
    }
    if let Some(row) = rows.get(s.focus)
        && row.press == Press::Nothing
        && row.prompts.as_ref().is_some_and(|prompts| {
            prompts
                .iter()
                .any(|prompt| prompt.glyphs.contains(&Glyph::A))
        })
    {
        out.push(Finding::new(
            DEAD_CONTROL,
            input_target(s, Input::A),
            "row advertises A but has no action",
        ));
    }
    let glyphs = shown_prompts.iter().flat_map(|prompt| &prompt.glyphs);
    for &glyph in glyphs {
        if input_of(glyph).is_none_or(|input| !s.inputs.contains(&input)) {
            out.push(Finding::new(
                DEAD_CONTROL,
                "legend",
                format!("{} is not handled by this page", glyph.name()),
            ));
        }
    }

    let list = s.dialog.is_none()
        && view.list.is_some()
        && (s.scrolls || !s.inputs.is_empty() || !s.title.trim().is_empty());
    let picker = s.content.picker
        || s.dialog
            .as_ref()
            .is_some_and(|d| d.base == crate::ui_kit::dialog::DialogBase::ValuePicker);
    if let Some(id) = &s.content.empty_text {
        if !text_present(view, id) {
            missing(&mut out, id, "empty list has no visible message");
        }
    } else if list && rows.is_empty() {
        missing(&mut out, "list", "empty list has no message");
    }
    if picker && rows.is_empty() {
        missing(&mut out, "list", "picker has no options");
    }

    if let Some(dialog) = &s.dialog {
        for (id, text) in [
            ("body", Some(dialog.body.as_str())),
            ("status", dialog.status.as_deref()),
        ] {
            if text.is_some_and(|text| !text.trim().is_empty()) && !text_present(view, id) {
                missing(&mut out, id, "declared dialog text has no visible widget");
            }
        }
    }
    for id in &s.content.required_icons {
        if !art_present(view, id) {
            missing(&mut out, id, "icon slot has no image");
        }
    }
    for (i, _) in s.shown_art().iter().enumerate() {
        let id = format!("art.{i}");
        if !s.content.required_icons.contains(&id) && !art_present(view, &id) {
            missing(&mut out, id, "declared art has no visible image");
        }
    }
    for t in &s.texts {
        check_value(&mut out, s, &t.id, &t.text);
        if !t.text.trim().is_empty() && !text_present(view, &t.id) {
            missing(&mut out, &t.id, "declared text has no visible widget");
        }
    }
    out
}

#[cfg(test)]
mod tests;
