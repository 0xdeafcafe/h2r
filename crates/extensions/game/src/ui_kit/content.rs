//! Content findings for the offline builder and the live UI watch.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use serde::Serialize;

use super::preview::overflows;
use super::spec::{RowKind, ScreenSpec};
use super::view::{Art, Kind, Metrics, View};
use crate::list_class::Rows;

mod controls;
pub use controls::Requirements;

/// A problem in one authored or observed piece of UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub kind: &'static str,
    pub id: String,
    pub detail: String,
}

impl Finding {
    pub fn new(kind: &'static str, id: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.id, self.detail)
    }
}

static ROWS: Mutex<BTreeMap<u32, Weak<dyn Rows>>> = Mutex::new(BTreeMap::new());

/// A class's authored rows, indexed by the vtable used by its live screen.
pub(crate) fn register_rows(vtable: u32, rows: Arc<dyn Rows>) {
    let mut registry = ROWS.lock().unwrap_or_else(|e| e.into_inner());
    registry.retain(|_, rows| rows.strong_count() > 0);
    registry.insert(vtable, Arc::downgrade(&rows));
}

pub(crate) fn runtime_spec(vtable: u32, focus: Option<usize>) -> Option<ScreenSpec> {
    let rows = ROWS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&vtable)?
        .upgrade()?;
    let mut spec = rows.content_spec()?;
    if let Some(focus) = focus {
        spec.focus = focus;
    }
    Some(spec)
}

fn identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c == '.' || c.is_ascii_alphanumeric())
}

fn placeholder(text: &str) -> bool {
    let brace = text.split('{').skip(1).any(|tail| {
        tail.split_once('}')
            .is_some_and(|(field, _)| identifier(field.split(':').next().unwrap_or(field)))
    });
    let dollar = text.split('$').skip(1).any(|tail| {
        tail.strip_prefix('{')
            .unwrap_or(tail)
            .chars()
            .next()
            .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
    });
    let printf = text.split('%').skip(1).any(|tail| {
        let mut conversion = tail
            .trim_start_matches(|c: char| {
                matches!(c, '-' | '+' | '.' | '#' | '*' | '$') || c.is_ascii_digit()
            })
            .chars();
        conversion
            .next()
            .is_some_and(|c| "diouxXfeEgGcs".contains(c))
            && !conversion
                .next()
                .is_some_and(|c| c == '_' || c.is_ascii_alphanumeric())
    });
    brace || dollar || printf
}

