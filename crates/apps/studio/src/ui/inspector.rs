//! The scenario inspector: the selection's fields, typed, edited together
//! when many placements are selected.
//!
//! - Position as x, y and z; rotation as yaw, pitch and roll in degrees;
//!   a facing in degrees; scale (0 reads as 1).
//! - The palette entry and the item reference as searchable combos.
//! - Flags fields as a checkbox per bit.
//! - A netgame flag's type by its viewport label or raw value, and its team.
//! - Netgame equipment's spawn settings under their own heading.
//!
//! With many selected, only the fields every one has are shown; a field
//! whose values differ says so, and an edit writes only what was touched
//! (one component of a vector, one bit of flags) to each. Every edit is one
//! undo step; a drag is one step however long.
mod area;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use dear_imgui_rs::{Drag, Ui};
use map::TagId;
use scenario_edit::geometry::flag_type_name;
use scenario_edit::layout::{FieldDef, FieldType, Role};
use scenario_edit::session::FieldEdit;
use scenario_edit::{BlockKind, Scenario, TagRef, Value};

use crate::app::tag_api::{self, Chain, Node};
use crate::app::tag_doc::Access;
use crate::app::{Action, App, Doc, NavAction, NavLocation, Selection, TagDoc};
use crate::fields;
use crate::play::issues::{Issue, IssueIndex, Kind};
use crate::play::pair::Request;
use crate::play::{Apply, LinkTone};
use crate::theme::{self, icon};

/// Text typed into the searchable combos.
#[derive(Default)]
pub struct InspectorState {
    search: String,
    tag_panel: fields::PanelState,
    chain: Option<(u64, TagId, &'static str, Chain)>,
    tag_world: Option<u64>,
    focus: Option<(TagId, String)>,
    /// The where-used answer per tag, as computed for the document's
    /// revision and the scenario's block layout: what the chain's node
    /// headers count and the Where this is used lists draw. A palette
    /// field edit changes the answer without changing either; `tag.uses`
    /// always reads the live one.
    used: Option<(u64, u64, HashMap<TagId, tag_api::Uses>)>,
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

pub(crate) fn issues(app: &App) -> &IssueIndex {
    app.pairing
        .link
        .as_ref()
        .map_or(IssueIndex::empty(), |link| link.issues.as_ref())
}

fn field_mark(kind: Kind) -> Option<(LinkTone, &'static str)> {
    match kind {
        Kind::Refused => Some((LinkTone::Error, "Refused")),
        Kind::Mismatch => Some((LinkTone::Warn, "Changed in game")),
        _ => None,
    }
}

pub(super) fn marked_row<T>(
    ui: &Ui,
    label: String,
    issue: Option<&Issue>,
    draw: impl FnOnce(String) -> T,
) -> T {
    if let Some(issue) = issue
        && let Some((tone, mark)) = field_mark(issue.kind)
    {
        let _colour =
            ui.push_style_color(dear_imgui_rs::StyleColor::Text, theme::link_colour(tone));
        let row = draw(format!("{label}  [{mark}]"));
        ui.set_item_tooltip(&issue.text);
        row
    } else {
        draw(label)
    }
}

fn field_problem(ui: &Ui, issue: &Issue, resolution: &mut Option<Apply>) {
    let Some((tone, mark)) = field_mark(issue.kind) else {
        return;
    };
    theme::link_badge(ui, tone, mark);
    ui.text_wrapped(&issue.text);
    if issue.kind == Kind::Mismatch && let Some(at) = issue.at {
        let _id = ui.push_id(at as usize);
        if ui.small_button("Take the game's value") {
            *resolution = Some(Apply::Take(at));
        }
        ui.same_line();
        if ui.small_button("Overwrite") {
            *resolution = Some(Apply::Overwrite(at));
        }
    }
}

pub(super) fn tag_field_marks<'a>(
    ui: &'a Ui,
    issues: &'a IssueIndex,
    resolution: &'a mut Option<Apply>,
) -> impl FnMut(TagId, u32) + 'a {
    move |tag, at| {
        for issue in issues.tag_field(tag, at) {
            field_problem(ui, issue, resolution);
        }
    }
}

