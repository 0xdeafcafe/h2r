//! The browser panel (docs/editor/studio-redesign.md, "The browser"):
//! one list of everything Studio can open, the rows `asset.list` answers
//! (`app::assets`). Three roots: **Tags** (every tag of the tags tree by
//! path, with the variants of a tag that maps hold differently), **Packages**
//! (the set, and a package's items) and **Live**. **Kind** and **Map**
//! narrow the Tags root, and every word of the search must match.
//!
//! The **Voices** kind lists each character and the announcer with its
//! dataset state; a double-click opens the voice document
//! ([`Action::OpenAsset`] on `voice:<id>`).
//!
//! A double-click on a tag opens it in the editor its group gets, on the
//! map that holds it ([`Action::OpenAsset`]); right-click offers **Open**
//! and **Copy into package**, the call `asset.copy` makes
//! ([`Action::CopyAsset`]).
use dear_imgui_rs::{ListClipper, MouseButton, Ui};
use serde_json::Value;

use crate::app::assets::{self, Query, Root};
use crate::app::{Action, App};
use crate::theme::{self, icon};

/// The most rows one list holds; a longer answer says it was cut.
const LIMIT: usize = 2000;

/// What a list was read for: a new key reads it again.
#[derive(Clone, PartialEq)]
struct Key {
    root: u8,
    package: Option<String>,
    kind: Option<String>,
    map: Option<String>,
    query: String,
    /// The tree may have appeared or grown with the first start page.
    start_page: bool,
    refresh: u32,
}

/// One answer of `asset.list`, kept until its key changes.
#[derive(Default)]
struct Listing {
    rows: Vec<Value>,
    total: usize,
    truncated: bool,
    maps: Vec<String>,
    kinds: Vec<String>,
    offline: bool,
    error: Option<String>,
    /// The local packages a tag can be copied into: id and name.
    locals: Vec<(String, String)>,
}

/// The panel's UI-only state.
pub struct BrowserUi {
    root: Root,
    /// The search box.
    pub query: String,
    /// The Kind picker: a kind or a tag group.
    pub kind: Option<String>,
    /// The Map picker.
    pub map: Option<String>,
    /// The package whose items the Packages root lists.
    package: Option<String>,
    /// The row last clicked.
    selected: Option<String>,
    refresh: u32,
    cache: Option<(Key, Listing)>,
    /// The asset ids the last frame drew a row for, for scripts and tests.
    pub shown: Vec<String>,
}

impl Default for BrowserUi {
    fn default() -> Self {
        Self {
            root: Root::Tags,
            query: String::new(),
            kind: None,
            map: None,
            package: None,
            selected: None,
            refresh: 0,
            cache: None,
            shown: Vec::new(),
        }
    }
}

fn root_id(root: Root) -> u8 {
    match root {
        Root::Tags => 0,
        Root::Packages => 1,
        Root::Live => 2,
    }
}

/// The list for `st`'s filters, and the local packages beside it.
fn load(app: &App, st: &BrowserUi) -> Listing {
    let set = assets::read_set(&app.data_root());
    let tags = st.root == Root::Tags;
    let q = Query {
        root: st.root,
        package: (st.root == Root::Packages).then(|| st.package.clone()).flatten(),
        kind: st.kind.clone().filter(|_| tags),
        map: st.map.clone().filter(|_| tags),
        text: st.query.clone(),
        limit: LIMIT,
    };
    let mut out = Listing::default();
    // The Voices kind lists voices, read from the data root.
    let listed = if tags && assets::is_voices_kind(q.kind.as_deref()) {
        Ok(app.voice_asset_list(&q.text, q.limit))
    } else {
        assets::list(&app.tags_root(), &set, &q)
    };
    match listed {
        Ok(v) => {
            out.rows = v["rows"].as_array().cloned().unwrap_or_default();
            out.total = v["total"].as_u64().unwrap_or(0) as usize;
            out.truncated = v["truncated"].as_bool().unwrap_or(false);
            out.offline = v["offline"].as_bool().unwrap_or(false);
            let names = |k: &str| -> Vec<String> {
                v[k].as_array()
                    .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default()
            };
            out.maps = names("maps");
            out.kinds = names("kinds");
        }
        Err(e) => out.error = Some(e),
    }
    out.locals = set
        .listed
        .iter()
        .filter(|l| l.dir.is_some())
        .map(|l| (l.id.clone(), l.name.clone()))
        .collect();
    out
}