fn raw_key(text: &str) -> bool {
    identifier(text)
        && ([
            "ui.", "menu.", "screen.", "text.", "string.", "loc.", "ui_", "menu_", "string_",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
            || (text.contains('_') && text.chars().all(|c| !c.is_ascii_lowercase())))
}

fn raw_id(word: &str) -> bool {
    word.strip_prefix("0x")
        .or_else(|| word.strip_prefix("0X"))
        .is_some_and(|hex| !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn mojibake(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{fffd}' | '\u{0080}'..='\u{009f}'))
        || ["Ã©", "Ã¨", "Ã±", "Ã¼", "Ã¶", "Â ", "â€", "ðŸ", "ï»¿"]
            .iter()
            .any(|bad| text.contains(bad))
}

fn prompt_glyph(c: char) -> bool {
    ('\u{e100}'..='\u{e1ff}').contains(&c)
}

fn bad_spacing(line: &str) -> bool {
    if line.ends_with([' ', '\t']) || line.contains('\t') {
        return true;
    }
    // Footers deliberately put extra space just before their next glyph.
    let mut spaces = 0;
    for ch in line.chars() {
        if ch == ' ' {
            spaces += 1;
        } else {
            if spaces > 1 && !prompt_glyph(ch) {
                return true;
            }
            spaces = 0;
        }
    }
    false
}

/// Checks copy before wrapping, so padding and unresolved tokens cannot
/// disappear when a preview splits a paragraph into lines.
pub(crate) fn check_text(id: &str, text: &str, required: bool) -> Vec<Finding> {
    if text.trim().is_empty() {
        return if required {
            vec![Finding::new("empty_text", id, "Required text is empty")]
        } else {
            Vec::new()
        };
    }
    let words = || text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    let mut out = Vec::new();
    for (bad, kind, detail) in [
        (
            placeholder(text),
            "placeholder",
            "Unresolved format placeholder",
        ),
        (
            raw_key(text.trim()) || matches!(text.trim(), "?" | "???"),
            "raw_key",
            "Unresolved string key",
        ),
        (words().any(raw_id), "raw_id", "Raw hexadecimal ID"),
        (
            words().any(|w| {
                ["TODO", "FIXME", "xxx"]
                    .iter()
                    .any(|t| w.eq_ignore_ascii_case(t))
            }),
            "unfinished_text",
            "Editor marker in player-facing text",
        ),
        (
            mojibake(text),
            "invalid_text",
            "Replacement character or damaged encoding",
        ),
        (
            text.lines().any(bad_spacing),
            "text_spacing",
            "Doubled or trailing whitespace",
        ),
        (
            matches!(text.trim(), "<" | ">")
                || text
                    .split_once('<')
                    .is_some_and(|(_, tail)| tail.contains('>')),
            "value_arrows",
            "Angle arrows in player-facing text",
        ),
    ] {
        if bad {
            out.push(Finding::new(kind, id, detail));
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LetterCase {
    Upper,
    Lower,
    Mixed,
}

fn label_case(label: &str) -> Option<LetterCase> {
    let mut upper = 0;
    let mut lower = 0;
    for ch in label.chars() {
        upper += usize::from(ch.is_uppercase());
        lower += usize::from(ch.is_lowercase());
    }
    match (upper, lower) {
        // A short all-caps word often names a game mode or a controller button.
        (0..=4, 0) if !label.contains(' ') => None,
        (0, 0) => None,
        (_, 0) => Some(LetterCase::Upper),
        (0, _) => Some(LetterCase::Lower),
        _ => Some(LetterCase::Mixed),
    }
}

pub(crate) fn case_findings(labels: impl IntoIterator<Item = (String, String)>) -> Vec<Finding> {
    let labels: Vec<_> = labels
        .into_iter()
        .filter_map(|(id, label)| label_case(&label).map(|case| (id, case)))
        .collect();
    let Some((_, first)) = labels.first() else {
        return Vec::new();
    };
    if labels.iter().all(|(_, case)| case == first) {
        return Vec::new();
    }
    vec![Finding::new(
        "inconsistent_case",
        &labels[0].0,
        "Row labels mix all-caps, lower-case or mixed-case styles",
    )]
}

fn spec_text(spec: &ScreenSpec) -> Vec<Finding> {
    let mut findings = check_text(
        "title",
        &spec.title,
        !spec.rows.is_empty() || spec.dialog.is_some(),
    );
    let rows = spec
        .dialog
        .as_ref()
        .map_or(&spec.rows, |dialog| &dialog.rows);
    findings.extend(case_findings(
        rows.iter()
            .enumerate()
            .map(|(i, row)| (format!("row.{i}.label"), row.label.clone())),
    ));
    for (i, row) in rows.iter().enumerate() {
        findings.extend(check_text(&format!("row.{i}.label"), &row.label, true));
        if let Some(description) = &row.description {
            findings.extend(check_text(&format!("row.{i}.help"), description, false));
        }
        match &row.kind {
            RowKind::Value { values, .. } => {
                for (j, value) in values.iter().enumerate() {
                    findings.extend(check_text(&format!("row.{i}.choice.{j}"), value, true));
                }
            }
            RowKind::Text { value, .. } | RowKind::Info { value } => {
                findings.extend(check_text(&format!("row.{i}.value"), value, false));
            }
            RowKind::Action => {}
        }
    }
    for text in &spec.texts {
        findings.extend(check_text(&text.id, &text.text, false));
    }
    if let Some(help) = &spec.help {
        findings.extend(check_text("help", help, false));
    }
    if let Some(dialog) = &spec.dialog {
        findings.extend(check_text("body", &dialog.body, false));
        if let Some(status) = &dialog.status {
            findings.extend(check_text("status", status, false));
        }
    }
    let prompts = spec.prompts.iter().chain(
        spec.rows
            .iter()
            .flat_map(|row| row.prompts.iter().flatten()),
    );
    for (i, prompt) in prompts.enumerate() {
        findings.extend(check_text(&format!("prompt.{i}"), &prompt.text, true));
    }
    findings
}

fn view_text(view: &View, metrics: &dyn Metrics) -> Vec<Finding> {
    let mut findings = Vec::new();
    for widget in view.widgets.iter().filter(|w| w.alpha > 0) {
        let Kind::Text { text, font, .. } = &widget.kind else {
            continue;
        };
        findings.extend(check_text(&widget.id, text, false));
        for ch in text.chars().filter(|c| !c.is_control()) {
            if !metrics.has_glyph(*font, ch) {
                let finding = Finding::new(
                    "missing_glyph",
                    &widget.id,
                    format!("Font {font} cannot draw U+{:04X}", ch as u32),
                );
                if !findings.contains(&finding) {
                    findings.push(finding);
                }
            }
        }
        if overflows(widget, metrics) {
            findings.push(Finding::new(
                "truncated_text",
                &widget.id,
                "Text exceeds its box at this font size",
            ));
        }
    }
    findings
}

fn check_icons(
    spec: &ScreenSpec,
    view: &View,
    available: impl Fn(&Art) -> Option<bool>,
) -> Vec<Finding> {
    spec.content
        .required_icons
        .iter()
        .filter_map(|id| {
            let Kind::Art { art, .. } = &view.widget(id)?.kind else {
                return None;
            };
            (available(art) == Some(false)).then(|| {
                Finding::new(
                    controls::MISSING_CONTENT,
                    id,
                    "icon slot's image could not be resolved",
                )
            })
        })
        .collect()
}

/// Content checks including required images when the preview source has assets.
pub fn check_with_source(
    spec: &ScreenSpec,
    view: &View,
    source: &dyn super::preview::Source,
) -> Vec<Finding> {
    let mut findings = check(spec, view, source);
    findings.extend(check_icons(spec, view, |art| source.art_available(art)));
    findings
}

/// All content checks for a named state at its resting layout.
pub fn check(spec: &ScreenSpec, view: &View, metrics: &dyn Metrics) -> Vec<Finding> {
    let mut findings = spec_text(spec);
    for finding in view_text(view, metrics)
        .into_iter()
        .chain(controls::check(spec, view))
    {
        if !findings.contains(&finding) {
            findings.push(finding);
        }
    }
    findings
}

#[cfg(test)]
mod tests;
