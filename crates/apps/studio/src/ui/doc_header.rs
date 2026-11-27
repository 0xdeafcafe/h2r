//! Shared document controls. See docs/editor/studio-flow.md, "The document header".
use dear_imgui_rs::{ItemHoveredFlags, Ui};
use crate::app::{Action, App, Doc, NavAction, Surface};
use crate::app::assets::{self, AssetId};
use crate::app::links::{self, Links};

/// The header's path for an asset: where it comes from, its kind and its name.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Crumbs {
    pub origin: String,
    pub kind: String,
    pub leaf: String,
    pub badge: String,
}

pub(crate) fn crumbs(app: &App, id: &str) -> Option<Crumbs> {
    let set = assets::read_set(&app.data_root());
    match AssetId::parse(id).ok()? {
        AssetId::Tag(key) => {
            let group = tags::defs::fourcc_text(key.group);
            Some(Crumbs {
                origin: app.edit_target().unwrap_or_else(|| "Disc".into()),
                kind: assets::kind_of(&group).to_owned(),
                badge: format!("{group}  {}", assets::badge_for(&set, &key)),
                leaf: key.path,
            })
        }
        AssetId::Item(package, n) => {
            let (kind, source) = links::item_label(&set, &package, n)?;
            Some(Crumbs { origin: package.clone(), kind: kind.clone(), leaf: source, badge: format!("{kind}  in {package}") })
        }
        AssetId::Voice(voice) => Some(Crumbs { origin: "Voices".into(), kind: "Voice".into(), leaf: voice.clone(), badge: "voice".into() }),
        AssetId::Package(package) => Some(Crumbs { origin: package.clone(), kind: "Package".into(), leaf: package.clone(), badge: "package".into() }),
    }
}

