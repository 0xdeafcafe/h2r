//! The field panel: every field of a tag element, labelled, with a widget
//! per type, reusable by any tool that edits tags (the Tags tool; a HUD
//! editor showing one widget's element).
//!
//! [`panel`] draws the element at a path from a tag's root and pushes
//! [`Action::Tag`] commands; it never writes itself. It draws the rows of
//! [`row::rows`], one line each: an edited dot, the label in words, the
//! widget, the unit and, with a game connected, when a change takes
//! effect. `tag.fields` returns the same rows. Numbers drag; an enum
//! shows its value's name and opens the pick dialog ([`picker`]);
//! switches show how many are on and open to checkboxes, the unnamed bits
//! folded; colours pick; angles show degrees; bounds drag as a pair;
//! string IDs edit as text (any string the map has); a tag reference
//! shows its kind and name, with **Open** and **Change...** (the dialog,
//! every tag of the allowed kinds); blocks list their elements with add,
//! remove and duplicate; data shows (and edits) as hex, a `view=function`
//! data reference as its curve ([`curve`]). Counts, addresses
//! and resource pointers are shown, never edited. Fields nothing names are
//! labelled by type and offset and shown as their value suggests (a
//! float, an integer or hex), with **Show as** (the dialog) to view them
//! otherwise.
pub mod curve;
pub mod find;
pub mod picker;
pub mod rich;
pub mod row;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use dear_imgui_rs::{
    Condition, Drag, DragRange, ListClipper, NumericFormat, StyleColor, TreeNodeToken, Ui,
};
use map::{FourCC, TagId};
use scenario_edit::{Role, Scenario};
use tags::registry::{Field, FieldView, Hint, Kind, Layout};

pub use row::{Drawn, Link, Row, rows};

use crate::App;
use crate::app::tag_api::FieldFocus;
use crate::app::{Action, BlockOp, NavAction, TagAction, TagDoc, TagWrite, ToolKind};
use crate::play::issues::IssueIndex;
use crate::play::{LinkTone, tag_sync::Effect};
use crate::theme::{self, icon};

/// How an unnamed span is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Float,
    Int,
    Hex,
}

impl View {
    pub const ALL: [Self; 3] = [Self::Float, Self::Int, Self::Hex];

    /// The id in `tag.view` and the saved layout.
    pub fn id(self) -> &'static str {
        match self {
            Self::Float => "decimal",
            Self::Int => "whole_number",
            Self::Hex => "hex",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.id() == id)
    }

    /// The picker's words.
    pub fn words(self) -> &'static str {
        match self {
            Self::Float => "Decimal",
            Self::Int => "Whole number",
            Self::Hex => "Hex",
        }
    }
}

/// How the panel shows unnamed fields, kept in [`crate::App`] and saved
/// with the layout (the `[Studio][Tags]` section of the editor's ini),
/// with the filter row's state, which is not saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// **Show unnamed fields**: every Unnamed fields group open.
    pub unnamed: bool,
    /// `Show as` per group and offset ([`show_as_key`]).
    pub show_as: BTreeMap<String, View>,
    /// The filter row's **Search fields** text; empty shows the rows.
    /// Not saved with the layout.
    pub search: String,
    /// The filter row's **Changed only**: the fields that differ from the
    /// file. Not saved with the layout.
    pub changed: bool,
    /// The tab shown for a tag whose group has tag views
    /// ([`rich::tab`]): `""` or `fields` for the field panel. Not saved
    /// with the layout.
    pub tab: String,
}

/// The ini section the settings are saved in, after Dear ImGui's own.
const INI_SECTION: &str = "[Studio][Tags]";

impl Settings {
    /// The settings as an ini section.
    pub fn ini(&self) -> String {
        let mut out = format!("{INI_SECTION}\nShowUnnamed={}\n", u8::from(self.unnamed));
        for (key, view) in &self.show_as {
            out.push_str(&format!("ShowAs={key}={}\n", view.id()));
        }
        out
    }

    /// The settings in an ini text (defaults where it has none). Dear
    /// ImGui skips the section, having no handler for it.
    pub fn from_ini(text: &str) -> Self {
        let mut out = Self::default();
        let mut inside = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                inside = line == INI_SECTION;
                continue;
            }
            if !inside {
                continue;
            }
            if let Some(v) = line.strip_prefix("ShowUnnamed=") {
                out.unnamed = v == "1";
            } else if let Some(v) = line.strip_prefix("ShowAs=")
                && let Some((key, id)) = v.rsplit_once('=')
                && let Some(view) = View::from_id(id)
            {
                out.show_as.insert(key.to_owned(), view);
            }
        }
        out
    }
}

/// The key a `Show as` choice is kept by: the tag's group, the element's
/// path without indices and the offset (`weap:parts+0x8`), so it holds for
/// every tag of the group and every element of a list.
pub fn show_as_key(group: FourCC, prefix: &str, offset: u32) -> String {
    let mut shape = String::new();
    let mut depth = 0;
    for c in prefix.trim_end_matches('.').chars() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ if depth == 0 => shape.push(c),
            _ => {}
        }
    }
    format!("{}:{shape}+{offset:#x}", group_text(group))
}

/// The selected row, which the Data and bytes window details: its field
/// and the element it is in. A click on a row selects it; the mouse
/// moving does not change it.
#[derive(Clone, Debug)]
pub struct Selected {
    pub tag: TagId,
    /// The element's address and layout.
    pub at: u32,
    pub layout: Arc<Layout>,
    /// The field's address.
    pub address: u32,
    /// The element's `tag.set` path with its trailing dot (`parts[1].`).
    pub prefix: String,
}

/// An element whose bytes the Data and bytes window shows.
#[derive(Clone, Debug)]
pub struct Element {
    pub tag: TagId,
    pub at: u32,
    pub layout: Arc<Layout>,
}

