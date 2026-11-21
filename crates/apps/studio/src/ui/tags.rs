//! The scenario tree, all tags by group or path, and the Tags tool's
//! field and resource windows. Actions go through the same navigation
//! and editing commands as the scene inspector.
use std::{collections::HashMap, path::PathBuf};

use dear_imgui_rs::{Condition, ListClipper, Ui};
use editor_core::tag_bundle;
use map::{FourCC, TagId};

use crate::app::tag_bundle::{ExportOptions, ImportOptions};
use crate::app::tag_doc::{Access, TagDoc, TagEntry};
use crate::app::tag_tree::{self, Arrange, Folder, Row, ScenarioRoots, ScenarioTree};
use crate::app::{Action, App, NavAction, TagAction, tag_api};
use crate::fields::{self, PanelState};
use crate::theme::{self, Fonts, icon};
use crate::ui::map_list::{self, MapListUi};
use crate::viewport::Gpu;

/// Resources the inspector lists at most.
const RESOURCES_SHOWN: usize = 200;

/// The tool's UI-only state.
#[derive(Default)]
pub struct TagsUi {
    pub filter: String,
    /// The filter the tree's open state was last set for.
    opened_for: Option<String>,
    scenario: ScenarioOutliner,
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
    let browse = app.browsing();
    let shown = match (browse, open) {
        (Some(b), _) => format!("{b} (read-only)"),
        (None, Some(m)) => m.to_owned(),
        (None, None) => "no map open".to_owned(),
    };
    ui.set_next_item_width(-1.0);
    if let Some(_c) = ui.begin_combo("##tag-map", &shown) {
        let entries = app.halo2_map_entries();
        // A package or a Map Pack map can be open without a source row.
        if let Some(m) = open
            && !entries
                .iter()
                .any(|entry| entry.stem.eq_ignore_ascii_case(m))
            && ui
                .selectable_config(format!("{m} (open map)##tag-map-open"))
                .selected(browse.is_none())
                .build()
        {
            actions.push(Action::BrowseTags(None));
        }
        let mut last = None;
        for entry in entries {
            map_list::section(ui, &mut last, entry.group);
            let m = entry.stem;
            let is_open = open.is_some_and(|o| o.eq_ignore_ascii_case(m));
            let selected = browse.map_or(is_open, |b| b.eq_ignore_ascii_case(m));
            let glyph = map_list::map_icon(entry.group);
            let label = entry.label;
            let suffix = if is_open { " (open map)" } else { "" };
            if ui
                .selectable_config(format!("{glyph}  {label}{suffix}##tag-map-{m}"))
                .selected(selected)
                .build()
                && !selected
            {
                actions.push(Action::BrowseTags((!is_open).then(|| m.to_owned())));
            }
        }
    }
    if ui.is_item_hovered() {
        ui.tooltip_text(
            "Whose tags to show: the open map (editable), or any other \
             map of the source read-only, the shared caches among them",
        );
    }
}

#[derive(Default)]
struct ScenarioOutliner {
    source: Option<(u64, Option<ScenarioRoots>)>,
    model: Option<((u64, u64), ScenarioTree)>,
    opened: HashMap<Vec<usize>, bool>,
}

impl ScenarioOutliner {
    fn update(&mut self, app: &App) {
        let Some(scenario) = app.session.scenario() else {
            return;
        };
        let world = app.session.world_rev();
        if self.source.as_ref().is_none_or(|(rev, _)| *rev != world) {
            let roots = app
                .session
                .source()
                .and_then(|source| source.open().ok())
                .map(|mut map| ScenarioRoots::read(&mut map));
            self.source = Some((world, roots));
            self.model = None;
            self.opened.clear();
        }
        let Some(roots) = self.source.as_ref().and_then(|(_, roots)| roots.as_ref()) else {
            return;
        };
        let rev = (app.tags.rev(), app.session.markers_rev());
        if self.model.as_ref().is_none_or(|(at, _)| *at != rev) {
            self.model = Some((
                rev,
                tag_tree::scenario_tree(&app.tags, scenario, roots, tag_api::DEFAULT_DEPTH),
            ));
        }
    }
}

