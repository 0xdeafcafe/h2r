//! The field panel's pick dialog: one modal for a choice (an enum's named
//! values and **Enter a number...**), a tag reference (**No tag**, then
//! every tag of the allowed kinds, with a `Kind` row when several are
//! allowed) and an unnamed value's `Show as`. A search box narrows the
//! list by words; the list is never capped (it is clipped as it scrolls).
//!
//! The lists come from the functions here, which `tag.choices` returns
//! too, so the agent API sees the dialog's list.
use ::tags::registry::{Field, Kind};
use dear_imgui_rs::{Key, ListClipper, Ui, WindowFlags};
use map::{FourCC, TagId};

use super::{PanelState, View, le, row, write_action};
use crate::app::tag_doc::TagEntry;
use crate::app::{Action, TagAction, TagDoc, tag_tree};
use crate::theme;

/// What the dialog picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    /// An enum's value.
    Choice,
    /// A tag reference's tag.
    Reference,
    /// The kind a reference's list shows (the second picker).
    Kind,
    /// How an unnamed value is shown.
    ShowAs,
}

/// The open dialog.
#[derive(Clone, Debug)]
pub struct Open {
    pub tag: TagId,
    /// The field's address.
    pub address: u32,
    pub field: Field,
    /// The row's label, the dialog's title.
    pub label: String,
    /// The field's `Show as` key ([`super::show_as_key`]).
    pub key: String,
    pub what: What,
    pub query: String,
    /// The kind a reference's list is narrowed to.
    pub kind: Option<FourCC>,
    /// The number being typed, once **Enter a number...** is pressed.
    pub number: Option<String>,
    /// Why the typed number was not taken.
    pub problem: Option<String>,
    opening: bool,
    focus_number: bool,
}

impl Open {
    pub fn new(
        tag: TagId,
        address: u32,
        field: &Field,
        label: &str,
        key: String,
        what: What,
    ) -> Self {
        Self {
            tag,
            address,
            field: field.clone(),
            label: label.to_owned(),
            key,
            what,
            query: String::new(),
            kind: None,
            number: None,
            problem: None,
            opening: true,
            focus_number: false,
        }
    }
}

/// An enum value the dialog lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub value: u32,
    /// Its name in words.
    pub name: String,
}

/// A bit of a switches field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bit {
    pub bit: u32,
    /// Its definition name, if one names it.
    pub name: Option<String>,
    /// Its name in words, or `Bit 7`.
    pub label: String,
}

/// Whether every word of `query` is in `text` (case-insensitive). An
/// empty query matches all.
fn hit(text: &str, query: &str) -> bool {
    let text = text.to_ascii_lowercase();
    query
        .split_whitespace()
        .all(|w| text.contains(&w.to_ascii_lowercase()))
}

/// An enum's named values matching `query` (by name or number), in value
/// order.
pub fn values(names: &[String], query: &str) -> Vec<Choice> {
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| row::is_name(n))
        .map(|(i, n)| Choice {
            value: i as u32,
            name: row::words(n),
        })
        .filter(|c| hit(&format!("{} {}", c.name, c.value), query))
        .collect()
}

/// Every bit of a switches field `width` bits wide.
pub fn bits(names: &[String], width: u32) -> Vec<Bit> {
    (0..width)
        .map(|bit| Bit {
            bit,
            name: names.get(bit as usize).filter(|n| row::is_name(n)).cloned(),
            label: row::bit_text(names, bit),
        })
        .collect()
}

/// The kinds of the map's tags a reference allowing `groups` takes, by
/// their code.
pub fn kinds(doc: &TagDoc, groups: &[FourCC]) -> Vec<FourCC> {
    let mut out: Vec<FourCC> = Vec::new();
    for t in doc.tags() {
        if doc.fits(t.group, groups) && !out.contains(&t.group) {
            out.push(t.group);
        }
    }
    out.sort_by_key(|g| ::tags::defs::fourcc_text(*g));
    out
}

/// The map's tags a reference allowing `groups` takes, of `kind` when one
/// is given, matching `query` by words of `group:name`, in the map's
/// order. Every one: the list has no cap.
pub fn tags<'d>(
    doc: &'d TagDoc,
    groups: &[FourCC],
    kind: Option<FourCC>,
    query: &str,
) -> Vec<&'d TagEntry> {
    doc.tags()
        .iter()
        .filter(|t| doc.fits(t.group, groups))
        .filter(|t| kind.is_none_or(|k| t.group == k))
        .filter(|t| tag_tree::matches(t, query))
        .collect()
}

/// The views an unnamed value of `len` bytes can be shown as (decimal
/// needs four).
pub fn views(len: u32) -> Vec<View> {
    View::ALL
        .into_iter()
        .filter(|v| *v != View::Float || len == 4)
        .collect()
}

