//! The Tags tool's windows. The outliner: which map's tags are shown (the
//! open map, or another map of the source browsed read-only: a shared
//! cache), a search, and the tags by group or as a tree of their paths,
//! each marked with where its data lives. The centre: the selected tag's
//! fields ([`crate::fields::panel`]), with export and import. The
//! inspector: the tag's data and raw resources (a bitmap's preview), then
//! the raw bytes of the element under the mouse with the hovered field
//! lit. State lives in [`crate::app::TagDoc`] ([`App::shown_tags`]);
//! everything goes back as [`Action`]s.
use std::path::PathBuf;

use dear_imgui_rs::{Condition, Ui};
use editor_core::tag_bundle;
use map::{FourCC, TagId};

use crate::app::tag_bundle::{ExportOptions, ImportOptions};
use crate::app::tag_doc::{Access, TagDoc, TagEntry};
use crate::app::tag_tree::{self, Arrange, Folder};
use crate::app::{Action, App, TagAction, tag_api};
use crate::fields::{self, PanelState};
use crate::theme::{self, Fonts, icon};
use crate::ui::map_list::MapListUi;
use crate::viewport::Gpu;

/// Tags a group or folder lists at most (type to narrow).
const LISTED: usize = 600;
/// Resources the inspector lists at most.
const RESOURCES_SHOWN: usize = 200;

/// The tool's UI-only state.
#[derive(Default)]
pub struct TagsUi {
    pub filter: String,
    /// The filter the tree's open state was last set for.
    opened_for: Option<String>,
    pub panel: PanelState,
    /// By group or by path.
    pub arrange: Arrange,
    /// Export the tags the selected one references too.
    pub export_dependencies: bool,
    /// Export raw resources (pixels, geometry) as blobs.
    pub export_resources: bool,
}

fn group_text(g: FourCC) -> String {
    tags::defs::fourcc_text(g)
}

/// Where a tag's data lives, as a short mark after its name: metadata
/// found in a shared cache, or raw data in one. Empty when all is here.
fn home_mark(doc: &TagDoc, t: &TagEntry) -> String {
    if let Some(m) = &t.elsewhere {
        return format!("  [in {m}]");
    }
    match doc.homes(t.id) {
        Some(h) if h.away() > 0 => format!("  [{}]", h.badge()),
        _ => String::new(),
    }
}

/// Which map's tags are shown: the open map, or another of the source.
fn map_picker(ui: &Ui, app: &App, actions: &mut Vec<Action>) {
    let open = app.loaded_map().filter(|_| app.session.is_open());
    let shown = match (app.browsing(), open) {
        (Some(b), _) => format!("{b} (read-only)"),
        (None, Some(m)) => m.to_owned(),
        (None, None) => "no map open".to_owned(),
    };
    ui.set_next_item_width(-1.0);
    if let Some(_c) = ui.begin_combo("##tag-map", &shown) {
        if let Some(m) = open
            && ui
                .selectable_config(format!("{m} (open map)##tag-map-open"))
                .selected(app.browsing().is_none())
                .build()
        {
            actions.push(Action::BrowseTags(None));
        }
        ui.separator();
        for m in app.maps() {
            if open.is_some_and(|o| o.eq_ignore_ascii_case(m)) {
                continue;
            }
            let selected = app.browsing().is_some_and(|b| b.eq_ignore_ascii_case(m));
            if ui
                .selectable_config(format!("{m}##tag-map-{m}"))
                .selected(selected)
                .build()
                && !selected
            {
                actions.push(Action::BrowseTags(Some(m.clone())));
            }
        }
    }
    if ui.is_item_hovered() {
        ui.tooltip_text(
            "Whose tags to show: the map open in the Scenario tool (editable), or any other \
             map of the source read-only, the shared caches among them",
        );
    }
}

