//! The Modeling section of the map inspector (docs/editor/modeling.md):
//! pick a cluster or instanced definition, open it in edit mode, select
//! and move its vertices with undo, and add a collision box to its
//! structure's level collision. The controls share Modeling and tag
//! commands with the agent API. Undo and Redo use the map document.

use dear_imgui_rs::{TreeNodeFlags, Ui};
use geometry_edit::{Target, TargetInfo, TargetKind};
use geometry_edit::mesh_edit::{Brush, Falloff, MeshOp, SelectMode};

use crate::App;
use crate::app::Action;
use crate::app::Doc;
use crate::app::modeling::{BrushSettings, ModelAction};
use crate::theme;

mod physics;

/// What the section keeps between frames.
pub struct ModelingPanel {
    /// The open map's targets (`World rev`, list), read when the section
    /// first opens on a map.
    targets: Option<(u64, Vec<TargetInfo>)>,
    picked: usize,
    physics: physics::Panel,
    collision_source: Option<Target>,
    delta: [f32; 3],
    /// The collision box's corners, world units.
    box_lo: [f32; 3],
    box_hi: [f32; 3],
    /// `Bevel`'s width.
    bevel_width: f32,
    /// `Inset`'s thickness, depth, and whether each face goes alone.
    inset_thickness: f32,
    inset_depth: f32,
    inset_individual: bool,
    /// `TrisToQuads`' flatness, degrees.
    quad_angle: f32,
}

impl Default for ModelingPanel {
    fn default() -> Self {
        ModelingPanel {
            targets: None,
            picked: 0,
            physics: physics::Panel::default(),
            collision_source: None,
            delta: [0.0; 3],
            box_lo: [0.0; 3],
            box_hi: [0.0; 3],
            bevel_width: 0.1,
            inset_thickness: 0.1,
            inset_depth: 0.0,
            inset_individual: false,
            quad_angle: 1.0,
        }
    }
}

