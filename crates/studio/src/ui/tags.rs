//! The Tags tool's windows: the open map's tags by group with a search in
//! the outliner, the selected tag's fields ([`crate::fields::panel`]) in
//! the centre, and the raw bytes of the element under the mouse with the
//! hovered field lit in the inspector. State lives in
//! [`crate::app::TagDoc`]; everything goes back as [`Action`]s.
use std::collections::BTreeMap;

use dear_imgui_rs::{Condition, Ui};
use map::{FourCC, TagId};

use crate::app::tag_doc::Access;
use crate::app::{Action, App, TagAction};
use crate::fields::{self, PanelState};
use crate::theme::{self, Fonts, icon};

/// Tags a group lists at most (type to narrow).
const LISTED: usize = 600;

/// The tool's UI-only state.
#[derive(Default)]
pub struct TagsUi {
    pub filter: String,
    /// The filter the tree's open state was last set for.
    opened_for: Option<String>,
    pub panel: PanelState,
}

fn group_text(g: FourCC) -> String {
    tags::defs::fourcc_text(g)
}

/// The outliner: groups, then their tags, filtered by `group` or name.
pub fn tree(ui: &Ui, app: &App, st: &mut TagsUi, fonts: &Fonts, actions: &mut Vec<Action>) {
    let doc = &app.tags;
    if !doc.is_open() {
        ui.text_wrapped(match &doc.error {
            Some(e) => format!("The map's tags could not be read: {e}"),
            None => "Open a map (Tools, Maps) to edit its tags.".to_owned(),
        });
        return;
    }
    ui.set_next_item_width(-1.0);
    ui.input_text("##tag-filter", &mut st.filter)
        .hint(format!("{} group or name", icon::SEARCH))
        .build();
    let filter = st.filter.to_ascii_lowercase();
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, t) in doc.tags().iter().enumerate() {
        let g = group_text(t.group);
        if filter.is_empty()
            || g.to_ascii_lowercase().contains(&filter)
            || t.name.to_ascii_lowercase().contains(&filter)
        {
            groups.entry(g).or_default().push(i);
        }
    }
    let refilter = st.opened_for.as_deref() != Some(filter.as_str());
    st.opened_for = Some(filter.clone());
    let selected = doc.selected;
    let sel_group = selected
        .and_then(|id| doc.tag(id))
        .map(|t| group_text(t.group));
    let f = ui.push_font(fonts.bold);
    ui.text(format!("{} groups", groups.len()));
    f.pop();
    for (g, list) in &groups {
        let mut node = ui
            .tree_node_config(format!("grp-{g}"))
            .label(format!("{g}  ({})", list.len()))
            .default_open(sel_group.as_deref() == Some(g.as_str()))
            .span_avail_width(true);
        if refilter && !filter.is_empty() {
            node = node.opened(list.len() <= 50, Condition::Always);
        }
        let Some(_n) = node.push() else { continue };
        for &i in list.iter().take(LISTED) {
            let t = &doc.tags()[i];
            let mark = if doc.is_changed(t.id) { "  *" } else { "" };
            let lock = match t.access {
                Access::Edit => "",
                _ => "  (read-only)",
            };
            if ui
                .selectable_config(format!("{}{mark}{lock}##tag-{:x}", t.name, t.id.0))
                .selected(selected == Some(t.id))
                .build()
            {
                actions.push(Action::Tag(TagAction::Select(t.id)));
            }
        }
        if list.len() > LISTED {
            ui.text_disabled(format!("{} more: type to narrow", list.len() - LISTED));
        }
    }
}

/// The selected tag's fields.
pub fn fields(ui: &Ui, app: &App, st: &mut TagsUi, fonts: &Fonts, actions: &mut Vec<Action>) {
    let doc = &app.tags;
    let Some(id) = doc.selected else {
        ui.text_wrapped(
            "Pick a tag on the left. Every field is shown: named ones by their name, the \
             rest by their offset and the type their bytes suggest. Edits save with the map \
             (File, Save map as / Save as package).",
        );
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
            return;
        }
    }
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

/// The raw bytes of the element under the mouse, the hovered field lit.
pub fn hex(ui: &Ui, app: &App, st: &TagsUi, fonts: &Fonts) {
    let doc = &app.tags;
    let Some(id) = doc.selected else {
        ui.text_disabled("No tag selected.");
        return;
    };
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
    app.tags.selected
}