/// The scenario comes first on the open map. The group and path browsers
/// are under All tags, including when browsing a shared cache.
pub fn tree(ui: &Ui, app: &App, st: &mut TagsUi, fonts: &Fonts, actions: &mut Vec<Action>) {
    map_picker(ui, app, actions);
    let doc = app.shown_tags();
    if !doc.is_open() {
        ui.text_wrapped(match &doc.error {
            Some(e) => format!("The map's tags could not be read: {e}"),
            None => "Open a map from the Maps tab to edit its tags, or pick a map above to browse \
                     its tags."
                .to_owned(),
        });
        return;
    }
    let browse = app.browsing().is_some();
    let scenario = (!browse).then(|| app.session.scenario()).flatten();
    let paired =
        !browse
            && app.pairing.published.as_ref().is_some_and(|status| {
                status["state"] == "connected" && status["matches"] == "same"
            });
    let view = RowView {
        doc,
        browse,
        paired,
        selected: selected(app),
        edited_scenario: scenario.filter(|s| s.is_modified()).map(|s| s.tag),
    };
    if !browse {
        st.scenario.update(app);
    }
    let model = st
        .scenario
        .model
        .as_ref()
        .filter(|_| scenario.is_some())
        .map(|(_, tree)| tree);
    if let Some(tree) = model {
        scenario_rows(ui, &view, tree, &mut st.scenario.opened, actions);
    }
    if let Some(_all) = ui
        .tree_node_config("##all-tags")
        .label("All tags")
        .default_open(true)
        .span_avail_width(true)
        .push()
    {
        all_tags(ui, &view, st, fonts, actions);
    }
    if let Some(tree) = st
        .scenario
        .model
        .as_ref()
        .filter(|_| scenario.is_some())
        .map(|(_, tree)| tree)
        && let Some(_unreachable) = ui
            .tree_node_config("##unreachable-tags")
            .label(format!(
                "Not reachable from the scenario ({})",
                tree.unreachable.len()
            ))
            .span_avail_width(true)
            .push()
    {
        tag_rows(
            ui,
            &view,
            &tree.unreachable,
            ListNames::GroupAndPath,
            actions,
        );
    }
}

fn all_tags(
    ui: &Ui,
    view: &RowView<'_>,
    st: &mut TagsUi,
    fonts: &Fonts,
    actions: &mut Vec<Action>,
) {
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
    let hits = tag_tree::search(view.doc.tags(), &filter);
    let refilter = st.opened_for.as_deref() != Some(filter.as_str());
    st.opened_for = Some(filter.clone());
    let narrowed = refilter && !filter.is_empty();
    match st.arrange {
        Arrange::Group => by_group(ui, view, &hits, narrowed, fonts, actions),
        Arrange::Path => {
            let root = tag_tree::by_path(view.doc.tags(), &hits);
            let f = ui.push_font(fonts.bold);
            ui.text(format!("{} tags", root.count));
            f.pop();
            folder(ui, view, &root, "", narrowed, actions);
        }
    }
}

fn by_group(
    ui: &Ui,
    view: &RowView<'_>,
    hits: &[usize],
    narrowed: bool,
    fonts: &Fonts,
    actions: &mut Vec<Action>,
) {
    let groups = tag_tree::by_group(view.doc.tags(), hits);
    let sel_group = view
        .selected
        .and_then(|id| view.doc.tag(id))
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
        tag_rows(ui, view, list, ListNames::Path, actions);
    }
}

fn folder(
    ui: &Ui,
    view: &RowView<'_>,
    f: &Folder,
    path: &str,
    narrowed: bool,
    actions: &mut Vec<Action>,
) {
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
        folder(ui, view, sub, &here, narrowed, actions);
    }
    tag_rows(ui, view, &f.tags, ListNames::GroupAndLeaf, actions);
}

enum ListNames {
    Path,
    GroupAndLeaf,
    GroupAndPath,
}