/// The panel's UI state (one per place the panel is drawn).
#[derive(Default)]
pub struct PanelState {
    /// The element open per list, by the address of its `(count,
    /// address)` pair; a list with none open is not in it.
    index: HashMap<(TagId, u32), u32>,
    /// The selected row ([`Selected`]).
    pub selected: Option<Selected>,
    /// The element whose bytes are shown: the selected row's, or one
    /// **Show bytes** named.
    pub bytes: Option<Element>,
    /// **Show bytes** was pressed: the Bytes section opens once (taken
    /// by the window that draws it).
    pub open_bytes: std::cell::Cell<bool>,
    /// The settings drawn with ([`PanelState::follow`]); a change waits
    /// for [`PanelState::store`].
    pub settings: Settings,
    settings_changed: bool,
    /// Unnamed fields groups opened for a changed or marked field, by
    /// element address, so a person can fold them again.
    auto_opened: HashSet<(TagId, u32)>,
    /// Text being typed into string ID and data fields, by address.
    text: HashMap<(TagId, u32), String>,
    /// A long list's filter, by the address of its `(count, address)`.
    filters: HashMap<(TagId, u32), String>,
    /// Lists offer **Open alone** (the Tags tool, on the open map).
    pub open_alone: bool,
    /// The pick dialog, while open.
    pick: Option<picker::Open>,
    /// The curve dialog, while open.
    curve: Option<curve::Open>,
    /// The labels the pick dialog lists this frame (every one, clipped or
    /// not): what `tag.choices` returns.
    pub listed: Vec<String>,
    /// Why the last typed value was not taken.
    pub problem: Option<String>,
    focus_target: Option<(TagId, u32)>,
    focus_blocks: Vec<u32>,
    /// The game link the rows are drawn for ([`PanelState::follow`]).
    pub link: Link,
    issues: Option<Arc<IssueIndex>>,
    /// The rows drawn this frame, in order (nested elements included).
    pub drawn: Vec<Drawn>,
}

impl PanelState {
    /// Draw for the open map's game link and its marks, with the saved
    /// settings.
    pub fn follow(&mut self, app: &App) {
        if !self.settings_changed {
            self.settings = app.tag_fields.clone();
        }
        self.link = Link::of(app);
        self.issues = match (self.link, &app.pairing.link) {
            (Link::Off, _) | (_, None) => None,
            (_, Some(link)) => Some(link.issues.clone()),
        };
    }

    /// Keep a settings change the panel made (call after drawing).
    pub fn store(&mut self, app: &mut App) {
        if std::mem::take(&mut self.settings_changed) {
            app.tag_fields = self.settings.clone();
        }
    }

    /// Change the settings from a control drawn outside the panel.
    pub fn set_settings(&mut self, settings: Settings) {
        if settings != self.settings {
            self.settings = settings;
            self.settings_changed = true;
        }
    }

    /// The context a row of the element with `prefix` is built in.
    fn context<'a>(&'a self, prefix: &'a str) -> row::Context<'a> {
        row::Context {
            link: self.link,
            issues: self.issues.as_deref().unwrap_or(IssueIndex::empty()),
            prefix,
        }
    }

    pub fn focus_field(&mut self, tag: TagId, focus: FieldFocus) {
        self.clear_field_focus();
        self.focus_target = Some((tag, focus.address));
        for (pair, index) in focus.blocks {
            self.index.insert((tag, pair), index);
            self.focus_blocks.push(pair);
        }
    }

    pub fn clear_field_focus(&mut self) {
        self.focus_target = None;
        self.focus_blocks.clear();
    }

    pub fn has_field_focus(&self, tag: TagId) -> bool {
        self.focus_target.is_some_and(|(id, _)| id == tag)
    }

    /// The selected row of `tag`, built as the panel draws it.
    pub fn selected_row(&self, doc: &TagDoc, tag: TagId) -> Option<Row<'_>> {
        let s = self.selected.as_ref().filter(|s| s.tag == tag)?;
        row::rows(doc, tag, s.at, &s.layout, &self.context(&s.prefix))
            .into_iter()
            .find(|r| r.address == s.address)
    }
}

/// Width of the label column, in points.
pub(crate) const LABEL: f32 = 210.0;
/// Width of the edited dot's column, in points.
const DOT: f32 = 12.0;
/// Space between a widget and its unit or chip, in points.
const GAP: f32 = 8.0;
/// The button that picks how an unnamed value is shown.
const SHOW_AS: &str = "Show as";
/// Bytes of a data reference shown and editable as hex.
const DATA_SHOWN: u32 = 4096;
/// Elements a list has before it gets a filter box.
const FILTER_AFTER: u32 = 12;

/// Draw the element at `path` (per step, a block field's offset and the
/// element index) of `tag`'s root: every field, editable where the tag
/// may be written. The empty path is the root.
pub fn panel(
    ui: &Ui,
    doc: &TagDoc,
    tag: TagId,
    path: &[(u32, u32)],
    st: &mut PanelState,
    actions: &mut Vec<Action>,
) {
    panel_with_marks(
        ui,
        doc,
        tag,
        path,
        st,
        actions,
        PanelRoot::Tags(None),
        &mut |_, _| {},
    );
}

/// The Tags panel can jump to Scene for the scenario it edits.
pub fn tag_panel(
    ui: &Ui,
    doc: &TagDoc,
    tag: TagId,
    scene: Option<&Scenario>,
    st: &mut PanelState,
    actions: &mut Vec<Action>,
) {
    panel_with_marks(
        ui,
        doc,
        tag,
        &[],
        st,
        actions,
        PanelRoot::Tags(scene),
        &mut |_, _| {},
    );
}

/// The root panel in the inspector, with reference buttons that keep the
/// placement selection and remember the source field.
pub fn inspector_panel(
    ui: &Ui,
    doc: &TagDoc,
    tag: TagId,
    trail: &[TagId],
    st: &mut PanelState,
    actions: &mut Vec<Action>,
) {
    panel_with_marks(
        ui,
        doc,
        tag,
        &[],
        st,
        actions,
        PanelRoot::Inspector(trail),
        &mut |_, _| {},
    );
}

