//! The scenario inspector: the selection's fields, typed, edited together
//! when many placements are selected.
//!
//! - Position as x, y and z; rotation as yaw, pitch and roll in degrees;
//!   a facing in degrees; scale (0 reads as 1).
//! - The palette entry and the item reference as searchable combos.
//! - Flags fields as a checkbox per bit.
//! - A netgame flag's type by name where the community's enum names it
//!   (`h1convert::placements::H2Flag`, not yet confirmed from engine
//!   code), team 8 as neutral.
//! - Netgame equipment's spawn settings under their own heading.
//!
//! With many selected, only the fields every one has are shown; a field
//! whose values differ says so, and an edit writes only what was touched
//! (one component of a vector, one bit of flags) to each. Every edit is one
//! undo step; a drag is one step however long.
use std::hash::{Hash, Hasher};

use dear_imgui_rs::{Drag, Ui};
use scenario_edit::layout::{FieldDef, FieldType, Role};
use scenario_edit::session::FieldEdit;
use scenario_edit::{BlockKind, Scenario, TagRef, Value};

use crate::app::{Action, App};
use crate::theme::{self, icon};

/// Text typed into the searchable combos.
#[derive(Default)]
pub struct InspectorState {
    search: String,
}

/// Netgame flag types the community's enum names (see the module docs).
pub fn flag_type_name(v: i64) -> Option<&'static str> {
    Some(match v {
        0 => "CTF flag spawn",
        1 => "CTF flag return",
        4 => "Oddball spawn",
        7 => "Teleporter source",
        8 => "Teleporter destination",
        11..=16 => "Hill marker",
        _ => return None,
    })
}

/// A field's heading in the inspector.
pub fn section(kind: BlockKind, f: &FieldDef) -> &'static str {
    match f.role {
        Role::Position | Role::Rotation | Role::Facing | Role::Scale => "Transform",
        Role::PaletteIndex => "Object",
        Role::Plain if f.ty == FieldType::TagRef => "Object",
        Role::Plain if kind == BlockKind::NetgameEquipment => "Spawn",
        Role::Plain => "Settings",
    }
}

/// Degrees shown for an angle in radians, in (-180, 180].
pub fn degrees(rad: f32) -> f32 {
    let d = rad.to_degrees() % 360.0;
    if d > 180.0 {
        d - 360.0
    } else if d <= -180.0 {
        d + 360.0
    } else {
        d
    }
}

/// Whether a field is drawn as a checkbox per bit.
pub fn is_flags(f: &FieldDef) -> bool {
    f.ty == FieldType::U32 && f.name.contains("flags")
}

fn merge_key(selection: &[(BlockKind, usize)], field: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ("studio-inspector", selection, field).hash(&mut h);
    h.finish()
}

