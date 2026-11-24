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
use crate::play::issues::IssueIndex;
use crate::play::pair::Request;
use super::inspector;
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
    /// The navigation's field last focused in the panel.
    focus: Option<(TagId, String)>,
    /// The header's lines drawn last frame, without icons (`tag.view`'s
    /// `header`, for tests).
    pub header: Vec<String>,
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
            None => "Open a map from the Browser to edit its tags, or pick a map above to browse \
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
        issues: if browse { IssueIndex::empty() } else { inspector::issues(app) },
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
    // `all_tags` borrows the whole state; look the model up again after it.
    let model = st
        .scenario
        .model
        .as_ref()
        .filter(|_| scenario.is_some())
        .map(|(_, tree)| tree);
    if let Some(tree) = model {
        let label = if tree.incomplete {
            "Not reachable from the scenario (walk incomplete)".to_owned()
        } else {
            format!("Not reachable from the scenario ({})", tree.unreachable.len())
        };
        if let Some(_unreachable) = ui
            .tree_node_config("##unreachable-tags")
            .label(label)
            .span_avail_width(true)
            .push()
        {
            if tree.incomplete {
                ui.text_disabled("Reachability is unknown for tags outside the walk.");
            } else {
                tag_rows(
                    ui,
                    &view,
                    &tree.unreachable,
                    ListNames::GroupAndPath,
                    actions,
                );
            }
        }
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
        let label = view.marked(t, &text, "");
        let issue = view.issues.tag_row(t.id);
        if inspector::marked_row(ui, label, issue, |label| {
            ui.selectable_config(format!("{label}###tag-{:x}", t.id.0))
                .selected(view.selected == Some(t.id))
                .build()
        }) {
            view.select(t.id, None, actions);
        }
        // A double-click opens the tag as a document tab, in the editor
        // its group gets (docs/editor/studio-redesign.md). The row reports
        // its click on release; the double-click is the second press.
        if !view.browse
            && ui.is_item_hovered()
            && ui.is_mouse_double_clicked(dear_imgui_rs::MouseButton::Left)
        {
            actions.push(Action::OpenDoc(crate::app::Doc::Tag(t.id)));
        }
    });
}

struct RowView<'a> {
    doc: &'a TagDoc,
    browse: bool,
    paired: bool,
    issues: &'a IssueIndex,
    selected: Option<TagId>,
    edited_scenario: Option<TagId>,
}