pub(crate) enum PanelRoot<'a> {
    Tags(Option<&'a Scenario>),
    Inspector(&'a [TagId]),
}

/// The panel gives its caller each field address just after the widget is drawn.
pub(crate) fn panel_with_marks(
    ui: &Ui,
    doc: &TagDoc,
    tag: TagId,
    path: &[(u32, u32)],
    st: &mut PanelState,
    actions: &mut Vec<Action>,
    root: PanelRoot<'_>,
    mark_field: &mut dyn FnMut(TagId, u32),
) {
    st.drawn.clear();
    let Some((at, layout)) = doc.resolve(tag, path) else {
        ui.text_disabled("Nothing to show: the element is not in the map's metadata.");
        return;
    };
    let (trail, scene) = match root {
        PanelRoot::Tags(scene) => (None, scene),
        PanelRoot::Inspector(trail) => (Some(trail), None),
    };
    let mut cx = Cx {
        ui,
        doc,
        tag,
        st,
        actions,
        trail,
        scene,
        mark_field,
        prefix: element_prefix(doc, tag, path),
        trailing: 0.0,
    };
    let _id = ui.push_id(tag.0 as usize);
    cx.element(at, &layout);
    picker::draw(ui, doc, st, actions);
    curve::draw(ui, doc, st, actions);
}

/// The `tag.set` path of the element at `path` with its trailing dot
/// (`parts[1].`); empty for the root.
fn element_prefix(doc: &TagDoc, tag: TagId, path: &[(u32, u32)]) -> String {
    let mut out = String::new();
    let Some(mut layout) = doc.layout(tag).cloned() else {
        return out;
    };
    for &(offset, index) in path {
        let Some(f) = layout.field_at(offset) else {
            break;
        };
        out.push_str(&format!("{}[{index}].", f.name));
        let Kind::Block(child) = &f.kind else {
            break;
        };
        let child = child.clone();
        layout = child;
    }
    out
}

struct Cx<'a, 'm> {
    ui: &'a Ui,
    doc: &'a TagDoc,
    tag: TagId,
    st: &'a mut PanelState,
    actions: &'a mut Vec<Action>,
    trail: Option<&'a [TagId]>,
    scene: Option<&'a Scenario>,
    prefix: String,
    mark_field: &'m mut dyn FnMut(TagId, u32),
    /// Width kept after the current row's widget for its unit and chip.
    trailing: f32,
}

fn key(address: u32) -> u64 {
    0x7461_6700_0000_0000 | u64::from(address)
}

/// The field panel's byte write, with the drag's merge key.
pub(crate) fn write_action(tag: TagId, address: u32, bytes: Vec<u8>, label: &str) -> Action {
    Action::Tag(TagAction::Write(TagWrite {
        tag,
        address,
        bytes,
        label: label.to_owned(),
        merge: Some(key(address)),
    }))
}

pub(crate) fn reference_action(trail: Option<&[TagId]>, prefix: &str, field: &str, tag: TagId) -> Action {
    match trail {
        Some(trail) => Action::Navigate(NavAction::Follow {
            from: trail.to_vec(),
            path: format!("{prefix}{field}"),
            tag,
        }),
        None => Action::Tag(TagAction::Select(tag)),
    }
}

fn le(b: &[u8], at: usize, n: usize) -> u64 {
    let mut v = 0u64;
    for k in (0..n).rev() {
        v = v << 8 | u64::from(b[at + k]);
    }
    v
}

fn sign(v: u64, bytes: usize) -> i64 {
    let shift = 64 - 8 * bytes as u32;
    ((v << shift) as i64) >> shift
}

fn group_text(g: FourCC) -> String {
    tags::defs::fourcc_text(g)
}

/// How an unnamed value is shown: the kept choice for `key`
/// ([`show_as_key`]), else as its bits suggest. Decimal needs four bytes.
pub fn shown_as(settings: &Settings, key: &str, f: &Field, v: u32) -> View {
    let len = f.kind.width();
    match settings.show_as.get(key).copied() {
        Some(View::Float) if len != 4 => View::Int,
        Some(view) => view,
        None if len == 4 => guess(v),
        None => View::Int,
    }
}

/// The guessed view of an unnamed word: a float when its bits read as an
/// ordinary float (or zero), an integer when small, else hex.
pub fn guess(v: u32) -> View {
    let f = f32::from_bits(v);
    if v == 0 {
        View::Int
    } else if f.is_finite() && (1e-4..1e7).contains(&f.abs()) {
        View::Float
    } else if v < 0x10000 || (v as i32) > -0x10000 && (v as i32) < 0 {
        View::Int
    } else {
        View::Hex
    }
}

impl<'a> Cx<'a, '_> {
    /// A row whose widget is a tree node (a list, data): the dot, the
    /// node labelled in words, then its summary, unit and chip.
    fn node(
        &mut self,
        at: u32,
        layout: &Arc<Layout>,
        r: &Row,
        id: String,
    ) -> Option<TreeNodeToken<'a>> {
        let ui = self.ui;
        self.dot(r);
        let node = ui.tree_node_config(id).label(&r.label).push();
        self.hover(at, layout, r);
        if let Some(summary) = &r.summary {
            ui.same_line_with_pos(LABEL);
            ui.text_disabled(summary);
            self.hover(at, layout, r);
        }
        self.tail(r);
        node
    }
}

impl Cx<'_, '_> {
    fn write(&mut self, address: u32, bytes: Vec<u8>, label: &str) {
        self.actions.push(write_action(self.tag, address, bytes, label));
    }

    /// After a widget: seal the drag when it ends.
    fn after(&mut self) {
        if self.ui.is_item_deactivated_after_edit() {
            self.actions.push(Action::Tag(TagAction::Seal));
        }
    }

    fn element(&mut self, at: u32, layout: &Arc<Layout>) {
        let ui = self.ui;
        let shown = row::rows(
            self.doc,
            self.tag,
            at,
            layout,
            &self.st.context(&self.prefix),
        );
        let (named, unnamed) = row::split(&shown);
        let mut section = String::new();
        for r in &named {
            if r.field.section != section {
                section = r.field.section.clone();
                if !section.is_empty() {
                    ui.separator_with_text(row::section_title(layout, &section));
                }
            }
            self.draw_row(at, layout, r);
        }
        if !unnamed.is_empty() {
            self.unnamed(at, layout, &unnamed, named.is_empty());
        }
    }

    fn draw_row(&mut self, at: u32, layout: &Arc<Layout>, r: &Row) {
        let ui = self.ui;
        let focused = self.st.focus_target == Some((self.tag, r.address));
        let _id = ui.push_id(r.field.offset as usize);
        self.st.drawn.push(r.drawn());
        self.field(at, layout, r);
        (self.mark_field)(self.tag, r.address);
        if focused {
            ui.set_scroll_here_y(0.5);
            self.st.clear_field_focus();
        }
    }

    /// The element's Unnamed fields group: folded, with what it holds,
    /// unless [`row::opens`] says otherwise. A changed or marked field
    /// opens it once, so it can be folded again.
    fn unnamed(&mut self, at: u32, layout: &Arc<Layout>, group: &[&Row], no_names: bool) {
        let ui = self.ui;
        if no_names {
            ui.text_disabled(row::NO_NAMES);
        }
        let key = (self.tag, at);
        let focus = self
            .st
            .focus_target
            .filter(|(tag, _)| *tag == self.tag)
            .map(|(_, a)| a);
        let marked = group.iter().any(|r| r.edited || r.live.is_some());
        // The toggle, an element nothing names and a focus open it every
        // frame (row::opens with the marks left out).
        let forced = row::opens(&[], self.st.settings.unnamed, no_names, None)
            || group.iter().any(|r| focus == Some(r.address));
        if forced || (marked && !self.st.auto_opened.contains(&key)) {
            ui.set_next_item_open(true);
        }
        if marked {
            self.st.auto_opened.insert(key);
        } else {
            self.st.auto_opened.remove(&key);
        }
        let node = ui
            .tree_node_config(format!("unnamed{at:x}"))
            .label(row::group_label(group.len()))
            .push();
        ui.same_line_with_pos(LABEL);
        ui.text_disabled(row::summary(group.iter().map(|r| r.field)));
        let Some(_node) = node else { return };
        for r in group {
            self.draw_row(at, layout, r);
        }
    }

