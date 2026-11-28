//! The browser panel (docs/editor/studio-redesign.md, "The browser"):
//! one list of everything Studio can open, the rows `asset.list` answers
//! (`app::assets`). Three roots: **Tags** (every tag of the tags tree by
//! path, with the variants of a tag that maps hold differently), **Packages**
//! (the set, and a package's items) and **Live**. **Kind** and **Map**
//! narrow the Tags root, and **Kind** narrows a package's items (**Game
//! types**, **Playlists** beside the tag kinds); every word of the
//! search must match.
//!
//! The **Voices** kind lists each character and the announcer with its
//! dataset state; a double-click opens the voice document
//! ([`Action::OpenAsset`] on `voice:<id>`). The **Forge variants** kind
//! lists the saved map variants, which open as documents
//! (`forge:<map>:<file>`), and **Forge items** the Forge palette, the
//! custom items included (`app::assets::forge_items`).
//!
//! A double-click on a tag opens it in the editor its group gets, on the
//! map that holds it ([`Action::OpenAsset`]). A double-click on a package
//! item opens the editor of what it builds, using the item's map when
//! specified. Right-click offers **Open** and, for an existing tag,
//! **New from this...**, the window form of `asset.copy`.
//!
//! **Where used** and **Show edits** in a row's menu
//! ([`Action::WhereUsed`], [`Action::ShowEdits`]) put the app's link view
//! (`app::assets::Browse`) in place of the list: one line per asset with
//! its role and where the link was read, opened by a double-click, until
//! **Clear** or a root button.
use std::collections::HashMap;
use std::sync::Arc;

use dear_imgui_rs::{ListClipper, MouseButton, TextureId, Ui};
use serde_json::Value;

use super::map_list::MapListUi;
use crate::app::map_pack::Picture;
use crate::viewport::Gpu;

use crate::app::assets::{self, CreateAsset, Query, Root};
use crate::app::{Action, App};
use crate::theme::{self, icon};

/// The most rows one list holds; a longer answer says it was cut.
const LIMIT: usize = 2000;
/// The widest a row's thumbnail draws, in thumbnail heights.
const THUMB_ASPECT: f32 = 2.0;
/// How many thumbnails are read in one frame; the rest wait for the next.
const DECODES: u32 = 2;
/// The key prefix of the thumbnails' pictures on the GPU.
const THUMB_KEY: &str = "browser-thumb-";

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
    package_revision: u64,
    refresh: u32,
    source_revision: usize,
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
}

/// The panel's UI-only state.
pub struct BrowserUi {
    create: Option<CreateAsset>,
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
    /// An explicit set from the document link menu.
    linked: Option<Vec<String>>,
    /// The app's link view last brought forward (`Browse::serial`).
    browse_seen: u32,
    /// The rows' thumbnails by asset id: `None` for an asset with no
    /// picture, so it is asked once.
    thumbs: HashMap<String, Option<Arc<Picture>>>,
    /// What the thumbnails were read for: the shown map, the packages'
    /// revision and the refresh count. Another reads them again.
    thumb_key: (Option<String>, u64, u32),
    /// The thumbnails this frame may still read.
    decodes: u32,
    /// The asset ids the last frame drew a row for, for scripts and tests.
    pub shown: Vec<String>,
    new_package_name: String,
    pub(super) add_item: super::package::AddItem,
}

