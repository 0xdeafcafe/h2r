//! The **Preview** menu (docs/editor/studio-redesign.md, "Preview and the
//! paired game"): the front document's preview target (Studio, My game,
//! Attached) and **Preview in game**. The items are
//! [`Action::SetPreviewTarget`] and [`Action::PreviewInGame`], the
//! commands `preview.target` and `pair.preview` run.

use dear_imgui_rs::Ui;

use crate::app::preview_switch::PreviewTarget;
use crate::app::{Action, App};

/// The menu's body.
pub(super) fn menu(ui: &Ui, app: &App, actions: &mut Vec<Action>) {
    let front = app.front_doc();
    let current = front.map(|d| app.preview_target(d));
    for target in PreviewTarget::ALL {
        let available = front.is_some() && app.preview_target_available(target).is_ok();
        if ui.menu_item_enabled_selected_no_shortcut(
            target.label(),
            current == Some(target),
            available,
        ) && let Some(doc) = front
        {
            actions.push(Action::SetPreviewTarget(doc, target));
        }
        if !available
            && ui.is_item_hovered_with_flags(dear_imgui_rs::ItemHoveredFlags::ALLOW_WHEN_DISABLED)
            && let Err(why) = app.preview_target_available(target)
        {
            ui.tooltip_text(why);
        }
    }
    ui.separator();
    let plan = front.map(|d| app.game_preview(d));
    let paired = [PreviewTarget::MyGame, PreviewTarget::Attached]
        .into_iter()
        .any(|t| app.preview_target_available(t).is_ok());
    let ready = paired && matches!(plan, Some(Ok(_)));
    if ui.menu_item_enabled_selected_with_shortcut("Preview in game", "Cmd+Enter", false, ready) {
        actions.push(Action::PreviewInGame(None));
    }
    let why = match &plan {
        None => Some("no document is open".to_owned()),
        Some(Err(why)) => Some(why.clone()),
        Some(Ok(_)) if !ready => Some("no game is paired (pair.list, pair.connect)".to_owned()),
        Some(Ok(_)) => None,
    };
    if let Some(why) = why
        && ui.is_item_hovered_with_flags(dear_imgui_rs::ItemHoveredFlags::ALLOW_WHEN_DISABLED)
    {
        ui.tooltip_text(why);
    }
}