    /// The edited dot's column: a dot when the value differs from the file.
    fn dot(&self, r: &Row) {
        let ui = self.ui;
        let x = ui.cursor_pos()[0];
        if r.edited {
            let [sx, sy] = ui.cursor_screen_pos();
            ui.get_window_draw_list()
                .add_circle(
                    [sx + DOT * 0.4, sy + ui.frame_height() * 0.5],
                    3.0,
                    theme::ACCENT,
                )
                .filled(true)
                .build();
        }
        ui.dummy([DOT, 0.0]);
        ui.same_line_with_pos(x + DOT);
    }

    /// The row's lead: the dot and the label, then the widget's column at
    /// the width the unit and chip leave.
    fn label(&mut self, at: u32, layout: &Arc<Layout>, r: &Row) {
        let ui = self.ui;
        self.dot(r);
        ui.align_text_to_frame_padding();
        let width = (LABEL - ui.cursor_pos()[0] - GAP).max(1.0);
        let dim = (!r.field.named).then(|| {
            ui.push_style_color(StyleColor::Text, ui.style_color(StyleColor::TextDisabled))
        });
        ui.selectable_config(format!("{}##label", r.label))
            .selected(self.is_selected(r))
            .size([width, 0.0])
            .build();
        drop(dim);
        self.hover(at, layout, r);
        ui.same_line_with_pos(LABEL);
        self.trailing = self.tail_width(r);
        ui.set_next_item_width(-self.trailing.max(1.0));
    }

    fn tail_width(&self, r: &Row) -> f32 {
        let ui = self.ui;
        let unit = r.unit.map_or(0.0, |u| ui.calc_text_size(u)[0] + GAP);
        let chip = r.chip.map_or(0.0, |c| ui.calc_text_size(c)[0] + 18.0 + GAP);
        let show_as = if matches!(r.field.kind, Kind::Unknown { .. }) {
            ui.calc_text_size(SHOW_AS)[0] + 2.0 * ui.clone_style().frame_padding()[0] + GAP
        } else {
            0.0
        };
        unit + chip + show_as
    }

    /// After the widget, on its line: the unit, then the chip.
    fn tail(&mut self, r: &Row) {
        let ui = self.ui;
        if let Some(unit) = r.unit {
            ui.same_line();
            ui.text_disabled(unit);
        }
        if let Some(chip) = r.chip {
            ui.same_line();
            let warn = r.effect == Effect::Reload || r.sending == Some(row::Sending::NotSent);
            let tone = if warn {
                LinkTone::Warn
            } else {
                LinkTone::View
            };
            theme::link_badge(ui, tone, chip);
            if ui.is_item_hovered() {
                match &r.live {
                    Some(live) => ui.tooltip_text(&live.text),
                    None => ui.tooltip_text(format!("{} {}", r.effect.sentence(), r.why)),
                }
            }
        }
        self.trailing = 0.0;
    }

    /// The last item's tooltip when it is hovered; a click on it selects
    /// the row.
    fn hover(&mut self, at: u32, layout: &Arc<Layout>, r: &Row) {
        let ui = self.ui;
        if !ui.is_item_hovered() {
            return;
        }
        let f = r.field;
        let mut tip = format!("{} at +{:#x}", r.type_words, f.offset);
        if let Some(unit) = r.unit {
            tip.push_str(&format!(", in {unit}"));
        }
        if !f.named {
            tip.push_str("\nNo definition names this field yet.");
        }
        if let Some(doc) = f.doc.as_deref().filter(|s| !s.is_empty()) {
            tip.push('\n');
            tip.push_str(doc);
        }
        if let Some(why) = r.read_only {
            tip.push('\n');
            tip.push_str(why);
        }
        if self.st.link.connected() {
            tip.push_str(&format!("\n{} {}", r.effect.sentence(), r.why));
        }
        if !f.meaning.is_empty() {
            tip.push('\n');
            tip.push_str(&f.meaning);
        }
        ui.set_item_tooltip(tip);
        if ui.is_item_clicked() {
            self.select(at, layout, r);
        }
    }

    /// Select the row: the Data and bytes window details it and shows its
    /// element's bytes.
    fn select(&mut self, at: u32, layout: &Arc<Layout>, r: &Row) {
        self.st.selected = Some(Selected {
            tag: self.tag,
            at,
            layout: layout.clone(),
            address: r.address,
            prefix: self.prefix.clone(),
        });
        self.st.bytes = Some(Element {
            tag: self.tag,
            at,
            layout: layout.clone(),
        });
    }

    fn is_selected(&self, r: &Row) -> bool {
        self.st
            .selected
            .as_ref()
            .is_some_and(|s| s.tag == self.tag && s.address == r.address)
    }

