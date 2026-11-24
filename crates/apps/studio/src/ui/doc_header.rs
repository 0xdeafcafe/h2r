//! Shared document controls. See docs/editor/studio-flow.md, "The document header".
use dear_imgui_rs::{ItemHoveredFlags, Ui};
use crate::app::{Action, App, Doc, NavAction, Surface};
use crate::app::assets::{self, AssetId};
use crate::app::links::{self, Links};

fn asset(app: &App, doc: Doc) -> Option<String> {
    match doc {
        Doc::Tag(id) => app.tags.tag(id).map(|t| format!("tag:{}:{}", tags::defs::fourcc_text(t.group), t.name)),
        Doc::Map => app.session.scenario().and_then(|s| app.tags.tag(s.tag)).map(|t| format!("tag:{}:{}", tags::defs::fourcc_text(t.group), t.name)),
        Doc::Ui => match &app.ui_screens.pick {
            Some(crate::app::ui_screens::Pick::Retail { tag, .. }) => Some(format!("tag:wgit:{tag}")),
            _ => None,
        },
        _ => None,
    }
}

fn button(ui: &Ui, label: &str, enabled: bool, reason: &str) -> bool {
    let guard = ui.begin_disabled_with_cond(!enabled);
    let clicked = ui.button(label);
    drop(guard);
    if !enabled && ui.is_item_hovered_with_flags(ItemHoveredFlags::ALLOW_WHEN_DISABLED) {
        ui.tooltip_text(reason);
    }
    clicked && enabled
}

fn load(app: &App, id: &str) -> Result<Links, String> {
    let id = AssetId::parse(id)?;
    links::links(&app.tags_root(), &assets::read_set(&app.data_root()), &id)
}

fn link_role(row: &links::Link) -> &'static str { row.role }

fn target_text(target: Option<String>, dirty: bool) -> String {
    format!("into: {}{}", target.unwrap_or_else(|| "no package".into()), if dirty { " *" } else { "" })
}

fn link_rows(ui: &Ui, rows: &[links::Link], actions: &mut Vec<Action>, browser: &mut super::browser::BrowserUi, section: &str) {
    let mut counts = std::collections::BTreeMap::new();
    for row in rows {
        let count = counts.entry(link_role(row)).or_insert(0usize);
        *count += 1;
        if *count > 20 { continue; }
        if *count == 1 { ui.separator(); ui.text_disabled(link_role(row)); }
        if ui.menu_item(&row.asset) {
            actions.push(Action::OpenAsset { asset: row.asset.clone(), map: None });
        }
        if ui.is_item_hovered() { ui.tooltip_text(&row.via); }
    }
    if rows.is_empty() { ui.text_disabled("None"); }
    if !rows.is_empty() && ui.menu_item(format!("Show all in the Browser##{section}")) {
        browser.show_links(rows);
    }
}