impl Default for BrowserUi {
    fn default() -> Self {
        Self {
            create: None,
            root: Root::Tags,
            query: String::new(),
            kind: None,
            map: None,
            package: None,
            selected: None,
            refresh: 0,
            cache: None,
            linked: None,
            browse_seen: 0,
            thumbs: HashMap::new(),
            thumb_key: (None, 0, 0),
            decodes: 0,
            shown: Vec::new(),
            new_package_name: String::new(),
            add_item: Default::default(),
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
        kind: st.kind.clone(),
        map: st.map.clone().filter(|_| tags),
        text: st.query.clone(),
        limit: LIMIT,
    };
    let mut out = Listing::default();
    // The Voices, Forge variants and Forge items kinds are read from the
    // data root (`App::kind_list`).
    let listed = match app.kind_list(&q) {
        Some(listed) => listed,
        None => assets::list(&app.tags_root(), &set, &q),
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
    /// Show an asset's row in the Browser.
    pub fn locate(&mut self, asset: &str) {
        self.linked = None;
        self.query.clear();
        self.kind = None;
        self.map = None;
        self.package = None;
        self.selected = Some(asset.to_owned());
        match assets::AssetId::parse(asset) {
            Ok(assets::AssetId::Tag(key)) => {
                self.root = Root::Tags;
                self.query = key.path;
            }
            Ok(assets::AssetId::Package(_)) => self.root = Root::Packages,
            Ok(assets::AssetId::Item(package, _)) => {
                self.root = Root::Packages;
                self.package = Some(package);
            }
            Ok(assets::AssetId::Voice(_)) => self.root = Root::Tags,
            Ok(assets::AssetId::Forge(..)) => {
                self.root = Root::Tags;
                self.kind = Some(crate::app::forge_variants::VARIANTS_KIND.to_owned());
            }
            Err(_) => self.selected = None,
        }
    }

    /// Show exactly the targets of a link section, across Browser roots.
    pub fn show_links(&mut self, rows: &[crate::app::links::Link]) {
        let mut ids: Vec<String> = rows.iter().map(|row| row.asset.clone()).collect();
        ids.sort();
        ids.dedup();
        self.linked = Some(ids);
        self.query.clear();
        self.selected = None;
    }

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

/// The size a thumbnail of `w` by `h` pixels draws at in a row: a text
/// line high, so every row keeps one height for the list's clipper.
fn thumb_size(w: u32, h: u32, line: f32) -> [f32; 2] {
    let aspect = (w as f32 / h.max(1) as f32).min(THUMB_ASPECT);
    [line * aspect, line]
}

/// A row's thumbnail on the GPU, read the first time the row is drawn
/// (at most [`DECODES`] a frame): a bitmap tag of the shown map or a
/// package's PNG bitmap item (`App::thumb_picture`).
fn thumb_of(
    app: &App,
    st: &mut BrowserUi,
    gpu: &mut Gpu<'_>,
    pictures: &mut MapListUi,
    row: &Value,
    asset: &str,
) -> Option<(TextureId, u32, u32)> {
    if !assets::has_thumb(row) {
        return None;
    }
    if !st.thumbs.contains_key(asset) {
        if st.decodes == 0 {
            return None;
        }
        st.decodes -= 1;
        let pic = assets::AssetId::parse(asset)
            .ok()
            .and_then(|id| app.thumb_picture(&id).ok())
            .map(Arc::new);
        st.thumbs.insert(asset.to_owned(), pic);
    }
    let pic = st.thumbs.get(asset)?.as_ref()?;
    let tex = pictures.picture(gpu, &format!("{THUMB_KEY}{asset}"), pic)?;
    Some((tex, pic.width, pic.height))
}

/// Draw the panel.
pub fn draw(
    ui: &Ui,
    app: &App,
    st: &mut BrowserUi,
    gpu: &mut Gpu<'_>,
    pictures: &mut MapListUi,
    actions: &mut Vec<Action>,
) {
    st.shown.clear();
    st.decodes = DECODES;
    let thumb_key = (
        app.shown_map_name().map(str::to_owned),
        app.package_document.revision,
        st.refresh,
    );
    if st.thumb_key != thumb_key {
        st.thumbs.clear();
        pictures.forget(gpu, THUMB_KEY);
        st.thumb_key = thumb_key;
    }
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
            st.linked = None;
            if app.browse.view.is_some() {
                actions.push(Action::ClearBrowse);
            }
        }
    }
    ui.same_line();
    if ui.small_button(format!("{}##browser-refresh", icon::ROTATE)) {
        st.refresh += 1;
    }
    if ui.is_item_hovered() {
        ui.tooltip_text("Read the tree and the packages again");
    }

    if let Some(view) = &app.browse.view {
        browse_view(ui, st, view, actions);
        return;
    }
    if st.linked.is_some() {
        ui.text("Linked assets");
        ui.same_line();
        if ui.small_button("Clear##browser-linked") { st.linked = None; }
        if let Some(ids) = st.linked.clone() {
            ui.set_next_item_width(-1.0);
            ui.input_text("##browser-linked-search", &mut st.query).hint("Search linked assets").build();
            let words = st.query.to_lowercase();
            theme::panel_body(ui, "##browser-rows", || {
                for id in ids.iter().filter(|id| words.split_whitespace().all(|w| id.to_lowercase().contains(w))) {
                    st.shown.push(id.clone());
                    if ui.selectable_config(format!("{id}##browser-link-{id}"))
                        .selected(st.selected.as_deref() == Some(id.as_str())).build() {
                        st.selected = Some(id.clone());
                    }
                    if ui.is_item_hovered() && ui.is_mouse_double_clicked(MouseButton::Left) {
                        open(actions, id, None);
                    }
                }
            });
            return;
        }
    }
    if st.root == Root::Packages {
        super::package::new_package(ui, &mut st.new_package_name, actions);
        // Where a dropped source file goes.
        ui.text_disabled(match st.drop_package().or_else(|| app.edit_target()) {
            Some(package) => format!("Drop a WAV, a PNG or a model YAML here to add it to {package}"),
            None => "Select a package, then drop a WAV, a PNG or a model YAML to add it".to_owned(),
        });
    }
    let key = Key {
        root: root_id(st.root),
        package: st.package.clone(),
        kind: st.kind.clone(),
        map: st.map.clone(),
        query: st.query.clone(),
        start_page: app.workspace_start_page(),
        package_revision: app.package_document.revision,
        refresh: st.refresh,
        source_revision: app.source_asset_revision(),
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
    body(ui, app, st, &list, gpu, pictures, actions);
    st.cache = Some((key, list));
}

/// The row of a link view: the asset, the word it is grouped by and where
/// the link was read from.
fn browse_label(row: &assets::BrowseRow) -> String {
    let mut text = format!("{}  [{}]", row.asset, row.role);
    if !row.via.is_empty() {
        text.push_str(&format!("  {}", row.via));
    }
    text
}

/// The rows of a link view that every word of `query` matches.
fn browse_rows<'a>(view: &'a assets::BrowseView, query: &str) -> Vec<&'a assets::BrowseRow> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    view.rows
        .iter()
        .filter(|row| {
            let hay = format!("{} {} {}", row.asset, row.role, row.via).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

/// **Where used** and **Show edits**: the app's link view in place of the
/// list.
fn browse_view(ui: &Ui, st: &mut BrowserUi, view: &assets::BrowseView, actions: &mut Vec<Action>) {
    ui.text(view.heading());
    ui.same_line();
    if ui.small_button("Clear##browser-view-clear") {
        actions.push(Action::ClearBrowse);
    }
    ui.text_disabled(&view.of);
    ui.set_next_item_width(-1.0);
    ui.input_text("##browser-view-search", &mut st.query)
        .hint("Search these rows")
        .build();
    ui.separator();
    for p in &view.problems {
        ui.text_disabled(p);
    }
    let rows = browse_rows(view, &st.query);
    if rows.is_empty() {
        ui.text_disabled(match view.mode {
            assets::BrowseMode::WhereUsed => "Not used by anything.",
            assets::BrowseMode::Edits => "No package changes this.",
        });
        return;
    }
    theme::panel_body(ui, "##browser-rows", || {
        for (n, row) in rows.iter().enumerate() {
            st.shown.push(row.asset.clone());
            let key = format!("{}##browser-view-{n}", browse_label(row));
            if ui
                .selectable_config(key)
                .selected(st.selected.as_deref() == Some(row.asset.as_str()))
                .build()
            {
                st.selected = Some(row.asset.clone());
            }
            if ui.is_item_hovered() && ui.is_mouse_double_clicked(MouseButton::Left) {
                open(actions, &row.asset, None);
            }
            if let Some(_menu) = ui.begin_popup_context_item() {
                if ui.menu_item("Open") {
                    open(actions, &row.asset, None);
                }
                link_entries(ui, &row.asset, true, actions);
            }
        }
    });
}

/// The **Where used** and **Show edits** entries of a row's menu.
/// `edits` is whether the asset is one Show edits answers for (a tag or a
/// package).
fn link_entries(ui: &Ui, asset: &str, edits: bool, actions: &mut Vec<Action>) {
    if ui.menu_item("Where used") {
        actions.push(Action::WhereUsed { asset: asset.to_owned() });
    }
    let edits = edits && (asset.starts_with("tag:") || asset.starts_with("package:"));
    if edits && ui.menu_item("Show edits") {
        actions.push(Action::ShowEdits { asset: asset.to_owned() });
    }
}

impl BrowserUi {
    /// The package a file dropped on the window goes into: the one whose
    /// items the Packages root lists, else the package row selected
    /// there. `None` is the edit target ([`Action::DropFile`]).
    pub fn drop_package(&self) -> Option<String> {
        if self.root != Root::Packages {
            return None;
        }
        self.package.clone().or_else(|| {
            self.selected
                .as_deref()
                .and_then(|asset| asset.strip_prefix("package:"))
                .map(str::to_owned)
        })
    }

    /// Whether the app has a link view this panel has not brought forward
    /// yet; answers once per view.
    pub(super) fn take_browse(&mut self, serial: u32) -> bool {
        let new = serial != self.browse_seen;
        self.browse_seen = serial;
        new
    }
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
            // The Kind picker names the items too: **Game types**,
            // **Playlists** beside the tag kinds (`app::assets`).
            if st.package.is_none() {
                combo(ui, "kind", "Any kind", &mut st.kind, &list.kinds);
            }
            if let Some(p) = st.package.clone() {
                ui.align_text_to_frame_padding();
                ui.text(format!("{} {p}", icon::PACKAGE_OPEN));
                ui.same_line();
                if ui.small_button("All packages##browser-package-back") {
                    st.package = None;
                }
                ui.same_line();
                combo(ui, "kind", "Any kind", &mut st.kind, &list.kinds);
            }
        }
        Root::Live => {}
    }
}

fn body(
    ui: &Ui,
    app: &App,
    st: &mut BrowserUi,
    list: &Listing,
    gpu: &mut Gpu<'_>,
    pictures: &mut MapListUi,
    actions: &mut Vec<Action>,
) {
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
                Line::Row(i) => {
                    let entry = &list.rows[i];
                    if let Some(asset) = entry["asset"].as_str().filter(|asset| asset.starts_with("package:"))
                        && entry["root"] == "packages"
                    {
                        package_row(ui, st, entry, asset, actions);
                    } else {
                        row(ui, app, st, entry, gpu, pictures, actions);
                    }
                }
                Line::Variant(i, v) => variant(ui, app, st, &list.rows[i], v, actions),
            }
        }
    });
}