    fn field(&mut self, at: u32, layout: &Arc<Layout>, r: &Row) {
        let ui = self.ui;
        let f = r.field;
        let a = r.address;
        let w = f.kind.width();
        let Some(b) = self.doc.read(self.tag, a, w) else {
            ui.text_disabled(format!("{}: outside the tag's address space", r.label));
            return;
        };
        match &f.kind {
            Kind::Block(child) => return self.block(at, layout, r, child),
            Kind::Data => return self.data(at, layout, r, &b),
            Kind::Flags { bytes, bits } => return self.flags(at, layout, r, &b, *bytes, bits),
            _ => {}
        }
        self.label(at, layout, r);
        // A reference's Open works on a read-only tag; its Change says why
        // it does not.
        let is_ref = matches!(f.kind, Kind::TagRef { .. } | Kind::Datum { .. });
        let off = ui.begin_disabled_with_cond(r.read_only.is_some() && !is_ref);
        match &f.kind {
            Kind::Int {
                bytes,
                signed,
                count,
            } => {
                let n = *bytes as usize;
                let (lo, hi) = match (n, signed) {
                    (1, false) => (0i64, 0xff),
                    (1, true) => (-0x80, 0x7f),
                    (2, false) => (0, 0xffff),
                    (2, true) => (-0x8000, 0x7fff),
                    (_, false) => (0, 0xffff_ffff),
                    (_, true) => (i64::from(i32::MIN), i64::from(i32::MAX)),
                };
                let mut v: Vec<i64> = (0..*count as usize)
                    .map(|i| {
                        let u = le(&b, i * n, n);
                        if *signed { sign(u, n) } else { u as i64 }
                    })
                    .collect();
                if Drag::<i64, _>::new("##v")
                    .range(lo, hi)
                    .speed(0.2)
                    .build_array(ui, &mut v)
                {
                    let mut out = Vec::with_capacity(w as usize);
                    for x in v {
                        out.extend_from_slice(&x.to_le_bytes()[..n]);
                    }
                    self.write(a, out, &f.name);
                }
                self.after();
            }
            Kind::Float { count, hint } => self.float(a, f, &b, *count, *hint),
            Kind::Enum { .. } => {
                let width = (ui.content_region_avail()[0] - self.trailing).max(60.0);
                let shown = r.summary.as_deref().unwrap_or_default();
                if ui.button_with_size(format!("{shown}##choose"), [width, 0.0]) {
                    self.open_picker(r, picker::What::Choice);
                }
            }
            Kind::StringId => self.string_id(a, f, &b),
            Kind::TagRef { groups } => self.reference(r, groups, le(&b, 4, 4) as u32),
            Kind::Datum { groups } => self.reference(r, groups, le(&b, 0, 4) as u32),
            Kind::Word => {
                let mut v = le(&b, 0, 4);
                if Drag::<u64, _>::new("##v")
                    .range(0, 0xffff_ffff)
                    .display_format(fmt_hex("0x%08llx"))
                    .build(ui, &mut v)
                {
                    self.write(a, (v as u32).to_le_bytes().to_vec(), &f.name);
                }
                self.after();
            }
            Kind::Raw | Kind::RawSize { .. } | Kind::Bsp => {
                let words: Vec<String> = (0..w as usize / 4)
                    .map(|i| format!("{:#010x}", le(&b, 4 * i, 4)))
                    .collect();
                ui.text(words.join(" "));
            }
            Kind::Pad { .. } => {
                ui.text_disabled(hex(&b));
            }
            Kind::Unknown { len } => self.unknown(r, &b, *len),
            Kind::Block(_) | Kind::Data | Kind::Flags { .. } => unreachable!(),
        }
        self.hover(at, layout, r);
        off.end();
        self.tail(r);
    }

    /// Open the pick dialog for the row's field.
    fn open_picker(&mut self, r: &Row, what: picker::What) {
        let group = self.doc.tag(self.tag).map_or(FourCC(*b"????"), |t| t.group);
        let kept = show_as_key(group, &self.prefix, r.field.offset);
        self.st.pick = Some(picker::Open::new(self.tag, r.address, r.field, &r.label, kept, what));
    }

    /// A switches row: how many are on; open, the named bits as
    /// checkboxes in two columns, then the rest folded as Unnamed bits
    /// (shown directly when none is named).
    fn flags(
        &mut self,
        at: u32,
        layout: &Arc<Layout>,
        r: &Row,
        b: &[u8],
        bytes: u8,
        names: &[String],
    ) {
        let ui = self.ui;
        let Some(_node) = self.node(at, layout, r, format!("flg{:x}", r.address)) else {
            return;
        };
        let n = usize::from(bytes);
        let v = le(b, 0, n);
        let all = picker::bits(names, 8 * n as u32);
        let (named, rest): (Vec<&picker::Bit>, Vec<&picker::Bit>) =
            all.iter().partition(|bit| bit.name.is_some());
        let _off = ui.begin_disabled_with_cond(r.read_only.is_some());
        if named.is_empty() {
            return self.bit_columns(at, layout, r, n, v, &rest);
        }
        self.bit_columns(at, layout, r, n, v, &named);
        let open = ui
            .tree_node_config(format!("ubits{:x}", r.address))
            .label("Unnamed bits")
            .push();
        let set = rest.iter().filter(|bit| v >> bit.bit & 1 == 1).count();
        ui.same_line_with_pos(LABEL);
        ui.text_disabled(format!("{set} of {} on", rest.len()));
        if open.is_some() {
            self.bit_columns(at, layout, r, n, v, &rest);
        }
    }

    /// Bits as checkboxes, two to a line; a click flips one, one undo step.
    fn bit_columns(
        &mut self,
        at: u32,
        layout: &Arc<Layout>,
        r: &Row,
        n: usize,
        v: u64,
        list: &[&picker::Bit],
    ) {
        let ui = self.ui;
        let x = ui.cursor_pos()[0];
        let half = (ui.content_region_avail()[0] * 0.5).max(120.0);
        for (i, bit) in list.iter().enumerate() {
            if i % 2 == 1 {
                ui.same_line_with_pos(x + half);
            }
            let mut on = v >> bit.bit & 1 == 1;
            if ui.checkbox(format!("{}##b{}", bit.label, bit.bit), &mut on) {
                let flipped = v ^ (1 << bit.bit);
                self.write(r.address, flipped.to_le_bytes()[..n].to_vec(), &r.field.name);
                self.actions.push(Action::Tag(TagAction::Seal));
            }
            self.hover(at, layout, r);
        }
    }

    /// A tag reference: its kind and name (the full path on hover), then
    /// **Open** and **Change...** (the pick dialog), on the next line when
    /// the name leaves no room for them.
    fn reference(&mut self, r: &Row, groups: &[FourCC], id: u32) {
        let ui = self.ui;
        let doc = self.doc;
        let summary = r.summary.as_deref().unwrap_or_default();
        let style = ui.clone_style();
        let button = |label: &str| ui.calc_text_size(label)[0] + 2.0 * style.frame_padding()[0];
        let gap = style.item_spacing()[0];
        let buttons = button("Open") + button("Change...") + 2.0 * gap;
        let fits = ui.calc_text_size(summary)[0] + buttons <= ui.content_region_avail()[0];
        ui.text(summary);
        if ui.is_item_hovered() {
            let mut tip = doc.tag_label(id);
            if !groups.is_empty() {
                let kinds: Vec<String> = groups.iter().map(|g| row::kind_words(doc, *g)).collect();
                tip.push_str(&format!("\nTakes: {}", kinds.join(", ")));
            }
            ui.set_item_tooltip(tip);
        }
        if fits {
            ui.same_line();
        }
        let target = doc.tag(TagId(id)).map(|t| t.id);
        {
            let _off = ui.begin_disabled_with_cond(target.is_none());
            if ui.small_button("Open##open")
                && let Some(tag) = target
            {
                self.actions.push(reference_action(self.trail, &self.prefix, &r.field.name, tag));
            }
        }
        ui.same_line();
        let _off = ui.begin_disabled_with_cond(r.read_only.is_some());
        if ui.small_button("Change...##change") {
            self.open_picker(r, picker::What::Reference);
        }
    }