fn clipped_rows(ui: &Ui, len: usize, mut draw: impl FnMut(usize)) {
    for i in ListClipper::new(len).begin(ui).iter() {
        draw(i);
    }
}

fn tag_rows(
    ui: &Ui,
    view: &RowView<'_>,
    indices: &[usize],
    names: ListNames,
    actions: &mut Vec<Action>,
) {
    clipped_rows(ui, indices.len(), |i| {
        let t = &view.doc.tags()[indices[i]];
        let text = match names {
            ListNames::Path => t.name.clone(),
            ListNames::GroupAndLeaf => {
                format!("{}  ({})", tag_tree::leaf(&t.name), group_text(t.group))
            }
            ListNames::GroupAndPath => format!("{}  {}", group_text(t.group), t.name),
        };
        if ui
            .selectable_config(format!("{}###tag-{:x}", view.marked(t, &text), t.id.0))
            .selected(view.selected == Some(t.id))
            .build()
        {
            view.select(t.id, None, actions);
        }
    });
}

struct RowView<'a> {
    doc: &'a TagDoc,
    browse: bool,
    paired: bool,
    selected: Option<TagId>,
    edited_scenario: Option<TagId>,
}

impl RowView<'_> {
    fn marked(&self, t: &TagEntry, text: &str) -> String {
        let edited = self.doc.is_changed(t.id);
        let mark = if edited || self.edited_scenario == Some(t.id) {
            "  *"
        } else {
            ""
        };
        // The game receives placement writes. Edited tag metadata is
        // still pending on this map while paired.
        let live = if self.paired && edited {
            "  [Pending]"
        } else {
            ""
        };
        let lock = match t.access {
            Access::Edit => "",
            Access::Read(_) => "  (read-only)",
            Access::None(_) => "  (no data here)",
        };
        format!("{text}{mark}{live}{lock}{}", home_mark(self.doc, t))
    }

    fn select(
        &self,
        id: TagId,
        palette: Option<&[(scenario_edit::BlockKind, usize)]>,
        actions: &mut Vec<Action>,
    ) {
        actions.push(Action::Tag(TagAction::Select(id)));
        if !self.browse {
            let nav = match palette {
                Some(placements) => NavAction::SelectPalette {
                    tag: id,
                    placements: placements.to_vec(),
                },
                None => NavAction::SelectTag(id),
            };
            actions.push(Action::Navigate(nav));
        }
    }
}

struct VisibleRow<'a> {
    row: &'a Row,
    path: Vec<usize>,
    open: bool,
}

fn visible_rows<'a>(
    row: &'a Row,
    path: &mut Vec<usize>,
    opened: &HashMap<Vec<usize>, bool>,
    out: &mut Vec<VisibleRow<'a>>,
) {
    let open = !row.children.is_empty() && opened.get(path).copied().unwrap_or(path.len() <= 1);
    out.push(VisibleRow {
        row,
        path: path.clone(),
        open,
    });
    if open {
        for (i, child) in row.children.iter().enumerate() {
            path.push(i);
            visible_rows(child, path, opened, out);
            path.pop();
        }
    }
}