/// The outliner: the map picker, the search, and the tags by group or by
/// path.
pub fn tree(ui: &Ui, app: &App, st: &mut TagsUi, fonts: &Fonts, actions: &mut Vec<Action>) {
    map_picker(ui, app, actions);
    let doc = app.shown_tags();
    if !doc.is_open() {
        ui.text_wrapped(match &doc.error {
            Some(e) => format!("The map's tags could not be read: {e}"),
            None => "Open a map (Tools, Maps) to edit its tags, or pick a map above to browse \
                     its tags."
                .to_owned(),
        });
        return;
    }
    if ui.radio_button_bool("Groups", st.arrange == Arrange::Group) {
        st.arrange = Arrange::Group;
    }
    ui.same_line();
    if ui.radio_button_bool("Paths", st.arrange == Arrange::Path) {
        st.arrange = Arrange::Path;
    }
    ui.set_next_item_width(-1.0);
    ui.input_text("##tag-filter", &mut st.filter)
        .hint(format!("{} words of group:name", icon::SEARCH))
        .build();
    let filter = st.filter.to_ascii_lowercase();
    let hits = tag_tree::search(doc.tags(), &filter);
    let refilter = st.opened_for.as_deref() != Some(filter.as_str());
    st.opened_for = Some(filter.clone());
    let narrowed = refilter && !filter.is_empty();
    match st.arrange {
        Arrange::Group => by_group(ui, doc, &hits, narrowed, fonts, actions),
        Arrange::Path => {
            let root = tag_tree::by_path(doc.tags(), &hits);
            let f = ui.push_font(fonts.bold);
            ui.text(format!("{} tags", root.count));
            f.pop();
            folder(ui, doc, &root, "", narrowed, actions);
        }
    }
}

fn by_group(
    ui: &Ui,
    doc: &TagDoc,
    hits: &[usize],
    narrowed: bool,
    fonts: &Fonts,
    actions: &mut Vec<Action>,
) {
    let groups = tag_tree::by_group(doc.tags(), hits);
    let sel_group = doc
        .selected
        .and_then(|id| doc.tag(id))
        .map(|t| group_text(t.group));
    let f = ui.push_font(fonts.bold);
    ui.text(format!("{} groups, {} tags", groups.len(), hits.len()));
    f.pop();
    for (g, list) in &groups {
        let mut node = ui
            .tree_node_config(format!("grp-{g}"))
            .label(format!("{g}  ({})", list.len()))
            .default_open(sel_group.as_deref() == Some(g.as_str()))
            .span_avail_width(true);
        if narrowed {
            node = node.opened(list.len() <= 50, Condition::Always);
        }
        let Some(_n) = node.push() else { continue };
        for &i in list.iter().take(LISTED) {
            row(ui, doc, &doc.tags()[i], &doc.tags()[i].name, actions);
        }
        if list.len() > LISTED {
            ui.text_disabled(format!("{} more: type to narrow", list.len() - LISTED));
        }
    }
}

fn folder(ui: &Ui, doc: &TagDoc, f: &Folder, path: &str, narrowed: bool, actions: &mut Vec<Action>) {
    for (name, sub) in &f.folders {
        let here = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}\\{name}")
        };
        let mut node = ui
            .tree_node_config(format!("dir-{here}"))
            .label(format!("{} {name}  ({})", icon::FOLDER, sub.count))
            .span_avail_width(true);
        if narrowed {
            node = node.opened(sub.count <= 50, Condition::Always);
        }
        let Some(_n) = node.push() else { continue };
        folder(ui, doc, sub, &here, narrowed, actions);
    }
    for &i in f.tags.iter().take(LISTED) {
        let t = &doc.tags()[i];
        let text = format!("{}  ({})", tag_tree::leaf(&t.name), group_text(t.group));
        row(ui, doc, t, &text, actions);
    }
    if f.tags.len() > LISTED {
        ui.text_disabled(format!("{} more: type to narrow", f.tags.len() - LISTED));
    }
}