/// The section, under the inspector's fields.
pub fn section(ui: &Ui, app: &mut App, st: &mut ModelingPanel, actions: &mut Vec<Action>) {
    if !app.session.is_open() {
        return;
    }
    if !ui.collapsing_header("Modeling##modeling", TreeNodeFlags::NONE) {
        return;
    }
    st.physics.draw(ui, app, actions);
    let rev = app.session.world_rev();
    if st.targets.as_ref().is_none_or(|(r, _)| *r != rev) {
        let list = app.modeling_targets(None).unwrap_or_default();
        st.targets = Some((rev, list));
        st.picked = 0;
        st.collision_source = None;
    }
    let Some((_, targets)) = &st.targets else {
        return;
    };
    if targets.is_empty() {
        ui.text_disabled("This map has no editable geometry.");
        return;
    }
    let label = |t: &TargetInfo| {
        format!(
            "{} {} of {} ({} vertices)",
            t.target.kind.label(),
            t.target.index,
            t.tag_name,
            t.vertices
        )
    };
    let picked = st.picked.min(targets.len() - 1);
    // A picker, not arrows: the list opens on top.
    if let Some(_c) = ui.begin_combo("Target##modeling-target", label(&targets[picked])) {
        for (i, t) in targets.iter().enumerate() {
            if ui.selectable_config(label(t)).selected(i == picked).build() {
                st.picked = i;
            }
        }
    }
    let info = &targets[st.picked.min(targets.len() - 1)];
    let (t, local) = (info.target, info.local);
    if matches!(t.kind, TargetKind::Cluster | TargetKind::InstanceDef) {
        collision_box(ui, app, st, t.tag, actions);
    }
    let g = &app.modeling.session;
    let editing = g.active == Some(t);
    if !editing {
        if ui.button("Edit mode##modeling-open") {
            actions.push(Action::Model(ModelAction::Open(t)));
        }
        if !local {
            ui.same_line();
            ui.text_disabled("In a shared cache: shown, not saved.");
        }
        return;
    }
    if ui.button("Leave edit mode##modeling-close") {
        actions.push(Action::Model(ModelAction::Close(t)));
    }
    if t.kind == TargetKind::ModeSection {
        let mut on = g.uses_for_lower_lods(t);
        if ui.checkbox("Use for lower LODs##modeling-lower-lods", &mut on) {
            actions.push(Action::Model(ModelAction::UseForLowerLods { target: t, on }));
        }
    }
    if t.kind == TargetKind::Coll {
        let sources = g.open_targets();
        let label = |source: Target| g.name(source).map_or_else(
            || source.id(),
            |name| format!("{name} ({source})"),
        );
        let source = st.collision_source.filter(|s| sources.contains(s));
        if let Some(_combo) = ui.begin_combo(
            "Render section##collision-source",
            source.map_or_else(|| "Choose an open section".to_owned(), label),
        ) {
            for candidate in sources.into_iter().filter(|s| s.kind != TargetKind::Coll) {
                if ui.selectable_config(label(candidate)).selected(source == Some(candidate)).build() {
                    st.collision_source = Some(candidate);
                }
            }
        }
        let _disabled = ui.begin_disabled_with_cond(source.is_none());
        if ui.button("From render##collision-from-render") {
            if let Some(render) = source {
                actions.push(Action::Model(ModelAction::CollisionFromRender {
                    render,
                    target: t,
                    materials: Vec::new(),
                }));
            }
        }
    }
    let apply = |ops: Vec<MeshOp>| {
        Action::Model(ModelAction::Apply {
            target: t,
            ops,
            merge: None,
        })
    };
    let selected = g.mesh(t).map_or(0, |m| m.selection().len());
    ui.text(format!("{selected} selected"));
    if ui.button("Select all##modeling-all") {
        actions.push(apply(vec![
            MeshOp::Mode {
                mode: SelectMode::Vert,
            },
            MeshOp::SelectAll,
        ]));
    }
    ui.same_line();
    if ui.button("Select none##modeling-none") {
        actions.push(apply(vec![MeshOp::SelectNone]));
    }
    for (axis, name) in ["x", "y", "z"].iter().enumerate() {
        let mut v = st.delta[axis];
        if ui.input_float(format!("Move {name}##modeling-delta-{name}"), &mut v) && v.is_finite() {
            st.delta[axis] = v;
        }
    }
    let finite = st.delta.iter().all(|v| v.is_finite());
    if ui.button("Move selection##modeling-move") && finite && selected > 0 {
        let d = st.delta;
        actions.push(apply(vec![MeshOp::Transform {
            matrix: [
                [1.0, 0.0, 0.0, d[0]],
                [0.0, 1.0, 0.0, d[1]],
                [0.0, 0.0, 1.0, d[2]],
            ],
            pivot: None,
        }]));
    }
    ui.separator();
    ui.text("Edge and face tools");
    let mut width = st.bevel_width;
    if ui.input_float("Bevel width##modeling-bevel-width", &mut width) && width.is_finite() {
        st.bevel_width = width;
    }
    if ui.button("Bevel selected edges##modeling-bevel") && st.bevel_width > 0.0 && selected > 0 {
        actions.push(apply(vec![MeshOp::Bevel { width: st.bevel_width }]));
    }
    let mut thickness = st.inset_thickness;
    if ui.input_float("Inset thickness##modeling-inset-thickness", &mut thickness) && thickness.is_finite() {
        st.inset_thickness = thickness;
    }
    let mut depth = st.inset_depth;
    if ui.input_float("Inset depth##modeling-inset-depth", &mut depth) && depth.is_finite() {
        st.inset_depth = depth;
    }
    ui.checkbox("Each face alone##modeling-inset-individual", &mut st.inset_individual);
    if ui.button("Inset selected faces##modeling-inset") && st.inset_thickness >= 0.0 && selected > 0 {
        actions.push(apply(vec![MeshOp::Inset {
            thickness: st.inset_thickness,
            depth: st.inset_depth,
            individual: st.inset_individual,
        }]));
    }
    let mut angle = st.quad_angle;
    if ui.input_float("Flat within (degrees)##modeling-quad-angle", &mut angle) && angle.is_finite() {
        st.quad_angle = angle.clamp(0.0, 180.0);
    }
    if ui.button("Triangles to quads##modeling-quads") && selected > 0 {
        actions.push(apply(vec![MeshOp::TrisToQuads { angle: st.quad_angle }]));
    }
    ui.separator();
    sculpt(ui, &mut app.modeling.brush, t.kind);
    ui.separator();
    // The map document's own stack, as Cmd+Z takes it in edit mode: every
    // editor's steps are steps of its document (docs/editor/studio-flow.md,
    // "Shared patterns").
    let (can_undo, can_redo) = app.doc_can(Doc::Map);
    let off = ui.begin_disabled_with_cond(!can_undo);
    if ui.button("Undo##modeling-undo") {
        actions.push(Action::UndoDoc(Doc::Map));
    }
    drop(off);
    ui.same_line();
    let off = ui.begin_disabled_with_cond(!can_redo);
    if ui.button("Redo##modeling-redo") {
        actions.push(Action::RedoDoc(Doc::Map));
    }
    drop(off);
    for c in g.check(Some(t)) {
        ui.text(format!(
            "{} moved, {} faces' lightmaps stale: {}",
            c.moved,
            c.lightmap_stale,
            c.class.name().replace('_', " ")
        ));
        if let Some(why) = &c.refused {
            ui.text_colored(theme::ERROR, why);
        }
    }
    if let Some(why) = app.geometry_save_refusal() {
        ui.text_wrapped(format!("Save refused: {why}"));
    }
}