fn package_row(ui: &Ui, st: &mut BrowserUi, entry: &Value, asset: &str, actions: &mut Vec<Action>) {
    st.shown.push(asset.to_owned());
    if ui.selectable_config(format!("{}##browser-{asset}", label(entry)))
        .selected(st.selected.as_deref() == Some(asset)).build()
    {
        st.selected = Some(asset.to_owned());
    }
    if ui.is_item_hovered() {
        tooltip(ui, entry);
        if ui.is_mouse_double_clicked(MouseButton::Left) {
            open(actions, asset, None);
        }
    }
    if let Some(_menu) = ui.begin_popup_context_item() {
        if ui.menu_item("Open") {
            open(actions, asset, None);
        }
        if ui.menu_item("Show items in Browser") {
            st.package = entry["id"].as_str().map(str::to_owned);
        }
        link_entries(ui, asset, true, actions);
    }
}

fn glyph(row: &Value) -> &'static str {
    match row["kind"].as_str().unwrap_or("") {
        "Maps" | "Forge variants" => icon::MAP,
        "Package" => icon::PACKAGE,
        "Models" | "Objects" | "Forge items" => icon::BOX,
        "Screens" | "HUDs" => icon::LAYERS,
        "Sounds" | "Voices" => icon::SOUND,
        "Game types" => icon::SWORDS,
        "Playlists" => icon::LOG,
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
    row: &Value,
    gpu: &mut Gpu<'_>,
    pictures: &mut MapListUi,
    actions: &mut Vec<Action>,
) {
    let Some(asset) = row["asset"].as_str() else {
        return;
    };
    st.shown.push(asset.to_owned());
    let tag = row["root"] == "tags";
    // The thumbnail, a line high, before the row's text.
    let thumb = thumb_of(app, st, gpu, pictures, row, asset);
    if let Some((tex, w, h)) = thumb {
        ui.image(tex, thumb_size(w, h, ui.text_line_height()));
        ui.same_line();
    }
    let clicked = ui
        .selectable_config(format!("{}##browser-{asset}", label(row)))
        .selected(st.selected.as_deref() == Some(asset))
        .build();
    if clicked {
        st.selected = Some(asset.to_owned());
    }
    if ui.is_item_hovered() {
        tooltip(ui, row);
        if let Some((tex, w, h)) = thumb {
            // The picture at its own size, up to the thumbnail's edge.
            ui.tooltip(|| {
                ui.image(tex, [w as f32, h as f32]);
            });
        }
        if ui.is_mouse_double_clicked(MouseButton::Left) {
            if tag {
                open(actions, asset, if row["new"] == true { None } else { open_map(app, row) });
            } else if asset.starts_with("item:") {
                open(actions, asset, item_map(app, row));
            } else if row["root"] == "voices" || asset.starts_with("forge:") {
                open(actions, asset, None);
            }
        }
    }
    if let Some(_menu) = ui.begin_popup_context_item() {
        if tag {
            if ui.menu_item(format!("{} Open", icon::FILE)) {
                open(actions, asset, if row["new"] == true { None } else { open_map(app, row) });
            }
            if row["new"] != true && ui.menu_item("Open as fields") {
                actions.push(Action::OpenFields {
                    asset: asset.to_owned(),
                    map: open_map(app, row),
                });
            }
            if row["new"] != true && ui.menu_item("New from this...") {
                st.new_from(asset);
            }
        } else if asset.starts_with("item:") {
            if ui.menu_item(format!("{} Open", icon::FILE)) {
                open(actions, asset, item_map(app, row));
            }
        } else if row["root"] == "voices" {
            if ui.menu_item(format!("{} Open", icon::SOUND)) {
                open(actions, asset, None);
            }
        } else if asset.starts_with("forge:") {
            if ui.menu_item(format!("{} Open", icon::MAP)) {
                open(actions, asset, None);
            }
        }
        link_entries(ui, asset, true, actions);
    }
}