/// Draw in the active document's existing window, never in a child window.
pub(super) fn draw(ui: &Ui, app: &App, actions: &mut Vec<Action>, browser: &mut super::browser::BrowserUi) {
    let Some(doc) = app.front_doc() else { return; };
    let id = asset(app, doc);
    if button(ui, "<", app.can_navigate(false), "No earlier location") { actions.push(Action::Navigate(NavAction::Back)); }
    ui.same_line();
    if button(ui, ">", app.can_navigate(true), "No later location") { actions.push(Action::Navigate(NavAction::Forward)); }
    ui.same_line();
    if let Some(id) = &id {
        if let Ok(AssetId::Tag(key)) = AssetId::parse(id) {
            let group = tags::defs::fourcc_text(key.group);
            if ui.small_button(app.edit_target().unwrap_or_else(|| "Disc".into())) { browser.locate(id); }
            ui.same_line(); ui.text(">"); ui.same_line();
            if ui.small_button(assets::kind_of(&group)) { browser.locate(id); browser.query.clear(); browser.kind = Some(assets::kind_of(&group).to_owned()); }
            ui.same_line(); ui.text(">"); ui.same_line();
            if ui.small_button(&key.path) { browser.locate(id); }
            ui.same_line(); if ui.small_button("In Browser") { browser.locate(id); }
            let badge = assets::badge_for(&assets::read_set(&app.data_root()), &key);
            ui.text(format!("{}  {}", group, badge));
        }
    } else { ui.text(app.doc_title(doc)); }
    ui.text(target_text(app.edit_target(), app.doc_dirty(doc)));
    ui.same_line();
    if ui.button("Preview") { ui.open_popup("##doc-preview"); }
    if let Some(_popup) = ui.begin_popup("##doc-preview") { super::preview_switch::menu(ui, app, actions); }
    ui.same_line();
    let plan = app.game_preview(doc);
    if button(ui, "In game", plan.is_ok(), "This document cannot preview in game") { actions.push(Action::PreviewInGame(Some(doc))); }
    if ui.button("Check") { actions.push(Action::RefreshProblems(None)); }
    ui.same_line();
    let (undo, redo) = app.doc_can(doc);
    // FL8 extends the document stack; the header always dispatches its actions.
    if button(ui, "Undo", undo, "Nothing to undo") { actions.push(Action::UndoDoc(doc)); }
    ui.same_line();
    if button(ui, "Redo", redo, "Nothing to redo") { actions.push(Action::RedoDoc(doc)); }
    ui.same_line();
    let _ = button(ui, "Revert", false, "Revert is not available for this document yet");
    ui.same_line();
    let save = match doc { Doc::Variant => Some(Action::SaveVariant), Doc::Playlists => Some(Action::SaveHopper), Doc::Map | Doc::Tag(_) => Some(Action::Save), _ => None };
    if button(ui, "Save", save.is_some(), "Save is not available for this document") { actions.extend(save); }
    ui.same_line();
    if ui.button("Links") { ui.open_popup("##doc-links"); }
    if let Some(_popup) = ui.begin_popup("##doc-links") {
        match id.as_deref().map(|id| load(app, id)) {
            Some(Ok(l)) => { ui.text_disabled("Uses"); link_rows(ui, &l.uses, actions, browser, "uses"); ui.separator(); ui.text_disabled("Used by"); link_rows(ui, &l.used_by, actions, browser, "used-by"); for p in l.problems { ui.text_disabled(p); } }
            Some(Err(e)) => ui.text_disabled(e), None => ui.text_disabled("No asset for this document"),
        }
    }
    ui.same_line();
    if ui.button("Open with") { ui.open_popup("##doc-open-with"); }
    if let Some(_popup) = ui.begin_popup("##doc-open-with") {
        match id.as_deref().map(|id| load(app, id)) {
            Some(Ok(l)) => for e in l.editors {
                let action = match e.id { "voices" => Some(Action::ShowVoices), "field_panel" | "modeling" | "audition" | "animation" => Some(Action::Show(Surface::Tags)), "scenario" => Some(Action::Show(Surface::Scene)), "textures" => Some(Action::Show(Surface::Textures)), "hud" => Some(Action::Show(Surface::Hud)), "ui_screens" => Some(Action::Show(Surface::UiScreens)), "scripts" => Some(Action::Show(Surface::Scripts)), "variants" => Some(Action::Show(Surface::Variants)), "playlists" => Some(Action::Show(Surface::Playlists)), _ => None };
                let available = action.is_some();
                if ui.menu_item_enabled_selected_no_shortcut(e.label, false, e.why_not.is_none() && available) { actions.extend(action); }
                if ui.is_item_hovered_with_flags(ItemHoveredFlags::ALLOW_WHEN_DISABLED) { if let Some(why) = e.why_not { ui.tooltip_text(why); } else if !available { ui.tooltip_text("This editor cannot be opened here yet"); } }
            },
            Some(Err(e)) => ui.text_disabled(e), None => ui.text_disabled("No asset for this document"),
        }
    }
    ui.separator();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invented_weapon_header_names_its_target_and_link_roles() {
        let weapon_links = [
            links::Link { asset: "tag:hlmt:invented/model".into(), role: "Model", via: "model".into() },
            links::Link { asset: "tag:snd!:invented/fire".into(), role: "Sounds", via: "sound".into() },
        ];
        assert_eq!(target_text(Some("pogo".into()), true), "into: pogo *");
        assert_eq!(weapon_links.iter().map(link_role).collect::<Vec<_>>(), ["Model", "Sounds"]);
    }
}