/// One tag in the outliner: its text, an edited mark, read-only and where
/// its data lives.
fn row(ui: &Ui, doc: &TagDoc, t: &TagEntry, text: &str, actions: &mut Vec<Action>) {
    let mark = if doc.is_changed(t.id) { "  *" } else { "" };
    let lock = match t.access {
        Access::Edit => "",
        Access::Read(_) => "  (read-only)",
        Access::None(_) => "  (no data here)",
    };
    let home = home_mark(doc, t);
    if ui
        .selectable_config(format!("{text}{mark}{lock}{home}##tag-{:x}", t.id.0))
        .selected(doc.selected == Some(t.id))
        .build()
    {
        actions.push(Action::Tag(TagAction::Select(t.id)));
    }
}

/// The selected tag's fields, with export and import.
pub fn fields(
    ui: &Ui,
    app: &App,
    st: &mut TagsUi,
    fonts: &Fonts,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    let doc = app.shown_tags();
    let Some(id) = doc.selected else {
        ui.text_wrapped(
            "Pick a tag on the left. Every field is shown: named ones by their name, the \
             rest by their offset and the type their bytes suggest. Edits save with the map \
             (File, Save map as / Save as package). Import a tag bundle here to replace a \
             tag of the open map.",
        );
        if app.browsing().is_none() && doc.is_open() {
            import_button(ui, None, dialogs, actions);
        }
        return;
    };
    let Some(t) = doc.tag(id) else { return };
    let f = ui.push_font(fonts.bold);
    ui.text(format!("{}  {}", group_text(t.group), t.name));
    f.pop();
    ui.text_disabled(format!(
        "{:#010x}  at {:#010x}, {} bytes{}",
        t.id.0,
        t.address,
        t.size,
        if doc.is_changed(id) { "  *edited" } else { "" }
    ));
    if let Some(m) = app.browsing() {
        ui.text_disabled(format!("{} in {m}, browsed read-only", icon::DISC));
    }
    if let Some(line) = app.tag_shared_note(id) {
        ui.text_disabled(format!("{} {line}", icon::LAYERS));
    }
    match &t.access {
        Access::Edit => {}
        Access::Read(why) => {
            ui.text_colored(theme::WARN, format!("{} Read-only: {why}.", icon::ALERT));
        }
        Access::None(why) => {
            ui.text_colored(theme::WARN, format!("{} {why}.", icon::ALERT));
            if let Some(m) = &t.elsewhere
                && ui.button(format!("{} Show it in {m}", icon::EYE))
            {
                actions.push(Action::BrowseTags(Some(m.clone())));
            }
            return;
        }
    }
    io_bar(ui, app, st, t, dialogs, actions);
    if let Some(layout) = doc.layout(id) {
        let c = layout.coverage();
        ui.text_disabled(format!(
            "layout {}: {} named fields, {} by offset",
            layout.name, c.named_fields, c.unnamed_fields
        ));
        ui.same_line();
        ui.checkbox("padding", &mut st.panel.show_pad);
    } else {
        // No layout: the bytes only.
        if let Some(b) = doc.read(t.address, t.size.min(4096)) {
            ui.text(fields::hex_lines(&b));
        }
        return;
    }
    if let Some(p) = &st.panel.problem {
        ui.text_colored(theme::ERROR, p);
        ui.same_line();
        if ui.small_button("ok##problem") {
            st.panel.problem = None;
        }
    }
    ui.separator();
    ui.child_window("##fields").build(ui, || {
        fields::panel(ui, doc, id, &[], &mut st.panel, actions)
    });
}