/// The selected trigger volume: its frame, size and what names it, read
/// only.
fn volume(ui: &Ui, fonts: &theme::Fonts, app: &mut App, index: usize) {
    use scenario_edit::volumes::{Holder, VolumeKind};
    let Some(volumes) = app.session.volumes() else {
        return;
    };
    let v = volumes.triggers[index].clone();
    let f = ui.push_font(fonts.title);
    ui.text(format!("{}  {}", icon::BOX, volumes.label(index)));
    f.pop();
    let kinds: Vec<&str> = volumes
        .kinds(index)
        .into_iter()
        .map(|k| scenario_edit::layers::LayerId::Volume(k).label())
        .collect();
    ui.text_disabled(format!("Trigger volume #{index}   {}", kinds.join(", ")));
    let holders = volumes.holders(index);
    let mut actions = Vec::new();
    if v.in_world() {
        if ui.button(format!("{} Frame", icon::FOCUS)) {
            actions.push(Action::FocusSelected);
        }
        ui.same_line();
    } else {
        ui.text_wrapped(
            "Its frame is relative to a placed object, which only a running game places, \
             so it is not drawn.",
        );
    }
    let refusal = app
        .session
        .volume_remove_refusal(VolumeKind::Trigger, index);
    let d = ui.begin_disabled_with_cond(refusal.is_some());
    if ui.button(format!("{} Delete", icon::TRASH)) {
        actions.push(Action::RemoveVolume(VolumeKind::Trigger, index));
    }
    d.end();
    if let Some(why) = &refusal {
        ui.set_item_tooltip(why);
    }
    let locked = app
        .session
        .scenario()
        .and_then(|s| s.volume_refusal(VolumeKind::Trigger));

    theme::section(ui, "Box");
    let d = ui.begin_disabled_with_cond(locked.is_some() || !v.in_world());
    let (edited, done) = volume_box(ui, &v.frame);
    d.end();
    if let Some(why) = &locked {
        ui.text_disabled(why);
    }
    let xyz = |p: [f32; 3]| format!("{:.3}, {:.3}, {:.3}", p[0], p[1], p[2]);
    for (label, value) in [
        ("Origin (corner)", xyz(v.frame.origin)),
        ("Forward", xyz(v.frame.forward)),
        ("Up", xyz(v.frame.up)),
    ] {
        ui.label_text(label, value);
    }
    if let Some(parent) = v.parent {
        ui.label_text("Parent object name", parent.to_string());
    }
    theme::section(ui, "Named by");
    if holders.is_empty() {
        ui.text_disabled("Nothing names it by index.");
    }
    for h in holders {
        ui.bullet_text(match h {
            Holder::Kill(i) => format!("Kill trigger #{i}"),
            Holder::BspSwitch { index, from, to } => {
                format!("BSP switch #{index}, BSP {from} to {to}")
            }
        });
        if let Holder::Kill(i) = h {
            ui.same_line();
            if ui.small_button(format!("Remove##kill-{i}")) {
                actions.push(Action::RemoveVolume(VolumeKind::Kill, i));
            }
            ui.set_item_tooltip("Remove the kill trigger; the volume stays a plain trigger");
        }
    }
    // Through the command layer, as the agent API's edits.
    if let Some(frame) = edited {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ("studio-inspector-volume", index).hash(&mut h);
        app.dispatch(Action::SetVolumeFrame {
            index,
            frame,
            merge: Some(h.finish()),
        });
    }
    if done {
        app.dispatch(Action::SealEdit);
    }
    for action in actions {
        app.dispatch(action);
    }
}

