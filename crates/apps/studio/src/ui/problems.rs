//! The Problems panel in the bottom dock (docs/editor/studio-redesign.md,
//! "The shell"): one list over every check and build error, the rows
//! `problems.list` answers (`app::problems`). **Check** reads the list
//! again ([`Action::RefreshProblems`]); the source buttons narrow it; a
//! double-click on a row, or **Open** in its menu, opens what it is about
//! ([`Action::OpenProblem`]): the tag with the field shown, the package in
//! the browser, or the tool that holds the screen or the HUD.
use dear_imgui_rs::{MouseButton, StyleColor, Ui};

use crate::api::checks::Severity;
use crate::app::problems::{SOURCES, source_label};
use crate::app::{Action, App};
use crate::theme::{self, icon};

/// The panel's UI-only state.
#[derive(Default)]
pub struct ProblemsUi {
    /// The source the list is narrowed to.
    pub source: Option<&'static str>,
    selected: Option<usize>,
    /// Frames left to ask the dock to bring the panel to the front (a
    /// new tab reaches the dock's tab bar a frame later).
    pub focus: u8,
    /// The row indexes the last frame drew, for scripts and tests.
    pub shown: Vec<usize>,
}

fn tone(s: Severity) -> ([f32; 4], &'static str) {
    match s {
        Severity::Error => (theme::ERROR, icon::ALERT),
        Severity::Warning => (theme::WARN, icon::ALERT),
        Severity::Note => (theme::DIM, icon::INFO),
    }
}

/// One finding as a line: the severity's mark and the text, as the HUD and
/// screen editors list the findings of their picture. Whether it is hovered.
pub fn finding_line(ui: &Ui, severity: Severity, text: &str) -> bool {
    let (color, glyph) = tone(severity);
    ui.text_colored(color, glyph);
    ui.same_line();
    ui.text_wrapped(text);
    ui.is_item_hovered()
}

/// Draw the panel.
pub fn draw(ui: &Ui, app: &App, st: &mut ProblemsUi, actions: &mut Vec<Action>) {
    st.shown.clear();
    if ui.button(format!("{} Check##problems-check", icon::ROTATE)) {
        actions.push(Action::RefreshProblems(st.source.map(str::to_owned)));
    }
    ui.same_line();
    let list = &app.problems;
    if list.checked {
        let c = list.counts(st.source);
        ui.text(format!(
            "{} errors, {} warnings, {} notes",
            c["error"], c["warning"], c["note"]
        ));
    } else {
        ui.text_disabled("Not read yet: Check reads every source");
    }
    for (i, label) in std::iter::once((None, "All"))
        .chain(SOURCES.iter().map(|s| (Some(*s), source_label(s))))
        .enumerate()
    {
        let (value, text) = label;
        ui.same_line();
        if ui
            .selectable_config(format!("{text}##problems-source-{i}"))
            .selected(st.source == value)
            .size([ui.calc_text_size(text)[0] + 12.0, 0.0])
            .build()
        {
            st.source = value;
        }
    }
    ui.separator();
    let rows: Vec<usize> = list
        .rows
        .iter()
        .enumerate()
        .filter(|(_, p)| st.source.is_none_or(|s| p.source == s))
        .map(|(i, _)| i)
        .collect();
    theme::panel_body(ui, "##problems-rows", || {
        if rows.is_empty() {
            ui.text_disabled(if list.checked {
                "No problems."
            } else {
                "Press Check to read the list."
            });
        }
        for &i in &rows {
            let p = &list.rows[i];
            st.shown.push(i);
            let (color, glyph) = tone(p.severity);
            let target = if p.target.is_empty() {
                String::new()
            } else {
                format!("  [{}]", p.target)
            };
            let colored = ui.push_style_color(StyleColor::Text, color);
            let clicked = ui
                .selectable_config(format!(
                    "{glyph}  {}{target}##problem-{i}",
                    p.message
                ))
                .selected(st.selected == Some(i))
                .build();
            drop(colored);
            if clicked {
                st.selected = Some(i);
            }
            if ui.is_item_hovered() {
                ui.tooltip(|| {
                    ui.text(&p.code);
                    if let Some(a) = &p.asset {
                        ui.text_disabled(a);
                    }
                    if let Some(f) = &p.field {
                        ui.text_disabled(format!("field {f}"));
                    }
                    if let Some(path) = &p.path {
                        ui.text_disabled(path);
                    }
                });
                if ui.is_mouse_double_clicked(MouseButton::Left) {
                    actions.push(Action::OpenProblem(i));
                }
            }
            if let Some(_menu) = ui.begin_popup_context_item() {
                let opens = p.asset.is_some() || p.tool.is_some();
                if ui.menu_item_enabled_selected_no_shortcut("Open", false, opens) {
                    actions.push(Action::OpenProblem(i));
                }
            }
        }
    });
}