/// Export the tag (with what it references, with its resources) and
/// import a bundle over it.
fn io_bar(
    ui: &Ui,
    app: &App,
    st: &mut TagsUi,
    t: &TagEntry,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    let has_layout = app.shown_tags().layout(t.id).is_some();
    if has_layout {
        if ui.small_button(format!("{} Export...##tag-export", icon::SAVE))
            && let Some(out) = export_dialog(dialogs, t)
        {
            actions.push(Action::TagExport {
                tags: vec![t.id],
                out: Some(out),
                options: ExportOptions {
                    dependencies: st.export_dependencies,
                    resources: st.export_resources,
                },
            });
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(
                "Write the tag as a tag bundle (.h2tag, JSON): its fields, blocks, references \
                 by name and string IDs by text. Outside any Git checkout.",
            );
        }
        ui.same_line();
        ui.checkbox("references##tag-export-deps", &mut st.export_dependencies);
        if ui.is_item_hovered() {
            ui.tooltip_text("Export the tags it references too, and theirs");
        }
        ui.same_line();
        ui.checkbox("resources##tag-export-raw", &mut st.export_resources);
        if ui.is_item_hovered() {
            ui.tooltip_text("Copy its raw resources (pixels, geometry) into the bundle");
        }
    }
    if app.browsing().is_none() && t.access == Access::Edit {
        ui.same_line();
        import_button(ui, Some(t.id), dialogs, actions);
    }
}

fn import_button(ui: &Ui, target: Option<TagId>, dialogs: bool, actions: &mut Vec<Action>) {
    let label = match target {
        Some(_) => format!("{} Import over this tag...##tag-import", icon::FOLDER_OPEN),
        None => format!("{} Import tags...##tag-import", icon::FOLDER_OPEN),
    };
    if ui.small_button(label)
        && let Some(path) = import_dialog(dialogs)
    {
        actions.push(Action::TagImport {
            path,
            options: ImportOptions {
                target,
                skip_missing: false,
            },
        });
    }
    if ui.is_item_hovered() {
        ui.tooltip_text(match target {
            Some(_) => {
                "Replace this tag with a bundle's (a bundle of one tag): one undo step, saved \
                 with the map"
            }
            None => {
                "Replace the open map's tags of the same group and name with a bundle's: one \
                 undo step, saved with the map. The map writer replaces tags; it does not add them"
            }
        });
    }
}

fn export_dialog(dialogs: bool, t: &TagEntry) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    rfd::FileDialog::new()
        .set_title("Export a tag")
        .add_filter("Tag bundle", &[tag_bundle::EXTENSION])
        .set_file_name(tag_bundle::default_file_name(&group_text(t.group), &t.name))
        .save_file()
}

fn import_dialog(dialogs: bool) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    rfd::FileDialog::new()
        .set_title("Import a tag bundle")
        .add_filter("Tag bundle", &[tag_bundle::EXTENSION])
        .pick_file()
}

/// The selected tag's data and raw resources, a bitmap's preview, and a
/// sound's permutations.
fn data(
    ui: &Ui,
    app: &App,
    id: TagId,
    gpu: &mut Gpu<'_>,
    pictures: &mut MapListUi,
    actions: &mut Vec<Action>,
) {
    let doc = app.shown_tags();
    let Some(t) = doc.tag(id) else { return };
    let Ok(res) = tag_api::resources(doc, id) else {
        return;
    };
    let data = res["data"].as_array().cloned().unwrap_or_default();
    let raw = res["raw"].as_array().cloned().unwrap_or_default();
    let is_bitmap = &t.group.0 == b"bitm";
    if data.is_empty() && raw.is_empty() && !is_bitmap {
        return;
    }
    if !ui.collapsing_header(
        format!("{} Data and resources##tag-data", icon::LAYERS),
        dear_imgui_rs::TreeNodeFlags::DEFAULT_OPEN,
    ) {
        return;
    }
    if is_bitmap {
        match app.tag_picture(id) {
            Some(Ok(pic)) => {
                if let Some(tex) = pictures.picture(gpu, &format!("tag-{:x}", id.0), pic) {
                    let w = ui.content_region_avail()[0].max(64.0);
                    let scale = (w / pic.width.max(1) as f32).min(1.0);
                    ui.image(tex, [pic.width as f32 * scale, pic.height as f32 * scale]);
                }
                ui.text_disabled(format!("image 0: {} x {}", pic.width, pic.height));
            }
            Some(Err(e)) => ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT)),
            None => {
                if ui.small_button(format!("{} Preview##tag-preview", icon::EYE)) {
                    actions.push(Action::TagPreview(id));
                }
            }
        }
    }
    for d in data.iter().take(RESOURCES_SHOWN) {
        ui.text_disabled(format!(
            "data  {}  {} bytes",
            d["path"].as_str().unwrap_or_default(),
            d["size"]
        ));
    }
    for r in raw.iter().take(RESOURCES_SHOWN) {
        let size = match r["size"].as_u64() {
            Some(n) => format!("{n} bytes"),
            None => "size not in the tag".to_owned(),
        };
        ui.text_disabled(format!(
            "raw  {}  {} +{:#x}  {size}",
            r["path"].as_str().unwrap_or_default(),
            r["file"].as_str().unwrap_or_default(),
            r["offset"].as_u64().unwrap_or_default(),
        ));
    }
    if data.len() > RESOURCES_SHOWN || raw.len() > RESOURCES_SHOWN || res["truncated"] == true {
        ui.text_disabled("(more not listed: tag.resources has them all)");
    }
    ui.separator();
}