/// The map a tag opens on: the open map when it holds the tag, else the
/// first that does.
fn open_map(app: &App, row: &Value) -> Option<String> {
    let maps: Vec<&str> = row["maps"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    open_map_of(app, &maps)
}

fn open_map_of(app: &App, maps: &[&str]) -> Option<String> {
    app.loaded_map()
        .filter(|open| maps.iter().any(|m| m.eq_ignore_ascii_case(open)))
        .or_else(|| maps.first().copied())
        .map(str::to_owned)
}

/// One line of the list: a row, or one of its variants.
enum Line {
    Row(usize),
    Variant(usize, usize),
}

/// The tag a double-click or **Open** opens: `asset`, on `map`.
fn open(actions: &mut Vec<Action>, asset: &str, map: Option<String>) {
    actions.push(Action::OpenAsset {
        asset: asset.to_owned(),
        map,
    });
}

impl BrowserUi {
    /// Show a package or one of its items, as a Problems row opens it.
    pub fn reveal(&mut self, asset: &str) {
        let package = asset
            .strip_prefix("package:")
            .or_else(|| asset.strip_prefix("item:").and_then(|r| r.rsplit_once(':').map(|(p, _)| p)));
        if let Some(p) = package {
            self.root = Root::Packages;
            self.package = Some(p.to_owned());
            self.selected = Some(asset.to_owned());
        }
    }
}

/// Draw the panel.
pub fn draw(ui: &Ui, app: &App, st: &mut BrowserUi, actions: &mut Vec<Action>) {
    st.shown.clear();
    for (root, label) in [(Root::Tags, "Tags"), (Root::Packages, "Packages"), (Root::Live, "Live")] {
        if root != Root::Tags {
            ui.same_line();
        }
        if ui
            .selectable_config(format!("{label}##browser-root-{}", root_id(root)))
            .selected(st.root == root)
            .size([64.0, 0.0])
            .build()
        {
            st.root = root;
        }
    }
    ui.same_line();
    if ui.small_button(format!("{}##browser-refresh", icon::ROTATE)) {
        st.refresh += 1;
    }
    if ui.is_item_hovered() {
        ui.tooltip_text("Read the tree and the packages again");
    }

    let key = Key {
        root: root_id(st.root),
        package: st.package.clone(),
        kind: st.kind.clone(),
        map: st.map.clone(),
        query: st.query.clone(),
        start_page: app.workspace_start_page(),
        refresh: st.refresh,
    };
    let (key, list) = match st.cache.take() {
        Some((k, l)) if k == key => (k, l),
        _ => (key, load(app, st)),
    };
    pickers(ui, st, &list);
    ui.set_next_item_width(-1.0);
    ui.input_text("##browser-search", &mut st.query)
        .hint(match st.root {
            Root::Tags => "Search tags: every word must match",
            Root::Packages => "Search packages",
            Root::Live => "Search the server's packages",
        })
        .build();
    ui.separator();
    body(ui, app, st, &list, actions);
    st.cache = Some((key, list));
}

/// A picker for the Kind and the Map: `None` is every one.
fn combo(ui: &Ui, id: &str, all: &str, value: &mut Option<String>, choices: &[String]) {
    let shown = value.clone().unwrap_or_else(|| all.to_owned());
    ui.set_next_item_width(ui.content_region_avail()[0] / 2.0 - 4.0);
    if let Some(_c) = ui.begin_combo(format!("##browser-{id}"), &shown) {
        if ui
            .selectable_config(format!("{all}##browser-{id}-all"))
            .selected(value.is_none())
            .build()
        {
            *value = None;
        }
        for c in choices {
            if ui
                .selectable_config(format!("{c}##browser-{id}-{c}"))
                .selected(value.as_deref() == Some(c))
                .build()
            {
                *value = Some(c.clone());
            }
        }
    }
}

fn pickers(ui: &Ui, st: &mut BrowserUi, list: &Listing) {
    match st.root {
        Root::Tags => {
            combo(ui, "kind", "Any kind", &mut st.kind, &list.kinds);
            ui.same_line();
            combo(ui, "map", "Any map", &mut st.map, &list.maps);
        }
        Root::Packages => {
            if let Some(p) = st.package.clone() {
                ui.align_text_to_frame_padding();
                ui.text(format!("{} {p}", icon::PACKAGE_OPEN));
                ui.same_line();
                if ui.small_button("All packages##browser-package-back") {
                    st.package = None;
                }
            }
        }
        Root::Live => {}
    }
}

fn body(ui: &Ui, app: &App, st: &mut BrowserUi, list: &Listing, actions: &mut Vec<Action>) {
    if let Some(e) = &list.error {
        ui.text_wrapped(e);
        return;
    }
    if list.offline {
        ui.text_disabled("Live lists the packages the server offers; it is read over the network.");
        return;
    }
    if list.rows.is_empty() {
        ui.text_disabled(if st.root == Root::Tags {
            "No tag matches."
        } else {
            "Nothing matches."
        });
        return;
    }
    let mut lines = Vec::new();
    for (i, row) in list.rows.iter().enumerate() {
        lines.push(Line::Row(i));
        let n = row["variants"].as_array().map_or(0, Vec::len);
        lines.extend((0..n).map(|v| Line::Variant(i, v)));
    }
    if list.truncated {
        ui.text_disabled(format!(
            "{} of {} shown: narrow the search",
            list.rows.len(),
            list.total
        ));
    }
    theme::panel_body(ui, "##browser-rows", || {
        for l in ListClipper::new(lines.len()).begin(ui).iter() {
            match lines[l] {
                Line::Row(i) => row(ui, app, st, list, &list.rows[i], actions),
                Line::Variant(i, v) => variant(ui, app, st, &list.rows[i], v, actions),
            }
        }
    });
}

fn glyph(row: &Value) -> &'static str {
    match row["kind"].as_str().unwrap_or("") {
        "Maps" => icon::MAP,
        "Package" => icon::PACKAGE,
        "Models" | "Objects" => icon::BOX,
        "Screens" | "HUDs" => icon::LAYERS,
        "Sounds" | "Voices" => icon::SOUND,
        _ => icon::FILE,
    }
}