/// The box's centre, size and (when it stands upright) its facing, as
/// drags: the new box when one changed, and whether a drag ended.
fn volume_box(
    ui: &Ui,
    frame: &scenario_edit::volumes::Frame,
) -> (Option<scenario_edit::volumes::Frame>, bool) {
    use scenario_edit::volumes::Frame;
    let (Some([forward, _, up]), Some(mut centre)) = (frame.axes(), frame.center()) else {
        return (None, false);
    };
    let mut size = frame.size;
    let mut done = false;
    let mut row = |name: &str, v: &mut [f32; 3]| {
        ui.align_text_to_frame_padding();
        ui.text_disabled(name);
        ui.same_line_with_pos(LABEL);
        let avail = ui.content_region_avail()[0];
        let w = ((avail - 2.0 * 4.0) / 3.0).max(30.0);
        let mut changed = false;
        for (k, axis) in ["x", "y", "z"].into_iter().enumerate() {
            if k > 0 {
                ui.same_line_with_spacing(0.0, 4.0);
            }
            ui.set_next_item_width(w);
            let fmt = format!("{axis} %.2f");
            changed |= drag_fmt(ui, format!("##volume-{name}-{k}"), 0.05, &fmt, &mut v[k]);
            done |= ui.is_item_deactivated_after_edit();
        }
        changed
    };
    let mut out = None;
    if row("centre", &mut centre) {
        out = Frame::around(centre, forward, up, size);
    }
    if row("size", &mut size) {
        size = size.map(|x| x.max(0.01));
        out = Frame::around(centre, forward, up, size);
    }
    // An upright box turns by its facing here; any box turns with the
    // gizmo.
    if (up[2] - 1.0).abs() < 1e-4 {
        ui.align_text_to_frame_padding();
        ui.text_disabled("facing °");
        ui.same_line_with_pos(LABEL);
        ui.set_next_item_width(-1.0);
        let mut yaw = forward[1].atan2(forward[0]).to_degrees();
        if drag_fmt(ui, "##volume-facing".into(), 0.5, "%.1f", &mut yaw) {
            let (s, c) = yaw.to_radians().sin_cos();
            out = Frame::around(centre, [c, s, 0.0], up, size);
        }
        done |= ui.is_item_deactivated_after_edit();
    }
    (out, done)
}

/// The selected level collision surface, read only: its class, plane,
/// raw flags, material and ring.
fn surface(ui: &Ui, fonts: &theme::Fonts, app: &mut App) {
    use scenario_edit::collision_view::SurfaceClass;
    let Some(s) = app.session.selected_surface_data().cloned() else {
        return;
    };
    let f = ui.push_font(fonts.title);
    ui.text(format!(
        "{}  {}",
        icon::BOX,
        crate::viewport::surface_label(&s)
    ));
    f.pop();
    ui.text_disabled(match s.class {
        SurfaceClass::Drawn => "Drawn: a render face covers it",
        SurfaceClass::CollisionOnly => "Collision only: nothing drawn covers it",
    });
    if ui.button(format!("{} Frame", icon::FOCUS)) {
        app.dispatch(Action::FocusSelected);
    }
    theme::section(ui, "Surface");
    let p = s.plane;
    for (label, value) in [
        (
            "Plane",
            format!("{:.3}, {:.3}, {:.3}, d {:.3}", p[0], p[1], p[2], p[3]),
        ),
        ("Covered", format!("{:.0}%", s.covered * 100.0)),
        ("Flags", format!("0x{:02x}", s.flags)),
        (
            "Breakable surface",
            if s.breakable == 0xff {
                "none".to_owned()
            } else {
                s.breakable.to_string()
            },
        ),
        ("Material", s.material.to_string()),
        ("Area", format!("{:.2}", s.area())),
    ] {
        ui.label_text(label, value);
    }
    ui.text_disabled("Bit 3 of the flags is breakable; what the others stop is not established.");
    theme::section(ui, "Ring");
    for (i, q) in s.ring.iter().enumerate() {
        ui.text(format!("{i}: {:.3}, {:.3}, {:.3}", q[0], q[1], q[2]));
    }
}