impl RowView<'_> {
    fn marked(&self, t: &TagEntry, text: &str, label: &str) -> String {
        let edited = self.doc.is_changed(t.id);
        let mark = if edited || self.edited_scenario == Some(t.id) {
            "  *"
        } else {
            ""
        };
        let live = if self.paired && self.issues.tag_waiting(t.id) {
            "  [Pending]"
        } else {
            ""
        };
        let lock = match t.access {
            Access::Edit => "",
            Access::Read(_) => "  (read-only)",
            Access::None(_) => "  (no data here)",
        };
        let bsp = tag_tree::bsp_label(t)
            .filter(|bsp| bsp.as_str() != label)
            .map_or_else(String::new, |bsp| format!("  [{bsp}]"));
        format!("{text}{mark}{live}{lock}{bsp}{}", home_mark(self.doc, t))
    }

    fn select(
        &self,
        id: TagId,
        palette: Option<&[(scenario_edit::BlockKind, usize)]>,
        actions: &mut Vec<Action>,
    ) {
        if self.browse {
            actions.push(Action::Tag(TagAction::Select(id)));
        } else {
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
        let _id = ui.push_id(format!("{path:?}").as_str());
        let indent = 16.0 * path.len() as f32;
        if indent > 0.0 {
            ui.indent_by(indent);
        }
        let entry = row.tag.and_then(|id| view.doc.tag(id));
        let mut label = row.label.clone();
        if let Some(id) = row.tag {
            let prefix = if label.is_empty() {
                String::new()
            } else {
                format!("{label}  ")
            };
            label = match entry {
                Some(t) => view.marked(
                    t,
                    &format!("{prefix}{}  {}", group_text(t.group), t.name),
                    &label,
                ),
                None => format!("{prefix}Missing tag {:#010x}", id.0),
            };
        }
        if let Some(placements) = &row.placements {
            label.push_str(&format!("  ({} placed)", placements.len()));
        }
        if row.cycle {
            label.push_str("  (cycle)");
        }
        let issue = row.tag.and_then(|id| view.issues.tag_row(id));
        let shown = inspector::marked_row(ui, label, issue, |label| {
            ui.tree_node_config("##row")
                .label(label)
                .opened(*open, Condition::Always)
                .no_tree_push_on_open(true)
                .span_avail_width(true)
                .leaf(row.children.is_empty())
                .open_on_arrow(entry.is_some())
                .selected(entry.is_some_and(|t| view.selected == Some(t.id)))
                .push()
                .is_some()
        });
        if !row.children.is_empty() && shown != *open {
            opened.insert(path.clone(), shown);
        }
        if let Some(t) = entry
            && ui.is_item_clicked()
            && !ui.is_item_toggled_open()
        {
            view.select(t.id, row.placements.as_deref(), actions);
        }
        if issue.is_none() && let Some(via) = &row.via {
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
    app: &mut App,
    st: &mut TagsUi,
    fonts: &Fonts,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    let doc = app.shown_tags();
    let Some(id) = doc.selected else {
        ui.text_wrapped(
            "Pick a tag on the left. Every field is shown: named ones by their name, the \
             rest by their type and offset under Unnamed fields. Edits save with the map \
             (File, Save map as / Save as package). Import a tag bundle here to replace a \
             tag of the open map.",
        );
        if app.browsing().is_none() && doc.is_open() {
            import_button(ui, None, dialogs, actions);
        }
        return;
    };
    let Some(t) = doc.tag(id) else { return };
    let lines = header_top(app, t);
    st.header = lines.iter().map(|l| l.text.clone()).collect();
    for line in lines {
        let text = match line.icon {
            Some(glyph) => format!("{glyph} {}", line.text),
            None => line.text,
        };
        match line.tone {
            Tone::Title => {
                let f = ui.push_font(fonts.title);
                ui.text(text);
                f.pop();
            }
            Tone::Note => ui.text_disabled(text),
            Tone::Warn => ui.text_colored(theme::WARN, text),
        }
    }
    if let Access::None(_) = &t.access {
        if let Some(m) = &t.elsewhere
            && ui.button(format!("{} Show it in {m}", icon::EYE))
        {
            actions.push(Action::BrowseTags(Some(m.clone())));
        }
        return;
    }
    io_bar(ui, app, st, t, dialogs, actions);
    st.panel.follow(app);
    // The datum, the address and the layout's coverage are developer
    // facts: folded, never on screen until asked for.
    if let Some(_dev) = ui
        .tree_node_config("tag-dev")
        .label("Developer details")
        .push()
    {
        ui.text_disabled(format!(
            "Datum {:#010x}, at {:#010x}, {} bytes",
            t.id.0, t.address, t.size
        ));
        if let Some(line) = coverage(doc, id) {
            ui.text_disabled(line);
        }
    }
    if coverage(doc, id).is_none() {
        // No layout: the bytes only.
        if t.bsp.is_some() {
            if let Some(bytes) = doc.read_source(id, t.address, t.size) {
                theme::panel_body(ui, "##bsp-bytes", || bsp_hex(ui, bytes));
            }
        } else if let Some(b) = doc.read(id, t.address, t.size.min(4096)) {
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
    // A group with tag views has a tab bar: Fields, then its views.
    let shown = fields::rich::tab(t.group, &st.panel.settings.tab);
    if let Some(chosen) = fields::rich::tab_bar(ui, t.group, shown) {
        let mut settings = st.panel.settings.clone();
        settings.tab = chosen.to_owned();
        st.panel.set_settings(settings);
    }
    if let Some(view) = fields::rich::tag_view(t.group, shown) {
        (view.draw)(ui, app, id, actions);
        st.panel.store(app);
        return;
    }
    let scene = if app.browsing().is_none() {
        app.session.scenario()
    } else {
        None
    };
    let issues = if app.browsing().is_some() {
        IssueIndex::empty()
    } else {
        inspector::issues(app)
    };
    let mut resolution = None;
    // A field the navigation names (nav.go, tag.view) is focused once;
    // an element it names (Open alone) is the whole panel.
    let (want, element) = match app.browsing() {
        None => {
            let location = app.nav_location();
            let here = location.tag().filter(|&tag| tag == id);
            (here.zip(location.path), location.element.filter(|_| here.is_some()))
        }
        Some(_) => (None, None),
    };
    let steps = element
        .as_deref()
        .and_then(|e| tag_api::element_steps(doc, id, e).ok())
        .unwrap_or_default();
    if !steps.is_empty() {
        crumbs(ui, doc, id, &steps, actions);
    }
    let path: Vec<(u32, u32)> = steps.iter().map(|s| (s.field.offset, s.index)).collect();
    st.panel.open_alone = app.browsing().is_none();
    if st.focus != want {
        st.panel.clear_field_focus();
        if let Some((tag, path)) = &want
            && let Ok(field) = tag_api::field_focus(doc, *tag, path)
        {
            st.panel.focus_field(*tag, field);
        }
        st.focus = want;
    }
    // The filter row sits above the rows: Search fields, Changed only and
    // Show unnamed fields together.
    let mut settings = st.panel.settings.clone();
    ui.set_next_item_width(fields::LABEL);
    ui.input_text("##field-search", &mut settings.search)
        .hint(format!("{} Search fields", icon::SEARCH))
        .build();
    ui.same_line();
    ui.checkbox("Changed only", &mut settings.changed);
    if ui.is_item_hovered() {
        ui.set_item_tooltip("The fields that differ from the loaded file, lists included");
    }
    ui.same_line();
    ui.checkbox("Show unnamed fields", &mut settings.unnamed);
    if ui.is_item_hovered() {
        ui.set_item_tooltip("Open every element's Unnamed fields group");
    }
    st.panel.set_settings(settings);
    if !st.panel.settings.search.is_empty() || st.panel.settings.changed {
        // Hits replace the rows: a flat list of breadcrumb and value.
        let found = fields::find::find_from(
            doc,
            id,
            &mut steps.clone(),
            st.panel.link,
            issues,
            &st.panel.settings.search,
            st.panel.settings.changed,
            st.panel.settings.unnamed,
        );
        if let Some(why) = found.why {
            ui.text_colored(theme::WARN, why);
        }
        clipped_rows(ui, found.hits.len(), |i| {
            let hit = &found.hits[i];
            let _id = ui.push_id(hit.row["path"].as_str().unwrap_or_default());
            // The element's crumbs name where the field is; the header
            // already names the tag, so its crumb is left out.
            let trail = hit.breadcrumbs.iter().skip(1).cloned().collect::<Vec<_>>().join(" > ");
            let label = if trail.is_empty() {
                hit.row["label"].as_str().unwrap_or_default().to_owned()
            } else {
                format!("{trail} > {}", hit.row["label"].as_str().unwrap_or_default())
            };
            let value = hit.row["summary"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| hit.row["value"].to_string());
            if ui.selectable_config(format!("{label}##hit")).build() {
                let path = hit.row["path"].as_str().unwrap_or_default().to_owned();
                actions.push(Action::Navigate(NavAction::Go {
                    tag: id,
                    path: Some(path),
                    element: None,
                }));
                // Going there shows the field's rows, not the hit list.
                let mut settings = st.panel.settings.clone();
                settings.search.clear();
                settings.changed = false;
                st.panel.set_settings(settings);
            }
            ui.same_line_with_pos(fields::LABEL);
            ui.text_disabled(&value);
        });
    } else {
        // Nothing is pinned over the rows: the header scrolls with them, in
        // the window (docs/editor/tag-editing.md, "Panels on short windows").
        fields::panel_with_marks(
            ui,
            doc,
            id,
            &path,
            &mut st.panel,
            actions,
            fields::PanelRoot::Tags(scene),
            &mut inspector::tag_field_marks(ui, issues, &mut resolution),
        );
    }
    if let Some(action) = resolution {
        app.pairing.requests.push(Request::Apply(action));
    }
    st.panel.store(app);
}

/// The breadcrumbs over an element shown alone: the tag, then each list
/// step; a click returns there.
fn crumbs(
    ui: &Ui,
    doc: &TagDoc,
    id: TagId,
    steps: &[tag_api::ElementStep],
    actions: &mut Vec<Action>,
) {
    let all = fields::row::breadcrumbs(doc, id, steps);
    let last = all.len() - 1;
    for (k, (text, element)) in all.into_iter().enumerate() {
        if k > 0 {
            ui.same_line();
            ui.text_disabled(">");
            ui.same_line();
        }
        if k == last {
            ui.text(text);
        } else if ui.small_button(format!("{text}##crumb{k}")) {
            actions.push(Action::Navigate(NavAction::Go {
                tag: id,
                path: None,
                element,
            }));
        }
    }
}

/// How a header line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Title,
    Note,
    Warn,
}

pub(crate) struct HeaderLine {
    pub tone: Tone,
    pub icon: Option<&'static str>,
    pub text: String,
}

/// The header's lines over the selected tag's fields, before the export
/// bar: its kind and name, its path with the unsaved count, where it is
/// browsed, its BSP, its shared-map note, how changes reach a game (for a
/// tag edited here) and why it is read-only. No datum or address: those
/// are under Developer details.
pub(crate) fn header_top(app: &App, t: &TagEntry) -> Vec<HeaderLine> {
    let doc = app.shown_tags();
    let line = |tone, glyph, text| HeaderLine {
        tone,
        icon: glyph,
        text,
    };
    let path = match fields::row::unsaved_words(fields::row::unsaved(doc, t.id)) {
        Some(n) => format!("{} · {n}", t.name),
        None => t.name.clone(),
    };
    let mut out = vec![
        line(Tone::Title, None, fields::row::reference_text(doc, t.id.0)),
        line(Tone::Note, None, path),
    ];
    if let Some(m) = app.browsing() {
        out.push(line(
            Tone::Note,
            Some(icon::DISC),
            format!("in {m}, browsed read-only"),
        ));
    }
    if let Some(bsp) = tag_tree::bsp_label(t) {
        out.push(line(Tone::Note, None, bsp));
    }
    if let Some(note) = app.tag_shared_note(t.id) {
        out.push(line(Tone::Note, Some(icon::LAYERS), note));
    }
    if app.browsing().is_none() && t.access == Access::Edit {
        for text in link_lines(fields::Link::of(app)) {
            out.push(line(Tone::Note, None, text.to_string()));
        }
    }
    match &t.access {
        Access::Edit => {}
        Access::Read(why) => out.push(line(
            Tone::Warn,
            Some(icon::ALERT),
            format!("Read-only: {why}."),
        )),
        Access::None(why) => out.push(line(Tone::Warn, Some(icon::ALERT), format!("{why}."))),
    }
    out
}

/// How a change reaches the game, by the game link: saved with the map,
/// waiting in Pending, or sent as it is made (with what an unmarked field
/// means).
pub(crate) fn link_lines(link: fields::Link) -> &'static [&'static str] {
    match link {
        fields::Link::Off => &["Changes are saved with the map."],
        fields::Link::Viewing => {
            &["Game connected, viewing. Changes wait in Pending until you turn Edit on."]
        }
        fields::Link::Editing => &[
            "Game connected. Changes are sent as you make them.",
            "Fields with no mark are sent at once; when the game reads them is not known. \
             Reload the map to be sure.",
        ],
    }
}

/// The layout's coverage, under the header's Developer details: how many
/// of its fields have names. None without a layout.
pub(crate) fn coverage(doc: &TagDoc, id: TagId) -> Option<String> {
    let layout = doc.layout(id)?;
    let c = layout.coverage();
    Some(format!(
        "layout {}: {} named fields, {} by offset",
        layout.name, c.named_fields, c.unnamed_fields
    ))
}

/// The header's lines as drawn, without icons, for `tag.view`.
pub(crate) fn header(app: &App, id: TagId) -> Vec<String> {
    let doc = app.shown_tags();
    let Some(t) = doc.tag(id) else {
        return Vec::new();
    };
    header_top(app, t).into_iter().map(|l| l.text).collect()
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

/// The Data and bytes window: the selected row's details, then the bytes
/// of its element (or of one **Show bytes** named) with the field lit,
/// then the tag's data and resources, each a folding header. It follows
/// the selected row; the mouse moving changes nothing here.
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
    if let Some(t) = doc.tag(id)
        && t.bsp.is_some()
        && doc.layout(id).is_none()
        && let Some(bytes) = doc.read_source(id, t.address, t.size)
    {
        bsp_hex(ui, bytes);
        return;
    }
    let p = &st.panel;
    let picked = p.selected_row(doc, id);
    if let Some(r) = &picked {
        field_details(ui, doc, id, r, fonts, actions);
    }
    let lit = picked
        .as_ref()
        .map(|r| (r.address, r.address + r.field.kind.width()));
    element_bytes(ui, doc, id, p, lit);
    data(ui, app, id, gpu, pictures, actions);
}

/// Field details: the label, description, type and unit, the file's
/// value with **Reset to file value** when it differs, when a change takes
/// effect and why it is read-only; then, folded, the developer facts.
fn field_details(
    ui: &Ui,
    doc: &TagDoc,
    id: TagId,
    r: &fields::Row,
    fonts: &Fonts,
    actions: &mut Vec<Action>,
) {
    if !ui.collapsing_header(
        "Field details##tag-field",
        dear_imgui_rs::TreeNodeFlags::DEFAULT_OPEN,
    ) {
        return;
    }
    let f = r.field;
    let width = f.kind.width();
    let bold = ui.push_font(fonts.bold);
    ui.text(&r.label);
    bold.pop();
    if let Some(text) = f.doc.as_deref().filter(|s| !s.is_empty()) {
        ui.text_wrapped(text);
    }
    ui.text_disabled(match r.unit {
        Some(unit) => format!("{}, in {unit}", r.type_words),
        None => r.type_words.to_owned(),
    });
    if r.edited
        && let Some(file) = file_value(doc, id, r)
    {
        ui.text(format!("In the file: {file}"));
        if r.read_only.is_none() {
            ui.same_line();
            if ui.small_button("Reset to file value##reset")
                && let Ok(Some(write)) = tag_api::reset(doc, id, &r.path)
            {
                actions.push(Action::Tag(write));
                actions.push(Action::Tag(TagAction::Seal));
            }
        }
    }
    ui.text_wrapped(format!("{} {}", r.effect.sentence(), r.why));
    if let Some(why) = r.read_only {
        ui.text_colored(theme::WARN, why);
    }
    let Some(_dev) = ui
        .tree_node_config("tag-field-dev")
        .label("Developer details")
        .push()
    else {
        return;
    };
    ui.text_disabled(format!("Path for tag.set: {}", r.path));
    ui.text_disabled(format!(
        "Offset +{:#x}, {width} bytes, {}",
        f.offset,
        f.kind.label()
    ));
    ui.text_disabled(format!("Address {:#010x}", r.address));
    if let Some(b) = doc.read(id, r.address, width.min(64)) {
        ui.text_disabled(format!("Bytes {}", fields::hex(&b)));
    }
    if !f.meaning.is_empty() {
        ui.text_wrapped(format!("Evidence: {}", f.meaning));
    }
}

/// A changed field's value in the loaded file, as tag.read writes it.
fn file_value(doc: &TagDoc, id: TagId, r: &fields::Row) -> Option<String> {
    let f = r.field;
    if matches!(f.kind, tags::registry::Kind::Block(_) | tags::registry::Kind::Data) {
        return None;
    }
    let file = doc.read_source(id, r.address, f.kind.width())?;
    let at = r.address - f.offset;
    Some(match tag_api::value_of(doc, id, at, f, file) {
        serde_json::Value::String(text) => text,
        other => other.to_string(),
    })
}

/// Bytes, folded until **Show bytes** opens it: the element's bytes, the
/// selected field's lit, changed bytes marked.
fn element_bytes(ui: &Ui, doc: &TagDoc, id: TagId, p: &PanelState, lit: Option<(u32, u32)>) {
    let (at, size, name) = match &p.bytes {
        Some(e) if e.tag == id => (e.at, e.layout.size, e.layout.name.clone()),
        _ => match (doc.tag(id), doc.layout(id)) {
            (Some(t), Some(l)) => (t.address, l.size, l.name.clone()),
            _ => return,
        },
    };
    if p.open_bytes.take() {
        ui.set_next_item_open(true);
    }
    if !ui.collapsing_header("Bytes##tag-bytes", dear_imgui_rs::TreeNodeFlags::NONE) {
        return;
    }
    ui.text_disabled(format!("{name}: {size} bytes"));
    let shown = size.min(4096);
    let Some(bytes) = doc.read(id, at, shown) else {
        return;
    };
    let source = doc.read_source(id, at, shown);
    let lit = lit.filter(|(lo, hi)| *lo >= at && *hi <= at + shown);
    for (line, chunk) in bytes.chunks(16).enumerate() {
        let base = line as u32 * 16;
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

fn bsp_hex(ui: &Ui, bytes: &[u8]) {
    clipped_rows(ui, bytes.len().div_ceil(16), |i| {
        let at = i * 16;
        let row = &bytes[at..(at + 16).min(bytes.len())];
        ui.text(format!("{at:08x}  {}", fields::hex(row)));
    });
}

/// The selected tag, for scripts and tests.
pub fn selected(app: &App) -> Option<TagId> {
    app.shown_tags().selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
            issues: IssueIndex::empty(),
            selected: Some(doc.tags()[0].id),
            edited_scenario: None,
        };
        let hits = tag_tree::search(doc.tags(), "");
        assert_eq!(hits.len(), COUNT);

        let _turn = crate::headless::ONE_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