    fn float(&mut self, a: u32, f: &Field, b: &[u8], count: u32, hint: Hint) {
        let ui = self.ui;
        let mut v: Vec<f32> = (0..count as usize)
            .map(|i| f32::from_bits(le(b, 4 * i, 4) as u32))
            .collect();
        let deg = 180.0 / std::f32::consts::PI;
        let changed = match hint {
            Hint::Colour if count == 3 => {
                let mut c = [v[0], v[1], v[2]];
                let ch = ui.color_edit3("##v", &mut c);
                v.copy_from_slice(&c);
                ch
            }
            Hint::Bounds if count == 2 => {
                let (mut lo, mut hi) = (v[0], v[1]);
                let ch = DragRange::<f32, _>::new("##v")
                    .speed(0.01)
                    .build(ui, &mut lo, &mut hi);
                v = vec![lo, hi];
                ch
            }
            Hint::Angle => {
                let mut d: Vec<f32> = v.iter().map(|x| x * deg).collect();
                let ch = Drag::<f32, _>::new("##v")
                    .speed(0.5)
                    .display_format(fmt_f("%.2f"))
                    .build_array(ui, &mut d);
                v = d.iter().map(|x| x / deg).collect();
                ch
            }
            _ => Drag::<f32, _>::new("##v")
                .speed(0.01)
                .display_format(fmt_f("%.4f"))
                .build_array(ui, &mut v),
        };
        if changed {
            let out = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            self.write(a, out, &f.name);
        }
        self.after();
    }

    fn unknown(&mut self, r: &Row, b: &[u8], len: u32) {
        let ui = self.ui;
        let (a, f) = (r.address, r.field);
        let v = le(b, 0, len as usize) as u32;
        let group = self.doc.tag(self.tag).map_or(FourCC(*b"????"), |t| t.group);
        let key = show_as_key(group, &self.prefix, f.offset);
        let view = shown_as(&self.st.settings, &key, f, v);
        let n = len as usize;
        match view {
            View::Float => {
                let mut x = f32::from_bits(v);
                if Drag::<f32, _>::new("##v")
                    .speed(0.01)
                    .display_format(fmt_f("%.4f"))
                    .build(ui, &mut x)
                {
                    self.write(a, x.to_le_bytes().to_vec(), &f.name);
                }
            }
            View::Hex => {
                let mut x = u64::from(v);
                let hexfmt = format!("0x%0{}llx", 2 * n);
                if Drag::<u64, _>::new("##v")
                    .range(0, (1u64 << (8 * n)) - 1)
                    .display_format(fmt_hex(hexfmt.as_str()))
                    .build(ui, &mut x)
                {
                    self.write(a, x.to_le_bytes()[..n].to_vec(), &f.name);
                }
            }
            View::Int => {
                let mut x = sign(u64::from(v), n);
                let lim = 1i64 << (8 * n - 1);
                if Drag::<i64, _>::new("##v")
                    .range(-lim, lim - 1)
                    .speed(0.2)
                    .build(ui, &mut x)
                {
                    self.write(a, x.to_le_bytes()[..n].to_vec(), &f.name);
                }
            }
        }
        self.after();
        ui.same_line();
        if ui.small_button(SHOW_AS) {
            self.open_picker(r, picker::What::ShowAs);
        }
        if ui.is_item_hovered() {
            ui.set_item_tooltip(
                "How to show this value: kept for this field of every tag of the kind",
            );
        }
    }

    fn string_id(&mut self, a: u32, f: &Field, b: &[u8]) {
        let ui = self.ui;
        let v = le(b, 0, 4) as u32;
        let current = match self.doc.string(v) {
            Some(s) => s.to_owned(),
            None if v == u32::MAX => String::new(),
            None => format!("#{v:#010x}"),
        };
        let text = self
            .st
            .text
            .entry((self.tag, a))
            .or_insert_with(|| current.clone());
        let entered = ui.input_text("##v", text).enter_returns_true(true).build();
        let active = ui.is_item_active();
        let typed = text.clone();
        if entered || ui.is_item_deactivated_after_edit() {
            if typed == current {
            } else if typed.is_empty() {
                self.write(a, u32::MAX.to_le_bytes().to_vec(), &f.name);
            } else if let Some(id) = self.doc.string_id(&typed) {
                self.write(a, id.to_le_bytes().to_vec(), &f.name);
            } else {
                // A string the map lacks: added to the map with the write.
                self.actions.push(Action::Tag(TagAction::SetString {
                    tag: self.tag,
                    address: a,
                    text: typed,
                    label: f.name.clone(),
                }));
            }
            self.st.text.remove(&(self.tag, a));
        } else if !active {
            // Follow the field (undo, another element) while not typing.
            self.st.text.insert((self.tag, a), current);
        }
    }

    /// A list: `Parts    2 items` with **Add**; open, one element's fields
    /// directly, or a row per element (`1  scope`, clipped, with a filter
    /// past [`FILTER_AFTER`]) of which one is open at a time. An empty list
    /// is one line and does not open.
    fn block(&mut self, at: u32, layout: &Arc<Layout>, r: &Row, child: &Arc<Layout>) {
        let ui = self.ui;
        let pair = r.address;
        let Some(list) = r.list else {
            ui.text_disabled(format!(
                "{}: its elements are outside the tag's address space",
                r.label
            ));
            return;
        };
        let n = list.count;
        let tag = self.tag;
        let open = self.st.index.get(&(tag, pair)).copied().filter(|&i| i < n);
        if n == 0 {
            self.label(at, layout, r);
            ui.text_disabled(r.summary.as_deref().unwrap_or_default());
            self.hover(at, layout, r);
            self.list_op(pair, child, n, &format!("{} Add##add", icon::PLUS), BlockOp::Add);
            self.scene_button(at, r, open.unwrap_or(0));
            self.tail(r);
            return;
        }
        if self.st.has_field_focus(tag) && self.st.focus_blocks.contains(&pair) {
            ui.set_next_item_open(true);
        }
        let opened = self.node(at, layout, r, format!("blk{pair:x}"));
        self.list_op(pair, child, n, &format!("{} Add##add", icon::PLUS), BlockOp::Add);
        self.scene_button(at, r, open.unwrap_or(0));
        let Some(_node) = opened else {
            return;
        };
        if n == 1 {
            self.element_ops(r, &list, child, 0, false);
            return self.open_element(r, &list, child, 0);
        }
        let shown = if n > FILTER_AFTER {
            let text = self.st.filters.entry((tag, pair)).or_default();
            ui.set_next_item_width(-1.0);
            ui.input_text(format!("##filter{pair:x}"), text)
                .hint(format!("{} Filter {} by title", icon::SEARCH, r.label))
                .build();
            let filter = text.clone();
            row::matching(self.doc, tag, &list, child, &filter)
        } else {
            (0..n).collect()
        };
        let split = open.and_then(|i| shown.iter().position(|&k| k == i));
        let (above, below) = match split {
            Some(k) => (&shown[..k], &shown[k + 1..]),
            None => (&shown[..], &[][..]),
        };
        for i in ListClipper::new(above.len()).begin(ui).iter() {
            self.element_row(r, &list, child, above[i], false);
        }
        if let Some(k) = split {
            self.element_row(r, &list, child, shown[k], true);
        }
        for i in ListClipper::new(below.len()).begin(ui).iter() {
            self.element_row(r, &list, child, below[i], false);
        }
    }