/// A typed number (decimal, or hex after `0x`) that a field of `bytes`
/// bytes holds.
pub fn number(text: &str, bytes: usize) -> Result<u64, String> {
    let max = match bytes {
        8.. => u64::MAX,
        n => (1u64 << (8 * n)) - 1,
    };
    let t = text.trim();
    let parsed = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => t.parse().ok(),
    };
    match parsed {
        Some(v) if v <= max => Ok(v),
        _ => Err(format!("Type a whole number from 0 to {max}.")),
    }
}

/// The list's width, in points.
const WIDTH: f32 = 420.0;
/// Rows the list shows before it scrolls.
const ROWS: usize = 14;

/// Draw the open dialog (call once per panel, after its rows, so it opens
/// at the panel's ID level). A pick is pushed as a write and sealed.
pub(super) fn draw(ui: &Ui, doc: &TagDoc, st: &mut PanelState, actions: &mut Vec<Action>) {
    let Some(mut open) = st.pick.take() else {
        st.listed.clear();
        return;
    };
    let opening = std::mem::take(&mut open.opening);
    let title = format!("{}###field-picker", open.label);
    if opening {
        ui.open_popup(&title);
    }
    let Some(_modal) = ui
        .begin_modal_popup_config(&title)
        .flags(WindowFlags::ALWAYS_AUTO_RESIZE | WindowFlags::NO_SAVED_SETTINGS)
        .begin()
    else {
        st.listed.clear();
        return;
    };
    let mut done = ui.is_key_pressed(Key::Escape);
    let now = doc
        .read(open.tag, open.address, open.field.kind.width())
        .unwrap_or_default();
    match (open.what, open.field.kind.clone()) {
        (
            What::Choice,
            Kind::Enum {
                bytes,
                values: names,
            },
        ) => {
            done |= choice(
                ui,
                &mut open,
                &names,
                usize::from(bytes),
                &now,
                st,
                actions,
                opening,
            );
        }
        (What::Reference, Kind::TagRef { groups } | Kind::Datum { groups }) => {
            done |= reference(ui, doc, &mut open, &groups, &now, st, actions, opening);
        }
        (What::Kind, Kind::TagRef { groups } | Kind::Datum { groups }) => {
            let all = kinds(doc, &groups);
            st.listed = all.iter().map(|g| row::kind_words(doc, *g)).collect();
            if ui
                .selectable_config("All kinds##kind-all")
                .selected(open.kind.is_none())
                .build()
            {
                open.kind = None;
                open.what = What::Reference;
            }
            for (i, g) in all.iter().enumerate() {
                let label = format!("{}##kind-{i}", st.listed[i]);
                if ui
                    .selectable_config(label)
                    .selected(open.kind == Some(*g))
                    .build()
                {
                    open.kind = Some(*g);
                    open.what = What::Reference;
                }
            }
            ui.separator();
            if ui.button("Cancel##kind-cancel") {
                open.what = What::Reference;
            }
        }
        (What::ShowAs, Kind::Unknown { len }) => {
            let v = le(&now, 0, now.len().min(4)) as u32;
            let shown = super::shown_as(&st.settings, &open.key, &open.field, v);
            let list = views(len);
            st.listed = list.iter().map(|v| v.words().to_owned()).collect();
            ui.text_disabled("Kept for this field of every tag of the kind.");
            for to in list {
                let label = format!("{}##pick-{}", to.words(), to.id());
                if ui.selectable_config(label).selected(shown == to).build() {
                    let mut settings = st.settings.clone();
                    settings.show_as.insert(open.key.clone(), to);
                    st.set_settings(settings);
                    done = true;
                }
            }
            ui.separator();
            done |= ui.button("Cancel##pick-cancel");
        }
        _ => done = true,
    }
    if done {
        ui.close_current_popup();
        st.listed.clear();
    } else {
        st.pick = Some(open);
    }
}

/// The search box, focused as the dialog opens.
fn search(ui: &Ui, open: &mut Open, opening: bool) {
    if opening {
        ui.set_keyboard_focus_here();
    }
    ui.set_next_item_width(WIDTH);
    ui.input_text("##pick-search", &mut open.query)
        .hint("Search")
        .build();
}

/// The write of a pick, sealed as one undo step.
fn pick(open: &Open, bytes: Vec<u8>, actions: &mut Vec<Action>) {
    actions.push(write_action(
        open.tag,
        open.address,
        bytes,
        &open.field.name,
    ));
    actions.push(Action::Tag(TagAction::Seal));
}