fn scenario_rows(
    ui: &Ui,
    view: &RowView<'_>,
    tree: &ScenarioTree,
    opened: &mut HashMap<Vec<usize>, bool>,
    actions: &mut Vec<Action>,
) {
    let mut rows = Vec::new();
    visible_rows(&tree.root, &mut Vec::new(), opened, &mut rows);
    clipped_rows(ui, rows.len(), |i| {
        let VisibleRow { row, path, open } = &rows[i];
        let _id = ui.push_id(format!("{path:?}"));
        let indent = 16.0 * path.len() as f32;
        if indent > 0.0 {
            ui.indent_by(indent);
        }
        let entry = row.tag.and_then(|id| view.doc.tag(id));
        let mut label = row.label.clone();
        if let Some(id) = row.tag {
            if !label.is_empty() {
                label.push_str("  ");
            }
            label = match entry {
                Some(t) => view.marked(t, &format!("{label}{}  {}", group_text(t.group), t.name)),
                None => format!("{label}Missing tag {:#010x}", id.0),
            };
        }
        if let Some(placements) = &row.placements {
            label.push_str(&format!("  ({} placed)", placements.len()));
        }
        if row.cycle {
            label.push_str("  (cycle)");
        }
        let shown = ui
            .tree_node_config("##row")
            .label(label)
            .opened(*open, Condition::Always)
            .no_tree_push_on_open(true)
            .span_avail_width(true)
            .leaf(row.children.is_empty())
            .open_on_arrow(entry.is_some())
            .selected(entry.is_some_and(|t| view.selected == Some(t.id)))
            .push()
            .is_some();
        if !row.children.is_empty() && shown != *open {
            opened.insert(path.clone(), shown);
        }
        if let Some(t) = entry
            && ui.is_item_clicked()
            && !ui.is_item_toggled_open()
        {
            view.select(t.id, row.placements.as_deref(), actions);
        }
        if let Some(via) = &row.via {
            ui.set_item_tooltip(via);
        }
        if indent > 0.0 {
            ui.unindent_by(indent);
        }
    });
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
    let f = ui.push_font(fonts.title);
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
        if ui.button(format!("{} Export...##tag-export", icon::SAVE))
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
    if ui.button(label)
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
                if ui.button(format!("{} Preview##tag-preview", icon::EYE)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an exclusive Dear ImGui context"]
    fn scenario_tree_large_group_draws_all_rows_as_it_scrolls() {
        use map::{
            CacheType, Map,
            build::{Block, TagPayload, TagSet},
        };

        const COUNT: usize = 5_000;
        const PREFIX: &str = "test_tag_";
        let group = FourCC(*b"demo");
        let mut set = TagSet::new(CacheType::Multiplayer, "many_tags", "test");
        set.add_group(group, None, None);
        for i in 0..COUNT {
            set.add_tag(
                group,
                &format!("{PREFIX}{i:04}"),
                TagPayload::Built(Block::zeroed(8)),
            )
            .unwrap();
        }
        let (bytes, _) = set.build().unwrap();
        let mut doc = TagDoc::default();
        doc.load(&mut Map::from_reader(std::io::Cursor::new(bytes)).unwrap())
            .unwrap();
        let view = RowView {
            doc: &doc,
            browse: true,
            paired: false,
            selected: Some(doc.tags()[0].id),
            edited_scenario: None,
        };
        let hits = tag_tree::search(doc.tags(), "");
        assert_eq!(hits.len(), COUNT);

        let (mut ctx, fonts) = crate::context(None);
        ctx.font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        ctx.io_mut().set_display_size([400.0, 300.0]);
        ctx.io_mut().set_delta_time(1.0 / 60.0);
        let mut drawn = vec![false; COUNT];
        let mut actions = Vec::new();
        let mut height = 0.0;
        // Each frame sees a little over ten rows. Overlap the pages to
        // check the whole range, including the last row.
        for first in (0..COUNT).step_by(10).chain([COUNT - 1]) {
            let ui = ctx.frame();
            crate::probe::begin();
            ui.window("Large tag group")
                .position([0.0, 0.0], Condition::Always)
                .size([380.0, 260.0], Condition::Always)
                .scroll([-1.0, first as f32 * height])
                .build(|| {
                    height = ui.text_line_height_with_spacing();
                    by_group(ui, &view, &hits, false, &fonts, &mut actions);
                });
            let mut rows_this_frame = 0;
            for item in crate::probe::items() {
                if let Some(index) = item.label.strip_prefix(PREFIX).and_then(|s| s.get(..4)) {
                    drawn[index.parse::<usize>().unwrap()] = true;
                    rows_this_frame += 1;
                }
            }
            assert!(rows_this_frame < 100);
            let _ = ctx.render_legacy();
        }
        assert_eq!(drawn.iter().filter(|&&shown| shown).count(), COUNT);
    }
}