    /// An element's row, `1  scope`, with its commands; open, its fields
    /// below it. Opening one closes the one open before.
    fn element_row(
        &mut self,
        r: &Row,
        list: &row::List,
        child: &Arc<Layout>,
        i: u32,
        open: bool,
    ) {
        let ui = self.ui;
        let pair = r.address;
        let title = row::element_title(self.doc, self.tag, list.address + i * child.size, child);
        let expanded = ui
            .tree_node_config(format!("el{pair:x}-{i}"))
            .label(row::element_text(i, title.as_deref()))
            .opened(open, Condition::Always)
            .push();
        if expanded.is_some() != open {
            if expanded.is_some() {
                self.st.index.insert((self.tag, pair), i);
            } else {
                self.st.index.remove(&(self.tag, pair));
            }
        }
        self.element_ops(r, list, child, i, true);
        if expanded.is_some() {
            self.open_element(r, list, child, i);
        }
    }

    /// An element's commands: **Open alone** (in the Tags tool), Duplicate,
    /// Remove and **Show bytes**; on the row's line when `same_line`.
    fn element_ops(
        &mut self,
        r: &Row,
        list: &row::List,
        child: &Arc<Layout>,
        i: u32,
        same_line: bool,
    ) {
        let ui = self.ui;
        let pair = r.address;
        let path = format!("{}{}[{i}]", self.prefix, r.field.name);
        if self.st.open_alone && self.trail.is_none() {
            if same_line {
                ui.same_line();
            }
            if ui.small_button(format!("Open alone##alone{i}")) {
                self.actions.push(Action::Navigate(NavAction::Go {
                    tag: self.tag,
                    path: None,
                    element: Some(path),
                }));
            }
            if ui.is_item_hovered() {
                ui.set_item_tooltip("Show this element as the whole panel");
            }
        } else if !same_line {
            ui.dummy([0.0, 0.0]);
        }
        let n = list.count;
        let duplicate = format!("{} Duplicate##dup{i}", icon::COPY);
        self.list_op(pair, child, n, &duplicate, BlockOp::Duplicate(i));
        let remove = format!("{} Remove##rm{i}", icon::TRASH);
        self.list_op(pair, child, n, &remove, BlockOp::Remove(i));
        ui.same_line();
        if ui.small_button(format!("Show bytes##show-bytes{i}")) {
            self.st.bytes = Some(Element {
                tag: self.tag,
                at: list.address + i * child.size,
                layout: child.clone(),
            });
            self.st.open_bytes.set(true);
        }
        if ui.is_item_hovered() {
            ui.set_item_tooltip("Show this element's bytes in Data and bytes");
        }
    }

    /// The fields of element `i`, under its path.
    fn open_element(&mut self, r: &Row, list: &row::List, child: &Arc<Layout>, i: u32) {
        let _id = self.ui.push_id(i as usize);
        let prefix_len = self.prefix.len();
        {
            use std::fmt::Write as _;
            let _ = write!(self.prefix, "{}[{i}].", r.field.name);
        }
        self.element(list.address + i * child.size, child);
        self.prefix.truncate(prefix_len);
    }

    /// A list command on the current line, disabled with the reason as its
    /// tooltip ([`TagDoc::block_refusal`]). Add and Duplicate open the new
    /// element.
    fn list_op(&mut self, pair: u32, child: &Arc<Layout>, n: u32, label: &str, op: BlockOp) {
        let ui = self.ui;
        let tag = self.tag;
        let why = self.doc.block_refusal(tag, pair, child, &op);
        ui.same_line();
        {
            let _d = ui.begin_disabled_with_cond(why.is_some());
            if ui.small_button(label) {
                self.actions.push(Action::Tag(TagAction::Block {
                    tag,
                    pair,
                    layout: child.clone(),
                    op: op.clone(),
                }));
                match op {
                    BlockOp::Add => {
                        self.st.index.insert((tag, pair), n);
                    }
                    BlockOp::Duplicate(i) => {
                        self.st.index.insert((tag, pair), i + 1);
                    }
                    BlockOp::Remove(_) => {}
                }
            }
        }
        if let Some(why) = why
            && ui.is_item_hovered_with_flags(dear_imgui_rs::ItemHoveredFlags::ALLOW_WHEN_DISABLED)
        {
            ui.set_item_tooltip(why);
        }
    }

    /// **Open ... in Scene** on a scenario placement list (or palette) of
    /// the scenario's root, selecting element `i`'s placement.
    fn scene_button(&mut self, at: u32, r: &Row, i: u32) {
        let ui = self.ui;
        let f = r.field;
        let Some(scene) = self.scene.filter(|s| s.tag == self.tag && s.address == at) else {
            return;
        };
        let Some(def) = self.doc.scene_block(self.tag, r.address) else {
            return;
        };
        ui.same_line();
        if ui.small_button(format!("Open {} in Scene", def.label)) {
            let block = scene.block(def.kind);
            let index = if f.offset == def.offset {
                let index = i as usize;
                (index < block.len()).then_some(index)
            } else {
                def.role(Role::PaletteIndex).and_then(|field| {
                    (0..block.len()).find(|&index| {
                        scene
                            .get(def.kind, index, field.name)
                            .ok()
                            .and_then(|value| value.as_int())
                            == Some(i64::from(i))
                    })
                })
            };
            self.actions.push(Action::SetTool(ToolKind::Scenario));
            self.actions
                .push(Action::Select(index.map(|index| (def.kind, index))));
        }
    }