/// An enum's dialog; true when it is done.
#[allow(clippy::too_many_arguments)]
fn choice(
    ui: &Ui,
    open: &mut Open,
    names: &[String],
    n: usize,
    now: &[u8],
    st: &mut PanelState,
    actions: &mut Vec<Action>,
    opening: bool,
) -> bool {
    let current = le(now, 0, n.min(now.len())) as u32;
    if !names.get(current as usize).is_some_and(|s| row::is_name(s)) {
        ui.text_disabled(format!("Now: {}", row::choice_text(names, current)));
    }
    search(ui, open, opening);
    let list = values(names, &open.query);
    st.listed = list.iter().map(|c| c.name.clone()).collect();
    let mut chosen = None;
    let height = ui.text_line_height_with_spacing();
    theme::bounded_list(ui, "##pick-list", WIDTH, height, list.len(), ROWS, || {
        if list.is_empty() {
            ui.text_disabled("No named value matches.");
        }
        for i in ListClipper::new(list.len())
            .items_height(height)
            .begin(ui)
            .iter()
        {
            let c = &list[i];
            let label = format!("{}##pick-{}", c.name, c.value);
            if ui
                .selectable_config(label)
                .selected(c.value == current)
                .build()
            {
                chosen = Some(u64::from(c.value));
            }
        }
    });
    match open.number.as_mut() {
        None => {
            if ui.button("Enter a number...##pick-number") {
                open.number = Some(current.to_string());
                open.focus_number = true;
            }
        }
        Some(text) => {
            if std::mem::take(&mut open.focus_number) {
                ui.set_keyboard_focus_here();
            }
            ui.set_next_item_width(WIDTH * 0.5);
            let entered = ui
                .input_text("##pick-number-text", text)
                .enter_returns_true(true)
                .build();
            ui.same_line();
            if entered | ui.button("Set##pick-number-set") {
                match number(text, n) {
                    Ok(v) => chosen = Some(v),
                    Err(why) => open.problem = Some(why),
                }
            }
        }
    }
    if let Some(why) = &open.problem {
        ui.text_colored(theme::WARN, why);
    }
    ui.separator();
    let cancel = ui.button("Cancel##pick-cancel");
    if let Some(v) = chosen {
        pick(open, v.to_le_bytes()[..n].to_vec(), actions);
        return true;
    }
    cancel
}

/// A tag reference's dialog; true when it is done.
#[allow(clippy::too_many_arguments)]
fn reference(
    ui: &Ui,
    doc: &TagDoc,
    open: &mut Open,
    groups: &[FourCC],
    now: &[u8],
    st: &mut PanelState,
    actions: &mut Vec<Action>,
    opening: bool,
) -> bool {
    let tagref = matches!(open.field.kind, Kind::TagRef { .. });
    let (group, current) = match (tagref, now.len()) {
        (true, 8..) => (le(now, 0, 4) as u32, le(now, 4, 4) as u32),
        (false, 4..) => (u32::MAX, le(now, 0, 4) as u32),
        _ => (u32::MAX, u32::MAX),
    };
    let all = kinds(doc, groups);
    if all.len() > 1 {
        ui.align_text_to_frame_padding();
        ui.text("Kind");
        ui.same_line_with_pos(80.0);
        ui.text(
            open.kind
                .map_or_else(|| "All kinds".to_owned(), |g| row::kind_words(doc, g)),
        );
        ui.same_line();
        if ui.small_button("Change...##kind") {
            open.what = What::Kind;
        }
    }
    search(ui, open, opening);
    let list = tags(doc, groups, open.kind, &open.query);
    st.listed = list.iter().map(|t| t.name.clone()).collect();
    let mut chosen: Option<(Option<FourCC>, u32)> = None;
    let height = ui.text_line_height_with_spacing();
    let several = all.len() > 1;
    theme::bounded_list(
        ui,
        "##pick-list",
        WIDTH,
        height,
        list.len() + 1,
        ROWS,
        || {
            if ui
                .selectable_config("No tag##pick-none")
                .selected(current == u32::MAX)
                .build()
            {
                chosen = Some((None, u32::MAX));
            }
            for i in ListClipper::new(list.len())
                .items_height(height)
                .begin(ui)
                .iter()
            {
                let t = list[i];
                let label = format!("{}##pick-t{i}", t.name);
                if ui
                    .selectable_config(label)
                    .selected(t.id.0 == current)
                    .build()
                {
                    chosen = Some((Some(t.group), t.id.0));
                }
                if several {
                    ui.same_line_with_pos(WIDTH - 120.0);
                    ui.text_disabled(row::kind_words(doc, t.group));
                }
            }
        },
    );
    ui.separator();
    let cancel = ui.button("Cancel##pick-cancel");
    let Some((g, id)) = chosen else {
        return cancel;
    };
    let bytes = if tagref {
        // No tag keeps the group the field had.
        let g = g.map_or(group, FourCC::as_u32);
        let mut out = g.to_le_bytes().to_vec();
        out.extend_from_slice(&id.to_le_bytes());
        out
    } else {
        id.to_le_bytes().to_vec()
    };
    pick(open, bytes, actions);
    true
}

#[cfg(test)]
mod tests;