/// The map an item row opens on: the item's own map names (the open one
/// first), none when its `maps` is a scope word.
fn item_map(app: &App, row: &Value) -> Option<String> {
    let maps: Vec<&str> = row["maps"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if maps.is_empty() {
        return None;
    }
    open_map_of(app, &maps)
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

impl BrowserUi {
    pub(super) fn new_from(&mut self, asset: &str) {
        if let Ok(assets::AssetId::Tag(key)) = assets::AssetId::parse(asset) {
            self.create = Some(CreateAsset::Copy {
                asset: asset.to_owned(),
                to: Some(format!("{}_copy", key.path)),
            });
        }
    }

    pub(super) fn import_buttons(&mut self, ui: &Ui, dialogs: bool) {
        let _disabled = ui.begin_disabled_with_cond(!dialogs);
        if ui.small_button("Import sound...")
            && let Some(file) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).pick_file()
        {
            let name = import_name(&file, "sound");
            self.create = Some(CreateAsset::Sound {
                file, tag: format!("sounds\\{name}"), class: None,
            });
        }
        ui.same_line();
        if ui.small_button("Add model...")
            && let Some(file) = rfd::FileDialog::new().add_filter("Model YAML", &["yaml", "yml"]).pick_file()
        {
            let name = import_name(&file, "model");
            self.create = Some(CreateAsset::Model { file, tag: format!("models\\{name}\\model") });
        }
    }

    pub(super) fn create_modal(&mut self, ui: &Ui, actions: &mut Vec<Action>) {
        const POPUP: &str = "New asset";
        let Some(mut request) = self.create.take() else { return };
        if !ui.is_popup_open(POPUP) {
            ui.open_popup(POPUP);
        }
        let Some(_popup) = ui.begin_modal_popup_config(POPUP)
            .flags(dear_imgui_rs::WindowFlags::ALWAYS_AUTO_RESIZE).begin()
        else {
            self.create = Some(request);
            return;
        };
        let (tag, button) = match &mut request {
            CreateAsset::Copy { to, .. } => (to.get_or_insert_with(String::new), "Create copy"),
            CreateAsset::Sound { tag, .. } => (tag, "Import sound"),
            CreateAsset::Model { tag, .. } => (tag, "Add model"),
        };
        ui.set_next_item_width(360.0);
        ui.input_text("Tag path", tag).build();
        let disabled = ui.begin_disabled_with_cond(tag.trim().is_empty());
        let submit = ui.button(button);
        drop(disabled);
        ui.same_line();
        if submit {
            actions.push(Action::CreateAsset(request));
            ui.close_current_popup();
        } else if ui.button("Cancel") || ui.is_key_pressed(dear_imgui_rs::Key::Escape) {
            ui.close_current_popup();
        } else {
            self.create = Some(request);
        }
    }
}

fn import_name(file: &std::path::Path, fallback: &str) -> String {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or(fallback);
    crate::app::edit_target::slug_of(stem)
}

pub(super) fn source_view(ui: &Ui, app: &mut App, doc: crate::app::Doc) {
    use assets::SourceView;
    let Some(source) = app.source_asset(doc) else { return };
    match &source.view {
        SourceView::Tag(yaml) => ui.text_wrapped(yaml),
        SourceView::Sound { .. } => super::sound::source(ui, app, doc),
        SourceView::Model(mesh) => model_view(ui, mesh),
    }
}

fn model_view(ui: &Ui, mesh: &content_kit::mesh::Mesh) {
    if mesh.triangles() == 0 {
        ui.text_disabled("This model has no triangles");
        return;
    }
    let [left, top] = ui.cursor_screen_pos();
    let [width, height] = ui.content_region_avail().map(|v| v.max(1.0));
    ui.dummy([width, height]);
    let (lo, hi) = mesh.bounds();
    let extent = (0..3).map(|i| hi[i] - lo[i]).fold(f32::EPSILON, f32::max);
    let scale = width.min(height) * 0.42 / extent;
    let project = |position: [f32; 3]| {
        let [x, y, z] = std::array::from_fn(|i| position[i] - (lo[i] + hi[i]) * 0.5);
        [left + width * 0.5 + (x - y) * scale, top + height * 0.5 - (z + (x + y) * 0.4) * scale]
    };
    let draw = ui.get_window_draw_list();
    for part in &mesh.parts {
        for &triangle in &part.triangles {
            let [a, b, c] = triangle.map(|i| project(part.vertices[usize::from(i)].position));
            for (from, to) in [(a, b), (b, c), (c, a)] {
                draw.add_line(from, to, theme::ACCENT).build();
            }
        }
    }
}

#[cfg(test)]
mod linked_tests {
    use super::*;
    use crate::app::links::Link;

    #[test]
    fn a_dropped_file_goes_to_the_package_shown_else_the_one_selected() {
        let mut browser = BrowserUi::default();
        // The Tags root names none: the edit target takes the file.
        browser.selected = Some("package:kit-test".into());
        assert_eq!(browser.drop_package(), None);
        browser.root = Root::Packages;
        assert_eq!(browser.drop_package().as_deref(), Some("kit-test"));
        browser.package = Some("pogo".into());
        assert_eq!(browser.drop_package().as_deref(), Some("pogo"));
        browser.package = None;
        browser.selected = Some("item:kit-test:0".into());
        assert_eq!(browser.drop_package(), None);
    }

    #[test]
    fn a_thumbnail_is_a_line_high_and_no_wider_than_twice_that() {
        assert_eq!(thumb_size(64, 64, 18.0), [18.0, 18.0]);
        assert_eq!(thumb_size(32, 64, 18.0), [9.0, 18.0]);
        assert_eq!(thumb_size(640, 64, 18.0), [18.0 * THUMB_ASPECT, 18.0]);
        assert_eq!(thumb_size(64, 0, 18.0), [18.0 * THUMB_ASPECT, 18.0]);
    }

    #[test]
    fn a_link_view_filters_by_every_word_and_comes_forward_once() {
        let row = |asset: &str, role: &str, via: &str| assets::BrowseRow {
            asset: asset.into(),
            role: role.into(),
            via: via.into(),
        };
        let view = assets::BrowseView {
            mode: assets::BrowseMode::WhereUsed,
            of: "tag:hlmt:objects\\rifle_model".into(),
            rows: vec![
                row("tag:weap:objects\\rifle", "Objects", "node 0 +0x0"),
                row("package:kit-test", "Package", "edits"),
            ],
            problems: Vec::new(),
        };
        assert_eq!(browse_rows(&view, "").len(), 2);
        let found = browse_rows(&view, "PACKAGE edits");
        assert_eq!(found.len(), 1);
        assert_eq!(browse_label(found[0]), "package:kit-test  [Package]  edits");
        let mut browser = BrowserUi::default();
        assert!(browser.take_browse(1));
        assert!(!browser.take_browse(1));
        assert!(browser.take_browse(2));
    }

    #[test]
    fn linked_assets_keep_all_roots_and_can_return_to_a_single_asset() {
        let mut browser = BrowserUi::default();
        let rows = [
            Link { asset: "tag:weap:objects\\test".into(), role: "Objects", via: "test".into() },
            Link { asset: "package:example".into(), role: "Package", via: "test".into() },
            Link { asset: "tag:weap:objects\\test".into(), role: "Objects", via: "other".into() },
        ];
        browser.show_links(&rows);
        assert_eq!(browser.linked.as_ref().unwrap().len(), 2);
        assert!(browser.linked.as_ref().unwrap().contains(&"package:example".to_owned()));
        browser.locate("tag:weap:objects\\test");
        assert!(browser.linked.is_none());
        assert_eq!(browser.query, "objects\\test");
    }
}
