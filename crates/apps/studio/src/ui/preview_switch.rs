//! The **Preview** menu (docs/editor/studio-redesign.md, "Preview and the
//! paired game"): the front document's preview target (Studio, My game,
//! Attached) and **In game**. The items are
//! [`Action::SetPreviewTarget`] and [`Action::PreviewInGame`], the
//! commands `preview.target` and `pair.preview` run.

use dear_imgui_rs::Ui;

use crate::app::preview_switch::PreviewTarget;
use crate::app::{Action, App, Doc};

/// The menu's body.
pub(super) fn menu(ui: &Ui, app: &App, actions: &mut Vec<Action>) {
    let front = app.front_doc();
    targets(ui, app, front, actions);
    ui.separator();
    let ready = in_game(app, front);
    if ui.menu_item_enabled_selected_with_shortcut("In game", "Cmd+Enter", false, ready.is_ok()) {
        if let Ok(action) = ready {
            actions.push(action);
        }
    } else {
        explain(ui, &ready);
    }
}

pub(super) fn targets(ui: &Ui, app: &App, front: Option<Doc>, actions: &mut Vec<Action>) {
    let current = front.map(|d| app.preview_target(d));
    for target in PreviewTarget::ALL {
        let ready = front.ok_or_else(|| "no document is open".to_owned())
            .and_then(|doc| app.preview_target_available(target).map(|()| doc));
        if ui.menu_item_enabled_selected_no_shortcut(
            target.label(),
            current == Some(target),
            ready.is_ok(),
        ) && let Ok(doc) = &ready
        {
            actions.push(Action::SetPreviewTarget(*doc, target));
        }
        explain(ui, &ready);
    }
}

pub(super) fn in_game(app: &App, doc: Option<Doc>) -> Result<Action, String> {
    let doc = doc.ok_or("no document is open")?;
    app.in_game_available(doc)?;
    Ok(Action::PreviewInGame(Some(doc)))
}

pub(super) fn explain<T>(ui: &Ui, ready: &Result<T, String>) {
    if let Err(why) = ready
        && ui.is_item_hovered_with_flags(dear_imgui_rs::ItemHoveredFlags::ALLOW_WHEN_DISABLED)
    {
        ui.tooltip_text(why);
    }
}