/// The structure's level collision: a box solid between two corners
/// added to it (`ModelAction::AddCollisionBox`), and the edit cleared.
/// A paired game gets each change live (docs/editor/realtime-edits.md,
/// "Collision").
fn collision_box(ui: &Ui, app: &App, st: &mut ModelingPanel, structure: map::TagId, actions: &mut Vec<Action>) {
    ui.separator();
    ui.text("Level collision");
    for (name, corner) in [("From", &mut st.box_lo), ("To", &mut st.box_hi)] {
        for (axis, a) in ["x", "y", "z"].iter().enumerate() {
            let mut v = corner[axis];
            if ui.input_float(format!("{name} {a}##modeling-box-{name}-{a}"), &mut v) && v.is_finite() {
                corner[axis] = v;
            }
        }
    }
    let (lo, hi) = (st.box_lo, st.box_hi);
    let solid = (0..3).all(|i| hi[i] > lo[i]);
    if ui.button("Add collision box##modeling-box-add") && solid {
        actions.push(Action::Model(ModelAction::AddCollisionBox {
            structure,
            lo,
            hi,
            material: 0,
        }));
    }
    if !solid {
        ui.same_line();
        ui.text_disabled("Each To must be above its From.");
    }
    if let Some(edit) = app.modeling.level_collision.get(&structure) {
        ui.text(format!(
            "{} collision surfaces ({} in the file)",
            edit.added.level.bsp.surfaces.len(),
            edit.source.level.bsp.surfaces.len()
        ));
        if ui.button("Clear collision boxes##modeling-box-clear") {
            actions.push(Action::Model(ModelAction::ClearCollision { structure }));
        }
    }
    ui.separator();
}

/// The sculpt brush's settings (docs/editor/modeling.md, "Sculpting"):
/// on, it turns a left drag in the viewport into a stroke
/// (`viewport::sculpt`). Brush and falloff are pickers.
fn sculpt(ui: &Ui, b: &mut BrushSettings, kind: TargetKind) {
    ui.text("Sculpt");
    ui.checkbox("Brush in the viewport##sculpt-on", &mut b.on);
    if b.on && kind != TargetKind::Cluster {
        ui.text_disabled("The viewport brush draws on structure clusters; mesh.sculpt takes any target.");
    }
    if let Some(_c) = ui.begin_combo("Brush##sculpt-brush", b.brush.name()) {
        for brush in Brush::ALL {
            if ui.selectable_config(brush.name()).selected(brush == b.brush).build() {
                b.brush = brush;
                if brush.fractional() {
                    b.strength = b.strength.min(1.0);
                }
            }
        }
    }
    let mut radius = b.radius;
    if ui.input_float("Radius##sculpt-radius", &mut radius) && radius.is_finite() && radius > 0.0 {
        b.radius = radius;
    }
    if b.brush != Brush::Cut {
        let label = if b.brush.fractional() { "Strength (0 to 1)##sculpt-strength" } else { "Strength (world units)##sculpt-strength" };
        let mut strength = b.strength;
        if ui.input_float(label, &mut strength) && strength.is_finite() && strength > 0.0 {
            b.strength = if b.brush.fractional() { strength.min(1.0) } else { strength };
        }
    }
    if let Some(_c) = ui.begin_combo("Falloff##sculpt-falloff", b.falloff.name()) {
        for falloff in Falloff::ALL {
            if ui.selectable_config(falloff.name()).selected(falloff == b.falloff).build() {
                b.falloff = falloff;
            }
        }
    }
    if b.brush == Brush::Flatten {
        let mut set = b.height.is_some();
        if ui.checkbox("Flatten to a set height##sculpt-height-on", &mut set) {
            b.height = set.then_some(0.0);
        }
        if let Some(h) = &mut b.height {
            let mut v = *h;
            if ui.input_float("Height##sculpt-height", &mut v) && v.is_finite() {
                *h = v;
            }
        } else {
            ui.text_disabled("The stroke's first point sets the height.");
        }
    }
    let mut detail = b.detail.unwrap_or(b.radius / 4.0);
    if ui.input_float("Detail (longest edge)##sculpt-detail", &mut detail) && detail.is_finite() && detail > 0.0 {
        b.detail = Some(detail);
    }
    ui.text_disabled("Drag over the mesh: the stroke applies when the button lifts, one undo step.");
}