/// The selected navigation thing, read only: a pathfinding sector's
/// flags and links, a bot polygon's links, a bot link's kind and ends, or
/// a spot's state and what the files say of it.
fn nav_pick(ui: &Ui, fonts: &theme::Fonts, app: &mut App, pick: scenario_edit::nav_view::NavPick) {
    use scenario_edit::nav_view::NavPick;
    let f = ui.push_font(fonts.title);
    ui.text(format!(
        "{}  {}",
        icon::BOX,
        crate::viewport::nav::label(app, pick)
    ));
    f.pop();
    if ui.button(format!("{} Frame", icon::FOCUS)) {
        app.dispatch(Action::FocusSelected);
    }
    let v3 = |p: [f32; 3]| format!("{:.3}, {:.3}, {:.3}", p[0], p[1], p[2]);
    let mut rows: Vec<(&str, String)> = Vec::new();
    let mut list: Vec<String> = Vec::new();
    let session = &app.session;
    match pick {
        NavPick::Sector(i) => {
            let Some(s) = session.pathfinding().and_then(|p| p.sectors.get(i)) else {
                return;
            };
            ui.text_disabled("Halo 2's own AI pathfinding: what the AI walks on.");
            rows.push(("Sector", s.sector.to_string()));
            rows.push(("Flags", format!("0x{:04x}", s.flags)));
            rows.push((
                "Hint",
                if s.hint == 0xffff {
                    "none".into()
                } else {
                    s.hint.to_string()
                },
            ));
            rows.push((
                "Floor",
                if s.floor() {
                    "yes"
                } else {
                    "no (a wall or ceiling)"
                }
                .into(),
            ));
            rows.push(("Links", s.links.len().to_string()));
            let p = session
                .pathfinding()
                .map(|p| p.links.as_slice())
                .unwrap_or_default();
            for &l in &s.links {
                let other = p
                    .iter()
                    .find(|k| k.bsp == s.bsp && k.link == l)
                    .and_then(|k| {
                        if k.left == Some(s.sector) {
                            k.right
                        } else {
                            k.left
                        }
                    });
                list.push(match other {
                    Some(o) => format!("link {l}: to sector {o}"),
                    None => format!("link {l}: edge"),
                });
            }
        }
        NavPick::BotPoly(i) => {
            let Some(b) = session.bot_nav() else { return };
            let Some(p) = b.polys.get(i) else { return };
            ui.text_disabled("The bots' own nav mesh polygon.");
            rows.push(("Polygon", p.poly.to_string()));
            rows.push(("Centre", v3(p.center())));
            rows.push(("Corners", p.verts.len().to_string()));
            rows.push((
                "In the map",
                if p.out_of_map {
                    "no: only a glitch reaches it"
                } else {
                    "yes"
                }
                .into(),
            ));
            for l in b.links.iter().filter(|l| l.from == i || l.to == i) {
                let (dir, other) = if l.from == i {
                    ("to", l.to)
                } else {
                    ("from", l.from)
                };
                list.push(format!(
                    "{} {dir} polygon {}",
                    l.kind.name(),
                    b.polys[other].poly
                ));
            }
        }
        NavPick::BotLink(i) => {
            let Some(b) = session.bot_nav() else { return };
            let Some(l) = b.links.get(i) else { return };
            rows.push(("Kind", l.kind.name().into()));
            rows.push(("From polygon", b.polys[l.from].poly.to_string()));
            rows.push(("To polygon", b.polys[l.to].poly.to_string()));
            rows.push(("One way", if l.one_way { "yes" } else { "no" }.into()));
            rows.push((
                if l.kind.traversal() {
                    "Takeoff"
                } else {
                    "Crossing from"
                },
                v3(l.a),
            ));
            rows.push((
                if l.kind.traversal() {
                    "Landing"
                } else {
                    "Crossing to"
                },
                v3(l.b),
            ));
        }
        NavPick::Spot(i) => {
            let Some(s) = session.bot_nav().and_then(|b| b.spots.get(i)) else {
                return;
            };
            rows.push(("Kind", s.kind().name().into()));
            rows.push(("Source", s.source.into()));
            rows.push(("State", s.state().name().into()));
            rows.push(("Stand", v3(s.stand)));
            rows.push(("Score", format!("{:.2}", s.score)));
            if s.kind() != scenario_edit::nav_view::SpotKind::Crouch {
                rows.push(("Launch", format!("{:.2}", s.launch)));
                rows.push(("Fall", format!("{:.2}", s.fall)));
            }
            if let Some(line) = s.line {
                rows.push((
                    "Learned",
                    format!("{line}: {} of {} tries bounced", s.hits, s.tries),
                ));
            }
            if s.trials > 0 {
                rows.push(("Verified", format!("{} of {} trials", s.ok, s.trials)));
            }
            rows.push(("Ghost run", if s.ghost { "kept" } else { "none" }.into()));
        }
    }
    theme::section(ui, "Navigation");
    for (label, value) in rows {
        ui.label_text(label, value);
    }
    if !list.is_empty() {
        theme::section(ui, "Links");
        for line in list {
            ui.text(line);
        }
    }
}