    fn data(&mut self, at: u32, layout: &Arc<Layout>, r: &Row, b: &[u8]) {
        let ui = self.ui;
        let f = r.field;
        let (n, a) = (le(b, 0, 4) as u32, le(b, 4, 4) as u32);
        // A function view: the curve row when the data parses as a
        // function; else the bytes row, with the reason said.
        if f.view == Some(FieldView::Function)
            && n > 0
            && let Some(bytes) = self.doc.read(self.tag, a, n)
            && let Ok(fun) = tags::effects::function::Function::parse(&bytes)
        {
            return curve::row(self, at, layout, r, fun);
        }
        let Some(_node) = self.node(at, layout, r, format!("dat{:x}", r.address)) else {
            return;
        };
        if f.view == Some(FieldView::Function) {
            ui.text_disabled(curve::not_a_function(self.doc, self.tag, n, a));
        }
        if n == 0 {
            return;
        }
        let shown = n.min(DATA_SHOWN);
        let Some(bytes) = self.doc.read(self.tag, a, shown) else {
            ui.text_disabled("its bytes are outside the tag's address space");
            return;
        };
        let editable = r.read_only.is_none() && n <= DATA_SHOWN;
        let current = hex_lines(&bytes);
        let text = self
            .st
            .text
            .entry((self.tag, a))
            .or_insert_with(|| current.clone());
        let lines = (shown as f32 / 16.0).ceil().clamp(2.0, 16.0);
        ui.input_text_multiline(
            "##hex",
            text,
            [-1.0, lines * ui.text_line_height_with_spacing()],
        )
        .read_only(!editable)
        .build();
        let done = ui.is_item_deactivated_after_edit();
        let active = ui.is_item_active();
        if n > DATA_SHOWN {
            ui.text_disabled(format!(
                "first {DATA_SHOWN} of {n} bytes shown; edit larger data elsewhere"
            ));
        }
        if done && editable {
            let typed = self.st.text.remove(&(self.tag, a)).unwrap_or_default();
            match parse_hex(&typed) {
                Some(new) if new.len() == bytes.len() => {
                    if new != bytes {
                        self.write(a, new, &f.name);
                        self.actions.push(Action::Tag(TagAction::Seal));
                    }
                }
                _ => {
                    self.st.problem = Some(format!(
                        "{}: type exactly {} hex bytes (data keeps its length)",
                        f.name,
                        bytes.len()
                    ))
                }
            }
        } else if !active {
            self.st.text.insert((self.tag, a), current);
        }
    }
}

/// Display formats (fixed here, so always valid).
fn fmt_f(s: &str) -> NumericFormat<'_, f32> {
    NumericFormat::new(s).expect("a valid float format")
}
/// Hex conversions are unsigned in C, so hex drags carry `u64`.
fn fmt_hex(s: &str) -> NumericFormat<'_, u64> {
    NumericFormat::new(s).expect("a valid hex format")
}

/// Bytes as space-separated hex.
pub fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Bytes as hex, 16 to a line.
pub fn hex_lines(b: &[u8]) -> String {
    b.chunks(16).map(hex).collect::<Vec<_>>().join("\n")
}

/// Hex digits (any whitespace between bytes) as bytes.
pub fn parse_hex(s: &str) -> Option<Vec<u8>> {
    let digits: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.len() % 2 != 0 {
        return None;
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_display_format_the_inspector_uses_is_valid() {
        super::fmt_hex("0x%08llx");
        for n in 1..=4 {
            super::fmt_hex(&format!("0x%0{}llx", 2 * n));
        }
        for f in ["%.2f", "%.4f"] {
            super::fmt_f(f);
        }
    }

    use super::*;

    #[test]
    fn unnamed_words_show_as_their_value_suggests() {
        assert_eq!(guess(0), View::Int);
        assert_eq!(guess(1.5f32.to_bits()), View::Float);
        assert_eq!(guess(7), View::Int);
        assert_eq!(guess((-3i32) as u32), View::Int);
        assert_eq!(guess(0x8123_4567), View::Hex);
    }

    #[test]
    fn show_as_is_kept_per_group_and_offset_and_saved_with_the_layout() {
        let weap = FourCC(*b"weap");
        assert_eq!(show_as_key(weap, "", 0x6), "weap:+0x6");
        assert_eq!(show_as_key(weap, "parts[1].", 0x4), "weap:parts+0x4");
        assert_eq!(
            show_as_key(weap, "parts[0].bones[12].", 0x4),
            show_as_key(weap, "parts[3].bones[0].", 0x4)
        );
        let mut settings = Settings {
            unnamed: true,
            ..Settings::default()
        };
        settings
            .show_as
            .insert(show_as_key(weap, "", 0x6), View::Hex);
        settings
            .show_as
            .insert(show_as_key(weap, "parts[0].", 0x8), View::Float);
        let ini = format!(
            "[Window][Tags]\nPos=0,0\n\n{}[Docking][Data]\n",
            settings.ini()
        );
        assert_eq!(Settings::from_ini(&ini), settings);
        assert_eq!(
            Settings::from_ini("[Window][Tags]\nPos=0,0\n"),
            Settings::default()
        );
        for view in View::ALL {
            assert_eq!(View::from_id(view.id()), Some(view));
        }

        let word = Field {
            offset: 0x6,
            name: "+0x6".into(),
            section: String::new(),
            named: false,
            kind: Kind::Unknown { len: 4 },
            meaning: String::new(),
            doc: None,
            applies: None,
            label: None,
            unit: None,
            axes: Vec::new(),
            view: None,
        };
        let key = show_as_key(weap, "", 0x6);
        assert_eq!(shown_as(&settings, &key, &word, 7), View::Hex);
        assert_eq!(
            shown_as(&Settings::default(), &key, &word, 1.5f32.to_bits()),
            View::Float
        );
        let half = Field {
            kind: Kind::Unknown { len: 2 },
            ..word
        };
        let decimal = Settings {
            show_as: [(key.clone(), View::Float)].into(),
            ..Settings::default()
        };
        assert_eq!(
            shown_as(&decimal, &key, &half, 7),
            View::Int,
            "decimal needs four bytes"
        );
    }

    /// Choices, references and Show as open the pick dialog: the panel
    /// draws no combo.
    #[test]
    fn the_panel_has_no_combos() {
        let combo = concat!("begin", "_combo");
        for (file, text) in [
            ("fields.rs", include_str!("fields.rs")),
            ("fields/picker.rs", include_str!("fields/picker.rs")),
            ("fields/row.rs", include_str!("fields/row.rs")),
        ] {
            assert!(!text.contains(combo), "{file} draws a combo");
        }
    }

    #[test]
    fn hex_round_trips() {
        let b: Vec<u8> = (0..40).collect();
        assert_eq!(parse_hex(&hex_lines(&b)).unwrap(), b);
        assert_eq!(parse_hex("0a 0"), None);
        assert_eq!(parse_hex("zz"), None);
    }
}
