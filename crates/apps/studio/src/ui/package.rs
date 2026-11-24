//! Package document and item authoring controls (docs/editor/studio-flow.md).

use dear_imgui_rs::{Ui, WindowKey};
use serde_json::{Value, json};
use studio_proto::method as m;

use crate::app::{Action, App};
use crate::app::edit_target::slug_of;

#[derive(Default)]
pub(super) struct PackageUi {
    new_name: String,
    map: Option<String>,
    name: String,
}

fn command(actions: &mut Vec<Action>, method: &'static str, params: Value) {
    actions.push(Action::PackageCommand { method, params });
}

pub(super) fn new_package(ui: &Ui, name: &mut String, actions: &mut Vec<Action>) {
    if ui.button("New package...") {
        name.clear();
        ui.open_popup("##new-package");
    }
    if let Some(_popup) = ui.begin_popup("##new-package") {
        ui.input_text("Name", name).build();
        let trimmed = name.trim();
        let _disabled = ui.begin_disabled_with_cond(trimmed.is_empty());
        if ui.button("Create") {
            command(actions, m::PACKAGE_NEW, json!({"id": slug_of(trimmed), "name": trimmed}));
            ui.close_current_popup();
        }
    }
}

pub(super) fn check(ui: &Ui, label: &str, id: &str, actions: &mut Vec<Action>) {
    if ui.button(label) {
        command(actions, m::PACKAGE_CHECK, json!({"id": id}));
    }
}

/// Entry from every header. The item names a source already in the selected
/// package, just as package.item.add does.
#[derive(Default)]
pub(super) struct AddItem {
    package: Option<String>,
    kind: String,
    source: String,
    tag: String,
}

pub(super) fn add_item(
    ui: &Ui,
    app: &App,
    st: &mut AddItem,
    asset: Option<&str>,
    actions: &mut Vec<Action>,
) {
    if ui.button("Add to package") {
        *st = AddItem {
            package: asset.and_then(|id| id.strip_prefix("package:")).map(str::to_owned)
                .or_else(|| app.edit_target()),
            kind: "tag".into(),
            tag: asset.and_then(|id| id.strip_prefix("tag:"))
                .and_then(|id| id.split_once(':')).map_or(String::new(), |(_, path)| path.to_owned()),
            ..Default::default()
        };
        ui.open_popup("##add-package-item");
    }
    if let Some(_popup) = ui.begin_popup("##add-package-item") {
        let locals = app.local_packages();
        if locals.is_empty() {
            ui.text_disabled("Create a local package in the Browser first.");
            return;
        }
        let selected = st.package.as_deref().unwrap_or("Choose a local package");
        if let Some(_list) = ui.begin_combo("Package", selected) {
            for (id, name) in &locals {
                if ui.menu_item(format!("{name}##{id}")) {
                    st.package = Some(id.clone());
                }
            }
        }
        ui.input_text("Item kind", &mut st.kind).build();
        ui.input_text("Source in package", &mut st.source).build();
        ui.input_text("Tag", &mut st.tag).hint("Optional").build();
        let id = st.package.as_deref().filter(|id| locals.iter().any(|(local, _)| local.as_str() == *id));
        let _disabled = ui.begin_disabled_with_cond(
            id.is_none() || st.kind.trim().is_empty() || st.source.trim().is_empty(),
        );
        if ui.button("Add item") && let Some(id) = id {
            let mut item = json!({"kind": st.kind.trim(), "source": st.source.trim()});
            if !st.tag.trim().is_empty() {
                item["tag"] = json!(st.tag.trim());
            }
            command(actions, m::PACKAGE_ITEM_ADD, json!({"id": id, "item": item}));
            ui.close_current_popup();
        }
    }
}

