//! The document tab strip in the bar before the game
//! link (docs/editor/studio-redesign.md, "The shell"): one tab per open
//! document, the front one in the accent colour, each with its close
//! button. Clicks become [`Action::FocusDoc`] and [`Action::CloseDoc`],
//! the commands `shell.focus` and `shell.close` run. Tabs that do not fit
//! go in a **More** menu at the end of the row. The strip sits in the bar,
//! so the docks under it never move.

use dear_imgui_rs::{StyleColor, Ui};

use crate::app::{Action, App, Doc};
use crate::theme;

/// The strip's label for a tab, with its unsaved mark, and its stable id.
fn label(app: &App, doc: Doc) -> String {
    let mark = if app.doc_dirty(doc) { " *" } else { "" };
    format!("{}{mark}###doc-tab-{}", app.doc_title(doc), app.doc_id(doc))
}

/// Draw the strip from the cursor, on one row `avail` points wide.
pub(super) fn draw(ui: &Ui, app: &App, actions: &mut Vec<Action>, avail: f32) {
    let row = ui.cursor_pos()[1];
    let docs: Vec<Doc> = app.tabs().iter().map(|t| t.doc).collect();
    if docs.is_empty() {
        ui.align_text_to_frame_padding();
        theme::clipped_text(
            ui,
            "No documents open: double-click a tag, or open a map",
            theme::DIM,
            avail,
        );
        return;
    }
    let style = ui.clone_style();
    let spacing = style.item_spacing()[0];
    let pad = style.frame_padding()[0] * 2.0;
    let close_w = ui.calc_text_size("x")[0] + pad;
    let more_w = ui.calc_text_size("More")[0] + pad + spacing;
    let front = app.front_doc();
    let mut used = 0.0;
    let mut hidden = Vec::new();
    for (i, &doc) in docs.iter().enumerate() {
        let text = label(app, doc);
        let shown = text.split("###").next().unwrap_or_default();
        let width = ui.calc_text_size(shown)[0] + pad + spacing + close_w + spacing;
        let room = if i + 1 == docs.len() {
            avail
        } else {
            avail - more_w
        };
        if used + width > room && used > 0.0 {
            hidden.extend_from_slice(&docs[i..]);
            break;
        }
        if i > 0 {
            ui.same_line();
            ui.set_cursor_pos([ui.cursor_pos()[0], row]);
        }
        used += width;
        let tone =
            (front == Some(doc)).then(|| ui.push_style_color(StyleColor::Button, theme::ACCENT));
        if ui.button(&text) && front != Some(doc) {
            actions.push(Action::FocusDoc(doc));
        }
        drop(tone);
        ui.set_item_tooltip(format!(
            "{} ({})",
            app.doc_id(doc),
            crate::api::tool_name(app.doc_tool(doc))
        ));
        ui.same_line_with_spacing(0.0, 0.0);
        ui.set_cursor_pos([ui.cursor_pos()[0], row]);
        if ui.button(format!("x###doc-close-{}", app.doc_id(doc))) {
            actions.push(Action::CloseDoc(doc));
        }
        ui.set_item_tooltip("Close");
    }
    if hidden.is_empty() {
        return;
    }
    ui.same_line();
    ui.set_cursor_pos([ui.cursor_pos()[0], row]);
    if ui.button("More###doc-tabs-more") {
        ui.open_popup("##doc-tabs-more");
    }
    if let Some(_popup) = ui.begin_popup("##doc-tabs-more") {
        for doc in hidden {
            if ui.menu_item_enabled_selected_no_shortcut(label(app, doc), front == Some(doc), true)
            {
                actions.push(Action::FocusDoc(doc));
            }
        }
    }
}