pub(super) fn button(ui: &Ui, label: &str, enabled: bool, reason: &str) -> bool {
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

/// The header's **Check**: the count of this document's findings in Problems.
pub(crate) fn check_label(count: usize) -> String {
    if count == 0 { "Check##doc-check".to_owned() } else { format!("Check {count}##doc-check") }
}

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
            if row.asset.starts_with("tag:") || row.asset.starts_with("package:") {
                actions.push(Action::OpenAsset { asset: row.asset.clone(), map: None });
            } else {
                browser.locate(&row.asset);
            }
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
    let id = app.doc_asset(doc);
    if button(ui, "<", app.can_navigate(false), "No earlier location") { actions.push(Action::Navigate(NavAction::Back)); }
    ui.same_line();
    if button(ui, ">", app.can_navigate(true), "No later location") { actions.push(Action::Navigate(NavAction::Forward)); }
    ui.same_line();
    if let Some(id) = &id {
        let source = app.source_asset(doc);
        if let Some(mut c) = crumbs(app, id) {
            if let Some(source) = source {
                c.origin = source.package.clone();
                c.badge = format!("{}  new in {}", tags::defs::fourcc_text(source.key.group), source.package);
            }
            if ui.small_button(format!("{}##crumb-origin", c.origin)) { browser.locate(id); }
            ui.same_line(); ui.text(">"); ui.same_line();
            if ui.small_button(format!("{}##crumb-kind", c.kind)) { browser.locate(id); browser.query.clear(); browser.kind = Some(c.kind.clone()); }
            ui.same_line(); ui.text(">"); ui.same_line();
            if ui.small_button(format!("{}##crumb-leaf", c.leaf)) { browser.locate(id); }
            ui.same_line(); if ui.small_button("In Browser") { browser.locate(id); }
            ui.text(&c.badge);
            if id.starts_with("tag:") && source.is_none() && !c.badge.contains("  new in ") {
                ui.same_line();
                if ui.small_button("New from this...") {
                    browser.new_from(id);
                }
            }
        }
    } else { ui.text(app.doc_title(doc)); }
    ui.text(target_text(app.edit_target(), app.doc_dirty(doc)));
    ui.same_line();
    let target = app.preview_target(doc).label();
    if ui.button(format!("Preview: {target}")) { ui.open_popup("##doc-preview"); }
    if let Some(_popup) = ui.begin_popup("##doc-preview") {
        super::preview_switch::targets(ui, app, Some(doc), actions);
    }
    ui.same_line();
    let ready = super::preview_switch::in_game(app, Some(doc));
    let disabled = ui.begin_disabled_with_cond(ready.is_err());
    let clicked = ui.button("In game");
    drop(disabled);
    super::preview_switch::explain(ui, &ready);
    if clicked && let Ok(action) = ready { actions.push(action); }
    let check = check_label(app.doc_problem_count(doc));
    if doc == Doc::Package && let Some(package) = app.package_document.id() {
        super::package::check(ui, &check, package, actions);
    } else if ui.button(check) {
        actions.push(Action::RefreshProblems(None));
    }
    ui.same_line();
    let (undo, redo) = app.doc_can(doc);
    if button(ui, "Undo", undo, "Nothing to undo") { actions.push(Action::UndoDoc(doc)); }
    ui.same_line();
    if button(ui, "Redo", redo, "Nothing to redo") { actions.push(Action::RedoDoc(doc)); }
    ui.same_line();
    let _ = button(ui, "Revert", false, "Revert is not available for this document yet");
    ui.same_line();
    let save = app.doc_save_action(doc);
    if button(ui, "Save", save.is_some(), "Save is not available for this document") { actions.extend(save); }
    ui.same_line();
    if ui.button("Links") { ui.open_popup("##doc-links"); }
    if let Some(_popup) = ui.begin_popup("##doc-links") {
        match id.as_deref().map(|id| load(app, id)) {
Some(Ok(l)) => {
                ui.text_disabled("Uses");
                link_rows(ui, &l.uses, actions, browser, "uses");
                ui.separator();
                ui.text_disabled("Used by");
                link_rows(ui, &l.used_by, actions, browser, "used-by");
                for p in l.problems { ui.text_disabled(p); }
                ui.separator();
                // The same rows as a Browser view, and the items that
                // change the asset (`app::assets::browse`).
                if let Some(asset) = &id {
                    if ui.menu_item("Where used##doc-where-used") {
                        actions.push(Action::WhereUsed { asset: asset.clone() });
                    }
                    if (asset.starts_with("tag:") || asset.starts_with("package:"))
                        && ui.menu_item("Show edits##doc-show-edits")
                    {
                        actions.push(Action::ShowEdits { asset: asset.clone() });
                    }
                }
            }
            Some(Err(e)) => ui.text_disabled(e), None => ui.text_disabled("No asset for this document"),
        }
    }
    ui.same_line();
    if ui.button("Open with") { ui.open_popup("##doc-open-with"); }
    if let Some(_popup) = ui.begin_popup("##doc-open-with") {
        match id.as_deref().map(|id| load(app, id)) {
            Some(Ok(l)) => for e in l.editors {
                let action = match e.id { "voices" => id.clone().map(|asset| Action::OpenAsset { asset, map: None }), "field_panel" | "modeling" | "audition" | "animation" => Some(Action::Show(Surface::Tags)), "scenario" => Some(Action::Show(Surface::Scene)), "textures" => Some(Action::Show(Surface::Textures)), "hud" => Some(Action::Show(Surface::Hud)), "ui_screens" => Some(Action::Show(Surface::UiScreens)), "scripts" => Some(Action::Show(Surface::Scripts)), "variants" => Some(Action::Show(Surface::Variants)), "playlists" => Some(Action::Show(Surface::Playlists)), _ => None };
                let available = action.is_some();
                if ui.menu_item_enabled_selected_no_shortcut(e.label, false, e.why_not.is_none() && available) { actions.extend(action); }
                if ui.is_item_hovered_with_flags(ItemHoveredFlags::ALLOW_WHEN_DISABLED) { if let Some(why) = e.why_not { ui.tooltip_text(why); } else if !available { ui.tooltip_text("This editor cannot be opened here yet"); } }
            },
            Some(Err(e)) => ui.text_disabled(e), None => ui.text_disabled("No asset for this document"),
        }
    }
    super::package::add_item(ui, app, &mut browser.add_item, id.as_deref(), actions);
    ui.separator();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_shows_the_count_of_findings_only_when_there_are_some() {
        assert_eq!(check_label(0), "Check##doc-check");
        assert_eq!(check_label(3), "Check 3##doc-check");
    }

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