/// The inspector: the tag's data and resources, then the raw bytes of the
/// element under the mouse, the hovered field lit.
pub fn hex(
    ui: &Ui,
    app: &App,
    st: &TagsUi,
    fonts: &Fonts,
    gpu: &mut Gpu<'_>,
    pictures: &mut MapListUi,
    actions: &mut Vec<Action>,
) {
    let doc = app.shown_tags();
    let Some(id) = doc.selected else {
        ui.text_disabled("No tag selected.");
        return;
    };
    data(ui, app, id, gpu, pictures, actions);
    let p = &st.panel;
    let (at, size, name) = match &p.focus {
        Some((at, l)) => (*at, l.size, l.name.clone()),
        None => match (doc.tag(id), doc.layout(id)) {
            (Some(t), Some(l)) => (t.address, l.size, l.name.clone()),
            _ => return,
        },
    };
    if let Some(h) = &p.hovered {
        let f = ui.push_font(fonts.bold);
        ui.text(&h.name);
        f.pop();
        ui.text_disabled(format!(
            "{} at +{:#x} ({:#010x}){}",
            h.kind,
            h.offset,
            h.address,
            if h.named { "" } else { ", unnamed" }
        ));
        if !h.meaning.is_empty() {
            ui.text_wrapped(&h.meaning);
        }
        ui.separator();
    }
    ui.text_disabled(format!("{name}: {size:#x} bytes at {at:#010x}"));
    let shown = size.min(4096);
    let Some(bytes) = doc.read(at, shown) else {
        return;
    };
    let source = doc.read_source(at, shown);
    let lit = p
        .hovered
        .as_ref()
        .map(|h| (h.address, h.address + h.len))
        .filter(|(lo, hi)| *lo >= at && *hi <= at + shown);
    for (row, chunk) in bytes.chunks(16).enumerate() {
        let base = row as u32 * 16;
        ui.text_disabled(format!("{base:04x}"));
        for (k, b) in chunk.iter().enumerate() {
            let a = at + base + k as u32;
            ui.same_line();
            let edited = source.is_some_and(|s| s[(base as usize) + k] != *b);
            let text = format!("{b:02x}");
            if lit.is_some_and(|(lo, hi)| a >= lo && a < hi) {
                ui.text_colored(theme::ACCENT, text);
            } else if edited {
                ui.text_colored(theme::WARN, text);
            } else {
                ui.text(text);
            }
        }
    }
    if size > shown {
        ui.text_disabled(format!("first {shown} of {size} bytes"));
    }
}

/// The selected tag, for scripts and tests.
pub fn selected(app: &App) -> Option<TagId> {
    app.shown_tags().selected
}