/// Draw the inspector for the selection and apply its edits.
pub fn draw(ui: &Ui, bold: dear_imgui_rs::FontId, app: &mut App, state: &mut InspectorState) {
    let selection = app.session.selection();
    let Some(&(kind, index)) = selection.first() else {
        ui.text_disabled("Select a placement in the outliner or the viewport.");
        ui.text_disabled("Ctrl/Cmd+click adds to the selection, Shift+click a run.");
        return;
    };
    let Some(scenario) = app.session.scenario() else {
        return;
    };
    let f = ui.push_font(bold);
    if selection.len() == 1 {
        ui.text(format!(
            "{}  {}",
            theme::block_icon(kind),
            scenario.label(kind, index)
        ));
    } else {
        ui.text(format!("{} {} placements", icon::LAYERS, selection.len()));
    }
    f.pop();
    let block = scenario.block(kind);
    if selection.len() == 1 {
        ui.text_disabled(format!(
            "{} #{index}   stride {:#x}",
            block.def.label, block.def.stride
        ));
    } else {
        let mut kinds: Vec<BlockKind> = selection.iter().map(|e| e.0).collect();
        kinds.dedup();
        let names: Vec<&str> = kinds.iter().map(|k| scenario.block(*k).def.label).collect();
        ui.text_disabled(format!(
            "{}; the fields they share, edited together",
            names.join(", ")
        ));
    }
    let named: usize = selection
        .iter()
        .map(|&(k, i)| scenario.names_of(k, i).len())
        .sum();
    if named > 0 {
        ui.text_colored(
            theme::WARN,
            format!("{} named by {named} scenario object name(s)", icon::ALERT),
        );
        ui.set_item_tooltip(
            "Scripts may create it by name; deleting it leaves the name naming nothing.",
        );
    }

    let fields = app.session.shared_fields();
    let mut edits: Vec<(&'static str, FieldEdit)> = Vec::new();
    let mut done = false;
    let mut heading = "";
    for f in fields {
        let values = app.session.selection_values(f.name);
        let Some(&first) = values.first() else {
            continue;
        };
        let mixed = values.iter().any(|v| *v != first);
        let h = section(kind, f);
        if h != heading {
            ui.separator_with_text(h);
            heading = h;
        }
        let d = ui.begin_disabled_with_cond(!f.editable);
        if let Some(e) = field_row(ui, scenario, kind, f, first, &values, mixed, state) {
            edits.push((f.name, e));
        }
        done |= ui.is_item_deactivated_after_edit();
        d.end();
    }
    if selection.len() == 1
        && let Some(p) = &block.def.palette
    {
        ui.separator();
        ui.text_disabled(format!(
            "palette: {} entries, stride {:#x}",
            block.palette.len(),
            p.stride
        ));
    }
    // Through the command layer, as the agent API's edits.
    for (field, edit) in edits {
        app.dispatch(Action::EditSelection {
            field: field.to_owned(),
            edit,
            merge: merge_key(&selection, field),
        });
    }
    if done {
        app.dispatch(Action::SealEdit);
    }
}

/// The label column's width.
const LABEL: f32 = 110.0;

fn label(ui: &Ui, f: &FieldDef, text: &str, mixed: bool) {
    ui.align_text_to_frame_padding();
    if mixed {
        ui.text_colored(theme::WARN, format!("{text} *"));
    } else {
        ui.text_disabled(text);
    }
    let note = if mixed {
        "\nThe selected placements differ here (the first one's value is shown)."
    } else {
        ""
    };
    ui.set_item_tooltip(format!(
        "{} at {:#x}  {}{note}",
        f.name, f.offset, f.evidence
    ));
    ui.same_line_with_pos(LABEL);
}

/// One field's row; returns the edit made.
#[allow(clippy::too_many_arguments)]
fn field_row(
    ui: &Ui,
    scenario: &Scenario,
    kind: BlockKind,
    f: &FieldDef,
    value: Value,
    values: &[Value],
    mixed: bool,
    state: &mut InspectorState,
) -> Option<FieldEdit> {
    let id = |s: &str| format!("##{}-{}", f.name, s);
    match (f.role, value) {
        (Role::Position, Value::Vec3(v)) => {
            vector_row(ui, f, values, mixed, ["x", "y", "z"], v, 0.05, |x| x, |x| x)
        }
        (Role::Rotation, Value::Vec3(v)) => vector_row(
            ui,
            f,
            values,
            mixed,
            ["yaw", "pitch", "roll"],
            v,
            0.5,
            degrees,
            f32::to_radians,
        ),
        (Role::Facing, Value::Float(x)) => {
            label(ui, f, "facing °", mixed);
            ui.set_next_item_width(-1.0);
            let mut d = degrees(x);
            drag_fmt(ui, id("facing"), 0.5, "%.1f°", &mut d)
                .then(|| FieldEdit::Set(Value::Float(d.to_radians())))
        }
        (Role::Scale, Value::Float(x)) => {
            label(ui, f, "scale", mixed);
            ui.set_next_item_width(-1.0);
            let mut s = if x == 0.0 { 1.0 } else { x };
            let r = Drag::new(id("scale"))
                .speed(0.01)
                .range(0.01, 100.0)
                .build(ui, &mut s)
                .then(|| FieldEdit::Set(Value::Float(s)));
            ui.set_item_tooltip("0 in the map reads as 1");
            r
        }
        (Role::PaletteIndex, Value::Int(x)) => {
            label(ui, f, "type", mixed);
            let palette = &scenario.block(kind).palette;
            let name = |i: i64| -> String {
                usize::try_from(i)
                    .ok()
                    .and_then(|i| palette.get(i))
                    .and_then(|r| scenario.tag_name(r.id))
                    .map(|n| format!("{i}: {}", short(n)))
                    .unwrap_or_else(|| if i < 0 { "none".into() } else { i.to_string() })
            };
            let options: Vec<(i64, String)> =
                (-1..palette.len() as i64).map(|i| (i, name(i))).collect();
            search_combo(ui, &id("palette"), &name(x), x, &options, state)
                .map(|i| FieldEdit::Set(Value::Int(i)))
        }
        (_, Value::Tag(r)) => {
            label(ui, f, f.name, mixed);
            let text = |r: TagRef| {
                if r.is_none() {
                    "none".to_owned()
                } else {
                    format!(
                        "{}: {}",
                        r.group,
                        scenario.tag_name(r.id).map_or("?", short)
                    )
                }
            };
            let mut options: Vec<(TagRef, String)> = scenario
                .tags
                .iter()
                .filter(|(_, t)| scenario_edit::layout::ITEM_GROUPS.contains(&t.group.0))
                .map(|(&id, t)| {
                    let o = TagRef { group: t.group, id };
                    (o, text(o))
                })
                .collect();
            options.sort_by(|a, b| a.1.cmp(&b.1));
            search_combo(ui, &id("tag"), &text(r), r, &options, state)
                .map(|t| FieldEdit::Set(Value::Tag(t)))
        }
        (_, Value::Int(x)) if is_flags(f) => {
            label(ui, f, f.name, mixed);
            ui.text_disabled(format!("{:#010x}", x as u32));
            flag_bits(ui, f, x, values)
        }
        (_, Value::Int(x)) if kind == BlockKind::NetgameFlags && f.name == "type" => {
            label(ui, f, "type", mixed);
            let text = |v: i64| match flag_type_name(v) {
                Some(n) => format!("{v}: {n}"),
                None => v.to_string(),
            };
            let options: Vec<(i64, String)> = (0..=16).map(|v| (v, text(v))).collect();
            let r = search_combo(ui, &id("flagtype"), &text(x), x, &options, state)
                .map(|v| FieldEdit::Set(Value::Int(v)));
            ui.set_item_tooltip(
                "Names from the community's enum for Halo 2's netgame flags \
                 (the disc agrees; not yet confirmed from engine code)",
            );
            r
        }
        (_, Value::Int(mut x)) => {
            let text = match (f.name, kind) {
                ("respawn time", _) => "respawn time (s)",
                ("team", BlockKind::NetgameFlags) => "team / channel",
                (n, _) => n,
            };
            label(ui, f, text, mixed);
            ui.set_next_item_width(-1.0);
            let (lo, hi) = match f.ty {
                FieldType::U8 => (0i64, 0xff),
                FieldType::I16 => (-0x8000, 0x7fff),
                FieldType::U16 => (0, 0xffff),
                _ => (0, 0xffff_ffff),
            };
            let r = Drag::<i64, _>::new(id("int"))
                .range(lo, hi)
                .speed(0.1)
                .build(ui, &mut x)
                .then_some(FieldEdit::Set(Value::Int(x)));
            match (f.name, kind) {
                ("respawn time", _) => ui.set_item_tooltip("0: the collection's default"),
                ("team", BlockKind::NetgameFlags) => ui.set_item_tooltip(
                    "The team (8 is neutral); a teleporter's channel; a hill's number",
                ),
                _ => {}
            }
            r
        }
        (_, Value::Float(mut x)) => {
            label(ui, f, f.name, mixed);
            ui.set_next_item_width(-1.0);
            Drag::new(id("float"))
                .speed(0.01)
                .build(ui, &mut x)
                .then_some(FieldEdit::Set(Value::Float(x)))
        }
        (_, Value::Vec3(v)) => {
            vector_row(ui, f, values, mixed, ["x", "y", "z"], v, 0.05, |x| x, |x| x)
        }
    }
}

/// A float drag showing `fmt` (a C format), or plain if ImGui refuses it.
fn drag_fmt(ui: &Ui, label: String, speed: f32, fmt: &str, value: &mut f32) -> bool {
    match Drag::new(label.clone())
        .speed(speed)
        .try_display_format(fmt)
    {
        Ok(d) => d.build(ui, value),
        Err(_) => Drag::new(label).speed(speed).build(ui, value),
    }
}

/// A tag name's last part (`objects\weapons\rifle\battle_rifle` →
/// `battle_rifle`), as the outliner labels entries.
fn short(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

/// Three labelled drags for a vector field; an edit changes only the
/// component dragged. `show` and `store` convert to and from what is shown
/// (degrees for rotations).
#[allow(clippy::too_many_arguments)]
fn vector_row(
    ui: &Ui,
    f: &FieldDef,
    values: &[Value],
    mixed: bool,
    names: [&str; 3],
    v: [f32; 3],
    speed: f32,
    show: impl Fn(f32) -> f32,
    store: impl Fn(f32) -> f32,
) -> Option<FieldEdit> {
    let text = if f.role == Role::Rotation {
        format!("{} °", f.name)
    } else {
        f.name.to_owned()
    };
    label(ui, f, &text, mixed);
    let avail = ui.content_region_avail()[0];
    let w = ((avail - 2.0 * 4.0) / 3.0).max(30.0);
    let mut out = None;
    for k in 0..3 {
        if k > 0 {
            ui.same_line_with_spacing(0.0, 4.0);
        }
        let differs = values
            .iter()
            .any(|x| x.as_vec3().is_some_and(|x| x[k] != v[k]));
        ui.set_next_item_width(w);
        let mut x = show(v[k]);
        let fmt = format!("{}{} %.2f", names[k], if differs { "*" } else { "" });
        if drag_fmt(ui, format!("##{}-{k}", f.name), speed, &fmt, &mut x) {
            out = Some(FieldEdit::Component(k, store(x)));
        }
    }
    out
}

/// A checkbox per bit, eight to a row; bits set on some of the selection
/// show as mixed.
fn flag_bits(ui: &Ui, f: &FieldDef, first: i64, values: &[Value]) -> Option<FieldEdit> {
    let mut out = None;
    for bit in 0..32u32 {
        if bit % 8 != 0 {
            ui.same_line();
        } else {
            ui.dummy([LABEL - 8.0, 0.0]);
            ui.same_line_with_pos(LABEL);
        }
        let mask = 1i64 << bit;
        let mut on = first & mask != 0;
        let mixed = values
            .iter()
            .any(|v| v.as_int().is_some_and(|x| (x & mask != 0) != on));
        let c = mixed.then(|| ui.push_style_color(dear_imgui_rs::StyleColor::FrameBg, theme::WARN));
        if ui.checkbox(format!("##{}-bit{bit}", f.name), &mut on) {
            out = Some(FieldEdit::Bit(bit, on));
        }
        if let Some(c) = c {
            c.pop();
        }
        ui.set_item_tooltip(format!(
            "bit {bit}{}",
            if mixed { " (differs)" } else { "" }
        ));
    }
    out
}

/// A combo whose popup starts with a search box: only options whose text
/// has every typed word are listed.
fn search_combo<T: Copy + PartialEq>(
    ui: &Ui,
    id: &str,
    preview: &str,
    current: T,
    options: &[(T, String)],
    state: &mut InspectorState,
) -> Option<T> {
    ui.set_next_item_width(-1.0);
    let mut chosen = None;
    if let Some(_c) = ui.begin_combo(id, preview) {
        if ui.is_window_appearing() {
            state.search.clear();
            ui.set_keyboard_focus_here();
        }
        ui.set_next_item_width(-1.0);
        ui.input_text(format!("{id}-search"), &mut state.search)
            .hint(format!("{} Search", icon::SEARCH))
            .build();
        for (value, text) in options
            .iter()
            .filter(|(_, t)| crate::ui::outliner::matches(&state.search, "", t, usize::MAX))
        {
            if ui
                .selectable_config(text)
                .selected(*value == current)
                .build()
            {
                chosen = Some(*value);
            }
        }
    }
    chosen.filter(|v| *v != current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenario_edit::layout;

    #[test]
    fn angles_show_between_minus_and_plus_half_a_turn() {
        assert!((degrees(std::f32::consts::FRAC_PI_2) - 90.0).abs() < 1e-4);
        assert!((degrees(3.0 * std::f32::consts::FRAC_PI_2) + 90.0).abs() < 1e-3);
        assert!((degrees(-std::f32::consts::PI) - 180.0).abs() < 1e-3);
        assert!((degrees(4.0 * std::f32::consts::PI)).abs() < 1e-3);
    }

    #[test]
    fn fields_fall_under_their_headings() {
        let flags = layout::block(BlockKind::NetgameEquipment);
        let heading = |k: BlockKind, name: &str| section(k, layout::block(k).field(name).unwrap());
        assert_eq!(heading(BlockKind::Weapons, "position"), "Transform");
        assert_eq!(heading(BlockKind::Weapons, "rotation"), "Transform");
        assert_eq!(heading(BlockKind::Weapons, "scale"), "Transform");
        assert_eq!(heading(BlockKind::Weapons, "palette index"), "Object");
        assert_eq!(heading(BlockKind::NetgameEquipment, "item"), "Object");
        assert_eq!(
            heading(BlockKind::NetgameEquipment, "respawn time"),
            "Spawn"
        );
        assert_eq!(heading(BlockKind::StartingLocations, "team"), "Settings");
        assert!(is_flags(flags.field("flags").unwrap()));
        assert!(is_flags(
            layout::block(BlockKind::Weapons)
                .field("placement flags")
                .unwrap()
        ));
        assert!(!is_flags(flags.field("respawn time").unwrap()));
    }

    #[test]
    fn netgame_flag_types_are_named_where_the_enum_names_them() {
        assert_eq!(flag_type_name(0), Some("CTF flag spawn"));
        assert_eq!(flag_type_name(4), Some("Oddball spawn"));
        assert_eq!(flag_type_name(12), Some("Hill marker"));
        assert_eq!(flag_type_name(2), None);
    }

    #[test]
    fn merge_keys_follow_the_selection_and_the_field() {
        let a = [(BlockKind::Weapons, 0)];
        let b = [(BlockKind::Weapons, 0), (BlockKind::Weapons, 1)];
        assert_eq!(merge_key(&a, "position"), merge_key(&a, "position"));
        assert_ne!(merge_key(&a, "position"), merge_key(&b, "position"));
        assert_ne!(merge_key(&a, "position"), merge_key(&a, "rotation"));
    }
}