pub(super) fn window(
    ui: &Ui,
    app: &App,
    key: &WindowKey,
    st: &mut PackageUi,
    browser: &mut super::browser::BrowserUi,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    ui.window(key).build(|| {
        let Some(read) = &app.package_document.read else {
            return;
        };
        let id = read["id"].as_str().unwrap_or_default();
        super::doc_header::draw(ui, app, actions, browser);
        ui.text(app.doc_title(crate::app::Doc::Package));
        ui.text_disabled(format!("{id}  ·  {}", read["origin"].as_str().unwrap_or_default()));
        if let Some(why) = read["why"].as_str() {
            ui.text_wrapped(why);
        }
        let map = st.map.as_deref().filter(|map| app.maps().iter().any(|m| m.as_str() == *map));
        if let Some(_combo) = ui.begin_combo("Map", map.unwrap_or("All applicable maps")) {
            if ui.menu_item("All applicable maps") {
                st.map = None;
            }
            for name in app.maps() {
                if ui.menu_item(name) {
                    st.map = Some(name.clone());
                }
            }
        }
        let map = st.map.as_deref().filter(|map| app.maps().iter().any(|m| m.as_str() == *map));
        if super::doc_header::button(ui, "Build", app.source().is_some(), "Open a map source first") {
            let mut params = json!({"id": id});
            if let Some(map) = map {
                params["map"] = json!(map);
            }
            command(actions, m::PACKAGE_BUILD, params);
        }
        ui.same_line();
        let paired = app.pairing.published.as_ref().is_some_and(|pair| pair["state"] == "connected");
        if super::doc_header::button(ui, "Play", map.is_some() && paired, "Choose a map and pair with a game") {
            command(actions, m::PACKAGE_PLAY, json!({"id": id, "map": map}));
        }
        ui.same_line();
        super::doc_header::button(ui, "Publish...", false, "Publishing needs Live package support (PK14)");
        ui.same_line();
        let local = read["editable"] == true;
        if super::doc_header::button(ui, "Export...", local && dialogs, "Export is available for local packages in the window")
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            command(actions, m::PACKAGE_EXPORT, json!({"id": id, "out": path.join(id)}));
        }
        if super::doc_header::button(
            ui,
            "Set as edit target",
            local && read["target"] != true,
            if local { "Already the edit target" } else { "Built-in packages are read-only" },
        ) {
            command(actions, m::PACKAGE_TARGET, json!({"id": id}));
        }
        ui.same_line();
        new_package(ui, &mut st.new_name, actions);
        ui.separator();

        let items = read["items"].as_array().map_or(&[][..], Vec::as_slice);
        ui.text(format!("Items ({})", items.len()));
        for item in items {
            let Some(asset) = item["asset"].as_str() else { continue };
            let source = item["source"].as_str().unwrap_or(asset);
            let kind = item["kind"].as_str().unwrap_or_default();
            if ui.selectable_config(format!("{source}  ({kind})##{asset}")).build() {
                let maps = item["maps"].as_array();
                let map = maps
                    .and_then(|names| {
                        let mut names = names.iter().filter_map(Value::as_str);
                        if let Some(open) = app.loaded_map()
                            && names.clone().any(|name| name.eq_ignore_ascii_case(open))
                        {
                            return Some(open);
                        }
                        names.find(|name| app.maps().iter().any(|map| map.eq_ignore_ascii_case(name)))
                    })
                    .map(str::to_owned);
                actions.push(Action::OpenAsset { asset: asset.to_owned(), map });
            }
            if ui.is_item_hovered() {
                ui.tooltip(|| {
                    ui.text(format!("Kind: {kind}"));
                    ui.text(format!("Maps: {}", item["maps"]));
                    ui.text(format!("Scope: {}", item["scope"]));
                    ui.text(format!("Builds: {}", item["builds"]));
                });
            }
        }
        if let Some(_manifest) = ui.tree_node("Manifest") {
            if local && ui.small_button("Rename...") {
                st.name = app.doc_title(crate::app::Doc::Package);
                ui.open_popup("##rename-package");
            }
            if let Some(_popup) = ui.begin_popup("##rename-package") {
                ui.input_text("Name", &mut st.name).build();
                let name = st.name.trim();
                let _disabled = ui.begin_disabled_with_cond(name.is_empty());
                if ui.button("Rename") {
                    command(actions, m::PACKAGE_SET, json!({"id": id, "path": "name", "value": name}));
                    ui.close_current_popup();
                }
            }
            for (label, value) in [("Package", &read["package"]), ("Credit", &read["credit"]), ("Lock", &read["lock"])] {
                ui.text_disabled(label);
                ui.text_wrapped(serde_json::to_string_pretty(value).unwrap_or_default());
            }
        }
        if let Some(findings) = read["problems"].as_array().filter(|rows| !rows.is_empty()) {
            ui.separator();
            ui.text("Problems");
            for finding in findings {
                ui.text_wrapped(finding["message"].as_str().unwrap_or_default());
            }
        }
        if let Some((method, result)) = &app.package_document.result {
            ui.separator();
            ui.text_disabled(method);
            match result {
                Ok(value) => ui.text_wrapped(serde_json::to_string_pretty(value).unwrap_or_default()),
                Err(message) => ui.text_wrapped(message),
            }
        }
    });
}