/// A row: the tag's path, the number of its variants and its badge.
fn label(row: &Value) -> String {
    let mut text = match row["root"].as_str() {
        Some("tags") => format!(
            "{}  {}  {}",
            glyph(row),
            row["path"].as_str().unwrap_or(""),
            row["group"].as_str().unwrap_or("").trim()
        ),
        _ => format!(
            "{}  {}",
            glyph(row),
            row["name"].as_str().unwrap_or(row["id"].as_str().unwrap_or(""))
        ),
    };
    let variants = row["variants"].as_array().map_or(0, Vec::len);
    if variants > 0 {
        text.push_str(&format!("  [{variants} variants]"));
    }
    if let Some(b) = row["badge"].as_str() {
        text.push_str(&format!("  [{b}]"));
    }
    text
}

fn row(
    ui: &Ui,
    app: &App,
    st: &mut BrowserUi,
    list: &Listing,
    row: &Value,
    actions: &mut Vec<Action>,
) {
    let Some(asset) = row["asset"].as_str() else {
        return;
    };
    st.shown.push(asset.to_owned());
    let tag = row["root"] == "tags";
    let clicked = ui
        .selectable_config(format!("{}##browser-{asset}", label(row)))
        .selected(st.selected.as_deref() == Some(asset))
        .build();
    if clicked {
        st.selected = Some(asset.to_owned());
    }
    if ui.is_item_hovered() {
        tooltip(ui, row);
        if ui.is_mouse_double_clicked(MouseButton::Left) {
            if tag {
                open(actions, asset, open_map(app, row));
            } else if row["root"] == "voices" {
                open(actions, asset, None);
            } else if row["root"] == "packages" && asset.starts_with("package:") {
                st.package = row["id"].as_str().map(str::to_owned);
                st.selected = None;
            }
        }
    }
    if let Some(_menu) = ui.begin_popup_context_item() {
        if tag {
            if ui.menu_item(format!("{} Open", icon::FILE)) {
                open(actions, asset, open_map(app, row));
            }
            copy_menu(ui, list, asset, actions);
        } else if row["root"] == "voices" {
            if ui.menu_item(format!("{} Open", icon::SOUND)) {
                open(actions, asset, None);
            }
        } else if asset.starts_with("package:") && ui.menu_item("Open items") {
            st.package = row["id"].as_str().map(str::to_owned);
        }
    }
}

fn tooltip(ui: &Ui, row: &Value) {
    ui.tooltip(|| {
        ui.text(row["asset"].as_str().unwrap_or(""));
        if let Some(maps) = row["maps"].as_array().filter(|m| !m.is_empty()) {
            let names: Vec<&str> = maps.iter().filter_map(Value::as_str).collect();
            ui.text_disabled(format!("in {}", names.join(", ")));
        }
        if let Some(why) = row["why"].as_str().filter(|w| !w.is_empty()) {
            ui.text_disabled(why);
        }
    });
}

/// **Copy into package**: one entry per local package.
fn copy_menu(ui: &Ui, list: &Listing, asset: &str, actions: &mut Vec<Action>) {
    let any = !list.locals.is_empty();
    if let Some(_m) = ui.begin_menu_with_enabled(format!("{} Copy into package...", icon::COPY), any) {
        for (id, name) in &list.locals {
            if ui.menu_item(format!("{name}##browser-copy-{id}")) {
                actions.push(Action::CopyAsset {
                    asset: asset.to_owned(),
                    package: id.clone(),
                });
            }
        }
    } else if !any && ui.is_item_hovered() {
        ui.tooltip_text("No package of yours is on: make one under <data>/packages/");
    }
}

/// One variant of a tag: the maps that hold it. A double-click opens the
/// tag on the first of them.
fn variant(ui: &Ui, app: &App, st: &mut BrowserUi, row: &Value, n: usize, actions: &mut Vec<Action>) {
    let Some(asset) = row["asset"].as_str() else {
        return;
    };
    let v = &row["variants"][n];
    let maps: Vec<&str> = v["maps"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let name = v["variant"].as_str().unwrap_or("base");
    ui.indent();
    let id = format!("{asset}-v-{n}");
    let clicked = ui
        .selectable_config(format!("{name}: {}##browser-{id}", maps.join(", ")))
        .selected(st.selected.as_deref() == Some(id.as_str()))
        .build();
    if clicked {
        st.selected = Some(id);
    }
    if ui.is_item_hovered() && ui.is_mouse_double_clicked(MouseButton::Left) {
        open(actions, asset, open_map_of(app, &maps));
    }
    ui.unindent();
}