/// Draw the inspector for the selection and apply its edits.
pub fn draw(ui: &Ui, fonts: &theme::Fonts, app: &mut App, state: &mut InspectorState) {
    if let Some(index) = app.session.selected_volume() {
        volume(ui, fonts, app, index);
        return;
    }
    if app.session.selected_surface().is_some() {
        surface(ui, fonts, app);
        return;
    }
    if let Some(pick) = app.session.selected_nav() {
        nav_pick(ui, fonts, app, pick);
        return;
    }
    let location = app.nav_location();
    let (kind, index) = match location.selection() {
        Some(Selection::Placement(kind, index)) => (kind, index),
        Some(Selection::Tag(_)) => {
            tags(ui, app, state, &location, None);
            return;
        }
        None => {
            state.focus = None;
            state.tag_panel.clear_field_focus();
            let title = if app.session.is_open() {
                "Click something in the level"
            } else {
                "Open a map to inspect it"
            };
            theme::empty_card(ui, fonts, title, || {});
            return;
        }
    };
    let selection = app.session.selection();
    let Some(scenario) = app.session.scenario() else {
        return;
    };
    let f = ui.push_font(fonts.title);
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

    theme::link_badge(
        ui,
        crate::play::LinkTone::View,
        crate::play::tag_sync::Effect::NextSpawn.chip(),
    );
    ui.set_item_tooltip(
        "Creation fields apply when the object is next made. In Edit, live objects can also follow position and rotation.",
    );
    let fields = app.session.shared_fields();
    let mut edits: Vec<(&'static str, FieldEdit)> = Vec::new();
    let issues = issues(app);
    let mut resolution = None;
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
            theme::section(ui, h);
            heading = h;
        }
        let d = ui.begin_disabled_with_cond(!f.editable);
        if let Some(e) = field_row(ui, scenario, kind, f, first, &values, mixed, state) {
            edits.push((f.name, e));
        }
        done |= ui.is_item_deactivated_after_edit();
        d.end();
        for &(k, i) in &selection {
            for issue in issues.placement_field(k, i, f.name) {
                if selection.len() > 1 {
                    ui.text_disabled(scenario.label(k, i));
                }
                field_problem(ui, issue, &mut resolution);
            }
        }
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
    if let Some(action) = resolution {
        app.pairing.requests.push(Request::Apply(action));
    }
    // A netgame flag's area (a hill, territory or site), or Convert to area
    // for a retail hill's corners.
    if selection.len() == 1 && kind == BlockKind::NetgameFlags {
        area::rows(ui, app, index);
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
    if selection.len() == 1 || location.tag().is_some() {
        let root = app.session.scenario().and_then(|s| tag_api::placement_root(s, kind, index));
        tags(ui, app, state, &location, root);
    }
}

/// Route the field panel's commands to the open map, even when the separate
/// Tags tool has a read-only browse open.
pub(crate) fn tag_action(action: Action) -> Action {
    match action {
        Action::Tag(action) => Action::InspectorTag(action),
        action => action,
    }
}

fn tags(
    ui: &Ui,
    app: &mut App,
    state: &mut InspectorState,
    location: &NavLocation,
    placement_root: Option<(TagId, &'static str)>,
) {
    let root = location.tag().map(|id| (id, "")).or(placement_root);
    let Some((tag, via)) = root else {
        state.focus = None;
        state.tag_panel.clear_field_focus();
        return;
    };
    app.sync_tags();
    theme::section(ui, "Tags");
    let doc = &app.tags;
    if let Some(error) = &doc.error {
        ui.text_colored(theme::ERROR, error);
        return;
    }
    if state.tag_world != doc.for_rev {
        state.tag_world = doc.for_rev;
        state.tag_panel = fields::PanelState::default();
        state.focus = None;
    }
    let focus = location.tag().zip(location.path.clone());
    if state.focus != focus {
        state.tag_panel.clear_field_focus();
        if let Some((tag, path)) = &focus
            && let Ok(field) = tag_api::field_focus(doc, *tag, path)
        {
            state.tag_panel.focus_field(*tag, field);
        }
        state.focus = focus;
    }
    let mut actions = Vec::new();
    let mut resolution = None;
    let issues = issues(app);
    state.tag_panel.follow(app);
    breadcrumbs(ui, app, location, &mut actions);
    // The tag's document is the stack, as for every editor (docs/editor/
    // studio-flow.md, "Shared patterns"): its own writes and its editor's
    // steps, taken newest first.
    let can = app.doc_can(Doc::Tag(tag));
    let undo = ui.begin_disabled_with_cond(!can.0);
    if ui.button("Undo tag edit") {
        actions.push(Action::UndoDoc(Doc::Tag(tag)));
    }
    undo.end();
    ui.same_line();
    let redo = ui.begin_disabled_with_cond(!can.1);
    if ui.button("Redo tag edit") {
        actions.push(Action::RedoDoc(Doc::Tag(tag)));
    }
    redo.end();
    let rev = doc.rev();
    if !state.chain.as_ref().is_some_and(|(r, t, v, _)| (*r, *t, *v) == (rev, tag, via)) {
        state.chain = Some((rev, tag, via, tag_api::chain(doc, root, tag_api::DEFAULT_DEPTH)));
    }
    // The where-used answers for every node of the chain, cached by the
    // document's revision and the scenario's block layout.
    let scenario = app.session.scenario();
    let scenario_key = scenario.map(|s| {
        s.blocks
            .iter()
            .map(|b| b.layout_rev().wrapping_mul(31).wrapping_add(b.len() as u64))
            .fold(0u64, |a, x| a.wrapping_mul(31).wrapping_add(x))
    });
    let key = (rev, scenario_key.unwrap_or(0));
    if !state.used.as_ref().is_some_and(|(r, s, _)| (*r, *s) == key) {
        state.used = Some((key.0, key.1, HashMap::new()));
    }
    if let Some((_, _, _, chain)) = &state.chain {
        if let Some(node) = &chain.root {
            let mut asked = Vec::new();
            collect_tags(node, &mut asked);
            if let Some((_, _, answers)) = &mut state.used {
                for tag in asked {
                    answers
                        .entry(tag)
                        .or_insert_with(|| tag_api::uses(doc, scenario, tag));
                }
            }
            let mut trail = location.trail.clone();
            // tag_node appends its own ID.
            trail.pop();
            if state.tag_panel.has_field_focus(tag) {
                ui.set_next_item_open(true);
            }
            let used = state.used.as_ref().map(|(_, _, m)| m).unwrap();
            tag_node(
                ui, doc, node, &mut trail, &mut state.tag_panel, used, &mut actions, issues,
                &mut resolution,
            );
        }
        if chain.truncated {
            ui.text_disabled("Tag chain limited to the first references.");
        }
    }
    if let Some(action) = resolution {
        app.pairing.requests.push(Request::Apply(action));
    }
    state.tag_panel.store(app);
    for action in actions {
        app.dispatch(tag_action(action));
    }
}

fn breadcrumbs(ui: &Ui, app: &App, location: &NavLocation, actions: &mut Vec<Action>) {
    for (label, forward, action) in [
        ("Back", false, NavAction::Back),
        ("Forward", true, NavAction::Forward),
    ] {
        let _disabled = ui.begin_disabled_with_cond(!app.can_navigate(forward));
        if ui.small_button(label) {
            actions.push(Action::Navigate(action));
        }
        ui.same_line();
    }
    if location.trail.is_empty() {
        ui.new_line();
        return;
    }
    if matches!(location.selection(), Some(Selection::Placement(..))) {
        if ui.small_button("Placement") {
            actions.push(Action::Navigate(NavAction::Open(Vec::new())));
        }
        ui.same_line();
    }
    for (i, &tag) in location.trail.iter().enumerate() {
        let _id = ui.push_id(i);
        let label = app.tags.tag(tag).map_or_else(
            || format!("{:#010x}", tag.0),
            |t| {
                let name = t.name.rsplit('\\').next().unwrap_or(&t.name);
                format!("{name}.{}", tags::defs::fourcc_text(t.group))
            },
        );
        if ui.small_button(label) {
            actions.push(Action::Navigate(NavAction::Open(location.trail[..=i].to_vec())));
        }
        if i + 1 < location.trail.len() {
            ui.same_line();
            ui.text_disabled(">");
            ui.same_line();
        }
    }
    if let Some(path) = &location.path {
        ui.text_disabled(path);
    }
}

fn tag_node(
    ui: &Ui,
    doc: &TagDoc,
    node: &Node,
    trail: &mut Vec<TagId>,
    panel: &mut fields::PanelState,
    used: &HashMap<TagId, tag_api::Uses>,
    actions: &mut Vec<Action>,
    issues: &IssueIndex,
    resolution: &mut Option<Apply>,
) {
    let _id = ui.push_id(node.via.as_str());
    let id = node.tag;
    let Some(t) = doc.tag(id) else {
        ui.text_disabled(format!("{} references missing tag {:#010x}", node.via, id.0));
        return;
    };
    let label = doc.tag_label(id.0);
    let label = if t.elsewhere.is_some() {
        format!("{label} (in shared)")
    } else {
        label
    };
    let label = match node.shortcut {
        Some(role) => format!("{role} · {label}"),
        None => label,
    };
    let issue = issues.tag_row(id);
    let item = marked_row(ui, label, issue, |label| {
        ui.tree_node_config(format!("tag{:08x}", id.0))
            .label(label)
            .default_open(node.via.is_empty())
            .span_avail_width(true)
            .push()
    });
    if issue.is_none() && !node.via.is_empty() && ui.is_item_hovered() {
        ui.set_item_tooltip(&node.via);
    }
    let Some(_node) = item else { return };
    match &t.access {
        Access::Edit => {
            // What an edit to it changes: the count of its uses.
            ui.text_disabled(used.get(&id).map_or_else(
                || "Edits affect every placement and tag that uses this tag.".to_owned(),
                shared_words,
            ));
        }
        Access::Read(why) | Access::None(why) => ui.text_disabled(why),
    }
    if let Some(cache) = &t.elsewhere {
        if ui.small_button(format!("Show in {cache}")) {
            actions.push(Action::BrowseSharedTag(id));
        }
        return;
    }
    trail.push(id);
    if !node.via.is_empty() && ui.small_button("Focus tag") {
        let end = trail.iter().position(|&tag| tag == id).unwrap() + 1;
        actions.push(Action::Navigate(NavAction::Open(trail[..end].to_vec())));
    }
    if let Some(variant) = &node.variant {
        ui.text_disabled(format!("Variant {variant}"));
    }
    // Where this is used: the placements whose palette reaches the tag
    // and the tags that refer to it, each row a jump or a selection.
    if let Some(uses) = used.get(&id)
        && !(uses.tags.is_empty() && uses.placements.is_empty())
        && let Some(_list) = ui.tree_node("Where this is used")
    {
        for u in &uses.tags {
            let label = doc.tag_label(u.tag.0);
            if ui.small_button(format!("{label}##used{:08x}", u.tag.0)) {
                let mut to = trail.clone();
                to.push(u.tag);
                actions.push(Action::Navigate(NavAction::Open(to)));
            }
            if ui.is_item_hovered() {
                ui.set_item_tooltip(&u.via);
            }
        }
        for p in &uses.placements {
            let label = scenario_edit::layout::BLOCKS
                .iter()
                .find(|d| d.kind == p.kind)
                .map_or_else(|| format!("{:?}", p.kind), |d| d.label.to_string());
            if ui.small_button(format!("{label} {}##used", p.index)) {
                actions.push(Action::Select(Some((p.kind, p.index))));
            }
        }
    }
    if !matches!(t.access, Access::None(_))
        && doc.layout(id).is_some_and(|layout| layout.size <= t.size)
    {
        if let Some(problem) = &panel.problem {
            ui.text_colored(theme::ERROR, problem);
        }
        fields::panel_with_marks(
            ui,
            doc,
            id,
            &[],
            panel,
            actions,
            fields::PanelRoot::Inspector(trail),
            &mut tag_field_marks(ui, issues, resolution),
        );
        // The selected row's description (a click on a row selects it).
        if let Some(r) = panel.selected_row(doc, id)
            && let Some(text) = r.field.doc.as_deref().filter(|s| !s.is_empty())
        {
            ui.text(&r.label);
            ui.text_wrapped(text);
        }
    }
    for child in node.children.iter().filter(|child| child.shortcut.is_some()) {
        tag_node(ui, doc, child, trail, panel, used, actions, issues, resolution);
    }
    if node.children.iter().any(|child| child.shortcut.is_none())
        && let Some(_refs) = ui.tree_node("All references")
    {
        for child in node.children.iter().filter(|child| child.shortcut.is_none()) {
            tag_node(ui, doc, child, trail, panel, used, actions, issues, resolution);
        }
    }
    trail.pop();
}

/// The node header's count of a tag's uses, in the design's words
/// ("shared by 14 placements, 3 tags").
fn shared_words(uses: &tag_api::Uses) -> String {
    let p = uses.placements.len();
    let t = uses.tags.len();
    let mut parts = Vec::new();
    if p > 0 {
        parts.push(format!("{p} placement{}", if p == 1 { "" } else { "s" }));
    }
    if t > 0 {
        parts.push(format!("{t} tag{}", if t == 1 { "" } else { "s" }));
    }
    if parts.is_empty() {
        "Not used by anything else.".to_owned()
    } else {
        format!("Edits affect every use: shared by {}.", parts.join(", "))
    }
}

/// Every tag a chain node covers, itself and its children.
fn collect_tags(node: &Node, out: &mut Vec<TagId>) {
    out.push(node.tag);
    for child in &node.children {
        collect_tags(child, out);
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
                .then_some(FieldEdit::Set(Value::Float(s)));
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
            let options: Vec<(i64, String)> = (0..=18).map(|v| (v, text(v))).collect();
            let r = search_combo(ui, &id("flagtype"), &text(x), x, &options, state)
                .map(|v| FieldEdit::Set(Value::Int(v)));
            ui.set_item_tooltip("Unidentified flag types are shown by their raw value.");
            r
        }
        (_, Value::Int(mut x)) => {
            let text = match f.name {
                "respawn time" => "respawn time (s)",
                name => name,
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
                ("team", BlockKind::NetgameFlags) => ui.set_item_tooltip("Team 8 is neutral."),
                ("identifier", BlockKind::NetgameFlags) => {
                    ui.set_item_tooltip("Teleporter channel or territory index.")
                }
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
    fn conflict_buttons_follow_the_selected_placement() {
        let temp = crate::play::testing::Temp::new("inspector-conflict");
        let mut app = App::new();
        app.set_data_root(temp.0.join("data"));
        app.dispatch(Action::OpenSource(temp.map("example")));
        app.pairing.pending.push(serde_json::json!({
            "target": {"block": "netgame_flags", "index": 0},
            "field": "position", "at": 123, "conflict": true, "why": "the game changed the field"
        }));
        app.pairing.link = Some(crate::play::Play::new().link_view(&app).into());

        let _turn = crate::headless::ONE_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
        let (mut ctx, fonts) = crate::context(None);
        ctx.font_atlas().try_claim_legacy_renderer().unwrap().build();
        ctx.io_mut().set_display_size([600.0, 800.0]);
        ctx.io_mut().set_delta_time(1.0 / 60.0);
        let mut state = InspectorState::default();
        for (kind, marked) in [
            (BlockKind::NetgameFlags, true),
            (BlockKind::StartingLocations, false),
        ] {
            app.dispatch(Action::Select(Some((kind, 0))));
            let ui = ctx.frame();
            crate::probe::begin();
            ui.window("Placement inspector")
                .size([580.0, 780.0], dear_imgui_rs::Condition::Always)
                .build(|| draw(ui, &fonts, &mut app, &mut state));
            let items = crate::probe::items();
            for label in ["Take the game's value", "Overwrite"] {
                assert_eq!(
                    items.iter().any(|item| item.label == label),
                    marked,
                    "{kind:?} {label}"
                );
            }
            let _ = ctx.render_legacy();
        }
    }

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
        assert_eq!(flag_type_name(18), Some("Hill marker"));
        assert_eq!(flag_type_name(19), None);
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
